mod ctl;
#[cfg(debug_assertions)]
mod demo;
mod icon;
mod ipc;
mod paster;
mod persist;
mod single;
mod store;
mod tray;
mod ui;
mod wayland;
mod x11;

use std::{
    env,
    os::unix::process::CommandExt,
    process::{self, Child, Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};

use ctl::{Action, Ctl};
use store::{Cmd, Store};

const USAGE: &str = "\
lclip: clipboard manager

usage:
  lclip             open the history window (starts lclip if it is not running)
  lclip toggle      open or close the window; bind this to a shortcut
  lclip --daemon    run in the background with a tray icon (what autostart uses)
  lclip show|hide|clear|quit
                    control a running lclip
";

fn main() -> Result<()> {
    let mut daemon = false;
    let mut window = false;
    let mut action = None;
    for a in env::args().skip(1) {
        match a.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(());
            }
            "-V" | "--version" => {
                println!("lclip {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--daemon" | "--hidden" => daemon = true,
            "window" => window = true,
            other => match Action::parse(other) {
                Some(act) => action = Some(act),
                None => {
                    eprint!("lclip: unknown argument {other}\n\n{USAGE}");
                    process::exit(2);
                }
            },
        }
    }

    if window {
        return ui::run().map_err(|e| anyhow::anyhow!("{e}"));
    }
    if daemon {
        if single::running() {
            return Ok(());
        }
        return run_daemon();
    }
    client(action)
}

/// `lclip`, `lclip toggle`, ...: talk to the daemon, starting it if needed.
fn client(action: Option<Action>) -> Result<()> {
    let request = action.unwrap_or(Action::Show);
    if single::send(request) {
        return Ok(());
    }
    if matches!(request, Action::Hide | Action::Clear | Action::Quit) {
        eprintln!("lclip is not running");
        return Ok(());
    }

    Command::new(env::current_exe()?)
        .arg("--daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .context("could not start lclip")?;

    let deadline = Instant::now() + Duration::from_secs(5);
    while !single::running() {
        if Instant::now() > deadline {
            bail!("lclip did not start");
        }
        thread::sleep(Duration::from_millis(50));
    }
    single::send(request);
    Ok(())
}

/// The history window is its own process, so it is native, crisp and cheap when closed.
#[derive(Default)]
struct Window {
    child: Option<Child>,
}

impl Window {
    fn alive(&mut self) -> bool {
        match self.child.as_mut().map(|c| c.try_wait()) {
            Some(Ok(None)) => true,
            _ => {
                self.child = None;
                false
            }
        }
    }

    fn show(&mut self) {
        if self.alive() {
            return;
        }
        match env::current_exe().and_then(|exe| Command::new(exe).arg("window").spawn()) {
            Ok(c) => self.child = Some(c),
            Err(e) => eprintln!("lclip: cannot open the window: {e}"),
        }
    }

    fn hide(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn run_daemon() -> Result<()> {
    let wayland_session = env::var_os("WAYLAND_DISPLAY").is_some();
    let use_wayland = wayland_session && wayland::supported();
    let backend = if use_wayland {
        "wayland"
    } else if env::var_os("DISPLAY").is_some() {
        if wayland_session { "x11 via xwayland" } else { "x11" }
    } else {
        bail!("no usable display: need wlr-data-control on Wayland, or X11/XWayland")
    };

    let mut store = Store::new(backend.to_string());
    persist::load(&mut store);
    let shared = Arc::new(Mutex::new(store));
    #[cfg(debug_assertions)]
    if env::var_os("LCLIP_DEMO").is_some() {
        demo::seed(&shared);
    }
    persist::spawn(Arc::clone(&shared));

    let ctl = Arc::new(Ctl::default());
    let (tx, rx) = mpsc::channel::<Cmd>();
    single::listen(Arc::clone(&ctl), Arc::clone(&shared), tx.clone());

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

    let _tray = tray::start(Arc::clone(&ctl));
    let paster = paster::Paster::spawn();
    let mut window = Window::default();

    loop {
        for a in ctl.drain() {
            match a {
                Action::Show => window.show(),
                Action::Hide => window.hide(),
                Action::Toggle => {
                    if window.alive() {
                        window.hide();
                    } else {
                        window.show();
                    }
                }
                Action::Clear => {
                    let (gone, current) = {
                        let mut s = shared.lock().unwrap();
                        (s.clear_unpinned(), s.current)
                    };
                    if current.is_some_and(|c| gone.contains(&c)) {
                        let _ = tx.send(Cmd::Clear);
                    }
                }
                Action::Paste => {
                    // Let focus go back to the app that had it before the popup.
                    window.hide();
                    thread::sleep(Duration::from_millis(160));
                    let keys = shared.lock().unwrap().settings.paste_keys;
                    paster.press(keys);
                }
                Action::Quit => {
                    window.hide();
                    persist::flush(&shared);
                    let _ = std::fs::remove_file(ipc::socket_path());
                    return Ok(());
                }
            }
        }
        window.alive();
        if worker.is_finished() {
            persist::flush(&shared);
            return Ok(());
        }
        thread::sleep(Duration::from_millis(40));
    }
}
