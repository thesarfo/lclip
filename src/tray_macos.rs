//! Menu bar icon and the global shortcut. Both need AppKit's run loop on the
//! main thread, so `run` takes the main thread over for good.

use std::{env, sync::Arc};

use anyhow::{Context, Result};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use tray_icon::{
    Icon, TrayIconBuilder,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
};

use crate::{
    ctl::{Action, Ctl},
    icon,
};

/// `LCLIP_SHORTCUT`, e.g. `cmd+shift+V` or `ctrl+alt+V`. `none` turns it off.
const DEFAULT_SHORTCUT: &str = "cmd+shift+KeyV";

pub fn run(ctl: Arc<Ctl>) -> Result<()> {
    let mtm = MainThreadMarker::new().context("the tray must start on the main thread")?;
    let app = NSApplication::sharedApplication(mtm);
    // No Dock icon, no app menu: lclip lives in the menu bar.
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let _hotkeys = shortcut(Arc::clone(&ctl));

    let menu = Menu::new();
    let items = [
        MenuItem::with_id("show", "Show lclip", true, None),
        MenuItem::with_id("clear", "Clear history", true, None),
        MenuItem::with_id("quit", "Quit lclip", true, None),
    ];
    menu.append_items(&[&items[0], &items[1], &PredefinedMenuItem::separator(), &items[2]])?;
    {
        let ctl = Arc::clone(&ctl);
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let action = match e.id.0.as_str() {
                "show" => Action::Show,
                "clear" => Action::Clear,
                "quit" => Action::Quit,
                _ => return,
            };
            ctl.send(action);
        }));
    }

    let size = 36;
    let _tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon_templated(Icon::from_rgba(icon::rgba(size, true), size, size)?)
        .with_tooltip("lclip")
        .build()
        .map_err(|e| eprintln!("lclip: no menu bar icon ({e})"))
        .ok();

    app.run();
    Ok(())
}

fn shortcut(ctl: Arc<Ctl>) -> Option<GlobalHotKeyManager> {
    let spec = env::var("LCLIP_SHORTCUT").unwrap_or_else(|_| DEFAULT_SHORTCUT.into());
    if spec.eq_ignore_ascii_case("none") {
        return None;
    }
    let hotkey: HotKey = match spec.parse() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("lclip: bad LCLIP_SHORTCUT {spec:?}: {e}");
            return None;
        }
    };
    let manager = GlobalHotKeyManager::new().ok()?;
    if let Err(e) = manager.register(hotkey) {
        eprintln!("lclip: cannot use shortcut {spec}: {e}");
        return None;
    }
    GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
        if e.state == HotKeyState::Pressed {
            ctl.send(Action::Toggle);
        }
    }));
    Some(manager)
}
