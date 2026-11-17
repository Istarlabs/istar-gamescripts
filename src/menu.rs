// SPDX-FileCopyrightText: 2026 Pascal Klose
// SPDX-License-Identifier: GPL-3.0-or-later

//! What the Dolphin menu calls: runs one of the commands and shows its output in a kurodlg window.
//!
//! Started from Dolphin there is no terminal, so the output goes into `kurodlg --textbox`, which has
//! a Copy button for exactly this. The window opens at once and fills as the command writes -
//! dnscheck takes a while. Without kurodlg it falls back to a Konsole that stays open.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::steam::*;

fn kurodlg() -> Option<PathBuf> {
    let path = std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|d| d.join("kurodlg")).find(|p| p.exists()));

    path.or_else(|| Some(home().join(".local/bin/kurodlg")).filter(|p| p.exists()))
}

/// English text, or German when LANG says so.
fn text(key: &str) -> &'static str {
    let german = std::env::var("LANG").is_ok_and(|lang| lang.starts_with("de"));
    let (english, deutsch) = match key {
        "steamlog" => ("Proton log", "Proton-Log"),
        "whereis" => ("Steam game of this folder", "Steam-Spiel dieses Ordners"),
        "saves" => ("Save folders", "Speicherstände"),
        "status" => ("Game and machine status", "Spiel- und Systemstatus"),
        "prefix-logs" => ("Recent logs in the prefix", "Neue Logs im Prefix"),
        "prefix-ids" => ("Device identity", "Geräte-Identität"),
        "dnscheck" => ("Blocked hosts", "Gesperrte Hosts"),
        "not-a-prefix" => (
            "{} is not inside a Wine prefix - pick a game's compatdata folder.",
            "{} liegt in keinem Wine-Prefix - wähle den compatdata-Ordner eines Spiels.",
        ),
        "ask-match" => (
            "Host names containing (text or regular expression):",
            "Hostnamen, die das enthalten (Text oder regulärer Ausdruck):",
        ),
        _ => ("Check hosts from the game's files", "Hosts aus den Spieldateien prüfen"),
    };

    if german { deutsch } else { english }
}

/// Runs this binary with `arguments` and streams what it prints into a kurodlg text window.
fn show(arguments: &[&str], title: &str, header: &str) -> Result<()> {
    let program = std::env::current_exe()?;

    let Some(viewer) = kurodlg() else {
        Command::new("konsole")
            .args(["--hold", "-e"])
            .arg(&program)
            .args(arguments)
            .status()?;
        return Ok(());
    };

    // stderr goes into the same pipe, so a failure shows up in the window too.
    let mut producer = Command::new("sh")
        .args(["-c", "exec \"$@\" 2>&1", "sh"])
        .arg(&program)
        .args(arguments)
        .stdout(Stdio::piped())
        .spawn()?;
    let mut window = Command::new(viewer)
        .args([
            "--textbox",
            header,
            "--title",
            title,
            "--icon",
            "applications-games",
            "--size",
            "1100x700",
        ])
        .stdin(Stdio::from(producer.stdout.take().ok_or("no pipe")?))
        .spawn()?;
    window.wait()?;

    // Closing the window early leaves the command writing into a pipe nobody reads.
    if producer.try_wait()?.is_none() {
        producer.kill()?;
    }

    Ok(())
}

fn message(body: &str) {
    let _ = match kurodlg() {
        Some(viewer) => Command::new(viewer)
            .args(["--textbox", body, "--title", "istar-gamescripts"])
            .stdin(Stdio::null())
            .status(),
        None => Command::new("kdialog").args(["--error", body]).status(),
    };
}

/// The host-name filter for dnscheck, from a kurodlg form; None when cancelled.
fn ask_match() -> Option<String> {
    let output = Command::new(kurodlg()?)
        .args(["--form", text("ask-title"), "--title", text("dnscheck"), "--field"])
        .arg(format!("match:text:{}:", text("ask-match")))
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("match="))
        .filter(|v| !v.is_empty())
        .map(String::from)
}

pub fn run(action: &str, targets: &[String]) -> Result<()> {
    let first = targets[0].as_str();

    match action {
        "steamlog" => {
            for target in targets {
                let name = Path::new(target).file_name().unwrap_or_default().to_string_lossy();
                show(&["steamlog", target], text("steamlog"), &name)?;
            }
        }
        "whereis" | "saves" | "status" => {
            let mut arguments = vec![action];
            arguments.extend(targets.iter().map(String::as_str));
            show(&arguments, text(action), first)?;
        }
        "prefix-logs" | "prefix-ids" | "dnscheck" => {
            let Some(root) = locate(Path::new(first)).and_then(|game| game.prefix) else {
                message(&text("not-a-prefix").replace("{}", first));
                return Ok(());
            };
            let root = root.to_string_lossy();

            match action {
                "prefix-logs" => show(&["prefix", &root, "--logs", "--since", "120"], text(action), &root)?,
                "prefix-ids" => show(&["prefix", &root, "--ids"], text(action), &root)?,
                _ => {
                    if let Some(filter) = ask_match() {
                        show(
                            &["dnscheck", "--from-prefix", &root, "--match", &filter],
                            text(action),
                            &format!("{root}  ({filter})"),
                        )?;
                    }
                }
            }
        }
        _ => return Err(format!("unknown action {action:?}").into()),
    }

    Ok(())
}
