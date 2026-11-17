// SPDX-FileCopyrightText: 2026 Pascal Klose
// SPDX-License-Identifier: GPL-3.0-or-later

//! Find out whether a host is blocked, and by whom: your own resolver, or a filtering upstream behind it.
//!
//! The resolver this machine uses is asked first - the system's, or the one `ISTAR_GAMESCRIPTS_DNS` names (a
//! Pi-hole or AdGuard Home, say). A filtering resolver such as DNS4EU's protective one answers a blocked
//! domain with the address of a block page instead (51.15.69.11 in September 2026, TTL 1) - a
//! normal-looking answer that a local resolver passes on as if nothing happened. The only way to see it is
//! to ask DNS4EU's unfiltered resolver and a public one the same question and compare.
//!
//! Queries go one at a time with a short pause: a local resolver may rate-limit a client, and a burst of
//! parallel queries comes back as timeouts that look exactly like a block.

use std::collections::BTreeSet;
use std::fs;
use std::process::Command;
use std::sync::LazyLock;
use std::time::Duration;

use regex::RegexBuilder;
use regex::bytes::Regex as BytesRegex;
use walkdir::WalkDir;

use crate::steam::*;

const NULL_ANSWERS: [&str; 3] = ["0.0.0.0", "127.0.0.1", "::"];
const PAUSE: Duration = Duration::from_millis(300);
static HOST: LazyLock<BytesRegex> = LazyLock::new(|| {
    BytesRegex::new(r"(?-u)\b(?:[a-z0-9](?:[a-z0-9\-]{0,61}[a-z0-9])?\.)+(?:com|net|org|cn|io|gg|jp|eu|de|co|cloud|app)\b").unwrap()
});

pub struct Answer {
    status: String,
    addresses: Vec<String>,
    ttl: Option<u32>,
}

/// local (the system's resolver unless ISTAR_GAMESCRIPTS_DNS names one), dns4eu (protective), dns4eu-unfiltered,
/// cloudflare. An empty address means the system's resolver.
fn resolvers() -> [(&'static str, String); 4] {
    [
        ("local", std::env::var("ISTAR_GAMESCRIPTS_DNS").unwrap_or_default()),
        ("dns4eu", "86.54.11.1".into()),
        ("dns4eu-unfiltered", "86.54.11.100".into()),
        ("cloudflare", "1.1.1.1".into()),
    ]
}

fn query(server: &str, host: &str) -> Answer {
    let at = format!("@{server}");
    let mut command = Command::new("dig");

    if !server.is_empty() {
        command.arg(&at);
    }

    let output = command
        .args([host, "A", "+time=3", "+tries=2", "+noall", "+answer", "+comments"])
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .unwrap_or_default();
    let status = output
        .lines()
        .find_map(|line| line.split("status: ").nth(1))
        .map_or("TIMEOUT", |rest| rest.split(',').next().unwrap_or("TIMEOUT"))
        .to_string();
    let records: Vec<Vec<&str>> = output
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with(';'))
        .map(|line| line.split_whitespace().collect::<Vec<_>>())
        .filter(|fields| fields.len() >= 5 && fields[3] == "A")
        .collect();
    let mut addresses: Vec<String> = records.iter().map(|fields| fields[4].to_string()).collect();
    addresses.sort();

    Answer {
        status,
        addresses,
        ttl: records.iter().filter_map(|fields| fields[1].parse().ok()).min(),
    }
}

fn verdict(local: &Answer, dns4eu: &Answer, unfiltered: &Answer, public: &Answer) -> String {
    let usable = |answer: &Answer| answer.status == "NOERROR" && !answer.addresses.is_empty();

    if !usable(unfiltered) && !usable(public) {
        return if unfiltered.status == "NXDOMAIN" || public.status == "NXDOMAIN" {
            "does not exist"
        } else {
            "unresolvable everywhere"
        }
        .into();
    }

    if local.status == "TIMEOUT" {
        return "your resolver timed out (rate limit? try again)".into();
    }

    if local.status != "NOERROR" || local.addresses.is_empty() || local.addresses.iter().any(|a| NULL_ANSWERS.contains(&a.as_str())) {
        return "BLOCKED by your resolver (its own lists)".into();
    }

    // The filtered and unfiltered DNS4EU answers differ, and the local resolver hands out the filtered one.
    let dns4eu_blocks = dns4eu.addresses != unfiltered.addresses && !dns4eu.addresses.iter().any(|a| public.addresses.contains(a));

    if dns4eu_blocks && local.addresses == dns4eu.addresses {
        return format!(
            "BLOCKED by DNS4EU (block page {}, TTL {})",
            dns4eu.addresses.join(" "),
            dns4eu.ttl.map_or("None".into(), |t| t.to_string())
        );
    }

    if dns4eu_blocks {
        "ok (DNS4EU filters it, your resolver goes around)"
    } else {
        "ok"
    }
    .into()
}

fn hosts_in(data: &[u8], wanted: &regex::Regex) -> BTreeSet<String> {
    HOST.find_iter(&data.to_ascii_lowercase())
        .map(|found| String::from_utf8_lossy(found.as_bytes()).into_owned())
        .filter(|host| wanted.is_match(host))
        .collect()
}

fn hosts_in_prefix(appid: &str, wanted: &regex::Regex) -> Result<BTreeSet<String>> {
    let root = prefix_path(appid)?;
    let drive = root.join("pfx/drive_c");
    let mut found = BTreeSet::new();
    let walker = WalkDir::new(&drive).into_iter().filter_entry(|entry| {
        let path = format!("{}/", entry.path().display());
        !entry.file_type().is_dir() || !(path.contains("/windows/") || path.contains("/ShaderBlob/"))
    });

    for entry in walker.flatten().filter(|e| !e.file_type().is_dir()) {
        if fs::metadata(entry.path()).is_ok_and(|m| m.len() <= 200 * 1024 * 1024)
            && let Ok(data) = fs::read(entry.path())
        {
            found.extend(hosts_in(&data, wanted));
        }
    }

    Ok(found)
}

pub struct Options {
    pub hosts: Vec<String>,
    pub from_prefix: Option<String>,
    pub from_log: Option<String>,
    pub from_file: Option<String>,
    pub matching: Option<String>,
    pub all: bool,
}

pub fn run(options: Options) -> Result<()> {
    let mut hosts = options.hosts.clone();
    let wanted = |flag: &str, example: &str| -> Result<regex::Regex> {
        let text = options
            .matching
            .as_deref()
            .ok_or(format!("{flag} needs --match, e.g. --match '{example}'"))?;
        Ok(RegexBuilder::new(text).case_insensitive(true).build()?)
    };

    if let Some(file) = &options.from_file {
        hosts.extend(
            read_text(file.as_ref())
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .map(String::from),
        );
    }

    if let Some(log) = &options.from_log {
        let wanted = wanted("--from-log", "launcher|cdn")?;
        hosts.extend(hosts_in(&fs::read(log)?, &wanted));
    }

    if let Some(appid) = &options.from_prefix {
        let wanted = wanted("--from-prefix", "launcher|cdn")?;
        hosts.extend(hosts_in_prefix(appid, &wanted)?);
    }

    if hosts.is_empty() {
        return Err("no hosts - name some, or use --from-prefix / --from-log / --from-file".into());
    }

    let servers = resolvers();
    let described: Vec<String> = servers
        .iter()
        .map(|(name, address)| format!("{name} {}", if address.is_empty() { "(system)" } else { address }))
        .collect();
    println!("{} host(s), asking {}\n", hosts.len(), described.join(", "));

    let mut counts: Vec<(String, usize)> = Vec::new();
    let mut seen = BTreeSet::new();

    for host in hosts.iter().filter(|host| seen.insert(host.as_str())) {
        let mut answers = servers.iter().map(|(_, address)| {
            let answer = query(address, host);
            std::thread::sleep(PAUSE);
            answer
        });
        let (local, dns4eu, unfiltered, public) = (
            answers.next().unwrap(),
            answers.next().unwrap(),
            answers.next().unwrap(),
            answers.next().unwrap(),
        );
        let result = verdict(&local, &dns4eu, &unfiltered, &public);
        let label = result.split(" (").next().unwrap_or("").to_string();

        match counts.iter_mut().find(|(name, _)| *name == label) {
            Some((_, count)) => *count += 1,
            None => counts.push((label, 1)),
        }

        if result != "ok" || options.all {
            let shown = if local.addresses.is_empty() {
                local.status.clone()
            } else {
                local.addresses.iter().take(2).cloned().collect::<Vec<_>>().join(" ")
            };
            println!("  {host:50} {result}   [your resolver: {shown}]");
        }
    }

    let summary: Vec<String> = counts.iter().map(|(name, count)| format!("{count} {name}")).collect();
    println!("\n{}", summary.join(", "));

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(status: &str, addresses: &[&str]) -> Answer {
        Answer {
            status: status.into(),
            addresses: addresses.iter().map(|a| a.to_string()).collect(),
            ttl: Some(1),
        }
    }

    #[test]
    fn dns4eu_block_page_is_told_apart_from_the_local_resolver() {
        let page = answer("NOERROR", &["51.15.69.11"]);
        let real = answer("NOERROR", &["1.2.3.4"]);

        assert!(verdict(&page, &page, &real, &real).starts_with("BLOCKED by DNS4EU"));
        assert!(verdict(&answer("NOERROR", &["0.0.0.0"]), &page, &real, &real).starts_with("BLOCKED by your resolver"));
        assert_eq!(verdict(&real, &real, &real, &real), "ok");
    }
}
