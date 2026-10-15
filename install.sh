#!/usr/bin/env bash
# install or remove lclip for the current user.
#   ./install.sh              install (uses a prebuilt ./lclip if present, otherwise builds)
#   ./install.sh uninstall    remove it
#
# on gnome this also binds Super+Shift+V to "lclip toggle".
# set LCLIP_SHORTCUT to change it, or LCLIP_SHORTCUT=none to skip.
set -euo pipefail

cd "$(dirname "$0")"

prefix="${PREFIX:-$HOME/.local}"
data="${XDG_DATA_HOME:-$HOME/.local/share}"
config="${XDG_CONFIG_HOME:-$HOME/.config}"

bin="$prefix/bin/lclip"
menu="$data/applications/lclip.desktop"
icon="$data/icons/hicolor/scalable/apps/lclip.svg"
autostart="$config/autostart/lclip.desktop"

kb_schema=org.gnome.settings-daemon.plugins.media-keys
kb_path=/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/lclip/

have_gnome_keys() {
    [ "${LCLIP_SHORTCUT:-}" != none ] && command -v gsettings >/dev/null &&
        gsettings list-schemas 2>/dev/null | grep -qx "$kb_schema"
}

# add or remove our path in the list of custom keybindings, keeping the others
edit_kb_list() {
    local action="$1" current
    current=$(gsettings get "$kb_schema" custom-keybindings)
    python3 - "$action" "$kb_path" "$current" <<'PY' | xargs -0 gsettings set "$kb_schema" custom-keybindings
import ast, sys
action, path, raw = sys.argv[1:4]
raw = raw.removeprefix("@as ").strip()
items = list(ast.literal_eval(raw)) if raw else []
if action == "add" and path not in items:
    items.append(path)
if action == "remove":
    items = [i for i in items if i != path]
sys.stdout.write(repr(items))
PY
}

case "${1:-install}" in
install)
    if [ -x ./lclip ]; then
        src=./lclip
    else
        cargo build --release
        src=target/release/lclip
    fi
    install -Dm755 "$src" "$bin"
    install -Dm644 assets/lclip.svg "$icon"
    mkdir -p "$(dirname "$menu")" "$(dirname "$autostart")"
    sed "s|@BIN@|$bin|" assets/lclip.desktop >"$menu"
    sed "s|@BIN@|$bin|" assets/lclip-autostart.desktop >"$autostart"
    command -v update-desktop-database >/dev/null && update-desktop-database "$data/applications" || true
    command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$data/icons/hicolor" || true
    echo "installed $bin"

    if have_gnome_keys; then
        accel="${LCLIP_SHORTCUT:-<Super><Shift>v}"
        key="$kb_schema.custom-keybinding:$kb_path"
        gsettings set "$key" name 'lclip'
        gsettings set "$key" command "$bin toggle"
        gsettings set "$key" binding "$accel"
        edit_kb_list add
        echo "shortcut: $accel opens and closes lclip"
    else
        echo "bind a key to: $bin toggle"
    fi

    case ":$PATH:" in
    *":$prefix/bin:"*) ;;
    *) echo "add $prefix/bin to your PATH to run: lclip" ;;
    esac
    echo "start it now with: $bin --daemon &"
    ;;
uninstall)
    "$bin" quit 2>/dev/null || true
    if have_gnome_keys; then
        edit_kb_list remove
        gsettings reset-recursively "$kb_schema.custom-keybinding:$kb_path" 2>/dev/null || true
    fi
    rm -f "$bin" "$menu" "$icon" "$autostart"
    echo "removed lclip"
    ;;
*)
    echo "usage: $0 [install|uninstall]" >&2
    exit 2
    ;;
esac
