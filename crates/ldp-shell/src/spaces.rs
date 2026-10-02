//! Spaces: macOS-style workspaces.
//!
//! One ordered list of spaces; every toplevel is assigned to one
//! (sticky toplevels are visible on all); each seat has an active
//! space. Visibility is a pure query: a window is visible to a seat
//! when it is sticky or assigned to that seat's active space. Count
//! changes reflow assignments (windows on removed spaces fall back to
//! the nearest surviving space, which the integrator reports through
//! `workspace_changed`).

#![forbid(unsafe_code)]

use std::collections::HashMap;

use crate::WindowKey;

/// Spaces misuse.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SpacesError {
    /// A seat with no assigned active space (the seat was never
    /// registered with [`Spaces::add_seat`]).
    UnknownSeat,
    /// A window not assigned to any space.
    UnknownWindow,
}

impl std::fmt::Display for SpacesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpacesError::UnknownSeat => f.write_str("seat has no active space"),
            SpacesError::UnknownWindow => f.write_str("window is not assigned to a space"),
        }
    }
}

impl std::error::Error for SpacesError {}

/// The spaces model.
#[derive(Clone, Debug)]
pub struct Spaces {
    count: u32,
    /// Window → space index.
    assignment: HashMap<WindowKey, u32>,
    /// Sticky windows (visible everywhere; assignment still records
    /// their "home" space for un-stickying).
    sticky: Vec<WindowKey>,
    /// Seat → active space index.
    active: HashMap<u32, u32>,
}

impl Spaces {
    /// A model with `count` spaces (at least 1).
    ///
    /// # Panics
    ///
    /// Never: a zero count is promoted to 1 (the shell always has one
    /// space).
    #[must_use]
    pub fn new(count: u32) -> Spaces {
        Spaces {
            count: count.max(1),
            assignment: HashMap::new(),
            sticky: Vec::new(),
            active: HashMap::new(),
        }
    }

    /// The space count.
    #[must_use]
    pub const fn count(&self) -> u32 {
        self.count
    }

    /// Whether `key` is sticky.
    #[must_use]
    pub fn is_sticky(&self, key: WindowKey) -> bool {
        self.sticky.contains(&key)
    }

    /// Register a seat with its initial active space (clamped).
    pub fn add_seat(&mut self, seat: u32, space: u32) {
        self.active
            .insert(seat, space.min(self.count.saturating_sub(1)));
    }

    /// A seat's active space.
    ///
    /// # Errors
    /// [`SpacesError::UnknownSeat`] when the seat was never added.
    pub fn active_of(&self, seat: u32) -> Result<u32, SpacesError> {
        self.active
            .get(&seat)
            .copied()
            .ok_or(SpacesError::UnknownSeat)
    }

    /// Switch a seat's active space (clamped).
    ///
    /// # Errors
    /// [`SpacesError::UnknownSeat`] for an unregistered seat.
    pub fn switch(&mut self, seat: u32, space: u32) -> Result<u32, SpacesError> {
        if !self.active.contains_key(&seat) {
            return Err(SpacesError::UnknownSeat);
        }
        let s = space.min(self.count.saturating_sub(1));
        self.active.insert(seat, s);
        Ok(s)
    }

    /// Assign a window to a space (clamped). New windows land here.
    pub fn assign(&mut self, key: WindowKey, space: u32) {
        self.assignment
            .insert(key, space.min(self.count.saturating_sub(1)));
    }

    /// The space a window is assigned to.
    ///
    /// # Errors
    /// [`SpacesError::UnknownWindow`] when the window was never
    /// assigned.
    pub fn space_of(&self, key: WindowKey) -> Result<u32, SpacesError> {
        self.assignment
            .get(&key)
            .copied()
            .ok_or(SpacesError::UnknownWindow)
    }

    /// `set_workspace` on a window (clamped). Returns the *actual*
    /// space (what `workspace_changed` should report).
    pub fn move_window(&mut self, key: WindowKey, space: u32) -> u32 {
        let s = space.min(self.count.saturating_sub(1));
        self.assignment.insert(key, s);
        s
    }

    /// Set a window's stickiness.
    pub fn set_sticky(&mut self, key: WindowKey, sticky: bool) {
        if sticky {
            if !self.sticky.contains(&key) {
                self.sticky.push(key);
            }
        } else {
            self.sticky.retain(|k| *k != key);
        }
    }

    /// Whether a window is visible to a seat: sticky, or assigned to
    /// the seat's active space.
    ///
    /// # Errors
    /// [`SpacesError::UnknownSeat`] for an unregistered seat;
    /// [`SpacesError::UnknownWindow`] for an unassigned window.
    pub fn visible_to(&self, seat: u32, key: WindowKey) -> Result<bool, SpacesError> {
        let active = self.active_of(seat)?;
        let home = self.space_of(key)?;
        Ok(self.is_sticky(key) || home == active)
    }

    /// Every window visible to a seat, in arbitrary order (stacking is
    /// the [`crate::stack::WindowStack`]'s job).
    ///
    /// # Errors
    /// [`SpacesError::UnknownSeat`] for an unregistered seat.
    pub fn visible_windows(&self, seat: u32) -> Result<Vec<WindowKey>, SpacesError> {
        let active = self.active_of(seat)?;
        let mut v: Vec<WindowKey> = self
            .assignment
            .iter()
            .filter(|(k, s)| **s == active || self.is_sticky(**k))
            .map(|(k, _)| *k)
            .collect();
        v.sort_unstable();
        Ok(v)
    }

    /// Change the space count. Returns the windows whose assignment
    /// was reflowed, each with its new space (for `workspace_changed`).
    ///
    /// Reflow policy: windows on a removed space move to the highest
    /// surviving space below theirs (or the lowest space when none is
    /// below); seats whose active space vanished move the same way.
    pub fn set_count(&mut self, count: u32) -> Vec<(WindowKey, u32)> {
        let new_count = count.max(1);
        let mut moved = Vec::new();
        if new_count < self.count {
            let last = new_count - 1;
            for (k, s) in &mut self.assignment {
                if *s > last {
                    let target = (*s).min(last);
                    *s = target;
                    moved.push((*k, target));
                }
            }
            for s in self.active.values_mut() {
                if *s > last {
                    *s = last;
                }
            }
        }
        self.count = new_count;
        moved.sort_unstable();
        moved
    }

    /// Forget a destroyed window from every structure (returns its
    /// former space).
    pub fn remove_window(&mut self, key: WindowKey) -> Option<u32> {
        self.sticky.retain(|k| *k != key);
        self.assignment.remove(&key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(n: u64) -> WindowKey {
        WindowKey::new(n)
    }

    #[test]
    fn count_is_at_least_one() {
        assert_eq!(Spaces::new(0).count(), 1);
        assert_eq!(Spaces::new(4).count(), 4);
    }

    #[test]
    fn assignment_clamps_and_reports() {
        let mut s = Spaces::new(3);
        s.assign(k(1), 99);
        assert_eq!(s.space_of(k(1)), Ok(2));
        assert_eq!(s.move_window(k(1), 1), 1);
        assert_eq!(s.space_of(k(1)), Ok(1));
        assert_eq!(s.space_of(k(2)), Err(SpacesError::UnknownWindow));
    }

    #[test]
    fn visibility_follows_the_active_space() {
        let mut s = Spaces::new(4);
        s.add_seat(7, 0);
        s.assign(k(1), 0);
        s.assign(k(2), 2);
        s.set_sticky(k(3), true);
        s.assign(k(3), 3);
        assert_eq!(s.visible_to(7, k(1)), Ok(true));
        assert_eq!(s.visible_to(7, k(2)), Ok(false));
        assert_eq!(s.visible_to(7, k(3)), Ok(true)); // sticky
        assert_eq!(s.switch(7, 2), Ok(2));
        assert_eq!(s.visible_to(7, k(2)), Ok(true));
        assert_eq!(s.visible_to(7, k(1)), Ok(false));
        assert_eq!(s.visible_to(8, k(1)), Err(SpacesError::UnknownSeat));
    }

    #[test]
    fn sticky_toggles_and_unsticky_restores_home() {
        let mut s = Spaces::new(2);
        s.add_seat(1, 1);
        s.assign(k(1), 0);
        s.set_sticky(k(1), true);
        assert_eq!(s.visible_to(1, k(1)), Ok(true));
        s.set_sticky(k(1), false);
        assert_eq!(s.visible_to(1, k(1)), Ok(false));
        // Home space survives the sticky episode.
        assert_eq!(s.space_of(k(1)), Ok(0));
    }

    #[test]
    fn visible_windows_lists_the_active_space_plus_sticky() {
        let mut s = Spaces::new(3);
        s.add_seat(1, 1);
        s.assign(k(10), 1);
        s.assign(k(11), 1);
        s.assign(k(12), 0);
        s.assign(k(13), 2);
        s.set_sticky(k(14), true);
        s.assign(k(14), 2);
        assert_eq!(s.visible_windows(1).unwrap(), vec![k(10), k(11), k(14)]);
    }

    #[test]
    fn count_shrink_reflows_windows_and_seats() {
        let mut s = Spaces::new(5);
        s.add_seat(1, 4);
        s.assign(k(1), 4);
        s.assign(k(2), 3);
        s.assign(k(3), 1);
        let moved = s.set_count(3);
        // Windows on spaces 3 and 4 fall to space 2; window 3 stays.
        // (Reported sorted by window key — the model is deterministic
        // despite the HashMap interior.)
        assert_eq!(moved, vec![(k(1), 2), (k(2), 2)]);
        assert_eq!(s.space_of(k(1)), Ok(2));
        assert_eq!(s.space_of(k(3)), Ok(1));
        assert_eq!(s.active_of(1), Ok(2));
        assert_eq!(s.count(), 3);
        // Growing introduces empty spaces; nothing moves.
        assert!(s.set_count(7).is_empty());
        assert_eq!(s.count(), 7);
        assert_eq!(s.active_of(1), Ok(2));
    }

    #[test]
    fn remove_window_cleans_every_structure() {
        let mut s = Spaces::new(2);
        s.assign(k(1), 1);
        s.set_sticky(k(1), true);
        assert_eq!(s.remove_window(k(1)), Some(1));
        assert!(!s.is_sticky(k(1)));
        assert_eq!(s.space_of(k(1)), Err(SpacesError::UnknownWindow));
        assert_eq!(s.remove_window(k(1)), None);
    }

    #[test]
    fn one_space_is_always_valid_everywhere() {
        let mut s = Spaces::new(1);
        s.add_seat(1, 0);
        s.assign(k(1), 0);
        s.set_sticky(k(1), false);
        assert_eq!(s.visible_to(1, k(1)), Ok(true));
        // Moving beyond the single space clamps to it.
        assert_eq!(s.move_window(k(1), 5), 0);
        assert!(s.set_count(0).is_empty());
        assert_eq!(s.count(), 1);
    }
}
