// SPDX-FileCopyrightText: 2026 Pascal Klose
// SPDX-License-Identifier: GPL-3.0-or-later

//! Where Steam keeps things: every library folder, and the game or prefix behind any path.
//!
//! Steam puts a game's `compatdata/<appid>` into the library the game is installed in, so with a
//! second library (e.g. /mnt/games/SteamLibrary) the prefix is not under ~/.steam.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;
use std::time::SystemTime;

use regex::Regex;
use walkdir::WalkDir;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

static PAIR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""([^"]+)"\s+"([^"]*)""#).unwrap());
static TOKEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#""((?:[^"\\]|\\.)*)"|([{}])"#).unwrap());

pub fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

pub fn steam_root() -> PathBuf {
    home().join(".local/share/Steam")
}

/// A file as text, bytes that are not UTF-8 replaced; empty when it cannot be read.
pub fn read_text(path: &Path) -> String {
    fs::read(path)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default()
}

pub fn format_time(time: SystemTime, format: &str) -> String {
    chrono::DateTime::<chrono::Local>::from(time).format(format).to_string()
}

pub fn truncate(text: &str, length: usize) -> String {
    text.chars().take(length).collect()
}

/// Every steamapps folder: the Steam root's own plus those in libraryfolders.vdf (that still exist).
pub fn libraries() -> Vec<PathBuf> {
    let root = steam_root();
    let mut folders = vec![root.join("steamapps")];

    for found in PAIR.captures_iter(&read_text(&root.join("steamapps/libraryfolders.vdf"))) {
        if &found[1] == "path" {
            folders.push(Path::new(&found[2]).join("steamapps"));
        }
    }

    let mut unique: Vec<PathBuf> = Vec::new();

    for folder in folders {
        if folder.is_dir() && !unique.contains(&folder) {
            unique.push(folder);
        }
    }

    unique
}

pub struct Manifest {
    pub name: String,
    pub installdir: String,
    pub folder: PathBuf,
}

/// appid -> manifest over all libraries.
pub fn manifests() -> HashMap<String, Manifest> {
    let mut found = HashMap::new();

    for folder in libraries() {
        let Ok(entries) = fs::read_dir(&folder) else { continue };

        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();

            if !(name.starts_with("appmanifest_") && name.ends_with(".acf")) {
                continue;
            }

            let text = read_text(&entry.path());
            let fields: HashMap<String, String> = PAIR.captures_iter(&text).map(|c| (c[1].to_string(), c[2].to_string())).collect();

            if let Some(appid) = fields.get("appid") {
                found.insert(
                    appid.clone(),
                    Manifest {
                        name: fields.get("name").cloned().unwrap_or_else(|| "?".into()),
                        installdir: fields.get("installdir").cloned().unwrap_or_default(),
                        folder: folder.clone(),
                    },
                );
            }
        }
    }

    found
}

/// A parsed VDF value. Keys are looked up case-insensitively, the last duplicate wins.
pub enum Vdf {
    Text(String),
    Number(u32),
    Map(Vec<(String, Vdf)>),
}

impl Vdf {
    pub fn get(&self, key: &str) -> Option<&Vdf> {
        match self {
            Vdf::Map(entries) => entries.iter().rev().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn dig(&self, keys: &[&str]) -> Option<&Vdf> {
        keys.iter().try_fold(self, |tree, key| tree.get(key))
    }

    pub fn text(&self) -> Option<String> {
        match self {
            Vdf::Text(text) => Some(text.clone()),
            Vdf::Number(number) => Some(number.to_string()),
            Vdf::Map(_) => None,
        }
    }

    pub fn entries(&self) -> &[(String, Vdf)] {
        match self {
            Vdf::Map(entries) => entries,
            _ => &[],
        }
    }
}

fn cstring(data: &[u8], position: &mut usize) -> Result<String> {
    let rest = data.get(*position..).ok_or("truncated VDF")?;
    let end = rest.iter().position(|&byte| byte == 0).ok_or("unterminated string in VDF")?;
    *position += end + 1;

    Ok(String::from_utf8_lossy(&rest[..end]).into_owned())
}

/// Steam's binary VDF (shortcuts.vdf).
pub fn binary_vdf(data: &[u8], position: &mut usize) -> Result<Vdf> {
    let mut entries = Vec::new();

    while *position < data.len() {
        let kind = data[*position];
        *position += 1;

        if kind == 8 {
            break;
        }

        let key = cstring(data, position)?;
        let value = match kind {
            0 => binary_vdf(data, position)?,
            1 => Vdf::Text(cstring(data, position)?),
            2 => {
                let bytes = data.get(*position..*position + 4).ok_or("truncated VDF")?;
                *position += 4;
                Vdf::Number(u32::from_le_bytes(bytes.try_into()?))
            }
            _ => return Err(format!("unknown binary VDF type {kind}").into()),
        };

        entries.push((key, value));
    }

    Ok(Vdf::Map(entries))
}

/// A text VDF (config.vdf, localconfig.vdf); empty when the file is missing.
pub fn text_vdf(path: &Path) -> Vdf {
    parse_text_vdf(&read_text(path))
}

pub fn parse_text_vdf(text: &str) -> Vdf {
    let mut stack: Vec<Vec<(String, Vdf)>> = vec![vec![]];
    let mut names: Vec<String> = vec![];
    let mut key: Option<String> = None;

    for token in TOKEN.captures_iter(text) {
        if let Some(quoted) = token.get(1) {
            match key.take() {
                None => key = Some(quoted.as_str().to_string()),
                Some(name) => stack.last_mut().unwrap().push((name, Vdf::Text(quoted.as_str().to_string()))),
            }
        } else if &token[2] == "{" {
            names.push(key.take().unwrap_or_default());
            stack.push(vec![]);
        } else if stack.len() > 1 {
            let done = stack.pop().unwrap();
            stack.last_mut().unwrap().push((names.pop().unwrap(), Vdf::Map(done)));
        }
    }

    Vdf::Map(stack.swap_remove(0))
}

/// `config/<name>` of every Steam user.
pub fn userdata_files(name: &str) -> Vec<PathBuf> {
    let Ok(users) = fs::read_dir(steam_root().join("userdata")) else {
        return vec![];
    };
    let mut files: Vec<PathBuf> = users
        .flatten()
        .map(|user| user.path().join("config").join(name))
        .filter(|path| path.is_file())
        .collect();
    files.sort();

    files
}

pub struct Shortcut {
    pub name: String,
    /// Also the compatdata folder name.
    pub appid: String,
    pub folders: Vec<PathBuf>,
    pub launch: String,
}

/// Non-Steam games from every user's shortcuts.vdf.
pub fn shortcuts() -> Vec<Shortcut> {
    let mut found = Vec::new();

    for path in userdata_files("shortcuts.vdf") {
        let Ok(data) = fs::read(&path) else { continue };
        let Ok(top) = binary_vdf(&data, &mut 0) else { continue };
        let Some(list) = top.get("shortcuts") else { continue };

        for (_, entry) in list.entries() {
            let field = |key: &str| entry.get(key).and_then(Vdf::text).unwrap_or_default();
            let mut folders = Vec::new();
            let startdir = field("startdir");
            let exe = field("exe");

            if !startdir.is_empty() {
                folders.push(PathBuf::from(startdir.trim_matches('"')));
            }

            if !exe.is_empty() {
                folders.push(Path::new(exe.trim_matches('"')).parent().unwrap_or(Path::new("")).to_path_buf());
            }

            found.push(Shortcut {
                name: entry.get("appname").and_then(Vdf::text).unwrap_or_else(|| "?".into()),
                appid: entry.get("appid").and_then(Vdf::text).unwrap_or_else(|| "0".into()),
                folders,
                launch: field("launchoptions"),
            });
        }
    }

    found
}

/// The compatdata/<appid> folder in whichever library has it.
pub fn find_prefix(appid: &str) -> Option<PathBuf> {
    libraries()
        .into_iter()
        .map(|folder| folder.join("compatdata").join(appid))
        .find(|path| path.join("pfx").is_dir())
}

pub struct Game {
    pub appid: String,
    pub name: String,
    pub prefix: Option<PathBuf>,
    pub installdir: Option<PathBuf>,
    pub launch: String,
    pub proton: Option<String>,
}

/// What a path belongs to; works from anything inside compatdata/<appid> or steamapps/common/<installdir>.
pub fn locate(selected: &Path) -> Option<Game> {
    let path = fs::canonicalize(selected).unwrap_or_else(|_| selected.to_path_buf());
    let known = manifests();
    let mut appid = None;

    for candidate in path.ancestors() {
        let parent = candidate.parent();
        let parent_name = parent.and_then(Path::file_name);
        let name = candidate.file_name().map(|name| name.to_string_lossy().into_owned());

        if parent_name.is_some_and(|n| n == "compatdata") {
            appid = name;
            break;
        }

        let steamapps = parent.and_then(Path::parent);

        if parent_name.is_some_and(|n| n == "common") && steamapps.and_then(Path::file_name).is_some_and(|n| n == "steamapps") {
            appid = known
                .iter()
                .find(|(_, m)| Some(&m.installdir) == name.as_ref() && Some(m.folder.as_path()) == steamapps)
                .map(|(id, _)| id.clone());
            break;
        }
    }

    let Some(appid) = appid else {
        for shortcut in shortcuts() {
            if let Some(folder) = shortcut.folders.iter().find(|folder| path.starts_with(folder)) {
                return Some(details(
                    &shortcut.appid,
                    &format!("{} (non-Steam shortcut)", shortcut.name),
                    Some(folder.clone()),
                ));
            }
        }

        return None;
    };

    Some(match known.get(&appid) {
        Some(m) if !m.installdir.is_empty() => {
            let installdir = m.folder.join("common").join(&m.installdir);
            let name = m.name.clone();
            details(&appid, &name, Some(installdir))
        }
        Some(m) => details(&appid, &m.name.clone(), None),
        None => details(&appid, "(not a Steam store game - no manifest)", None),
    })
}

/// The Game with the Proton version and launch options Steam has for it.
pub fn details(appid: &str, name: &str, installdir: Option<PathBuf>) -> Game {
    let launch = shortcuts()
        .into_iter()
        .find(|shortcut| shortcut.appid == appid)
        .map(|shortcut| shortcut.launch)
        .or_else(|| {
            userdata_files("localconfig.vdf").iter().find_map(|path| {
                let config = text_vdf(path);
                let options = config
                    .dig(&["UserLocalConfigStore", "Software", "Valve", "Steam", "apps", appid, "LaunchOptions"])
                    .and_then(Vdf::text);

                options.filter(|text| !text.is_empty())
            })
        })
        .unwrap_or_default();

    let config = text_vdf(&steam_root().join("config/config.vdf"));
    let proton = config
        .dig(&["InstallConfigStore", "Software", "Valve", "Steam", "CompatToolMapping"])
        .and_then(|mapping| mapping.get(appid).or_else(|| mapping.get("0")))
        .and_then(|tool| tool.get("name"))
        .and_then(Vdf::text);

    Game {
        appid: appid.into(),
        name: name.into(),
        prefix: find_prefix(appid),
        installdir,
        launch,
        proton,
    }
}

/// A game from an appid or any path of it.
pub fn game_from(argument: &str) -> Option<Game> {
    locate(Path::new(argument))
        .or_else(|| (!argument.is_empty() && argument.bytes().all(|byte| byte.is_ascii_digit())).then(|| details(argument, "?", None)))
}

pub fn steam_running() -> bool {
    Command::new("pgrep")
        .args(["-f", "ubuntu12_32/steam"])
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Bytes under path (files only, links not followed).
pub fn tree_size(path: &Path) -> u64 {
    WalkDir::new(path)
        .into_iter()
        .flatten()
        .filter(|entry| entry.file_type().is_file())
        .filter_map(|entry| entry.metadata().ok())
        .map(|metadata| metadata.len())
        .sum()
}

/// A prefix from an appid (any library) or from a path to a prefix copy.
pub fn prefix_path(value: &str) -> Result<PathBuf> {
    let path = if Path::new(value).is_dir() {
        Some(PathBuf::from(value))
    } else {
        find_prefix(value)
    };

    match path {
        Some(path) if path.join("pfx").is_dir() => Ok(path),
        _ => {
            let searched: Vec<String> = libraries().iter().map(|f| f.display().to_string()).collect();
            Err(format!(
                "no prefix for {value} in {} - pass an appid or a path to a copy of one",
                searched.join(", ")
            )
            .into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_vdf_reads_a_shortcut() {
        let mut data = b"\0shortcuts\0\x000\0\x01AppName\0Game\0\x02appid\0".to_vec();
        data.extend(3_000_000_001u32.to_le_bytes());
        data.extend(b"\x08\x08\x08");

        let tree = binary_vdf(&data, &mut 0).unwrap();
        let entry = tree.dig(&["shortcuts", "0"]).unwrap();

        assert_eq!(entry.get("appname").and_then(Vdf::text).unwrap(), "Game");
        assert_eq!(entry.get("appid").and_then(Vdf::text).unwrap(), "3000000001");
    }

    #[test]
    fn text_vdf_nests_and_ignores_case() {
        let tree = parse_text_vdf(r#""A" { "b" { "Name" "proton_9" } "c" "d" }"#);

        assert_eq!(tree.dig(&["a", "B", "name"]).and_then(Vdf::text).unwrap(), "proton_9");
        assert_eq!(tree.dig(&["a", "c"]).and_then(Vdf::text).unwrap(), "d");
    }
}
