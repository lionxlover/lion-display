//! Stacking order: the per-parent back-to-front child list with
//! validated restacking and change records for the damage engine.
//!
//! One [`StackingList`] per parent (roots and every surface with
//! children). The list is back-to-front: the *last* child is the
//! front-most. `place_above`/`place_below` validate that the sibling
//! shares the parent (the protocol's requirement) and produce
//! [`StackChange`] records carrying the crossed range — the exact input
//! the damage engine's restack rule needs.

use crate::surface::SurfaceId;

/// A recorded restack: `moved` was relocated, crossing every id in
/// `crossed` (in the parent's new back-to-front order, the front-most
/// participant is `frontmost`).
///
/// The damage rule derived from the fold model: cells covered by *both*
/// the moved surface and any crossed sibling may change value, unless
/// an opaque surface above the whole crossed range covers them — the
/// engine evaluates that mask when its front-to-back walk reaches
/// `frontmost`.
#[derive(Clone, Debug)]
pub struct StackChange {
    /// The surface that moved.
    pub moved: SurfaceId,
    /// The siblings it crossed (old neighborhood, exclusive of itself).
    pub crossed: Vec<SurfaceId>,
    /// The participant closest to the front after the move: the walk
    /// flushes this change's pair-damage when it reaches this surface.
    pub frontmost: SurfaceId,
}

/// One parent's back-to-front child order.
#[derive(Clone, Debug, Default)]
pub struct StackingList {
    children: Vec<SurfaceId>,
}

impl StackingList {
    /// An empty list.
    #[must_use]
    pub const fn new() -> StackingList {
        StackingList {
            children: Vec::new(),
        }
    }

    /// The back-to-front order (last = front-most).
    #[must_use]
    pub fn order(&self) -> &[SurfaceId] {
        &self.children
    }

    /// Number of children.
    #[must_use]
    pub fn len(&self) -> usize {
        self.children.len()
    }

    /// Whether the list is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.children.is_empty()
    }

    /// The index of `id`, if present.
    #[must_use]
    pub fn position(&self, id: SurfaceId) -> Option<usize> {
        self.children.iter().position(|c| *c == id)
    }

    /// Append a child at the front (top of the stack).
    pub fn push_front(&mut self, id: SurfaceId) {
        self.children.push(id);
    }

    /// Remove a child (returns whether it was present).
    pub fn remove(&mut self, id: SurfaceId) -> bool {
        let Some(i) = self.position(id) else {
            return false;
        };
        self.children.remove(i);
        true
    }

    /// Move `mover` to the very front of the parent's stack.
    /// Returns the crossed ids (everything that was above it), or
    /// `None` when `mover` is not a child (or is already alone at the
    /// front — a null restack records nothing).
    pub fn raise_to_front(&mut self, mover: SurfaceId) -> Option<StackChange> {
        let i = self.position(mover)?;
        if i + 1 == self.children.len() {
            return None; // already front-most
        }
        let crossed = self.children[i + 1..].to_vec();
        let frontmost = mover;
        self.children.remove(i);
        self.children.push(mover);
        Some(StackChange {
            moved: mover,
            crossed,
            frontmost,
        })
    }

    /// Move `mover` to the very back of the parent's stack.
    /// Returns the crossed ids (everything that was below it, which the
    /// mover now crosses downward), or `None` for a null restack.
    pub fn lower_to_back(&mut self, mover: SurfaceId) -> Option<StackChange> {
        let i = self.position(mover)?;
        if i == 0 {
            return None; // already back-most
        }
        let crossed = self.children[..i].to_vec();
        // Front-most participant: the sibling that ends up directly in
        // front of the mover's new back-most slot — the last crossed id.
        let frontmost = *crossed.last()?;
        self.children.remove(i);
        self.children.insert(0, mover);
        Some(StackChange {
            moved: mover,
            crossed,
            frontmost,
        })
    }

    /// Restack `mover` immediately above `sibling` (front side).
    /// Both must be children of this parent. Returns the crossed range
    /// (excluding the mover) and the front-most participant, or `None`
    /// on a null restack (already in place).
    ///
    /// Index semantics: back-to-front, so "above" = higher index; the
    /// mover lands at `sibling_index + 1`. Crossing `sibling` itself
    /// happens exactly when the mover starts below it (the relative
    /// order flips); moving down from above crosses only the ids
    /// strictly between.
    ///
    /// # Errors
    ///
    /// [`StackError::ForeignSibling`] when `sibling` is not a child;
    /// [`StackError::ForeignMover`] when `mover` is not a child;
    /// [`StackError::SelfSibling`] when the sibling *is* the mover.
    pub fn place_above(
        &mut self,
        mover: SurfaceId,
        sibling: SurfaceId,
    ) -> Result<Option<StackChange>, StackError> {
        if sibling == mover {
            return Err(StackError::SelfSibling);
        }
        let Some(i) = self.position(mover) else {
            return Err(StackError::ForeignMover);
        };
        let Some(j) = self.position(sibling) else {
            return Err(StackError::ForeignSibling);
        };
        let new_i = j + 1; // immediately above the sibling
        if i == new_i {
            return Ok(None); // already there
        }
        let (crossed, frontmost, insert_at) = if i < new_i {
            // Moving toward the front: crosses i+1..=j (the sibling
            // included — the relative order flips); the mover ends as
            // the front-most participant.
            (self.children[i + 1..=j].to_vec(), mover, j)
        } else {
            // Moving toward the back: crosses j+1..i-1 (the sibling is
            // already below and stays below); the crossed surface that
            // ends front-most is the last one crossed.
            (
                self.children[new_i..i].to_vec(),
                self.children[i - 1],
                new_i,
            )
        };
        self.children.remove(i);
        self.children.insert(insert_at, mover);
        Ok(Some(StackChange {
            moved: mover,
            crossed,
            frontmost,
        }))
    }

    /// Restack `mover` immediately below `sibling` (back side).
    ///
    /// The mover lands at `sibling_index - 1` when it starts below the
    /// sibling (no order flip: it slides up behind it), or at
    /// `sibling_index` when it starts above (order flips with the
    /// sibling, which is therefore crossed).
    ///
    /// # Errors
    ///
    /// As [`Self::place_above`].
    pub fn place_below(
        &mut self,
        mover: SurfaceId,
        sibling: SurfaceId,
    ) -> Result<Option<StackChange>, StackError> {
        if sibling == mover {
            return Err(StackError::SelfSibling);
        }
        let Some(i) = self.position(mover) else {
            return Err(StackError::ForeignMover);
        };
        let Some(j) = self.position(sibling) else {
            return Err(StackError::ForeignSibling);
        };
        if i + 1 == j {
            return Ok(None); // already immediately below
        }
        let (crossed, frontmost, insert_at) = if i > j {
            // Moving toward the back: crosses j..=i-1 (the sibling
            // included — the relative order flips); the last crossed
            // surface ends front-most.
            (self.children[j..i].to_vec(), self.children[i - 1], j)
        } else {
            // Moving toward the front (i < j): crosses i+1..=j-1 (the
            // sibling is already above and stays above); the mover ends
            // front-most among the participants.
            (self.children[i + 1..j].to_vec(), mover, j - 1)
        };
        self.children.remove(i);
        self.children.insert(insert_at, mover);
        Ok(Some(StackChange {
            moved: mover,
            crossed,
            frontmost,
        }))
    }
}

/// Stacking-list misuse.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StackError {
    /// The mover is not a child of this parent.
    ForeignMover,
    /// The sibling is not a child of this parent.
    ForeignSibling,
    /// The sibling is the mover itself.
    SelfSibling,
}

impl std::fmt::Display for StackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StackError::ForeignMover => f.write_str("mover is not a child of this parent"),
            StackError::ForeignSibling => f.write_str("sibling is not a child of this parent"),
            StackError::SelfSibling => f.write_str("a surface cannot stack relative to itself"),
        }
    }
}

impl std::error::Error for StackError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: u64) -> Vec<SurfaceId> {
        (1..=n).map(SurfaceId::from_raw).collect()
    }

    fn list(n: u64) -> StackingList {
        let mut l = StackingList::new();
        for id in ids(n) {
            l.push_front(id);
        }
        l
    }

    #[test]
    fn order_is_back_to_front() {
        let l = list(3);
        assert_eq!(l.order(), ids(3).as_slice());
        assert_eq!(l.len(), 3);
    }

    #[test]
    fn raise_crosses_everything_above() {
        let mut l = list(4); // [1,2,3,4]
        let change = l.raise_to_front(SurfaceId::from_raw(1)).unwrap();
        assert_eq!(change.crossed, ids(4)[1..].to_vec()); // crossed 2,3,4
        assert_eq!(change.frontmost, SurfaceId::from_raw(1));
        assert_eq!(l.order()[3], SurfaceId::from_raw(1));
    }

    #[test]
    fn front_raise_is_null() {
        let mut l = list(3);
        assert!(l.raise_to_front(SurfaceId::from_raw(3)).is_none());
    }

    #[test]
    fn lower_crosses_everything_below() {
        let mut l = list(4); // [1,2,3,4]
        let change = l.lower_to_back(SurfaceId::from_raw(4)).unwrap();
        // Crossed 1,2,3 (it passes them downward); front-most
        // participant is 3 (now directly in front of it).
        assert_eq!(change.crossed, ids(3));
        assert_eq!(change.frontmost, SurfaceId::from_raw(3));
        assert_eq!(l.order()[0], SurfaceId::from_raw(4));
    }

    #[test]
    fn place_above_moves_immediately_above_sibling() {
        let mut l = list(5); // [1,2,3,4,5]
                             // 2 moves above 4: crossed [3,4] (order with 4 flips); result
                             // [1,3,4,2,5]; the mover ends front-most among participants.
        let change = l
            .place_above(SurfaceId::from_raw(2), SurfaceId::from_raw(4))
            .unwrap()
            .unwrap();
        assert_eq!(
            l.order().iter().map(|s| s.raw()).collect::<Vec<_>>(),
            vec![1, 3, 4, 2, 5]
        );
        assert_eq!(
            change.crossed.iter().map(|s| s.raw()).collect::<Vec<_>>(),
            vec![3, 4]
        );
        assert_eq!(change.frontmost, SurfaceId::from_raw(2));
    }

    #[test]
    fn place_above_from_the_front_crosses_downward() {
        let mut l = list(5); // [1,2,3,4,5]
                             // 5 moves above 2: crossed [3,4] (sibling 2 already below, no
                             // flip); result [1,2,5,3,4]; front-most participant is 4.
        let change = l
            .place_above(SurfaceId::from_raw(5), SurfaceId::from_raw(2))
            .unwrap()
            .unwrap();
        assert_eq!(
            l.order().iter().map(|s| s.raw()).collect::<Vec<_>>(),
            vec![1, 2, 5, 3, 4]
        );
        assert_eq!(
            change.crossed.iter().map(|s| s.raw()).collect::<Vec<_>>(),
            vec![3, 4]
        );
        assert_eq!(change.frontmost, SurfaceId::from_raw(4));
    }

    #[test]
    fn place_below_moves_immediately_below_sibling() {
        let mut l = list(5); // [1,2,3,4,5]
                             // 5 moves below 2: crossed [2,3,4] (order with 2 flips); result
                             // [1,5,2,3,4]; front-most participant is 4.
        let change = l
            .place_below(SurfaceId::from_raw(5), SurfaceId::from_raw(2))
            .unwrap()
            .unwrap();
        assert_eq!(
            l.order().iter().map(|s| s.raw()).collect::<Vec<_>>(),
            vec![1, 5, 2, 3, 4]
        );
        assert_eq!(change.frontmost, SurfaceId::from_raw(4));
        assert_eq!(
            change.crossed.iter().map(|s| s.raw()).collect::<Vec<_>>(),
            vec![2, 3, 4]
        );
    }

    #[test]
    fn place_below_from_the_back_crosses_upward() {
        let mut l = list(5); // [1,2,3,4,5]
                             // 1 moves below 3: crossed [2] (sibling 3 already above, no
                             // flip); result [2,1,3,4,5]; the mover ends front-most among
                             // the participants.
        let change = l
            .place_below(SurfaceId::from_raw(1), SurfaceId::from_raw(3))
            .unwrap()
            .unwrap();
        assert_eq!(
            l.order().iter().map(|s| s.raw()).collect::<Vec<_>>(),
            vec![2, 1, 3, 4, 5]
        );
        assert_eq!(
            change.crossed.iter().map(|s| s.raw()).collect::<Vec<_>>(),
            vec![2]
        );
        assert_eq!(change.frontmost, SurfaceId::from_raw(1));
    }

    #[test]
    fn foreign_and_self_siblings_are_rejected() {
        let mut l = list(3);
        let foreign = SurfaceId::from_raw(99);
        assert_eq!(
            l.place_above(SurfaceId::from_raw(1), foreign).unwrap_err(),
            StackError::ForeignSibling
        );
        assert_eq!(
            l.place_above(foreign, SurfaceId::from_raw(1)).unwrap_err(),
            StackError::ForeignMover
        );
        let one = SurfaceId::from_raw(1);
        assert_eq!(
            l.place_above(one, one).unwrap_err(),
            StackError::SelfSibling
        );
        // No mutation happened on the error paths.
        assert_eq!(l.order(), ids(3).as_slice());
    }

    #[test]
    fn already_in_place_restacks_are_null() {
        let mut l = list(4); // [1,2,3,4]
        assert!(l
            .place_above(SurfaceId::from_raw(4), SurfaceId::from_raw(3))
            .unwrap()
            .is_none());
        assert!(l
            .place_below(SurfaceId::from_raw(1), SurfaceId::from_raw(2))
            .unwrap()
            .is_none());
        assert_eq!(l.order(), ids(4).as_slice());
    }

    #[test]
    fn remove_keeps_relative_order() {
        let mut l = list(4);
        assert!(l.remove(SurfaceId::from_raw(2)));
        assert_eq!(
            l.order().iter().map(|s| s.raw()).collect::<Vec<_>>(),
            vec![1, 3, 4]
        );
        assert!(!l.remove(SurfaceId::from_raw(2)));
    }
}
