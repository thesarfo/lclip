use std::{
    env,
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    sync::Arc,
    thread,
};

use crate::ctl::{Action, Ctl};

fn socket_path() -> PathBuf {
    let dir = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir);
    dir.join("lclip.sock")
}

/// Hand an action to an already running instance. False means none is running.
pub fn send(action: Action) -> bool {
    let Ok(mut stream) = UnixStream::connect(socket_path()) else { return false };
    stream.write_all(format!("{}\n", action.name()).as_bytes()).is_ok()
}

/// Listen for actions from later `lclip` invocations.
pub fn listen(ctl: Arc<Ctl>) {
    let path = socket_path();
    // A stale socket from a crashed run would block bind; `send` already proved nobody answers.
    let _ = std::fs::remove_file(&path);
    let Ok(listener) = UnixListener::bind(&path) else {
        eprintln!("lclip: cannot listen on {}", path.display());
        return;
    };
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                if let Some(a) = Action::parse(&line) {
                    ctl.send(a);
                }
            }
        }
    });
}
