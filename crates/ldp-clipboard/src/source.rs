//! The `data_source` machine: a MIME-typed offer the client serves on
//! demand.
//!
//! Lifecycle (spec `ldp.data.data_source`):
//!
//! ```text
//! constructed ──offer*──▶ offering ──attach──▶ attached ──cancel──▶ cancelled
//!    (idle)                          (selection │ drag payload)     (dead)
//!                                      │
//!                                      └──dnd path──▶ dropped ──▶ finished
//! ```
//!
//! * `offer` accumulates the MIME list; it is only accepted while the
//!   source is unattached (the spec: "must precede selection").
//! * Attaching happens when the source becomes the clipboard/primary
//!   selection or the payload of a started drag.
//! * While attached, `send` transfers are served: the server hands the
//!   source client a pipe write end per request. The machine counts
//!   them (the transfer registry owns the FD budget).
//! * `cancelled` fires when the source loses the selection, the drag
//!   aborts, or the integrator clears the slot. After `cancelled` the
//!   client may destroy the object; the machine rejects everything
//!   but bookkeeping queries.
//!
//! The machine is addressed by [`SourceKey`]; the manager owns the
//! table. Pure policy: no I/O, no clock.

#![forbid(unsafe_code)]

use crate::mime::Mime;
use ldp_core::limits::Limits;

/// Opaque data_source identity (the integrator maps these to protocol
/// object IDs; nothing in this crate dereferences them).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct SourceKey(u64);

impl SourceKey {
    /// Construct from an integrator-chosen value.
    #[must_use]
    pub const fn new(id: u64) -> SourceKey {
        SourceKey(id)
    }

    /// The raw key.
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

/// Where a source is currently attached.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Attachment {
    /// The seat's clipboard selection.
    Selection,
    /// The seat's primary selection.
    Primary,
    /// The payload of an in-flight drag.
    Drag,
}

/// Why a `data_source` request was rejected.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SourceError {
    /// `offer` after the source attached (or after cancel).
    OfferTooLate,
    /// The source is dead (cancelled/finished).
    Dead,
    /// The source is already attached to a slot or drag.
    AlreadyAttached,
    /// The offer list would exceed the per-source cap.
    TooManyOffers,
    /// The MIME string is invalid (the parser error is logged by the
    /// dispatcher; the machine sees only the failure).
    BadMime,
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            SourceError::OfferTooLate => "offer after the source attached",
            SourceError::Dead => "the data source is dead",
            SourceError::AlreadyAttached => "the source is already attached",
            SourceError::TooManyOffers => "too many offered MIME types",
            SourceError::BadMime => "invalid MIME type",
        };
        f.write_str(s)
    }
}

impl std::error::Error for SourceError {}

/// Maximum distinct MIME types one source may offer (policy constant:
/// generous against real offer lists, tight against floods).
pub const MAX_OFFERS: usize = 64;

/// One `data_source`.
#[derive(Clone, Debug)]
pub struct DataSource {
    key: SourceKey,
    offers: Vec<Mime>,
    attachment: Option<Attachment>,
    dead: bool,
    served: u32,
    target: Option<String>,
    actions: Option<crate::dnd::ActionSet>,
}

impl DataSource {
    /// A freshly created source (the `create_data_source` request).
    #[must_use]
    pub fn new(key: SourceKey) -> DataSource {
        DataSource {
            key,
            offers: Vec::new(),
            attachment: None,
            dead: false,
            served: 0,
            target: None,
            actions: None,
        }
    }

    /// The source's key.
    #[must_use]
    pub const fn key(&self) -> SourceKey {
        self.key
    }

    /// The offered MIME list, in offer order.
    #[must_use]
    pub fn offers(&self) -> &[Mime] {
        &self.offers
    }

    /// The current attachment (`None`: unattached).
    #[must_use]
    pub const fn attachment(&self) -> Option<Attachment> {
        self.attachment
    }

    /// Whether the source was cancelled or finished.
    #[must_use]
    pub const fn dead(&self) -> bool {
        self.dead
    }

    /// How many `send` transfers were served.
    #[must_use]
    pub const fn served(&self) -> u32 {
        self.served
    }

    /// The accepted target (`data_source.target`), if any interaction
    /// settled on one.
    #[must_use]
    pub fn target(&self) -> Option<&str> {
        self.target.as_deref()
    }

    /// `offer` — add one MIME type. Duplicates are no-ops (the list is
    /// a set; re-offering the same type is harmless client behavior,
    /// not a protocol error).
    ///
    /// # Errors
    ///
    /// [`SourceError::OfferTooLate`] once attached or dead;
    /// [`SourceError::TooManyOffers`] past the cap;
    /// [`SourceError::BadMime`] on a malformed string.
    pub fn offer(&mut self, raw: &str, limits: &Limits) -> Result<(), SourceError> {
        if self.dead {
            return Err(SourceError::Dead);
        }
        if self.attachment.is_some() {
            return Err(SourceError::OfferTooLate);
        }
        let mime = Mime::parse(raw, limits).map_err(|_| SourceError::BadMime)?;
        if !self.offers.contains(&mime) {
            if self.offers.len() >= MAX_OFFERS {
                return Err(SourceError::TooManyOffers);
            }
            self.offers.push(mime);
        }
        Ok(())
    }

    /// Attach as the selection/primary/drag payload. Unattached only.
    ///
    /// # Errors
    ///
    /// [`SourceError::Dead`] on a dead source. Attaching an
    /// *unoffered* source is legal (an empty offer list means "the
    /// source never serves anything" — receivers see an empty offer
    /// list and simply never call `receive`).
    pub fn attach(&mut self, to: Attachment) -> Result<(), SourceError> {
        if self.dead {
            return Err(SourceError::Dead);
        }
        if self.attachment.is_some() {
            return Err(SourceError::AlreadyAttached);
        }
        self.attachment = Some(to);
        Ok(())
    }

    /// Record one served `send` transfer (the transfer registry calls
    /// this when it admits a transfer for this source).
    ///
    /// # Errors
    ///
    /// [`SourceError::Dead`] on a dead source.
    pub fn record_send(&mut self) -> Result<(), SourceError> {
        if self.dead {
            return Err(SourceError::Dead);
        }
        self.served = self.served.saturating_add(1);
        Ok(())
    }

    /// Record the accepted target MIME (the `target` event payload).
    /// An empty string clears it (the receiver declined).
    ///
    /// # Errors
    ///
    /// [`SourceError::Dead`] on a dead source.
    pub fn set_target(&mut self, mime: Option<String>) -> Result<(), SourceError> {
        if self.dead {
            return Err(SourceError::Dead);
        }
        self.target = mime;
        Ok(())
    }

    /// Record the currently available drag action set (the `actions`
    /// event payload).
    ///
    /// # Errors
    ///
    /// [`SourceError::Dead`] on a dead source.
    pub fn set_actions(&mut self, actions: crate::dnd::ActionSet) -> Result<(), SourceError> {
        if self.dead {
            return Err(SourceError::Dead);
        }
        self.actions = Some(actions);
        Ok(())
    }

    /// The last announced action set (drag sources only).
    #[must_use]
    pub const fn actions(&self) -> Option<crate::dnd::ActionSet> {
        self.actions
    }

    /// Cancel: the source lost the selection / the drag aborted. The
    /// source is dead afterwards (the client may destroy it).
    ///
    /// No-op (idempotent) when already dead — the manager can call it
    /// on every eviction path without ordering worries.
    pub fn cancel(&mut self) {
        if !self.dead {
            self.dead = true;
            self.attachment = None;
        }
    }

    /// Finish: the drag completed (`dnd_finished`). Dead afterwards.
    pub fn finish(&mut self) {
        if !self.dead {
            self.dead = true;
            self.attachment = None;
        }
    }

    /// Detach without dying (the manager moves a source from drag to
    /// finished contexts internally; also the destroy-without-clear
    /// path). Returns the previous attachment.
    pub fn detach(&mut self) -> Option<Attachment> {
        self.attachment.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offer_accumulates_and_dedups() {
        let mut s = DataSource::new(SourceKey::new(1));
        assert!(s
            .offer("text/plain;charset=utf-8", &Limits::default())
            .is_ok());
        assert!(s.offer("image/png", &Limits::default()).is_ok());
        assert!(s.offer("image/png", &Limits::default()).is_ok());
        assert_eq!(s.offers().len(), 2);
        assert_eq!(s.offers()[0].to_string(), "text/plain;charset=utf-8");
        assert_eq!(s.offers()[1].to_string(), "image/png");
    }

    #[test]
    fn offer_rejects_malformed_and_flood() {
        let mut s = DataSource::new(SourceKey::new(2));
        assert_eq!(
            s.offer("not a mime", &Limits::default()).unwrap_err(),
            SourceError::BadMime
        );
        for i in 0..MAX_OFFERS {
            assert!(s
                .offer(&format!("application/x-n{i}"), &Limits::default())
                .is_ok());
        }
        assert_eq!(
            s.offer("application/x-one-too-many", &Limits::default())
                .unwrap_err(),
            SourceError::TooManyOffers
        );
    }

    #[test]
    fn offer_after_attach_is_too_late() {
        let mut s = DataSource::new(SourceKey::new(3));
        s.attach(Attachment::Selection).unwrap();
        assert_eq!(
            s.offer("text/plain", &Limits::default()).unwrap_err(),
            SourceError::OfferTooLate
        );
    }

    #[test]
    fn cancel_is_terminal_and_idempotent() {
        let mut s = DataSource::new(SourceKey::new(4));
        s.attach(Attachment::Drag).unwrap();
        s.cancel();
        s.cancel();
        assert!(s.dead());
        assert_eq!(s.attachment(), None);
        assert_eq!(
            s.attach(Attachment::Selection).unwrap_err(),
            SourceError::Dead
        );
        assert_eq!(
            s.offer("text/plain", &Limits::default()).unwrap_err(),
            SourceError::Dead
        );
        assert_eq!(s.record_send().unwrap_err(), SourceError::Dead);
    }

    #[test]
    fn send_target_actions_accounting() {
        let mut s = DataSource::new(SourceKey::new(5));
        s.record_send().unwrap();
        s.record_send().unwrap();
        assert_eq!(s.served(), 2);
        s.set_target(Some("image/png".into())).unwrap();
        assert_eq!(s.target(), Some("image/png"));
        s.set_target(None).unwrap();
        assert_eq!(s.target(), None);
        let set = crate::dnd::ActionSet::ALL;
        s.set_actions(set).unwrap();
        assert_eq!(s.actions(), Some(set));
    }

    #[test]
    fn finish_is_terminal() {
        let mut s = DataSource::new(SourceKey::new(6));
        s.attach(Attachment::Drag).unwrap();
        s.finish();
        assert!(s.dead());
        assert_eq!(s.detach(), None);
    }
}
