// SPDX-FileCopyrightText: 2026 Pascal Klose
// SPDX-License-Identifier: GPL-3.0-or-later

//! whereis, status, saves, orphans: what Steam and the prefix say about a game.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use walkdir::WalkDir;

use crate::steam::*;

pub fn whereis(targets: &[String]) {
    for target in targets {
        let Some(found) = locate(Path::new(target)) else {
            println!("{target}\n  not inside a game's compatdata or steamapps/common folder\n");
            continue;
        };
        let show = |path: &Option<PathBuf>, otherwise: &str| path.as_ref().map_or(otherwise.to_string(), |p| p.display().to_string());

        println!(
            "{target}\n  APPID      {}\n  name       {}\n  install    {}\n  compatdata {}\n  Proton     {}\n  launch     {}\n",
            found.appid,
            found.name,
            show(&found.installdir, "-"),
            show(&found.prefix, "- (no prefix yet)"),
            found.proton.as_deref().unwrap_or("- (Steam default)"),
            if found.launch.is_empty() { "-" } else { &found.launch },
        );
    }
}

fn output(program: &str, arguments: &[&str]) -> String {
    Command::new(program)
        .args(arguments)
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .unwrap_or_default()
}

pub fn status(target: Option<String>) -> Result<()> {
    let or = |text: String, otherwise: &str| if text.is_empty() { otherwise.to_string() } else { text };
    let gpu = or(
        output("nvidia-smi", &["--query-gpu=name,driver_version", "--format=csv,noheader"]),
        &output("sh", &["-c", "lspci | grep -i vga"]),
    );
    let vulkan: Vec<String> = output("vulkaninfo", &["--summary"])
        .lines()
        .filter(|line| line.contains("deviceName"))
        .filter_map(|line| line.split('=').nth(1).map(|name| name.trim().to_string()))
        .collect();

    println!("Steam running   {}", if steam_running() { "yes" } else { "no" });
    println!(
        "ntsync          {}",
        if Path::new("/dev/ntsync").exists() {
            "/dev/ntsync present"
        } else {
            "no /dev/ntsync - PROTON_USE_NTSYNC=1 falls back"
        }
    );
    println!("GPU             {gpu}");
    println!("Vulkan devices  {}", or(vulkan.join(", "), "- (vulkaninfo not installed)"));

    for library in libraries() {
        let free: f64 = output("df", &["-B1", "--output=avail", &library.to_string_lossy()])
            .lines()
            .nth(1)
            .and_then(|line| line.trim().parse().ok())
            .unwrap_or(0.0);
        println!("free in library {:>7.0} GB  {}", free / 1e9, library.display());
    }

    let Some(target) = target else { return Ok(()) };
    let Some(found) = game_from(&target) else {
        return Err(format!("{target} belongs to no Steam game").into());
    };

    println!("\n{}  (APPID {})", found.name, found.appid);
    println!("Proton          {}", found.proton.as_deref().unwrap_or("- (Steam default)"));
    println!("launch options  {}", if found.launch.is_empty() { "-" } else { &found.launch });

    if found.launch.contains("PROTON_LOG=1") {
        println!("PROTON_LOG      set - ~/steam-{}.log is written", found.appid);
    } else {
        println!("PROTON_LOG      not set");
    }

    if let Some(prefix) = &found.prefix {
        println!("prefix          {}  ({:.2} GB)", prefix.display(), tree_size(prefix) as f64 / 1e9);
    }

    for library in libraries() {
        let cache = library.join("shadercache").join(&found.appid);

        if cache.is_dir() {
            println!("shader cache    {:.0} MB  {}", tree_size(&cache) as f64 / 1e6, cache.display());
        }
    }

    Ok(())
}

const ROOTS: [&str; 5] = ["Documents", "Saved Games", "AppData/Roaming", "AppData/LocalLow", "AppData/Local"];
const NOISE: [&str; 17] = [
    "Microsoft",
    "Temp",
    "Packages",
    "NVIDIA",
    "D3DSCache",
    "CrashDumps",
    "My Music",
    "My Pictures",
    "My Videos",
    "My Games",
    "dxvk",
    "AT",
    "ATI",
    "AMD",
    "NVIDIA Corporation",
    "vkd3d-proton",
    "wine",
];

fn children(folder: &Path) -> Vec<PathBuf> {
    fs::read_dir(folder)
        .map(|entries| entries.flatten().map(|e| e.path()).collect())
        .unwrap_or_default()
}

/// (path, newest mtime, size) for each save-candidate folder of the game, newest first.
fn candidates(game: &Game, prefix: &Path) -> Vec<(PathBuf, SystemTime, u64)> {
    let user = prefix.join("pfx/drive_c/users/steamuser");
    let mut folders = Vec::new();

    for root in ROOTS {
        for folder in children(&user.join(root)) {
            // "My Games" holds the games' own folders, not saves of its own.
            let inner = if folder.file_name().is_some_and(|n| n == "My Games") {
                children(&folder)
            } else {
                vec![folder]
            };
            folders.extend(
                inner
                    .into_iter()
                    .filter(|f| f.is_dir() && !NOISE.iter().any(|n| f.file_name().is_some_and(|name| name == *n))),
            );
        }
    }

    for userdata in children(&steam_root().join("userdata")) {
        let remote = userdata.join(&game.appid).join("remote");

        if remote.is_dir() {
            folders.push(remote);
        }
    }

    let mut result = Vec::new();

    for folder in folders {
        let files: Vec<_> = WalkDir::new(&folder)
            .into_iter()
            .flatten()
            .filter(|e| e.file_type().is_file())
            .filter_map(|e| e.metadata().ok())
            .collect();
        let newest = files.iter().filter_map(|m| m.modified().ok()).max();

        if let Some(newest) = newest {
            result.push((folder, newest, files.iter().map(|m| m.len()).sum()));
        }
    }

    result.sort_by_key(|row| std::cmp::Reverse(row.1));

    result
}

pub fn saves(game: &str, zip: Option<String>) -> Result<()> {
    let found = game_from(game)
        .filter(|g| g.prefix.is_some())
        .ok_or("no game with a prefix found for that")?;
    let rows = candidates(&found, found.prefix.as_ref().unwrap());

    for (folder, modified, size) in &rows {
        println!(
            "{} {:>8.1} MB  {}",
            format_time(*modified, "%Y-%m-%d %H:%M"),
            *size as f64 / 1e6,
            folder.display()
        );
    }

    if rows.is_empty() {
        println!("no files in Documents, Saved Games, AppData or Steam Cloud's local copy - the game may save beside its exe");
    }

    let Some(zip) = zip.filter(|_| !rows.is_empty()) else {
        return Ok(());
    };
    let target = format!("{zip}.zip");
    let mut archive = zip::ZipWriter::new(File::create(&target)?);
    let options = zip::write::SimpleFileOptions::default();
    let mut manifest = String::new();

    for (number, (folder, _, _)) in rows.iter().enumerate() {
        let name = folder.file_name().unwrap_or_default().to_string_lossy();
        manifest += &format!("{number:02}-{name}  <-  {}\n", folder.display());

        for entry in WalkDir::new(folder).into_iter().flatten().filter(|e| e.file_type().is_file()) {
            let relative = entry.path().strip_prefix(folder)?.to_string_lossy();
            archive.start_file(format!("{number:02}-{name}/{relative}"), options)?;
            std::io::copy(&mut File::open(entry.path())?, &mut archive)?;
        }
    }

    archive.start_file("MANIFEST.txt", options)?;
    archive.write_all(manifest.as_bytes())?;
    archive.finish()?;
    println!("\nwritten {target} - MANIFEST.txt inside names each folder's original path");

    Ok(())
}

/// compatdata and shadercache folders that no installed game and no non-Steam shortcut points to. Deletes nothing.
pub fn orphans() {
    let mut known: std::collections::HashSet<String> = manifests().into_keys().collect();
    known.extend(shortcuts().into_iter().map(|s| s.appid));
    let mut total = 0;

    for library in libraries() {
        for kind in ["compatdata", "shadercache"] {
            let mut folders = children(&library.join(kind));
            folders.sort();

            for folder in folders {
                let name = folder.file_name().unwrap_or_default().to_string_lossy().into_owned();

                if known.contains(&name) {
                    continue;
                }

                let size = tree_size(&folder);
                total += size;
                let used = [folder.join("pfx/user.reg"), folder.clone()]
                    .iter()
                    .filter_map(|p| fs::metadata(p).ok()?.modified().ok())
                    .max()
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                println!(
                    "{:>9.0} MB  {}  {kind:11} {name:12} {}",
                    size as f64 / 1e6,
                    format_time(used, "%Y-%m-%d"),
                    library.display()
                );
            }
        }
    }

    println!(
        "\n{:.2} GB in folders nothing refers to (id 0 is Steam's own default prefix)",
        total as f64 / 1e9
    );
}
