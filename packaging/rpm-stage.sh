#!/bin/sh
# SPDX-FileCopyrightText: 2026 Pascal Klose
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Lays out the package below STAGE as it will sit under /: the binary and the Dolphin menu, rendered for
# /usr/bin by devscripts' servicemenu.py (found the way the Scripts/ shims find devscripts). Called by
# devscripts' rpm.py ([rpm] build in Scripts/project.toml) after `cargo build --release`.
#
#     sh packaging/rpm-stage.sh STAGE
set -eu

stage=$1
install -Dm755 target/release/istar-gamescripts "$stage/usr/bin/istar-gamescripts"

devscripts=${DEVSCRIPTS:-$HOME/dev/tools/devscripts}
[ -d "$devscripts/shared" ] || devscripts=.devscripts
install -d "$stage/usr/share/kio/servicemenus"
python3 "$devscripts/shared/lib/servicemenu.py" servicemenus/istar-gamescripts.menu.toml --bin /usr/bin \
    > "$stage/usr/share/kio/servicemenus/istar-gamescripts.desktop"
