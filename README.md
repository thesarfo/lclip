# lclip

a small clipboard manager for linux, written in rust.

it keeps what you copy alive after the source app closes, and shows your history in a small window.

- wayland: wlr-data-control
- x11 / xwayland: xfixes selection owner
- clipboard and primary kept apart
- password manager copies are masked, memory only, cleared after 30 s
- items over 64 mb are skipped

```
cargo run --release              # window + daemon
cargo run --release -- --daemon  # daemon only
```

keys: `↑↓` move, `↵` paste, `p` pin, `⌫` delete, `/` search.

history is memory only for now.
