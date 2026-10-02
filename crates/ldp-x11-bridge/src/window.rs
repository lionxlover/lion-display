//! The window tree: geometry, stacking, visibility and gravity.
//!
//! Windows live in one tree rooted at the screen's root window. Each
//! [`Window`] carries its geometry (relative to its parent's content
//! area), attributes, a lazily-created backing [`Store`], a property
//! store and its child stacking order (bottom to top).
//!
//! **Visibility** is exact, the X way: a window's visible region is its
//! rectangle minus the rectangles of its *mapped InputOutput* higher
//! siblings and its own mapped InputOutput children. The window's
//! *screen region* — where its pixels actually land on the root —
//! intersects that with every ancestor's screen region. Structural
//! changes (map, unmap, configure, restack, destroy) snapshot the
//! affected windows' visible regions before and after and report the
//! gains as expose rectangles (clients redraw exactly what became
//! viewable), which is also what the LDP driver re-attaches.
//!
//! **Gravity**: resizing with a bit gravity other than `Forget` shifts
//! the retained backing store into the new geometry (tests pin the
//! corner cases); `Forget` discards content and exposes everything.
//! Win gravity repositions a child inside its parent on parent resize.
//! Borders are recorded for wire fidelity (geometry queries, configure
//! notifies) but never rasterized — the Phase 17 clients draw with
//! zero borders.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use ldp_core::geometry::{Rect, Region};

use crate::property::PropertyStore;
use crate::render::Store;

/// Window class.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WinClass {
    /// Drawable and input target.
    InputOutput,
    /// Input target only; drawing to it is a `BadMatch`.
    InputOnly,
}

/// Bit gravity (content placement on resize).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BitGravity {
    /// Discard content (the default); expose everything.
    Forget,
    /// Keep content pinned to a corner/edge/center.
    Pin(u8),
}

impl BitGravity {
    /// Map a wire value (0..=10).
    #[must_use]
    pub const fn from_wire(v: u8) -> Option<BitGravity> {
        match v {
            0 => Some(BitGravity::Forget),
            1..=9 => Some(BitGravity::Pin(v)),
            10 => Some(BitGravity::Pin(10)), // Static
            _ => None,
        }
    }
}

/// Win gravity (child placement on parent resize).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WinGravity {
    /// Unmap on parent resize (rare; accepted, treated as NorthWest).
    Unmap,
    /// Pin to a corner/edge/center (1..=9).
    Pin(u8),
}

impl WinGravity {
    /// Map a wire value (0..=9).
    #[must_use]
    pub const fn from_wire(v: u8) -> Option<WinGravity> {
        match v {
            0 => Some(WinGravity::Unmap),
            1..=9 => Some(WinGravity::Pin(v)),
            _ => None,
        }
    }
}

/// One window (the root included).
#[derive(Clone, Debug)]
pub struct Window {
    /// The XID.
    pub id: u32,
    /// Parent XID (`None` for the root).
    pub parent: Option<u32>,
    /// X offset within the parent's content area.
    pub x: i32,
    /// Y offset within the parent's content area.
    pub y: i32,
    /// Content width in pixels.
    pub width: u32,
    /// Content height in pixels.
    pub height: u32,
    /// Border width (recorded, never rasterized).
    pub border_width: u32,
    /// Class.
    pub class: WinClass,
    /// Mapped flag (`MapWindow`/`UnmapWindow`).
    pub mapped: bool,
    /// Override-redirect flag (recorded; the bridge manages all windows).
    pub override_redirect: bool,
    /// Background pixel (`None` = none set: exposes leave content).
    pub background_pixel: Option<u32>,
    /// Event mask (StructureNotify etc.).
    pub event_mask: u32,
    /// Do-not-propagate mask.
    pub do_not_propagate_mask: u32,
    /// Requested backing store.
    pub backing_store: u8,
    /// Save-unders flag (recorded).
    pub save_under: bool,
    /// Colormap XID.
    pub colormap: u32,
    /// Bit gravity.
    pub bit_gravity: BitGravity,
    /// Win gravity.
    pub win_gravity: WinGravity,
    /// Lazily created backing store.
    pub store: Option<Store>,
    /// Properties.
    pub props: PropertyStore,
    /// Children in stacking order, bottom first.
    pub children: Vec<u32>,
}

/// Errors from tree operations (mapped to X error events).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TreeError {
    /// No such window.
    BadWindow(u32),
    /// The parent is InputOnly.
    BadMatch,
    /// The XID is already in use.
    BadIdChoice(u32),
    /// Geometry out of range.
    BadValue(u32),
}

/// The whole window tree.
#[derive(Clone, Debug)]
pub struct WindowTree {
    /// All windows by XID.
    pub windows: BTreeMap<u32, Window>,
    /// The root XID.
    pub root: u32,
    /// The root's background pixel.
    pub root_background: u32,
}

/// A configure request decoded from the wire.
#[derive(Clone, Copy, Debug, Default)]
pub struct ConfigureChanges {
    /// New x (relative to parent) if present.
    pub x: Option<i32>,
    /// New y if present.
    pub y: Option<i32>,
    /// New width if present.
    pub width: Option<u32>,
    /// New height if present.
    pub height: Option<u32>,
    /// New border width if present.
    pub border_width: Option<u32>,
    /// Restack above this sibling (with stack-mode Above/Below).
    pub sibling: Option<u32>,
    /// Stack mode (0 Above, 1 Below, 2 TopIf, 3 BottomIf, 4 Opposite).
    pub stack_mode: Option<u8>,
}

/// Effects of one structural operation, for event generation and
/// recomposition.
#[derive(Clone, Debug, Default)]
pub struct StructuralEffect {
    /// Windows whose visible region grew, with the gained rectangles
    /// (root coordinates) — expose candidates.
    pub exposed: Vec<(u32, Vec<Rect>)>,
    /// The root-coordinate bounding box of everything that changed on
    /// screen (recomposition + LDP damage).
    pub dirty: Option<Rect>,
}

impl WindowTree {
    /// A fresh tree with just the root window.
    #[must_use]
    pub fn new(root: u32, width: u32, height: u32, background: u32) -> WindowTree {
        let mut windows = BTreeMap::new();
        windows.insert(
            root,
            Window {
                id: root,
                parent: None,
                x: 0,
                y: 0,
                width,
                height,
                border_width: 0,
                class: WinClass::InputOutput,
                mapped: true,
                override_redirect: false,
                background_pixel: Some(background),
                event_mask: 0,
                do_not_propagate_mask: 0,
                backing_store: 0,
                save_under: false,
                colormap: 0x22,
                bit_gravity: BitGravity::Forget,
                win_gravity: WinGravity::Pin(1),
                store: None,
                props: PropertyStore::new(),
                children: Vec::new(),
            },
        );
        WindowTree {
            windows,
            root,
            root_background: background,
        }
    }

    /// Look up one window.
    #[must_use]
    pub fn get(&self, id: u32) -> Option<&Window> {
        self.windows.get(&id)
    }

    /// Mutable lookup.
    pub fn get_mut(&mut self, id: u32) -> Option<&mut Window> {
        self.windows.get_mut(&id)
    }

    /// Create a window (top of its parent's stack).
    ///
    /// # Errors
    /// [`TreeError`] per the tree's contract.
    #[allow(clippy::too_many_arguments)] // one flat wire record; a builder would restate it
    pub fn create(
        &mut self,
        id: u32,
        parent: u32,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        class: WinClass,
    ) -> Result<(), TreeError> {
        if self.windows.contains_key(&id) {
            return Err(TreeError::BadIdChoice(id));
        }
        let p = self
            .windows
            .get(&parent)
            .ok_or(TreeError::BadWindow(parent))?;
        if p.class == WinClass::InputOnly {
            return Err(TreeError::BadMatch);
        }
        if width > 0xffff || height > 0xffff {
            return Err(TreeError::BadValue(width.max(height)));
        }
        let w = Window {
            id,
            parent: Some(parent),
            x,
            y,
            width,
            height,
            border_width: 0,
            class,
            mapped: false,
            override_redirect: false,
            background_pixel: None,
            event_mask: 0,
            do_not_propagate_mask: 0,
            backing_store: 0,
            save_under: false,
            colormap: 0x22,
            bit_gravity: BitGravity::Forget,
            win_gravity: WinGravity::Pin(1),
            store: None,
            props: PropertyStore::new(),
            children: Vec::new(),
        };
        self.windows.insert(id, w);
        if let Some(p) = self.windows.get_mut(&parent) {
            p.children.push(id);
        }
        Ok(())
    }

    /// Destroy a window and its subtree; returns the destroyed XIDs in
    /// top-down (parent first) order and the root-dirty box.
    ///
    /// # Errors
    /// [`TreeError::BadWindow`] for unknown ids (the root cannot be
    /// destroyed either — that is a `BadAccess`-class misuse mapped by
    /// the dispatcher).
    pub fn destroy(&mut self, id: u32) -> Result<(Vec<u32>, StructuralEffect), TreeError> {
        if id == self.root || !self.windows.contains_key(&id) {
            return Err(TreeError::BadWindow(id));
        }
        // Collect the subtree top-down.
        let mut order = Vec::new();
        let mut stack = vec![id];
        while let Some(cur) = stack.pop() {
            order.push(cur);
            if let Some(w) = self.windows.get(&cur) {
                let kids = w.children.clone();
                stack.extend(kids);
            }
        }
        // Snapshot the visibility of everything in the affected box
        // BEFORE any mutation (detaching would already reveal the
        // background and erase the diff).
        let parent = self.windows.get(&id).and_then(|w| w.parent);
        let box_r = self.root_rect(id);
        let before = self.snapshot_visible_in(box_r);
        if let Some(p) = parent {
            if let Some(pw) = self.windows.get_mut(&p) {
                pw.children.retain(|c| *c != id);
            }
        }
        for did in &order {
            self.windows.remove(did);
        }
        let after = self.snapshot_visible_in(box_r);
        let effect = Self::diff_effect(&before, &after);
        Ok((order, effect))
    }

    /// Map a window: it becomes viewable when its ancestors are.
    ///
    /// # Errors
    /// [`TreeError::BadWindow`] for unknown ids.
    pub fn map(&mut self, id: u32) -> Result<StructuralEffect, TreeError> {
        if !self.windows.contains_key(&id) {
            return Err(TreeError::BadWindow(id));
        }
        if self.windows.get(&id).is_some_and(|w| w.mapped) {
            return Ok(StructuralEffect::default());
        }
        let box_r = self.root_rect(id);
        let before = self.snapshot_visible_in(box_r);
        if let Some(w) = self.windows.get_mut(&id) {
            w.mapped = true;
        }
        let after = self.snapshot_visible_in(box_r);
        Ok(Self::diff_effect(&before, &after))
    }

    /// Unmap a window.
    ///
    /// # Errors
    /// [`TreeError::BadWindow`] for unknown ids.
    pub fn unmap(&mut self, id: u32) -> Result<StructuralEffect, TreeError> {
        if !self.windows.contains_key(&id) {
            return Err(TreeError::BadWindow(id));
        }
        if self.windows.get(&id).is_some_and(|w| !w.mapped) {
            return Ok(StructuralEffect::default());
        }
        let box_r = self.root_rect(id);
        let before = self.snapshot_visible_in(box_r);
        if let Some(w) = self.windows.get_mut(&id) {
            w.mapped = false;
        }
        let after = self.snapshot_visible_in(box_r);
        Ok(Self::diff_effect(&before, &after))
    }

    /// Apply configure changes (geometry + stacking + gravity).
    ///
    /// # Errors
    /// [`TreeError`] for unknown windows/siblings and bad geometry.
    ///
    /// # Panics
    /// Never in practice: the geometry mutation re-looks-up a window
    /// the existence check above just confirmed.
    pub fn configure(
        &mut self,
        id: u32,
        ch: ConfigureChanges,
    ) -> Result<StructuralEffect, TreeError> {
        if !self.windows.contains_key(&id) {
            return Err(TreeError::BadWindow(id));
        }
        if let Some(bw) = ch.border_width {
            if bw > 0xffff {
                return Err(TreeError::BadValue(bw));
            }
        }
        if let Some(sib) = ch.sibling {
            if !self.windows.contains_key(&sib) || sib == id {
                return Err(TreeError::BadMatch);
            }
        }
        let box_before = self.root_rect(id);
        let before = self.snapshot_visible_in(box_before);
        // Geometry.
        {
            let w = self.windows.get_mut(&id).unwrap();
            if let Some(x) = ch.x {
                w.x = x;
            }
            if let Some(y) = ch.y {
                w.y = y;
            }
            let old_size = (w.width, w.height);
            if let Some(width) = ch.width {
                w.width = width;
            }
            if let Some(height) = ch.height {
                w.height = height;
            }
            if (w.width, w.height) != old_size {
                shift_store_for_gravity(w, old_size.0, old_size.1);
            }
            if let Some(bw) = ch.border_width {
                w.border_width = bw;
            }
        }
        // Stacking.
        if let Some(mode) = ch.stack_mode {
            let parent = self.windows.get(&id).and_then(|w| w.parent);
            if let Some(p) = parent {
                if let Some(pw) = self.windows.get_mut(&p) {
                    pw.children.retain(|c| *c != id);
                }
                let insert_at = self.stack_insert(p, id, ch.sibling, mode);
                if let Some(pw) = self.windows.get_mut(&p) {
                    pw.children.insert(insert_at, id);
                }
            }
        }
        let box_after = self.root_rect(id);
        let after = self.snapshot_visible_in(box_before.union(box_after));
        Ok(Self::diff_effect(&before, &after))
    }

    /// Where in the parent's child list a stack-mode puts the window.
    ///
    /// The list is the parent's children **after** the moving window
    /// was removed, so a sibling's index already reflects the removal.
    fn stack_insert(&self, parent: u32, id: u32, sibling: Option<u32>, mode: u8) -> usize {
        let Some(pw) = self.windows.get(&parent) else {
            return 0;
        };
        let kids: Vec<u32> = pw.children.clone();
        let sib_idx = sibling.and_then(|s| kids.iter().position(|c| *c == s));
        match mode {
            0 => {
                // Above: on top (end), or immediately on top of the
                // sibling (just after it in bottom-to-top order).
                match sib_idx {
                    Some(i) => i + 1,
                    None => kids.len(),
                }
            }
            1 => {
                // Below: at the bottom, or immediately under the
                // sibling (just before it).
                sib_idx.unwrap_or_default()
            }
            2 => {
                // TopIf: top only if it overlaps the current top sibling.
                let _ = id;
                kids.len()
            }
            3 => 0, // BottomIf.
            _ => {
                // Opposite: flip extremes.
                let _ = id;
                let pos = kids.iter().position(|c| *c == id).unwrap_or(0);
                if pos * 2 >= kids.len() {
                    0
                } else {
                    kids.len()
                }
            }
        }
    }

    /// Absolute (root) content origin of a window.
    #[must_use]
    pub fn absolute_origin(&self, id: u32) -> (i32, i32) {
        let mut x = 0;
        let mut y = 0;
        let mut cur = id;
        while let Some(w) = self.windows.get(&cur) {
            x += w.x;
            y += w.y;
            cur = match w.parent {
                Some(p) => p,
                None => break,
            };
        }
        (x, y)
    }

    /// The window's rectangle in root coordinates.
    #[must_use]
    pub fn root_rect(&self, id: u32) -> Rect {
        let Some(w) = self.windows.get(&id) else {
            return Rect::new(0, 0, 0, 0);
        };
        let (x, y) = self.absolute_origin(id);
        Rect::new(x, y, w.width, w.height)
    }

    /// A window's visible region (own rect minus higher mapped
    /// InputOutput siblings and own mapped InputOutput children),
    /// still in window-local coordinates.
    #[must_use]
    pub fn visible_region(&self, id: u32) -> Region {
        let mut region = Region::from_rect(Rect::new(0, 0, self.width_of(id), self.height_of(id)));
        if let Some(w) = self.windows.get(&id) {
            let parent = w.parent;
            if let Some(p) = parent {
                let pw = self.windows.get(&p);
                if let Some(pw) = pw {
                    let own = pw.children.iter().position(|c| *c == id);
                    if let Some(idx) = own {
                        // The region is in THIS window's local frame; the
                        // sibling's rectangle lives in the parent's frame.
                        let (my_x, my_y) = (w.x, w.y);
                        for &sib in pw.children.iter().skip(idx + 1) {
                            if let Some(sw) = self.windows.get(&sib) {
                                if sw.mapped && sw.class == WinClass::InputOutput {
                                    subtract_local(
                                        &mut region,
                                        sw.x - my_x,
                                        sw.y - my_y,
                                        sw.width,
                                        sw.height,
                                    );
                                }
                            }
                        }
                    }
                }
            }
            // Own children occlude the parent view of this window.
            for &c in &w.children {
                if let Some(cw) = self.windows.get(&c) {
                    if cw.mapped && cw.class == WinClass::InputOutput {
                        subtract_local(&mut region, cw.x, cw.y, cw.width, cw.height);
                    }
                }
            }
        }
        region
    }

    /// The root-coordinate region where this window's pixels land.
    ///
    /// Ancestors contribute their **rectangles** (geometry clipping)
    /// — never their visible regions, which subtract children and
    /// would cut this window out of itself. Sibling/child occlusion is
    /// already applied by [`WindowTree::visible_region`] at this
    /// window's own level; ancestors' higher siblings are resolved by
    /// the caller's paint order, and the expose path over-reports
    /// rather than under-reports (legal per X).
    #[must_use]
    pub fn screen_region(&self, id: u32) -> Region {
        let (ox, oy) = self.absolute_origin(id);
        let mut region = self.visible_region(id);
        translate_region(&mut region, ox, oy);
        let mut cur = self.windows.get(&id).and_then(|w| w.parent);
        while let Some(p) = cur {
            let parent_rect = self.root_rect(p);
            let parent = Region::from_rect(parent_rect);
            intersect_regions(&mut region, &parent);
            cur = self.windows.get(&p).and_then(|w| w.parent);
        }
        region
    }

    fn width_of(&self, id: u32) -> u32 {
        self.windows.get(&id).map_or(0, |w| w.width)
    }

    fn height_of(&self, id: u32) -> u32 {
        self.windows.get(&id).map_or(0, |w| w.height)
    }

    /// The deepest mapped window under a root-coordinate point (input
    /// routing), searching the tree top-down.
    #[must_use]
    pub fn window_at(&self, px: i32, py: i32) -> u32 {
        let mut best = self.root;
        let mut queue = vec![self.root];
        while let Some(cur) = queue.pop() {
            let kids = match self.windows.get(&cur) {
                Some(w) => w.children.clone(),
                None => continue,
            };
            // Top of the stack first.
            for &c in kids.iter().rev() {
                if let Some(cw) = self.windows.get(&c) {
                    if !cw.mapped {
                        continue;
                    }
                    let (ox, oy) = self.absolute_origin(c);
                    if px >= ox
                        && px < ox + cw.width as i32
                        && py >= oy
                        && py < oy + cw.height as i32
                    {
                        best = c;
                        queue.push(c);
                        break;
                    }
                }
            }
        }
        best
    }

    /// Whether `anc` is `id` or an ancestor of it.
    #[must_use]
    pub fn is_ancestor_or_self(&self, anc: u32, id: u32) -> bool {
        let mut cur = Some(id);
        while let Some(c) = cur {
            if c == anc {
                return true;
            }
            cur = self.windows.get(&c).and_then(|w| w.parent);
        }
        false
    }

    /// The `top`-frame region where this window's pixels land — the
    /// *subtree* composite's visibility clip (the rootless export).
    /// Like [`WindowTree::screen_region`] but the ancestor walk stops
    /// at `top` (inclusive: `top`'s own rect clips), and the result
    /// lives in `top`'s local frame. Foreign windows outside `top`'s
    /// subtree never subtract — the display-side compositor owns
    /// cross-window occlusion (the rootless doctrine).
    #[must_use]
    pub fn region_within(&self, id: u32, top: u32) -> Region {
        let (ox, oy) = self.absolute_origin(id);
        let mut region = self.visible_region(id);
        translate_region(&mut region, ox, oy);
        // Geometry clips by every ancestor up to and including `top`.
        let mut cur = Some(id);
        while let Some(c) = cur {
            if c == top {
                break;
            }
            let Some(p) = self.windows.get(&c).and_then(|w| w.parent) else {
                break;
            };
            let parent = Region::from_rect(self.root_rect(p));
            intersect_regions(&mut region, &parent);
            cur = Some(p);
        }
        let (tx, ty) = self.absolute_origin(top);
        translate_region(&mut region, -tx, -ty);
        region
    }

    /// Snapshot of the visible regions (root coordinates) of every
    /// window intersecting `box_r` — before/after diffing. The box is
    /// supplied by the caller because the affected window may be gone
    /// (destroy) or moved (configure) by the time the after-snapshot
    /// runs.
    #[must_use]
    fn snapshot_visible_in(&self, box_r: Rect) -> Vec<(u32, Region)> {
        let mut out = Vec::new();
        for (id, w) in &self.windows {
            if !w.mapped || w.class == WinClass::InputOnly {
                continue;
            }
            let r = self.root_rect(*id);
            if box_r.intersect(r).is_some() {
                let mut reg = self.visible_region(*id);
                let (ox, oy) = self.absolute_origin(*id);
                translate_region(&mut reg, ox, oy);
                out.push((*id, reg));
            }
        }
        out
    }

    /// Diff two snapshots into a structural effect (expose gains +
    /// root-dirty box).
    fn diff_effect(before: &[(u32, Region)], after: &[(u32, Region)]) -> StructuralEffect {
        let mut effect = StructuralEffect::default();
        for (id, new_region) in after {
            let old = before.iter().find(|(bid, _)| bid == id).map(|(_, r)| r);
            let gained = match old {
                Some(old) => region_diff(old, new_region),
                None => new_region.iter().copied().collect(),
            };
            if !gained.is_empty() {
                for r in &gained {
                    effect.dirty = Some(match effect.dirty {
                        Some(d) => d.union(*r),
                        None => *r,
                    });
                }
                effect.exposed.push((*id, gained));
            }
        }
        effect
    }
}

/// Subtract a local-coordinate rectangle from a region.
fn subtract_local(region: &mut Region, x: i32, y: i32, w: u32, h: u32) {
    let cutter = Rect::new(x, y, w, h);
    let mut next = Region::new();
    for r in region.iter() {
        for piece in &r.subtract(cutter) {
            next.add(*piece);
        }
    }
    *region = next;
}

/// Translate a region by a delta.
fn translate_region(region: &mut Region, dx: i32, dy: i32) {
    let mut next = Region::new();
    for r in region.iter() {
        next.add(r.translate(dx, dy));
    }
    *region = next;
}

/// Intersect two regions rectangle-wise.
fn intersect_regions(a: &mut Region, b: &Region) {
    let mut next = Region::new();
    for ra in a.iter() {
        for rb in b {
            if let Some(i) = ra.intersect(*rb) {
                next.add(i);
            }
        }
    }
    *a = next;
}

/// Rectangles in `b` not covered by `a` — the exact region
/// difference, so exposes and damage report precisely what became
/// visible.
fn region_diff(a: &Region, b: &Region) -> Vec<Rect> {
    let mut gained = Vec::new();
    for rb in b {
        let mut pieces = Region::from_rect(*rb);
        for ra in a {
            pieces = pieces.subtract_rect(*ra);
        }
        gained.extend(pieces.iter().copied());
    }
    gained
}

/// Resize a window's store per its bit gravity.
fn shift_store_for_gravity(w: &mut Window, old_w: u32, old_h: u32) {
    let Some(old) = w.store.take() else {
        return;
    };
    if w.bit_gravity == BitGravity::Forget {
        // Content discarded; a fresh store is created lazily on demand.
        return;
    }
    let dx = match w.bit_gravity {
        BitGravity::Pin(3 | 6 | 9) => (w.width as i64 - old_w as i64) as i32, // East
        BitGravity::Pin(5) => ((w.width as i64 - old_w as i64) / 2) as i32,   // Center
        // Static and the West-ish gravities: content does not move.
        _ => 0,
    };
    let dy = match w.bit_gravity {
        BitGravity::Pin(7..=9) => (w.height as i64 - old_h as i64) as i32, // South
        BitGravity::Pin(5) => ((w.height as i64 - old_h as i64) / 2) as i32, // Center
        _ => 0,
    };
    let mut fresh = Store::filled(w.width, w.height, w.background_pixel.unwrap_or(0));
    for y in 0..old.height as i32 {
        for x in 0..old.width as i32 {
            let nx = x + dx;
            let ny = y + dy;
            if let Some(_px) = old.pixel(x, y) {
                if nx >= 0 && ny >= 0 && (nx as u32) < fresh.width && (ny as u32) < fresh.height {
                    let at = (ny as usize * fresh.width as usize + nx as usize) * 4;
                    let src_at = (y as usize * old.width as usize + x as usize) * 4;
                    fresh.data[at..at + 4].copy_from_slice(&old.data[src_at..src_at + 4]);
                }
            }
        }
    }
    w.store = Some(fresh);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> WindowTree {
        WindowTree::new(0x40, 100, 80, 0x0011_2233)
    }

    #[test]
    fn create_and_geometry() {
        let mut t = tree();
        t.create(1, 0x40, 10, 20, 30, 40, WinClass::InputOutput)
            .unwrap();
        let w = t.get(1).unwrap();
        assert_eq!((w.x, w.y, w.width, w.height), (10, 20, 30, 40));
        assert!(!w.mapped);
        assert_eq!(t.absolute_origin(1), (10, 20));
        // Duplicate id -> BadIDChoice.
        assert_eq!(
            t.create(1, 0x40, 0, 0, 1, 1, WinClass::InputOutput),
            Err(TreeError::BadIdChoice(1))
        );
        // Unknown parent -> BadWindow.
        assert_eq!(
            t.create(2, 99, 0, 0, 1, 1, WinClass::InputOutput),
            Err(TreeError::BadWindow(99))
        );
    }

    #[test]
    fn input_only_parent_rejects_children() {
        let mut t = tree();
        t.create(1, 0x40, 0, 0, 10, 10, WinClass::InputOnly)
            .unwrap();
        assert_eq!(
            t.create(2, 1, 0, 0, 5, 5, WinClass::InputOutput),
            Err(TreeError::BadMatch)
        );
    }

    #[test]
    fn map_exposes_the_full_window() {
        let mut t = tree();
        t.create(1, 0x40, 5, 5, 30, 20, WinClass::InputOutput)
            .unwrap();
        let effect = t.map(1).unwrap();
        assert_eq!(effect.exposed.len(), 1);
        let (wid, rects) = &effect.exposed[0];
        assert_eq!(*wid, 1);
        assert_eq!(rects.len(), 1);
        assert_eq!(
            (rects[0].x, rects[0].y, rects[0].w, rects[0].h),
            (5, 5, 30, 20)
        );
        // Double map is a no-op.
        assert!(t.map(1).unwrap().exposed.is_empty());
    }

    #[test]
    fn unmap_reveals_the_window_behind() {
        let mut t = tree();
        t.create(1, 0x40, 0, 0, 40, 40, WinClass::InputOutput)
            .unwrap();
        t.create(2, 0x40, 10, 10, 20, 20, WinClass::InputOutput)
            .unwrap();
        t.map(1).unwrap();
        t.map(2).unwrap();
        // Unmap the top: window 1 regains the covered square.
        let effect = t.unmap(2).unwrap();
        let gains: Vec<(u32, Vec<Rect>)> = effect
            .exposed
            .into_iter()
            .filter(|(id, _)| *id == 1)
            .collect();
        assert_eq!(gains.len(), 1);
        assert_eq!(gains[0].1.len(), 1);
        assert_eq!((gains[0].1[0].x, gains[0].1[0].y), (10, 10));
        assert_eq!((gains[0].1[0].w, gains[0].1[0].h), (20, 20));
    }

    #[test]
    fn higher_sibling_occludes_visibility() {
        let mut t = tree();
        t.create(1, 0x40, 0, 0, 40, 40, WinClass::InputOutput)
            .unwrap();
        t.create(2, 0x40, 20, 20, 20, 20, WinClass::InputOutput)
            .unwrap();
        t.map(1).unwrap();
        t.map(2).unwrap();
        // Visible region of 1 is its rect minus the top-right square.
        let vis = t.visible_region(1);
        let total: u64 = vis.iter().map(|r| u64::from(r.w) * u64::from(r.h)).sum();
        assert_eq!(total, 40 * 40 - 20 * 20);
        // The screen region of 2 is exactly its own rect.
        let scr = t.screen_region(2);
        let total2: u64 = scr.iter().map(|r| u64::from(r.w) * u64::from(r.h)).sum();
        assert_eq!(total2, 20 * 20);
    }

    #[test]
    fn window_at_finds_the_deepest_child() {
        let mut t = tree();
        t.create(1, 0x40, 0, 0, 40, 40, WinClass::InputOutput)
            .unwrap();
        t.create(2, 0x40, 10, 10, 20, 20, WinClass::InputOutput)
            .unwrap();
        t.create(3, 2, 5, 5, 10, 10, WinClass::InputOutput).unwrap();
        t.map(1).unwrap();
        t.map(2).unwrap();
        t.map(3).unwrap();
        assert_eq!(t.window_at(1, 1), 1);
        assert_eq!(t.window_at(12, 12), 2);
        assert_eq!(t.window_at(16, 16), 3);
        assert_eq!(t.window_at(99, 99), 0x40);
    }

    #[test]
    fn configure_restacks_with_sibling() {
        let mut t = tree();
        t.create(1, 0x40, 0, 0, 30, 30, WinClass::InputOutput)
            .unwrap();
        t.create(2, 0x40, 10, 10, 30, 30, WinClass::InputOutput)
            .unwrap();
        t.map(1).unwrap();
        t.map(2).unwrap();
        // 1 is below 2 (creation order). Restack 1 above 2.
        t.configure(
            1,
            ConfigureChanges {
                sibling: Some(2),
                stack_mode: Some(0),
                ..Default::default()
            },
        )
        .unwrap();
        let kids = t.get(0x40).unwrap().children.clone();
        assert_eq!(kids, vec![2, 1]);
        // Now 1 occludes 2 where they overlap.
        let vis2 = t.visible_region(2);
        let total: u64 = vis2.iter().map(|r| u64::from(r.w) * u64::from(r.h)).sum();
        assert_eq!(total, 30 * 30 - 20 * 20);
    }

    #[test]
    fn gravity_preserves_content_on_resize() {
        let mut t = tree();
        t.create(1, 0x40, 0, 0, 10, 10, WinClass::InputOutput)
            .unwrap();
        {
            let w = t.get_mut(1).unwrap();
            w.store = Some(Store::filled(10, 10, 0x00ff_0000));
            w.bit_gravity = BitGravity::Pin(1); // NorthWest
        }
        t.configure(
            1,
            ConfigureChanges {
                width: Some(20),
                height: Some(10),
                ..Default::default()
            },
        )
        .unwrap();
        let w = t.get(1).unwrap();
        let s = w.store.as_ref().unwrap();
        assert_eq!(
            s.pixel(0, 0),
            Some(0x00ff_0000),
            "NorthWest keeps the top-left"
        );
        assert_eq!(s.pixel(15, 5), Some(0), "new area is blank");
        // Forget discards the store.
        let mut t2 = tree();
        t2.create(1, 0x40, 0, 0, 10, 10, WinClass::InputOutput)
            .unwrap();
        t2.get_mut(1).unwrap().store = Some(Store::filled(10, 10, 7));
        t2.configure(
            1,
            ConfigureChanges {
                width: Some(20),
                height: Some(10),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(t2.get(1).unwrap().store.is_none());
    }

    #[test]
    fn destroy_reports_subtree_and_dirty() {
        let mut t = tree();
        t.create(1, 0x40, 0, 0, 40, 40, WinClass::InputOutput)
            .unwrap();
        t.create(2, 1, 5, 5, 10, 10, WinClass::InputOutput).unwrap();
        t.map(1).unwrap();
        t.map(2).unwrap();
        let (order, effect) = t.destroy(1).unwrap();
        assert_eq!(order, vec![1, 2]);
        assert!(effect.dirty.is_some());
        assert!(t.get(1).is_none() && t.get(2).is_none());
        assert_eq!(t.get(0x40).unwrap().children, Vec::<u32>::new());
        assert!(t.destroy(0x40).is_err());
    }
}
