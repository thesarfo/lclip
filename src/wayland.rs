use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Write, pipe},
    sync::Arc,
    thread,
};

use anyhow::{Context, Result};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle, event_created_child,
    backend::ObjectId,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_registry, wl_seat},
};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1::{self, ZwlrDataControlDeviceV1},
    zwlr_data_control_manager_v1::ZwlrDataControlManagerV1,
    zwlr_data_control_offer_v1::{self, ZwlrDataControlOfferV1},
    zwlr_data_control_source_v1::{self, ZwlrDataControlSourceV1},
};

// Offered on our own source so we can tell our re-offer from someone else's copy.
const OWNER_MARK: &str = "x-lclip/owner";
// Password managers set this to ask clipboard tools not to keep the copy.
const SECRET_HINT: &str = "x-kde-passwordManagerHint";
const MAX_BYTES: u64 = 64 * 1024 * 1024;

struct State {
    manager: ZwlrDataControlManagerV1,
    conn: Connection,
    offers: HashMap<ObjectId, Vec<String>>,
    held: Arc<HashMap<String, Vec<u8>>>,
    source: Option<ZwlrDataControlSourceV1>,
    finished: bool,
}

impl State {
    fn take_over(&mut self, offer: &ZwlrDataControlOfferV1, device: &ZwlrDataControlDeviceV1, qh: &QueueHandle<Self>) {
        let mimes = self.offers.remove(&offer.id()).unwrap_or_default();
        if mimes.iter().any(|m| m == OWNER_MARK || m == SECRET_HINT) {
            offer.destroy();
            return;
        }

        let mut held = HashMap::new();
        for mime in mimes.iter().filter(|m| is_data_mime(m)) {
            let Ok((mut rx, tx)) = pipe() else { continue };
            offer.receive(mime.clone(), tx.as_fd_ref());
            drop(tx);
            if self.conn.flush().is_err() {
                continue;
            }
            let mut buf = Vec::new();
            if rx.by_ref().take(MAX_BYTES).read_to_end(&mut buf).is_ok() && !buf.is_empty() {
                held.insert(mime.clone(), buf);
            }
        }
        offer.destroy();

        if held.is_empty() {
            return;
        }
        let bytes: usize = held.values().map(Vec::len).sum();
        eprintln!("kept {} formats, {} bytes", held.len(), bytes);

        let source = self.manager.create_data_source(qh, ());
        source.offer(OWNER_MARK.to_string());
        for mime in held.keys() {
            source.offer(mime.clone());
        }
        device.set_selection(Some(&source));
        if let Some(old) = self.source.replace(source) {
            old.destroy();
        }
        self.held = Arc::new(held);
    }
}

// X11 apps via XWayland advertise target atoms that carry no data.
fn is_data_mime(mime: &str) -> bool {
    !matches!(mime, "TARGETS" | "TIMESTAMP" | "MULTIPLE" | "SAVE_TARGETS" | OWNER_MARK)
}

trait AsFdRef {
    fn as_fd_ref(&self) -> std::os::fd::BorrowedFd<'_>;
}
impl AsFdRef for std::io::PipeWriter {
    fn as_fd_ref(&self) -> std::os::fd::BorrowedFd<'_> {
        std::os::fd::AsFd::as_fd(self)
    }
}

impl Dispatch<ZwlrDataControlDeviceV1, ()> for State {
    fn event(
        state: &mut Self,
        device: &ZwlrDataControlDeviceV1,
        event: zwlr_data_control_device_v1::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        use zwlr_data_control_device_v1::Event;
        match event {
            Event::DataOffer { id } => {
                state.offers.insert(id.id(), Vec::new());
            }
            Event::Selection { id: Some(offer) } => state.take_over(&offer, device, qh),
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
                let Some(data) = state.held.get(&mime_type).map(|_| Arc::clone(&state.held)) else {
                    return;
                };
                // Large pastes must not stall the event loop.
                thread::spawn(move || {
                    let _ = File::from(fd).write_all(&data[&mime_type]);
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

pub fn supported() -> bool {
    let Ok(conn) = Connection::connect_to_env() else { return false };
    let Ok((globals, _)) = registry_queue_init::<State>(&conn) else { return false };
    globals
        .contents()
        .clone_list()
        .iter()
        .any(|g| g.interface == "zwlr_data_control_manager_v1")
}

pub fn run() -> Result<()> {
    let conn = Connection::connect_to_env().context("no Wayland session (is WAYLAND_DISPLAY set?)")?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
    let qh = queue.handle();

    let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=1, ())?;
    let manager: ZwlrDataControlManagerV1 = globals
        .bind(&qh, 1..=2, ())
        .context("compositor does not support wlr-data-control")?;
    manager.get_data_device(&seat, &qh, ());

    let mut state = State {
        manager,
        conn,
        offers: HashMap::new(),
        held: Arc::default(),
        source: None,
        finished: false,
    };

    eprintln!("lclip running");
    while !state.finished {
        queue.blocking_dispatch(&mut state)?;
    }
    Ok(())
}
