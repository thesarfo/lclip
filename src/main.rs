mod store;
mod ui;
mod wayland;
mod x11;

use std::{
    env,
    sync::{Arc, Mutex, mpsc},
    thread,
};

use anyhow::{Result, bail};

use store::Store;

fn main() -> Result<()> {
    let headless = env::args().any(|a| a == "--daemon");
    let wayland_session = env::var_os("WAYLAND_DISPLAY").is_some();
    let use_wayland = wayland_session && wayland::supported();

    let backend = if use_wayland {
        "wayland"
    } else if env::var_os("DISPLAY").is_some() {
        if wayland_session { "x11 via xwayland" } else { "x11" }
    } else {
        bail!("no usable display: need wlr-data-control on Wayland, or X11/XWayland")
    };

    let shared = Arc::new(Mutex::new(Store::new(backend.to_string())));
    let (tx, rx) = mpsc::channel();

    let worker = {
        let shared = Arc::clone(&shared);
        thread::spawn(move || {
            let result = if use_wayland {
                wayland::run(Arc::clone(&shared), rx)
            } else {
                x11::run(Arc::clone(&shared), rx)
            };
            if let Err(e) = result {
                eprintln!("lclip: {e:#}");
                shared.lock().unwrap().backend = "stopped".into();
            }
        })
    };

    if headless {
        let _ = worker.join();
        return Ok(());
    }
    ui::run(shared, tx).map_err(|e| anyhow::anyhow!("{e}"))
}
