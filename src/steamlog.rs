// SPDX-FileCopyrightText: 2026 Pascal Klose
// SPDX-License-Identifier: GPL-3.0-or-later

//! Read a Proton log (steam-<gameid>.log) without drowning in it.
//!
//! A log written with PROTON_LOG=1 is mostly Wine tracing - exception unwinding, DLL loads - and the
//! few lines that matter are what the game itself printed (debugstr) and what Wine reports as an
//! error. This keeps those, folds repeats into one line with a count, and says what ran.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::LazyLock;

use regex::{Regex, RegexBuilder};

use crate::steam::{Result, home, truncate};

static LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?P<time>\d+\.\d+):[0-9a-f]+:[0-9a-f]+:(?P<level>\w+):(?P<channel>\w+):(?P<text>.*)$").unwrap());
/// err: lines Wine prints on every run under Proton that say nothing about the game.
static ROUTINE_ERRORS: LazyLock<Regex> = LazyLock::new(|| {
    RegexBuilder::new("openxr|winebth|hid:|LoadLibraryShim|steamclient_init_registry|IRpcStubBuffer")
        .case_insensitive(true)
        .build()
        .unwrap()
});
static DEBUG_PREFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"^OutputDebugString[AW] L?""#).unwrap());
static INNER_TIME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[\d{4}/\d{6}\.\d+:").unwrap());

const HEADER_KEYS: [&str; 4] = ["Proton:", "SteamGameId:", "Command:", "Kernel:"];

/// Counts that list most frequent first, ties in first-seen order.
#[derive(Default)]
struct Counter {
    order: Vec<String>,
    counts: HashMap<String, usize>,
}

impl Counter {
    fn add(&mut self, key: String) {
        if let Some(count) = self.counts.get_mut(&key) {
            *count += 1;
        } else {
            self.order.push(key.clone());
            self.counts.insert(key, 1);
        }
    }

    fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    fn most_common(&self) -> Vec<(&String, usize)> {
        let mut rows: Vec<_> = self.order.iter().map(|key| (key, self.counts[key])).collect();
        rows.sort_by_key(|row| std::cmp::Reverse(row.1));

        rows
    }
}

fn newest_log() -> Result<PathBuf> {
    fs::read_dir(home())?
        .flatten()
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.starts_with("steam-") && name.ends_with(".log")
        })
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .max()
        .map(|(_, path)| path)
        .ok_or_else(|| "no ~/steam-*.log found - start the game with PROTON_LOG=1 %command% as launch option".into())
}

/// What the game printed, without Wine's quoting and the timestamps inside it.
fn message(text: &str) -> String {
    let text = DEBUG_PREFIX.replace(text, "");
    let text = text.trim_end_matches('"').replace("\\n", " ").replace("\\r", " ");

    INNER_TIME.replace_all(text.trim(), "[").into_owned()
}

pub fn run(log: Option<PathBuf>, grep: Option<String>, all_errors: bool) -> Result<()> {
    let path = match log {
        Some(path) => path,
        None => newest_log()?,
    };
    let pattern = grep
        .map(|text| RegexBuilder::new(&text).case_insensitive(true).build())
        .transpose()?;

    let mut header = Vec::new();
    let mut channels = Counter::default();
    let (mut game_output, mut errors, mut other) = (Counter::default(), Counter::default(), Counter::default());
    let (mut first, mut last): (Option<f64>, f64) = (None, 0.0);
    let mut lines = 0;
    let mut noisy = 0;

    for raw in BufReader::new(File::open(&path)?).split(b'\n') {
        let raw = String::from_utf8_lossy(&raw?).into_owned();
        let line = raw.as_str();

        if HEADER_KEYS.iter().any(|key| line.starts_with(key)) {
            header.push(line.to_string());
            continue;
        }

        let Some(found) = LINE.captures(line) else {
            if pattern.as_ref().is_some_and(|p| p.is_match(line)) {
                other.add(truncate(line.trim(), 300));
            }
            continue;
        };

        let seconds: f64 = found["time"].parse().unwrap_or(0.0);
        first.get_or_insert(seconds);
        last = seconds;
        let (level, channel, text) = (&found["level"], &found["channel"], &found["text"]);
        channels.add(format!("{level}:{channel}"));
        lines += 1;

        if matches!(channel, "unwind" | "seh" | "loaddll") {
            noisy += 1;
        }

        if pattern.as_ref().is_some_and(|p| !p.is_match(text)) {
            continue;
        }

        if channel == "debugstr" || (channel == "seh" && text.contains("OutputDebugString")) {
            game_output.add(truncate(&message(text), 300));
        } else if level == "err" && (all_errors || !ROUTINE_ERRORS.is_match(text)) {
            errors.add(format!("{channel}: {}", truncate(text.trim(), 280)));
        } else if pattern.is_some() && !matches!(channel, "unwind" | "seh" | "loaddll") {
            other.add(format!("{level}:{channel}: {}", truncate(text.trim(), 280)));
        }
    }

    let megabytes = fs::metadata(&path)?.len() as f64 / 1_048_576.0;
    let runtime = first.map(|first| format!(", {:.0} s of runtime", last - first)).unwrap_or_default();
    println!("{}  ({megabytes:.1} MB{runtime})", path.display());

    for line in &header {
        println!("  {line}");
    }

    println!("\n{lines} Wine lines, {noisy} of them exception/loader tracing (skipped)");

    for (title, counter) in [
        ("What the game printed", &game_output),
        ("Wine errors", &errors),
        ("Other matching lines", &other),
    ] {
        if counter.is_empty() {
            continue;
        }

        println!("\n{title}:");

        for (text, count) in counter.most_common() {
            println!("  {count:5}x  {text}");
        }
    }

    if game_output.is_empty() && errors.is_empty() && other.is_empty() {
        println!("\nnothing matched");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::message;

    #[test]
    fn message_strips_wine_quoting() {
        assert_eq!(message(r#"OutputDebugStringA L"[2026/091500.123: hello\n""#), "[ hello");
    }
}
