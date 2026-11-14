mod ctl;
#[cfg(debug_assertions)]
mod demo;
mod icon;
mod ipc;
#[cfg(target_os = "macos")]
mod macos;
#[cfg_attr(target_os = "macos", path = "paster_macos.rs")]
mod paster;
mod persist;
mod screenshots;
mod single;
mod store;
#[cfg_attr(target_os = "macos", path = "tray_macos.rs")]
mod tray;
mod ui;
#[cfg(target_os = "linux")]
mod wayland;
#[cfg(target_os = "linux")]
mod x11;

use std::{
    env,
    os::unix::process::CommandExt,
    process::{self, Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};

use ctl::{Action, Ctl};
use store::{Cmd, Shared, Store};

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

/// A clipboard backend: watches copies and serves `Cmd`s until it fails.
type Backend = fn(Shared, Receiver<Cmd>) -> Result<()>;

#[cfg(target_os = "linux")]
fn backend() -> Result<(&'static str, Backend)> {
    let wayland_session = env::var_os("WAYLAND_DISPLAY").is_some();
    if wayland_session && wayland::supported() {
        Ok(("wayland", wayland::run))
    } else if env::var_os("DISPLAY").is_some() {
        Ok((if wayland_session { "x11 via xwayland" } else { "x11" }, x11::run))
    } else {
        bail!("no usable display: need wlr-data-control on Wayland, or X11/XWayland")
    }
}

#[cfg(target_os = "macos")]
fn backend() -> Result<(&'static str, Backend)> {
    Ok(("macos", macos::run))
}

fn run_daemon() -> Result<()> {
    let (name, run) = backend()?;

    let mut store = Store::new(name.to_string());
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
    screenshots::spawn(Arc::clone(&shared), tx.clone());

    let worker = {
        let shared = Arc::clone(&shared);
        thread::spawn(move || {
            if let Err(e) = run(Arc::clone(&shared), rx) {
                eprintln!("lclip: {e:#}");
                shared.lock().unwrap().backend = "stopped".into();
            }
        })
    };

    #[cfg(target_os = "linux")]
    {
        let _tray = tray::start(Arc::clone(&ctl));
        control(&ctl, &shared, &tx, &worker)
    }
    // AppKit wants the main thread for the menu bar, so the control loop moves aside.
    #[cfg(target_os = "macos")]
    {
        let c = Arc::clone(&ctl);
        thread::spawn(move || {
            if let Err(e) = control(&c, &shared, &tx, &worker) {
                eprintln!("lclip: {e:#}");
            }
            process::exit(0);
        });
        tray::run(ctl)
    }
}

/// Act on requests from the tray, the shortcut and other `lclip` commands until quit.
fn control(ctl: &Ctl, shared: &Shared, tx: &Sender<Cmd>, worker: &JoinHandle<()>) -> Result<()> {
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
                    persist::flush(shared);
                    let _ = std::fs::remove_file(ipc::socket_path());
                    return Ok(());
                }
            }
        }
        window.alive();
        if worker.is_finished() {
            persist::flush(shared);
            return Ok(());
        }
        thread::sleep(Duration::from_millis(40));
    }
}
