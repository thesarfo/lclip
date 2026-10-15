use std::sync::Arc;

use ksni::{
    MenuItem,
    blocking::{Handle, TrayMethods},
    menu::StandardItem,
};

use crate::{
    ctl::{Action, Ctl},
    icon,
};

pub struct Tray {
    ctl: Arc<Ctl>,
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "lclip".into()
    }

    fn title(&self) -> String {
        "lclip".into()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        [22, 32, 48]
            .into_iter()
            .map(|s| ksni::Icon { width: s as i32, height: s as i32, data: icon::argb(s, true) })
            .collect()
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.ctl.send(Action::Toggle);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let item = |label: &str, action: Action| -> MenuItem<Self> {
            StandardItem {
                label: label.into(),
                activate: Box::new(move |t: &mut Self| t.ctl.send(action)),
                ..Default::default()
            }
            .into()
        };
        vec![
            item("Show lclip", Action::Toggle),
            item("Clear history", Action::Clear),
            MenuItem::Separator,
            item("Quit", Action::Quit),
        ]
    }
}

/// Start the tray icon. None when there is no session bus or no tray host.
pub fn start(ctl: Arc<Ctl>) -> Option<Handle<Tray>> {
    match (Tray { ctl }).spawn() {
        Ok(handle) => Some(handle),
        Err(e) => {
            eprintln!("lclip: no tray ({e}); closing the window will quit");
            None
        }
    }
}
