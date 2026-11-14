//! Puts each new screenshot on the clipboard, so you can paste it right after taking it.
//!
//! Screenshot tools save a file. We watch the folders they save to and pick up
//! new images once they stop growing. Files already there when lclip starts,
//! or while the setting is off, are left alone.

use std::{
    collections::{HashMap, HashSet},
    env, fs,
    path::{Path, PathBuf},
    sync::{Arc, mpsc::Sender},
    thread,
    time::Duration,
};

use crate::store::{Cmd, Format, Kind, Sel, Shared};

const TICK: Duration = Duration::from_millis(500);
// Folders can move (macOS lets you pick one), so look them up again now and then.
const RESCAN_EVERY: u32 = 20;

pub fn spawn(shared: Shared, tx: Sender<Cmd>) {
    thread::spawn(move || {
        let mut dirs = folders();
        let mut seen: HashSet<PathBuf> = images(&dirs).map(|(p, _)| p).collect();
        // Size at the last tick, for files that may still be being written.
        let mut growing: HashMap<PathBuf, u64> = HashMap::new();
        let mut ticks = 0u32;

        loop {
            thread::sleep(TICK);
            ticks += 1;
            if ticks.is_multiple_of(RESCAN_EVERY) {
                let now = folders();
                if now != dirs {
                    dirs = now;
                    seen.extend(images(&dirs).map(|(p, _)| p));
                }
            }

            if !shared.lock().unwrap().settings.copy_screenshots {
                seen.extend(images(&dirs).map(|(p, _)| p));
                growing.clear();
                continue;
            }

            for (path, len) in images(&dirs) {
                if seen.contains(&path) {
                    continue;
                }
                if len == 0 || growing.get(&path) != Some(&len) {
                    growing.insert(path, len);
                    continue;
                }
                growing.remove(&path);
                seen.insert(path.clone());
                if is_screenshot(&path) {
                    take(&shared, &tx, &path);
                }
            }
        }
    });
}

fn take(shared: &Shared, tx: &Sender<Cmd>, path: &Path) {
    let Some(mime) = mime_of(path) else { return };
    let Ok(data) = fs::read(path) else { return };

    let id = {
        let mut s = shared.lock().unwrap();
        if data.len() > s.settings.max_item {
            return;
        }
        // Some tools (GNOME's among them) copy the screenshot themselves; don't add it twice.
        let just_copied = s.clipboard.first().is_some_and(|e| {
            e.kind() == Kind::Image && e.at.elapsed().is_ok_and(|age| age < Duration::from_secs(5))
        });
        if just_copied {
            return;
        }
        s.push(Sel::Clipboard, vec![Format { mime: mime.into(), data: Arc::new(data) }], false)
    };
    if let Some(id) = id {
        let _ = tx.send(Cmd::Use { id, mime: None });
    }
}

fn mime_of(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "tif" | "tiff" => "image/tiff",
        _ => return None,
    })
}

/// Visible image files in `dirs`, with their sizes.
fn images(dirs: &[PathBuf]) -> impl Iterator<Item = (PathBuf, u64)> + '_ {
    dirs.iter()
        .filter_map(|d| fs::read_dir(d).ok())
        .flatten()
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter(|e| mime_of(&e.path()).is_some())
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            // Skip anything old that shows up, like a file moved in from elsewhere.
            let fresh = m.modified().ok()?.elapsed().is_ok_and(|age| age < Duration::from_secs(60));
            (m.is_file() && fresh).then(|| (e.path(), m.len()))
        })
}

fn home() -> PathBuf {
    env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

#[cfg(target_os = "macos")]
fn expand(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None if p == "~" => home(),
        None => PathBuf::from(p),
    }
}

/// Where the system screenshot tool saves: your pick in Cmd+Shift+5, or the Desktop.
#[cfg(target_os = "macos")]
fn folders() -> Vec<PathBuf> {
    let picked = std::process::Command::new("defaults")
        .args(["read", "com.apple.screencapture", "location"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty());
    vec![picked.map_or_else(|| home().join("Desktop"), |p| expand(&p))]
}

/// macOS tags its screenshots in the file's metadata, whatever the language of the name.
#[cfg(target_os = "macos")]
fn is_screenshot(path: &Path) -> bool {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    unsafe extern "C" {
        fn getxattr(
            path: *const std::ffi::c_char,
            name: *const std::ffi::c_char,
            value: *mut std::ffi::c_void,
            size: usize,
            position: u32,
            options: std::ffi::c_int,
        ) -> isize;
    }
    let Ok(p) = CString::new(path.as_os_str().as_bytes()) else { return false };
    let name = c"com.apple.metadata:kMDItemIsScreenCapture";
    unsafe { getxattr(p.as_ptr(), name.as_ptr(), std::ptr::null_mut(), 0, 0, 0) >= 0 }
}

/// `~/Pictures/Screenshots` (GNOME) and `~/Pictures` (KDE and most others).
#[cfg(not(target_os = "macos"))]
fn folders() -> Vec<PathBuf> {
    let pictures = pictures_dir().unwrap_or_else(|| home().join("Pictures"));
    vec![pictures.join("Screenshots"), pictures]
}

/// XDG_PICTURES_DIR from ~/.config/user-dirs.dirs, which is often translated.
#[cfg(not(target_os = "macos"))]
fn pictures_dir() -> Option<PathBuf> {
    let config = env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config"));
    let text = fs::read_to_string(config.join("user-dirs.dirs")).ok()?;
    let line = text.lines().find_map(|l| l.trim().strip_prefix("XDG_PICTURES_DIR="))?;
    let value = line.trim_matches('"');
    let path = match value.strip_prefix("$HOME") {
        Some(rest) => home().join(rest.trim_start_matches('/')),
        None => PathBuf::from(value),
    };
    (path != home()).then_some(path)
}

/// Everything in a Screenshots folder counts; elsewhere the name has to say so.
#[cfg(not(target_os = "macos"))]
fn is_screenshot(path: &Path) -> bool {
    let in_screenshots = path.parent().and_then(|p| p.file_name()).is_some_and(|n| n == "Screenshots");
    let named = path.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().contains("screenshot"));
    in_screenshots || named
}
