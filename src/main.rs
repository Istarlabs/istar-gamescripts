// SPDX-FileCopyrightText: 2026 Pascal Klose
// SPDX-License-Identifier: GPL-3.0-or-later

//! Small helpers for games on Linux under Steam/Proton - one binary, one subcommand per helper.

mod dnscheck;
mod info;
mod menu;
mod prefix;
mod steam;
mod steamlog;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// A Proton log without the Wine noise: what ran, what the game printed, Wine's real errors
    Steamlog {
        /// Default: the newest ~/steam-*.log
        log: Option<PathBuf>,
        /// Only lines matching this regular expression (case-insensitive)
        #[arg(long)]
        grep: Option<String>,
        /// Keep Wine's routine err: lines too
        #[arg(long)]
        all_errors: bool,
    },
    /// Which Steam game a folder belongs to: APPID, name, install folder, compatdata
    Whereis {
        /// A game's install folder, its compatdata folder, or anything inside either
        #[arg(required = true)]
        paths: Vec<String>,
    },
    /// Proton, launch options, GPU, ntsync, shader cache, free space
    Status {
        /// Appid or any path of a game; without it only the machine part
        game: Option<String>,
    },
    /// Folders that may hold a game's saves (prefix and Steam Cloud copy), optionally zipped
    Saves {
        /// Appid or any path of the game
        game: String,
        /// Put them all into FILE.zip
        #[arg(long, value_name = "FILE")]
        zip: Option<String>,
    },
    /// compatdata/shadercache folders no game or shortcut refers to
    Orphans,
    /// A game's Wine prefix: the logs its launcher wrote, and the device identity it carries
    Prefix {
        /// compatdata folder name, or a path to a prefix copy
        appid: String,
        /// Log files written recently
        #[arg(long)]
        logs: bool,
        /// Minutes back for --logs
        #[arg(long, default_value_t = 120)]
        since: u64,
        /// Machine and device ids
        #[arg(long)]
        ids: bool,
        /// Another copy of the prefix to set the ids beside
        #[arg(long, value_name = "PATH")]
        compare: Option<String>,
        /// Copy registry files to ~/Documents/prefix-backups/APPID/TIMESTAMP
        #[arg(long)]
        backup: bool,
        /// Copy a backup back (default: the newest); Steam must be closed
        #[arg(long, num_args = 0..=1, value_name = "PATH")]
        restore: Option<Option<PathBuf>>,
    },
    /// Whether a host is blocked, and by whom - your resolver's lists or a filtering upstream behind it
    Dnscheck {
        hosts: Vec<String>,
        /// Collect host names from a game's prefix (needs --match)
        #[arg(long, value_name = "APPID")]
        from_prefix: Option<String>,
        /// Collect host names from a log, e.g. ~/steam-123.log (needs --match)
        #[arg(long, value_name = "FILE")]
        from_log: Option<String>,
        /// One host per line
        #[arg(long, value_name = "FILE")]
        from_file: Option<String>,
        /// Only names matching this text/regex
        #[arg(long = "match", value_name = "RE")]
        matching: Option<String>,
        /// Also list hosts that resolve normally
        #[arg(long)]
        all: bool,
    },
    /// What the Dolphin menu calls: one of the above in a kurodlg window
    Menu {
        /// steamlog, whereis, saves, status, prefix-logs, prefix-ids, dnscheck
        action: String,
        #[arg(required = true)]
        targets: Vec<String>,
    },
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Steamlog { log, grep, all_errors } => steamlog::run(log, grep, all_errors),
        Command::Whereis { paths } => {
            info::whereis(&paths);
            Ok(())
        }
        Command::Status { game } => info::status(game),
        Command::Saves { game, zip } => info::saves(&game, zip),
        Command::Orphans => {
            info::orphans();
            Ok(())
        }
        Command::Prefix {
            appid,
            logs,
            since,
            ids,
            compare,
            backup,
            restore,
        } => prefix::run(prefix::Options {
            appid,
            logs,
            since,
            ids,
            compare,
            backup,
            restore,
        }),
        Command::Dnscheck {
            hosts,
            from_prefix,
            from_log,
            from_file,
            matching,
            all,
        } => dnscheck::run(dnscheck::Options {
            hosts,
            from_prefix,
            from_log,
            from_file,
            matching,
            all,
        }),
        Command::Menu { action, targets } => menu::run(&action, &targets),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
