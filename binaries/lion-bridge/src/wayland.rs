//! The Wayland face: a Unix-socket Wayland *display* whose clients
//! become LDP windows.
//!
//! One listener (`--wayland PATH`; a leading `@` names an abstract
//! socket), one session per foreign client: each session owns its
//! [`WlBridgeDriver`] (the pure protocol machinery) and its own LDP
//! connection — one `lion-display` client identity per foreign window
//! set, the security doctrine (identity never blurs across foreign
//! clients).
//!
//! The process layer supplies exactly what the pure crate left as
//! seams:
//!
//! * **pool memory** — `wl_shm.create_pool` descriptors arrive as
//!   SCM_RIGHTS on the client socket; the session's pool watcher
//!   watches the byte stream just enough (registry bind → `wl_shm`
//!   object → `create_pool`) to adopt each descriptor under its pool
//!   id, and the shared `WlHost` serves `read_pool` from it with
//!   positioned reads;
//! * **the keymap** — one memfd of xkb v1 text ([`Keymap`]), attached
//!   to the wire whenever the driver emits a keymap event;
//! * **the LDP pool export** — the export host mints one memfd per
//!   export pool (or grows it on resize) and mirrors each commit's
//!   pixels in, the descriptor riding the `shm.create_pool` message.

#![forbid(unsafe_code)]
use std::collections::{BTreeMap, VecDeque};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::fs::FileExt;
use std::sync::{Arc, Mutex};

use ldp_core::error::LdpError;
use ldp_core::wire::Value;
use ldp_protocol::generated::{core, shell};
use ldp_protocol::Message;
use ldp_transport::sys::{recv_with_control, send_msg, ControlBuffer, RawRecv};
use ldp_transport::{is_would_block, UnixAddr};
use ldp_wayland_bridge::dispatch::Host;
use ldp_wayland_bridge::driver::{AuthError, DriverHost, TokenCheck, WlBridgeDriver};

use crate::ldp::{LdpEvent, LdpLink};

/// The bound compositor global (the driver binds it at id 3 — the
/// documented bootstrap plan).
const COMPOSITOR_ID: u32 = 3;
/// The bound shell global (the driver binds it at id 6).
const SHELL_ID: u32 = 6;

/// The honest fallback keymap (valid xkb v1 text): clients that bind
/// a keyboard get a well-formed wire event even when no distribution
/// keymap is present.
const EMPTY_KEYMAP: &[u8] = b"xkb_keymap {\n};\n\0";

/// Why a session ended (the face closes it and reports).
#[derive(Clone, Debug)]
pub enum SessionEnd {
    /// The foreign client closed its socket.
    ClientGone,
    /// The foreign client violated the protocol.
    Protocol(String),
    /// The LDP side failed.
    Ldp(String),
}

/// The keymap blob this bridge serves.
#[derive(Clone)]
pub struct Keymap {
    blob: Arc<Vec<u8>>,
}

impl Keymap {
    /// Load the keymap: an explicit `path` first (must work or the
    /// operator hears about it), then the distribution default, else
    /// the empty fallback.
    ///
    /// # Errors
    ///
    /// [`std::io::Error`] when the explicit `path` cannot be read.
    pub fn load(path: Option<&str>) -> Result<Keymap, std::io::Error> {
        if let Some(p) = path {
            let blob = std::fs::read(p)?;
            return Ok(Keymap {
                blob: Arc::new(blob),
            });
        }
        for candidate in [
            "/etc/ldp/default-keymap.xkb",
            "/usr/share/ldp/default-keymap.xkb",
        ] {
            if let Ok(blob) = std::fs::read(candidate) {
                return Ok(Keymap {
                    blob: Arc::new(blob),
                });
            }
        }
        Ok(Keymap {
            blob: Arc::new(EMPTY_KEYMAP.to_vec()),
        })
    }

    /// The served blob (NUL-terminated xkb v1 text).
    #[must_use]
    pub fn blob(&self) -> &[u8] {
        &self.blob
    }
}

/// One adopted foreign pool: the client's descriptor plus its size.
struct ForeignPool {
    file: std::fs::File,
    size: u64,
}

/// The adoption state the dispatch's `create_pool` seam feeds and the
/// session resolves against each batch's ancillary descriptors:
/// `pools` holds every adopted pool; `wanted` holds the
/// (pool, fd-index, size) triples the dispatch reported for the batch
/// in flight (cleared once the session resolves them).
struct Adoption {
    pools: BTreeMap<u32, ForeignPool>,
    wanted: Vec<(u32, u32, u64)>,
}

type SharedAdoption = Arc<Mutex<Adoption>>;

/// The foreign-side host: pool memory and the keymap.
struct WlHost {
    adoption: SharedAdoption,
    keymap: Keymap,
}

impl Host for WlHost {
    fn read_pool(&mut self, pool: u32, offset: usize, len: usize) -> Option<Vec<u8>> {
        let guard = self.adoption.lock().expect("pool host lock");
        let p = guard.pools.get(&pool)?;
        if u64::try_from(offset.checked_add(len)?).ok()? > p.size {
            return None;
        }
        let mut out = vec![0u8; len];
        let got = p.file.read_at(&mut out, offset as u64).ok()?;
        out.truncate(got);
        Some(out)
    }

    fn keymap_fd(&mut self) -> u32 {
        0 // the first ancillary slot
    }

    fn keymap(&mut self) -> Vec<u8> {
        self.keymap.blob().to_vec()
    }

    fn create_pool(&mut self, pool: u32, fd_index: u32, size: u64) {
        // The session resolves the index against the batch's
        // descriptors right after the feed returns.
        self.adoption
            .lock()
            .expect("pool host lock")
            .wanted
            .push((pool, fd_index, size));
    }
}

/// The LDP-side export host: one memfd per pool, mirrored per commit.
struct ExportHost {
    pools: BTreeMap<u32, std::fs::File>,
}

impl ExportHost {
    fn new() -> ExportHost {
        ExportHost {
            pools: BTreeMap::new(),
        }
    }
}

impl DriverHost for ExportHost {
    fn pool_fd(&mut self, pool: u32, size: u64) -> u32 {
        match self.pools.get(&pool) {
            // Resize: the same descriptor grows (the server re-maps).
            Some(file) => {
                let _ = crate::sys::ftruncate(file.as_raw_fd(), size);
            }
            None => {
                if let Ok(fd) = crate::sys::memfd(size) {
                    self.pools.insert(pool, std::fs::File::from(fd));
                }
                // A failed mint surfaces at the send (the message's
                // descriptor is missing — the session dies loudly).
            }
        }
        0 // the first ancillary slot
    }

    fn sync_pool(&mut self, pool: u32, bytes: &[u8]) {
        if let Some(file) = self.pools.get(&pool) {
            // Positioned writes only: no shared cursor; concurrent
            // message sends never race the offset.
            let _ = file.write_at(bytes, 0);
        }
    }
}

/// The driver-side forwarder: the session shares the export host
/// with the driver's boxed host (the pool descriptors and mirrors
/// flow through one state).
struct ExportForward {
    export: Arc<Mutex<ExportHost>>,
}

impl DriverHost for ExportForward {
    fn pool_fd(&mut self, pool: u32, size: u64) -> u32 {
        self.export.lock().expect("export host").pool_fd(pool, size)
    }
    fn sync_pool(&mut self, pool: u32, bytes: &[u8]) {
        self.export
            .lock()
            .expect("export host")
            .sync_pool(pool, bytes)
    }
}

/// One foreign client session.
pub struct WlSession {
    /// The foreign socket (the owner of the polled descriptor;
    /// dropping this closes the session).
    wl: std::os::unix::net::UnixStream,
    driver: WlBridgeDriver,
    link: LdpLink,
    adoption: SharedAdoption,
    export: Arc<Mutex<ExportHost>>,
    /// Descriptors queued for outbound keymap events (one per event).
    keymap_fds: VecDeque<OwnedFd>,
    keymap: Keymap,
    /// Partial inbound bytes (a torn request tail).
    inbox: Vec<u8>,
    /// Outbound backlog the socket refused.
    outbox: Vec<u8>,
}

impl WlSession {
    /// Accept one foreign client: authenticate the driver, connect the
    /// LDP link (the driver's bootstrap rides the same connect), and
    /// create the seat's device proxies.
    ///
    /// # Errors
    ///
    /// [`SessionEnd`] when the LDP side refuses (connect, handshake,
    /// or the token gate).
    pub fn accept(
        wl: std::os::unix::net::UnixStream,
        addr: &UnixAddr,
        token: [u32; 8],
        app_id: &str,
        keymap: &Keymap,
        check: &mut dyn TokenCheck,
    ) -> Result<WlSession, SessionEnd> {
        wl.set_nonblocking(true)
            .map_err(|e| SessionEnd::Ldp(e.to_string()))?;
        let adoption: SharedAdoption = Arc::new(Mutex::new(Adoption {
            pools: BTreeMap::new(),
            wanted: Vec::new(),
        }));
        let export: Arc<Mutex<ExportHost>> = Arc::new(Mutex::new(ExportHost::new()));
        let mut driver = WlBridgeDriver::new(Box::new(WlHost {
            adoption: Arc::clone(&adoption),
            keymap: keymap.clone(),
        }))
        .with_driver_host(Box::new(ExportForward {
            export: Arc::clone(&export),
        }));
        driver
            .authenticate(app_id, token, check)
            .map_err(|e| SessionEnd::Ldp(auth_reason(e)))?;
        let bootstrap = driver.take_ldp_messages();
        let mut link =
            LdpLink::connect(addr, &bootstrap, None).map_err(|e| SessionEnd::Ldp(e.to_string()))?;
        // Input routing needs the seat's device proxies (the link's
        // own factory objects).
        let factories = link.seat_factories();
        link.send(&factories, &mut ldp_transport::fd::FdList::new())
            .map_err(|e| SessionEnd::Ldp(e.to_string()))?;
        Ok(WlSession {
            wl,
            driver,
            link,
            adoption,
            export,
            keymap_fds: VecDeque::new(),
            keymap: keymap.clone(),
            inbox: Vec::new(),
            outbox: Vec::new(),
        })
    }

    /// The foreign socket's poll row.
    #[must_use]
    pub fn interest(&self) -> crate::sys::Interest {
        crate::sys::Interest {
            fd: self.wl.as_raw_fd(),
            read: true,
            write: !self.outbox.is_empty(),
        }
    }

    /// The LDP link's poll row.
    #[must_use]
    pub fn ldp_interest(&self) -> crate::sys::Interest {
        crate::sys::Interest {
            fd: self.link.fd(),
            read: true,
            write: self.link.wants_write(),
        }
    }

    /// One pump turn (readiness from both rows).
    ///
    /// # Errors
    ///
    /// [`SessionEnd`] when either side fails fatally.
    pub fn pump(
        &mut self,
        wl_ready: &crate::sys::Ready,
        ldp_ready: &crate::sys::Ready,
    ) -> Result<(), SessionEnd> {
        if ldp_ready.readable || ldp_ready.gone {
            self.pump_ldp().map_err(|e| {
                let note = self.server_note();
                let mut end = end_of(e);
                if let SessionEnd::Ldp(text) = &mut end {
                    if !note.is_empty() {
                        *text = format!("{text}; server said: {note}");
                    }
                }
                end
            })?;
        }
        if wl_ready.readable || wl_ready.gone {
            self.pump_wl()?;
        }
        if (wl_ready.writable && !self.outbox.is_empty()) || ldp_ready.writable {
            self.flush_wl().map_err(end_of)?;
            self.link.flush().map_err(end_of)?;
        }
        Ok(())
    }

    /// The server's own rejection diagnostic, when it sent one.
    fn server_note(&self) -> String {
        server_error_of(&self.link).unwrap_or_default()
    }

    /// Read the foreign socket, adopt pool descriptors, feed the
    /// driver, write its events back.
    fn pump_wl(&mut self) -> Result<(), SessionEnd> {
        let mut buf = [0u8; 64 * 1024];
        let mut control = ControlBuffer::with_fd_capacity(32);
        let got: RawRecv = match recv_with_control(self.wl.as_raw_fd(), &mut buf, &mut control) {
            Ok(g) => g,
            Err(e) if is_would_block(&e) => return Ok(()),
            Err(e) => return Err(SessionEnd::ClientGone).map_err(|_| end_of(e)),
        };
        if got.bytes == 0 {
            return Err(SessionEnd::ClientGone);
        }
        // Adopt any riding descriptors.
        let mut fds: Vec<OwnedFd> = Vec::new();
        if got.control_used > 0 {
            let mut list = ldp_transport::fd::FdList::new();
            if control
                .parse_rights_into(got.control_used, &mut list)
                .is_ok()
            {
                fds = list.take_all();
            }
        }
        // Accumulate (a torn tail completes next turn).
        self.inbox.extend_from_slice(&buf[..got.bytes]);
        let batch = std::mem::take(&mut self.inbox);
        // Feed the pure machinery — its `create_pool` seam reports the
        // (pool, fd-index, size) triples as it parses them.
        if let Err(fatal) = self.driver.wl_feed(&batch) {
            return Err(SessionEnd::Protocol(fatal_reason(&fatal)));
        }
        // Resolve the reported adoptions against this batch's
        // descriptors (the indexes address the ancillary array the
        // same recvmsg delivered; unclaimed descriptors close with
        // the scratch list — the Wayland rule).
        {
            let mut guard = self.adoption.lock().expect("pool host lock");
            for (pool_id, fd_index, size) in std::mem::take(&mut guard.wanted) {
                if let Some(fd) = fds.get_mut(fd_index as usize) {
                    if let Some(file) = fd.try_clone().ok().map(std::fs::File::from) {
                        guard.pools.insert(pool_id, ForeignPool { file, size });
                    }
                }
            }
        }
        drop(fds);
        // The frame tick: a foreign commit is content readiness (the
        // driver exports newly mapped surfaces and re-commits changed
        // ones). The presented-driven tick below carries the
        // steady-state cycle; this one starts it.
        self.driver.on_frame_tick();
        self.drain_driver_output().map_err(end_of)?;
        self.drain_ldp_messages().map_err(end_of)?;
        Ok(())
    }

    /// Pump the LDP link: route events into the driver.
    fn pump_ldp(&mut self) -> Result<(), ldp_core::error::LdpError> {
        loop {
            match self.link.recv_one() {
                Ok(Some(event)) => self.apply(event)?,
                Ok(None) => break,
                Err(e) => return Err(e),
            }
        }
        self.drain_ldp_messages()?;
        Ok(())
    }

    /// Route one LDP event into the driver's translation surface.
    fn apply(&mut self, event: LdpEvent) -> Result<(), ldp_core::error::LdpError> {
        match event {
            LdpEvent::Configure {
                toplevel,
                width,
                height,
            } => self.driver.on_ldp_configure(toplevel, width, height),
            // The Wayland face exports toplevels only (xdg_popup
            // translation is its own roadmap line): popup events
            // never arrive, and the arms say so.
            LdpEvent::PopupConfigure { .. } | LdpEvent::PopupDone { .. } => {}
            LdpEvent::Close { toplevel } => {
                self.driver.on_ldp_close(toplevel);
            }
            LdpEvent::Presented => {
                // The frame landed: tick the foreign re-commit cycle.
                self.driver.on_frame_tick();
            }
            LdpEvent::PointerMotion { surface, x, y } => {
                self.driver.ldp_pointer(surface, x, y);
            }
            LdpEvent::PointerLeave => {}
            LdpEvent::PointerButton { button, pressed } => {
                // Button routing is export-agnostic in the v1 shape
                // (the pure driver broadcasts to the foreign pointers).
                self.driver.ldp_pointer_button(0, button, pressed);
            }
            LdpEvent::KeyboardEnter { surface, keys } => {
                self.driver.ldp_keyboard_enter(surface, &keys);
            }
            LdpEvent::KeyboardLeave => {
                self.driver.ldp_keyboard_leave(0);
            }
            LdpEvent::Key { keycode, pressed } => {
                self.driver.ldp_key(keycode, pressed);
            }
        }
        self.drain_driver_output()?;
        Ok(())
    }

    /// Write the driver's pending Wayland events toward the socket.
    fn drain_driver_output(&mut self) -> Result<(), ldp_core::error::LdpError> {
        let (bytes, fd_count) = self.driver.wl_output();
        if bytes.is_empty() {
            return Ok(());
        }
        // One keymap memfd per fd-carrying event (the keymap is
        // send-once per keyboard object — each gets its own copy).
        for _ in 0..fd_count {
            let fd = crate::sys::memfd(self.keymap.blob().len() as u64).map_err(ldp_io)?;
            let mut file = std::fs::File::from(fd);
            use std::io::Write as _;
            let _ = file.write_all(self.keymap.blob());
            let _ = file.flush();
            self.keymap_fds.push_back(file.into());
        }
        self.outbox.extend_from_slice(&bytes);
        Ok(())
    }

    /// Send the driver's pending LDP messages (pool descriptors ride
    /// their `create_pool` messages; the link learns the export ids).
    fn drain_ldp_messages(&mut self) -> Result<(), ldp_core::error::LdpError> {
        let msgs = self.driver.take_ldp_messages();
        if msgs.is_empty() {
            return Ok(());
        }
        // Register the ids the link routes by: the compositor's
        // create_surface and the shell's get_toplevel mint the
        // export's surface and toplevel.
        for m in &msgs {
            if m.object_id == COMPOSITOR_ID && m.opcode == core::compositor::request::CREATE_SURFACE
            {
                if let Some(id) = new_id_of(m) {
                    self.link.watch_surface(id);
                }
            }
            if m.object_id == SHELL_ID && m.opcode == shell::shell::request::GET_TOPLEVEL {
                if let Some(id) = new_id_of(m) {
                    self.link.watch_toplevel(id);
                }
            }
        }
        let mut plain: Vec<Message> = Vec::new();
        for m in msgs {
            let carries_fd = m.args.iter().any(|a| matches!(a, Value::Fd(_)));
            if carries_fd {
                if !plain.is_empty() {
                    self.link
                        .send(&plain, &mut ldp_transport::fd::FdList::new())?;
                    plain.clear();
                }
                // The pool id is the message's new_id argument; the
                // descriptor is the host's memfd for that pool (a
                // dup — the SCM_RIGHTS send dups again, the original
                // stays live for the mirror writes).
                let pool = new_id_of(&m).unwrap_or(0);
                let file = self
                    .export
                    .lock()
                    .expect("export host")
                    .pools
                    .get(&pool)
                    .and_then(|f| f.try_clone().ok());
                match file {
                    Some(file) => {
                        self.link
                            .send_with_fd(std::slice::from_ref(&m), file.into())?;
                    }
                    None => {
                        return Err(ldp_core::error::LdpError::Logic {
                            what: "the export host lost the pool descriptor",
                        });
                    }
                }
            } else {
                plain.push(m);
            }
        }
        if !plain.is_empty() {
            self.link
                .send(&plain, &mut ldp_transport::fd::FdList::new())?;
        }
        Ok(())
    }

    /// Push the outbound backlog toward the foreign socket.
    fn flush_wl(&mut self) -> Result<(), LdpError> {
        while !self.outbox.is_empty() {
            let take = self.outbox.len().min(64 * 1024);
            // A queued keymap descriptor rides the head's send.
            let (prefix, pop_fd) = match self.keymap_fds.front() {
                Some(fd) => {
                    let mut control = ControlBuffer::with_fd_capacity(1);
                    let used = control.encode_rights(&[fd.as_raw_fd()])?;
                    (Some(control.encoded_prefix(used).to_vec()), true)
                }
                None => (None, false),
            };
            match send_msg(self.wl.as_raw_fd(), &self.outbox[..take], prefix.as_deref()) {
                Ok(sent) => {
                    if pop_fd {
                        self.keymap_fds.pop_front();
                    }
                    self.outbox.drain(..sent);
                }
                Err(e) if is_would_block(&e) => return Ok(()),
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

fn auth_reason(e: AuthError) -> String {
    match e {
        AuthError::Rejected => "the bridge token was rejected".into(),
        AuthError::AlreadyRunning => "the driver was already authenticated".into(),
    }
}

fn fatal_reason(f: &ldp_wayland_bridge::dispatch::Fatal) -> String {
    format!("{f:?}")
}

fn ldp_io(e: std::io::Error) -> ldp_core::error::LdpError {
    ldp_core::error::LdpError::Io(std::sync::Arc::new(e))
}

fn end_of(e: ldp_core::error::LdpError) -> SessionEnd {
    SessionEnd::Ldp(e.to_string())
}

/// The link's death diagnostic, if it carried one (the server's own
/// `connection.error` message — the reason the session ended).
#[must_use]
pub fn server_error_of(link: &LdpLink) -> Option<String> {
    match link.death() {
        Some(crate::ldp::LinkDeath::Server(m)) => Some(m.clone()),
        _ => None,
    }
}

fn new_id_of(m: &Message) -> Option<u32> {
    m.args.iter().find_map(|a| match a {
        Value::NewId(id) => Some(id.as_u32()),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fallback_keymap_is_valid_xkb_v1_text() {
        let km = Keymap::load(None).expect("the fallback always loads");
        assert!(km.blob().starts_with(b"xkb_keymap"));
        assert!(km.blob().ends_with(b"\0"));
    }

    #[test]
    fn the_adoption_state_resolves_the_dispatch_reports() {
        // The host half: the dispatch's create_pool seam reports the
        // (pool, fd-index, size) triples.
        let adoption: SharedAdoption = Arc::new(Mutex::new(Adoption {
            pools: BTreeMap::new(),
            wanted: Vec::new(),
        }));
        let mut host = WlHost {
            adoption: Arc::clone(&adoption),
            keymap: Keymap::load(None).expect("the fallback"),
        };
        host.create_pool(12, 0, 64);
        host.create_pool(14, 1, 128);
        // The session half: resolve against the batch's descriptors.
        let mut fds: Vec<OwnedFd> = vec![
            crate::sys::memfd(64).expect("memfd"),
            crate::sys::memfd(128).expect("memfd"),
        ];
        {
            let mut guard = adoption.lock().expect("pool host lock");
            for (pool_id, fd_index, size) in std::mem::take(&mut guard.wanted) {
                if let Some(fd) = fds.get_mut(fd_index as usize) {
                    if let Some(file) = fd.try_clone().ok().map(std::fs::File::from) {
                        guard.pools.insert(pool_id, ForeignPool { file, size });
                    }
                }
            }
        }
        // The dispatch's read_pool now serves the adopted descriptors.
        assert_eq!(host.read_pool(12, 0, 4).as_deref(), Some(&[0, 0, 0, 0][..]));
        assert_eq!(host.read_pool(14, 0, 4).as_deref(), Some(&[0, 0, 0, 0][..]));
        // Bounds discipline: past the pool's size refuses.
        assert!(host.read_pool(12, 60, 8).is_none());
        // Unknown pools refuse.
        assert!(host.read_pool(99, 0, 4).is_none());
        // The originals stay alive for the session (the clones ride).
        assert_eq!(fds.len(), 2);
    }

    #[test]
    fn an_unresolved_index_leaves_the_pool_absent() {
        let adoption: SharedAdoption = Arc::new(Mutex::new(Adoption {
            pools: BTreeMap::new(),
            wanted: Vec::new(),
        }));
        let mut host = WlHost {
            adoption: Arc::clone(&adoption),
            keymap: Keymap::load(None).expect("the fallback"),
        };
        // The descriptor was lost (a short ancillary array): the pool
        // stays absent — the honest refusal, never a panic.
        host.create_pool(12, 3, 64);
        let mut fds: Vec<OwnedFd> = vec![crate::sys::memfd(64).expect("memfd")];
        {
            let mut guard = adoption.lock().expect("pool host lock");
            for (pool_id, fd_index, size) in std::mem::take(&mut guard.wanted) {
                if let Some(fd) = fds.get_mut(fd_index as usize) {
                    if let Some(file) = fd.try_clone().ok().map(std::fs::File::from) {
                        guard.pools.insert(pool_id, ForeignPool { file, size });
                    }
                }
            }
        }
        assert!(host.read_pool(12, 0, 4).is_none());
    }
}
