#!/usr/bin/env bash
# install or remove lclip for the current user.
#   ./install.sh              install (uses a prebuilt ./lclip if present, otherwise builds)
#   ./install.sh uninstall    remove it
#
# on gnome this also binds Super+Shift+V to "lclip toggle". on macOS it adds a
# LaunchAgent and lclip itself listens for Cmd+Shift+V.
# set LCLIP_SHORTCUT to change it, or LCLIP_SHORTCUT=none to skip.
set -euo pipefail

cd "$(dirname "$0")"

# macOS: a LaunchAgent starts it at login; the shortcut is built into lclip.
if [ "$(uname -s)" = Darwin ]; then
    bin="${PREFIX:-$HOME/.local}/bin/lclip"
    label=com.thesarfo.lclip
    agent="$HOME/Library/LaunchAgents/$label.plist"
    domain="gui/$(id -u)"

    case "${1:-install}" in
    install)
        if [ -x ./lclip ]; then
            src=./lclip
        else
            cargo build --release
            src=target/release/lclip
        fi
        launchctl bootout "$domain/$label" 2>/dev/null || true
        "$bin" quit 2>/dev/null || true
        mkdir -p "$(dirname "$bin")" "$(dirname "$agent")" "$HOME/Library/Logs"
        install -m755 "$src" "$bin"

        shortcut="${LCLIP_SHORTCUT:-cmd+shift+KeyV}"
        esc() { printf '%s' "$1" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g'; }
        cat >"$agent" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>$label</string>
    <key>ProgramArguments</key>
    <array><string>$(esc "$bin")</string><string>--daemon</string></array>
    <key>EnvironmentVariables</key>
    <dict><key>LCLIP_SHORTCUT</key><string>$(esc "$shortcut")</string></dict>
    <key>RunAtLoad</key><true/>
    <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
    <key>ProcessType</key><string>Interactive</string>
    <key>StandardErrorPath</key><string>$(esc "$HOME/Library/Logs/lclip.log")</string>
</dict>
</plist>
EOF
        launchctl bootstrap "$domain" "$agent"
        echo "installed $bin, running now and at login"
        [ "$shortcut" = none ] || echo "shortcut: $shortcut opens and closes lclip"
        echo "to paste for you, allow lclip under System Settings, Privacy & Security, Accessibility"
        ;;
    uninstall)
        launchctl bootout "$domain/$label" 2>/dev/null || true
        "$bin" quit 2>/dev/null || true
        rm -f "$bin" "$agent"
        echo "removed lclip (history stays in ~/Library/Application Support/lclip)"
        ;;
    *)
        echo "usage: $0 [install|uninstall]" >&2
        exit 2
        ;;
    esac
    exit 0
fi

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
