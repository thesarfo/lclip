mod ctl;
mod icon;
mod single;
mod store;
mod tray;
mod ui;
mod wayland;
mod x11;

use std::{
    env, process,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

use anyhow::{Result, bail};

use ctl::{Action, Ctl};
use store::{Cmd, Store};

const USAGE: &str = "\
lclip: clipboard manager

usage:
  lclip             open the window (starts lclip if it is not running)
  lclip --hidden    start in the tray without a window
  lclip --daemon    run headless, no window or tray
  lclip show|hide|toggle|clear|quit
                    control a running lclip
";

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    let mut hidden = false;
    let mut headless = false;
    let mut action = None;
    for a in &args {
        match a.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(());
            }
            "-V" | "--version" => {
                println!("lclip {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--hidden" => hidden = true,
            "--daemon" => headless = true,
            other => match Action::parse(other) {
                Some(act) => action = Some(act),
                None => {
                    eprint!("lclip: unknown argument {other}\n\n{USAGE}");
                    process::exit(2);
                }
            },
        }
    }

    // A second invocation just talks to the first.
    let request = action.unwrap_or(if hidden || headless { Action::Hide } else { Action::Show });
    if single::send(request) {
        return Ok(());
    }
    if matches!(action, Some(Action::Hide | Action::Clear | Action::Quit)) {
        eprintln!("lclip is not running");
        return Ok(());
    }

    let wayland_session = env::var_os("WAYLAND_DISPLAY").is_some();
    let use_wayland = wayland_session && wayland::supported();
    let has_x11 = env::var_os("DISPLAY").is_some();
    let backend = if use_wayland {
        "wayland"
    } else if has_x11 {
        if wayland_session { "x11 via xwayland" } else { "x11" }
    } else {
        bail!("no usable display: need wlr-data-control on Wayland, or X11/XWayland")
    };

    // A native Wayland window cannot be hidden to the tray; an XWayland one can.
    // The wayland backend needs the variable, so only drop it on the X11 path.
    if !use_wayland && has_x11 && !headless {
        // SAFETY: still single threaded here.
        unsafe { env::remove_var("WAYLAND_DISPLAY") };
    }

    let shared = Arc::new(Mutex::new(Store::new(backend.to_string())));
    let ctl = Arc::new(Ctl::default());
    let (tx, rx) = mpsc::channel::<Cmd>();

    single::listen(Arc::clone(&ctl));

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
        loop {
            for a in ctl.drain() {
                match a {
                    Action::Quit => return Ok(()),
                    Action::Clear => {
                        let gone = shared.lock().unwrap().clear_unpinned();
                        let current = shared.lock().unwrap().current;
                        if current.is_some_and(|c| gone.contains(&c)) {
                            let _ = tx.send(Cmd::Clear);
                        }
                    }
                    _ => {}
                }
            }
            if worker.is_finished() {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(200));
        }
    }

    let tray = tray::start(Arc::clone(&ctl));
    let result = ui::run(shared, tx, ctl, ui::Launch { hidden, tray: tray.is_some() });
    drop(tray);
    result.map_err(|e| anyhow::anyhow!("{e}"))
}
