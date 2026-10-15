use std::{collections::VecDeque, sync::Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Show,
    Hide,
    Toggle,
    Clear,
    Quit,
    /// Internal: the window picked an entry, send the paste keys.
    Paste,
}

impl Action {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim() {
            "show" => Self::Show,
            "hide" => Self::Hide,
            "toggle" => Self::Toggle,
            "clear" => Self::Clear,
            "quit" => Self::Quit,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Show => "show",
            Self::Hide => "hide",
            Self::Toggle => "toggle",
            Self::Clear => "clear",
            Self::Quit => "quit",
            Self::Paste => "paste",
        }
    }
}

/// Requests from outside the window (tray, other `lclip` invocations).
#[derive(Default)]
pub struct Ctl {
    queue: Mutex<VecDeque<Action>>,
}

impl Ctl {
    pub fn send(&self, action: Action) {
        self.queue.lock().unwrap().push_back(action);
    }

    pub fn drain(&self) -> Vec<Action> {
        self.queue.lock().unwrap().drain(..).collect()
    }
}
