#!/bin/bash -ex

PREFIX="${PREFIX:-$HOME/.local}"
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"

cd "$(dirname "$0")"
cargo build --release --locked

install -Dm755 target/release/hematite "$PREFIX/bin/hematite"
install -Dm644 data/io.github.hematite.Editor.desktop "$DATA/applications/io.github.hematite.Editor.desktop"
install -Dm644 data/icons/scalable/apps/io.github.hematite.Editor.svg "$DATA/icons/hicolor/scalable/apps/io.github.hematite.Editor.svg"

gtk-update-icon-cache -qtf "$DATA/icons/hicolor" || true
update-desktop-database -q "$DATA/applications" || true
