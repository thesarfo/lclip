#!/usr/bin/env bash
# install or remove lclip for the current user.
#   ./install.sh              build and install
#   ./install.sh uninstall    remove it
set -euo pipefail

cd "$(dirname "$0")"

prefix="${PREFIX:-$HOME/.local}"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
config="${XDG_CONFIG_HOME:-$HOME/.config}"

bin="$prefix/bin/lclip"
menu="$data/applications/lclip.desktop"
icon="$data/icons/hicolor/scalable/apps/lclip.svg"
autostart="$config/autostart/lclip.desktop"

case "${1:-install}" in
install)
    cargo build --release
    install -Dm755 target/release/lclip "$bin"
    install -Dm644 assets/lclip.svg "$icon"
    mkdir -p "$(dirname "$menu")" "$(dirname "$autostart")"
    sed "s|@BIN@|$bin|" assets/lclip.desktop >"$menu"
    sed "s|@BIN@|$bin|" assets/lclip-autostart.desktop >"$autostart"
    command -v update-desktop-database >/dev/null && update-desktop-database "$data/applications" || true
    command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$data/icons/hicolor" || true
    echo "installed $bin"
    case ":$PATH:" in
    *":$prefix/bin:"*) ;;
    *) echo "add $prefix/bin to your PATH to run: lclip" ;;
    esac
    echo "start it now with: lclip --hidden &"
    ;;
uninstall)
    "$bin" quit 2>/dev/null || true
    rm -f "$bin" "$menu" "$icon" "$autostart"
    echo "removed lclip"
    ;;
*)
    echo "usage: $0 [install|uninstall]" >&2
    exit 2
    ;;
esac
