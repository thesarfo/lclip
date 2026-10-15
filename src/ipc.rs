//! How the window talks to the daemon: one JSON line in, one reply out, per connection.

use std::{
    env,
    io::{Read, Write},
    net::Shutdown,
    os::unix::net::UnixStream,
    path::PathBuf,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use crate::store::{Entry, Kind, Settings};

pub fn socket_path() -> PathBuf {
    env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir)
        .join("lclip.sock")
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Snapshot,
    /// Make an entry the clipboard. `paste` also types it into the focused app.
    Use { id: u64, mime: Option<String>, paste: bool },
    Pin { id: u64 },
    Delete { id: u64 },
    Clear,
    Settings { settings: Settings },
    /// Raw bytes of one format, for thumbnails.
    Blob { id: u64, mime: String },
}

#[derive(Serialize, Deserialize, Clone)]
pub struct FormatView {
    pub mime: String,
    pub len: usize,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct EntryView {
    pub id: u64,
    pub age_secs: u64,
    pub pinned: bool,
    pub secret: bool,
    pub expires_in_ms: Option<u64>,
    pub kind: Kind,
    pub title: String,
    pub text: Option<String>,
    pub formats: Vec<FormatView>,
    pub size: usize,
}

impl EntryView {
    pub fn of(e: &Entry) -> Self {
        Self {
            id: e.id,
            age_secs: e.at.elapsed().map_or(0, |d| d.as_secs()),
            pinned: e.pinned,
            secret: e.secret,
            expires_in_ms: e
                .expires
                .map(|t| t.saturating_duration_since(Instant::now()).as_millis() as u64),
            kind: e.kind(),
            title: if e.secret { String::new() } else { e.title() },
            text: if e.secret { None } else { e.text().map(|t| t.chars().take(400).collect()) },
            formats: e
                .visible_formats()
                .map(|f| FormatView { mime: f.mime.clone(), len: f.data.len() })
                .collect(),
            size: e.size(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Snapshot {
    pub backend: String,
    pub current: Option<u64>,
    pub settings: Option<Settings>,
    pub clipboard: Vec<EntryView>,
    pub primary: Vec<EntryView>,
}

/// Send one request and read the whole reply.
pub fn call(req: &Request) -> std::io::Result<Vec<u8>> {
    let mut s = UnixStream::connect(socket_path())?;
    s.set_read_timeout(Some(Duration::from_secs(5)))?;
    s.write_all(&serde_json::to_vec(req)?)?;
    s.write_all(b"\n")?;
    s.shutdown(Shutdown::Write)?;
    let mut out = Vec::new();
    s.take(64 * 1024 * 1024).read_to_end(&mut out)?;
    Ok(out)
}

pub fn snapshot() -> Option<Snapshot> {
    serde_json::from_slice(&call(&Request::Snapshot).ok()?).ok()
}
