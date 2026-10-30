//! Types Cmd+V into the focused app with a synthetic key event.
//!
//! macOS only lets apps with Accessibility permission post key events. The
//! first run asks for it; until it is granted the entry stays on the clipboard
//! and the user presses paste.

use core_foundation::{
    base::TCFType,
    boolean::CFBoolean,
    dictionary::{CFDictionary, CFDictionaryRef},
    string::{CFString, CFStringRef},
};
use core_graphics::{
    event::{CGEvent, CGEventFlags, CGEventTapLocation},
    event_source::{CGEventSource, CGEventSourceStateID},
};

use crate::store::PasteKeys;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> u8;
    static kAXTrustedCheckOptionPrompt: CFStringRef;
}

// kVK_ANSI_V from Carbon's Events.h.
const KEY_V: u16 = 0x09;

/// Whether we may post key events. `prompt` shows the system dialog if not.
fn trusted(prompt: bool) -> bool {
    let key = unsafe { CFString::wrap_under_get_rule(kAXTrustedCheckOptionPrompt) };
    let value = if prompt { CFBoolean::true_value() } else { CFBoolean::false_value() };
    let opts = CFDictionary::from_CFType_pairs(&[(key.as_CFType(), value.as_CFType())]);
    unsafe { AXIsProcessTrustedWithOptions(opts.as_concrete_TypeRef()) != 0 }
}

#[derive(Clone)]
pub struct Paster;

impl Paster {
    pub fn spawn() -> Self {
        if !trusted(true) {
            eprintln!("lclip: allow lclip under Privacy & Security, Accessibility to paste for you");
        }
        Self
    }

    /// The setting is for Linux terminals; every Mac app pastes with Cmd+V.
    pub fn press(&self, _keys: PasteKeys) {
        if !trusted(false) {
            eprintln!("lclip: no Accessibility permission, paste it yourself");
            return;
        }
        let Ok(source) = CGEventSource::new(CGEventSourceStateID::HIDSystemState) else { return };
        for down in [true, false] {
            if let Ok(ev) = CGEvent::new_keyboard_event(source.clone(), KEY_V, down) {
                ev.set_flags(CGEventFlags::CGEventFlagCommand);
                ev.post(CGEventTapLocation::HID);
            }
        }
    }
}
