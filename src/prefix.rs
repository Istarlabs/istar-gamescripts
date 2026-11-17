// SPDX-FileCopyrightText: 2026 Pascal Klose
// SPDX-License-Identifier: GPL-3.0-or-later

//! Look into the Wine prefix of a Steam (non-Steam) game: what it logged, and who it thinks it is.
//!
//! A game's own logs sit inside its prefix, not in the Proton log - launchers write theirs under
//! AppData. And a game that binds an account to a device builds that device's identity from values in
//! the prefix, so a recreated prefix is a new device to it.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime};

use regex::Regex;
use walkdir::WalkDir;

use crate::steam::*;

static LOG_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\.(log|txt)$|\.log\.|^log|Player\.log").unwrap());
/// Registry values that make up a device's identity. MachineGuid is Windows' own; the others are what
/// launchers and game SDKs store (names like SDK_DEVICE_ID, DeviceFp, HYPDeviceId). Extend the
/// pattern when another game turns up with its own.
static ID_VALUE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)^"(MachineGuid|[^"]*Device_?(Id|Fp|Fingerprint)[^"]*)"="([^"]*)""#).unwrap());

const SKIPPED_DIRECTORIES: [&str; 4] = ["/windows/", "/ShaderBlob/", "/fedata/", "/dxvk/"];
const REGISTRY: [&str; 3] = ["system.reg", "user.reg", "userdef.reg"];

fn backups() -> PathBuf {
    home().join("Documents/prefix-backups")
}

fn recent_logs(root: &Path, minutes: u64) -> Vec<(SystemTime, u64, String)> {
    let since = SystemTime::now() - Duration::from_secs(minutes * 60);
    let drive = root.join("pfx/drive_c");
    let mut found = Vec::new();
    let walker = WalkDir::new(&drive).into_iter().filter_entry(|entry| {
        let path = format!("{}/", entry.path().display());
        !entry.file_type().is_dir() || !SKIPPED_DIRECTORIES.iter().any(|skip| path.contains(skip))
    });

    for entry in walker.flatten().filter(|e| !e.file_type().is_dir()) {
        if !LOG_NAME.is_match(&entry.file_name().to_string_lossy()) {
            continue;
        }

        let Ok(metadata) = fs::metadata(entry.path()) else { continue };
        let Ok(modified) = metadata.modified() else { continue };

        if modified >= since {
            let shown = entry
                .path()
                .strip_prefix(&drive)
                .map(|p| format!("C:/{}", p.display()))
                .unwrap_or_default();
            found.push((modified, metadata.len(), shown));
        }
    }

    found.sort();

    found
}

/// Registry key and value name -> value, for everything ID_VALUE recognises.
fn identity(root: &Path) -> BTreeMap<String, String> {
    let mut ids = BTreeMap::new();

    for name in ["system.reg", "user.reg"] {
        let mut key = String::new();

        for line in read_text(&root.join("pfx").join(name)).lines() {
            if let Some(rest) = line.strip_prefix('[') {
                key = rest.split(']').next().unwrap_or("").replace("\\\\", "\\");
            } else if let Some(found) = ID_VALUE.captures(line) {
                ids.insert(format!("{name}  {key}\\{}", &found[1]), found[3].to_string());
            }
        }
    }

    ids
}

fn created(root: &Path) -> String {
    let marker = root.join("version");
    let modified = fs::metadata(if marker.exists() { &marker } else { root })
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);

    format_time(modified, "%Y-%m-%d %H:%M")
}

/// Relative paths (to the compatdata folder) of what a backup holds: the registry and launcher Local Storage.
fn saved_files(root: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = REGISTRY
        .iter()
        .map(|name| Path::new("pfx").join(name))
        .filter(|relative| root.join(relative).exists())
        .collect();

    for entry in WalkDir::new(root.join("pfx/drive_c")).into_iter().flatten() {
        let storage = entry.path().join("Local Storage");

        if entry.file_name() == "fedata" && storage.is_dir() {
            files.extend(storage.strip_prefix(root).map(Path::to_path_buf));
        }
    }

    files
}

fn copy(source: &Path, destination: &Path) -> Result<()> {
    if source.is_dir() {
        for entry in WalkDir::new(source).follow_links(true) {
            let entry = entry?;
            let target = destination.join(entry.path().strip_prefix(source)?);

            if entry.file_type().is_dir() {
                fs::create_dir_all(&target)?;
            } else {
                fs::copy(entry.path(), &target)?;
            }
        }
    } else {
        fs::create_dir_all(destination.parent().ok_or("no parent folder")?)?;
        fs::copy(source, destination)?;
        File::options()
            .write(true)
            .open(destination)?
            .set_modified(fs::metadata(source)?.modified()?)?;
    }

    Ok(())
}

fn backup(root: &Path) -> Result<()> {
    let stamp = format_time(SystemTime::now(), "%Y-%m-%d_%H%M%S");
    let target = backups().join(root.file_name().unwrap_or_default()).join(stamp);

    for relative in saved_files(root) {
        copy(&root.join(&relative), &target.join(&relative))?;
        println!("  saved {}", relative.display());
    }

    if steam_running() {
        println!("  note: Steam is running - if the game was open, its registry may be older than it looks");
    }

    println!("backup: {}", target.display());

    Ok(())
}

fn restore(root: &Path, source: Option<PathBuf>) -> Result<()> {
    if steam_running() {
        return Err("Steam is running - close it first, it rewrites the registry files on exit".into());
    }

    let source = match source {
        Some(source) => source,
        None => {
            let folder = backups().join(root.file_name().unwrap_or_default());
            let mut found: Vec<PathBuf> = fs::read_dir(&folder)
                .map(|d| d.flatten().map(|e| e.path()).collect())
                .unwrap_or_default();
            found.sort();
            found.pop().ok_or_else(|| format!("no backups in {}", folder.display()))?
        }
    };

    for relative in saved_files(&source) {
        copy(&source.join(&relative), &root.join(&relative))?;
        println!("  restored {}", relative.display());
    }

    println!("restored from {}", source.display());

    Ok(())
}

pub struct Options {
    pub appid: String,
    pub logs: bool,
    pub since: u64,
    pub ids: bool,
    pub compare: Option<String>,
    pub backup: bool,
    pub restore: Option<Option<PathBuf>>,
}

pub fn run(options: Options) -> Result<()> {
    let root = prefix_path(&options.appid)?;

    if options.backup {
        return backup(&root);
    }

    if let Some(source) = options.restore {
        return restore(&root, source);
    }

    println!(
        "{}\n  Proton last set it up {} (on creation, or when its Proton version changed)",
        root.display(),
        created(&root)
    );

    if options.logs {
        println!("\nlogs written in the last {} min:", options.since);
        let logs = recent_logs(&root, options.since);

        for (modified, size, path) in &logs {
            println!("  {} {size:>10}  {path}", format_time(*modified, "%H:%M:%S"));
        }

        if logs.is_empty() {
            println!("  none");
        }
    }

    if options.ids || options.compare.is_some() {
        let mine = identity(&root);
        let other = match &options.compare {
            Some(path) => identity(&prefix_path(path)?),
            None => BTreeMap::new(),
        };
        println!("\nidentity:");

        for key in mine.keys().chain(other.keys()).collect::<BTreeSet<_>>() {
            println!("  {key}\n      this:  {}", mine.get(key).map_or("-", String::as_str));

            if options.compare.is_some() {
                let same = if mine.get(key) == other.get(key) { "same" } else { "DIFFERENT" };
                println!("      other: {}   ({same})", other.get(key).map_or("-", String::as_str));
            }
        }
    }

    Ok(())
}
