# lclip

a small clipboard manager for linux, written in rust.

it keeps what you copy alive after the source app closes, and shows your history in a small window.

- wayland: wlr-data-control
- x11 / xwayland: xfixes selection owner
- clipboard and primary kept apart
- password manager copies are masked, memory only, cleared after 30 s
- items over 64 mb are skipped

## install

```
# from a release
tar xzf lclip-v*-x86_64-linux.tar.gz && cd lclip-v*-x86_64-linux && ./install.sh

# from source
./install.sh
./install.sh uninstall
```

this puts `lclip` in `~/.local/bin`, adds a menu entry, starts it at login, and on gnome binds
**Super+Shift+V** to open and close the window (`LCLIP_SHORTCUT='<Super>v' ./install.sh` to change it).
on other desktops bind a key to `lclip toggle`.

## use

```
lclip           # open the window (starts lclip if it is not running)
lclip toggle    # open or close it, bind this to a key
lclip --daemon  # run in the background with a tray icon
lclip show|hide|clear|quit
```

click an entry and it is pasted where your cursor is. on wayland this goes through the remote desktop
portal, so the first time your desktop asks once whether lclip may send keys. terminals need
ctrl shift v, set that under settings.

keys: `↑↓` move, `enter` paste, `shift+enter` paste as plain text, `ctrl+p` pin, `del` remove, `esc` close.
the window closes when you click away from it.

history is saved to `~/.local/share/lclip` (private files, mode 600). secrets are never saved.

## fonts

bundled under the sil open font license (texts in `assets/fonts`):
hanken grotesk, ibm plex mono, instrument serif.
