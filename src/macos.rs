//! The macOS clipboard. The pasteboard sends no change events, so we poll its
//! change count. It keeps its own copy of what was copied, so unlike X11 there
//! is nothing to serve after the source app quits.

use std::{
    sync::{Arc, mpsc::Receiver},
    thread::sleep,
    time::{Duration, Instant},
};

use anyhow::Result;
use objc2::{rc::{Retained, autoreleasepool}, runtime::ProtocolObject};
use objc2_app_kit::{NSPasteboard, NSPasteboardItem};
use objc2_foundation::{NSArray, NSData, NSString};

use crate::store::{Cmd, Format, SECRET_HINT, Sel, Shared};

const TICK: Duration = Duration::from_millis(200);

/// Pasteboard types we keep, and the mime names the rest of lclip uses for them.
const TYPES: &[(&str, &str)] = &[
    ("public.utf8-plain-text", "text/plain;charset=utf-8"),
    ("public.html", "text/html"),
    ("public.rtf", "text/rtf"),
    ("public.png", "image/png"),
    ("public.jpeg", "image/jpeg"),
    ("public.tiff", "image/tiff"),
];
const FILE_URL: &str = "public.file-url";
const URI_LIST: &str = "text/uri-list";

// Markers from nspasteboard.org that password managers set on copies they want forgotten.
const CONCEALED: &str = "org.nspasteboard.ConcealedType";
const TRANSIENT: &str = "org.nspasteboard.TransientType";

fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

fn uti_of(mime: &str) -> Option<&'static str> {
    if matches!(mime, "UTF8_STRING" | "STRING") || mime.starts_with("text/plain") {
        return Some("public.utf8-plain-text");
    }
    TYPES.iter().find(|(_, m)| *m == mime).map(|(u, _)| *u)
}

struct Ctx {
    pb: Retained<NSPasteboard>,
    shared: Shared,
    /// Last change count we looked at.
    seen: isize,
    /// Change count right after our own write, so we don't record it again.
    ours: Option<isize>,
}

pub fn run(shared: Shared, rx: Receiver<Cmd>) -> Result<()> {
    let pb = NSPasteboard::generalPasteboard();
    let seen = pb.changeCount();
    let mut ctx = Ctx { pb, shared, seen, ours: None };

    eprintln!("lclip running (macos)");
    let mut last_sweep = Instant::now();
    loop {
        autoreleasepool(|_| {
            let now = ctx.pb.changeCount();
            if now != ctx.seen {
                ctx.seen = now;
                if ctx.ours != Some(now) {
                    ctx.grab();
                }
            }
            while let Ok(cmd) = rx.try_recv() {
                match cmd {
                    Cmd::Use { id, mime } => ctx.offer(id, mime.as_deref()),
                    Cmd::Clear => ctx.release(),
                }
            }
            if last_sweep.elapsed() >= Duration::from_millis(500) {
                last_sweep = Instant::now();
                ctx.sweep();
            }
        });
        sleep(TICK);
    }
}

impl Ctx {
    fn grab(&mut self) {
        let max = self.shared.lock().unwrap().settings.max_item;
        let Some(items) = self.pb.pasteboardItems() else { return };
        let Some(first) = items.iter().next() else { return };

        let types: Vec<String> = first.types().iter().map(|t| t.to_string()).collect();
        let secret = types.iter().any(|t| t == CONCEALED || t == TRANSIENT);

        let files: Vec<String> =
            items.iter().filter_map(|i| i.stringForType(&ns(FILE_URL))).map(|s| s.to_string()).collect();

        let mut formats = Vec::new();
        for (uti, mime) in TYPES {
            // Finder puts the file icon next to copied files; that is not the content.
            if !files.is_empty() && mime.starts_with("image/") {
                continue;
            }
            let Some(data) = first.dataForType(&ns(uti)) else { continue };
            let bytes = data.to_vec();
            if !bytes.is_empty() && bytes.len() <= max {
                formats.push(Format { mime: mime.to_string(), data: Arc::new(bytes) });
            }
        }
        if !files.is_empty() {
            let list = files.join("\r\n") + "\r\n";
            formats.push(Format { mime: URI_LIST.into(), data: Arc::new(list.into_bytes()) });
        }
        if formats.is_empty() {
            return;
        }
        if secret {
            formats.push(Format { mime: SECRET_HINT.into(), data: Arc::new(b"secret".to_vec()) });
        }

        let mut s = self.shared.lock().unwrap();
        s.current = s.push(Sel::Clipboard, formats, secret);
    }

    /// Put entry `id` on the pasteboard, or just one of its formats.
    fn offer(&mut self, id: u64, only: Option<&str>) {
        let formats: Vec<Format> = {
            let s = self.shared.lock().unwrap();
            let Some(e) = s.find(id) else { return };
            e.formats
                .iter()
                .filter(|f| only.is_none_or(|m| f.mime == m) || f.mime == SECRET_HINT)
                .cloned()
                .collect()
        };

        // Each file is its own pasteboard item; the other formats ride on the first one.
        let mut items: Vec<Retained<NSPasteboardItem>> = Vec::new();
        if let Some(list) = formats.iter().find(|f| f.mime == URI_LIST) {
            for line in String::from_utf8_lossy(&list.data).lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                let item = NSPasteboardItem::new();
                item.setString_forType(&ns(line), &ns(FILE_URL));
                items.push(item);
            }
        }
        if items.is_empty() {
            items.push(NSPasteboardItem::new());
        }
        let head = &items[0];
        for f in &formats {
            if f.mime == SECRET_HINT {
                head.setString_forType(&ns(""), &ns(CONCEALED));
            } else if let Some(uti) = uti_of(&f.mime) {
                head.setData_forType(&NSData::with_bytes(&f.data), &ns(uti));
            }
        }

        let objects: Vec<Retained<ProtocolObject<_>>> =
            items.into_iter().map(ProtocolObject::from_retained).collect();
        self.pb.clearContents();
        self.pb.writeObjects(&NSArray::from_retained_slice(&objects));
        self.seen = self.pb.changeCount();
        self.ours = Some(self.seen);
        self.shared.lock().unwrap().current = Some(id);
    }

    /// Empty the pasteboard, but only if it still holds what we last saw.
    fn release(&mut self) {
        if self.pb.changeCount() == self.seen {
            self.pb.clearContents();
            self.seen = self.pb.changeCount();
            self.ours = Some(self.seen);
        }
        self.shared.lock().unwrap().current = None;
    }

    fn sweep(&mut self) {
        let (gone, current) = {
            let mut s = self.shared.lock().unwrap();
            (s.sweep(), s.current)
        };
        if current.is_some_and(|c| gone.contains(&c)) {
            self.release();
        }
    }
}
