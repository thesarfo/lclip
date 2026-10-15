# lclip

a small clipboard manager for linux, written in rust.

it keeps what you copy alive after the source app closes.

- wayland: wlr-data-control
- x11 / xwayland: xfixes selection owner
- skips password manager copies, caps items at 64 mb

```
cargo run --release
```
