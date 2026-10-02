//! The focus stack: most-recently-used ordering of mapped root surfaces
//! plus the keyboard focus pointer.
//!
//! Focus is shell policy data, kept deliberately separate from stacking
//! (the shell layer of Phase 7 decides *when* raising follows focus).
//! Invariants the structure maintains by construction:
//!
//! * no duplicates in the MRU list,
//! * the keyboard focus is always a member (or `None`),
//! * unmapping/destroying a surface removes it from both,
//! * activation order is stable and inspectable (the compositor's
//!   Alt-Tab / focus-follows-mouse policies read it directly).

use crate::surface::SurfaceId;

/// MRU-ordered focus tracking for a set of root surfaces.
#[derive(Clone, Debug, Default)]
pub struct FocusStack {
    /// Most-recently-used first.
    mru: Vec<SurfaceId>,
    /// The keyboard focus (`None` = unfocused).
    keyboard: Option<SurfaceId>,
}

impl FocusStack {
    /// An empty stack.
    #[must_use]
    pub fn new() -> FocusStack {
        FocusStack::default()
    }

    /// The MRU order (front = most recently used).
    #[must_use]
    pub fn mru(&self) -> &[SurfaceId] {
        &self.mru
    }

    /// The current keyboard focus.
    #[must_use]
    pub fn keyboard(&self) -> Option<SurfaceId> {
        self.keyboard
    }

    /// Number of tracked surfaces.
    #[must_use]
    pub fn len(&self) -> usize {
        self.mru.len()
    }

    /// Whether nothing is tracked.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.mru.is_empty()
    }

    /// Activate a surface: move it to the MRU front and make it the
    /// keyboard focus. Unknown surfaces are ignored (the embedder maps
    /// ids; a focus request for an untracked id is a no-op, not an
    /// error — spurious activates from late input events must not
    /// corrupt the stack).
    pub fn activate(&mut self, id: SurfaceId) {
        if !self.mru.contains(&id) {
            return;
        }
        self.mru.retain(|s| *s != id);
        self.mru.insert(0, id);
        self.keyboard = Some(id);
    }

    /// Set the keyboard focus without touching the MRU order (focus
    /// follows a non-activating gesture). Unknown ids are ignored.
    pub fn focus_keyboard(&mut self, id: Option<SurfaceId>) {
        if let Some(id) = id {
            if !self.mru.contains(&id) {
                return;
            }
        }
        self.keyboard = id;
    }

    /// A surface became visible/mapped: track it. Returns whether it
    /// was newly added (a duplicate map event is a no-op).
    pub fn mapped(&mut self, id: SurfaceId) -> bool {
        if self.mru.contains(&id) {
            return false;
        }
        self.mru.push(id);
        true
    }

    /// A surface unmapped or destroyed: drop it from the MRU list and
    /// clear the keyboard focus if it held it. Returns whether anything
    /// changed.
    pub fn removed(&mut self, id: SurfaceId) -> bool {
        let had_focus = self.keyboard == Some(id);
        let before = self.mru.len();
        self.mru.retain(|s| *s != id);
        let changed = self.mru.len() != before || had_focus;
        if had_focus {
            self.keyboard = self.mru.first().copied();
        }
        changed
    }

    /// The surface below `id` in the MRU order (the Alt-Tab cycle
    /// target when `id` is focused); `None` when `id` is untracked or
    /// alone.
    #[must_use]
    pub fn next_below(&self, id: SurfaceId) -> Option<SurfaceId> {
        if self.mru.len() < 2 {
            return None;
        }
        let i = self.mru.iter().position(|s| *s == id)?;
        Some(self.mru[(i + 1) % self.mru.len()])
    }

    /// Cycle the keyboard focus one step down the MRU order and return
    /// the new focus (the classic Alt-Tab step).
    pub fn cycle(&mut self) -> Option<SurfaceId> {
        let next = self.keyboard.and_then(|k| self.next_below(k));
        if let Some(n) = next {
            self.keyboard = Some(n);
        }
        self.keyboard
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u64) -> SurfaceId {
        SurfaceId::from_raw(n)
    }

    #[test]
    fn activation_orders_mru_and_focuses() {
        let mut f = FocusStack::new();
        for i in 1..=3 {
            f.mapped(id(i));
        }
        f.activate(id(2));
        assert_eq!(f.mru(), &[id(2), id(1), id(3)]);
        assert_eq!(f.keyboard(), Some(id(2)));
        f.activate(id(1));
        assert_eq!(f.mru(), &[id(1), id(2), id(3)]);
        assert_eq!(f.keyboard(), Some(id(1)));
    }

    #[test]
    fn unknown_activation_is_ignored() {
        let mut f = FocusStack::new();
        f.mapped(id(1));
        f.activate(id(9));
        assert_eq!(f.mru(), &[id(1)]);
        assert_eq!(f.keyboard(), None);
        f.focus_keyboard(Some(id(9)));
        assert_eq!(f.keyboard(), None);
    }

    #[test]
    fn removal_reassigns_focus_to_the_next_mru() {
        let mut f = FocusStack::new();
        for i in 1..=3 {
            f.mapped(id(i));
        }
        f.activate(id(3));
        f.removed(id(3));
        assert_eq!(f.keyboard(), Some(id(1)));
        assert_eq!(f.mru(), &[id(1), id(2)]);
    }

    #[test]
    fn removing_the_last_surface_clears_focus() {
        let mut f = FocusStack::new();
        f.mapped(id(1));
        f.activate(id(1));
        assert!(f.removed(id(1)));
        assert!(f.is_empty());
        assert_eq!(f.keyboard(), None);
    }

    #[test]
    fn duplicate_maps_are_noops() {
        let mut f = FocusStack::new();
        assert!(f.mapped(id(1)));
        assert!(!f.mapped(id(1)));
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn cycling_wraps_around() {
        let mut f = FocusStack::new();
        for i in 1..=3 {
            f.mapped(id(i));
        }
        f.activate(id(1));
        assert_eq!(f.cycle(), Some(id(2)));
        assert_eq!(f.cycle(), Some(id(3)));
        assert_eq!(f.cycle(), Some(id(1)));
    }

    #[test]
    fn cycling_with_one_or_zero_surfaces() {
        let mut f = FocusStack::new();
        assert_eq!(f.cycle(), None);
        f.mapped(id(1));
        f.activate(id(1));
        assert_eq!(f.cycle(), Some(id(1)));
    }

    #[test]
    fn focus_without_activation_keeps_mru() {
        let mut f = FocusStack::new();
        for i in 1..=3 {
            f.mapped(id(i));
        }
        f.activate(id(1)); // MRU: [1, 2, 3], focus 1
        f.focus_keyboard(Some(id(3)));
        assert_eq!(f.keyboard(), Some(id(3)));
        assert_eq!(f.mru(), &[id(1), id(2), id(3)]);
        f.focus_keyboard(None);
        assert_eq!(f.keyboard(), None);
    }
}
