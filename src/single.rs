use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    sync::{Arc, mpsc::Sender},
    thread,
};

use crate::{
    ctl::{Action, Ctl},
    ipc::{EntryView, Request, Snapshot, socket_path},
    store::{Cmd, Shared},
};

/// Hand an action to an already running daemon. False means none is running.
pub fn send(action: Action) -> bool {
    let Ok(mut stream) = UnixStream::connect(socket_path()) else { return false };
    stream.write_all(format!("{}\n", action.name()).as_bytes()).is_ok()
}

pub fn running() -> bool {
    UnixStream::connect(socket_path()).is_ok()
}

/// Serve actions from `lclip toggle` and requests from the window.
pub fn listen(ctl: Arc<Ctl>, shared: Shared, tx: Sender<Cmd>) {
    let path = socket_path();
    // A stale socket from a crashed run would block bind; callers already proved nobody answers.
    let _ = std::fs::remove_file(&path);
    let Ok(listener) = UnixListener::bind(&path) else {
        eprintln!("lclip: cannot listen on {}", path.display());
        return;
    };
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (ctl, shared, tx) = (Arc::clone(&ctl), Arc::clone(&shared), tx.clone());
            thread::spawn(move || handle(stream, &ctl, &shared, &tx));
        }
    });
}

fn handle(mut stream: UnixStream, ctl: &Ctl, shared: &Shared, tx: &Sender<Cmd>) {
    let mut line = String::new();
    if BufReader::new(&stream).take_line(&mut line).is_err() {
        return;
    }
    let line = line.trim();

    if !line.starts_with('{') {
        if let Some(a) = Action::parse(line) {
            ctl.send(a);
        }
        return;
    }
    let Ok(req) = serde_json::from_str::<Request>(line) else { return };

    let reply: Vec<u8> = match req {
        Request::Snapshot => {
            let s = shared.lock().unwrap();
            let snap = Snapshot {
                backend: s.backend.clone(),
                current: s.current,
                settings: Some(s.settings.clone()),
                clipboard: s.clipboard.iter().map(EntryView::of).collect(),
                primary: s.primary.iter().map(EntryView::of).collect(),
            };
            serde_json::to_vec(&snap).unwrap_or_default()
        }
        Request::Use { id, mime, paste } => {
            let _ = tx.send(Cmd::Use { id, mime });
            if paste {
                ctl.send(Action::Paste);
            }
            Vec::new()
        }
        Request::Pin { id } => {
            shared.lock().unwrap().toggle_pin(id);
            Vec::new()
        }
        Request::Delete { id } => {
            let mut s = shared.lock().unwrap();
            if s.current == Some(id) {
                let _ = tx.send(Cmd::Clear);
            }
            s.remove(id);
            Vec::new()
        }
        Request::Clear => {
            ctl.send(Action::Clear);
            Vec::new()
        }
        Request::Settings { mut settings } => {
            settings.max_entries = settings.max_entries.clamp(10, 1000);
            settings.max_item = settings.max_item.clamp(1024 * 1024, 512 * 1024 * 1024);
            let mut s = shared.lock().unwrap();
            if s.settings != settings {
                s.settings = settings;
                s.touch();
            }
            Vec::new()
        }
        Request::Blob { id, mime } => {
            let s = shared.lock().unwrap();
            s.find(id)
                .and_then(|e| e.formats.iter().find(|f| f.mime == mime))
                .map(|f| f.data.to_vec())
                .unwrap_or_default()
        }
    };
    let _ = stream.write_all(&reply);
}

/// `BufRead::read_line` without letting a client send an endless line.
trait TakeLine {
    fn take_line(self, out: &mut String) -> std::io::Result<usize>;
}

impl<R: std::io::Read> TakeLine for BufReader<R> {
    fn take_line(self, out: &mut String) -> std::io::Result<usize> {
        use std::io::Read;
        BufReader::new(self.take(256 * 1024)).read_line(out)
    }
}
