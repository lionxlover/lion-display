//! The `data_offer` machine: the receiver's view of a source.
//!
//! An offer is created by the *server* when a source becomes visible
//! to a client: selection/primary changes publish an offer per
//! interested device, and a drag entering a surface publishes the drag
//! offer. The offer mirrors the source's MIME list (a snapshot —
//! offer lists are frozen at publication because `data_source.offer`
//! is refused after attachment).
//!
//! Requests the receiver may make:
//!
//! * `accept(mime)` — announce what it will request (the source sees
//!   `target`); empty string clears.
//! * `receive(mime, fd)` — start a transfer. One per MIME type per
//!   offer; the MIME must be offered (satisfies-match allowed — the
//!   charset folding of [`crate::mime`]).
//! * `set_actions` — DnD offers only (the negotiation narrowing).
//! * `finish` — DnD offers only, after the drop.
//!
//! Permission gating: `receive` consults the [`crate::permission`]
//! decision recorded at admission time; a denied receive produces no
//! transfer (the dispatcher answers with the protocol error).

#![forbid(unsafe_code)]

use crate::dnd::{ActionSet, ClientKey, DndAction};
use crate::mime::Mime;
use crate::source::SourceKey;
use ldp_core::limits::Limits;

/// Opaque data_offer identity.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct OfferKey(u64);

impl OfferKey {
    /// Construct from an integrator-chosen value.
    #[must_use]
    pub const fn new(id: u64) -> OfferKey {
        OfferKey(id)
    }

    /// The raw key.
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

/// What the offer is for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OfferKind {
    /// A clipboard/primary selection publication.
    Selection,
    /// The payload of an in-flight drag.
    Drag,
}

/// Why a `data_offer` request was rejected.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OfferError {
    /// The MIME string is malformed.
    BadMime,
    /// The MIME is not in the offer list.
    NotOffered,
    /// This MIME was already received on this offer.
    AlreadyReceived,
    /// `set_actions`/`finish` on a selection offer (DnD-only).
    NotDrag,
    /// `finish` before the drag was dropped.
    NotDropped,
    /// The offer was destroyed.
    Dead,
    /// The transfer registry rejected admission (FD budget).
    TransferRejected,
}

impl std::fmt::Display for OfferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            OfferError::BadMime => "invalid MIME type",
            OfferError::NotOffered => "MIME type not offered",
            OfferError::AlreadyReceived => "MIME type already received",
            OfferError::NotDrag => "request valid only on drag offers",
            OfferError::NotDropped => "the drag has not been dropped",
            OfferError::Dead => "the data offer is dead",
            OfferError::TransferRejected => "transfer budget exceeded",
        };
        f.write_str(s)
    }
}

impl std::error::Error for OfferError {}

/// One `data_offer`.
#[derive(Clone, Debug)]
pub struct DataOffer {
    key: OfferKey,
    /// The client the offer was published to (the only one allowed to
    /// act on it).
    client: ClientKey,
    /// The source this offer mirrors.
    source: SourceKey,
    kind: OfferKind,
    offers: Vec<Mime>,
    accepted: Option<String>,
    received: Vec<Mime>,
    dead: bool,
}

impl DataOffer {
    /// A published offer (the server creates it; `offers` is the
    /// frozen source list).
    #[must_use]
    pub fn new(
        key: OfferKey,
        client: ClientKey,
        source: SourceKey,
        kind: OfferKind,
        offers: Vec<Mime>,
    ) -> DataOffer {
        DataOffer {
            key,
            client,
            source,
            kind,
            offers,
            accepted: None,
            received: Vec::new(),
            dead: false,
        }
    }

    /// The offer's key.
    #[must_use]
    pub const fn key(&self) -> OfferKey {
        self.key
    }

    /// The owning client.
    #[must_use]
    pub const fn client(&self) -> ClientKey {
        self.client
    }

    /// The mirrored source.
    #[must_use]
    pub const fn source(&self) -> SourceKey {
        self.source
    }

    /// The kind (selection or drag).
    #[must_use]
    pub const fn kind(&self) -> OfferKind {
        self.kind
    }

    /// The frozen offer list.
    #[must_use]
    pub fn offers(&self) -> &[Mime] {
        &self.offers
    }

    /// The accepted target (empty-cleared to `None`).
    #[must_use]
    pub fn accepted(&self) -> Option<&str> {
        self.accepted.as_deref()
    }

    /// The MIME types already received.
    #[must_use]
    pub fn received(&self) -> &[Mime] {
        &self.received
    }

    /// Whether the offer is dead (source gone, drag finished, object
    /// destroyed).
    #[must_use]
    pub const fn dead(&self) -> bool {
        self.dead
    }

    /// Kill the offer (no further requests accepted).
    pub fn kill(&mut self) {
        self.dead = true;
    }

    /// `accept(mime)` — record the target. An empty string clears.
    ///
    /// # Errors
    ///
    /// [`OfferError::Dead`]; [`OfferError::NotOffered`] when the MIME
    /// is neither offered (folding included) nor the empty clear.
    pub fn accept(&mut self, raw: &str, limits: &Limits) -> Result<(), OfferError> {
        if self.dead {
            return Err(OfferError::Dead);
        }
        if raw.is_empty() {
            self.accepted = None;
            return Ok(());
        }
        let mime = Mime::parse(raw, limits).map_err(|_| OfferError::BadMime)?;
        if !self.offers.iter().any(|o| o.satisfies(&mime)) {
            return Err(OfferError::NotOffered);
        }
        self.accepted = Some(raw.to_owned());
        Ok(())
    }

    /// `receive(mime)` — validate a transfer request and commit it in
    /// one step (the direct-request path; the manager uses
    /// [`DataOffer::check_receive`] + [`DataOffer::commit_receive`]
    /// so admission can interleave without leaving traces on
    /// rejection).
    ///
    /// # Errors
    ///
    /// [`OfferError::Dead`], [`OfferError::BadMime`],
    /// [`OfferError::NotOffered`], [`OfferError::AlreadyReceived`].
    pub fn receive(&mut self, raw: &str, limits: &Limits) -> Result<Mime, OfferError> {
        let mime = self.check_receive(raw, limits)?;
        self.commit_receive(&mime);
        Ok(mime)
    }

    /// The validation half of `receive` (no state change).
    ///
    /// # Errors
    ///
    /// [`OfferError::Dead`], [`OfferError::BadMime`],
    /// [`OfferError::NotOffered`], [`OfferError::AlreadyReceived`].
    pub fn check_receive(&self, raw: &str, limits: &Limits) -> Result<Mime, OfferError> {
        if self.dead {
            return Err(OfferError::Dead);
        }
        let mime = Mime::parse(raw, limits).map_err(|_| OfferError::BadMime)?;
        if !self.offers.iter().any(|o| o.satisfies(&mime)) {
            return Err(OfferError::NotOffered);
        }
        if self.received.contains(&mime) {
            return Err(OfferError::AlreadyReceived);
        }
        Ok(mime)
    }

    /// The commitment half of `receive`: mark the MIME received. The
    /// caller must have run [`DataOffer::check_receive`] with the
    /// same string immediately before (and nothing may interleave
    /// between the two on this offer).
    pub fn commit_receive(&mut self, mime: &Mime) {
        if !self.received.contains(mime) {
            self.received.push(mime.clone());
        }
    }

    /// `set_actions` (drag offers only). The manager owns the
    /// machine; this only guards the offer's own kind and liveness.
    ///
    /// # Errors
    ///
    /// [`OfferError::Dead`], [`OfferError::NotDrag`].
    pub fn set_actions(&mut self, _actions: ActionSet) -> Result<(), OfferError> {
        if self.dead {
            return Err(OfferError::Dead);
        }
        if self.kind != OfferKind::Drag {
            return Err(OfferError::NotDrag);
        }
        Ok(())
    }

    /// `finish` (drag offers only, after the drop).
    ///
    /// # Errors
    ///
    /// [`OfferError::Dead`], [`OfferError::NotDrag`],
    /// [`OfferError::NotDropped`].
    pub fn finish(&mut self, dropped: bool) -> Result<(), OfferError> {
        if self.dead {
            return Err(OfferError::Dead);
        }
        if self.kind != OfferKind::Drag {
            return Err(OfferError::NotDrag);
        }
        if !dropped {
            return Err(OfferError::NotDropped);
        }
        self.dead = true;
        Ok(())
    }

    /// The negotiated action the receiver must be told about
    /// (bookkeeping for the manager's `data_offer.action` event).
    #[must_use]
    pub const fn action_to_announce(picked: DndAction) -> DndAction {
        picked
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dnd::ClientKey as Ck;

    const CLIENT: Ck = Ck(1);
    const SRC: SourceKey = SourceKey::new(2);

    fn offer(kind: OfferKind) -> DataOffer {
        let mut offers = Vec::new();
        for m in ["text/plain;charset=utf-8", "image/png", "text/html"] {
            offers.push(Mime::parse(m, &Limits::default()).unwrap());
        }
        DataOffer::new(OfferKey::new(9), CLIENT, SRC, kind, offers)
    }

    #[test]
    fn accept_records_and_clears() {
        let mut o = offer(OfferKind::Selection);
        o.accept("image/png", &Limits::default()).unwrap();
        assert_eq!(o.accepted(), Some("image/png"));
        o.accept("", &Limits::default()).unwrap();
        assert_eq!(o.accepted(), None);
        // Folding accept is legal.
        o.accept("text/plain", &Limits::default()).unwrap();
        // Unoffered accept is an error.
        assert_eq!(
            o.accept("application/json", &Limits::default())
                .unwrap_err(),
            OfferError::NotOffered
        );
        assert_eq!(
            o.accept("bogus", &Limits::default()).unwrap_err(),
            OfferError::BadMime
        );
    }

    #[test]
    fn receive_once_per_mime_with_folding() {
        let mut o = offer(OfferKind::Selection);
        // Folded ask matches the offered charset variant.
        let m1 = o.receive("text/plain", &Limits::default()).unwrap();
        // The same folded ask again is a duplicate.
        assert_eq!(
            o.receive("text/plain", &Limits::default()).unwrap_err(),
            OfferError::AlreadyReceived
        );
        // A *different* charset is a distinct MIME: served separately.
        assert!(o
            .receive("text/plain;charset=utf-8", &Limits::default())
            .is_ok());
        assert!(o.receive("image/png", &Limits::default()).is_ok());
        assert_eq!(o.received().len(), 3);
        assert_eq!(m1.essence(), "text/plain");
        assert_eq!(
            o.receive("application/json", &Limits::default())
                .unwrap_err(),
            OfferError::NotOffered
        );
    }

    #[test]
    fn dnd_only_requests_gated() {
        let mut sel = offer(OfferKind::Selection);
        assert_eq!(
            sel.set_actions(ActionSet::ALL).unwrap_err(),
            OfferError::NotDrag
        );
        assert_eq!(sel.finish(true).unwrap_err(), OfferError::NotDrag);
        let mut dnd = offer(OfferKind::Drag);
        assert_eq!(dnd.finish(false).unwrap_err(), OfferError::NotDropped);
        dnd.finish(true).unwrap();
        assert!(dnd.dead());
        assert_eq!(
            dnd.accept("image/png", &Limits::default()).unwrap_err(),
            OfferError::Dead
        );
    }

    #[test]
    fn kill_is_terminal() {
        let mut o = offer(OfferKind::Selection);
        o.kill();
        assert_eq!(
            o.receive("image/png", &Limits::default()).unwrap_err(),
            OfferError::Dead
        );
    }
}
