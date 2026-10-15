//! Types the paste keys into the focused app with the X11 XTest extension.
//!
//! This reaches X11 apps and XWayland apps without any permission prompt.
//! Native Wayland windows ignore it; there the entry is on the clipboard
//! and the user presses paste.

use std::sync::mpsc::{Sender, channel};
use std::thread;

use anyhow::{Context, Result, anyhow};
use x11rb::{
    connection::Connection,
    protocol::{xproto::ConnectionExt as _, xtest::ConnectionExt as _},
    rust_connection::RustConnection,
};

use crate::store::PasteKeys;

const KS_CONTROL: u32 = 0xffe3;
const KS_SHIFT: u32 = 0xffe1;
const KS_V: u32 = 0x0076;
const KS_INSERT: u32 = 0xff63;

#[derive(Clone)]
pub struct Paster {
    tx: Sender<PasteKeys>,
}

impl Paster {
    pub fn spawn() -> Self {
        let (tx, rx) = channel::<PasteKeys>();
        thread::spawn(move || {
            let mut conn: Option<RustConnection> = None;
            while let Ok(keys) = rx.recv() {
                if conn.is_none() {
                    conn = RustConnection::connect(None).ok().map(|(c, _)| c);
                }
                let Some(c) = conn.as_ref() else {
                    eprintln!("lclip: no X display, the entry is on the clipboard, paste it yourself");
                    continue;
                };
                if let Err(e) = press(c, keys) {
                    eprintln!("lclip: paste failed: {e:#}");
                    conn = None;
                }
            }
        });
        Self { tx }
    }

    pub fn press(&self, keys: PasteKeys) {
        let _ = self.tx.send(keys);
    }
}

fn keycode(c: &RustConnection, keysym: u32) -> Result<u8> {
    let setup = c.setup();
    let (min, max) = (setup.min_keycode, setup.max_keycode);
    let map = c.get_keyboard_mapping(min, max - min + 1)?.reply()?;
    let per = map.keysyms_per_keycode as usize;
    map.keysyms
        .chunks(per)
        .position(|k| k.contains(&keysym))
        .map(|i| min + i as u8)
        .ok_or_else(|| anyhow!("no key for keysym {keysym:#x}"))
}

fn press(c: &RustConnection, keys: PasteKeys) -> Result<()> {
    let ctrl = keycode(c, KS_CONTROL)?;
    let shift = keycode(c, KS_SHIFT)?;
    let seq: Vec<u8> = match keys {
        PasteKeys::CtrlV => vec![ctrl, keycode(c, KS_V)?],
        PasteKeys::CtrlShiftV => vec![ctrl, shift, keycode(c, KS_V)?],
        PasteKeys::ShiftInsert => vec![shift, keycode(c, KS_INSERT)?],
    };
    let fake = |ty: u8, code: u8| c.xtest_fake_input(ty, code, 0, 0, 0, 0, 0).context("xtest");
    for &k in &seq {
        fake(2, k)?; // KeyPress
    }
    for &k in seq.iter().rev() {
        fake(3, k)?; // KeyRelease
    }
    c.flush()?;
    Ok(())
}
