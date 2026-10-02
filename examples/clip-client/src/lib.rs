//! # clip-client — the clipboard client
//!
//! Two halves, both honest:
//!
//! * **`--live`** connects to a running server and reports whether
//!   the `ldp.data.data_device_manager` family is advertised. A
//!   pre-data stack (the Phase 30 era) advertised no data global —
//!   the client said so and exited cleanly instead of binding an
//!   unadvertised global (a fatal protocol error by design). The
//!   served compositor (Phase 32) answers with the real manager
//!   behind it.
//! * **The local negotiation** (the default) runs the complete
//!   offer/accept/receive choreography in-process through the real
//!   [`ldp_clipboard`] manager — the exact library a server embeds —
//!   with two synthetic clients: one offers two MIME types and takes
//!   the clipboard slot; the other receives the selection event,
//!   accepts the best MIME it understands, and opens the pipe
//!   transfer. Every step prints; nothing is simulated but the two
//!   peers.
//!
//! ```text
//! clip-client [--socket NAME] [--live]
//! ```
//!
//! The negotiation demonstrates the v1 contract precisely: ownership
//! is serial-bound, offers are created by the *server* on selection,
//! the receiver narrows the MIME, and `receive` is permission-gated
//! (`ClipboardRead` in the manifest).
//!
//! Safety note: the only `unsafe` in this crate is the two-line
//! `pipe(2)` seam in the private `pipe` module (each call carrying
//! its SAFETY comment — the `ldp-transport`/`ldp-tools` precedent; a
//! crate-level `forbid` cannot be lifted for that module).

use std::io::Write;

use ldp_clipboard::dnd::ClientKey;
use ldp_clipboard::manager::{ClipboardManager, SeatKey};
use ldp_clipboard::{DataEvent, Slot};
use ldp_core::caps::{Scope, ScopeSet};
use ldp_core::ids::ObjectId;
use ldp_core::limits::Limits;
use ldp_tools::error::{Result, ToolError};
use ldp_tools::session::ToolSession;
use ldp_transport::UnixAddr;

/// The clipboard family's global (the interface the live mode probes).
pub const CLIPBOARD_GLOBAL: &str = "ldp.data.data_device_manager";

/// Parsed command line.
#[derive(Clone, Debug, Default)]
pub struct ClipArgs {
    /// `--socket NAME` / `LDP_SOCKET`.
    pub socket: Option<String>,
    /// `--live`: probe a running server.
    pub live: bool,
}

/// The usage text.
#[must_use]
pub fn usage() -> String {
    "clip-client — the clipboard client\n\
     \n\
     USAGE:\n\
     \x20 clip-client [--live [--socket NAME]]\n\
     \n\
     OPTIONS:\n\
     \x20 --live            probe the running server's clipboard availability\n\
     \x20 --socket NAME     abstract socket name (or LDP_SOCKET)\n\
     \x20 --help | --version  this text\n\
     \n\
     With no flags: the in-process offer/accept/receive negotiation\n\
     (two synthetic clients through the real manager)."
        .to_owned()
}

/// Run the example; returns the process exit code.
///
/// # Errors
/// [`ToolError`] on connection or negotiation failures.
pub fn run(args: &ClipArgs, out: &mut dyn Write) -> Result<i32, ToolError> {
    if args.live {
        live_report(args, out)
    } else {
        local_negotiation(out);
        Ok(0)
    }
}

/// The live availability report (the honest-degradation doctrine).
fn live_report(args: &ClipArgs, out: &mut dyn Write) -> Result<i32, ToolError> {
    let addr = resolve_socket(args)?;
    let mut session = ToolSession::connect(&addr)?;
    session.bootstrap()?;
    let _ = writeln!(
        out,
        "clip: {} — {} global(s) advertised",
        addr.display_string(),
        session.globals().len()
    );
    if session.has_global(CLIPBOARD_GLOBAL) {
        let _ = writeln!(
            out,
            "clip: {CLIPBOARD_GLOBAL} is served — a full stack answers the \
             negotiation choreography with real device objects"
        );
    } else {
        let _ = writeln!(
            out,
            "clip: {CLIPBOARD_GLOBAL} is NOT served — this compositor runs no \
             clipboard (the Phase 10 vertical slice composites and presents; \
             the data family arrives with the full stack); the in-process \
             negotiation below exercises the same contract the server embeds"
        );
    }
    Ok(0)
}

/// The in-process negotiation: source client, receiver client, one
/// seat, the real manager.
/// The demo seat: key 0.
const SEAT: SeatKey = SeatKey(0);

/// The payload the source serves through the pipe.
const PAYLOAD: &[u8] = b"the lion sleeps tonight";

#[allow(clippy::too_many_lines)] // one linear protocol narrative
fn local_negotiation(out: &mut dyn Write) {
    let limits = Limits::default();
    let mut manager = ClipboardManager::new(limits);

    // Two clients on one seat, each with a data device.
    let client_a = ClientKey(0x1001);
    let client_b = ClientKey(0x2001);
    let device_a = ObjectId::from_wire(0x6001);
    let device_b = ObjectId::from_wire(0x6002);
    manager.create_device(client_a, SEAT, device_a);
    manager.create_device(client_b, SEAT, device_b);
    let _ = writeln!(
        out,
        "clip: two clients bound data devices on seat {} (objects {:#x}, {:#x})",
        SEAT.0,
        device_a.as_u32(),
        device_b.as_u32()
    );

    // Client A offers two MIME types and takes the clipboard slot
    // (serial-bound ownership).
    let source_obj = ObjectId::from_wire(0x6101);
    let source = manager.create_source(client_a, source_obj);
    manager
        .source_offer(source, "text/plain;charset=utf-8")
        .expect("offer utf-8");
    manager
        .source_offer(source, "image/png")
        .expect("offer png");
    let _ = writeln!(
        out,
        "clip: client {:#x} set the selection — offers {}",
        client_a.0,
        manager
            .source_offers(source)
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    );
    let mut next_id: u32 = 0x7000;
    let batch = manager
        .set_slot(
            SEAT,
            Slot::Clipboard,
            client_a,
            Some(source),
            Some(1),
            1,
            &mut |_client| {
                next_id += 1;
                ObjectId::from_wire(next_id)
            },
        )
        .expect("set_selection");

    // The routed events: the other device client learns of the offer.
    let mut offer_id = None;
    for routed in &batch {
        let _ = writeln!(
            out,
            "clip: → client {:#x} on object {:#x}: {}",
            routed.client.0,
            routed.object.as_u32(),
            event_name(&routed.event)
        );
        if routed.client == client_b {
            if let DataEvent::DeviceDataOffer { id } = routed.event {
                offer_id = Some(id);
            }
        }
    }
    let offer = manager
        .clipboard_offer(client_b, SEAT)
        .expect("the receiver has a selection offer");
    let _ = writeln!(
        out,
        "clip: client {:#x} sees the selection offer (object {:#x})",
        client_b.0,
        offer_id.map_or(0, ObjectId::as_u32)
    );

    // Client B narrows to the MIME it understands (the accept step).
    let accepted = manager
        .offer_accept(client_b, offer, "text/plain;charset=utf-8")
        .expect("accept");
    for routed in &accepted {
        let _ = writeln!(
            out,
            "clip: → client {:#x} on object {:#x}: {}",
            routed.client.0,
            routed.object.as_u32(),
            event_name(&routed.event)
        );
    }
    let _ = writeln!(
        out,
        "clip: client {:#x} accepted text/plain;charset=utf-8 — {} routed event(s)",
        client_b.0,
        accepted.len()
    );

    // And receives: the permission gate runs first (the manifest
    // carries ClipboardRead), then the transfer is admitted and the
    // source is told to send into the pipe.
    let (read_end, write_end) = pipe::pair().expect("pipe");
    let manifest = ScopeSet::single(Scope::ClipboardRead);
    let (routed_send, transfer) = manager
        .offer_receive(
            client_b,
            offer,
            "text/plain;charset=utf-8",
            write_end,
            manifest,
            None,
        )
        .expect("receive");
    let _ = writeln!(
        out,
        "clip: transfer {} admitted — the gate saw ClipboardRead and allowed",
        transfer.as_u64()
    );
    for routed in &routed_send {
        let _ = writeln!(
            out,
            "clip: → client {:#x} on object {:#x}: {}",
            routed.client.0,
            routed.object.as_u32(),
            event_name(&routed.event)
        );
    }

    // The payload: the source client writes into its pipe end; the
    // receiver reads the bytes back.
    let written = pipe::serve(&routed_send, PAYLOAD);
    let _ = writeln!(out, "clip: the source served {written} byte(s)");
    // The pipe contract's other half: EOF reaches the reader only
    // when every write end closes — the manager hands the original
    // back to the integrator, so it closes here (the clone in
    // `serve` already dropped).
    drop(routed_send);
    let received = pipe::drain(read_end);
    let _ = writeln!(
        out,
        "clip: the receiver read {} byte(s): {:?}",
        received.len(),
        String::from_utf8_lossy(&received)
    );
    assert_eq!(received, PAYLOAD);
}

/// The event's operator-facing name.
fn event_name(event: &DataEvent) -> &'static str {
    match event {
        DataEvent::DeviceDataOffer { .. } => "data_device.data_offer",
        DataEvent::DeviceSelection { .. } => "data_device.selection",
        DataEvent::DevicePrimarySelection { .. } => "data_device.primary_selection",
        DataEvent::SourceTarget { .. } => "data_source.target",
        DataEvent::SourceSend { .. } => "data_source.send",
        DataEvent::SourceCancelled => "data_source.cancelled",
        _ => "(other)",
    }
}

/// Resolve the socket the same way the tools do.
///
/// # Errors
/// A usage-shaped [`ToolError`] when neither source names a socket.
fn resolve_socket(args: &ClipArgs) -> Result<UnixAddr, ToolError> {
    let env = std::env::var("LDP_SOCKET").ok();
    let name = args.socket.as_deref().or(env.as_deref()).map(str::to_owned);
    let Some(name) = name else {
        return Err(ToolError::Logic(
            "no socket name: pass --socket NAME or set LDP_SOCKET".to_owned(),
        ));
    };
    let name = name.strip_prefix('@').unwrap_or(&name).to_owned();
    if name.is_empty() {
        return Err(ToolError::Logic("socket name is empty".to_owned()));
    }
    UnixAddr::abstract_name(name.as_bytes()).map_err(|e| ToolError::Logic(format!("{e}")))
}

/// The tiniest pipe seam for the demo: a synchronous os pipe with the
/// write end stashed in the routed `send` event's FD slot.
mod pipe {
    use std::io::{Read, Write};
    use std::os::fd::{FromRawFd as _, OwnedFd};

    use ldp_clipboard::Routed;

    /// A pipe pair: (the receiver's read end, the source's write end).
    ///
    /// # Errors
    /// The OS error from `pipe(2)`.
    pub fn pair() -> std::io::Result<(ReadEnd, OwnedFd)> {
        // SAFETY: a plain pipe(2) into a two-slot array; both ends are
        // immediately wrapped in owned descriptors (closed on drop).
        let mut fds = [0i32; 2];
        let rc = unsafe { libc_pipe(&mut fds) };
        if rc != 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: both descriptors are fresh and owned from here.
        let read_end = unsafe { OwnedFd::from_raw_fd(fds[0]) };
        let write_end = unsafe { OwnedFd::from_raw_fd(fds[1]) };
        Ok((ReadEnd(read_end), write_end))
    }

    /// The receiver's end.
    pub struct ReadEnd(pub OwnedFd);

    /// Serve `payload` through the routed send's pipe write end.
    pub fn serve(routed: &[Routed], payload: &[u8]) -> usize {
        let Some(fd) = routed.iter().find_map(|r| r.fd.as_ref()) else {
            return 0;
        };
        let mut sink = std::fs::File::from(fd.try_clone().expect("dup write end"));
        sink.write_all(payload).expect("serve payload");
        payload.len()
    }

    /// Drain everything the source wrote.
    pub fn drain(end: ReadEnd) -> Vec<u8> {
        let mut out = Vec::new();
        let mut file = std::fs::File::from(end.0);
        let _ = file.read_to_end(&mut out);
        out
    }

    /// `pipe(2)` through the standard library's libc binding.
    ///
    /// # Safety
    /// `fds` must point to two writable `int` slots.
    unsafe fn libc_pipe(fds: &mut [i32; 2]) -> i32 {
        unsafe { libc::pipe(fds.as_mut_ptr()) }
    }
}
