use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Write, pipe},
    sync::{Arc, mpsc::Receiver},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle,
    backend::ObjectId,
    event_created_child,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_registry, wl_seat},
};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1::{self, ZwlrDataControlDeviceV1},
    zwlr_data_control_manager_v1::ZwlrDataControlManagerV1,
    zwlr_data_control_offer_v1::{self, ZwlrDataControlOfferV1},
    zwlr_data_control_source_v1::{self, ZwlrDataControlSourceV1},
};

use crate::store::{Cmd, Format, OWNER_MARK, SECRET_HINT, Sel, Shared, is_data_mime};

struct State {
    manager: ZwlrDataControlManagerV1,
    device: ZwlrDataControlDeviceV1,
    qh: QueueHandle<State>,
    conn: Connection,
    shared: Shared,
    offers: HashMap<ObjectId, Vec<String>>,
    held: Arc<HashMap<String, Arc<Vec<u8>>>>,
    source: Option<ZwlrDataControlSourceV1>,
    finished: bool,
}

pub fn supported() -> bool {
    let Ok(conn) = Connection::connect_to_env() else { return false };
    let Ok((globals, _)) = registry_queue_init::<State>(&conn) else { return false };
    globals
        .contents()
        .clone_list()
        .iter()
        .any(|g| g.interface == "zwlr_data_control_manager_v1")
}

pub fn run(shared: Shared, rx: Receiver<Cmd>) -> Result<()> {
    let conn = Connection::connect_to_env().context("no Wayland session")?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
    let qh = queue.handle();

    let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=1, ())?;
    let manager: ZwlrDataControlManagerV1 = globals
        .bind(&qh, 1..=2, ())
        .context("compositor does not support wlr-data-control")?;
    let device = manager.get_data_device(&seat, &qh, ());

    let mut state = State {
        manager,
        device,
        qh,
        conn: conn.clone(),
        shared,
        offers: HashMap::new(),
        held: Arc::default(),
        source: None,
        finished: false,
    };

    eprintln!("lclip running (wayland)");
    let mut last_sweep = Instant::now();
    while !state.finished {
        queue.dispatch_pending(&mut state)?;
        conn.flush()?;

        if let Some(guard) = queue.prepare_read() {
            let ready = {
                let fd = guard.connection_fd();
                let mut fds = [PollFd::new(&fd, PollFlags::IN)];
                poll(&mut fds, Some(&Timespec { tv_sec: 0, tv_nsec: 30_000_000 }))?
            };
            if ready > 0 {
                guard.read()?;
            }
        }
        queue.dispatch_pending(&mut state)?;

        while let Ok(cmd) = rx.try_recv() {
            match cmd {
                Cmd::Use { id, mime } => state.offer(id, mime.as_deref()),
                Cmd::Clear => state.release(),
            }
        }
        if last_sweep.elapsed() >= Duration::from_millis(500) {
            last_sweep = Instant::now();
            let (gone, current) = {
                let mut s = state.shared.lock().unwrap();
                (s.sweep(), s.current)
            };
            if current.is_some_and(|c| gone.contains(&c)) {
                state.release();
            }
        }
    }
    Ok(())
}

impl State {
    fn release(&mut self) {
        self.held = Arc::default();
        self.device.set_selection(None);
        if let Some(old) = self.source.take() {
            old.destroy();
        }
        self.shared.lock().unwrap().current = None;
    }

    /// Become the clipboard owner serving entry `id`, or just one of its formats.
    fn offer(&mut self, id: u64, only: Option<&str>) {
        let formats: Vec<Format> = {
            let s = self.shared.lock().unwrap();
            let Some(e) = s.find(id) else { return };
            e.formats
                .iter()
                .filter(|f| only.is_none_or(|m| f.mime == m) || f.mime == SECRET_HINT)
                .cloned()
                .collect()
        };

        let source = self.manager.create_data_source(&self.qh, ());
        source.offer(OWNER_MARK.to_string());
        for f in &formats {
            source.offer(f.mime.clone());
        }
        self.device.set_selection(Some(&source));
        if let Some(old) = self.source.replace(source) {
            old.destroy();
        }
        self.held = Arc::new(formats.into_iter().map(|f| (f.mime, f.data)).collect());
        self.shared.lock().unwrap().current = Some(id);
    }

    fn take_over(&mut self, offer: &ZwlrDataControlOfferV1, sel: Sel) {
        let mimes = self.offers.remove(&offer.id()).unwrap_or_default();
        if mimes.iter().any(|m| m == OWNER_MARK) {
            offer.destroy();
            return;
        }

        let max_item = self.shared.lock().unwrap().settings.max_item;
        let secret = mimes.iter().any(|m| m == SECRET_HINT);
        let mut formats = Vec::new();
        for mime in mimes.iter().filter(|m| is_data_mime(m) && *m != SECRET_HINT) {
            let Ok((mut rx, tx)) = pipe() else { continue };
            offer.receive(mime.clone(), std::os::fd::AsFd::as_fd(&tx));
            drop(tx);
            if self.conn.flush().is_err() {
                continue;
            }
            let mut buf = Vec::new();
            let read = rx.by_ref().take(max_item as u64 + 1).read_to_end(&mut buf);
            if read.is_ok() && !buf.is_empty() && buf.len() <= max_item {
                formats.push(Format { mime: mime.clone(), data: Arc::new(buf) });
            }
        }
        offer.destroy();
        if formats.is_empty() {
            return;
        }
        if secret {
            formats.push(Format { mime: SECRET_HINT.into(), data: Arc::new(b"secret".to_vec()) });
        }

        let id = self.shared.lock().unwrap().push(sel, formats, secret);
        if sel == Sel::Clipboard {
            match id {
                Some(id) => self.offer(id, None),
                None => self.shared.lock().unwrap().current = None,
            }
        }
    }
}

impl Dispatch<ZwlrDataControlDeviceV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrDataControlDeviceV1,
        event: zwlr_data_control_device_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_data_control_device_v1::Event;
        match event {
            Event::DataOffer { id } => {
                state.offers.insert(id.id(), Vec::new());
            }
            Event::Selection { id: Some(offer) } => state.take_over(&offer, Sel::Clipboard),
            Event::PrimarySelection { id: Some(offer) } => state.take_over(&offer, Sel::Primary),
            Event::Finished => state.finished = true,
            _ => {}
        }
    }

    event_created_child!(State, ZwlrDataControlDeviceV1, [
        zwlr_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ZwlrDataControlOfferV1, ()),
    ]);
}

impl Dispatch<ZwlrDataControlOfferV1, ()> for State {
    fn event(
        state: &mut Self,
        offer: &ZwlrDataControlOfferV1,
        event: zwlr_data_control_offer_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zwlr_data_control_offer_v1::Event::Offer { mime_type } = event {
            state.offers.entry(offer.id()).or_default().push(mime_type);
        }
    }
}

impl Dispatch<ZwlrDataControlSourceV1, ()> for State {
    fn event(
        state: &mut Self,
        source: &ZwlrDataControlSourceV1,
        event: zwlr_data_control_source_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_data_control_source_v1::Event;
        match event {
            Event::Send { mime_type, fd } => {
                let Some(data) = state.held.get(&mime_type).cloned() else { return };
                // Large pastes must not stall the event loop.
                thread::spawn(move || {
                    let _ = File::from(fd).write_all(&data);
                });
            }
            Event::Cancelled => {
                source.destroy();
                if state.source.as_ref() == Some(source) {
                    state.source = None;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(_: &mut Self, _: &wl_seat::WlSeat, _: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<ZwlrDataControlManagerV1, ()> for State {
    fn event(_: &mut Self, _: &ZwlrDataControlManagerV1, _: <ZwlrDataControlManagerV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
