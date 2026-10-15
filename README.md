# lclip

a small clipboard manager for linux, written in rust.

it keeps what you copy alive after the source app closes, and shows your history in a small window.

- wayland: wlr-data-control
- x11 / xwayland: xfixes selection owner
- clipboard and primary kept apart
- password manager copies are masked, memory only, cleared after 30 s
- items over 64 mb are skipped

install:

```
./install.sh            # builds, puts lclip in ~/.local/bin, adds menu entry and autostart
./install.sh uninstall
```

then:

```
lclip                   # open the window, or start lclip if it is not running
lclip --hidden          # start in the tray
lclip --daemon          # headless, no window or tray
lclip show|hide|toggle|clear|quit
```

closing the window sends it to the tray. the tray menu has show, clear history and quit.
on gnome the tray needs the appindicator extension (ubuntu ships it on).

keys: `↑↓` move, `↵` paste, `p` pin, `⌫` delete, `/` search.

history is memory only for now.
