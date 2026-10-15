//! The history window. A separate, frameless process that asks the daemon for state.
//! Click an entry and it is typed into whatever had focus before the window opened.

use std::{
    collections::{HashMap, HashSet},
    process,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::Duration,
};

use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Key, Margin, Pos2, Rect,
    Response, Sense, Shape, Stroke, StrokeKind, TextureHandle, UiBuilder, Vec2,
};

use crate::{
    icon,
    ipc::{self, EntryView, Request, Snapshot},
    store::{Kind, PasteKeys, Sel},
};

// Somu Labs palette.
const INK: Color32 = Color32::from_rgb(0x18, 0x21, 0x2A);
const SLATE: Color32 = Color32::from_rgb(0x2C, 0x3A, 0x47);
const SAGE: Color32 = Color32::from_rgb(0x7F, 0x9A, 0x7B);
const PAPER: Color32 = Color32::from_rgb(0xF0, 0xEF, 0xEA);
const MIST: Color32 = Color32::from_rgb(0xDC, 0xE4, 0xD8);
const STONE: Color32 = Color32::from_rgb(0x5E, 0x67, 0x6A);
const CARD: Color32 = Color32::from_rgb(0xFA, 0xF9, 0xF6);

const W: f32 = 400.0;
const H: f32 = 620.0;
const PAD: f32 = 20.0;
const ROW_H: f32 = 60.0;

pub fn run() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([W, H])
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_title("lclip")
            .with_app_id("lclip")
            .with_icon(egui::IconData { rgba: icon::rgba(64, false), width: 64, height: 64 }),
        ..Default::default()
    };
    eframe::run_native(
        "lclip",
        options,
        Box::new(|cc| {
            fonts(&cc.egui_ctx);
            style(&cc.egui_ctx);
            Ok(Box::new(App::new(&cc.egui_ctx)))
        }),
    )
}

// ---- type ---------------------------------------------------------------------------------

fn serif(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("serif".into()))
}

fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("medium".into()))
}

fn body(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
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
    // egui's built in fonts stay behind ours as the fallback for symbols.
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
    let mut v = egui::Visuals::light();
    v.panel_fill = Color32::TRANSPARENT;
    v.window_fill = CARD;
    v.extreme_bg_color = PAPER;
    v.override_text_color = Some(INK);
    v.selection.bg_fill = MIST;
    v.selection.stroke = Stroke::new(1.0, SLATE);
    ctx.set_visuals(v);
}

// ---- state --------------------------------------------------------------------------------

struct App {
    snap: Arc<Mutex<Option<Snapshot>>>,
    tab: Sel,
    query: String,
    cursor: usize,
    settings: bool,
    thumbs: HashMap<u64, TextureHandle>,
    asked: HashSet<u64>,
    thumb_tx: Sender<(u64, egui::ColorImage)>,
    thumb_rx: Receiver<(u64, egui::ColorImage)>,
    was_focused: bool,
    frames: u64,
}

impl App {
    fn new(ctx: &egui::Context) -> Self {
        let snap: Arc<Mutex<Option<Snapshot>>> = Arc::default();
        let (thumb_tx, thumb_rx) = mpsc::channel();

        let (poll_snap, poll_ctx) = (Arc::clone(&snap), ctx.clone());
        thread::spawn(move || {
            let mut misses = 0;
            loop {
                match ipc::snapshot() {
                    Some(s) => {
                        misses = 0;
                        *poll_snap.lock().unwrap() = Some(s);
                        poll_ctx.request_repaint();
                    }
                    None => {
                        misses += 1;
                        // The daemon is gone; a window with nothing behind it is useless.
                        if misses >= 6 {
                            process::exit(0);
                        }
                    }
                }
                thread::sleep(Duration::from_millis(300));
            }
        });

        Self {
            snap,
            tab: Sel::Clipboard,
            query: String::new(),
            cursor: 0,
            settings: false,
            thumbs: HashMap::new(),
            asked: HashSet::new(),
            thumb_tx,
            thumb_rx,
            was_focused: false,
            frames: 0,
        }
    }

    /// Fire a request without blocking the frame, then refresh.
    fn act(&self, ctx: &egui::Context, req: Request) {
        let (snap, ctx) = (Arc::clone(&self.snap), ctx.clone());
        thread::spawn(move || {
            let _ = ipc::call(&req);
            if let Some(s) = ipc::snapshot() {
                *snap.lock().unwrap() = Some(s);
                ctx.request_repaint();
            }
        });
    }

    /// Put the entry on the clipboard and type it into the previous app, then get out of the way.
    fn pick(&self, e: &EntryView, plain: bool) -> ! {
        let mime = plain
            .then(|| {
                ["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "STRING"]
                    .iter()
                    .find(|m| e.formats.iter().any(|f| f.mime == **m))
                    .map(|m| m.to_string())
            })
            .flatten();
        let _ = ipc::call(&Request::Use { id: e.id, mime, paste: true });
        process::exit(0)
    }

    fn want_thumb(&mut self, ctx: &egui::Context, e: &EntryView) {
        if e.kind != Kind::Image || e.secret || self.asked.contains(&e.id) {
            return;
        }
        let Some(f) = e
            .formats
            .iter()
            .find(|f| matches!(f.mime.as_str(), "image/png" | "image/jpeg" | "image/jpg"))
            .filter(|f| f.len <= 32 * 1024 * 1024)
        else {
            return;
        };
        self.asked.insert(e.id);
        let (id, mime, tx, ctx) = (e.id, f.mime.clone(), self.thumb_tx.clone(), ctx.clone());
        thread::spawn(move || {
            let Ok(bytes) = ipc::call(&Request::Blob { id, mime }) else { return };
            let Ok(img) = image::load_from_memory(&bytes) else { return };
            let small = img.thumbnail(256, 256).to_rgba8();
            let size = [small.width() as usize, small.height() as usize];
            let _ = tx.send((id, egui::ColorImage::from_rgba_unmultiplied(size, small.as_raw())));
            ctx.request_repaint();
        });
    }
}

impl eframe::App for App {
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.frames += 1;
        ctx.request_repaint_after(Duration::from_millis(500));

        while let Ok((id, img)) = self.thumb_rx.try_recv() {
            let tex = ctx.load_texture(format!("thumb-{id}"), img, egui::TextureOptions::LINEAR);
            self.thumbs.insert(id, tex);
        }

        let full = ui.max_rect();
        ui.painter().rect(full, 16, CARD, Stroke::new(1.0, MIST), StrokeKind::Inside);

        // A launcher closes when you click away from it.
        let focused = ctx.input(|i| i.viewport().focused).unwrap_or(false);
        let keep_open = cfg!(debug_assertions) && std::env::var_os("LCLIP_SNAPSHOT").is_some();
        if self.was_focused && !focused && !keep_open {
            process::exit(0);
        }
        self.was_focused |= focused;

        #[cfg(debug_assertions)]
        debug::hook(&ctx, self.frames, &mut self.settings, &mut self.query);

        let snap = self.snap.lock().unwrap().clone();
        let Some(snap) = snap else {
            ui.painter().text(full.center(), Align2::CENTER_CENTER, "Connecting…", body(14.0), STONE);
            return;
        };

        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            if self.settings {
                self.settings = false;
            } else {
                process::exit(0);
            }
        }

        self.header(ui, &ctx, &snap, full);
        if self.settings {
            self.settings_view(ui, &ctx, &snap, full);
        } else {
            self.history(ui, &ctx, &snap, full);
        }
        self.footer(ui, full);
    }
}

// ---- layout -------------------------------------------------------------------------------

impl App {
    fn header(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, snap: &Snapshot, full: Rect) {
        let rect = Rect::from_min_size(full.min + Vec2::new(PAD, 14.0), Vec2::new(W - 2.0 * PAD, 40.0));

        // The whole bar drags the window; buttons go on top of it.
        let drag = ui.interact(rect, ui.id().with("drag"), Sense::drag());
        if drag.drag_started() {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }

        let mut x = rect.left();
        if self.settings {
            let back = Rect::from_min_size(Pos2::new(x, rect.center().y - 16.0), Vec2::splat(32.0));
            if icon_button(ui, back, |p, c| {
                let st = Stroke::new(1.8, INK);
                p.add(Shape::line(vec![c + Vec2::new(3.0, -6.0), c + Vec2::new(-3.0, 0.0), c + Vec2::new(3.0, 6.0)], st));
            })
            .clicked()
            {
                self.settings = false;
            }
            x += 42.0;
        }
        let title = if self.settings { "Settings" } else { "lclip" };
        ui.painter().text(Pos2::new(x, rect.center().y), Align2::LEFT_CENTER, title, serif(30.0), INK);

        if !self.settings {
            let gear = Rect::from_center_size(Pos2::new(rect.right() - 16.0, rect.center().y), Vec2::splat(32.0));
            if icon_button(ui, gear, |p, c| {
                let st = Stroke::new(1.7, INK);
                for (dy, kx) in [(-5.0, -3.0), (0.0, 3.5), (5.0, -1.0)] {
                    p.line_segment([c + Vec2::new(-8.0, dy), c + Vec2::new(8.0, dy)], st);
                    p.circle(c + Vec2::new(kx, dy), 2.6, CARD, st);
                }
            })
            .clicked()
            {
                self.settings = true;
            }

            // Which backend is doing the work.
            let ok = snap.backend != "stopped";
            let g = ui.painter().layout_no_wrap(snap.backend.clone(), mono(11.5), STONE);
            let w = g.size().x + 30.0;
            let chip = Rect::from_min_size(Pos2::new(gear.left() - 8.0 - w, rect.center().y - 12.0), Vec2::new(w, 24.0));
            let p = ui.painter();
            p.rect_filled(chip, 12, MIST);
            p.circle_filled(Pos2::new(chip.left() + 12.0, chip.center().y), 3.5, if ok { SAGE } else { INK });
            p.galley(Pos2::new(chip.left() + 21.0, chip.center().y - g.size().y / 2.0), g, STONE);
        }
    }

    fn history(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, snap: &Snapshot, full: Rect) {
        // search
        let search = Rect::from_min_size(full.min + Vec2::new(PAD, 64.0), Vec2::new(W - 2.0 * PAD, 40.0));
        ui.painter().rect(search, 10, PAPER, Stroke::new(1.0, MIST), StrokeKind::Inside);
        {
            let p = ui.painter();
            let c = Pos2::new(search.left() + 22.0, search.center().y - 1.0);
            p.circle_stroke(c, 5.5, Stroke::new(1.6, STONE));
            p.line_segment([c + Vec2::new(4.0, 4.0), c + Vec2::new(9.0, 9.0)], Stroke::new(1.6, STONE));
        }
        let field = Rect::from_min_max(
            Pos2::new(search.left() + 42.0, search.top() + 4.0),
            Pos2::new(search.right() - 10.0, search.bottom() - 4.0),
        );
        let edit = ui.put(
            field,
            egui::TextEdit::singleline(&mut self.query)
                .hint_text("Search history")
                .font(body(15.0))
                .frame(egui::Frame::NONE)
                .desired_width(field.width())
                .vertical_align(egui::Align::Center),
        );
        if self.frames < 3 {
            edit.request_focus();
        }
        if edit.changed() {
            self.cursor = 0;
        }

        // tabs
        let tabs = Rect::from_min_size(full.min + Vec2::new(PAD, 114.0), Vec2::new(W - 2.0 * PAD, 36.0));
        ui.painter().rect_filled(tabs, 10, PAPER);
        let counts = [snap.clipboard.len(), snap.primary.len()];
        for (i, (sel, name)) in [(Sel::Clipboard, "Clipboard"), (Sel::Primary, "Primary")].into_iter().enumerate() {
            let seg = Rect::from_min_size(
                tabs.min + Vec2::new(4.0 + i as f32 * (tabs.width() - 8.0) / 2.0, 4.0),
                Vec2::new((tabs.width() - 8.0) / 2.0, tabs.height() - 8.0),
            );
            let resp = ui.interact(seg, ui.id().with(("tab", i)), Sense::click());
            let on = self.tab == sel;
            let p = ui.painter();
            if on {
                p.rect(seg, 8, CARD, Stroke::new(1.0, MIST), StrokeKind::Inside);
            }
            let color = if on { INK } else { STONE };
            let g = p.layout_no_wrap(name.to_string(), medium(13.5), color);
            let gn = p.layout_no_wrap(counts[i].to_string(), mono(11.5), STONE);
            let total = g.size().x + 8.0 + gn.size().x;
            let x0 = seg.center().x - total / 2.0;
            let gw = g.size().x;
            p.galley(Pos2::new(x0, seg.center().y - g.size().y / 2.0), g.clone(), color);
            p.galley(Pos2::new(x0 + gw + 8.0, seg.center().y - gn.size().y / 2.0), gn, STONE);
            if resp.clicked() {
                self.tab = sel;
                self.cursor = 0;
            }
        }

        // list
        let q = self.query.to_lowercase();
        let list: Vec<EntryView> = {
            let src = if self.tab == Sel::Clipboard { &snap.clipboard } else { &snap.primary };
            let mut v: Vec<EntryView> = src
                .iter()
                .filter(|e| {
                    q.is_empty()
                        || e.text.as_deref().is_some_and(|t| t.to_lowercase().contains(&q))
                        || e.title.to_lowercase().contains(&q)
                })
                .cloned()
                .collect();
            v.sort_by_key(|e| !e.pinned);
            v
        };
        self.cursor = self.cursor.min(list.len().saturating_sub(1));

        let (down, up, enter, shift, ctrl_p, del) = ctx.input(|i| {
            (
                i.key_pressed(Key::ArrowDown),
                i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::Enter),
                i.modifiers.shift,
                i.modifiers.command && i.key_pressed(Key::P),
                i.key_pressed(Key::Delete),
            )
        });
        let mut reveal = false;
        if down && self.cursor + 1 < list.len() {
            self.cursor += 1;
            reveal = true;
        }
        if up && self.cursor > 0 {
            self.cursor -= 1;
            reveal = true;
        }
        if let Some(e) = list.get(self.cursor) {
            if enter {
                self.pick(e, shift);
            }
            if ctrl_p {
                self.act(ctx, Request::Pin { id: e.id });
            }
            if del && self.query.is_empty() {
                self.act(ctx, Request::Delete { id: e.id });
            }
        }

        let area = Rect::from_min_max(
            full.min + Vec2::new(PAD - 8.0, 162.0),
            Pos2::new(full.right() - PAD + 8.0, full.bottom() - 44.0),
        );

        if list.is_empty() {
            let c = area.center();
            let p = ui.painter();
            let msg = if self.query.is_empty() { "Nothing copied yet" } else { "No matches" };
            p.text(c - Vec2::new(0.0, 14.0), Align2::CENTER_CENTER, msg, serif(30.0), INK);
            let sub = if self.query.is_empty() {
                "Copy something in any app and it shows up here."
            } else {
                "Try a different word."
            };
            p.text(c + Vec2::new(0.0, 16.0), Align2::CENTER_CENTER, sub, body(14.0), STONE);
            return;
        }

        ui.scope_builder(UiBuilder::new().max_rect(area), |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(0.0, 6.0);
                ui.add_space(2.0);
                for (i, e) in list.iter().enumerate() {
                    self.row(ui, ctx, e, i, i == self.cursor, reveal && i == self.cursor);
                }
                ui.add_space(4.0);
            });
        });
    }

    fn row(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, e: &EntryView, i: usize, active: bool, reveal: bool) {
        self.want_thumb(ctx, e);

        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width() - 8.0, ROW_H), Sense::click());
        let rect = rect.translate(Vec2::new(4.0, 0.0));
        if reveal {
            ui.scroll_to_rect(rect, None);
        }

        // Mouse and keyboard share one highlight.
        if resp.hovered() && ctx.input(|i| i.pointer.delta() != Vec2::ZERO) {
            self.cursor = i;
        }
        let hot = active || resp.hovered();

        let del_rect = Rect::from_center_size(Pos2::new(rect.right() - 24.0, rect.center().y), Vec2::splat(28.0));
        let pin_rect = Rect::from_center_size(Pos2::new(rect.right() - 56.0, rect.center().y), Vec2::splat(28.0));
        let (mut pin_hit, mut del_hit) = (false, false);
        if hot {
            let pin = ui.interact(pin_rect, ui.id().with(("pin", e.id)), Sense::click());
            let del = ui.interact(del_rect, ui.id().with(("del", e.id)), Sense::click());
            pin_hit = pin.hovered();
            del_hit = del.hovered();
            if pin.clicked() {
                self.act(ctx, Request::Pin { id: e.id });
            }
            if del.clicked() {
                self.act(ctx, Request::Delete { id: e.id });
            }
        }

        let shift = ctx.input(|i| i.modifiers.shift);
        if resp.clicked() && !pin_hit && !del_hit {
            self.pick(e, shift);
        }

        // card
        let p = ui.painter();
        p.rect(
            rect,
            12,
            if hot { Color32::WHITE } else { CARD },
            Stroke::new(if active { 1.5 } else { 1.0 }, if active { SAGE } else { MIST }),
            StrokeKind::Inside,
        );

        // tile
        let tile = Rect::from_min_size(rect.min + Vec2::new(11.0, 11.0), Vec2::splat(38.0));
        let (tile_fill, tile_ink) = if e.secret { (INK, PAPER) } else { (MIST, SLATE) };
        p.rect_filled(tile, 10, tile_fill);
        if let Some(tex) = self.thumbs.get(&e.id).filter(|_| !e.secret) {
            cover_image(ui, tex, tile, 10);
        } else {
            draw_icon(ui.painter(), tile.center(), if e.secret { None } else { Some(e.kind) }, tile_ink);
        }

        // text
        let p = ui.painter();
        let tx = rect.left() + 62.0;
        let max_w = rect.right() - if hot { 74.0 } else { 44.0 } - tx;
        if e.secret {
            for k in 0..10 {
                p.circle_filled(Pos2::new(tx + 4.0 + k as f32 * 9.0, rect.top() + 22.0), 2.6, INK);
            }
        } else {
            let mut s = e.title.clone();
            let mut g = p.layout_no_wrap(s.clone(), medium(15.0), INK);
            while g.size().x > max_w && s.chars().count() > 2 {
                s = s.chars().take(s.chars().count() - 1).collect();
                g = p.layout_no_wrap(format!("{}…", s.trim_end()), medium(15.0), INK);
            }
            p.galley(Pos2::new(tx, rect.top() + 22.0 - g.size().y / 2.0), g, INK);
        }
        p.text(Pos2::new(tx, rect.top() + 42.0), Align2::LEFT_CENTER, meta(e), mono(11.5), STONE);

        // right edge
        if hot {
            paint_action(p, pin_rect, pin_hit, ActionIcon::Pin(e.pinned));
            paint_action(p, del_rect, del_hit, ActionIcon::Close);
        } else if let Some(ms) = e.expires_in_ms {
            let s = ms / 1000;
            let g = p.layout_no_wrap(format!("{}:{:02}", s / 60, s % 60), mono(11.5), SLATE);
            p.galley(Pos2::new(rect.right() - 14.0 - g.size().x, rect.center().y - g.size().y / 2.0), g, SLATE);
        } else if e.pinned {
            paint_action(p, Rect::from_center_size(Pos2::new(rect.right() - 24.0, rect.center().y), Vec2::splat(28.0)), false, ActionIcon::Pin(true));
        }
    }

    fn settings_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, snap: &Snapshot, full: Rect) {
        let Some(orig) = snap.settings.clone() else { return };
        let mut set = orig.clone();

        let area = Rect::from_min_max(
            full.min + Vec2::new(PAD, 66.0),
            Pos2::new(full.right() - PAD, full.bottom() - 44.0),
        );
        let mut clear = false;
        ui.scope_builder(UiBuilder::new().max_rect(area), |ui| {
          egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 4.0);

            group(ui, "HISTORY", |ui| {
                setting(ui, "Entries to keep", None, |ui| {
                    stepper(ui, &mut set.max_entries, 10, 1000, 10, "");
                });
                divider(ui);
                setting(ui, "Keep for", None, |ui| {
                    let opts = ["1 h", "24 h", "7 d", "Forever"];
                    let cur = match set.keep_for.map(|d| d.as_secs()) {
                        Some(3600) => 0,
                        Some(604800) => 2,
                        None => 3,
                        Some(_) => 1,
                    };
                    if let Some(i) = choice(ui, &opts, cur) {
                        set.keep_for = [Some(3600), Some(86400), Some(604800), None][i].map(Duration::from_secs);
                    }
                });
                divider(ui);
                setting(ui, "Largest item", Some("bigger copies are skipped"), |ui| {
                    let mut mb = set.max_item / (1024 * 1024);
                    stepper(ui, &mut mb, 1, 512, 8, " MB");
                    set.max_item = mb * 1024 * 1024;
                });
            });

            group(ui, "PASTING", |ui| {
                setting(ui, "Paste keys", None, |ui| {
                    let opts = ["Ctrl V", "Ctrl Shift V", "Shift Ins"];
                    let cur = match set.paste_keys {
                        PasteKeys::CtrlV => 0,
                        PasteKeys::CtrlShiftV => 1,
                        PasteKeys::ShiftInsert => 2,
                    };
                    if let Some(i) = choice(ui, &opts, cur) {
                        set.paste_keys = [PasteKeys::CtrlV, PasteKeys::CtrlShiftV, PasteKeys::ShiftInsert][i];
                    }
                });
            });

            ui.label(egui::RichText::new("Terminals need Ctrl Shift V.").font(body(12.5)).color(STONE));

            group(ui, "PRIVACY", |ui| {
                setting(ui, "Skip password manager copies", None, |ui| {
                    toggle(ui, &mut set.skip_secrets);
                });
                divider(ui);
                setting(ui, "Save history to disk", Some("secrets are never saved"), |ui| {
                    toggle(ui, &mut set.persist);
                });
            });

            ui.add_space(6.0);
            let w = ui.available_width();
            if button(ui, "Clear history", false, w).clicked() {
                clear = true;
            }
            ui.add_space(8.0);
          });
        });

        if clear {
            self.act(ctx, Request::Clear);
            self.settings = false;
        }
        if set != orig {
            if let Some(s) = self.snap.lock().unwrap().as_mut() {
                s.settings = Some(set.clone());
            }
            self.act(ctx, Request::Settings { settings: set });
        }
    }

    fn footer(&self, ui: &mut egui::Ui, full: Rect) {
        let y = full.bottom() - 44.0;
        let p = ui.painter();
        p.line_segment([Pos2::new(full.left() + 1.0, y), Pos2::new(full.right() - 1.0, y)], Stroke::new(1.0, MIST));
        let hints: &[(&str, &str)] = if self.settings {
            &[("Esc", "back")]
        } else {
            &[("Enter", "paste"), ("Shift", "plain"), ("Ctrl P", "pin"), ("Esc", "close")]
        };
        let mut x = full.left() + PAD;
        let cy = y + 22.0;
        for (key, what) in hints {
            let g = p.layout_no_wrap(key.to_string(), mono(10.5), SLATE);
            let r = Rect::from_min_size(Pos2::new(x, cy - 10.0), Vec2::new(g.size().x + 12.0, 20.0));
            p.rect(r, 5, PAPER, Stroke::new(1.0, MIST), StrokeKind::Inside);
            p.galley(Pos2::new(r.left() + 6.0, cy - g.size().y / 2.0), g, SLATE);
            let t = p.layout_no_wrap(what.to_string(), body(12.5), STONE);
            let tw = t.size().x;
            p.galley(Pos2::new(r.right() + 6.0, cy - t.size().y / 2.0), t, STONE);
            x = r.right() + 6.0 + tw + 14.0;
        }
    }
}

// ---- pieces -------------------------------------------------------------------------------

fn meta(e: &EntryView) -> String {
    if e.secret {
        return "password manager · memory only".into();
    }
    let first = e.formats.first().map_or("", |f| f.mime.as_str()).split(';').next().unwrap_or("");
    let first = if matches!(first, "UTF8_STRING" | "STRING") { "text/plain" } else { first };
    let extra = e.formats.len().saturating_sub(1);
    let mime = if extra > 0 { format!("{first} +{extra}") } else { first.to_string() };
    format!("{mime} · {} · {}", human(e.size), ago(e.age_secs))
}

fn human(n: usize) -> String {
    match n {
        0..1024 => format!("{n} B"),
        1024..1_048_576 => format!("{:.1} KB", n as f64 / 1024.0),
        _ => format!("{:.1} MB", n as f64 / 1_048_576.0),
    }
}

fn ago(secs: u64) -> String {
    match secs {
        0..10 => "just now".into(),
        10..60 => format!("{secs} s ago"),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86400 => format!("{} h ago", secs / 3600),
        _ => format!("{} d ago", secs / 86400),
    }
}

fn icon_button(ui: &mut egui::Ui, rect: Rect, draw: impl FnOnce(&egui::Painter, Pos2)) -> Response {
    let resp = ui.interact(rect, ui.id().with(("ib", rect.min.x as i32, rect.min.y as i32)), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, 9, PAPER);
    }
    draw(p, rect.center());
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

enum ActionIcon {
    Pin(bool),
    Close,
}

fn paint_action(p: &egui::Painter, rect: Rect, hover: bool, icon: ActionIcon) {
    if hover {
        p.rect_filled(rect, 8, PAPER);
    }
    let c = rect.center();
    match icon {
        ActionIcon::Pin(on) => {
            let col = if on { SAGE } else { STONE };
            let st = Stroke::new(1.6, col);
            if on {
                p.circle_filled(c - Vec2::new(0.0, 2.0), 4.5, col);
            } else {
                p.circle_stroke(c - Vec2::new(0.0, 2.0), 4.0, st);
            }
            p.line_segment([c + Vec2::new(0.0, 2.5), c + Vec2::new(0.0, 8.0)], st);
        }
        ActionIcon::Close => {
            let st = Stroke::new(1.6, STONE);
            p.line_segment([c + Vec2::new(-4.5, -4.5), c + Vec2::new(4.5, 4.5)], st);
            p.line_segment([c + Vec2::new(-4.5, 4.5), c + Vec2::new(4.5, -4.5)], st);
        }
    }
}

fn button(ui: &mut egui::Ui, label: &str, primary: bool, min_w: f32) -> Response {
    let g = ui.painter().layout_no_wrap(label.to_string(), medium(14.0), INK);
    let size = Vec2::new((g.size().x + 32.0).max(min_w), 40.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let hover = resp.hovered();
    let (fill, ink, stroke) = if primary {
        (if hover { SLATE } else { INK }, PAPER, INK)
    } else {
        (if hover { PAPER } else { CARD }, INK, MIST)
    };
    let p = ui.painter();
    p.rect(rect, 10, fill, Stroke::new(1.0, stroke), StrokeKind::Inside);
    p.text(rect.center(), Align2::CENTER_CENTER, label, medium(14.0), ink);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn group(ui: &mut egui::Ui, name: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(6.0);
    ui.label(egui::RichText::new(name).font(mono(11.0)).color(STONE));
    egui::Frame::new()
        .fill(Color32::WHITE)
        .stroke(Stroke::new(1.0, MIST))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::symmetric(14, 2))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

fn divider(ui: &mut egui::Ui) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(r, 0, MIST);
}

fn setting(ui: &mut egui::Ui, label: &str, hint: Option<&str>, control: impl FnOnce(&mut egui::Ui)) {
    let h = if hint.is_some() { 50.0 } else { 42.0 };
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::hover());
    let p = ui.painter();
    let ty = if hint.is_some() { rect.center().y - 8.0 } else { rect.center().y };
    p.text(Pos2::new(rect.left(), ty), Align2::LEFT_CENTER, label, body(14.5), INK);
    if let Some(h) = hint {
        p.text(Pos2::new(rect.left(), rect.center().y + 11.0), Align2::LEFT_CENTER, h, mono(11.0), STONE);
    }
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    control(&mut child);
}

fn toggle(ui: &mut egui::Ui, on: &mut bool) {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(42.0, 24.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
    }
    let p = ui.painter();
    p.rect_filled(rect, 12, if *on { SLATE } else { MIST });
    let x = if *on { rect.right() - 12.0 } else { rect.left() + 12.0 };
    p.circle_filled(Pos2::new(x, rect.center().y), 9.0, Color32::WHITE);
}

fn stepper(ui: &mut egui::Ui, value: &mut usize, min: usize, max: usize, step: usize, suffix: &str) {
    // right-to-left layout: plus first, then the number, then minus
    let (plus, pr) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::click());
    let g = ui.painter().layout_no_wrap(format!("{value}{suffix}"), medium(14.0), INK);
    let (num, _) = ui.allocate_exact_size(Vec2::new(g.size().x.max(46.0) + 8.0, 30.0), Sense::hover());
    let (minus, mr) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::click());

    if pr.clicked() {
        *value = (*value + step).min(max);
    }
    if mr.clicked() {
        *value = value.saturating_sub(step).max(min);
    }
    let p = ui.painter();
    for (r, resp, plus_sign) in [(plus, &pr, true), (minus, &mr, false)] {
        p.rect(r, 8, if resp.hovered() { PAPER } else { CARD }, Stroke::new(1.0, MIST), StrokeKind::Inside);
        let c = r.center();
        let st = Stroke::new(1.6, INK);
        p.line_segment([c - Vec2::new(5.0, 0.0), c + Vec2::new(5.0, 0.0)], st);
        if plus_sign {
            p.line_segment([c - Vec2::new(0.0, 5.0), c + Vec2::new(0.0, 5.0)], st);
        }
    }
    p.galley(Pos2::new(num.center().x - g.size().x / 2.0, num.center().y - g.size().y / 2.0), g, INK);
}

/// A row of options; returns the one just clicked.
fn choice(ui: &mut egui::Ui, options: &[&str], current: usize) -> Option<usize> {
    let galleys: Vec<_> = options
        .iter()
        .map(|o| ui.painter().layout_no_wrap(o.to_string(), medium(12.5), INK))
        .collect();
    let widths: Vec<f32> = galleys.iter().map(|g| g.size().x + 20.0).collect();
    let total: f32 = widths.iter().sum::<f32>() + 8.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(total, 32.0), Sense::hover());
    ui.painter().rect_filled(rect, 9, PAPER);

    let mut clicked = None;
    let mut x = rect.left() + 4.0;
    for (i, (g, w)) in galleys.into_iter().zip(widths).enumerate() {
        let seg = Rect::from_min_size(Pos2::new(x, rect.top() + 4.0), Vec2::new(w, 24.0));
        let resp = ui.interact(seg, ui.id().with(("choice", i, rect.min.x as i32, rect.min.y as i32)), Sense::click());
        let p = ui.painter();
        if i == current {
            p.rect(seg, 7, CARD, Stroke::new(1.0, MIST), StrokeKind::Inside);
        }
        let col = if i == current { INK } else { STONE };
        p.galley(Pos2::new(seg.center().x - g.size().x / 2.0, seg.center().y - g.size().y / 2.0), g, col);
        if resp.clicked() {
            clicked = Some(i);
        }
        x += w;
    }
    clicked
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
            p.text(c, Align2::CENTER_CENTER, "Aa", serif(21.0), color);
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

/// Debug builds only: a self screenshot, for checking the layout without a screen grabber.
#[cfg(debug_assertions)]
mod debug {
    use eframe::egui;

    /// LCLIP_VIEW = history | settings | search, LCLIP_SNAPSHOT = output png.
    pub fn hook(ctx: &egui::Context, frame: u64, settings: &mut bool, query: &mut String) {
        let Some(path) = std::env::var_os("LCLIP_SNAPSHOT") else { return };
        if frame == 3 {
            match std::env::var("LCLIP_VIEW").as_deref() {
                Ok("settings") => *settings = true,
                Ok("search") => *query = "gye".into(),
                _ => {}
            }
        }
        if frame == 40 {
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
