//! Types the paste keys into the focused app through a virtual keyboard on
//! /dev/uinput.
//!
//! The kernel hands the keys to the compositor like any other keyboard, so it
//! works in X11 and native Wayland windows and never asks for permission.
//! It needs write access to /dev/uinput, which most desktops give the
//! logged-in user. Without it the entry stays on the clipboard and the user
//! presses paste.

use std::sync::mpsc::{Sender, channel};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use evdev::{AttributeSet, EventType, InputEvent, KeyCode, uinput::VirtualDevice};

use crate::store::PasteKeys;

#[derive(Clone)]
pub struct Paster {
    tx: Sender<PasteKeys>,
}

impl Paster {
    pub fn spawn() -> Self {
        let (tx, rx) = channel::<PasteKeys>();
        thread::spawn(move || {
            let mut dev = open()
                .map_err(|e| eprintln!("lclip: no virtual keyboard ({e:#}), paste it yourself"))
                .ok();
            while let Ok(keys) = rx.recv() {
                let Some(d) = dev.as_mut() else { continue };
                if let Err(e) = press(d, keys) {
                    eprintln!("lclip: paste failed: {e:#}");
                }
            }
        });
        Self { tx }
    }

    pub fn press(&self, keys: PasteKeys) {
        let _ = self.tx.send(keys);
    }
}

fn open() -> Result<VirtualDevice> {
    let mut keys = AttributeSet::<KeyCode>::new();
    for k in [KeyCode::KEY_LEFTCTRL, KeyCode::KEY_LEFTSHIFT, KeyCode::KEY_V, KeyCode::KEY_INSERT] {
        keys.insert(k);
    }
    let dev = VirtualDevice::builder()
        .context("/dev/uinput")?
        .name("lclip paste")
        .with_keys(&keys)?
        .build()?;
    // The compositor needs a moment to notice the new device.
    thread::sleep(Duration::from_millis(300));
    Ok(dev)
}

fn press(dev: &mut VirtualDevice, keys: PasteKeys) -> Result<()> {
    let seq = match keys {
        PasteKeys::CtrlV => vec![KeyCode::KEY_LEFTCTRL, KeyCode::KEY_V],
        PasteKeys::CtrlShiftV => vec![KeyCode::KEY_LEFTCTRL, KeyCode::KEY_LEFTSHIFT, KeyCode::KEY_V],
        PasteKeys::ShiftInsert => vec![KeyCode::KEY_LEFTSHIFT, KeyCode::KEY_INSERT],
    };
    let ev = |k: KeyCode, v: i32| InputEvent::new(EventType::KEY.0, k.code(), v);
    for &k in &seq {
        dev.emit(&[ev(k, 1)])?;
    }
    for &k in seq.iter().rev() {
        dev.emit(&[ev(k, 0)])?;
    }
    Ok(())
}
