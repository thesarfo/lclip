use std::{
    collections::VecDeque,
    rc::Rc,
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

// Larger values go out in INCR chunks; must stay a multiple of 4.
const CHUNK: usize = 256 * 1024;
const MAX_BYTES: usize = 64 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(2);
const SECRET_HINT: &str = "x-kde-passwordManagerHint";

struct Atoms {
    clipboard: Atom,
    targets: Atom,
    incr: Atom,
    prop: Atom,
    hint: Atom,
    skip: Vec<Atom>,
}

struct Held {
    target: Atom,
    ty: Atom,
    format: u8,
    data: Rc<Vec<u8>>,
}

struct Transfer {
    win: Window,
    prop: Atom,
    ty: Atom,
    format: u8,
    data: Rc<Vec<u8>>,
    offset: usize,
}

struct Ctx {
    conn: RustConnection,
    win: Window,
    atoms: Atoms,
    held: Vec<Held>,
    queue: VecDeque<Event>,
    transfers: Vec<Transfer>,
}

pub fn run() -> Result<()> {
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
        targets: intern("TARGETS")?,
        incr: intern("INCR")?,
        prop: intern("LCLIP_DATA")?,
        hint: intern(SECRET_HINT)?,
        // Protocol atoms that describe a selection but carry no content.
        skip: ["TARGETS", "TIMESTAMP", "MULTIPLE", "SAVE_TARGETS", "DELETE"]
            .iter()
            .map(|n| intern(n))
            .collect::<Result<_>>()?,
    };

    conn.xfixes_query_version(5, 0)?.reply().context("XFixes extension missing")?;
    conn.xfixes_select_selection_input(win, atoms.clipboard, xfixes::SelectionEventMask::SET_SELECTION_OWNER)?;
    conn.flush()?;

    let mut ctx = Ctx { conn, win, atoms, held: Vec::new(), queue: VecDeque::new(), transfers: Vec::new() };
    eprintln!("lclip running (x11)");
    loop {
        while let Some(ev) = ctx.queue.pop_front() {
            ctx.handle(ev)?;
        }
        let ev = ctx.conn.wait_for_event()?;
        ctx.handle(ev)?;
    }
}

impl Ctx {
    fn handle(&mut self, ev: Event) -> Result<()> {
        match ev {
            Event::XfixesSelectionNotify(e) if e.selection == self.atoms.clipboard => {
                if e.owner != self.win && e.owner != NONE {
                    self.grab()?;
                }
            }
            Event::SelectionRequest(r) => self.serve(r)?,
            Event::PropertyNotify(p) if p.state == Property::DELETE => self.continue_transfer(p.window, p.atom)?,
            _ => {}
        }
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
            if r.bytes_after == 0 || data.len() > MAX_BYTES {
                break (r.type_, r.format);
            }
        };
        self.conn.delete_property(self.win, self.atoms.prop)?;
        self.conn.flush()?;
        Ok((ty, format, data))
    }

    fn fetch(&mut self, target: Atom) -> Result<Option<Held>> {
        let (win, prop) = (self.win, self.atoms.prop);
        self.conn.delete_property(win, prop)?;
        self.conn.convert_selection(win, self.atoms.clipboard, target, prop, CURRENT_TIME)?;
        self.conn.flush()?;

        let notified = self.wait_event(|e| matches!(e, Event::SelectionNotify(n) if n.requestor == win && n.target == target))?;
        let Some(Event::SelectionNotify(n)) = notified else { return Ok(None) };
        if n.property == NONE {
            return Ok(None);
        }

        let (mut ty, mut format, mut data) = self.read_prop()?;
        if ty == self.atoms.incr {
            data.clear();
            loop {
                let next = self.wait_event(|e| {
                    matches!(e, Event::PropertyNotify(p) if p.window == win && p.atom == prop && p.state == Property::NEW_VALUE)
                })?;
                if next.is_none() {
                    return Ok(None);
                }
                let (t, f, chunk) = self.read_prop()?;
                if chunk.is_empty() {
                    break;
                }
                (ty, format) = (t, f);
                data.extend_from_slice(&chunk);
                if data.len() > MAX_BYTES {
                    return Ok(None);
                }
            }
        }
        if data.is_empty() {
            return Ok(None);
        }
        Ok(Some(Held { target, ty, format, data: Rc::new(data) }))
    }

    fn grab(&mut self) -> Result<()> {
        let Some(offered) = self.fetch(self.atoms.targets)? else { return Ok(()) };
        let targets: Vec<Atom> = offered
            .data
            .chunks_exact(4)
            .map(|b| u32::from_ne_bytes(b.try_into().unwrap()))
            .collect();
        if targets.contains(&self.atoms.hint) {
            return Ok(());
        }

        let mut held = Vec::new();
        let skip = self.atoms.skip.clone();
        for t in targets.into_iter().filter(|t| !skip.contains(t)) {
            if let Some(h) = self.fetch(t)? {
                held.push(h);
            }
        }
        if held.is_empty() {
            return Ok(());
        }
        let bytes: usize = held.iter().map(|h| h.data.len()).sum();
        eprintln!("kept {} formats, {} bytes", held.len(), bytes);

        self.held = held;
        self.transfers.clear();
        self.conn.set_selection_owner(self.win, self.atoms.clipboard, CURRENT_TIME)?;
        self.conn.flush()?;
        Ok(())
    }

    fn serve(&mut self, r: SelectionRequestEvent) -> Result<()> {
        let prop = if r.property == NONE { r.target } else { r.property };
        let mut reply = prop;

        if r.selection != self.atoms.clipboard {
            reply = NONE;
        } else if r.target == self.atoms.targets {
            let mut list = vec![self.atoms.targets];
            list.extend(self.held.iter().map(|h| h.target));
            self.conn.change_property32(PropMode::REPLACE, r.requestor, prop, AtomEnum::ATOM, &list)?;
        } else if let Some(h) = self.held.iter().find(|h| h.target == r.target) {
            if h.data.len() > CHUNK {
                self.conn.change_window_attributes(
                    r.requestor,
                    &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
                )?;
                self.conn.change_property32(
                    PropMode::REPLACE,
                    r.requestor,
                    prop,
                    self.atoms.incr,
                    &[h.data.len() as u32],
                )?;
                self.transfers.push(Transfer {
                    win: r.requestor,
                    prop,
                    ty: h.ty,
                    format: h.format,
                    data: Rc::clone(&h.data),
                    offset: 0,
                });
            } else {
                let units = h.data.len() / (h.format as usize / 8);
                self.conn
                    .change_property(PropMode::REPLACE, r.requestor, prop, h.ty, h.format, units as u32, &h.data)?;
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
        let units = chunk.len() / (t.format as usize / 8);
        self.conn
            .change_property(PropMode::REPLACE, win, prop, t.ty, t.format, units as u32, chunk)?;
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
