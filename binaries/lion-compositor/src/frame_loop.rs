//! The frame loop: the pump that advances the world to quiescence.
//!
//! Headless time doctrine (locked in the Phase 10 design): the mock
//! device's injected clock is the *only* clock, and it advances only
//! here, at wake points — never speculatively. One [`World::pump`]
//! call runs the world until nothing more can happen:
//!
//! 1. commits are pending → **the shared damage pass** (Phase 31):
//!    take the tree changes, run the damage engine over the whole
//!    *logical desktop* (the union of the served outputs), resolve
//!    per-surface visibility per output (enter/leave_output), expand
//!    the Liquid effect rects, advance the dock, and *accumulate* the
//!    repaint into every output's pending region — the damage pass
//!    runs once per commit, however many outputs serve it;
//! 2. an output has pending damage and no flip in flight → **render**:
//!    re-target the persistent renderer at that output (its own
//!    description, its damage translated to output-local coordinates),
//!    composite the layer stack (bounds translated by the output's
//!    layout position), copy the frame into that output's back scanout
//!    buffer, and submit its own atomic page flip
//!    (`PAGE_FLIP_EVENT | NONBLOCK`) on its own CRTC;
//! 3. a flip is in flight on some output → advance the clock *exactly*
//!    to the next device event (idle vblanks from sibling outputs ride
//!    along and are consumed), drain the page-flip event, feed the
//!    scheduler (`observe_flip` — the *primary* output's flips anchor
//!    the pacing grid), release superseded buffers whose every gate
//!    has passed, and loop;
//! 4. otherwise the world is quiescent.
//!
//! Events produced along the way route by ownership: the waking
//! client's entries return for direct emission through its session
//! context; everyone else's park in the outboxes for their own next
//! wake. Presentation feedback therefore flows to idle clients at
//! their next message — and to the driving client immediately, which
//! is exactly the self-sustaining animation loop the protocol's
//! `frame` → `frame_target` → `commit` → `presented` cycle assumes.

#![forbid(unsafe_code)]

use std::collections::HashMap;

use ldp_compositor::damage::{DamageEngine, FrameDamage};
use ldp_compositor::sched_types::SchedEvent;
use ldp_compositor::surface::SurfaceId;
use ldp_core::buffer::Modifier;
use ldp_core::geometry::{Rect, Region};
use ldp_core::ids::ClientId;
use ldp_core::time::Mono;
use ldp_core::wire::Value;
use ldp_display::events::DeviceEvent;
use ldp_display::serve;
use ldp_planes::LayerFacts;
use ldp_renderer::{BufferView, Renderer, SurfaceLayer};

use crate::low_bits;
use crate::outbox::OutboxEntry;
use crate::rearrange::Rearrange;
use crate::scene::{PendingRelease, Route, World};
use crate::sys;

/// A frame-loop failure: render, device, fence-minting, or
/// re-arrangement errors — all fatal for the calling session (display
/// pipelines do not degrade silently).
#[derive(Debug)]
pub enum FrameError {
    /// The renderer refused the frame.
    Render(ldp_renderer::RendererError),
    /// The device rejected the atomic commit.
    Display(ldp_display::DisplayError),
    /// The release fence could not be minted.
    Sys(sys::SysError),
    /// A live re-arrangement failed: scanout allocation or the new
    /// pipeline's enable was rejected (Phase 26's migration arm).
    Rearrange(String),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Render(e) => write!(f, "render failure: {e}"),
            Self::Display(e) => write!(f, "device rejected the flip: {e}"),
            Self::Sys(e) => write!(f, "release-fence minting failed: {e}"),
            Self::Rearrange(why) => write!(f, "re-arrangement failed: {why}"),
        }
    }
}

impl std::error::Error for FrameError {}

impl From<ldp_renderer::RendererError> for FrameError {
    fn from(e: ldp_renderer::RendererError) -> Self {
        Self::Render(e)
    }
}

impl From<ldp_display::DisplayError> for FrameError {
    fn from(e: ldp_display::DisplayError) -> Self {
        Self::Display(e)
    }
}

impl From<sys::SysError> for FrameError {
    fn from(e: sys::SysError) -> Self {
        Self::Sys(e)
    }
}

impl World {
    /// Advance the world to quiescence; returns the waking client's
    /// direct-emission entries (others were routed to their outboxes).
    ///
    /// The dark state (Phase 26: every connector gone) short-circuits
    /// — no scanout to render into, no flip to land. The scene still
    /// absorbs commits (the desktop waits for light); releases owed
    /// flush immediately (nothing is being read anymore); the
    /// scheduler's parked emissions drain.
    ///
    /// # Errors
    ///
    /// [`FrameError`] — fatal for the calling session.
    pub fn pump(&mut self, me: ClientId) -> Result<Vec<OutboxEntry>, FrameError> {
        let mut mine = Vec::new();
        // Phase 47: compositor-owned motion advances at every wake —
        // one integration per pump, at the driver's clock (the dock's
        // advance doctrine). A live transition (or one that just
        // settled — its exact-terminal frame must render) dirties the
        // scene; the frame-callback economy drives the cadence, and
        // the serve loop's poll tick wakes a desktop whose clients all
        // sleep (`pump_animations`).
        self.advance_transitions();
        if self.outputs.is_empty() {
            let entries = self.fire_releases()?;
            self.route(entries, me, &mut mine);
            let sched = self.scene.scheduler.drain();
            let entries = sched_entries(&self.scene.routes, sched);
            self.route(entries, me, &mut mine);
            return Ok(mine);
        }
        loop {
            // Commits landed: the shared damage pass distributes the
            // repaint across the outputs' pending regions.
            if self.scene.dirty {
                let entries = self.accumulate_pass();
                self.route(entries, me, &mut mine);
                continue;
            }
            // Every output with pending damage and a free flip
            // renders; the primary also renders when it owes the
            // commit's frame (empty damage included).
            let mut rendered = false;
            for slot in 0..self.outputs.len() {
                let busy = self.outputs[slot].scanout.flip_pending;
                let owed = self.outputs[slot].owes;
                if (!self.outputs[slot].pending.is_empty() || owed) && !busy {
                    self.render_slot(slot)?;
                    rendered = true;
                }
            }
            if rendered {
                continue;
            }
            // A flip is in flight somewhere: land it.
            if self.outputs.iter().any(|s| s.scanout.flip_pending) {
                let entries = self.land_flip()?;
                self.route(entries, me, &mut mine);
                continue;
            }
            break;
        }
        // Whatever the scheduler concluded (deferred frame targets,
        // expiries) drains last.
        let sched = self.scene.scheduler.drain();
        let entries = sched_entries(&self.scene.routes, sched);
        self.route(entries, me, &mut mine);
        Ok(mine)
    }

    /// Split a batch of entries into the waking client's direct batch
    /// and everyone else's outbox parking.
    fn route(&mut self, entries: Vec<OutboxEntry>, me: ClientId, mine: &mut Vec<OutboxEntry>) {
        for e in entries {
            if e.client == me {
                mine.push(e);
            } else {
                self.outboxes.push(e);
            }
        }
    }

    /// The logical desktop's bounds — the union of the served
    /// outputs' layout rectangles (the damage engine's coordinate
    /// space). A single-output world at the origin is exactly that
    /// output's bounds (the Phase 25 space).
    ///
    /// The mirror doctrine (Phase 37): the desktop is the *primary's*
    /// bounds — the whole desktop is what the primary shows, every
    /// other display a crop of it. (A union would inflate the desktop
    /// to the largest display; windows would place into area no
    /// display anchors, and the primary would clip content the
    /// desktop claimed to serve. The overlap at the origin is the
    /// clone signal itself.)
    #[must_use]
    pub fn desktop_bounds(&self) -> Rect {
        if self.arrangement == crate::scene::OutputArrangement::Mirrored {
            return self.primary_bounds();
        }
        let mut bounds = Rect::EMPTY;
        for slot in &self.outputs {
            bounds = bounds.union(slot.output.bounds());
        }
        bounds
    }

    /// The primary output's bounds (the shell's coordinate space —
    /// the primary always sits at the layout origin, so its bounds are
    /// the Phase 28 space verbatim).
    fn primary_bounds(&self) -> Rect {
        self.outputs
            .first()
            .map_or(Rect::EMPTY, |slot| slot.output.bounds())
    }

    /// The shared damage pass (Phase 31): one damage computation over
    /// the whole desktop, distributed to every output's pending
    /// region. Runs once per commit batch — however many outputs serve
    /// the desktop, the damage engine runs once and each output clips
    /// what is ours at render time.
    ///
    /// Also the enter/leave_output authority: per-surface visibility
    /// resolves per output here (a window spanning two monitors is
    /// entered on both), and the routes' `visible_on` sets update here
    /// (the release-gate truth for superseded buffers).
    ///
    /// # Panics
    ///
    /// Never in-crate: the pump runs this only on a lit world (the
    /// dark state short-circuits first).
    // Phase 48 grew the pass by the ghosts' rect claims (the vacate
    // doctrine's third member); the damage narrative stays one
    // function by doctrine.
    #[allow(clippy::too_many_lines)]
    fn accumulate_pass(&mut self) -> Vec<OutboxEntry> {
        let desktop = self.desktop_bounds();
        let primary = self.primary_bounds();
        let changes = self.scene.take_changes();
        let mut damage = DamageEngine::compute(&mut self.scene.tree, &changes, desktop);
        // The system dock rides above every client surface (Phase
        // 28): a client layer beneath it can never direct-scanout
        // while the dock shows over that area — subtract the dock's
        // reservation from the scanout candidacy (the compositor
        // composites; the plane-assignment oracle stays honest).
        if let Some(dock) = self.shell.dock_rect() {
            damage.scanout = std::mem::take(&mut damage.scanout).subtract_rect(dock);
        }
        let entries = self.visibility_entries(&damage);
        let snapshot = self.scene.snapshot();
        // The damage pass's own repaint moves into the working set: the
        // spread below only ever grows it, and the pass struct keeps
        // only its visible map afterward — the v0.11 pass cloned the
        // whole region here, a full rect-list copy per commit batch for
        // nothing.
        let mut repaint = std::mem::take(&mut damage.repaint);
        if self.effects != ldp_renderer::EffectTier::Minimal {
            // The Liquid spreads: a styled surface's shadow reaches
            // beyond its ink, so the repaint grows to the effect rects
            // (the shadow below a freshly-mapped window paints in the
            // same frame — the damage engine sees only protocol
            // damage). Extra repaint area is cost, never wrongness:
            // the full stack re-composites there.
            for id in snapshot.render_order() {
                let Some(node) = snapshot.node(*id) else {
                    continue;
                };
                if !node.mapped {
                    continue;
                }
                let style = surface_style(
                    self.effects,
                    node,
                    primary,
                    self.popups.is_popup_surface(*id),
                    self.scene.material_requests.get(id).copied(),
                );
                if !style.is_plain() {
                    repaint.add(style.effect_rect(node.bounds));
                }
            }
        }
        // The dock's frame (Phase 28) — the advance, the repaint
        // claims, and this frame's placement (see `advance_dock`).
        self.advance_dock(primary, &mut repaint);
        // Phase 47: the compositor's own motion claims its rects — a
        // live fade moves every pixel under its surface (the vacate
        // doctrine, the dock's own). The snapshot carries the current
        // bounds; a surface mid-fade that moved claims its new rect
        // (the geometry is the claim, the alpha is the render's).
        for id in self
            .scene
            .transitions
            .live
            .keys()
            .copied()
            .collect::<Vec<_>>()
        {
            if let Some(node) = snapshot.node(id) {
                if node.mapped {
                    repaint.add(node.bounds);
                }
            }
        }
        // Phase 48: the ghosts claim their rects — a fading ghost
        // moves every pixel under it (the vacate doctrine again), and
        // a styled ghost's frozen chrome rides along (a fading shadow
        // is still a shadow — the claim matches the client walk's).
        for ghost in &self.scene.ghosts.ghosts {
            repaint.add(ghost.dest);
            if !ghost.style.is_plain() {
                repaint.add(ghost.style.effect_rect(ghost.dest));
            }
        }
        // The vacated rects — the ghosts the last advance settled: a
        // ghost is nobody's surface (no client frame economy keeps
        // its region rendering), so the removal claim is the host's
        // own — the settle frame repaints the ink off the canvas.
        for rect in std::mem::take(&mut self.scene.ghosts.vacated) {
            repaint.add(rect);
        }
        // Phase 49: the states arm's own claims — minimized and
        // off-space windows are nobody's layer while hidden (no
        // client frame economy re-renders their region), so the
        // hide/unhide itself owns the repaint: the same vacate
        // doctrine, the hidden set's fourth member.
        for rect in std::mem::take(&mut self.scene.hide_claims) {
            repaint.add(rect);
        }
        // The HDR fold (Phase 31): the stack summary over the desktop
        // (every mapped surface's color description and HDR metadata)
        // feeds the output-mode controller's hysteresis — an HDR
        // surface takes the composite onto the PQ canvas after the
        // dwell; all-SDR returns it. The negotiated peak (the stack's
        // brightest content clamped to the panel) is recorded for
        // the render's tone-mapping ceiling.
        //
        // Phase 38: the recording itself. The panel's *effective*
        // peak (the advertisement, already `--hdr-peak`-capped at
        // bring-up) meets the stack's brightest content
        // (`negotiated_ceiling`) — dimmer content negotiates to
        // itself, brighter content maps down to the panel. The canvas
        // and every HDR layer's luminance tail read this value.
        if self.hdr.is_some() {
            let caps = self
                .outputs
                .first()
                .and_then(|slot| slot.output.hdr)
                .unwrap_or_else(ldp_hdr::policy::OutputCaps::sdr_only);
            let mut summary = ldp_hdr::policy::StackSummary::all_sdr();
            for id in snapshot.render_order() {
                let Some(node) = snapshot.node(*id) else {
                    continue;
                };
                if !node.mapped {
                    continue;
                }
                if node.color.is_hdr() {
                    if node.color.transfer == ldp_core::color::TransferFunction::Pq {
                        summary.any_pq = true;
                    } else if node.color.transfer == ldp_core::color::TransferFunction::Hlg {
                        summary.any_hlg = true;
                    }
                }
                if let Some(meta) = &node.hdr {
                    summary.max_content_luminance = summary.max_content_luminance.max(meta.max_cll);
                    if meta.mastering_primaries != ldp_core::color::Primaries::Bt709 {
                        summary.any_wide_gamut = true;
                    }
                }
                if node.color.primaries != ldp_core::color::Primaries::Bt709 {
                    summary.any_wide_gamut = true;
                }
            }
            if let Some(controller) = self.hdr.as_mut() {
                controller.step(&caps, &summary);
            }
            // The negotiation (Phase 38): only an HDR panel negotiates.
            self.negotiated = caps.hdr.then(|| {
                ldp_hdr::negotiation::negotiated_ceiling(
                    caps.max_luminance,
                    summary.max_content_luminance,
                )
                .as_nits()
            });
        }
        // Distribute: every output accumulates the desktop repaint
        // intersected with its own bounds (desktop coordinates — the
        // render pass translates to output-local). The primary *also*
        // owes a frame whatever the damage says — the pacing grid's
        // flip must land for every commit's presentation verdict (the
        // Phase 25 semantics: dirty renders, damage or no damage; an
        // attach-less commit still presents).
        for (index, slot) in self.outputs.iter_mut().enumerate() {
            let ours = repaint.clipped_to(slot.output.bounds());
            slot.pending.add_region(&ours);
            if index == 0 {
                slot.owes = true;
            }
        }
        self.scene.dirty = false;
        entries
    }

    /// The render pass for one output (Phase 31): re-target the
    /// persistent renderer at the output's own description with its
    /// pending damage translated to output-local coordinates, composite
    /// the layer stack (bounds translated by the output's layout
    /// position), deliver the frame into that output's back scanout
    /// buffer, and submit the output's own page flip.
    ///
    /// Phase 34 — the hardware arm: the layer stack is *also* graded
    /// for plane eligibility (the import walk's framebuffer per
    /// buffer, the placement/color/opaque facts), the
    /// [`ldp_planes`] solver maps the eligible suffix onto the
    /// output's plane set, and the frame takes one of three shapes:
    ///
    /// * **zero-composite** — every layer rides hardware and the
    ///   bottom layer's own buffer covers the output opaquely: the
    ///   renderer emits *no pass at all* (the client's pixels scan
    ///   out untouched — zero GPU passes, zero uploads, zero
    ///   copies; the dump path forces the composite arm so
    ///   screenshots keep their meaning);
    /// * **split** — the composite prefix renders into the canvas on
    ///   the primary, the offloaded suffix rides overlays above it
    ///   (the underlay shape Windows MPO serves);
    /// * **full composite** — nothing rides: the Phase 31 bytes
    ///   verbatim.
    ///
    /// The dock renders on the primary output of an extended desktop
    /// (the macOS doctrine: the dock owns the primary display) — and
    /// on *every* output of a mirrored one (Phase 37: the dock is the
    /// desktop's own chrome, and the mirror shows the desktop; the
    /// primary still sits at the layout origin, so its coordinates
    /// *are* desktop coordinates — the Phase 28 geometry unchanged).
    /// The dock's own
    /// ink composites in v1 (CPU-generated pixels; the plane delivery
    /// path is the dumb-buffer staging roadmap line) — which means a
    /// visible dock pins the frame to the composite arm, exactly the
    /// honest subtraction the demotion ledger names.
    ///
    /// # Panics
    ///
    /// The hardware arm's grading walk (Phase 34): the solver's facts
    /// and the display model's layer stack for one output's frame —
    /// one pass over the mapped surfaces (the import walk's
    /// framebuffer per buffer, the Liquid styling's demotion facts)
    /// plus the dock's CPU-ink fact at the stack's end. The two
    /// vectors stay index-aligned by construction.
    ///
    /// A free associated function over a context bundle so the
    /// returned layers borrow the *scene* (not the world): the render
    /// pass keeps its disjoint-field borrows — the scene feeds the
    /// stack, the renderer and the outputs mutate independently.
    ///
    /// # Panics
    ///
    /// Never in-crate: the walk guards every lookup.
    // The context is a borrow bundle moved in for the call (its
    // mutable members are used, the struct itself needs no drop).
    #[allow(clippy::needless_pass_by_value)]
    // Phase 47 grew the walk by the one-opacity truth (two lines); the
    // grading narrative stays one function by doctrine.
    #[allow(clippy::too_many_lines)]
    fn grade_frame<'a>(
        ctx: GradeContext<'a, '_>,
        layout: (i32, i32),
        bounds: Rect,
        dock_here: bool,
    ) -> (Vec<LayerFacts>, Vec<Option<SurfaceLayer<'a>>>) {
        let mut facts: Vec<LayerFacts> = Vec::new();
        let mut stack: Vec<Option<SurfaceLayer<'_>>> = Vec::new();
        let scene = ctx.scene;
        let snapshot = ctx.snapshot;
        // Phase 48 — the ghost queue: the live close fades, sorted by
        // the z slot each dying window held (the drain inserts every
        // ghost before the layer that took its place — the fade
        // keeps the window's own position in the stack, never above
        // the windows that were above it).
        let mut ghosts: Vec<&crate::scene::Ghost> = scene.ghosts.ghosts.iter().collect::<Vec<_>>();
        ghosts.sort_by_key(|g| g.z);
        let mut gi = 0usize;
        for id in snapshot.render_order() {
            let Some(node) = snapshot.node(*id) else {
                continue;
            };
            if !node.mapped {
                continue;
            }
            let Some(route) = scene.routes.get(id) else {
                continue;
            };
            // Phase 49: the states arm's hidden set — a minimized or
            // off-space window is nobody's layer (the vacate claims
            // repaint the ink it left; the frame economy parks).
            if scene.hidden.contains(id) {
                continue;
            }
            let Some((buffer_obj, buf)) = &route.buffer else {
                continue;
            };
            let pool_key = crate::scene::ObjectKey {
                client: buf.pool().client,
                object: buf.pool().object,
            };
            let Some(pool) = scene.pools.get(&pool_key) else {
                continue;
            };
            // The import walk: once per buffer object (the ledger
            // caches refusals as honestly as successes).
            let buffer_key = crate::scene::ObjectKey {
                client: route.client.as_u32(),
                object: buffer_obj.as_u32(),
            };
            let fb =
                crate::plane_session::import_walk(ctx.device, ctx.imports, buffer_key, buf, pool);
            let local_bounds = node.bounds.translate(-layout.0, -layout.1);
            let local_opaque = node.opaque_region.translate(-layout.0, -layout.1);
            // Phase 47: the one opacity truth — the transitions host's
            // value (1.0 when no motion serves), feeding BOTH the
            // solver's facts and the renderer's layer, so the two
            // cannot diverge (the material map's own doctrine).
            let opacity = ctx.scene.transitions.opacity_of(*id);
            let style = surface_style(
                ctx.effects,
                node,
                bounds,
                ctx.popups.is_popup_surface(*id),
                ctx.scene.material_requests.get(id).copied(),
            );
            let geometry = buf.geometry(pool.size());
            // The ghosts whose slot this layer takes (the fade's z
            // fidelity: the ghost lands exactly where its window
            // stood, before the layer that now occupies the index).
            while gi < ghosts.len() && ghosts[gi].z <= facts.len() {
                let (f, l) = ghost_layer(ghosts[gi], layout, ctx.negotiated);
                facts.push(f);
                stack.push(l);
                gi += 1;
            }
            facts.push(crate::plane_session::client_layer_facts(
                local_bounds,
                &geometry,
                node,
                local_opaque.clone(),
                &style,
                fb,
                opacity,
            ));
            let layer = buf.view(pool).ok().map(|view| {
                let mut layer = SurfaceLayer::new(
                    view,
                    local_bounds,
                    node.transform,
                    node.color,
                    opacity,
                    local_opaque,
                );
                layer.style = style;
                layer.tone = layer_tone_policy(node, ctx.negotiated);
                layer
            });
            stack.push(layer);
        }
        // The ghosts the walk never passed (their window was
        // topmost, or the stack shrank past their slot): they render
        // above the remaining windows, below the dock — the chrome
        // stays the chrome.
        while gi < ghosts.len() {
            let (f, l) = ghost_layer(ghosts[gi], layout, ctx.negotiated);
            facts.push(f);
            stack.push(l);
            gi += 1;
        }
        // The dock's facts (the desktop's chrome, drawn where it
        // shows — the primary's own layer in an extended desktop,
        // every display's in a mirror): the ink is CPU-generated — no
        // framebuffer, the honest NoFb demotion.
        if dock_here && ctx.shell.dock.is_some() {
            let dock_rect = ctx
                .shell
                .dock
                .as_ref()
                .map_or(Rect::EMPTY, super::shell::SystemDock::current_rect);
            let needs_backdrop = ctx
                .shell
                .dock
                .as_ref()
                .is_some_and(|d| d.style(ctx.effects).backdrop.is_some());
            let dock_styled = ctx.shell.dock.as_ref().is_some_and(|d| {
                let style = d.style(ctx.effects);
                style.corner_radius > 0 || style.shadow.is_some()
            });
            facts.push(LayerFacts {
                dest: dock_rect,
                width: dock_rect.w,
                height: dock_rect.h,
                format: ldp_core::buffer::FourCC::ARGB8888,
                modifier: Modifier::LINEAR,
                transform: ldp_core::geometry::Transform::Normal,
                color: ldp_core::color::ColorDescription::srgb_sdr(),
                opacity: 1.0,
                opaque: Region::new(),
                fb: None,
                needs_backdrop,
                styled: dock_styled,
                system: true,
            });
            let dock_layer = ctx.shell.dock.as_ref().and_then(|dock| {
                dock.layer(dock.current_rect(), dock.style(ctx.effects))
                    .ok()
            });
            stack.push(dock_layer);
        }
        (facts, stack)
    }

    // The three frame shapes (zero / split / full) are one
    // narrative — splitting them would hide the damage handoff
    // between the arms.
    #[allow(clippy::too_many_lines)]
    fn render_slot(&mut self, slot_index: usize) -> Result<(), FrameError> {
        // The flip this pass submits releases a sleeping panel (the
        // device's own implicit rescan — the kernel clears the
        // property when a page flip arrives; the mock models it, the
        // timeline re-anchoring one full nominal later). The machine
        // must learn *now*, before the commit: a Damage exit, no
        // property write (the flip carries the release), the ledger
        // pays the rescan (Phase 35).
        self.psr_disturb_slot(slot_index, ldp_power::psr::PsrExit::Damage, false);
        let (bounds, desc, layout, was_all_planes, is_primary) = {
            let slot = &self.outputs[slot_index];
            let bounds = slot.output.bounds();
            // The HDR mode's canvas (Phase 31): the controller's
            // current output mode picks the description — PQ when
            // the dwell settled on HDR over an HDR panel, sRGB
            // otherwise (the Phase 25 bytes). Phase 38: the PQ
            // canvas carries the negotiated ceiling (the stack's
            // brightest content clamped to the panel's effective
            // peak) as its luminance max — the honest record of
            // what the frame will hold.
            let hdr_mode = self.hdr.as_ref().map_or(
                ldp_hdr::policy::OutputMode::Sdr,
                ldp_hdr::policy::ModeController::mode,
            );
            (
                bounds,
                slot.output.output_desc_hdr(hdr_mode, self.negotiated),
                slot.output.layout,
                slot.all_planes,
                slot_index == 0,
            )
        };
        let snapshot = self.scene.snapshot();
        // The dock draws where the desktop's chrome shows (Phase 37):
        // the primary owns it in an extended desktop; a mirrored
        // desktop shows it on *every* display — the dock is the
        // desktop's own chrome, and the mirror shows the desktop.
        let dock_here = is_primary || self.arrangement == crate::scene::OutputArrangement::Mirrored;
        // The tone policies serve only on the PQ canvas (Phase 38):
        // the negotiated ceiling applies when the mode controller
        // settled on HDR over an HDR panel — an SDR canvas (the
        // dwell pending, or an SDR panel) keeps the anchor-pull
        // doctrine, every pre-Phase-38 byte.
        let tone_ceiling = if desc.color.transfer == ldp_core::color::TransferFunction::Pq {
            self.negotiated
        } else {
            None
        };
        let (facts, stack) = Self::grade_frame(
            GradeContext {
                scene: &self.scene,
                snapshot: &snapshot,
                shell: &self.shell,
                effects: self.effects,
                negotiated: tone_ceiling,
                popups: &self.popups,
                device: self.device.as_mut(),
                imports: &mut self.imports,
            },
            layout,
            bounds,
            dock_here,
        );
        let inventory = self.outputs[slot_index].planes.clone();
        let out_facts =
            crate::plane_session::output_facts(desc.width, desc.height, desc.format, desc.color);
        let plan = ldp_planes::Assigner.solve(&facts, &inventory, &out_facts);
        // The dump path observes the composed truth (the diagnostic
        // doctrine): `--dump` pins the frame to the composite arm.
        let dump_on = self.dump.is_some();
        let zero = plan.zero_composite && !dump_on;

        if zero {
            // The zero-composite frame: no renderer pass, no canvas
            // write — the client's own buffer scans out. The display
            // model still composes first (the stale canvas as the
            // base is harmless: the primary-role layer covers
            // opaquely, its blend replaces every word), then the
            // borrows end and the mutable frame bookkeeping runs.
            let readout = self.renderer.readout();
            let mut base: Vec<u32> =
                if readout.len() == (desc.width as usize) * (desc.height as usize) {
                    readout.into_owned()
                } else {
                    vec![
                        ldp_renderer::pack_canonical(0, 0, 0, 255);
                        desc.width as usize * desc.height as usize
                    ]
                };
            // The base is owned here (the readout clone or the fresh
            // black canvas) — the planes blend straight into it, no
            // second full-canvas copy and no extra allocation (the
            // historical `compose_display` copied the base again to
            // produce the model).
            crate::plane_session::blend_planes(&mut base, desc.width, desc.height, &stack, &plan);
            // The stack's scene borrow ends here; the frame's
            // bookkeeping mutates the world's own fields.
            drop(stack);
            drop(facts);
            self.submit_plane_frame(slot_index, &plan, None)?;
            let slot = &mut self.outputs[slot_index];
            slot.display = base;
            slot.pending.clear();
            slot.owes = false;
            slot.all_planes = true;
            self.frames += 1;
            self.zero_pass_frames += 1;
            return Ok(());
        }

        // The composite arm: the renderer composites the layers the
        // plan did not offload (all of them, when the plan is empty —
        // the Phase 31 bytes). Returning from an all-planes frame
        // means the persistent canvas is stale by exactly one frame:
        // the damage grows to the full output.
        let local_damage = if was_all_planes {
            Region::from_rect(Rect::new(0, 0, desc.width, desc.height))
        } else {
            self.outputs[slot_index]
                .pending
                .clipped_to(bounds)
                .translate(-layout.0, -layout.1)
        };
        let offloaded: std::collections::HashSet<usize> =
            plan.assignments.iter().map(|a| a.layer).collect();
        self.renderer.begin_frame(&desc, &local_damage)?;
        // The background pass: damage not covered by the layer stack
        // falls back to opaque black — unmaps, detaches, and removals
        // repaint to the desktop.
        self.renderer.clear_damage(0, 0, 0, 0xFF)?;

        // The renderer composites the subset no plane took (the
        // shared stack, index-aligned with the plan — parents before
        // children, the dock last on the primary, exactly the
        // sequence the facts walk built); the display model then
        // blends the plane-carried layers over the fresh canvas in
        // zpos order — what the panel's plane blender shows.
        {
            let composite: Vec<SurfaceLayer<'_>> = stack
                .iter()
                .enumerate()
                .filter(|(i, layer)| layer.is_some() && !offloaded.contains(i))
                .filter_map(|(_, layer)| layer.clone())
                .collect();
            self.renderer.submit(&composite)?;
            self.renderer.end_frame()?;
            // The display model, allocation-stable: the slot's own Vec
            // carries the fresh base (capacity reused across frames —
            // one base memcpy, no per-frame heap allocation; the
            // historical path built a fresh full-canvas Vec every
            // frame), then the planes blend over it in place.
            {
                let frame = self.renderer.readout();
                let base: &[u32] = &frame;
                let slot = &mut self.outputs[slot_index];
                slot.display.clear();
                slot.display.extend_from_slice(base);
            }
            crate::plane_session::blend_planes(
                &mut self.outputs[slot_index].display,
                desc.width,
                desc.height,
                &stack,
                &plan,
            );
        }

        // Deliver the frame: full copy into the back buffer (the
        // renderer's framebuffer is the canonical composition; the
        // scanout chain is the delivery vehicle — damage clipping
        // saved the *render* work, not the copy). The canvas is handed
        // to the scanout *by borrow* — the historical `to_vec()` here
        // was a second full-canvas copy per frame (8 MiB at 4K) whose
        // only reader was the optional frame dump, which can borrow
        // the same slice itself.
        let canvas_fb = {
            let frame = self.renderer.readout();
            let words: &[u32] = &frame;
            let slot = &mut self.outputs[slot_index];
            slot.scanout.write_back(words);
            slot.scanout.fbs[1 - slot.scanout.front]
        };
        let slot = &mut self.outputs[slot_index];
        slot.pending.clear();
        slot.owes = false;
        self.frames += 1;
        if is_primary {
            if let Some(dir) = &self.dump {
                crate::dump::write_frame(dir, self.frames, &desc, &self.renderer.readout());
            }
        }
        // Phase 34: the split frame's commit carries the canvas on the
        // primary *and* the offloaded overlays above it.
        self.submit_plane_frame(slot_index, &plan, Some(canvas_fb))?;
        Ok(())
    }

    /// Submit the multi-plane atomic frame (Phase 34): the plane
    /// commit (assignments on with zpos, retired overlays off, the
    /// canvas or the bottom layer's own framebuffer on the primary),
    /// the VRR arming the flip family has always carried, and the
    /// flip ledger bookkeeping — `submitted()` sets the pending flip
    /// the pump waits on, `active_planes` records what this commit
    /// left on (the next commit's off-set), `all_planes` records the
    /// zero-composite shape (the next composite frame's full-damage
    /// rule).
    ///
    /// # Panics
    ///
    /// Never in-crate: the pump only flips slots of a lit world.
    fn submit_plane_frame(
        &mut self,
        slot_index: usize,
        plan: &ldp_planes::ScanoutPlan,
        canvas_fb: Option<ldp_display::ids::FbId>,
    ) -> Result<(), FrameError> {
        let (crtc, vrr_capable, plane_ids) = {
            let slot = &self.outputs[slot_index];
            (slot.crtc, slot.output.vrr.is_some(), slot.plane)
        };
        let mode = self.outputs[slot_index].output.mode().clone();
        let (w, h) = (u32::from(mode.hdisplay), u32::from(mode.vdisplay));
        let mut request =
            crate::plane_session::plane_commit(&self.outputs[slot_index], plan, canvas_fb, (w, h));
        // Adaptive sync (Phase 31, `--vrr`): arm the CRTC's VRR
        // enablement on every flip of a VRR-capable output — the
        // panel stretches inside its window (the mock clamps exactly
        // like the kernel). Phase 41's uniform collapse
        // (`--vrr-uniform`, the `vrr-sibling-flicker` escape): a
        // mixed desktop serves one fixed sync across the seam — no
        // output arms, whatever its own capability.
        if self.vrr_enabled && vrr_capable && !self.vrr_collapse {
            request = request.crtc_vrr(crtc, true);
        }
        let _ = plane_ids;
        self.device.commit(&request)?;
        let slot = &mut self.outputs[slot_index];
        slot.active_planes = plan.assignments.iter().map(|a| a.plane).collect();
        slot.all_planes = plan.zero_composite && canvas_fb.is_none();
        slot.scanout.submitted();
        Ok(())
    }

    /// The dock's frame (Phase 28): integrate the intro rise at the
    /// driver's clock (ms — the same timestamps the flips land on,
    /// the motion doctrine), collect this frame's placement, and
    /// claim the repaint the rise and the frost need.
    ///
    /// * While rising: the vacate rule adds the old and new
    ///   placements (`SystemDock::advance`).
    /// * At rest with a styled tier: the frost samples the backdrop
    ///   *inside* the dock's rect (clamp-edge blur — no spread beyond
    ///   it), so damage intersecting the dock's rect must extend to
    ///   the whole rect (the frost re-blurs, the ink re-composites
    ///   over it). At Minimal the dock is plain ink — its pixels are
    ///   their own truth, no claim.
    fn advance_dock(&mut self, bounds: Rect, repaint: &mut Region) {
        // (Hoisted before the mutable dock borrow: the immutable
        // self reads must not nest inside it.)
        let now_ms = self.now().as_ns() / 1_000_000;
        let styled = self.effects != ldp_renderer::EffectTier::Minimal;
        self.shell.dock.as_mut().map(|system_dock| {
            let dest = system_dock.advance(now_ms, repaint);
            if styled {
                let claim = rect_intersect(dest, bounds);
                if !claim.is_empty() && !repaint.clipped_to(claim).is_empty() {
                    repaint.add(claim);
                }
            }
            dest
        });
    }

    /// Advance the compositor-owned transitions one pump-wake (Phase
    /// 47): the host integrates at the driver's clock and reports
    /// whether any motion was live — the repaint claim that keeps the
    /// fade advancing (a just-settled transition claims its final,
    /// exact-terminal frame; the next wake claims nothing —
    /// quiescence).
    fn advance_transitions(&mut self) {
        if !self.scene.transitions.enabled {
            return;
        }
        let now_ms = self.now().as_ns() / 1_000_000;
        let mut live = self.scene.transitions.advance(now_ms);
        // Phase 48: the close fades advance in the same breath — one
        // clock, one integration per wake (a ghost that just settled
        // claims its removal frame; the next wake claims nothing).
        live |= self.scene.ghosts.advance(now_ms);
        if live {
            self.scene.dirty = true;
        }
    }

    /// Whether compositor-owned motion is live (the serve loop's
    /// self-wake predicate — `pump_animations` consults it at the
    /// poll cadence).
    #[must_use]
    pub fn transitions_live(&self) -> bool {
        self.scene.transitions.any_live() || self.scene.ghosts.any_live()
    }

    /// The serve loop's animation self-wake (Phase 47): while motion
    /// is live, run the world to quiescence under the *system* wake
    /// identity — [`ClientId::SERVER`], the reserved "not a client"
    /// origin that owns no routes, so every emission parks in its
    /// owner's outbox exactly as a client-driven wake would route it
    /// (the system wake never swallows events).
    ///
    /// # Errors
    /// [`FrameError`] — fatal for the calling session (the same
    /// contract every pump serves).
    pub fn pump_animations(&mut self) -> Result<(), FrameError> {
        if !self.transitions_live() {
            return Ok(());
        }
        self.pump(ClientId::SERVER)?;
        Ok(())
    }

    /// enter/leave_output transitions from the damage pass's visible
    /// map, in SurfaceId order (deterministic) — Phase 31's per-output
    /// resolution: a surface visible on two outputs enters both (one
    /// event per output object the client holds); a surface leaving
    /// one output but not the other leaves only the one.
    ///
    /// Also the `visible_on` bookkeeping: the routes' record of which
    /// outputs' last rendered frames showed each surface — the
    /// release-gate truth for superseded buffers.
    ///
    /// Phase 45 — the occlusion quiescing authority (the App-Nap
    /// doctrine, the one structural power macOS's concentration buys
    /// and this wiring earns): a surface visible on **zero** outputs —
    /// fully occluded, unmapped, or unplaced — has its scheduler slot
    /// hidden here, and one visible again is unhidden. The transition
    /// is change-driven (the route's `quiesced` flag against this
    /// pass's truth), so the steady frame costs nothing; the
    /// scheduler's own semantics do the rest — the hidden slot's live
    /// registration dies with `SurfaceHidden`, its later `frame`
    /// requests park unanswered, and the unhide both re-enables
    /// presentation and answers what parked. The occluded client
    /// stops being asked to draw; the power it would spend rendering
    /// frames nobody can see stays its own.
    fn visibility_entries(&mut self, damage: &FrameDamage) -> Vec<OutboxEntry> {
        let mut ids: Vec<SurfaceId> = self.scene.routes.keys().copied().collect();
        ids.sort_by_key(|id| id.raw());
        let mut entries = Vec::new();
        for id in ids {
            let visible_region = damage.visible.get(&id).cloned().unwrap_or_else(Region::new);
            // The outputs showing this surface, in slot order.
            let mut now_visible: Vec<ldp_display::ids::CrtcId> = self
                .outputs
                .iter()
                .filter(|slot| !visible_region.clipped_to(slot.output.bounds()).is_empty())
                .map(|slot| slot.crtc)
                .collect();
            // Phase 49: a states-arm-hidden surface (minimized, or
            // homed on a space the seat is not viewing) is on *no*
            // output — the enter/leave truth, the release-gate
            // truth, and the quiesce truth all follow from this one
            // clearing (App Nap parks its frame requests through the
            // flip below, exactly the occlusion path's own seam).
            if self.scene.hidden.contains(&id) {
                now_visible.clear();
            }
            // Phase 45: record the bookkeeping and detect the quiesce
            // transition under one short borrow, then drive the
            // scheduler outside it (routes and scheduler are disjoint
            // fields of the scene, but the route borrow must not
            // straddle the scheduler call).
            //
            // Only a **mapped** surface quiesces: "fully occluded" is
            // the App-Nap predicate — the surface exists on the
            // desktop and everything it owns is hidden behind other
            // windows. An unmapped surface's slot stays untouched, so
            // the frame-before-first-commit choreography answers
            // exactly as it always has (the mapping commit's own pass
            // is what resolves it onto its outputs).
            let quiesce_flip = {
                let Some(route) = self.scene.routes.get_mut(&id) else {
                    continue;
                };
                route.visible_on.clone_from(&now_visible);
                let mapped = self
                    .scene
                    .tree
                    .get(id)
                    .is_some_and(|s| s.state().is_mapped());
                let now_hidden = mapped && now_visible.is_empty();
                (route.quiesced != now_hidden).then(|| {
                    route.quiesced = now_hidden;
                    now_hidden
                })
            };
            if let Some(hidden) = quiesce_flip {
                let now = self.now();
                self.scene.scheduler.set_visibility(id, hidden, now);
            }
            let Some(route) = self.scene.routes.get_mut(&id) else {
                continue;
            };
            // Enters: newly visible outputs the client holds objects
            // for (a bind is required — the event carries the object).
            let mut enter: Vec<(ldp_display::ids::CrtcId, ldp_core::ids::ObjectId)> = Vec::new();
            for crtc in &now_visible {
                if !route.entered.contains(crtc) {
                    if let Some(object) = self
                        .scene
                        .output_binds
                        .get(&(route.client.as_u32(), *crtc))
                        .copied()
                    {
                        route.entered.push(*crtc);
                        enter.push((*crtc, object));
                    }
                }
            }
            // Leaves: outputs no longer visible.
            let mut leave: Vec<(ldp_display::ids::CrtcId, ldp_core::ids::ObjectId)> = Vec::new();
            let mut still: Vec<ldp_display::ids::CrtcId> = Vec::new();
            // The entered set is taken and reassembled (still-visible
            // first, then the new enters append): the v0.11 pass cloned
            // the vec per route per frame to keep the borrow checker
            // happy — the take is the same arithmetic without the copy.
            let mut entered = std::mem::take(&mut route.entered);
            for crtc in entered.drain(..) {
                if now_visible.contains(&crtc) {
                    still.push(crtc);
                } else if let Some(object) = self
                    .scene
                    .output_binds
                    .get(&(route.client.as_u32(), crtc))
                    .copied()
                {
                    leave.push((crtc, object));
                } else {
                    // Left an output whose bind is already gone (the
                    // object was revoked with the output): nothing to
                    // tell the client — the revocation already did.
                    still.push(crtc);
                }
            }
            route.entered = still;
            for (crtc, object) in enter {
                entries.push(OutboxEntry::event(
                    route.client,
                    route.surface_obj,
                    "enter_output",
                    vec![Value::Object(Some(object))],
                ));
                // The scale hint (Phase 31): the entered output's
                // fractional factor — the client's rendering truth
                // (identity emits 256, the Phase 25 byte-stream plus
                // one honest hint).
                if let Some(slot) = self.outputs.iter().find(|s| s.crtc == crtc) {
                    entries.push(OutboxEntry::event(
                        route.client,
                        route.surface_obj,
                        "preferred_scale",
                        vec![Value::Uint32(slot.output.scale.to_q8())],
                    ));
                }
            }
            for (_, object) in leave {
                entries.push(OutboxEntry::event(
                    route.client,
                    route.surface_obj,
                    "leave_output",
                    vec![Value::Object(Some(object))],
                ));
            }
        }
        entries
    }

    /// Advance to the next pending flip's landing and process the
    /// event: scheduler observation, buffer releases. The wait is the
    /// driver's — the mock advances its deterministic clock exactly
    /// to the landing; the real device blocks on the DRM fd until the
    /// kernel delivers the flip (the wake-point integration).
    ///
    /// Phase 31: a multi-output wait may surface an idle output's
    /// bare vblank *before* the flip we are waiting for — those ride
    /// along and are consumed (the ladder's food); the wait loops
    /// until a page flip actually lands.
    fn land_flip(&mut self) -> Result<Vec<OutboxEntry>, FrameError> {
        loop {
            let events = self.device.wait_events(None)?;
            let has_flip = events.iter().any(|e| matches!(e, DeviceEvent::PageFlip(_)));
            let entries = self.land_events(events)?;
            if has_flip {
                return Ok(entries);
            }
            // Idle vblanks from a sibling output arrived first: they
            // are consumed and the wait continues — the pending flip
            // is still the thing being waited for.
        }
    }

    /// Apply a batch of landed device events: flips feed the owning
    /// output's ledger (and the *primary's* flips feed the scheduler —
    /// the pacing grid), every release whose gates have all passed
    /// fires. Shared by the session pump (the driving client's flips)
    /// and the serve loop's own device service (flips orphaned by a
    /// finished session, hotplug).
    ///
    /// [`FrameError`]: the release-fence minting arm — fatal for the
    /// calling session.
    fn land_events(&mut self, events: Vec<DeviceEvent>) -> Result<Vec<OutboxEntry>, FrameError> {
        let mut entries = Vec::new();
        let mut landed = false;
        let events_were_empty = events.is_empty();
        let primary_crtc = self.outputs.first().map(|slot| slot.crtc);
        for event in events {
            if let DeviceEvent::PageFlip(flip) = event {
                if let Some(slot) = self.outputs.iter_mut().find(|s| s.crtc == flip.crtc) {
                    slot.scanout.flip_pending = false;
                    slot.flips += 1;
                    self.scene.flips.insert(slot.crtc, slot.flips);
                    // The primary's flips anchor the pacing grid (the
                    // scheduler's PLL). Sibling outputs keep their own
                    // ledgers — per-output presentation clocks are the
                    // follow-up roadmap line.
                    if Some(slot.crtc) == primary_crtc {
                        self.scene.scheduler.observe_flip(flip.timestamp);
                        // The latency rig's closing arm (Phase 31):
                        // the primary's flip that carries an injected
                        // input's frame closes the input→photon
                        // measurement.
                        self.measure_input_photon(flip.timestamp);
                        // The governor's load signal (Phase 35): one
                        // more landed flip on the pacing grid — the
                        // window counter the power tick feeds the
                        // clock ladder.
                        self.governor_flips += 1;
                    }
                    landed = true;
                }
            }
        }
        if !landed && events_were_empty {
            for slot in &mut self.outputs {
                if slot.scanout.flip_pending {
                    // A wait that returned nothing while a flip was pending
                    // cannot happen on a live pipeline (the driver's contract);
                    // treat the flight as landed to avoid a wedge — the honest
                    // degradation.
                    slot.scanout.flip_pending = false;
                }
            }
        }
        // Buffers the render path no longer reads: their fences land
        // when every output that was scanning them out has flipped
        // past them (releases are flip-gated by construction).
        let pending = core::mem::take(&mut self.scene.pending_releases);
        let (due, keep): (Vec<PendingRelease>, Vec<PendingRelease>) = pending
            .into_iter()
            .partition(|r| self.release_gates_passed(r));
        self.scene.pending_releases = keep;
        entries.extend(Self::due_releases(due)?);
        let sched = self.scene.scheduler.drain();
        entries.extend(sched_entries(&self.scene.routes, sched));
        Ok(entries)
    }

    /// Whether every release gate of `r` has passed: each gate names
    /// an output's CRTC and a flip count on it; a CRTC that no longer
    /// exists satisfies its gate (that output stopped reading the
    /// buffer when its pipeline died — the Phase 26 dark-flush rule,
    /// applied per output).
    fn release_gates_passed(&self, r: &PendingRelease) -> bool {
        r.after_flips.iter().all(|(crtc, after)| {
            self.outputs
                .iter()
                .find(|slot| slot.crtc == *crtc)
                .map_or(true, |slot| slot.flips >= *after)
        })
    }

    /// Service device events outside any session's pump — the serve
    /// loop's own wake: drain what is already due (never blocking,
    /// never advancing the mock's clock speculatively), land flips
    /// orphaned by finished sessions, and — when a hotplug event rode
    /// along — run the live re-arrangement (one signal, every
    /// consequence: the whole topology is re-probed and the served
    /// pipelines follow). Returns the re-arrangement's summary when
    /// one ran. Events destined for clients park in their outboxes
    /// for the next wake.
    ///
    /// # Errors
    ///
    /// [`FrameError`] — fatal for the caller (the serve loop exits
    /// honestly rather than degrade silently).
    pub fn service_device_events(&mut self) -> Result<Option<Rearrange>, FrameError> {
        let events = self.device.wait_events(Some(core::time::Duration::ZERO))?;
        self.device_events += u64::try_from(events.len()).unwrap_or(0);
        let hotplug = events.iter().any(|e| matches!(e, DeviceEvent::Hotplug(_)));
        let entries = self.land_events(events)?;
        self.outboxes.extend(entries);
        if !hotplug {
            return Ok(None);
        }
        Ok(Some(self.rearrange()?))
    }

    /// Teardown: the applied disable commit for *every* served
    /// pipeline — planes released, CRTCs off, connectors unbound —
    /// leaving the displays dark and the framebuffers releasable.
    /// Works over both drivers; the mock's state assertions pin it in
    /// CI, the real path runs it on exit. A dark world has nothing to
    /// disable (the re-arrangement already did), so this is a no-op
    /// there.
    ///
    /// # Errors
    ///
    /// [`FrameError`] when the device rejects a disable (fatal at
    /// teardown — reported, not swallowed).
    pub fn teardown(&mut self) -> Result<(), FrameError> {
        let pipelines: Vec<serve::Pipeline> = self
            .outputs
            .iter()
            .map(|slot| serve::Pipeline {
                connector: slot.output.connector.id,
                crtc: slot.crtc,
                plane: slot.plane,
                mode: slot.output.mode().clone(),
            })
            .collect();
        for pipeline in &pipelines {
            serve::disable(self.device.as_mut(), pipeline)?;
        }
        Ok(())
    }

    /// Take every output's DRM lease (the teardown path's
    /// object-release vocabulary: dumb buffers destroyed, master
    /// dropped — one master per device, released once).
    pub fn take_leases(&mut self) -> Vec<crate::scene::DrmLease> {
        self.outputs
            .iter_mut()
            .filter_map(|slot| slot.lease.take())
            .collect()
    }

    /// The compositor's current time — the driver's (the mock's
    /// injected clock; the kernel's CLOCK_MONOTONIC on real
    /// hardware).
    #[must_use]
    pub fn now(&self) -> Mono {
        self.device.now()
    }

    /// The primary output's visible content as premultiplied ARGB
    /// words — what the panel is *showing*. Phase 34: the display
    /// model (the canvas with every plane-carried layer blended in
    /// zpos order) when the frame loop has produced one; the scanout
    /// chain's front buffer before that (the pre-Phase-34 oracle).
    /// `None` is the honest dark state (there is no front buffer to
    /// read).
    #[must_use]
    pub fn scanout_words(&self) -> Option<Vec<u32>> {
        self.outputs.first().map(slot_display_words)
    }

    /// One output's visible content by slot index (the multi-output
    /// oracle).
    #[must_use]
    pub fn slot_scanout_words(&self, slot_index: usize) -> Option<Vec<u32>> {
        self.outputs.get(slot_index).map(slot_display_words)
    }
}

/// Whether a snapshot node shows the material through: its opaque
/// region does not cover its bounds (the frost tier's predicate —
/// docks, control panels, translucent toplevels).
fn surface_is_translucent(node: &ldp_compositor::snapshot::SnapshotNode) -> bool {
    let opaque_here = node.opaque_region.clipped_to(node.bounds);
    let holes = Region::from_rect(node.bounds).subtract(&opaque_here);
    !holes.is_empty()
}

/// The intersection of two rectangles (empty when disjoint or
/// edge-touching — right/bottom exclusive, the damage convention).
fn rect_intersect(a: Rect, b: Rect) -> Rect {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = a.right().min(b.right());
    let y1 = a.bottom().min(b.bottom());
    if x1 > x0 && y1 > y0 {
        Rect::new(x0, y0, (x1 - x0) as u32, (y1 - y0) as u32)
    } else {
        Rect::EMPTY
    }
}

/// The Liquid style one surface wears at `tier` — the material
/// family's policy (Phase 40), one resolution shared by the damage
/// expansion and the layer assembly: the desktop's own fullscreen
/// opaque surface goes undressed (it is the material everything else
/// floats on); a popup wears the **menu glass** — the vibrant frost
/// plus the luminous hairline, "backdrop blur everywhere" the giants
/// mean it; a translucent surface wears the sheet; an opaque window
/// the panel.
///
/// Phase 45, the per-surface material request (the NSVisualEffectView
/// doctrine): `requested` overrides the resolution's own choice —
/// the client's *identity* claim for its window's chrome — except
/// the two server-owned invariants: the fullscreen-opaque plain rule
/// (a fullscreen window is the material others float on; its own
/// chrome is invisible by construction) and the popup role (the menu
/// glass is what a popup *is*). `Minimal` keeps resolving every
/// material plain — the tier is the server's quality budget, not the
/// client's to override.
///
/// The popup arm's shadow parameters equal the sheet's at every tier
/// (pinned in `style.rs`), so a damage path that resolves without
/// popup knowledge still computes identical effect rects — the
/// constraint that keeps the two call sites' repaint spreads honest
/// even if they ever diverge in what they know.
pub(crate) fn surface_style(
    tier: ldp_renderer::EffectTier,
    node: &ldp_compositor::snapshot::SnapshotNode,
    output_bounds: Rect,
    popup: bool,
    requested: Option<ldp_renderer::Material>,
) -> ldp_renderer::LayerStyle {
    let translucent = surface_is_translucent(node);
    if node.bounds == output_bounds && !translucent {
        ldp_renderer::LayerStyle::default()
    } else {
        let material = if popup {
            ldp_renderer::Material::Menu
        } else {
            requested.unwrap_or(if translucent {
                ldp_renderer::Material::Sheet
            } else {
                ldp_renderer::Material::Panel
            })
        };
        material.style(tier)
    }
}

/// Map scheduler emissions to outbox entries through the routes.
/// Phase 39: the parked entries carry the §10.4 presentation class —
/// `frame_target` and `presented` coalesce to the latest per surface
/// and kind under backlog (a slow-draining client reads the freshest
/// feedback, not a queue of stale verdicts); `frame_dropped` is a
/// per-frame terminal signal and never replaces (the class doctrine
/// pins it).
fn sched_entries(routes: &HashMap<SurfaceId, Route>, events: Vec<SchedEvent>) -> Vec<OutboxEntry> {
    let mut out = Vec::new();
    for ev in events {
        let (surface, event, args, kind): (SurfaceId, &'static str, Vec<Value>, Option<u8>) =
            match ev {
                SchedEvent::FrameTarget {
                    surface,
                    frame,
                    deadline,
                } => (
                    surface,
                    "frame_target",
                    vec![
                        Value::Uint64(frame),
                        Value::Ts(deadline.deadline.as_ns()),
                        Value::Uint64(deadline.refresh.as_ns()),
                        Value::Uint64(deadline.budget_ns),
                        Value::Enum(deadline.mode.to_wire()),
                    ],
                    Some(crate::outbox::PRESENTATION_TARGET),
                ),
                SchedEvent::Presented { surface, timing } => (
                    surface,
                    "presented",
                    vec![
                        Value::Uint64(timing.frame),
                        Value::Ts(timing.presented_at.as_ns()),
                        Value::Uint64(timing.refresh.as_ns()),
                        Value::Bitset(low_bits(timing.flags.to_wire())),
                    ],
                    Some(crate::outbox::PRESENTATION_PRESENTED),
                ),
                SchedEvent::FrameDropped {
                    surface,
                    frame,
                    reason,
                } => (
                    surface,
                    "frame_dropped",
                    vec![Value::Uint64(frame), Value::Enum(reason.to_wire())],
                    None,
                ),
            };
        if let Some(route) = routes.get(&surface) {
            let entry = match kind {
                Some(tag) => {
                    OutboxEntry::presentation(route.client, route.surface_obj, event, args, tag)
                }
                None => OutboxEntry::event(route.client, route.surface_obj, event, args),
            };
            out.push(entry);
        }
    }
    out
}

/// The grading walk's borrow bundle (Phase 34): the scene feeds the
/// stack (the returned layers borrow it — an *immutable* field borrow,
/// so the render pass keeps its mutable device/output borrows), the
/// device and the import ledger mutate — disjoint fields of the
/// world, one context.
struct GradeContext<'a, 'b> {
    /// The protocol scene (surfaces, routes, pools).
    scene: &'a crate::scene::Scene,
    /// The frame's snapshot (taken before the walk — the tree's
    /// back-to-front order and the frozen node facts).
    snapshot: &'a ldp_compositor::snapshot::FrameSnapshot,
    /// The positioning shell (the system dock).
    shell: &'a crate::shell::Shell,
    /// The resolved Liquid tier.
    effects: ldp_renderer::EffectTier,
    /// The negotiated luminance ceiling (Phase 38), in nits: `Some`
    /// only when this frame composites onto the PQ canvas over an HDR
    /// panel — the value every HDR layer's tone policy carries.
    negotiated: Option<u32>,
    /// The popup-role host (Phase 40's material routing: a popup
    /// surface wears the menu glass — the vibrant frost and the
    /// luminous hairline).
    popups: &'a crate::shell::PopupHost,
    /// The driven device (the import walk's ioctls) — the shorter
    /// borrow: it ends when the walk returns, so the render pass
    /// keeps its mutable device access afterward.
    device: &'b mut dyn ldp_display::driver::DisplayDriver,
    /// The buffer-import ledger (the same shorter borrow).
    imports: &'b mut HashMap<crate::scene::ObjectKey, Option<ldp_display::ids::FbId>>,
}

/// The per-layer luminance tail (Phase 38): an HDR layer on the
/// negotiated PQ canvas carries its tone policy — the mastering
/// refinement when the surface declared static metadata (the declared
/// bounds meet the negotiated ceiling in the BT.2390-structured knee),
/// the honest clip otherwise. SDR layers ride the BT.2408 anchor
/// (`Pass`), and an SDR canvas (`None` — no negotiation) keeps every
/// pre-Phase-38 byte.
fn layer_tone_policy(
    node: &ldp_compositor::snapshot::SnapshotNode,
    negotiated: Option<u32>,
) -> ldp_renderer::TonePolicy {
    let Some(ceiling) = negotiated else {
        return ldp_renderer::TonePolicy::Pass;
    };
    if !node.color.is_hdr() {
        return ldp_renderer::TonePolicy::Pass;
    }
    match &node.hdr {
        Some(meta) => {
            let (lo, hi) = ldp_hdr::layer_mastering(&node.color, Some(meta));
            ldp_renderer::TonePolicy::Eetf {
                master_min_nits: lo.as_nits() as f32,
                master_max_nits: hi.as_nits() as f32,
                ceiling_nits: ceiling as f32,
            }
        }
        None => ldp_renderer::TonePolicy::Clip {
            ceiling_nits: ceiling as f32,
        },
    }
}

/// The ghost's own tone policy (Phase 48): the same anchor-pull
/// doctrine over the fading window's frozen color truth — the
/// `layer_tone_policy` mirror for a layer whose node is gone.
fn ghost_tone_policy(
    ghost: &crate::scene::Ghost,
    negotiated: Option<u32>,
) -> ldp_renderer::TonePolicy {
    let Some(ceiling) = negotiated else {
        return ldp_renderer::TonePolicy::Pass;
    };
    if !ghost.color.is_hdr() {
        return ldp_renderer::TonePolicy::Pass;
    }
    match &ghost.hdr {
        Some(meta) => {
            let (lo, hi) = ldp_hdr::layer_mastering(&ghost.color, Some(meta));
            ldp_renderer::TonePolicy::Eetf {
                master_min_nits: lo.as_nits() as f32,
                master_max_nits: hi.as_nits() as f32,
                ceiling_nits: ceiling as f32,
            }
        }
        None => ldp_renderer::TonePolicy::Clip {
            ceiling_nits: ceiling as f32,
        },
    }
}

/// One ghost's fact and layer (Phase 48): the fading window's own
/// geometry, style, and color — drawn exactly as the last frame drew
/// them, with the close spring's opacity the only change. The fact is
/// the dock's own shape (owned ink, no framebuffer — the composite
/// path only, never a plane offload) and the layer borrows the
/// ghost's owned ink, exactly the dock's pattern.
fn ghost_layer(
    ghost: &crate::scene::Ghost,
    layout: (i32, i32),
    negotiated: Option<u32>,
) -> (LayerFacts, Option<SurfaceLayer<'_>>) {
    let local_dest = ghost.dest.translate(-layout.0, -layout.1);
    let opacity = ghost.transition.opacity();
    let fact = LayerFacts {
        dest: local_dest,
        width: ghost.geometry.width(),
        height: ghost.geometry.height(),
        format: ghost.geometry.format(),
        modifier: ghost.geometry.modifier(),
        transform: ghost.transform,
        color: ghost.color,
        opacity,
        // A fading layer is never opaque — the demotion ledger's
        // honest subtraction.
        opaque: Region::new(),
        // Owned ink: no framebuffer to offload — the ghost always
        // composites (the dock's own NoFb doctrine).
        fb: None,
        needs_backdrop: ghost.style.backdrop.is_some(),
        styled: !ghost.style.is_plain(),
        // The compositor's own layer (the solver's system class —
        // never a client's).
        system: true,
    };
    let layer = BufferView::new(ghost.id, &ghost.ink, ghost.geometry.clone())
        .ok()
        .map(|view| {
            let mut layer = SurfaceLayer::new(
                view,
                local_dest,
                ghost.transform,
                ghost.color,
                opacity,
                Region::new(),
            );
            layer.style = ghost.style;
            layer.tone = ghost_tone_policy(ghost, negotiated);
            layer
        });
    (fact, layer)
}

/// One output's visible words: the display model when the frame loop
/// maintains it, the scanout chain's front otherwise (the
/// pre-Phase-34 oracle).
fn slot_display_words(slot: &crate::scene::OutputSlot) -> Vec<u32> {
    if slot.display.is_empty() {
        slot.scanout.scanout_words()
    } else {
        slot.display.clone()
    }
}
