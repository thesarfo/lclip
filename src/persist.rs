use std::{
    collections::HashSet,
    env, fs,
    io::Write,
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::Arc,
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};

use crate::store::{Entry, Format, Settings, Shared, Store};

#[derive(Serialize, Deserialize)]
struct Index {
    next_id: u64,
    entries: Vec<EntryRec>,
}

#[derive(Serialize, Deserialize)]
struct EntryRec {
    id: u64,
    at: u64,
    pinned: bool,
    formats: Vec<FormatRec>,
}

#[derive(Serialize, Deserialize)]
struct FormatRec {
    mime: String,
    file: String,
}

#[derive(Serialize, Deserialize)]
struct SettingsRec {
    max_entries: usize,
    keep_for_secs: Option<u64>,
    max_item: usize,
    skip_secrets: bool,
    persist: bool,
}

/// What the writer needs, copied out of the store so the lock is not held during disk io.
pub struct Snapshot {
    pub settings: Settings,
    pub next_id: u64,
    pub entries: Vec<Entry>,
}

fn dir() -> PathBuf {
    env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(env::temp_dir)
        .join("lclip")
}

fn secs(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Everything here is private to the user.
fn private_dir(path: &Path) -> std::io::Result<()> {
    fs::DirBuilder::new().recursive(true).mode(0o700).create(path)
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    fs::rename(tmp, path)
}

pub fn load(store: &mut Store) {
    let root = dir();
    if let Some(rec) = fs::read(root.join("settings.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<SettingsRec>(&b).ok())
    {
        store.settings = Settings {
            max_entries: rec.max_entries.clamp(10, 1000),
            keep_for: rec.keep_for_secs.map(Duration::from_secs),
            max_item: rec.max_item.max(1024 * 1024),
            skip_secrets: rec.skip_secrets,
            persist: rec.persist,
        };
    }
    if !store.settings.persist {
        return;
    }
    let Some(index) = fs::read(root.join("history.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Index>(&b).ok())
    else {
        return;
    };

    let entries = index
        .entries
        .into_iter()
        .filter_map(|rec| {
            let formats: Vec<Format> = rec
                .formats
                .into_iter()
                .filter_map(|f| {
                    let data = fs::read(root.join("blobs").join(&f.file)).ok()?;
                    Some(Format { mime: f.mime, data: Arc::new(data) })
                })
                .collect();
            (!formats.is_empty()).then(|| Entry {
                id: rec.id,
                formats,
                at: UNIX_EPOCH + Duration::from_secs(rec.at),
                pinned: rec.pinned,
                secret: false,
                expires: None,
            })
        })
        .collect();
    store.restore(entries, index.next_id);
}

pub fn save(snap: &Snapshot) {
    if let Err(e) = try_save(snap) {
        eprintln!("lclip: could not save history: {e}");
    }
}

fn try_save(snap: &Snapshot) -> std::io::Result<()> {
    let root = dir();
    private_dir(&root)?;

    let s = &snap.settings;
    let rec = SettingsRec {
        max_entries: s.max_entries,
        keep_for_secs: s.keep_for.map(|d| d.as_secs()),
        max_item: s.max_item,
        skip_secrets: s.skip_secrets,
        persist: s.persist,
    };
    write_private(&root.join("settings.json"), &serde_json::to_vec_pretty(&rec)?)?;

    let blobs = root.join("blobs");
    if !s.persist {
        // Turning it off removes what was saved.
        let _ = fs::remove_file(root.join("history.json"));
        let _ = fs::remove_dir_all(&blobs);
        return Ok(());
    }
    private_dir(&blobs)?;

    let mut keep = HashSet::new();
    let mut entries = Vec::new();
    for e in snap.entries.iter().filter(|e| !e.secret) {
        let mut formats = Vec::new();
        for (n, f) in e.formats.iter().enumerate() {
            let file = format!("{}-{}.bin", e.id, n);
            let path = blobs.join(&file);
            if !path.exists() {
                write_private(&path, &f.data)?;
            }
            keep.insert(file.clone());
            formats.push(FormatRec { mime: f.mime.clone(), file });
        }
        entries.push(EntryRec { id: e.id, at: secs(e.at), pinned: e.pinned, formats });
    }
    let index = Index { next_id: snap.next_id, entries };
    write_private(&root.join("history.json"), &serde_json::to_vec(&index)?)?;

    for f in fs::read_dir(&blobs)?.flatten() {
        if !keep.contains(f.file_name().to_string_lossy().as_ref()) {
            let _ = fs::remove_file(f.path());
        }
    }
    Ok(())
}

/// Write pending changes now. Called on the way out.
pub fn flush(shared: &Shared) {
    let snap = shared.lock().unwrap().take_dirty();
    if let Some(snap) = snap {
        save(&snap);
    }
}

/// Save a couple of seconds after the last change.
pub fn spawn(shared: Shared) {
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_secs(2));
            flush(&shared);
        }
    });
}
