# lclip

A clipboard manager for Linux, written in Rust.

Press a shortcut, click something you copied earlier, and it is pasted where your cursor is.

- Keeps history after the app you copied from closes
- Works on Wayland and X11, with a tray icon
- Text, links, images, files and rich text
- Password manager copies are hidden and cleared after 30 seconds

## Install

```sh
git clone git@github.com:thesarfo/lclip.git
cd lclip
./install.sh
```

Or unpack a release tarball and run `./install.sh` inside it.

This installs the `lclip` command, an app menu entry, autostart, and on GNOME the shortcut `Super+Shift+V`.
Set `LCLIP_SHORTCUT='<Super>v'` to change it, or `none` to skip it. On other desktops, bind a key to `lclip toggle`.
Remove everything with `./install.sh uninstall`.

## Use

| Command | What it does |
| --- | --- |
| `lclip` | Open the window |
| `lclip toggle` | Open or close it. Bind this to a key. |
| `lclip clear` | Clear history. Pinned items stay. |
| `lclip quit` | Stop lclip |

In the window: click or `Enter` pastes, `Shift` pastes plain text, `Ctrl+P` pins, `Del` removes, `Esc` closes.

## Notes

- Pasting sends `Ctrl+V` through a virtual keyboard on `/dev/uinput`, with no permission prompt. If your user cannot write to it, the entry stays on the clipboard and you press paste yourself.
- Terminals paste with `Ctrl+Shift+V`. Set it under Settings, Paste keys.
- History is saved in `~/.local/share/lclip`, readable only by you. Secrets are never saved.
- GNOME on Wayland reads the clipboard through XWayland.

## Build

```sh
cargo build --release
```

Bundled fonts are under the SIL Open Font License.
