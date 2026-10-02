//! The transfer model: streaming policy and the pump.
//!
//! Doctrine (architecture §15): *the server never buffers payloads
//! larger than a page — clients stream.* A `receive` hands the
//! source's pipe write end straight through; the server's only job
//! is bookkeeping (admission, budget, liveness) and — for
//! integrator-driven copies — a chunk pump whose entire working set
//! is one page ([`PAGE`]).
//!
//! The pump moves bytes between an abstract source and sink
//! ([`ReadHalf`]/[`WriteHalf`]) one bounded step at a time; each
//! step is a pure state transition ([`PumpStep`]) so drivers can be
//! event-loop-friendly, deterministic, and deadlock-free by
//! construction: every pump call terminates after at most one
//! read + one write attempt, and the fuzz suite drives millions of
//! steps across random MIME/size/pattern corpora asserting every
//! sequence terminates in a terminal state.
//!
//! The registry ([`TransferRegistry`]) enforces the FD budget from
//! [`Limits::client_fds`] at admission — *before* any FD is adopted —
//! and releases capacity exactly once per terminal state.

#![forbid(unsafe_code)]

use crate::dnd::ClientKey;
use crate::offer::OfferKey;
use crate::source::SourceKey;
use ldp_core::limits::Limits;
use std::io;

/// The pump's working set: one page. No transfer ever allocates more.
pub const PAGE: usize = 4096;

/// Opaque transfer identity.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct TransferId(u64);

impl TransferId {
    /// Construct from an integrator-chosen value.
    #[must_use]
    pub const fn new(id: u64) -> TransferId {
        TransferId(id)
    }

    /// The raw id.
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

/// Where a transfer is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TransferState {
    /// Admitted; bytes may flow.
    Streaming,
    /// Source reached EOF; the sink was flushed and the transfer
    /// completed (`n` bytes total).
    Done(u64),
    /// The transfer failed (`io` error kind; `n` bytes had moved).
    Failed(io::ErrorKind, u64),
    /// Cancelled by policy (source died, offer killed, client gone).
    Cancelled(u64),
}

/// Why admission was rejected.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AdmissionError {
    /// The client is already at its FD ceiling
    /// (`Limits::client_fds`).
    FdBudget,
    /// The transfer id already exists.
    Duplicate,
}

impl std::fmt::Display for AdmissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            AdmissionError::FdBudget => "client FD budget exhausted",
            AdmissionError::Duplicate => "transfer id already registered",
        };
        f.write_str(s)
    }
}

impl std::error::Error for AdmissionError {}

/// One tracked transfer.
#[derive(Clone, Debug)]
pub struct Transfer {
    id: TransferId,
    client: ClientKey,
    /// The offer the receive arrived on.
    pub offer: OfferKey,
    /// The source serving it.
    pub source: SourceKey,
    /// The canonical requested MIME.
    pub mime: String,
    state: TransferState,
    bytes: u64,
}

impl Transfer {
    /// The transfer's id.
    #[must_use]
    pub const fn id(&self) -> TransferId {
        self.id
    }

    /// The receiving client.
    #[must_use]
    pub const fn client(&self) -> ClientKey {
        self.client
    }

    /// The current state.
    #[must_use]
    pub const fn state(&self) -> TransferState {
        self.state
    }

    /// Bytes moved so far.
    #[must_use]
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Whether the transfer is over.
    #[must_use]
    pub const fn terminal(&self) -> bool {
        !matches!(self.state, TransferState::Streaming)
    }
}

/// The FD-budget ledger for streaming transfers.
#[derive(Clone, Debug)]
pub struct TransferRegistry {
    transfers: std::collections::BTreeMap<TransferId, Transfer>,
    per_client: std::collections::BTreeMap<ClientKey, u32>,
    limits: Limits,
}

impl TransferRegistry {
    /// An empty registry under `limits`.
    #[must_use]
    pub const fn new(limits: Limits) -> TransferRegistry {
        TransferRegistry {
            transfers: std::collections::BTreeMap::new(),
            per_client: std::collections::BTreeMap::new(),
            limits,
        }
    }

    /// The limits in force.
    #[must_use]
    pub const fn limits(&self) -> Limits {
        self.limits
    }

    /// Live + terminal transfers (terminal entries await reaping).
    #[must_use]
    pub fn len(&self) -> usize {
        self.transfers.len()
    }

    /// Whether the registry tracks nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.transfers.is_empty()
    }

    /// One transfer by id.
    #[must_use]
    pub fn get(&self, id: TransferId) -> Option<&Transfer> {
        self.transfers.get(&id)
    }

    /// All transfers, id order (deterministic).
    pub fn all(&self) -> impl Iterator<Item = &Transfer> {
        self.transfers.values()
    }

    /// Active transfer count for one client.
    #[must_use]
    pub fn client_transfers(&self, client: ClientKey) -> u32 {
        self.per_client.get(&client).copied().unwrap_or(0)
    }

    /// Admit a transfer: budget-checked *before* any FD is adopted.
    ///
    /// # Errors
    ///
    /// [`AdmissionError::FdBudget`] when the client is at its ceiling;
    /// [`AdmissionError::Duplicate`] on id reuse before reaping.
    pub fn admit(
        &mut self,
        id: TransferId,
        client: ClientKey,
        offer: OfferKey,
        source: SourceKey,
        mime: &str,
    ) -> Result<TransferId, AdmissionError> {
        if self.transfers.contains_key(&id) {
            return Err(AdmissionError::Duplicate);
        }
        let held = self.client_transfers(client);
        if held >= self.limits.client_fds {
            return Err(AdmissionError::FdBudget);
        }
        self.per_client.insert(client, held + 1);
        self.transfers.insert(
            id,
            Transfer {
                id,
                client,
                offer,
                source,
                mime: mime.to_owned(),
                state: TransferState::Streaming,
                bytes: 0,
            },
        );
        Ok(id)
    }

    /// Record a terminal state (idempotent; first write wins). The
    /// FD slot is released exactly once.
    pub fn settle(&mut self, id: TransferId, state: TransferState) {
        let client = self.transfers.get(&id).map(|t| t.client);
        if let (Some(client), Some(t)) = (client, self.transfers.get_mut(&id)) {
            if matches!(t.state, TransferState::Streaming) {
                // Terminal states carry the authoritative byte count.
                let carried = match state {
                    TransferState::Done(n)
                    | TransferState::Failed(_, n)
                    | TransferState::Cancelled(n) => Some(n),
                    TransferState::Streaming => None,
                };
                t.state = state;
                if let Some(n) = carried {
                    t.bytes = n;
                }
                let held = self.per_client.get(&client).copied().unwrap_or(0);
                self.per_client.insert(client, held.saturating_sub(1));
            }
        }
    }

    /// Record progress: `n` more bytes flowed.
    pub fn advance(&mut self, id: TransferId, n: u64) {
        if let Some(t) = self.transfers.get_mut(&id) {
            t.bytes = t.bytes.saturating_add(n);
        }
    }

    /// Reap a terminal transfer (drops the bookkeeping row).
    pub fn reap(&mut self, id: TransferId) -> Option<Transfer> {
        let t = self.transfers.get(&id)?;
        if t.terminal() {
            self.transfers.remove(&id)
        } else {
            None
        }
    }

    /// Cancel every transfer of `client` (client gone). Returns the
    /// ids that were cancelled (deterministic id order).
    pub fn cancel_client(&mut self, client: ClientKey) -> Vec<TransferId> {
        self.cancel_where(|t| t.client == client)
    }

    /// Cancel every transfer served by `source` (source died).
    pub fn cancel_source(&mut self, source: SourceKey) -> Vec<TransferId> {
        self.cancel_where(|t| t.source == source)
    }

    /// Cancel every transfer of `offer` (offer killed).
    pub fn cancel_offer(&mut self, offer: OfferKey) -> Vec<TransferId> {
        self.cancel_where(|t| t.offer == offer)
    }

    fn cancel_where(&mut self, pred: impl Fn(&Transfer) -> bool) -> Vec<TransferId> {
        let ids: Vec<TransferId> = self
            .transfers
            .values()
            .filter(|t| pred(t) && !t.terminal())
            .map(|t| t.id)
            .collect();
        for id in &ids {
            let client = self.transfers.get(id).map(|t| t.client);
            if let (Some(client), Some(t)) = (client, self.transfers.get_mut(id)) {
                let moved = t.bytes;
                t.state = TransferState::Cancelled(moved);
                let held = self.per_client.get(&client).copied().unwrap_or(0);
                self.per_client.insert(client, held.saturating_sub(1));
            }
        }
        ids
    }
}

/// The byte source the pump reads from.
pub trait ReadHalf {
    /// Read one chunk into `buf`; `Ok(0)` = EOF.
    ///
    /// # Errors
    ///
    /// Propagated as [`PumpStep::Failed`].
    fn read_chunk(&mut self, buf: &mut [u8]) -> io::Result<usize>;
}

/// The byte sink the pump writes to.
pub trait WriteHalf {
    /// Write as much of `buf` as the sink takes now (partial writes
    /// are legal; the pump retries).
    ///
    /// # Errors
    ///
    /// Propagated as [`PumpStep::Failed`].
    fn write_chunk(&mut self, buf: &[u8]) -> io::Result<usize>;
}

/// The abstract over `io::Read`/`io::Write` (in-memory fuzz sources
/// and sinks use these blanket impls).
impl<T: io::Read> ReadHalf for T {
    fn read_chunk(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        io::Read::read(self, buf)
    }
}

impl<T: io::Write> WriteHalf for T {
    fn write_chunk(&mut self, buf: &[u8]) -> io::Result<usize> {
        io::Write::write(self, buf)
    }
}

/// One pump step's outcome.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PumpStep {
    /// `n` bytes moved source → sink **and were accepted by the
    /// sink** (a partially-written chunk stays pending in the pump
    /// until a later step drains it).
    Progress(u64),
    /// Source EOF reached *and* the pending chunk fully drained.
    Eof,
    /// The source would block now (nonblocking I/O; retry later).
    WouldBlock,
    /// The sink would block now; the pending chunk is retained.
    SinkFull,
    /// The transfer failed.
    Failed(io::ErrorKind),
}

/// The page-bounded copy engine: one [`PAGE`]-sized buffer plus the
/// write cursor of the chunk currently draining.
///
/// The pump is *stateful on purpose*: a sink that accepts 2 KB of a
/// 4 KB chunk and then signals `EAGAIN` must not lose the remaining
/// 2 KB — the pending tail stays buffered (still inside the one
/// page) and the next step resumes exactly there. A stateless
/// read→write step cannot make that guarantee, and the transfer fuzz
/// proved it the hard way (byte-count mismatches under stall-heavy
/// sinks).
///
/// The working-set doctrine is strengthened accordingly: one page per
/// *pump*, not per step — a driver juggling N concurrent transfers
/// holds N pages and nothing else.
#[derive(Debug)]
pub struct Pump {
    buf: Box<[u8]>,
    /// Bytes `[written..filled)` of `buf` are awaiting the sink.
    written: usize,
    filled: usize,
    eof: bool,
    written_total: u64,
}

impl Default for Pump {
    fn default() -> Self {
        Pump::new()
    }
}

impl Pump {
    /// A pump with a fresh page.
    #[must_use]
    pub fn new() -> Pump {
        Pump {
            buf: vec![0u8; PAGE].into_boxed_slice(),
            written: 0,
            filled: 0,
            eof: false,
            written_total: 0,
        }
    }

    /// Bytes moved through this pump so far.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.written_total
    }

    /// Whether the pump holds an undrained chunk.
    #[must_use]
    pub const fn pending(&self) -> usize {
        self.filled - self.written
    }

    /// One bounded step: drain pending first, then (only when the
    /// page is empty) read one chunk and start draining it. The step
    /// never loops on a stalling sink — it returns [`PumpStep::SinkFull`]
    /// with the tail retained.
    pub fn step(&mut self, src: &mut dyn ReadHalf, dst: &mut dyn WriteHalf) -> PumpStep {
        // Drain phase (also runs right after the read below).
        if self.pending() > 0 {
            let from = self.written;
            let to = self.filled;
            match dst.write_chunk(&self.buf[from..to]) {
                Ok(w) => {
                    self.written += w;
                    self.written_total += w as u64;
                    if self.pending() > 0 {
                        return PumpStep::SinkFull;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    return PumpStep::SinkFull;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                    return PumpStep::SinkFull;
                }
                Err(e) => return PumpStep::Failed(e.kind()),
            }
        }
        if self.eof {
            return PumpStep::Eof;
        }
        // Fill phase: the page is empty, read one chunk.
        match src.read_chunk(&mut self.buf) {
            Ok(0) => {
                self.eof = true;
                PumpStep::Eof
            }
            Ok(n) => {
                self.filled = n;
                self.written = 0;
                // Give the sink a first shot within this step.
                match dst.write_chunk(&self.buf[..n]) {
                    Ok(w) => {
                        self.written = w;
                        self.written_total += w as u64;
                        if self.pending() == 0 {
                            PumpStep::Progress(n as u64)
                        } else {
                            PumpStep::SinkFull
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => PumpStep::SinkFull,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => PumpStep::SinkFull,
                    Err(e) => PumpStep::Failed(e.kind()),
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => PumpStep::WouldBlock,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => PumpStep::WouldBlock,
            Err(e) => PumpStep::Failed(e.kind()),
        }
    }
}

/// Drive a transfer to completion over a pump (blocking halves or a
/// driver that retries stalls — the fuzz suites inject adversarial
/// would-blocks and keep calling). Returns total bytes moved.
///
/// # Errors
///
/// The first terminal failure propagates.
pub fn pump_to_completion(
    src: &mut dyn ReadHalf,
    dst: &mut dyn WriteHalf,
    pump: &mut Pump,
) -> io::Result<u64> {
    loop {
        match pump.step(src, dst) {
            PumpStep::Progress(_) => {}
            PumpStep::Eof => return Ok(pump.total()),
            PumpStep::WouldBlock | PumpStep::SinkFull => {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "blocking pump stalled",
                ));
            }
            PumpStep::Failed(kind) => return Err(io::Error::from(kind)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLIENT: ClientKey = ClientKey(1);

    fn admit(r: &mut TransferRegistry, id: u64) {
        r.admit(
            TransferId::new(id),
            CLIENT,
            OfferKey::new(1),
            SourceKey::new(1),
            "text/plain",
        )
        .unwrap();
    }

    #[test]
    fn admission_enforces_fd_budget() {
        let limits = Limits {
            client_fds: 3,
            ..Limits::default()
        };
        let mut r = TransferRegistry::new(limits);
        admit(&mut r, 1);
        admit(&mut r, 2);
        admit(&mut r, 3);
        assert_eq!(
            r.admit(
                TransferId::new(4),
                CLIENT,
                OfferKey::new(1),
                SourceKey::new(1),
                "text/plain"
            )
            .unwrap_err(),
            AdmissionError::FdBudget
        );
        // A different client still fits.
        assert!(r
            .admit(
                TransferId::new(5),
                ClientKey(2),
                OfferKey::new(1),
                SourceKey::new(1),
                "text/plain"
            )
            .is_ok());
    }

    #[test]
    fn settle_releases_budget_exactly_once() {
        let limits = Limits {
            client_fds: 2,
            ..Limits::default()
        };
        let mut r = TransferRegistry::new(limits);
        admit(&mut r, 1);
        admit(&mut r, 2);
        r.settle(TransferId::new(1), TransferState::Done(10));
        // Double-settle must not double-release.
        r.settle(TransferId::new(1), TransferState::Done(10));
        assert_eq!(r.client_transfers(CLIENT), 1);
        admit(&mut r, 3);
        // Reaping a terminal row drops it entirely.
        let reaped = r.reap(TransferId::new(1)).unwrap();
        assert_eq!(reaped.bytes(), 10);
        assert!(r.reap(TransferId::new(1)).is_none());
        // Reaping a streaming row is refused.
        assert!(r.reap(TransferId::new(2)).is_none());
    }

    #[test]
    fn cancellation_by_source_offer_client() {
        let mut r = TransferRegistry::new(Limits::default());
        admit(&mut r, 1);
        admit(&mut r, 2);
        r.admit(
            TransferId::new(3),
            ClientKey(2),
            OfferKey::new(9),
            SourceKey::new(1),
            "text/plain",
        )
        .unwrap();
        r.advance(TransferId::new(1), 7);
        let cancelled = r.cancel_source(SourceKey::new(1));
        assert_eq!(cancelled.len(), 3);
        assert_eq!(
            r.get(TransferId::new(1)).unwrap().state(),
            TransferState::Cancelled(7)
        );
        assert_eq!(r.client_transfers(CLIENT), 0);
        assert_eq!(r.client_transfers(ClientKey(2)), 0);
    }

    #[test]
    fn pump_moves_bytes_and_eofs_once() {
        let data: Vec<u8> = (0u8..=255).cycle().take(10_000).collect();
        let mut src: &[u8] = &data;
        let mut dst = Vec::new();
        let mut pump = Pump::new();
        let n = pump_to_completion(&mut src, &mut dst, &mut pump).unwrap();
        assert_eq!(n, 10_000);
        assert_eq!(pump.total(), 10_000);
        assert_eq!(dst, data);
        assert_eq!(pump.pending(), 0);
    }

    /// A sink that accepts one byte per call then stalls must not
    /// lose a single byte — the pending tail survives in the page.
    #[test]
    fn stalling_sink_loses_nothing() {
        struct Stingy {
            out: Vec<u8>,
            calls: u64,
        }
        impl WriteHalf for Stingy {
            fn write_chunk(&mut self, buf: &[u8]) -> io::Result<usize> {
                if buf.is_empty() {
                    return Ok(0);
                }
                // Every third *call* stalls before accepting — the
                // call counter advances even on stalls (an Err means
                // nothing was accepted; the contract the pump relies
                // on), so the sink always eventually un-stalls, the
                // readiness-loop shape.
                self.calls += 1;
                if self.calls % 3 == 0 {
                    return Err(io::Error::from(io::ErrorKind::WouldBlock));
                }
                self.out.push(buf[0]);
                Ok(1)
            }
        }
        let data: Vec<u8> = (0u8..=255).cycle().take(1_000).collect();
        let mut src: &[u8] = &data;
        let mut dst = Stingy {
            out: Vec::new(),
            calls: 0,
        };
        let mut pump = Pump::new();
        // The driver retries stalls (the event-loop shape).
        let mut stalls = 0;
        let total = loop {
            match pump.step(&mut src, &mut dst) {
                PumpStep::Progress(_) => {}
                PumpStep::Eof => break pump.total(),
                PumpStep::WouldBlock | PumpStep::SinkFull => stalls += 1,
                PumpStep::Failed(k) => panic!("unexpected failure: {k:?}"),
            }
        };
        assert_eq!(total, 1_000);
        assert_eq!(dst.out, data);
        assert!(stalls > 100, "stall path exercised: {stalls}");
    }

    #[test]
    fn pump_reports_failures() {
        struct Broken;
        impl ReadHalf for Broken {
            fn read_chunk(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
                Err(io::Error::from(io::ErrorKind::BrokenPipe))
            }
        }
        let mut dst = Vec::new();
        let mut pump = Pump::new();
        assert_eq!(
            pump.step(&mut Broken, &mut dst),
            PumpStep::Failed(io::ErrorKind::BrokenPipe)
        );
    }

    #[test]
    fn page_is_one_page() {
        assert_eq!(PAGE, 4096);
    }
}
