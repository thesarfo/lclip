use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime},
};

// Offered on our own sources so a backend can tell its re-offer from someone else's copy.
pub const OWNER_MARK: &str = "x-lclip/owner";
// Password managers set this to say "do not keep this".
pub const SECRET_HINT: &str = "x-kde-passwordManagerHint";
// How long a secret stays in memory.
pub const SECRET_TTL: Duration = Duration::from_secs(30);

/// Protocol atoms that describe a selection but carry no content.
pub fn is_data_mime(mime: &str) -> bool {
    !matches!(mime, "TARGETS" | "TIMESTAMP" | "MULTIPLE" | "SAVE_TARGETS" | "DELETE" | OWNER_MARK)
}

#[derive(Clone)]
pub struct Format {
    pub mime: String,
    pub data: Arc<Vec<u8>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Link,
    Rich,
    Image,
    Files,
}

#[derive(Clone)]
pub struct Entry {
    pub id: u64,
    pub formats: Vec<Format>,
    pub at: SystemTime,
    pub pinned: bool,
    pub secret: bool,
    pub expires: Option<Instant>,
}

impl Entry {
    /// Formats worth showing as paste targets.
    pub fn visible_formats(&self) -> impl Iterator<Item = &Format> {
        self.formats.iter().filter(|f| f.mime != SECRET_HINT)
    }

    pub fn text(&self) -> Option<String> {
        ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "STRING", "text/uri-list"]
            .iter()
            .find_map(|m| self.formats.iter().find(|f| f.mime == *m))
            .map(|f| String::from_utf8_lossy(&f.data).into_owned())
    }

    pub fn kind(&self) -> Kind {
        let has = |p: &str| self.formats.iter().any(|f| f.mime.starts_with(p));
        if has("image/") {
            Kind::Image
        } else if has("text/uri-list") {
            Kind::Files
        } else if has("text/html") {
            Kind::Rich
        } else if self
            .text()
            .is_some_and(|t| t.starts_with("http://") || t.starts_with("https://"))
        {
            Kind::Link
        } else {
            Kind::Text
        }
    }

    pub fn size(&self) -> usize {
        self.visible_formats().map(|f| f.data.len()).sum()
    }

    fn same_content(&self, formats: &[Format]) -> bool {
        self.formats.len() == formats.len()
            && self
                .formats
                .iter()
                .zip(formats)
                .all(|(a, b)| a.mime == b.mime && a.data == b.data)
    }
}

pub enum Cmd {
    /// Make an entry the live clipboard, optionally offering only one format.
    Use { id: u64, mime: Option<String> },
    Clear,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Sel {
    Clipboard,
    Primary,
}

#[derive(Clone, PartialEq)]
pub struct Settings {
    pub max_entries: usize,
    pub keep_for: Option<Duration>,
    pub max_item: usize,
    pub skip_secrets: bool,
    /// Keep history on disk between runs.
    pub persist: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            max_entries: 100,
            keep_for: Some(Duration::from_secs(24 * 3600)),
            max_item: 64 * 1024 * 1024,
            skip_secrets: false,
            persist: true,
        }
    }
}

pub struct Store {
    pub clipboard: Vec<Entry>,
    pub primary: Vec<Entry>,
    pub settings: Settings,
    pub backend: String,
    /// Entry currently offered as the clipboard, if it is one of ours.
    pub current: Option<u64>,
    next_id: u64,
    waker: Option<Arc<dyn Fn() + Send + Sync>>,
    dirty: bool,
}

pub type Shared = Arc<Mutex<Store>>;

impl Store {
    pub fn new(backend: String) -> Self {
        Self {
            clipboard: Vec::new(),
            primary: Vec::new(),
            settings: Settings::default(),
            backend,
            current: None,
            next_id: 1,
            waker: None,
            dirty: false,
        }
    }

    pub fn set_waker(&mut self, waker: Arc<dyn Fn() + Send + Sync>) {
        self.waker = Some(waker);
    }

    fn wake(&mut self) {
        self.dirty = true;
        if let Some(w) = &self.waker {
            w();
        }
    }

    /// Settings changed from the window.
    pub fn touch(&mut self) {
        self.wake();
    }

    /// History loaded from disk, newest first.
    pub fn restore(&mut self, entries: Vec<Entry>, next_id: u64) {
        self.clipboard = entries;
        self.next_id = self.next_id.max(next_id);
        self.sweep();
        self.dirty = false;
    }

    /// A copy of what should be on disk, if anything changed since the last call.
    pub fn take_dirty(&mut self) -> Option<crate::persist::Snapshot> {
        if !std::mem::take(&mut self.dirty) {
            return None;
        }
        Some(crate::persist::Snapshot {
            settings: self.settings.clone(),
            next_id: self.next_id,
            entries: self.clipboard.clone(),
        })
    }

    pub fn list(&self, sel: Sel) -> &Vec<Entry> {
        match sel {
            Sel::Clipboard => &self.clipboard,
            Sel::Primary => &self.primary,
        }
    }

    fn list_mut(&mut self, sel: Sel) -> &mut Vec<Entry> {
        match sel {
            Sel::Clipboard => &mut self.clipboard,
            Sel::Primary => &mut self.primary,
        }
    }

    pub fn find(&self, id: u64) -> Option<&Entry> {
        self.clipboard.iter().chain(&self.primary).find(|e| e.id == id)
    }

    /// Add a copy. None means it was refused (a secret while skipping secrets).
    pub fn push(&mut self, sel: Sel, formats: Vec<Format>, secret: bool) -> Option<u64> {
        if secret && self.settings.skip_secrets {
            return None;
        }
        let max = self.settings.max_entries;
        let list = self.list_mut(sel);
        if let Some(i) = list.iter().position(|e| e.same_content(&formats)) {
            let mut e = list.remove(i);
            e.at = SystemTime::now();
            let id = e.id;
            list.insert(0, e);
            self.wake();
            return Some(id);
        }

        let id = self.next_id;
        self.next_id += 1;
        let list = self.list_mut(sel);
        list.insert(
            0,
            Entry {
                id,
                formats,
                at: SystemTime::now(),
                pinned: false,
                secret,
                expires: secret.then(|| Instant::now() + SECRET_TTL),
            },
        );
        while list.len() > max {
            match list.iter().rposition(|e| !e.pinned) {
                Some(i) => {
                    list.remove(i);
                }
                None => break,
            }
        }
        self.wake();
        Some(id)
    }

    pub fn remove(&mut self, id: u64) {
        self.clipboard.retain(|e| e.id != id);
        self.primary.retain(|e| e.id != id);
        self.wake();
    }

    /// Drop everything that is not pinned. Returns the removed ids.
    pub fn clear_unpinned(&mut self) -> Vec<u64> {
        let mut gone = Vec::new();
        for list in [&mut self.clipboard, &mut self.primary] {
            list.retain(|e| {
                if !e.pinned {
                    gone.push(e.id);
                }
                e.pinned
            });
        }
        self.wake();
        gone
    }

    pub fn toggle_pin(&mut self, id: u64) {
        for e in self.clipboard.iter_mut().chain(self.primary.iter_mut()) {
            if e.id == id {
                e.pinned = !e.pinned;
            }
        }
        self.wake();
    }

    /// Drop expired secrets and entries past `keep_for`. Returns the removed ids.
    pub fn sweep(&mut self) -> Vec<u64> {
        let now = Instant::now();
        let keep_for = self.settings.keep_for;
        let mut gone = Vec::new();
        for list in [&mut self.clipboard, &mut self.primary] {
            list.retain(|e| {
                let old = keep_for
                    .zip(e.at.elapsed().ok())
                    .is_some_and(|(limit, age)| age > limit && !e.pinned);
                let expired = e.expires.is_some_and(|t| t <= now);
                if old || expired {
                    gone.push(e.id);
                }
                !(old || expired)
            });
        }
        if !gone.is_empty() {
            self.wake();
        }
        gone
    }
}
