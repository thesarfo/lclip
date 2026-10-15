use std::{
    collections::HashMap,
    sync::{Arc, mpsc::Sender},
    time::Duration,
};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Frame, Key, Margin, Pos2,
    Rect, RichText, Sense, Shape, Stroke, StrokeKind, TextureHandle, Vec2,
};

use crate::{
    ctl::{Action, Ctl},
    icon,
    store::{Cmd, Entry, Kind, SECRET_HINT, Sel, Shared},
};

// Somu Labs palette.
const INK: Color32 = Color32::from_rgb(0x18, 0x21, 0x2A);
const SLATE: Color32 = Color32::from_rgb(0x2C, 0x3A, 0x47);
const SAGE: Color32 = Color32::from_rgb(0x7F, 0x9A, 0x7B);
const PAPER: Color32 = Color32::from_rgb(0xF0, 0xEF, 0xEA);
const MIST: Color32 = Color32::from_rgb(0xDC, 0xE4, 0xD8);
const STONE: Color32 = Color32::from_rgb(0x5E, 0x67, 0x6A);
const CARD: Color32 = Color32::from_rgb(0xFA, 0xF9, 0xF6);

const ROW_H: f32 = 56.0;

pub struct Launch {
    pub hidden: bool,
    pub tray: bool,
}

pub fn run(shared: Shared, tx: Sender<Cmd>, ctl: Arc<Ctl>, launch: Launch) -> eframe::Result {
    #[cfg(debug_assertions)]
    if std::env::var_os("LCLIP_DEMO").is_some() {
        debug::seed(&shared);
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([400.0, 600.0])
            .with_min_inner_size([360.0, 420.0])
            .with_title("lclip")
            .with_app_id("lclip")
            .with_visible(!launch.hidden)
            .with_icon(egui::IconData { rgba: icon::rgba(64, false), width: 64, height: 64 }),
        ..Default::default()
    };
    eframe::run_native(
        "lclip",
        options,
        Box::new(move |cc| {
            style(&cc.egui_ctx);
            let ctx = cc.egui_ctx.clone();
            shared.lock().unwrap().set_waker(Arc::new(move || ctx.request_repaint()));
            let ctx = cc.egui_ctx.clone();
            ctl.set_waker(Arc::new(move || ctx.request_repaint()));
            Ok(Box::new(App::new(shared, tx, ctl, launch)))
        }),
    )
}

fn serif(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("serif".into()))
}

fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("medium".into()))
}

fn mono(size: f32) -> FontId {
    FontId::monospace(size)
}

fn fonts(ctx: &egui::Context) {
    let mut f = FontDefinitions::default();
    let files: [(&str, &'static [u8]); 4] = [
        ("hanken", include_bytes!("../assets/fonts/HankenGrotesk-Regular.ttf")),
        ("hanken-medium", include_bytes!("../assets/fonts/HankenGrotesk-Medium.ttf")),
        ("plex-mono", include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf")),
        ("instrument", include_bytes!("../assets/fonts/InstrumentSerif-Regular.ttf")),
    ];
    for (name, bytes) in files {
        f.font_data.insert(name.into(), Arc::new(FontData::from_static(bytes)));
    }
    // egui's built in fonts stay behind ours as the fallback for arrows and symbols.
    f.families.get_mut(&FontFamily::Proportional).unwrap().insert(0, "hanken".into());
    f.families.get_mut(&FontFamily::Monospace).unwrap().insert(0, "plex-mono".into());
    let fallback = f.families[&FontFamily::Proportional].clone();
    for (family, first) in [("serif", "instrument"), ("medium", "hanken-medium")] {
        let mut list = vec![first.to_string()];
        list.extend(fallback.iter().filter(|n| *n != "hanken").cloned());
        f.families.insert(FontFamily::Name(family.into()), list);
    }
    ctx.set_fonts(f);
}

fn style(ctx: &egui::Context) {
    fonts(ctx);
    let mut v = egui::Visuals::light();
    v.panel_fill = CARD;
    v.window_fill = CARD;
    v.extreme_bg_color = PAPER;
    v.override_text_color = Some(INK);
    v.selection.bg_fill = MIST;
    v.selection.stroke = Stroke::new(1.0, SLATE);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, MIST);
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active] {
        w.corner_radius = CornerRadius::same(9);
    }
    v.widgets.inactive.bg_fill = CARD;
    v.widgets.inactive.weak_bg_fill = CARD;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, MIST);
    v.widgets.hovered.weak_bg_fill = MIST;
    ctx.set_visuals(v);
    ctx.style_mut_of(egui::Theme::Light, |s| s.spacing.item_spacing = Vec2::new(8.0, 8.0));
}

struct App {
    shared: Shared,
    tx: Sender<Cmd>,
    query: String,
    tab: Sel,
    selected: Option<u64>,
    fmt: Option<String>,
    settings: bool,
    ctl: Arc<Ctl>,
    visible: bool,
    quitting: bool,
    tray: bool,
    thumbs: HashMap<u64, Option<TextureHandle>>,
    decoded_this_frame: u8,
    reveal: bool,
}

impl App {
    fn new(shared: Shared, tx: Sender<Cmd>, ctl: Arc<Ctl>, launch: Launch) -> Self {
        Self {
            shared,
            tx,
            query: String::new(),
            tab: Sel::Clipboard,
            selected: None,
            fmt: None,
            settings: false,
            ctl,
            visible: !launch.hidden,
            quitting: false,
            tray: launch.tray,
            thumbs: HashMap::new(),
            decoded_this_frame: 0,
            reveal: false,
        }
    }

    /// Tray clicks, `lclip show`, and the window close button.
    fn handle_actions(&mut self, ctx: &egui::Context) {
        use egui::ViewportCommand as V;
        for action in self.ctl.drain() {
            match action {
                Action::Show => self.set_visible(ctx, true),
                Action::Hide => self.set_visible(ctx, false),
                Action::Toggle => self.set_visible(ctx, !self.visible),
                Action::Clear => {
                    let (gone, current) = {
                        let mut s = self.shared.lock().unwrap();
                        (s.clear_unpinned(), s.current)
                    };
                    if current.is_some_and(|c| gone.contains(&c)) {
                        let _ = self.tx.send(Cmd::Clear);
                    }
                    self.selected = None;
                }
                Action::Quit => {
                    self.quitting = true;
                    ctx.send_viewport_cmd(V::Close);
                }
            }
        }
        // Closing the window sends it to the tray instead of quitting.
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting && self.tray {
            ctx.send_viewport_cmd(V::CancelClose);
            self.set_visible(ctx, false);
        }
    }

    fn set_visible(&mut self, ctx: &egui::Context, show: bool) {
        use egui::ViewportCommand as V;
        self.visible = show;
        ctx.send_viewport_cmd(V::Visible(show));
        if show {
            ctx.send_viewport_cmd(V::Minimized(false));
            ctx.send_viewport_cmd(V::Focus);
        }
    }

    fn paste(&self, id: u64, mime: Option<String>) {
        let _ = self.tx.send(Cmd::Use { id, mime });
    }

    fn delete(&mut self, id: u64) {
        let mut s = self.shared.lock().unwrap();
        if s.current == Some(id) {
            let _ = self.tx.send(Cmd::Clear);
        }
        s.remove(id);
        if self.selected == Some(id) {
            self.selected = None;
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Secret countdowns, "x min ago" labels, and tray requests while hidden.
        ctx.request_repaint_after(Duration::from_millis(500));
        self.handle_actions(&ctx);
        self.decoded_this_frame = 0;
        #[cfg(debug_assertions)]
        debug::snapshot(&ctx, &mut self.selected, &mut self.settings, &self.shared);

        let (backend, count) = {
            let s = self.shared.lock().unwrap();
            self.thumbs.retain(|id, _| s.find(*id).is_some());
            (s.backend.clone(), s.list(self.tab).len())
        };

        egui::Panel::top("head")
            .frame(Frame::new().fill(CARD).inner_margin(Margin { left: 20, right: 20, top: 16, bottom: 10 }))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("lclip").font(serif(28.0)).color(INK));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let label = if self.settings { "done" } else { "settings" };
                        if ui.button(RichText::new(label).monospace().size(11.0)).clicked() {
                            self.settings = !self.settings;
                        }
                        Frame::new()
                            .fill(MIST)
                            .corner_radius(13)
                            .inner_margin(Margin::symmetric(10, 4))
                            .show(ui, |ui| {
                                ui.label(RichText::new(format!("● {backend}")).monospace().size(11.0).color(SLATE));
                            });
                    });
                });
            });

        egui::Panel::bottom("foot")
            .frame(Frame::new().fill(CARD).stroke(Stroke::new(1.0, MIST)).inner_margin(Margin::symmetric(20, 12)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let hint = if self.settings {
                        "changes apply right away".to_string()
                    } else {
                        "↵ paste   del delete   p pin   / search".to_string()
                    };
                    ui.label(RichText::new(hint).monospace().size(11.0).color(STONE));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(format!("{count} items")).monospace().size(11.0).color(STONE));
                    });
                });
            });

        egui::CentralPanel::default()
            .frame(Frame::new().fill(CARD).inner_margin(Margin::symmetric(20, 4)))
            .show(ui, |ui| {
                if self.settings {
                    self.settings_view(ui);
                } else {
                    self.history_view(ui, &ctx);
                }
            });
    }
}

impl App {
    fn history_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let search = ui.add(
            egui::TextEdit::singleline(&mut self.query)
                .hint_text("Search history")
                .desired_width(f32::INFINITY)
                .margin(Margin::symmetric(12, 10)),
        );
        if !ctx.egui_wants_keyboard_input() && ctx.input(|i| i.key_pressed(Key::Slash)) {
            search.request_focus();
        }

        ui.horizontal(|ui| {
            for (sel, label) in [(Sel::Clipboard, "CLIPBOARD"), (Sel::Primary, "PRIMARY")] {
                let on = self.tab == sel;
                let text = RichText::new(label).monospace().size(11.0).color(if on { INK } else { STONE });
                let w = (ui.available_width() - ui.spacing().item_spacing.x) / if sel == Sel::Clipboard { 2.0 } else { 1.0 };
                if ui.add_sized([w, 30.0], egui::Button::new(text).selected(on)).clicked() {
                    self.tab = sel;
                    self.selected = None;
                }
            }
        });

        if self.tab == Sel::Primary {
            ui.label(
                RichText::new("Text you select with the mouse. Middle-click pastes the live one. Kept apart from the clipboard.")
                    .size(12.5)
                    .color(STONE),
            );
        }

        // Snapshot so the lock is not held while drawing.
        let q = self.query.to_lowercase();
        let mut entries: Vec<Entry> = {
            let s = self.shared.lock().unwrap();
            s.list(self.tab)
                .iter()
                .filter(|e| q.is_empty() || e.text().is_some_and(|t| t.to_lowercase().contains(&q)))
                .cloned()
                .collect()
        };
        entries.sort_by_key(|e| !e.pinned);

        self.keys(ctx, &entries);

        if entries.is_empty() {
            ui.add_space(70.0);
            ui.vertical_centered(|ui| {
                let msg = if self.query.is_empty() { "Nothing copied yet" } else { "No matches" };
                ui.label(RichText::new(msg).font(serif(30.0)));
                ui.label(
                    RichText::new("Copy something in any app. It shows up here and stays after the app closes.")
                        .color(STONE),
                );
            });
            return;
        }

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for e in &entries {
                self.row(ui, e);
                if self.selected == Some(e.id) {
                    self.detail(ui, e);
                    if std::mem::take(&mut self.reveal) {
                        ui.scroll_to_cursor(Some(egui::Align::BOTTOM));
                    }
                }
            }
        });
    }

    fn keys(&mut self, ctx: &egui::Context, entries: &[Entry]) {
        let (down, up, enter, del, pin, esc) = ctx.input(|i| {
            (
                i.key_pressed(Key::ArrowDown),
                i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::Enter),
                i.key_pressed(Key::Delete),
                i.key_pressed(Key::P) && !i.modifiers.any(),
                i.key_pressed(Key::Escape),
            )
        });
        let typing = ctx.egui_wants_keyboard_input();
        let pos = self.selected.and_then(|id| entries.iter().position(|e| e.id == id));
        if down && !entries.is_empty() {
            self.selected = Some(entries[pos.map_or(0, |p| (p + 1).min(entries.len() - 1))].id);
        }
        if up && !entries.is_empty() {
            self.selected = Some(entries[pos.map_or(0, |p| p.saturating_sub(1))].id);
        }
        if (down || up) && self.selected.is_some() {
            self.reveal = true;
        }
        if esc {
            self.selected = None;
        }
        let Some(id) = self.selected else { return };
        if enter {
            self.paste(id, None);
        }
        if !typing && del {
            self.delete(id);
        }
        if !typing && pin {
            self.shared.lock().unwrap().toggle_pin(id);
        }
    }

    fn row(&mut self, ui: &mut egui::Ui, e: &Entry) {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_H), Sense::click());
        let selected = self.selected == Some(e.id);
        let p = ui.painter();
        p.rect(
            rect,
            12,
            if selected { Color32::WHITE } else { CARD },
            Stroke::new(if selected { 1.5 } else { 1.0 }, if selected { SAGE } else { MIST }),
            StrokeKind::Inside,
        );

        let tile = Rect::from_min_size(rect.min + Vec2::new(10.0, 10.0), Vec2::splat(36.0));
        let (tile_fill, tile_ink) = if e.secret { (INK, PAPER) } else { (MIST, SLATE) };
        p.rect_filled(tile, 10, tile_fill);
        let thumb = if e.kind() == Kind::Image && !e.secret && ui.is_rect_visible(rect) {
            self.thumb(ui.ctx(), e)
        } else {
            None
        };
        if let Some(tex) = thumb {
            cover_image(ui, &tex, tile, 10);
        } else {
            draw_icon(ui.painter(), tile.center(), if e.secret { None } else { Some(e.kind()) }, tile_ink);
        }
        let p = ui.painter();

        let title = if e.secret { String::new() } else { title(e) };
        if e.secret {
            for i in 0..10 {
                p.circle_filled(rect.min + Vec2::new(62.0 + i as f32 * 9.0, 20.0), 2.6, INK);
            }
        }
        p.text(
            rect.min + Vec2::new(58.0, 20.0),
            Align2::LEFT_CENTER,
            title,
            medium(14.5),
            INK,
        );
        let meta = if e.secret { "password manager · memory only".to_string() } else { meta(e) };
        p.text(rect.min + Vec2::new(58.0, 38.0), Align2::LEFT_CENTER, meta, mono(11.0), STONE);

        let right = rect.right_center() - Vec2::new(16.0, 0.0);
        if let Some(t) = e.expires {
            let left = t.saturating_duration_since(std::time::Instant::now()).as_secs();
            p.text(
                right,
                Align2::RIGHT_CENTER,
                format!("clears {}:{:02}", left / 60, left % 60),
                FontId::monospace(11.0),
                SLATE,
            );
        } else if e.pinned {
            p.circle_filled(right, 5.0, SAGE);
        } else {
            p.circle_stroke(right, 4.5, Stroke::new(1.5, MIST));
        }

        if resp.clicked() {
            self.selected = if selected { None } else { Some(e.id) };
            self.fmt = None;
            self.reveal = true;
        }
        if resp.double_clicked() {
            self.paste(e.id, None);
        }
    }

    fn detail(&mut self, ui: &mut egui::Ui, e: &Entry) {
        Frame::new()
            .fill(Color32::WHITE)
            .stroke(Stroke::new(1.0, MIST))
            .corner_radius(12)
            .inner_margin(12)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if let Some(t) = e.text().filter(|_| !e.secret) {
                    Frame::new().fill(PAPER).corner_radius(8).inner_margin(10).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let cut: String = t.chars().take(220).collect();
                        ui.label(RichText::new(cut).size(13.0).color(SLATE));
                    });
                }
                if e.kind() == Kind::Image && !e.secret {
                    if let Some(tex) = self.thumb(ui.ctx(), e) {
                        let w = ui.available_width();
                        let [tw, th] = tex.size();
                        let h = (w * th as f32 / tw as f32).clamp(40.0, 160.0);
                        let (r, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
                        ui.painter().rect_filled(r, 8, PAPER);
                        let fit = Vec2::new(tw as f32, th as f32) * (r.width() / tw as f32).min(r.height() / th as f32);
                        egui::Image::from_texture((tex.id(), fit))
                            .corner_radius(6)
                            .paint_at(ui, Rect::from_center_size(r.center(), fit));
                    }
                }
                ui.label(RichText::new("PASTE AS").monospace().size(11.0).color(STONE));
                for f in e.visible_formats() {
                    let on = self.fmt.as_deref() == Some(f.mime.as_str());
                    let text = format!("{}   {}", f.mime, human(f.data.len()));
                    if ui.selectable_label(on, RichText::new(text).monospace().size(12.0)).clicked() {
                        self.fmt = if on { None } else { Some(f.mime.clone()) };
                    }
                }
                ui.horizontal(|ui| {
                    let paste = egui::Button::new(RichText::new("Paste ↵").color(PAPER)).fill(INK);
                    if ui.add(paste).clicked() {
                        self.paste(e.id, self.fmt.clone());
                    }
                    if ui.button(if e.pinned { "Unpin" } else { "Pin" }).clicked() {
                        self.shared.lock().unwrap().toggle_pin(e.id);
                    }
                    if ui.button(RichText::new("Delete").color(STONE)).clicked() {
                        self.delete(e.id);
                    }
                });
            });
        ui.add_space(4.0);
    }

    fn settings_view(&mut self, ui: &mut egui::Ui) {
        let mut s = self.shared.lock().unwrap();
        let mut set = s.settings.clone();

        let group = |ui: &mut egui::Ui, name: &str, body: &mut dyn FnMut(&mut egui::Ui)| {
            ui.add_space(8.0);
            ui.label(RichText::new(name).monospace().size(11.0).color(STONE));
            Frame::new()
                .fill(Color32::WHITE)
                .stroke(Stroke::new(1.0, MIST))
                .corner_radius(12)
                .inner_margin(Margin::symmetric(12, 6))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    body(ui);
                });
        };
        let row = |ui: &mut egui::Ui, label: &str, hint: Option<&str>, control: &mut dyn FnMut(&mut egui::Ui)| {
            let size = Vec2::new(ui.available_width(), 38.0);
            ui.allocate_ui_with_layout(size, egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.label(label);
                    if let Some(h) = hint {
                        ui.label(RichText::new(h).monospace().size(11.0).color(STONE));
                    }
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| control(ui));
            });
        };

        group(ui, "HISTORY", &mut |ui| {
            row(ui, "Entries to keep", None, &mut |ui| {
                ui.add(egui::DragValue::new(&mut set.max_entries).range(10..=1000));
            });
            row(ui, "Keep for", None, &mut |ui| {
                let label = match set.keep_for.map(|d| d.as_secs() / 3600) {
                    Some(1) => "1 h",
                    Some(24) => "24 h",
                    Some(168) => "7 days",
                    Some(_) => "custom",
                    None => "forever",
                };
                egui::ComboBox::from_id_salt("keep").selected_text(label).show_ui(ui, |ui| {
                    ui.selectable_value(&mut set.keep_for, Some(Duration::from_secs(3600)), "1 h");
                    ui.selectable_value(&mut set.keep_for, Some(Duration::from_secs(24 * 3600)), "24 h");
                    ui.selectable_value(&mut set.keep_for, Some(Duration::from_secs(7 * 24 * 3600)), "7 days");
                    ui.selectable_value(&mut set.keep_for, None, "forever");
                });
            });
            row(ui, "Largest item", Some("bigger copies are skipped"), &mut |ui| {
                let mut mb = set.max_item / (1024 * 1024);
                ui.add(egui::DragValue::new(&mut mb).range(1..=512).suffix(" MB"));
                set.max_item = mb * 1024 * 1024;
            });
        });

        group(ui, "PRIVACY", &mut |ui| {
            row(ui, "Skip password manager copies", None, &mut |ui| {
                ui.checkbox(&mut set.skip_secrets, "");
            });
            row(ui, "Save history to disk", Some("secrets are never saved"), &mut |ui| {
                ui.checkbox(&mut set.persist, "");
            });
            row(ui, "Secrets", Some("memory only, cleared after 30 s"), &mut |_| {});
        });

        if s.settings != set {
            s.settings = set;
            s.touch();
        }
    }
}

fn title(e: &Entry) -> String {
    let first = match e.kind() {
        Kind::Image => "Image".to_string(),
        Kind::Files => {
            let n = e.text().map_or(0, |t| t.lines().count());
            format!("{n} file{}", if n == 1 { "" } else { "s" })
        }
        _ => e.text().map(|t| t.lines().next().unwrap_or("").trim().to_string()).unwrap_or_default(),
    };
    let first = if first.is_empty() { "(empty)".to_string() } else { first };
    if first.chars().count() > 40 {
        format!("{}…", first.chars().take(39).collect::<String>())
    } else {
        first
    }
}

fn meta(e: &Entry) -> String {
    let mut fmts = e.visible_formats();
    let first = fmts.next().map_or("", |f| f.mime.as_str()).split(';').next().unwrap_or("");
    let first = if matches!(first, "UTF8_STRING" | "STRING") { "text/plain" } else { first }.to_string();
    let more = fmts.count();
    let mime = if more > 0 { format!("{first} +{more}") } else { first };
    format!("{mime} · {} · {}", human(e.size()), ago(e))
}

fn human(n: usize) -> String {
    match n {
        0..1024 => format!("{n} B"),
        1024..1_048_576 => format!("{:.1} KB", n as f64 / 1024.0),
        _ => format!("{:.1} MB", n as f64 / 1_048_576.0),
    }
}

fn ago(e: &Entry) -> String {
    let secs = e.at.elapsed().map_or(0, |d| d.as_secs());
    match secs {
        0..10 => "just now".into(),
        10..60 => format!("{secs} s ago"),
        60..3600 => format!("{} min ago", secs / 60),
        _ => format!("{} h ago", secs / 3600),
    }
}

#[allow(dead_code)]
const _: &str = SECRET_HINT;

impl App {
    /// A small texture for an image entry, decoded once on first sight.
    fn thumb(&mut self, ctx: &egui::Context, e: &Entry) -> Option<TextureHandle> {
        if let Some(t) = self.thumbs.get(&e.id) {
            return t.clone();
        }
        // Keep the frame smooth when many images scroll into view at once.
        if self.decoded_this_frame >= 2 {
            ctx.request_repaint();
            return None;
        }
        self.decoded_this_frame += 1;

        let tex = e
            .formats
            .iter()
            .find(|f| matches!(f.mime.as_str(), "image/png" | "image/jpeg" | "image/jpg"))
            .filter(|f| f.data.len() <= 32 * 1024 * 1024)
            .and_then(|f| image::load_from_memory(&f.data).ok())
            .map(|img| {
                let small = img.thumbnail(320, 320).to_rgba8();
                let size = [small.width() as usize, small.height() as usize];
                let color = egui::ColorImage::from_rgba_unmultiplied(size, small.as_raw());
                ctx.load_texture(format!("thumb-{}", e.id), color, egui::TextureOptions::LINEAR)
            });
        self.thumbs.insert(e.id, tex.clone());
        tex
    }
}

/// Fill `rect` with the texture, cropping to cover.
fn cover_image(ui: &egui::Ui, tex: &TextureHandle, rect: Rect, radius: u8) {
    let [w, h] = tex.size();
    let (w, h) = (w as f32, h as f32);
    let side = w.min(h);
    let uv = Rect::from_min_max(
        Pos2::new((w - side) / 2.0 / w, (h - side) / 2.0 / h),
        Pos2::new(1.0 - (w - side) / 2.0 / w, 1.0 - (h - side) / 2.0 / h),
    );
    egui::Image::from_texture((tex.id(), rect.size()))
        .uv(uv)
        .corner_radius(radius)
        .paint_at(ui, rect);
}

/// Drawn on an 18 px grid centred on `c`. `None` kind is the lock for secrets.
fn draw_icon(p: &egui::Painter, c: Pos2, kind: Option<Kind>, color: Color32) {
    let o = c - Vec2::splat(9.0);
    let pt = |x: f32, y: f32| o + Vec2::new(x, y);
    let st = Stroke::new(1.6, color);
    let line = |pts: &[(f32, f32)]| {
        p.add(Shape::line(pts.iter().map(|&(x, y)| pt(x, y)).collect(), st));
    };
    match kind {
        Some(Kind::Text) => {
            line(&[(2.0, 4.0), (16.0, 4.0)]);
            line(&[(2.0, 9.0), (16.0, 9.0)]);
            line(&[(2.0, 14.0), (10.0, 14.0)]);
        }
        Some(Kind::Link) => {
            line(&[(5.0, 13.0), (13.0, 5.0)]);
            line(&[(6.0, 5.0), (13.0, 5.0), (13.0, 12.0)]);
        }
        Some(Kind::Image) => {
            p.rect_stroke(Rect::from_min_size(pt(2.0, 3.0), Vec2::new(14.0, 12.0)), 2, st, StrokeKind::Inside);
            line(&[(2.0, 12.0), (6.0, 8.0), (9.0, 11.0), (11.0, 9.0), (16.0, 14.0)]);
        }
        Some(Kind::Files) => {
            p.add(Shape::closed_line(
                [(2.0, 4.0), (7.0, 4.0), (9.0, 6.5), (16.0, 6.5), (16.0, 15.0), (2.0, 15.0)]
                    .iter()
                    .map(|&(x, y)| pt(x, y))
                    .collect(),
                st,
            ));
        }
        Some(Kind::Rich) => {
            p.text(c, Align2::CENTER_CENTER, "Aa", serif(20.0), color);
        }
        None => {
            p.rect_stroke(Rect::from_min_size(pt(3.0, 8.0), Vec2::new(12.0, 8.0)), 2, st, StrokeKind::Inside);
            let arc: Vec<Pos2> = (0..=12)
                .map(|i| {
                    let a = std::f32::consts::PI * (1.0 + i as f32 / 12.0);
                    pt(9.0 + 3.0 * a.cos(), 6.0 + 3.0 * a.sin())
                })
                .collect();
            p.add(Shape::line(arc, st));
            line(&[(6.0, 8.0), (6.0, 6.0)]);
            line(&[(12.0, 8.0), (12.0, 6.0)]);
        }
    }
}

/// Debug builds only: demo data and a self screenshot, for checking the layout without a screen grabber.
#[cfg(debug_assertions)]
mod debug {
    use std::{io::Cursor, sync::Arc};

    use eframe::egui;

    use crate::store::{Format, SECRET_HINT, Sel, Shared};

    fn fmt(mime: &str, data: &[u8]) -> Format {
        Format { mime: mime.into(), data: Arc::new(data.to_vec()) }
    }

    pub fn seed(shared: &Shared) {
        let mut png = Vec::new();
        let img = image::RgbaImage::from_fn(240, 140, |x, y| {
            image::Rgba([(x as f32 / 240.0 * 160.0) as u8 + 40, (y as f32 / 140.0 * 120.0) as u8 + 80, 150, 255])
        });
        img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png).unwrap();

        let mut s = shared.lock().unwrap();
        s.settings.skip_secrets = false;
        s.settings.persist = false;
        s.push(Sel::Clipboard, vec![fmt("text/plain", b"wlr-data-control")], false);
        s.push(Sel::Primary, vec![fmt("text/plain", b"selected with the mouse")], false);
        s.push(
            Sel::Clipboard,
            vec![
                fmt("text/html", b"<h1>Gye Payments design brief</h1><p>Direction A, light.</p>"),
                fmt("text/plain", b"Gye Payments design brief. Direction A, light, with the Somu Labs palette."),
            ],
            false,
        );
        s.push(Sel::Clipboard, vec![fmt("text/uri-list", b"file:///a/main.rs\nfile:///a/ui.rs\nfile:///a/x11.rs")], false);
        s.push(Sel::Clipboard, vec![fmt("image/png", &png)], false);
        s.push(Sel::Clipboard, vec![fmt("text/plain", b"https://github.com/blackprince001/skills")], false);
        s.push(Sel::Clipboard, vec![fmt("text/plain", b"cargo build --release")], false);
        let id = s.push(
            Sel::Clipboard,
            vec![fmt("text/plain", b"hunter2"), fmt(SECRET_HINT, b"secret")],
            true,
        );
        if let Some(e) = s.clipboard.iter_mut().find(|e| Some(e.id) == id) {
            e.pinned = false;
        }
    }

    /// LCLIP_VIEW = history | detail | settings, LCLIP_SNAPSHOT = output png.
    pub fn snapshot(ctx: &egui::Context, selected: &mut Option<u64>, settings: &mut bool, shared: &Shared) {
        let Some(path) = std::env::var_os("LCLIP_SNAPSHOT") else { return };
        let frame = ctx.cumulative_frame_nr();
        if frame == 2 {
            match std::env::var("LCLIP_VIEW").as_deref() {
                Ok("settings") => *settings = true,
                Ok("detail") => {
                    *selected = shared.lock().unwrap().clipboard.iter().find(|e| e.kind() == crate::store::Kind::Rich).map(|e| e.id)
                }
                Ok("image") => {
                    *selected = shared.lock().unwrap().clipboard.iter().find(|e| e.kind() == crate::store::Kind::Image).map(|e| e.id)
                }
                _ => {}
            }
        }
        if frame == 30 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
        ctx.request_repaint();
        let shot = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = shot {
            let [w, h] = img.size;
            let raw = image::RgbaImage::from_raw(w as u32, h as u32, img.as_raw().to_vec()).unwrap();
            raw.save(path).unwrap();
            std::process::exit(0);
        }
    }
}
