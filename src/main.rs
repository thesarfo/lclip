mod wayland;
mod x11;

use anyhow::{Result, bail};

fn main() -> Result<()> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() && wayland::supported() {
        return wayland::run();
    }
    if std::env::var_os("DISPLAY").is_some() {
        return x11::run();
    }
    bail!("no usable display: need wlr-data-control on Wayland, or X11/XWayland")
}
