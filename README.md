# istar-gamescripts

Small helpers for games on Linux under Steam/Proton: one Rust binary, `istar-gamescripts`, with a subcommand per
helper (`--help` on each), and a Dolphin context menu that runs them on a log file or a game's folder.

| Command | For |
| --- | --- |
| `steamlog` | A Proton log without the Wine noise: what ran, what the game printed, Wine's real errors |
| `whereis` | Which Steam game a folder belongs to: APPID, name, install folder, compatdata - across all Steam libraries |
| `status` | Proton version, launch options, GPU, ntsync, shader cache, free space |
| `saves` | Folders that may hold a game's saves (prefix and Steam Cloud copy), optionally zipped |
| `orphans` | compatdata/shadercache folders no game or shortcut refers to |
| `prefix` | A game's Wine prefix: the logs its launcher wrote, and the device identity it carries |
| `dnscheck` | Whether a host is blocked, and by whom - your resolver's lists or a filtering upstream behind it |

## Install

The scripts in `Scripts/` come from [devscripts](https://github.com/Istarlabs/devscripts), pinned as a submodule:
clone with `git clone --recurse-submodules`, or run `git submodule update --init` in an existing clone.

**As a user, into `~/.local`** - the binary into `~/.local/bin` and the Dolphin menu into
`~/.local/share/kio/servicemenus`, with `kbuildsycoca6` run so Dolphin sees it at once:

```bash
python3 Scripts/rust.py install
```

**As an RPM** for `/usr/bin` and `/usr/share/kio/servicemenus`:

```bash
python3 Scripts/rpm.py build      # ~/.cache/devscripts/rpm/istar-gamescripts-<version>-<timestamp>.x86_64.rpm
sudo dnf install ~/.cache/devscripts/rpm/istar-gamescripts-*.rpm
```

Needs a current stable Rust toolchain to build. At run time: `dig` (bind-utils) for `dnscheck`, and
[kurodlg](https://github.com/Istarlabs/kurodlg) for the menu's output window (without it, a Konsole that stays open).

`python3 Scripts/rust.py verify` is clippy, tests, rustfmt and the licence lines.

## In Dolphin

The **Game tools** submenu. One menu serves logs and folders, so the log entry also shows on folders (and the game
entries on logs) and says so there.

| Right-click on | Entry | Runs |
| --- | --- | --- |
| a `.log` file | Summarise Proton log | `steamlog` |
| any folder of a game (install or compatdata) | Steam game: APPID and compatdata | `whereis` |
| any folder of a game | Steam game: Proton, launch options, caches | `status` |
| any folder of a game | Steam game: find save folders | `saves` |
| a game's compatdata folder, its `pfx`, or anything in it | Prefix: recent logs | `prefix --logs --since 120` |
| the same | Prefix: device identity | `prefix --ids` |
| the same | Prefix: check for blocked hosts… | asks for a host-name filter, then `dnscheck --from-prefix` |

`istar-gamescripts menu` is what the entries call. The output streams into a `kurodlg --textbox` window, which has a
**Copy** button for the whole text. The prefix entries appear on every folder - on one that is not inside a prefix they
say so.

All commands find prefixes through `src/steam.rs`, which reads `libraryfolders.vdf`: a game installed in a second
Steam library has its prefix in that library's `steamapps/compatdata`, not under `~/.local/share/Steam`. The prefix
entries work from a game's install folder (`steamapps/common/<game>`) too.

## Commands

Each command, with examples and what it reads, is on the [wiki](https://github.com/Istarlabs/istar-gamescripts/wiki):
[whereis](https://github.com/Istarlabs/istar-gamescripts/wiki/Whereis),
[status, saves, orphans](https://github.com/Istarlabs/istar-gamescripts/wiki/Status-saves-orphans),
[steamlog](https://github.com/Istarlabs/istar-gamescripts/wiki/Steamlog),
[prefix](https://github.com/Istarlabs/istar-gamescripts/wiki/Prefix) and
[dnscheck](https://github.com/Istarlabs/istar-gamescripts/wiki/Dnscheck).

## Licence

istar-gamescripts is licensed under the [GNU General Public License v3.0 or later](LICENSE.md).
