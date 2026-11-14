# lclip

A clipboard manager for Linux and macOS, written in Rust.

Press a shortcut, click something you copied earlier, and it is pasted where your cursor is.

- Keeps history after the app you copied from closes
- Works on Wayland, X11 and macOS, with a tray or menu bar icon
- Text, links, images, files and rich text
- New screenshots go straight onto the clipboard, ready to paste
- Password manager copies are hidden and cleared after 30 seconds

## Install

```sh
git clone git@github.com:thesarfo/lclip.git
cd lclip
./install.sh
```

Or unpack a release tarball and run `./install.sh` inside it.

On Linux this installs the `lclip` command, an app menu entry, autostart, and on GNOME the shortcut `Super+Shift+V`.
Set `LCLIP_SHORTCUT='<Super>v'` to change it, or `none` to skip it. On other desktops, bind a key to `lclip toggle`.
Remove everything with `./install.sh uninstall`.

On macOS it installs the `lclip` command and a LaunchAgent that starts it at login. lclip listens for `Cmd+Shift+V` itself.
Set `LCLIP_SHORTCUT` before installing to change it, for example `ctrl+alt+KeyV`, or `none` to turn it off.
The release binary is not notarized, so run `xattr -d com.apple.quarantine lclip` in the unpacked folder first.

## Use

| Command | What it does |
| --- | --- |
| `lclip` | Open the window |
| `lclip toggle` | Open or close it. Bind this to a key. |
| `lclip clear` | Clear history. Pinned items stay. |
| `lclip quit` | Stop lclip |

In the window: click or `Enter` pastes, `Shift` pastes plain text, `Ctrl+P` (`Cmd+P` on macOS) pins, `Del` removes, `Esc` closes.

## Notes

- Pasting sends `Ctrl+V` through a virtual keyboard on `/dev/uinput`, with no permission prompt. If your user cannot write to it, the entry stays on the clipboard and you press paste yourself.
- Screenshots are picked up from where your screenshot tool saves them: the folder chosen in `Cmd+Shift+5` (the Desktop by default) on macOS, `~/Pictures/Screenshots` and `~/Pictures` on Linux. Turn it off under Settings, Screenshots. On macOS the file is only saved once the floating thumbnail goes away; turn the thumbnail off in `Cmd+Shift+5`, Options, to get it right away.
- Terminals paste with `Ctrl+Shift+V`. Set it under Settings, Paste keys.
- On macOS, pasting sends `Cmd+V`, which needs Accessibility permission. lclip asks on first run; allow it under System Settings, Privacy & Security, Accessibility. Each build from source is a new binary to macOS, so allow it again after reinstalling.
- History is saved in `~/.local/share/lclip` (`~/Library/Application Support/lclip` on macOS), readable only by you. Secrets are never saved.
- GNOME on Wayland reads the clipboard through XWayland.

## Build

```sh
cargo build --release
```

Bundled fonts are under the SIL Open Font License.
