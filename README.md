# lclip

A clipboard manager for Linux, written in Rust.

Press a shortcut, click something you copied earlier, and it is pasted where your cursor is.

- Keeps your history after the app you copied from closes
- Works on Wayland and X11, with a tray icon
- Handles text, links, images, files and rich text
- Hides password manager copies and clears them after 30 seconds
- Saves history to disk, privately, and never saves secrets

## Install

**From a release**

```sh
tar xzf lclip-v0.2.0-x86_64-linux.tar.gz
cd lclip-v0.2.0-x86_64-linux
./install.sh
```

**From source**

```sh
git clone git@github.com:thesarfo/lclip.git
cd lclip
./install.sh
```

The installer:

| What | Where |
| --- | --- |
| `lclip` command | `~/.local/bin/lclip` |
| App menu entry and icon | `~/.local/share` |
| Start at login | `~/.config/autostart/lclip.desktop` |
| Shortcut (GNOME) | `Super+Shift+V` runs `lclip toggle` |

Change the shortcut with `LCLIP_SHORTCUT='<Super>v' ./install.sh`, or skip it with `LCLIP_SHORTCUT=none`.
On other desktops, bind any key to `lclip toggle`. To remove everything, run `./install.sh uninstall`.

## Use

Open the window with your shortcut, or run `lclip`.

| Command | What it does |
| --- | --- |
| `lclip` | Open the window. Starts lclip first if it is not running. |
| `lclip toggle` | Open or close the window. Bind this to a key. |
| `lclip --daemon` | Run in the background with a tray icon. This is what autostart uses. |
| `lclip show`, `hide` | Open or close the window. |
| `lclip clear` | Clear history. Pinned items stay. |
| `lclip quit` | Stop lclip. |

**In the window**

| Key | What it does |
| --- | --- |
| Click, or `Enter` | Paste the entry |
| `Shift` + click or `Enter` | Paste as plain text |
| `↑` `↓` | Move |
| `Ctrl+P` | Pin or unpin |
| `Del` | Remove |
| `Esc` | Close |

Type to search. The window also closes when you click away from it.

## How pasting works

Wayland does not let one app type into another, so lclip asks the desktop through the
RemoteDesktop portal. The first time, your desktop asks once whether lclip may send keys.
After you allow it, it remembers.

Terminals paste with `Ctrl+Shift+V`. Pick that under Settings, Paste keys.

If the portal is not available, the entry is still placed on the clipboard and you paste it yourself.

## Your data

- History is saved in `~/.local/share/lclip`, in files only you can read
- Secrets are kept in memory only and cleared after 30 seconds
- Turn saving off under Settings and the saved files are deleted
- Items over 64 MB are skipped, and you can change that limit

## Backends

| Session | How lclip reads the clipboard |
| --- | --- |
| Wayland with `wlr-data-control` (Sway, Hyprland, KDE and others) | Native Wayland |
| GNOME on Wayland | Through XWayland. GNOME does not offer `wlr-data-control`. |
| X11 | Native X11 |

The header of the window shows which one is in use.

## Build

```sh
cargo build --release
```

You need a recent stable Rust. Pushing a tag like `v0.2.1` builds and publishes a release automatically.

## Fonts

Bundled under the SIL Open Font License, with the license texts in `assets/fonts`:
Hanken Grotesk, IBM Plex Mono and Instrument Serif.
