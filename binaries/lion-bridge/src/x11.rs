//! The X11 face: a Unix-socket X *display* whose clients become one
//! rootful LDP window.
//!
//! One listener (`--x11 PATH`), one shared X server
//! ([`XBridgeDriver`]'s), one LDP connection: the whole X screen —
//! every foreign window, the root, the cursor glyphs the clients
//! draw — composites as a single rootful LDP toplevel, the
//! XWayland-rootless trade made explicit (rootless splitting is the
//! shell-protocol roadmap line).
//!
//! The process layer supplies what the pure crate left as seams:
//!
//! * **MIT-SHM backing** — the dispatcher's `attach_segment` seam
//!   hands every `ShmAttach`'s `(XID, shmid)` to the SysV segment
//!   host, which maps the segment and serves the read/write host seam
//!   through it;
//! * **the LDP pool export** — one memfd for the root pool, grown on
//!   resize, mirrored per frame tick (the root pool host);
//! * **the event clock** — X events carry a time field; the face
//!   stamps them from a monotonic start (the pure crates read no
//!   clock).

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::os::fd::AsRawFd;
use std::os::unix::fs::FileExt;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use ldp_core::wire::Value;
use ldp_protocol::Message;
use ldp_transport::sys::{recv_plain, send_msg};
use ldp_transport::{is_would_block, UnixAddr};
#[cfg(test)]
use ldp_x11_bridge::dispatch::ext::SHM as X11_SHM_OPCODE;
use ldp_x11_bridge::dispatch::ClosePolicy;
use ldp_x11_bridge::driver::{AuthError, DriverHost, TokenCheck, XBridgeDriver};
use ldp_x11_bridge::setup::ScreenParams;
use ldp_x11_bridge::shm::ShmHost;

use crate::ldp::{LdpEvent, LdpLink};
use crate::sys::{Interest, Ready, ShmMap};
use ldp_x11_bridge::rootless::RootlessDriver;

/// Why a connection or the face ended.
#[derive(Clone, Debug)]
pub enum SessionEnd {
    /// The foreign client closed its socket.
    ClientGone,
    /// The foreign client violated the protocol.
    Protocol(String),
    /// The LDP side failed.
    Ldp(String),
}

/// The MIT-SHM host over real SysV segments: the dispatcher's
/// `attach_segment` seam maps each `ShmAttach`'d shmid under its XID.
struct SysvShm {
    segments: BTreeMap<u32, ShmMap>,
}

impl ShmHost for SysvShm {
    fn read_segment(&mut self, seg: u32, offset: usize, len: usize) -> Option<Vec<u8>> {
        self.segments.get(&seg)?.read(offset, len)
    }

    fn write_segment(&mut self, seg: u32, offset: usize, data: &[u8]) -> Option<()> {
        self.segments.get_mut(&seg)?.write(offset, data)
    }

    fn attach_segment(&mut self, seg: u32, shmid: u64, read_only: bool) {
        // The read-only flag is advisory on the host side (the bridge
        // maps read-write: `ShmGetImage` writes back); a dead shmid
        // leaves the segment unmapped, so its first use fails the
        // honest way (the dispatcher's `None` → the X error path).
        let _ = read_only;
        let Ok(key) = libc::key_t::try_from(shmid) else {
            return;
        };
        if let Ok(map) = ShmMap::attach(key) {
            self.segments.insert(seg, map);
        }
    }
}

/// The root pool's export host: one memfd, grown on resize.
struct RootPoolHost {
    file: Option<std::fs::File>,
}

impl DriverHost for RootPoolHost {
    fn pool_fd(&mut self, _pool: u32, size: u64) -> u32 {
        match &self.file {
            // Resize: the same descriptor grows (the server re-maps).
            Some(file) => {
                let _ = crate::sys::ftruncate(file.as_raw_fd(), size);
            }
            None => {
                if let Ok(fd) = crate::sys::memfd(size) {
                    self.file = Some(std::fs::File::from(fd));
                }
            }
        }
        0 // the first ancillary slot
    }

    fn sync_pool(&mut self, _pool: u32, bytes: &[u8]) {
        if let Some(file) = &self.file {
            // Positioned writes only: no shared cursor; concurrent
            // message sends never race the offset.
            let _ = file.write_at(bytes, 0);
        }
    }
}

/// One connected X client.
struct XClient {
    /// The client socket (the owner of the polled descriptor;
    /// dropping this closes the connection).
    stream: std::os::unix::net::UnixStream,
    id: ldp_x11_bridge::dispatch::ClientId,
    outbox: Vec<u8>,
    /// True once the server saw the connection's setup (only then do
    /// request frames flow).
    live: bool,
}

/// Which X-to-LDP proxy serves this face: the rootless driver (the
/// default — every top-level X window rides its own LDP surface, OR
/// windows anchor as popups) or the rootful one (the whole X screen
/// as one window, the `--x11-rootful` escape).
enum XDriver {
    /// Rootful: one toplevel, one buffer, the root composite whole.
    Rootful(ldp_x11_bridge::driver::XBridgeDriver),
    /// Rootless: per-window surfaces, the subtree composite, the
    /// popup-anchored OR windows.
    Rootless(RootlessDriver),
}

impl XDriver {
    /// The bridge-scope token gate (both drivers refuse without it).
    fn authenticate(
        &mut self,
        app_id: &str,
        token: [u32; 8],
        check: &mut dyn TokenCheck,
    ) -> Result<(), AuthError> {
        match self {
            Self::Rootful(d) => d.authenticate(app_id, token, check),
            Self::Rootless(d) => d.authenticate(app_id, token, check),
        }
    }

    /// The X server state (client attach/remove, feeding).
    fn server_mut(&mut self) -> &mut ldp_x11_bridge::dispatch::Server {
        match self {
            Self::Rootful(d) => d.server_mut(),
            Self::Rootless(d) => d.server_mut(),
        }
    }

    /// The frame tick (both doctrines: distribute damage, commit).
    fn on_frame_tick(&mut self) {
        match self {
            Self::Rootful(d) => d.on_frame_tick(),
            Self::Rootless(d) => d.on_frame_tick(),
        }
    }

    /// The pending LDP messages.
    fn take_ldp_messages(&mut self) -> Vec<ldp_protocol::Message> {
        match self {
            Self::Rootful(d) => d.take_ldp_messages(),
            Self::Rootless(d) => d.take_ldp_messages(),
        }
    }

    /// The presented verdict.
    fn on_presented(&mut self) {
        match self {
            Self::Rootful(d) => d.on_presented(),
            Self::Rootless(d) => d.on_presented(),
        }
    }

    /// A toplevel configure (the rootless driver ignores proposals —
    /// the X client owns its geometry; the rootful one resizes the
    /// root).
    fn on_ldp_configure(&mut self, toplevel: u32, w: u32, h: u32) {
        match self {
            Self::Rootful(d) => d.on_ldp_configure(w, h),
            Self::Rootless(d) => d.on_ldp_configure(toplevel, w, h),
        }
    }

    /// A close routed at one window (the ICCCM dance).
    fn on_ldp_close(&mut self, toplevel: u32, time: u32) -> ClosePolicy {
        match self {
            Self::Rootful(d) => d.on_ldp_close(time),
            Self::Rootless(d) => d.on_ldp_close(toplevel, time),
        }
    }

    /// A pointer motion (surface-local — the rootless driver
    /// translates through the owning X window's origin).
    fn ldp_pointer_motion(&mut self, surface: u32, x: i32, y: i32, time: u32) {
        match self {
            Self::Rootful(d) => d.ldp_pointer_motion(x, y, time),
            Self::Rootless(d) => d.ldp_pointer_motion(surface, x, y, time),
        }
    }

    /// A pointer button.
    fn ldp_pointer_button(&mut self, button: u8, pressed: bool, time: u32) {
        match self {
            Self::Rootful(d) => d.ldp_pointer_button(button, pressed, time),
            Self::Rootless(d) => d.ldp_pointer_button(button, pressed, time),
        }
    }

    /// A key event.
    fn ldp_key(&mut self, keycode: u32, pressed: bool, time: u32) {
        match self {
            Self::Rootful(d) => d.ldp_key(keycode, pressed, time),
            Self::Rootless(d) => d.ldp_key(keycode, pressed, time),
        }
    }

    /// A keyboard focus transition.
    fn ldp_keyboard_focus(&mut self, focused: bool, time: u32) {
        match self {
            Self::Rootful(d) => d.ldp_keyboard_focus(focused, time),
            Self::Rootless(d) => d.ldp_keyboard_focus(focused, time),
        }
    }

    /// The watch feed: (kind, role id, surface id) per export — the
    /// link's event routing registration.
    fn watch_ids(&self) -> Vec<(ldp_x11_bridge::rootless::WatchKind, u32, u32)> {
        match self {
            Self::Rootful(_) => vec![(
                ldp_x11_bridge::rootless::WatchKind::Toplevel,
                ldp_x11_bridge::driver::ids::TOPLEVEL,
                ldp_x11_bridge::driver::ids::SURFACE,
            )],
            Self::Rootless(d) => d.watch_ids(),
        }
    }
}

/// The X11 face: the listener, the shared server, the LDP link.
pub struct X11Face {
    listener: std::os::unix::net::UnixListener,
    clients: Vec<XClient>,
    driver: XDriver,
    link: LdpLink,
    root_pool: Arc<Mutex<RootPoolHost>>,
    started: Instant,
}

impl X11Face {
    /// Bind the listener and bring up the driver: the token gate,
    /// the LDP connection, the seat's device proxies. `rootless`
    /// picks the proxy — the default splits at the natural seam
    /// (every top-level X window rides its own LDP surface, OR
    /// windows anchor as popups); the `--x11-rootful` escape serves
    /// the whole X screen as one window.
    ///
    /// # Errors
    ///
    /// [`SessionEnd`] when the socket cannot be bound or the LDP side
    /// refuses.
    pub fn open(
        path: &str,
        screen: ScreenParams,
        addr: &UnixAddr,
        token: [u32; 8],
        app_id: &str,
        check: &mut dyn TokenCheck,
        rootless: bool,
    ) -> Result<X11Face, SessionEnd> {
        // The conventional X socket directory, created on demand.
        if let Some(dir) = std::path::Path::new(path).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::remove_file(path);
        let listener = std::os::unix::net::UnixListener::bind(path)
            .map_err(|e| SessionEnd::Ldp(e.to_string()))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| SessionEnd::Ldp(e.to_string()))?;
        let shm = Arc::new(Mutex::new(SysvShm {
            segments: BTreeMap::new(),
        }));
        let root_pool = Arc::new(Mutex::new(RootPoolHost { file: None }));
        let driver = if rootless {
            XDriver::Rootless(
                RootlessDriver::new(screen)
                    .with_shm_host(Box::new(ShmForward {
                        shm: Arc::clone(&shm),
                    }))
                    .with_driver_host(Box::new(PoolForward {
                        pool: Arc::clone(&root_pool),
                    })),
            )
        } else {
            XDriver::Rootful(
                XBridgeDriver::new(screen)
                    .with_shm_host(Box::new(ShmForward {
                        shm: Arc::clone(&shm),
                    }))
                    .with_driver_host(Box::new(PoolForward {
                        pool: Arc::clone(&root_pool),
                    })),
            )
        };
        let mut driver = driver;
        XDriver::authenticate(&mut driver, app_id, token, check)
            .map_err(|e| SessionEnd::Ldp(auth_reason(e)))?;
        let bootstrap = driver.take_ldp_messages();
        // The rootful bootstrap carries the root pool's descriptor
        // (its `create_pool` rides the same connect); the rootless
        // one mints its pool with the first window — no descriptor.
        let bootstrap_fd = if rootless {
            None
        } else {
            root_pool
                .lock()
                .expect("root pool")
                .file
                .as_ref()
                .and_then(|f| f.try_clone().ok())
                .map(std::os::fd::OwnedFd::from)
        };
        let mut link = LdpLink::connect(addr, &bootstrap, bootstrap_fd)
            .map_err(|e| SessionEnd::Ldp(e.to_string()))?;
        // The fixed rootful ids / the dynamic rootless set: the watch
        // feed registers the link's event routing.
        for (kind, role, surface) in driver.watch_ids() {
            match kind {
                ldp_x11_bridge::rootless::WatchKind::Toplevel => {
                    link.watch_toplevel(role);
                }
                ldp_x11_bridge::rootless::WatchKind::Popup => {
                    link.watch_popup(role);
                }
            }
            link.watch_surface(surface);
        }
        let factories = link.seat_factories();
        link.send(&factories, &mut ldp_transport::fd::FdList::new())
            .map_err(|e| SessionEnd::Ldp(e.to_string()))?;
        // The `shm` Arc's ownership moves into the driver's boxed
        // host (the face holds no second reference — the seam is the
        // only path).
        Ok(X11Face {
            listener,
            clients: Vec::new(),
            driver,
            link,
            root_pool,
            started: Instant::now(),
        })
    }

    /// The listener's poll row.
    #[must_use]
    pub fn listener_interest(&self) -> Interest {
        Interest {
            fd: self.listener.as_raw_fd(),
            read: true,
            write: false,
        }
    }

    /// The LDP link's poll row.
    #[must_use]
    pub fn ldp_interest(&self) -> Interest {
        Interest {
            fd: self.link.fd(),
            read: true,
            write: self.link.wants_write(),
        }
    }

    /// Every client's poll row (drained by the engine's single turn).
    #[must_use]
    pub fn client_interests(&self) -> Vec<Interest> {
        self.clients
            .iter()
            .map(|c| Interest {
                fd: c.stream.as_raw_fd(),
                read: true,
                write: !c.outbox.is_empty(),
            })
            .collect()
    }

    /// One engine turn: accept, read clients, pump the LDP link.
    ///
    /// # Errors
    ///
    /// [`SessionEnd`] when the LDP side fails fatally (the face ends;
    /// client-level failures close that client only).
    pub fn turn(&mut self, listener_ready: bool, ldp_ready: &Ready) -> Result<(), SessionEnd> {
        if listener_ready {
            while let Ok((stream, _)) = self.listener.accept() {
                let _ = stream.set_nonblocking(true);
                let id = self.driver.server_mut().add_client();
                self.clients.push(XClient {
                    stream,
                    id,
                    outbox: Vec::new(),
                    live: false,
                });
            }
        }
        // Read every client once (nonblocking: EAGAIN is a no-op; the
        // dispatcher's own seams drive the SHM mappings).
        let mut closed: Vec<usize> = Vec::new();
        for (i, client) in self.clients.iter_mut().enumerate() {
            match Self::pump_client(&mut self.driver, client) {
                Ok(()) => {}
                Err(SessionEnd::ClientGone) => closed.push(i),
                Err(e) => return Err(e),
            }
        }
        for i in closed.iter().rev() {
            let client = self.clients.remove(*i);
            self.driver.server_mut().remove_client(client.id);
        }
        // The frame tick: X drawing is content readiness (the driver
        // distributes the damage and commits — the rootful root store
        // or the rootless per-window frames).
        // The presented-driven tick below carries the steady-state
        // cycle; this one starts it.
        if !self.clients.is_empty() {
            self.driver.on_frame_tick();
            if std::env::var("LDP_BRIDGE_DEBUG").is_ok() {
                let pending = self.driver.take_ldp_messages();
                let windows = match &self.driver {
                    XDriver::Rootful(_) => {
                        usize::from(self.driver.server_mut().root_store().is_some())
                    }
                    XDriver::Rootless(_) => self.driver.server_mut().top_level_windows().len(),
                };
                eprintln!(
                    "DBG x11 tick: {} ldp message(s) pending, exports={} clients={}",
                    pending.len(),
                    windows,
                    self.clients.len()
                );
                // The drain consumed them; the re-tick below re-derives.
                drop(pending);
            }
            self.driver.on_frame_tick(); // re-derive (messages were taken)
            self.drain_ldp_messages().map_err(end_of)?;
            // The rootless watch sync: newly minted windows (and
            // teardowns) keep the link's routing tables current.
            self.sync_watches();
        }
        if ldp_ready.readable || ldp_ready.gone {
            self.pump_ldp().map_err(end_of)?;
        }
        if ldp_ready.writable {
            self.link.flush().map_err(end_of)?;
        }
        // The quiescent keepalive: nothing flowed either direction
        // this turn (no X drawing, no presentation, no inbound
        // events), so the compositor's parked events — input among
        // them — would wait for our next message forever. One
        // `connection.sync` polls them out (the protocol's own
        // keepalive: never advances protocol state).
        if !self.clients.is_empty() && !ldp_ready.readable && !ldp_ready.writable {
            self.link.sync().map_err(end_of)?;
        }
        Ok(())
    }

    /// Read one client, feed the shared server, write its pending
    /// output — the flush runs on **every** turn, read or not: the
    /// server's spontaneous output (input events the driver injected)
    /// parks in its per-client queue, and a client that only listens
    /// (a menu waiting for its click) would never see it otherwise.
    fn pump_client(driver: &mut XDriver, client: &mut XClient) -> Result<(), SessionEnd> {
        let mut buf = [0u8; 64 * 1024];
        match recv_plain(client.stream.as_raw_fd(), &mut buf) {
            Ok(g) => {
                if g.bytes == 0 {
                    return Err(SessionEnd::ClientGone);
                }
                let outcome = driver.server_mut().feed(client.id, &buf[..g.bytes]);
                client.live = true;
                if let Err(reason) = outcome {
                    return Err(SessionEnd::Protocol(format!(
                        "x11 wire violation: {reason:?}"
                    )));
                }
            }
            Err(e) if is_would_block(&e) => {}
            Err(_) => return Err(SessionEnd::ClientGone),
        }
        // Write this client's pending output (flushed every turn).
        let out = driver.server_mut().take_output(client.id);
        if !out.is_empty() {
            client.outbox.extend_from_slice(&out);
        }
        while !client.outbox.is_empty() {
            let take = client.outbox.len().min(64 * 1024);
            match send_msg(client.stream.as_raw_fd(), &client.outbox[..take], None) {
                Ok(sent) => {
                    client.outbox.drain(..sent);
                }
                Err(e) if is_would_block(&e) => break,
                Err(_) => return Err(SessionEnd::ClientGone),
            }
        }
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
        let time = self.time_ms();
        match event {
            LdpEvent::Configure {
                toplevel,
                width,
                height,
            } => {
                let (w, h) = (
                    u32::try_from(width.max(0)).unwrap_or(0),
                    u32::try_from(height.max(0)).unwrap_or(0),
                );
                self.driver.on_ldp_configure(toplevel, w, h);
            }
            LdpEvent::Close { toplevel } => {
                let _ = self.driver.on_ldp_close(toplevel, time);
            }
            LdpEvent::Presented => {
                self.driver.on_presented();
                self.driver.on_frame_tick();
            }
            LdpEvent::PointerMotion { surface, x, y } => {
                eprintln!("DBG motion arrives at bridge: surface={surface:x} ({x},{y})");
                self.driver.ldp_pointer_motion(surface, x, y, time);
            }
            LdpEvent::PopupConfigure { popup, serial } => {
                // The solver's placement proposal: the rootless
                // driver acks (the X geometry stays the X truth).
                if let XDriver::Rootless(d) = &mut self.driver {
                    d.on_ldp_popup_configure(popup, serial);
                }
            }
            LdpEvent::PopupDone { popup } => {
                if let XDriver::Rootless(d) = &mut self.driver {
                    d.on_ldp_popup_done(popup);
                }
            }
            LdpEvent::PointerLeave => {}
            LdpEvent::PointerButton { button, pressed } => {
                let x_button = u8::try_from(button).unwrap_or(1);
                self.driver.ldp_pointer_button(x_button, pressed, time);
            }
            LdpEvent::KeyboardEnter {
                surface: _,
                keys: _,
            } => {
                self.driver.ldp_keyboard_focus(true, time);
            }
            LdpEvent::KeyboardLeave => {
                self.driver.ldp_keyboard_focus(false, time);
            }
            LdpEvent::Key { keycode, pressed } => {
                self.driver.ldp_key(keycode, pressed, time);
            }
        }
        self.drain_ldp_messages()?;
        Ok(())
    }

    /// Send the driver's pending LDP messages (the root pool's
    /// descriptor rides its `create_pool`).
    fn drain_ldp_messages(&mut self) -> Result<(), ldp_core::error::LdpError> {
        let msgs = self.driver.take_ldp_messages();
        if msgs.is_empty() {
            return Ok(());
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
                let file = self
                    .root_pool
                    .lock()
                    .expect("root pool")
                    .file
                    .as_ref()
                    .and_then(|f| f.try_clone().ok());
                match file {
                    Some(file) => {
                        self.link
                            .send_with_fd(std::slice::from_ref(&m), file.into())?;
                    }
                    None => {
                        return Err(ldp_core::error::LdpError::Logic {
                            what: "the root pool descriptor was never minted",
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

    /// Milliseconds since the face started (the X event clock).
    fn time_ms(&self) -> u32 {
        u32::try_from(self.started.elapsed().as_millis()).unwrap_or(0)
    }

    /// Re-register the link's routing tables from the driver's watch
    /// feed (idempotent — the rootless set is dynamic).
    fn sync_watches(&mut self) {
        for (kind, role, surface) in self.driver.watch_ids() {
            match kind {
                ldp_x11_bridge::rootless::WatchKind::Toplevel => {
                    self.link.watch_toplevel(role);
                }
                ldp_x11_bridge::rootless::WatchKind::Popup => {
                    self.link.watch_popup(role);
                }
            }
            self.link.watch_surface(surface);
        }
    }
}

/// The shared-host forwarders (the driver owns its boxes; the face
/// shares the state with the seams).
struct ShmForward {
    shm: Arc<Mutex<SysvShm>>,
}

impl ShmHost for ShmForward {
    fn read_segment(&mut self, seg: u32, offset: usize, len: usize) -> Option<Vec<u8>> {
        self.shm
            .lock()
            .expect("shm host lock")
            .read_segment(seg, offset, len)
    }
    fn write_segment(&mut self, seg: u32, offset: usize, data: &[u8]) -> Option<()> {
        self.shm
            .lock()
            .expect("shm host lock")
            .write_segment(seg, offset, data)
    }
    fn attach_segment(&mut self, seg: u32, shmid: u64, read_only: bool) {
        self.shm
            .lock()
            .expect("shm host lock")
            .attach_segment(seg, shmid, read_only)
    }
}

struct PoolForward {
    pool: Arc<Mutex<RootPoolHost>>,
}

impl DriverHost for PoolForward {
    fn pool_fd(&mut self, pool: u32, size: u64) -> u32 {
        self.pool.lock().expect("root pool").pool_fd(pool, size)
    }
    fn sync_pool(&mut self, pool: u32, bytes: &[u8]) {
        self.pool.lock().expect("root pool").sync_pool(pool, bytes)
    }
}

fn auth_reason(e: AuthError) -> String {
    match e {
        AuthError::Rejected => "the bridge token was rejected".into(),
        AuthError::AlreadyRunning => "the driver was already authenticated".into(),
    }
}

fn end_of(e: ldp_core::error::LdpError) -> SessionEnd {
    SessionEnd::Ldp(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shm_host_maps_and_round_trips() {
        // Create, attach through the seam, THEN mark for destruction
        // (the mapping holds it alive until the host drops).
        let shmid = crate::sys::create_shm_segment(4096).expect("the private segment");
        let mut host = SysvShm {
            segments: BTreeMap::new(),
        };
        host.attach_segment(0x2100_0001, shmid as u64, false);
        crate::sys::mark_shm_destroyed(shmid).expect("marked");
        host.write_segment(0x2100_0001, 4, b"bridge")
            .expect("the mapped segment writes");
        assert_eq!(
            host.read_segment(0x2100_0001, 4, 6).as_deref(),
            Some(&b"bridge"[..])
        );
        assert!(
            host.read_segment(0x2100_0001, 4090, 8).is_none(),
            "past the end"
        );
        // Unknown segments refuse honestly.
        assert!(host.read_segment(0xDEAD_BEEF, 0, 4).is_none());
    }

    #[test]
    fn a_dead_shmid_leaves_the_segment_unmapped() {
        let mut host = SysvShm {
            segments: BTreeMap::new(),
        };
        host.attach_segment(0x2100_0002, u64::from(u32::MAX), false);
        assert!(host.read_segment(0x2100_0002, 0, 4).is_none());
    }

    #[test]
    fn the_root_pool_grows_without_replacing_its_descriptor() {
        let mut host = RootPoolHost { file: None };
        assert_eq!(host.pool_fd(8, 4096), 0);
        let first = host.file.as_ref().expect("minted").as_raw_fd();
        // Resize keeps the descriptor (the LDP server re-maps the
        // same fd; a replacement would orphan its mapping).
        assert_eq!(host.pool_fd(8, 8192), 0);
        assert_eq!(host.file.as_ref().expect("still there").as_raw_fd(), first);
        host.sync_pool(8, b"pixels");
        let mut read = Vec::new();
        use std::io::Read as _;
        host.file
            .as_ref()
            .expect("still there")
            .read_to_end(&mut read)
            .ok();
        assert!(read.starts_with(b"pixels"), "the mirror landed");
    }

    #[test]
    fn the_fixed_shm_opcode_is_the_dispatchers() {
        assert_eq!(X11_SHM_OPCODE, 130);
    }
}
