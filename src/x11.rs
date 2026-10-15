use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, mpsc::Receiver},
    thread::sleep,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use x11rb::{
    COPY_FROM_PARENT, CURRENT_TIME, NONE,
    connection::Connection,
    protocol::{
        Event,
        xfixes::{self, ConnectionExt as _},
        xproto::{
            Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, CreateWindowAux,
            EventMask, PropMode, Property, SELECTION_NOTIFY_EVENT, SelectionNotifyEvent,
            SelectionRequestEvent, Window, WindowClass,
        },
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as _,
};

use crate::store::{Cmd, Format, SECRET_HINT, Sel, Shared, is_data_mime};

// Larger values go out in INCR chunks; must stay a multiple of 4.
const CHUNK: usize = 256 * 1024;
const TIMEOUT: Duration = Duration::from_secs(2);
const TICK: Duration = Duration::from_millis(15);

struct Atoms {
    clipboard: Atom,
    primary: Atom,
    targets: Atom,
    incr: Atom,
    prop: Atom,
}

struct Transfer {
    win: Window,
    prop: Atom,
    ty: Atom,
    data: Arc<Vec<u8>>,
    offset: usize,
}

struct Ctx {
    conn: RustConnection,
    win: Window,
    atoms: Atoms,
    shared: Shared,
    rx: Receiver<Cmd>,
    names: HashMap<Atom, String>,
    ids: HashMap<String, Atom>,
    held: Vec<(Atom, Arc<Vec<u8>>)>,
    queue: VecDeque<Event>,
    transfers: Vec<Transfer>,
    max_item: usize,
}

pub fn run(shared: Shared, rx: Receiver<Cmd>) -> Result<()> {
    let (conn, screen_num) = RustConnection::connect(None).context("cannot connect to X server")?;
    let root = conn.setup().roots[screen_num].root;

    let win = conn.generate_id()?;
    conn.create_window(
        COPY_FROM_PARENT as u8,
        win,
        root,
        0,
        0,
        1,
        1,
        0,
        WindowClass::INPUT_ONLY,
        0,
        &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
    )?;

    let intern = |name: &str| -> Result<Atom> { Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom) };
    let atoms = Atoms {
        clipboard: intern("CLIPBOARD")?,
        primary: AtomEnum::PRIMARY.into(),
        targets: intern("TARGETS")?,
        incr: intern("INCR")?,
        prop: intern("LCLIP_DATA")?,
    };

    conn.xfixes_query_version(5, 0)?.reply().context("XFixes extension missing")?;
    for sel in [atoms.clipboard, atoms.primary] {
        conn.xfixes_select_selection_input(win, sel, xfixes::SelectionEventMask::SET_SELECTION_OWNER)?;
    }
    conn.flush()?;

    let mut ctx = Ctx {
        conn,
        win,
        atoms,
        shared,
        rx,
        names: HashMap::new(),
        ids: HashMap::new(),
        held: Vec::new(),
        queue: VecDeque::new(),
        transfers: Vec::new(),
        max_item: 0,
    };

    eprintln!("lclip running (x11)");
    let mut last_sweep = Instant::now();
    loop {
        while let Some(ev) = ctx.queue.pop_front() {
            ctx.handle(ev)?;
        }
        while let Some(ev) = ctx.conn.poll_for_event()? {
            ctx.handle(ev)?;
        }
        while let Ok(cmd) = ctx.rx.try_recv() {
            ctx.command(cmd)?;
        }
        if last_sweep.elapsed() >= Duration::from_millis(500) {
            last_sweep = Instant::now();
            ctx.sweep()?;
        }
        sleep(TICK);
    }
}

impl Ctx {
    fn atom(&mut self, name: &str) -> Result<Atom> {
        if let Some(a) = self.ids.get(name) {
            return Ok(*a);
        }
        let a = self.conn.intern_atom(false, name.as_bytes())?.reply()?.atom;
        self.ids.insert(name.to_string(), a);
        self.names.insert(a, name.to_string());
        Ok(a)
    }

    fn name(&mut self, atom: Atom) -> Result<String> {
        if let Some(n) = self.names.get(&atom) {
            return Ok(n.clone());
        }
        let n = String::from_utf8_lossy(&self.conn.get_atom_name(atom)?.reply()?.name).into_owned();
        self.names.insert(atom, n.clone());
        self.ids.insert(n.clone(), atom);
        Ok(n)
    }

    fn handle(&mut self, ev: Event) -> Result<()> {
        match ev {
            Event::XfixesSelectionNotify(e) if e.owner != self.win && e.owner != NONE => {
                if e.selection == self.atoms.clipboard {
                    self.grab(self.atoms.clipboard, Sel::Clipboard)?;
                } else if e.selection == self.atoms.primary {
                    self.grab(self.atoms.primary, Sel::Primary)?;
                }
            }
            Event::SelectionRequest(r) => self.serve(r)?,
            Event::PropertyNotify(p) if p.state == Property::DELETE => self.continue_transfer(p.window, p.atom)?,
            _ => {}
        }
        Ok(())
    }

    fn command(&mut self, cmd: Cmd) -> Result<()> {
        match cmd {
            Cmd::Use { id, mime } => self.offer(id, mime.as_deref())?,
            Cmd::Clear => self.release()?,
        }
        Ok(())
    }

    fn sweep(&mut self) -> Result<()> {
        let (gone, current) = {
            let mut s = self.shared.lock().unwrap();
            (s.sweep(), s.current)
        };
        if current.is_some_and(|c| gone.contains(&c)) {
            self.release()?;
        }
        Ok(())
    }

    fn release(&mut self) -> Result<()> {
        self.held.clear();
        self.transfers.clear();
        self.conn.set_selection_owner(NONE, self.atoms.clipboard, CURRENT_TIME)?;
        self.conn.flush()?;
        self.shared.lock().unwrap().current = None;
        Ok(())
    }

    /// Become the clipboard owner serving entry `id`, or just one of its formats.
    fn offer(&mut self, id: u64, only: Option<&str>) -> Result<()> {
        let formats: Vec<Format> = {
            let s = self.shared.lock().unwrap();
            let Some(e) = s.find(id) else { return Ok(()) };
            e.formats
                .iter()
                .filter(|f| only.is_none_or(|m| f.mime == m) || f.mime == SECRET_HINT)
                .cloned()
                .collect()
        };
        let mut held = Vec::new();
        for f in formats {
            held.push((self.atom(&f.mime)?, f.data));
        }
        self.held = held;
        self.transfers.clear();
        self.conn.set_selection_owner(self.win, self.atoms.clipboard, CURRENT_TIME)?;
        self.conn.flush()?;
        self.shared.lock().unwrap().current = Some(id);
        Ok(())
    }

    /// Wait for an event matching `pred`; everything else is queued for the main loop.
    fn wait_event(&mut self, pred: impl Fn(&Event) -> bool) -> Result<Option<Event>> {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            while let Some(ev) = self.conn.poll_for_event()? {
                if pred(&ev) {
                    return Ok(Some(ev));
                }
                self.queue.push_back(ev);
            }
            if Instant::now() > deadline {
                return Ok(None);
            }
            sleep(Duration::from_millis(2));
        }
    }

    /// Read our scratch property in full, then delete it (which tells the owner to continue).
    fn read_prop(&self) -> Result<(Atom, u8, Vec<u8>)> {
        let (mut data, mut offset) = (Vec::new(), 0u32);
        let (ty, format) = loop {
            let r = self
                .conn
                .get_property(false, self.win, self.atoms.prop, AtomEnum::ANY, offset, 1 << 20)?
                .reply()?;
            offset += (r.value.len() / 4) as u32;
            data.extend_from_slice(&r.value);
            if r.bytes_after == 0 || data.len() > self.max_item {
                break (r.type_, r.format);
            }
        };
        self.conn.delete_property(self.win, self.atoms.prop)?;
        self.conn.flush()?;
        Ok((ty, format, data))
    }

    /// Ask the selection owner for one target. Returns the property format and bytes.
    fn fetch(&mut self, selection: Atom, target: Atom) -> Result<Option<(u8, Vec<u8>)>> {
        let (win, prop) = (self.win, self.atoms.prop);
        self.conn.delete_property(win, prop)?;
        self.conn.convert_selection(win, selection, target, prop, CURRENT_TIME)?;
        self.conn.flush()?;

        let notified = self.wait_event(|e| matches!(e, Event::SelectionNotify(n) if n.requestor == win && n.target == target))?;
        let Some(Event::SelectionNotify(n)) = notified else { return Ok(None) };
        if n.property == NONE {
            return Ok(None);
        }

        let (ty, mut format, mut data) = self.read_prop()?;
        if ty == self.atoms.incr {
            data.clear();
            loop {
                let next = self.wait_event(|e| {
                    matches!(e, Event::PropertyNotify(p) if p.window == win && p.atom == prop && p.state == Property::NEW_VALUE)
                })?;
                if next.is_none() {
                    return Ok(None);
                }
                let (_, f, chunk) = self.read_prop()?;
                if chunk.is_empty() {
                    break;
                }
                format = f;
                data.extend_from_slice(&chunk);
                if data.len() > self.max_item {
                    return Ok(None);
                }
            }
        }
        if data.is_empty() || data.len() > self.max_item {
            return Ok(None);
        }
        Ok(Some((format, data)))
    }

    fn grab(&mut self, selection: Atom, sel: Sel) -> Result<()> {
        self.max_item = self.shared.lock().unwrap().settings.max_item;

        let Some((_, raw)) = self.fetch(selection, self.atoms.targets)? else { return Ok(()) };
        let mut targets = Vec::new();
        for b in raw.chunks_exact(4) {
            let atom = u32::from_ne_bytes(b.try_into().unwrap());
            targets.push((atom, self.name(atom)?));
        }

        let secret = targets.iter().any(|(_, n)| n == SECRET_HINT);
        let mut formats = Vec::new();
        for (atom, name) in targets {
            if !is_data_mime(&name) || name == SECRET_HINT {
                continue;
            }
            if let Some((8, data)) = self.fetch(selection, atom)? {
                formats.push(Format { mime: name, data: Arc::new(data) });
            }
        }
        if formats.is_empty() {
            return Ok(());
        }
        if secret {
            formats.push(Format { mime: SECRET_HINT.into(), data: Arc::new(b"secret".to_vec()) });
        }

        let id = self.shared.lock().unwrap().push(sel, formats, secret);
        if sel == Sel::Clipboard {
            match id {
                Some(id) => self.offer(id, None)?,
                None => self.shared.lock().unwrap().current = None,
            }
        }
        Ok(())
    }

    fn serve(&mut self, r: SelectionRequestEvent) -> Result<()> {
        let prop = if r.property == NONE { r.target } else { r.property };
        let mut reply = prop;

        if r.selection != self.atoms.clipboard {
            reply = NONE;
        } else if r.target == self.atoms.targets {
            let mut list = vec![self.atoms.targets];
            list.extend(self.held.iter().map(|(t, _)| *t));
            self.conn.change_property32(PropMode::REPLACE, r.requestor, prop, AtomEnum::ATOM, &list)?;
        } else if let Some((_, data)) = self.held.iter().find(|(t, _)| *t == r.target) {
            if data.len() > CHUNK {
                self.conn.change_window_attributes(
                    r.requestor,
                    &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
                )?;
                self.conn.change_property32(
                    PropMode::REPLACE,
                    r.requestor,
                    prop,
                    self.atoms.incr,
                    &[data.len() as u32],
                )?;
                self.transfers.push(Transfer {
                    win: r.requestor,
                    prop,
                    ty: r.target,
                    data: Arc::clone(data),
                    offset: 0,
                });
            } else {
                self.conn.change_property8(PropMode::REPLACE, r.requestor, prop, r.target, data)?;
            }
        } else {
            reply = NONE;
        }

        let event = SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: r.time,
            requestor: r.requestor,
            selection: r.selection,
            target: r.target,
            property: reply,
        };
        self.conn.send_event(false, r.requestor, EventMask::NO_EVENT, event)?;
        self.conn.flush()?;
        Ok(())
    }

    fn continue_transfer(&mut self, win: Window, prop: Atom) -> Result<()> {
        let Some(i) = self.transfers.iter().position(|t| t.win == win && t.prop == prop) else {
            return Ok(());
        };
        let t = &mut self.transfers[i];
        let end = (t.offset + CHUNK).min(t.data.len());
        let chunk = &t.data[t.offset..end];
        self.conn.change_property8(PropMode::REPLACE, win, prop, t.ty, chunk)?;
        t.offset = end;

        if chunk.is_empty() {
            self.transfers.remove(i);
            self.conn
                .change_window_attributes(win, &ChangeWindowAttributesAux::new().event_mask(EventMask::NO_EVENT))?;
        }
        self.conn.flush()?;
        Ok(())
    }
}
