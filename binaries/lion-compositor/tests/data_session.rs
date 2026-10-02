//! Phase 32 exit criteria — the data family, end to end through the
//! real protocol.
//!
//! The clipboard the bridges need, CI-proven:
//!
//! * **The selection choreography** — the owner learns a serial from
//!   the shell's configure, offers two MIME types, and takes the
//!   clipboard slot; the receiver's device session learns of the
//!   offer (the server-chosen object arriving as a real object it
//!   can send requests on), enumerates the MIME types, and sees the
//!   selection event.
//! * **The transfer** — the receiver narrows the MIME (`accept`, the
//!   source sees `target`), then `receive`s through a real pipe: the
//!   write end crosses the request, the permission gate passes, the
//!   source's `send` event rides the same descriptor back, and the
//!   payload lands byte-exact in the receiver's read end.
//! * **The freshness doctrine** — a stale serial (an earlier
//!   configure's) is a fatal `invalid_state`; the serial clock is
//!   the seat's, drawn by every serial-bearing delivery.
//! * **The teardown truth** — the owner destroying its source
//!   announces `selection(null)` to the receiver; the clipboard
//!   empties honestly.

mod testbench;

use std::io::{Read as _, Write as _};

use ldp_client::{Connection, EventHandler, Proxy};
use ldp_core::wire::Value;
use ldp_transport::fd::FdList;
use testbench::Collector;

use lion_compositor::server::CompositorConfig;

/// The payload the source serves through the pipe.
const PAYLOAD: &[u8] = b"the lion sleeps tonight - over the real wire";

/// The first offered MIME type (the receiver accepts this one).
const MIME_UTF8: &str = "text/plain;charset=utf-8";
/// The second offered MIME type (enumerated, never accepted).
const MIME_PNG: &str = "image/png";

/// The client shapes the rigs drive: a connection plus recorded
/// events (the plain recorder, or the source's payload-serving pump).
trait Syncs {
    /// The wire.
    fn conn(&mut self) -> &mut Connection;
    /// One round-trip.
    fn sync(&mut self);
    /// The record so far.
    fn records(&self) -> &Collector;
    /// Round-trip until the predicate holds.
    fn wait_until(&mut self, pred: impl Fn(&Collector) -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !pred(self.records()) {
            assert!(
                deadline > std::time::Instant::now(),
                "timed out waiting for an event; collected so far: {:#?}",
                self.records().records
            );
            self.sync();
        }
    }
}

impl Syncs for testbench::TestClient {
    fn conn(&mut self) -> &mut Connection {
        &mut self.conn
    }
    fn sync(&mut self) {
        self.conn.roundtrip(&mut self.events).expect("roundtrip");
    }
    fn records(&self) -> &Collector {
        &self.events
    }
}

/// A client that records every event AND serves the payload when the
/// compositor hands it the transfer pipe (the source's half of the
/// contract: write into the descriptor, close it — EOF is the
/// receiver's signal).
struct SourcePump {
    /// Every event in arrival order.
    records: Collector,
    /// The bytes to serve on the next `data_source.send`.
    payload: Vec<u8>,
}

impl EventHandler for SourcePump {
    fn on_event(&mut self, event: &ldp_client::queue::Event) -> ldp_core::error::Result<()> {
        if event.interface == "ldp.data.data_source" && event.op.name == "send" {
            let raw = event.fds.raw_at(0).expect("send rides the pipe descriptor");
            serve_payload(raw, &self.payload);
        }
        self.records.on_event(event)
    }
}

/// The source-owning client.
struct SourceClient {
    /// The connection.
    conn: Connection,
    /// The recording, payload-serving pump.
    pump: SourcePump,
}

impl Syncs for SourceClient {
    fn conn(&mut self) -> &mut Connection {
        &mut self.conn
    }
    fn sync(&mut self) {
        self.conn.roundtrip(&mut self.pump).expect("roundtrip");
    }
    fn records(&self) -> &Collector {
        &self.pump.records
    }
}

/// Write the payload into the pipe's write end and close it (a dup of
/// the descriptor the event owns — the original closes when the event
/// drops, so the receiver's EOF arrives exactly once).
fn serve_payload(raw: i32, payload: &[u8]) {
    use std::os::fd::FromRawFd as _;
    // SAFETY: dup(2) of a descriptor the event's FD list owns for the
    // duration of this call; the duplicate is adopted by File and
    // closed on drop.
    let dup = unsafe { libc::dup(raw) };
    assert!(dup != -1, "dup of the transfer pipe");
    // SAFETY: the dup result is a fresh, unowned descriptor.
    let mut file = unsafe { std::fs::File::from_raw_fd(dup) };
    file.write_all(payload).expect("serve the payload");
    // Drop closes the dup; the event's own descriptor closes after
    // the handler returns — the receiver reads payload then EOF.
}

/// Bring up the plain testbench (the data family needs no flags —
/// the manager rides every world).
fn start(tag: &str) -> testbench::Testbench {
    testbench::Testbench::start_with(
        tag,
        CompositorConfig {
            socket: format!("lion-it-data-{tag}-{}", std::process::id()),
            ..CompositorConfig::default()
        },
    )
}

/// Bind a global and drain its `bound` confirmation.
fn bind<C: Syncs>(client: &mut C, interface: &str) -> Proxy {
    let proxy = client.conn().bind(interface).expect("bind");
    client.wait_until(|c| c.records.iter().any(|r| r.event == "bound"));
    proxy
}

/// The serial-learning step: bind the shell, create a toplevel, wait
/// for *that toplevel's* configure — return the serial it carried.
fn learn_serial<C: Syncs>(client: &mut C) -> u32 {
    let shell = bind(client, "ldp.shell.shell");
    let comp = bind(client, "ldp.core.compositor");
    let surface = client
        .conn()
        .create_object(&comp, "create_surface", vec![])
        .expect("surface");
    let toplevel = client
        .conn()
        .create_object(
            &shell,
            "get_toplevel",
            vec![
                Value::Object(Some(surface.id())),
                Value::Enum(2), // client-side decorations
            ],
        )
        .expect("toplevel");
    let target = toplevel.id().as_u32();
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "configure" && r.target == target)
    });
    let configure = client
        .records()
        .records
        .iter()
        .find(|r| r.event == "configure" && r.target == target)
        .expect("the configure arrived");
    match configure.args.first() {
        Some(Value::Uint32(serial)) => *serial,
        other => panic!("configure's serial argument is not a uint32: {other:?}"),
    }
}

/// Bind the seat and mint the data device (the door into the
/// family), syncing so the server has processed the creation before
/// the caller proceeds — a later selection's publication targets
/// every *then-existing* device client on the seat, and cross-session
/// ordering is otherwise racy.
fn data_device<C: Syncs>(client: &mut C) -> Proxy {
    let seat = bind(client, "ldp.input.seat");
    let manager = bind(client, "ldp.data.data_device_manager");
    let device = client
        .conn()
        .create_object(
            &manager,
            "get_data_device",
            vec![Value::Object(Some(seat.id()))],
        )
        .expect("data device");
    client.sync();
    device
}

/// Mint a data source offering one MIME type.
fn source_with<C: Syncs>(client: &mut C, mimes: &[&str]) -> Proxy {
    let manager = bind(client, "ldp.data.data_device_manager");
    let source = client
        .conn()
        .create_object(&manager, "create_data_source", vec![])
        .expect("source");
    for mime in mimes {
        client
            .conn()
            .send_request(&source, "offer", vec![Value::String((*mime).into())])
            .expect("offer");
    }
    source
}

/// A fresh OS pipe: (the receiver's read end, the write end to hand
/// the compositor).
fn pipe_pair() -> (std::fs::File, std::os::fd::OwnedFd) {
    use std::os::fd::FromRawFd as _;
    let mut fds = [0i32; 2];
    // SAFETY: a plain pipe(2) into a two-slot array; both ends are
    // immediately wrapped in owned handles (closed on drop).
    let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
    assert_eq!(rc, 0, "pipe(2) for the transfer");
    // SAFETY: the two fresh pipe descriptors, adopted exactly once.
    let read_end = unsafe { std::fs::File::from_raw_fd(fds[0]) };
    // SAFETY: the write end, adopted exactly once.
    let write_end = unsafe { std::os::fd::OwnedFd::from_raw_fd(fds[1]) };
    (read_end, write_end)
}

/// THE exit criterion: the full clipboard choreography over the wire —
/// selection, publication, narrowing, and the byte-exact transfer.
#[test]
#[allow(clippy::too_many_lines)] // one session, one narrative: the rig reads best unsplit
fn the_selection_publishes_and_the_transfer_lands_byte_exact() {
    let tb = start("clip-wire");
    let mut owner = SourceClient {
        conn: Connection::connect(&tb.addr).expect("owner connect"),
        pump: SourcePump {
            records: Collector::default(),
            payload: PAYLOAD.to_vec(),
        },
    };
    let mut receiver = testbench::TestClient::connect(&tb.addr);

    // The receiver's device exists first (the publication targets
    // every non-owner device client on the seat).
    let _receiver_device = data_device(&mut receiver);

    // The owner learns the serial the shell's configure carried.
    let serial = learn_serial(&mut owner);
    assert_eq!(
        serial, 1,
        "the first configure draws the clock's first serial"
    );

    // The owner mints its device and source, offers two MIME types,
    // and takes the clipboard slot.
    let owner_device = data_device(&mut owner);
    let source = source_with(&mut owner, &[MIME_UTF8, MIME_PNG]);
    owner
        .conn()
        .send_request(
            &owner_device,
            "set_selection",
            vec![Value::Object(Some(source.id())), Value::Uint32(serial)],
        )
        .expect("set_selection");
    owner.sync();

    // The receiver learns of the offer: the announcement (its own
    // object, server-chosen), the MIME enumeration, the selection.
    receiver.wait_until(|c| c.records.iter().any(|r| r.event == "selection"));
    let announcement = receiver
        .records()
        .records
        .iter()
        .find(|r| r.event == "data_offer")
        .expect("the data_offer announcement");
    let offer_id = match announcement.args.first() {
        Some(Value::NewId(id)) => *id,
        other => panic!("data_offer's argument is not a new_id: {other:?}"),
    };
    let offers: Vec<String> = receiver
        .records()
        .records
        .iter()
        .filter(|r| r.event == "offer")
        .map(|r| match &r.args[0] {
            Value::String(m) => m.to_string(),
            other => panic!("offer's argument is not a string: {other:?}"),
        })
        .collect();
    assert_eq!(
        offers,
        vec![MIME_UTF8.to_owned(), MIME_PNG.to_owned()],
        "the offer enumerates both MIME types in declaration order"
    );
    let selection = receiver
        .records()
        .records
        .iter()
        .find(|r| r.event == "selection")
        .expect("the selection event");
    assert_eq!(
        selection.args.first(),
        Some(&Value::Object(Some(offer_id))),
        "the selection references the announced offer"
    );

    // The receiver narrows to the MIME it understands; the source
    // learns the target.
    let offer = Proxy::new(offer_id, "ldp.data.data_offer", 1);
    receiver
        .conn
        .send_request(&offer, "accept", vec![Value::String(MIME_UTF8.into())])
        .expect("accept");
    owner.wait_until(|c| c.records.iter().any(|r| r.event == "target"));
    let target = owner
        .records()
        .records
        .iter()
        .find(|r| r.event == "target")
        .expect("the source's target event");
    assert_eq!(
        target.args.first(),
        Some(&Value::String(MIME_UTF8.into())),
        "the source sees the accepted MIME"
    );

    // The transfer: the receiver creates the pipe, hands the write
    // end over, and reads the payload back byte-exact.
    let (mut read_end, write_end) = pipe_pair();
    let mut fds = FdList::new();
    fds.push(write_end);
    receiver
        .conn
        .send_request_fd(
            &offer,
            "receive",
            vec![Value::String(MIME_UTF8.into()), Value::Fd(0)],
            &mut fds,
        )
        .expect("receive");
    // The source's pump serves the payload on the send event.
    owner.wait_until(|c| c.records.iter().any(|r| r.event == "send"));
    let send = owner
        .records()
        .records
        .iter()
        .find(|r| r.event == "send")
        .expect("the source's send event");
    assert_eq!(
        send.args.first(),
        Some(&Value::String(MIME_UTF8.into())),
        "the send event names the MIME being served"
    );

    // The receiver drains the pipe: the payload, then EOF.
    let mut payload = Vec::new();
    read_end
        .read_to_end(&mut payload)
        .expect("read the transfer");
    assert_eq!(
        payload, PAYLOAD,
        "the transfer landed byte-exact over the real pipe"
    );
}

/// The freshness doctrine: a serial the seat has already moved past
/// is a fatal `invalid_state` — the clock's current value is the only
/// valid reference.
#[test]
fn a_stale_serial_is_a_fatal_invalid_state() {
    let tb = start("clip-stale");
    let mut owner = testbench::TestClient::connect(&tb.addr);

    // Two configures: the clock has moved past the first serial.
    let first = learn_serial(&mut owner);
    let second = learn_serial(&mut owner);
    assert!(second > first, "the clock is monotonic");

    let owner_device = data_device(&mut owner);
    let source = source_with(&mut owner, &[MIME_UTF8]);

    // The stale serial: accepted on the wire, fatal in the reply —
    // the only rejection channel v1 has.
    owner
        .conn
        .send_request(
            &owner_device,
            "set_selection",
            vec![Value::Object(Some(source.id())), Value::Uint32(first)],
        )
        .expect("the send itself succeeds (the verdict rides the reply)");
    let err = drain_until_error(&mut owner);
    let text = format!("{err:?}");
    assert!(
        text.contains("InvalidState") || text.contains("invalid_state") || text.contains("serial"),
        "the stale serial is a fatal invalid_state: {text}"
    );
}

/// The teardown truth: the owner destroying its source announces
/// `selection(null)` to the receiver — the clipboard empties
/// honestly.
#[test]
fn destroying_the_source_announces_null_to_the_receiver() {
    let tb = start("clip-teardown");
    let mut owner = testbench::TestClient::connect(&tb.addr);
    let mut receiver = testbench::TestClient::connect(&tb.addr);

    let _receiver_device = data_device(&mut receiver);
    let serial = learn_serial(&mut owner);
    let owner_device = data_device(&mut owner);
    let source = source_with(&mut owner, &[MIME_UTF8]);
    owner
        .conn
        .send_request(
            &owner_device,
            "set_selection",
            vec![Value::Object(Some(source.id())), Value::Uint32(serial)],
        )
        .expect("set_selection");
    owner.sync();
    receiver.wait_until(|c| c.records.iter().any(|r| r.event == "selection"));

    // The owner destroys the source; the receiver sees null.
    owner.conn.destroy(&source).expect("destroy the source");
    owner.sync();
    receiver.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "selection" && r.args.first() == Some(&Value::Object(None)))
    });
}

/// Drain the client until the connection dies; return the error.
fn drain_until_error(client: &mut testbench::TestClient) -> ldp_client::error::ClientError {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        assert!(
            deadline > std::time::Instant::now(),
            "the connection never died"
        );
        match client.conn.roundtrip(&mut client.events) {
            Ok(()) => {}
            Err(e) => return e,
        }
    }
}
