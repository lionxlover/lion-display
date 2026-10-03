//! The compositor scene: protocol identity meets the surface tree.
//!
//! [`Scene`] shadows every protocol object the compositor serves with
//! its real resource — a surface object with a [`SurfaceId`] in the
//! [`SurfaceTree`], a pool object with its
//! mapping, a buffer object with its window into a pool — and owns the
//! per-output [`FrameScheduler`]. It is the *only* mutable state the
//! dispatcher family touches; together with the device, renderer, and
//! scanout chain it forms the [`World`], one mutex guarding the whole
//! compositor (the v-slice's single-pipeline doctrine: render and
//! dispatch serialize; finer pipelining is later work).
//!
//! Identity rule: object IDs are per-client namespaces, so every key is
//! the pair [`ObjectKey`] (client u32, object u32). A generation-aware
//! `ObjectId` on the wire resolves through the session's store before
//! the dispatcher ever sees it, so the scene keys on the wire pair and
//! trusts the session's liveness discipline.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ldp_compositor::scheduler::{FrameScheduler, SchedulerConfig};
use ldp_compositor::semantics::{SceneProfile, SecurityClass, SemanticRole, Semantics};
use ldp_compositor::snapshot::FrameSnapshot;
use ldp_compositor::state::BufferAttachment;
use ldp_compositor::surface::{Surface, SurfaceId};
use ldp_compositor::transitions::{Transition, TransitionKind};
use ldp_compositor::tree::{FrameChanges, SurfaceTree};
use ldp_core::buffer::{BufferGeometry, PlaneLayout};
use ldp_core::color::{ColorDescription, HdrMetadata};
use ldp_core::geometry::{Rect, Transform};
use ldp_core::ids::{ClientId, ObjectId};
use ldp_core::scale::ScaleFactor;
use ldp_core::time::Mono;

use crate::outbox::Outboxes;
use crate::output::OutputGlobal;
use crate::shm::{ShmBuffer, ShmPool};
use ldp_display::driver::DisplayDriver;
use ldp_display::drm::sys::DumbMapping;
use ldp_display::ids::{CrtcId, FbId, PlaneId};
use ldp_display::serve::{read_frame_rows, write_frame_rows};

/// The per-client object key (a wire pair).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ObjectKey {
    /// The owning client.
    pub client: u32,
    /// The object's wire id.
    pub object: u32,
}

impl ObjectKey {
    /// Build from the wire pair.
    #[must_use]
    pub fn new(client: ClientId, object: ObjectId) -> ObjectKey {
        ObjectKey {
            client: client.as_u32(),
            object: object.as_u32(),
        }
    }
}

/// A surface's routing: who owns it, through which objects, with which
/// committed buffer.
#[derive(Debug)]
pub struct Route {
    /// The owning client.
    pub client: ClientId,
    /// The `ldp.core.surface` object.
    pub surface_obj: ObjectId,
    /// The `ldp.core.subsurface` role object, for subsurfaces.
    pub role_obj: Option<ObjectId>,
    /// The `ldp.shell.toplevel` role object (Phase 45): the
    /// `set_material` request's address, mapped back to this surface
    /// through `by_toplevel_obj`.
    pub toplevel_obj: Option<ObjectId>,
    /// The committed buffer (object + resource), when mapped.
    pub buffer: Option<(ObjectId, Arc<ShmBuffer>)>,
    /// The outputs whose `enter_output` has been sent (and not left) —
    /// Phase 31's per-output visibility: a surface spanning two
    /// monitors is entered on both.
    pub entered: Vec<CrtcId>,
    /// The outputs whose last rendered frame showed this surface —
    /// the release-gate set for its superseded buffers (a buffer the
    /// compositor stops reading is released once every output that
    /// was scanning it out has flipped past it).
    pub visible_on: Vec<CrtcId>,
    /// Phase 45, the occlusion quiescing state: whether the last
    /// damage pass resolved this surface onto **zero** outputs (fully
    /// occluded, unmapped, or unplaced) — the scheduler's hidden flag
    /// mirrors this on transitions (the pass is the only authority;
    /// the initial `false` is the pre-pass state, so a surface's first
    /// visibility resolution is what arms it).
    pub quiesced: bool,
    /// Whether the positioning shell has placed this root (Phase 28:
    /// placement rides the first attach; a re-attach before commit
    /// must not re-place).
    pub placed: bool,
}

/// A buffer awaiting its release fence after the compositor's last read.
#[derive(Debug)]
pub struct PendingRelease {
    /// The owning client.
    pub client: ClientId,
    /// The `ldp.core.buffer` object to signal.
    pub buffer_obj: ObjectId,
    /// The resource (keeps the pool window alive until released).
    pub buffer: Arc<ShmBuffer>,
    /// The release gates: one per output that was scanning the buffer
    /// out — `(crtc, after_flips)`. Every gate must pass (that output
    /// flipped at least `after_flips` times) before the fence fires;
    /// a gate whose CRTC no longer exists is satisfied by construction
    /// (that output stopped reading the buffer when its pipeline died).
    /// A single-output world carries exactly one gate — the Phase 25
    /// semantics, byte-identical.
    pub after_flips: Vec<(CrtcId, u64)>,
}

/// The protocol-facing scene state.
pub struct Scene {
    /// The authoring tree.
    pub tree: SurfaceTree,
    next_surface: u64,
    /// SurfaceId → routing.
    pub routes: HashMap<SurfaceId, Route>,
    /// (client, surface object) → SurfaceId.
    pub by_surface_obj: HashMap<ObjectKey, SurfaceId>,
    /// (client, subsurface role object) → SurfaceId.
    pub by_role_obj: HashMap<ObjectKey, SurfaceId>,
    /// Pool objects → resources (owned; buffers reference by key).
    pub pools: HashMap<ObjectKey, ShmPool>,
    /// Buffer objects → resources.
    pub buffers: HashMap<ObjectKey, Arc<ShmBuffer>>,
    /// The pending attach per surface: `Some` = attach at next commit,
    /// `None` = detach. Absent = no attach request since the last commit.
    pub pending_attach: HashMap<SurfaceId, Option<(ObjectId, Arc<ShmBuffer>)>>,
    /// The per-output deadline scheduler (Phase 31: the pacing grid is
    /// the *primary* output's — slot 0 of the world's outputs; a
    /// per-surface home-output assignment is the follow-up roadmap
    /// line).
    pub scheduler: FrameScheduler,
    /// Commits landed since the last render (the render trigger).
    pub dirty: bool,
    /// Buffers whose release fence is waiting on a flip.
    pub pending_releases: Vec<PendingRelease>,
    /// Completed flips per CRTC — the mirror the release gates read
    /// when a commit queues them (the world keeps it in lockstep with
    /// the outputs' own counters).
    pub flips: HashMap<CrtcId, u64>,
    /// Per-client bound output objects, keyed `(client, output CRTC)` —
    /// one object per (client, output) pair: a client that wants every
    /// display binds `ldp.core.output` once per output and each bind
    /// mirrors the next one (the dispatcher's round-robin).
    pub output_binds: HashMap<(u32, CrtcId), ObjectId>,
    /// Per-client output-bind round-robin cursor (which output the
    /// client's *next* output bind mirrors).
    pub output_bind_seq: HashMap<u32, u64>,
    /// (client, toplevel object) → SurfaceId (Phase 45: the
    /// `set_material` request arrives on the toplevel object; this is
    /// its way back to the surface whose chrome it names).
    pub by_toplevel_obj: HashMap<ObjectKey, SurfaceId>,
    /// The per-surface material requests (Phase 45, the
    /// NSVisualEffectView doctrine): a surface whose client claimed a
    /// Liquid material wears it at the next style resolution — the
    /// two call sites (damage spread + layer assembly) consult this
    /// one map, so the two truths cannot diverge.
    pub material_requests: HashMap<SurfaceId, ldp_renderer::Material>,
    /// The per-surface semantic claims (Phase 47, the semantic
    /// scene): the role, security class, and scene profile a surface's
    /// client claimed — the `material_requests` pattern (one side-map,
    /// dying with the surface). The claims' enforcement seams live
    /// where they have teeth: the capture path (security classes),
    /// the scheduler (scene profiles), and the transitions host
    /// (roles).
    pub semantics: HashMap<SurfaceId, Semantics>,
    /// The compositor-owned transitions host (Phase 47): the live
    /// per-surface motions, advanced at the pump's cadence (the dock's
    /// own doctrine — motion is a function of the driver clock, never
    /// timer-driven). Off by default (the byte-exactness doctrine);
    /// the serve loop's poll cadence wakes a desktop whose clients all
    /// sleep.
    pub transitions: TransitionHost,
    /// The close-fade ghosts (Phase 48): the owned snapshots of dying
    /// windows, fading out under the transitions catalog's close
    /// spring. The host is the render path's — the ghost is a layer
    /// the compositor owns (the dock's own pattern: owned ink, a
    /// borrowed view), so it lives in the scene the grade walk
    /// borrows. Empty by default and with transitions off (the same
    /// byte-exactness doctrine).
    pub ghosts: GhostHost,
    /// The states arm's hidden set (Phase 49): surfaces the desktop
    /// does not render — minimized windows and windows on a
    /// non-active space. Membership is the *render and input*
    /// truth (the grade walk skips these routes, the router never
    /// finds them); the visibility pass clears their output set
    /// (the enter/leave and quiesce truth — App Nap rides the same
    /// seam the occlusion path owns).
    pub hidden: std::collections::HashSet<SurfaceId>,
    /// The rects the last hide/unhide touched (Phase 49): the vacate
    /// doctrine's fourth member — a hidden window is nobody's layer
    /// (no client frame economy re-renders its region), so the hide
    /// itself claims the ink it removes and the unhide claims the
    /// ink it restores. Drained by the damage pass.
    pub hide_claims: Vec<Rect>,
    /// The drawn chrome's ledger (Phase 52): each serving band's
    /// last-claimed frame rect. The damage pass diffs it against the
    /// live truth — a band that moved, resized, hid, or died claims
    /// its old rect, a band that came back or changed claims its new
    /// one (the R2 rule's chrome sibling; the damage engine knows
    /// only protocol damage, the band is the compositor's own ink
    /// around it).
    pub chrome_prev: HashMap<SurfaceId, Rect>,
    /// Phase 54 — the title ledger: the last-claimed strip rect and
    /// title per serving window (the ink-change claims — a title the
    /// band's own rect never sees).
    pub title_prev: HashMap<SurfaceId, (Rect, Box<str>)>,
}

impl Scene {
    /// A scene over one output's scheduler.
    ///
    /// # Errors
    /// [`ldp_compositor::scheduler::ConfigError`] for a malformed
    /// scheduler config.
    pub fn new(
        nominal: ldp_core::time::RefreshInterval,
        config: SchedulerConfig,
    ) -> Result<Scene, ldp_compositor::scheduler::ConfigError> {
        Ok(Scene {
            tree: SurfaceTree::new(),
            next_surface: 0,
            routes: HashMap::new(),
            by_surface_obj: HashMap::new(),
            by_role_obj: HashMap::new(),
            pools: HashMap::new(),
            buffers: HashMap::new(),
            pending_attach: HashMap::new(),
            scheduler: FrameScheduler::new(nominal, config)?,
            by_toplevel_obj: HashMap::new(),
            material_requests: HashMap::new(),
            semantics: HashMap::new(),
            transitions: TransitionHost::default(),
            ghosts: GhostHost::default(),
            hidden: std::collections::HashSet::new(),
            hide_claims: Vec::new(),
            chrome_prev: HashMap::new(),
            title_prev: HashMap::new(),
            dirty: false,
            pending_releases: Vec::new(),
            flips: HashMap::new(),
            output_binds: HashMap::new(),
            output_bind_seq: HashMap::new(),
        })
    }

    /// Mint the next SurfaceId.
    fn mint(&mut self) -> SurfaceId {
        self.next_surface += 1;
        SurfaceId::from_raw(self.next_surface)
    }

    /// `compositor.create_surface`: a root at the layout origin (the
    /// shell that positions toplevels arrives in Phase 12).
    ///
    /// # Panics
    ///
    /// Panics if the world lock is poisoned (a session thread died
    /// mid-operation) — startup-fatal by policy.
    pub fn create_surface(&mut self, client: ClientId, surface_obj: ObjectId) -> SurfaceId {
        let id = self.mint();
        self.tree.create_root(id, 0, 0).expect("fresh SurfaceId");
        self.by_surface_obj
            .insert(ObjectKey::new(client, surface_obj), id);
        self.routes.insert(
            id,
            Route {
                client,
                surface_obj,
                role_obj: None,
                toplevel_obj: None,
                buffer: None,
                entered: Vec::new(),
                visible_on: Vec::new(),
                quiesced: false,
                placed: false,
            },
        );
        id
    }

    /// `compositor.create_subsurface(surface, parent, role)`: an
    /// unmapped, never-committed root converts to the subsurface role —
    /// the tree's create inserts fresh nodes, so the conversion removes
    /// the pristine root and re-inserts it under the parent (identical
    /// semantics: no state existed to lose).
    ///
    /// # Errors
    /// `None` when either object is not a known surface, the surface is
    /// mapped (role changes on live content), or the tree rejects the
    /// nesting.
    pub fn create_subsurface(
        &mut self,
        client: ClientId,
        surface_obj: ObjectId,
        parent_obj: ObjectId,
        role_obj: ObjectId,
    ) -> Option<SurfaceId> {
        let surface = *self
            .by_surface_obj
            .get(&ObjectKey::new(client, surface_obj))?;
        let parent = *self
            .by_surface_obj
            .get(&ObjectKey::new(client, parent_obj))?;
        // Only pristine (unmapped) roots convert: a mapped surface has
        // live content a role change would silently drop.
        let pristine = self
            .tree
            .get(surface)
            .is_some_and(|s| !s.state().is_mapped());
        if !pristine {
            return None;
        }
        self.tree.destroy(surface).ok()?;
        self.tree.create_subsurface(surface, parent).ok()?;
        self.by_role_obj
            .insert(ObjectKey::new(client, role_obj), surface);
        self.routes.get_mut(&surface)?.role_obj = Some(role_obj);
        Some(surface)
    }

    /// Resolve a surface object to its SurfaceId.
    #[must_use]
    pub fn surface_of(&self, client: ClientId, object: ObjectId) -> Option<SurfaceId> {
        self.by_surface_obj
            .get(&ObjectKey::new(client, object))
            .copied()
    }

    /// Resolve a subsurface role object to its SurfaceId.
    #[must_use]
    pub fn subsurface_of(&self, client: ClientId, object: ObjectId) -> Option<SurfaceId> {
        self.by_role_obj
            .get(&ObjectKey::new(client, object))
            .copied()
    }

    /// Register the toplevel role object for a surface (Phase 45: the
    /// `set_material` request's address). Idempotent per surface — the
    /// shell mints one toplevel per `get_toplevel`, and a re-mint
    /// after a role release simply replaces the mapping.
    pub fn register_toplevel(
        &mut self,
        client: ClientId,
        surface_obj: ObjectId,
        toplevel_obj: ObjectId,
    ) -> Option<SurfaceId> {
        let surface = *self
            .by_surface_obj
            .get(&ObjectKey::new(client, surface_obj))?;
        self.by_toplevel_obj
            .insert(ObjectKey::new(client, toplevel_obj), surface);
        self.routes.get_mut(&surface)?.toplevel_obj = Some(toplevel_obj);
        Some(surface)
    }

    /// Resolve a toplevel role object to its SurfaceId.
    #[must_use]
    pub fn toplevel_of(&self, client: ClientId, object: ObjectId) -> Option<SurfaceId> {
        self.by_toplevel_obj
            .get(&ObjectKey::new(client, object))
            .copied()
    }

    /// Release the toplevel role object's mapping (Phase 49: the
    /// object's destroy — the surface may outlive the role, roleless;
    /// a re-mint replaces both mappings). The route's role slot
    /// clears only when it still names this object (a re-mint over
    /// the same surface owns it now).
    pub fn unregister_toplevel(
        &mut self,
        client: ClientId,
        toplevel_obj: ObjectId,
        surface: SurfaceId,
    ) {
        self.by_toplevel_obj
            .remove(&ObjectKey::new(client, toplevel_obj));
        if let Some(route) = self.routes.get_mut(&surface) {
            if route.toplevel_obj == Some(toplevel_obj) {
                route.toplevel_obj = None;
            }
        }
    }

    /// Apply a per-surface material request (Phase 45, the
    /// NSVisualEffectView doctrine): `None` clears the override — the
    /// server's own resolution rules dress the surface again. The
    /// scene dirties: a changed material is a changed chrome (the
    /// Liquid spread recomputes its effect rects), so the repaint is
    /// the server's, exactly as an unmap's is.
    pub fn set_material(&mut self, surface: SurfaceId, material: Option<ldp_renderer::Material>) {
        match material {
            Some(m) => {
                self.material_requests.insert(surface, m);
            }
            None => {
                self.material_requests.remove(&surface);
            }
        }
        self.dirty = true;
    }

    /// Apply a semantic-role claim (Phase 47, `set_semantic_role`):
    /// `None` clears (the plain window doctrine). The role's teeth are
    /// the transitions catalog's eligibility (a re-claimed surface's
    /// *next* mapping animates under the new role) and the security
    /// floor at capture time — no immediate pixel change, so the
    /// scene does not dirty (the claim's effects are policy, not
    /// chrome).
    pub fn set_semantic_role(&mut self, surface: SurfaceId, role: Option<SemanticRole>) {
        let entry = self.semantics.entry(surface).or_default();
        entry.role = role;
        if *entry == Semantics::default() {
            self.semantics.remove(&surface);
        }
    }

    /// Apply a security-class claim (Phase 47, `set_security_class`):
    /// `None` clears (the server treats the surface as `normal`). The
    /// class takes effect at the next capture — no pixel change on
    /// the display (the user keeps seeing their window; the
    /// *screenshot* is what changes).
    pub fn set_security_class(&mut self, surface: SurfaceId, class: Option<SecurityClass>) {
        let entry = self.semantics.entry(surface).or_default();
        entry.security = class.unwrap_or(SecurityClass::Normal);
        if *entry == Semantics::default() {
            self.semantics.remove(&surface);
        }
    }

    /// Apply a scene-profile claim (Phase 47, `set_scene_profile`)
    /// and feed the scheduler the same truth under one lock: the
    /// semantics map is the wire claim (introspection), the slot's
    /// profile is the enforcement (the budget floor). Two truths
    /// updated in one breath cannot diverge.
    pub fn set_scene_profile(&mut self, surface: SurfaceId, profile: SceneProfile, now: Mono) {
        let entry = self.semantics.entry(surface).or_default();
        entry.profile = profile;
        if *entry == Semantics::default() {
            self.semantics.remove(&surface);
        }
        self.scheduler.set_profile(surface, profile, now);
    }

    /// The surface's semantic claims (defaults when unclaimed).
    #[must_use]
    pub fn semantics_of(&self, surface: SurfaceId) -> Semantics {
        self.semantics.get(&surface).copied().unwrap_or_default()
    }

    /// Apply a commit: splice the pending attach into the tree, run the
    /// atomic cascade, and record buffer swaps for release.
    ///
    /// Returns the surface that committed (sync-mode children may have
    /// applied too — the tree records that in `take_changes`).
    ///
    /// # Errors
    /// `None` when the surface is unknown (a session-store bug: the
    /// object resolved but the scene lost it).
    pub fn commit(
        &mut self,
        client: ClientId,
        surface_obj: ObjectId,
        now: Mono,
    ) -> Option<SurfaceId> {
        let id = self.surface_of(client, surface_obj)?;
        // The gate set for a superseded buffer: the outputs whose last
        // render showed this surface (their front buffers may still be
        // scanning the old content out).
        let visible: Vec<CrtcId> = self
            .routes
            .get(&id)
            .map(|r| r.visible_on.clone())
            .unwrap_or_default();
        // Splice the pending attach into the tree's pending state.
        if let Some(pending) = self.pending_attach.remove(&id) {
            let attachment = pending.as_ref().map(|(_, buf)| BufferAttachment {
                width: buf.width(),
                height: buf.height(),
                generation: buf.identity(),
            });
            self.tree.pending_mut(id).ok()?.attach(attachment);
            let (owner, swap) = if let Some((obj, buf)) = pending {
                let route = self.routes.get_mut(&id)?;
                (route.client, route.buffer.replace((obj, buf)))
            } else {
                let route = self.routes.get_mut(&id)?;
                (route.client, route.buffer.take())
            };
            if let Some((prev_obj, prev)) = swap {
                self.release_buffer(owner, prev_obj, prev, &visible);
            }
        }
        self.tree.commit(id).ok()?;
        self.scheduler.commit(id, now);
        self.dirty = true;
        Some(id)
    }

    /// Queue a release for a buffer the render path no longer reads:
    /// the fence lands after every output that was scanning it out
    /// has flipped past it — one gate per such output's CRTC.
    ///
    /// `visible` is the set of outputs whose last rendered frame showed
    /// the surface (empty for a surface never rendered: the buffer was
    /// never read anywhere, so the conservative gate is every live
    /// output's next flip — the Phase 25 semantics verbatim in a
    /// single-output world).
    pub fn release_buffer(
        &mut self,
        client: ClientId,
        buffer_obj: ObjectId,
        buffer: Arc<ShmBuffer>,
        visible: &[CrtcId],
    ) {
        let after_flips: Vec<(CrtcId, u64)> = if visible.is_empty() {
            self.flips.iter().map(|(c, f)| (*c, *f + 1)).collect()
        } else {
            visible
                .iter()
                .filter_map(|c| self.flips.get(c).map(|f| (*c, *f + 1)))
                .collect()
        };
        self.pending_releases.push(PendingRelease {
            client,
            buffer_obj,
            buffer,
            after_flips,
        });
    }

    /// Destroy a surface (and its subtree): routes go, committed
    /// buffers queue for release, and the tree records the removal.
    pub fn destroy_surface(&mut self, id: SurfaceId) {
        if !self.routes.contains_key(&id) {
            return;
        }
        // Collect the whole subtree parent-before-child.
        let mut subtree = Vec::new();
        let mut stack = vec![id];
        while let Some(cur) = stack.pop() {
            subtree.push(cur);
            stack.extend(self.tree.children_of(Some(cur)).iter().copied());
        }
        for cur in subtree {
            if let Some(route) = self.routes.remove(&cur) {
                if let Some((obj, buf)) = route.buffer {
                    let visible = route.visible_on.clone();
                    self.release_buffer(route.client, obj, buf, &visible);
                }
                if let Some(role) = route.role_obj {
                    self.by_role_obj.remove(&ObjectKey::new(route.client, role));
                }
                self.by_surface_obj
                    .remove(&ObjectKey::new(route.client, route.surface_obj));
                // Phase 45: the toplevel mapping and the material
                // request die with the surface (a re-attached role
                // starts from the server's own resolution again).
                if let Some(toplevel) = route.toplevel_obj {
                    self.by_toplevel_obj
                        .remove(&ObjectKey::new(route.client, toplevel));
                }
                self.material_requests.remove(&cur);
                // Phase 47: the semantic claims and any live
                // transition die with the surface (a re-attached role
                // starts from the server's own doctrine again).
                self.semantics.remove(&cur);
                self.transitions.live.remove(&cur);
            }
            self.pending_attach.remove(&cur);
        }
        self.tree.destroy(id).ok();
        self.dirty = true;
    }

    /// Drop every object of a client (session end): surfaces release
    /// their buffers, pools and buffer objects lose their entries.
    pub fn drop_client(&mut self, client: ClientId) {
        let ids: Vec<SurfaceId> = self
            .routes
            .iter()
            .filter(|(_, r)| r.client == client)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.destroy_surface(id);
        }
        self.pools.retain(|k, _| k.client != client.as_u32());
        self.buffers.retain(|k, _| k.client != client.as_u32());
        // Pending attaches: the subtree loop above already removed this
        // client's entries (every one of its routed surfaces walked
        // `destroy_surface`, which drops each surface's pending attach).
        // The historical `clear()` here swept the *whole* map — a
        // survivor's in-flight attach (sent, not yet committed) was
        // silently discarded, its next commit attached nothing, and no
        // error was ever delivered: a cross-client state leak from any
        // concurrent session teardown. The retain is the honest orphan
        // sweep instead: an entry whose surface has no route is dead
        // state regardless of who owned it, and a live route is always
        // somebody else's attach to keep.
        self.pending_attach
            .retain(|id, _| self.routes.contains_key(id));
        self.output_binds.retain(|(c, _), _| *c != client.as_u32());
        self.output_bind_seq.remove(&client.as_u32());
        // Pending releases too: a release event for a gone client is
        // undeliverable, and letting one fire would re-create the
        // client's outbox queue post-mortem (and leak the entry's
        // eventfd). Surfaced by the Phase 19 crash corpus
        // (`tests/src/crash.rs`, the after-commit cut point).
        self.pending_releases.retain(|r| r.client != client);
    }

    /// Take the accumulated tree changes (the render pass's input).
    pub fn take_changes(&mut self) -> FrameChanges {
        self.tree.take_changes()
    }

    /// Drain *every* pending release, flip-gating be damned — the
    /// dark-state flush (Phase 26). Going dark destroys the scanout:
    /// the compositor stops reading client buffers the moment the
    /// disable applies, so every superseded buffer owed a fence gets
    /// it now instead of waiting for a flip that will never land.
    /// Attach swaps made *while* dark route through the same flush
    /// (see [`World::pump`](crate::frame_loop)).
    pub fn take_pending_releases(&mut self) -> Vec<PendingRelease> {
        core::mem::take(&mut self.pending_releases)
    }

    /// The current immutable snapshot for the render pass.
    #[must_use]
    pub fn snapshot(&mut self) -> FrameSnapshot {
        self.tree.snapshot()
    }
}

/// The compositor-owned transitions host (Phase 47): the live
/// per-surface motions, one [`Transition`] per animating surface.
///
/// The host is the driver-side half of the transitions catalog: the
/// frame loop advances it at every pump (the dock's advance pattern
/// — one integration per wake, at the driver's clock), the damage
/// pass claims each live transition's surface rectangle (the vacate
/// doctrine: a fade moves every pixel under it), and the render path
/// reads [`TransitionHost::opacity_of`] at layer assembly — the one
/// opacity truth both the solver's facts and the renderer's layer
/// consume, so the two cannot diverge.
///
/// A settled transition is *removed* at the advance that settled
/// it: the settled frame renders at the default opacity (exactly
/// 1.0), byte-identical with the never-animated one — every pixel
/// oracle the equivalence corpora pin keeps its meaning.
#[derive(Debug, Default)]
pub struct TransitionHost {
    /// Whether the server serves compositor-owned transitions at all
    /// (the config's `transitions` switch, `--transitions` at the
    /// CLI). The **library default is off** — plain pixels,
    /// byte-identical, so every equivalence oracle keeps its meaning;
    /// the CLI turns the choreography on for the served desktop.
    pub enabled: bool,
    /// The live transitions by surface (unsettled only — settle is
    /// removal).
    pub live: HashMap<SurfaceId, Transition>,
    /// The driver timestamp the springs last integrated at (ms).
    pub last_ms: u64,
    /// Transitions begun so far (test introspection).
    pub begun: u64,
}

impl TransitionHost {
    /// Begin a transition on `surface` (a live one is replaced — the
    /// freshest motion wins, the spring restarts from the current
    /// truth).
    pub fn begin(&mut self, surface: SurfaceId, kind: TransitionKind, now_ms: u64) {
        self.live.insert(surface, Transition::open(kind, now_ms));
        self.begun += 1;
        self.last_ms = now_ms;
    }

    /// The layer opacity a surface renders at this frame: the live
    /// transition's current value, or 1.0 (the never-animated
    /// default — the settled oracle).
    #[must_use]
    pub fn opacity_of(&self, surface: SurfaceId) -> f32 {
        self.live.get(&surface).map_or(1.0, Transition::opacity)
    }

    /// Whether any motion is live.
    #[must_use]
    pub fn any_live(&self) -> bool {
        !self.live.is_empty()
    }

    /// Advance every live spring to `now_ms`, dropping the settled
    /// ones. Returns whether **any motion was live before the
    /// advance** — the pump's repaint claim: a transition that just
    /// settled still returns `true` (its final, exact-1.0 frame must
    /// render), and the next advance (nothing live) returns `false`
    /// (quiescence).
    pub fn advance(&mut self, now_ms: u64) -> bool {
        let had_live = !self.live.is_empty();
        let mut settled_ids = Vec::new();
        for (id, t) in &mut self.live {
            t.advance(now_ms);
            if t.settled() {
                settled_ids.push(*id);
            }
        }
        for id in settled_ids {
            self.live.remove(&id);
        }
        self.last_ms = now_ms;
        had_live
    }
}

/// One close-fade ghost (Phase 48): the owned snapshot of a window
/// that just left the desktop, fading out under the transitions
/// catalog's close spring.
///
/// The ghost is the *pixels* the dying window had — a row-tight copy
/// of its last committed buffer (the pool's mapping may be reused or
/// freed the moment the release lands, so the copy is the only
/// honest carrier) — plus everything the render path needs to draw
/// it exactly as the last frame drew it: the geometry, the transform,
/// the color description, the resolved style (the same
/// `surface_style` the layer walk resolves), and the z slot the
/// window held in the render order (the ghost keeps the window's own
/// place in the stack — a fading window never jumps above the
/// windows that were above it).
///
/// The close transition animates one scalar, the layer opacity — the
/// same per-layer alpha the pipeline already speaks. A settled ghost
/// is *removed* at the advance that settled it: the frame after the
/// fade is the plain post-removal desktop, byte-identical with the
/// never-animated one.
///
/// Phase 57 — the chrome ghost: a server-decorated window's band
/// rides the fade. The content's ink is owned (the client's pool is
/// the client's); the chrome's is not copied at all — the band's ink
/// is *stateless* (the shape is the raster's whole truth), so the
/// ghost carries the frozen shape and the render path reads the same
/// chrome cache the living desktop reads. The whole frame — band,
/// strip, content — leaves as one, at the one close-spring opacity.
#[derive(Debug)]
pub struct Ghost {
    /// The ghost's identity (the buffer-view key — unique per ghost,
    /// never a client buffer's).
    pub id: u64,
    /// The desktop-space rectangle the window occupied (output-local
    /// translation is the grade walk's, exactly a client layer's).
    pub dest: Rect,
    /// The owned ink: tightly packed rows, `width * 4` bytes each
    /// (the SHM family is 32-bit).
    pub ink: Vec<u8>,
    /// The validated geometry over the owned ink.
    pub geometry: BufferGeometry,
    /// The buffer-to-surface transform the window wore.
    pub transform: Transform,
    /// The color description the window wore.
    pub color: ColorDescription,
    /// The HDR metadata the window wore (the tone policy's input).
    pub hdr: Option<HdrMetadata>,
    /// The resolved style at capture (the material's chrome, frozen
    /// at death — the shadow and corners fade with the ink).
    pub style: ldp_renderer::LayerStyle,
    /// The render-order slot the window held (the ghost's z
    /// fidelity: it is inserted at this index in the layer stack,
    /// clamped to the stack the fade walks).
    pub z: usize,
    /// The dying window's frozen chrome (Phase 57): `Some` exactly
    /// when the window wore the server-drawn band at death — the
    /// frame's shape truth, frozen the same breath the style froze.
    pub chrome: Option<GhostChrome>,
    /// The close fade itself.
    pub transition: Transition,
}

/// One ghost's chrome payload (Phase 57): everything the render path
/// needs to draw the dying window's band exactly as the last frame
/// drew it. The ink is the chrome pass's own cache (the shape and the
/// strip key are the ink's *whole* stateless truth — same shape, same
/// bytes), so the ghost borrows the desktop's rasters instead of
/// owning copies: the band, the strip, and the content fade at the
/// one opacity, in the one z slot, reading the one cache.
#[derive(Debug)]
pub struct GhostChrome {
    /// The frame rect the chrome wore (content + insets, world
    /// coordinates — the band's place on the desktop).
    pub frame: Rect,
    /// The raster's shape key (the insets, the scaled close metrics,
    /// the Liquid variant — the ink's stateless inputs, frozen at
    /// death exactly as the client style froze).
    pub shape: crate::shell::ChromeShape,
    /// The title strip's key and rect, if a title served at death
    /// (the strip's ink rides the same cache's truth).
    pub strip: Option<(crate::shell::StripKey, Rect)>,
}

/// The ghost host: every live close fade (Phase 48).
///
/// The host advances at the pump's cadence with the transitions host
/// (one clock, one integration per wake — the dock's doctrine), the
/// damage pass claims each live ghost's rectangle plus its style's
/// effect rect (a fading shadow is still a shadow), and the render
/// path reads [`GhostHost::opacity_of`] at layer assembly — the one
/// opacity truth, exactly the transitions host's contract.
#[derive(Debug, Default)]
pub struct GhostHost {
    /// The live ghosts (unsettled only — settle is removal).
    pub ghosts: Vec<Ghost>,
    /// The rectangles the last advance vacated — the removal claims
    /// (a ghost is nobody's surface: no client economy re-renders its
    /// rect when it goes, so the host itself must name the region the
    /// settle frame repaints — the ghost's last ink leaves the canvas
    /// with the ghost). The damage pass drains these.
    pub vacated: Vec<Rect>,
    /// The next ghost identity.
    next_id: u64,
    /// Ghosts begun so far (test introspection).
    pub begun: u64,
}

impl GhostHost {
    /// Take a ghost (the capture is the caller's — the host only
    /// owns the advancing and the identity).
    pub fn push(&mut self, mut ghost: Ghost) {
        ghost.id = self.next_id;
        self.next_id += 1;
        self.begun += 1;
        self.ghosts.push(ghost);
    }

    /// Whether any close fade is live.
    #[must_use]
    pub fn any_live(&self) -> bool {
        !self.ghosts.is_empty()
    }

    /// The layer opacity a ghost renders at this frame: the close
    /// spring's current value (never the never-animated 1.0 — a
    /// ghost exists only while fading).
    #[must_use]
    pub fn opacity_of(&self, ghost: &Ghost) -> f32 {
        ghost.transition.opacity()
    }

    /// Advance every ghost's spring to `now_ms`, dropping the
    /// settled ones and recording their rectangles in
    /// [`GhostHost::vacated`] — the removal claims the damage pass
    /// drains (the settle frame repaints the ghost's last ink off the
    /// canvas). Phase 57: a ghost that carried chrome vacates the
    /// *frame* — the band's region is the ghost's own to clear, no
    /// client economy repaints it. Returns whether **any ghost was
    /// live before the advance** — the pump's repaint claim.
    pub fn advance(&mut self, now_ms: u64) -> bool {
        let had_live = !self.ghosts.is_empty();
        let mut vacated = Vec::new();
        self.ghosts.retain_mut(|g| {
            g.transition.advance(now_ms);
            if g.transition.settled() {
                vacated.push(g.dest);
                if !g.style.is_plain() {
                    vacated.push(g.style.effect_rect(g.dest));
                }
                if let Some(chrome) = g.chrome.as_ref() {
                    vacated.push(chrome.frame);
                }
                false
            } else {
                true
            }
        });
        self.vacated = vacated;
        had_live
    }
}

/// The double-buffered scanout chain: two framebuffers and the
/// delivery surface behind them.
///
/// Two stores serve two worlds: the [`ScanoutStore::Shadow`] pair of
/// ARGB word buffers (the mock vehicle — what the headless oracle
/// reads), and the [`ScanoutStore::Mapped`] pair of CPU-mapped
/// scanout buffers (the real one — kernel dumb buffers on hardware,
/// anonymous mappings on the CI equivalence path). Flip tracking
/// (`front`, `flip_pending`) is store-independent: the atomic page
/// flip addresses framebuffers, not memory.
#[derive(Debug)]
pub struct ScanoutChain {
    /// Their registered framebuffer ids.
    pub fbs: [FbId; 2],
    /// Index of the buffer the last submitted flip scans out.
    pub front: usize,
    /// Whether a flip is in flight (no render until it lands).
    pub flip_pending: bool,
    /// The backing store the render pass delivers into.
    pub store: ScanoutStore,
}

/// Where scanout memory actually lives.
#[derive(Debug)]
pub enum ScanoutStore {
    /// The headless shadow: two premultiplied-ARGB word buffers (the
    /// test oracle — what the display would be showing).
    Shadow([Vec<u32>; 2]),
    /// Real mapped scanout buffers with the kernel's row pitch: dumb
    /// buffers on hardware, anonymous mappings on the CI equivalence
    /// path (the same lifetime discipline, the same pitch-honoring
    /// writes).
    Mapped {
        /// The two mappings (back/front rotation via `front`).
        maps: [DumbMapping; 2],
        /// Row pitch in bytes — the allocator's, not `width * 4`.
        pitch: usize,
        /// Mode width in pixels.
        width: u32,
        /// Mode height in pixels.
        height: u32,
    },
}

impl ScanoutChain {
    /// The shadow store's initial state: opaque black, both buffers.
    #[must_use]
    pub fn shadow(pixels: usize, fbs: [FbId; 2]) -> Self {
        Self {
            fbs,
            front: 0,
            flip_pending: true, // the bring-up flip is in flight
            store: ScanoutStore::Shadow([vec![0xFF00_0000; pixels], vec![0xFF00_0000; pixels]]),
        }
    }

    /// The mapped store's state over two mappings at a pitch.
    #[must_use]
    pub fn mapped(
        maps: [DumbMapping; 2],
        pitch: usize,
        width: u32,
        height: u32,
        fbs: [FbId; 2],
    ) -> Self {
        Self {
            fbs,
            front: 0,
            flip_pending: true, // the bring-up flip is in flight
            store: ScanoutStore::Mapped {
                maps,
                pitch,
                width,
                height,
            },
        }
    }

    /// Deliver a composed frame into the back buffer (the render
    /// pass's one write): shadow stores take the words directly,
    /// mapped stores take them row-by-row at the kernel's pitch.
    ///
    /// # Panics
    ///
    /// Never in-crate (frames are mode-shaped by construction); a
    /// malformed frame fails fast inside the row copier's asserts.
    pub fn write_back(&mut self, frame: &[u32]) {
        match &mut self.store {
            ScanoutStore::Shadow(buffers) => {
                buffers[1 - self.front].copy_from_slice(frame);
            }
            ScanoutStore::Mapped {
                maps,
                pitch,
                width,
                height,
            } => {
                let back = &mut maps[1 - self.front];
                write_frame_rows(
                    back.as_mut_slice(),
                    *pitch,
                    (*width) as usize,
                    (*height) as usize,
                    frame,
                );
            }
        }
    }

    /// The front buffer's content as premultiplied ARGB words — what
    /// the display is scanning out (the test oracle; the capture
    /// path's source).
    #[must_use]
    pub fn scanout_words(&self) -> Vec<u32> {
        match &self.store {
            ScanoutStore::Shadow(buffers) => buffers[self.front].clone(),
            ScanoutStore::Mapped {
                maps,
                pitch,
                width,
                height,
            } => read_frame_rows(
                maps[self.front].as_slice(),
                *pitch,
                (*width) as usize,
                (*height) as usize,
            ),
        }
    }

    /// Mark the back buffer submitted: it becomes the front once the
    /// flip lands; the old front becomes the next render target.
    pub fn submitted(&mut self) {
        self.front = 1 - self.front;
        self.flip_pending = true;
    }
}

/// How multiple served outputs arrange on the logical desktop
/// (Phase 37, `--outputs mirror`).
///
/// `Extended` is the Phase 31 doctrine: the outputs lay out
/// left-to-right into one logical desktop — a window spanning the
/// seam enters both, each display carrying its own portion.
/// `Mirrored` is the clone doctrine: every output sits at the layout
/// *origin*, so each shows the same desktop (the primary's bounds)
/// cropped to its own mode — a smaller display crops the top-left,
/// a larger letterboxes in the desktop's own background. The
/// geometry event reports the shared origin (the overlapping-output
/// convention: a client that binds two outputs at the same position
/// knows they are clones — the same vocabulary X11 and the Wayland
/// compositors serve for clone mode).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum OutputArrangement {
    /// One logical desktop, outputs appended left-to-right (the
    /// default — the Phase 31 bytes).
    #[default]
    Extended,
    /// Every output at the origin: one desktop, every display a
    /// mirror of it (`--outputs mirror`).
    Mirrored,
}

/// One served output: the pipeline's full state under one roof — the
/// protocol face, the CRTC and primary plane driving it, the scanout
/// chain, the DRM lease, and that output's own flip ledger. The
/// world's `outputs` vector holds these; slot 0 is the primary (the
/// pacing scheduler's output, the shell's layout owner).
// The bools are each documented semantic state (owes, all_planes,
// psr_capable, psr_live) — the same doctrine the World's own flag
// family carries: no collapsed state machines inventing states the
// protocol never named.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug)]
pub struct OutputSlot {
    /// The output's protocol face (identity, modes, layout position).
    pub output: OutputGlobal,
    /// The CRTC scanning this output out.
    pub crtc: CrtcId,
    /// The primary plane feeding the CRTC.
    pub plane: PlaneId,
    /// The double-buffered scanout chain.
    pub scanout: ScanoutChain,
    /// The DRM lease (kernel objects this pipeline owns); `None` on
    /// the mock path.
    pub lease: Option<DrmLease>,
    /// Completed flips on this output's CRTC — the release-gate
    /// counter for buffers this output was scanning out.
    pub flips: u64,
    /// Damage accumulated for this output since its last render —
    /// desktop coordinates (the pump clips and translates at render
    /// time). Empty means the output's frame is current.
    pub pending: ldp_core::geometry::Region,
    /// Whether this output owes a frame to the next pump — the primary
    /// rides every commit (the pacing grid's flip must land for the
    /// presentation verdict, damage or no damage: an attach-less
    /// commit still presents). Non-primary outputs owe only their own
    /// damage.
    pub owes: bool,
    /// The output's plane inventory (Phase 34): the per-CRTC
    /// capability walk — the primary, the overlays above it, the
    /// reserved cursor — that the frame loop's plane solver consumes
    /// every frame. Collected at bring-up (and at every
    /// re-arrangement); an inventory with no overlays is the honest
    /// pre-Phase-34 composite-only doctrine.
    pub planes: ldp_planes::PlaneInventory,
    /// The planes the last submitted commit left ON (Phase 34): the
    /// state the next commit must fully replace — an overlay that
    /// drops out of the plan is turned *off* here, or it would keep
    /// scanning stale pixels above the canvas.
    pub active_planes: Vec<PlaneId>,
    /// Whether the last submitted frame was the all-planes shape
    /// (zero composite passes — the renderer's framebuffer was not
    /// touched). The next composite frame must take *full* damage:
    /// the persistent canvas is stale by exactly one zero-composite
    /// frame.
    pub all_planes: bool,
    /// The display model (Phase 34): the composed truth — the canvas
    /// with every plane-carried layer blended in zpos order, what the
    /// panel's plane blender shows. Maintained per frame by the
    /// render pass; empty until the first frame (the pre-Phase-34
    /// oracle reads the scanout chain then).
    pub display: Vec<u32>,
    /// The panel self-refresh machine (Phase 35): the per-output
    /// decision layer — entry earned through consecutive quiet flip
    /// opportunities, exits named. The world's `psr_tick` drives it
    /// at the serve loop's cadence; every flip submitted on this
    /// output is a disturbance (the device's own implicit release).
    pub psr: ldp_power::psr::PsrMachine,
    /// Whether the connector carries the `panel self refresh`
    /// property (the bring-up walk's answer — the mock's connectors
    /// all do; a real panel that does not simply never engages).
    pub psr_capable: bool,
    /// The device-side mirror: whether the engage commit landed (the
    /// property is believed on). Cleared by every exit and by the
    /// device's implicit release (a flip); re-proven only by the next
    /// engage commit.
    pub psr_live: bool,
}

/// The whole compositor world under one lock.
// The bools are semantic state flags, each documented at its field —
// not a collapsed state machine (merging them would invent states the
// protocol never names).
#[allow(clippy::struct_excessive_bools)]
pub struct World {
    /// The driven device — mock or real, behind the serve loop's
    /// driver seam (Phase 25: the wake-point integration; both
    /// backends execute the same frame choreography through it).
    pub device: Box<dyn DisplayDriver>,
    /// The served outputs — slot 0 is the *primary* (the scheduler's
    /// pacing grid, the positioning shell's layout owner, the capture
    /// surface). Empty is the honest dark state (Phase 26): every
    /// connector gone, the pipelines disabled, the protocol still
    /// serving — the first re-probe with a connected connector
    /// relights it. One entry is the Phase 25 single-pipeline doctrine
    /// (the library default); several is Phase 31's multi-output
    /// serving.
    pub outputs: Vec<OutputSlot>,
    /// The protocol scene.
    pub scene: Scene,
    /// The persistent renderer (its framebuffer is the canonical
    /// composition; scanout buffers are delivery vehicles). The
    /// hardware-first selection picked it at bring-up: the GL backend
    /// on machines with a GL stack, the software reference otherwise.
    /// One renderer serves every output — each render pass re-targets
    /// it with that output's description.
    pub renderer: crate::renderer::CompositorRenderer,
    /// The topology snapshot the last re-arrangement (or bring-up)
    /// saw — the "before" the next re-probe diffs against.
    pub topology: Vec<(ldp_display::ids::ConnectorId, ldp_display::ConnectorStatus)>,
    /// Whether the scanout store is the mapped kind (real mappings at
    /// a pitch) or the shadow kind (word buffers) — persists across
    /// re-arrangements so a migration re-allocates the same kind the
    /// world was configured with.
    pub mapped_scanout: bool,
    /// Cross-client pending events.
    pub outboxes: Outboxes,
    /// Live registry objects (Phase 44, the dynamic-globals fan-out
    /// targets): one entry per `connection.get_registry` the session
    /// minted — the withdrawal and re-advertisement sweeps emit
    /// `registry.global_remove` / `registry.global` on exactly these.
    /// Dropped at the object's destroy and at session end.
    pub live_registries: HashMap<ObjectId, ClientId>,
    /// Every live global-bound object's interface (Phase 44): the
    /// withdrawal sweep revokes exactly these, per interface — the
    /// spec's "objects of that interface are revoked before this
    /// event" ordering, served by pushing the revocations ahead of
    /// the removal barrier on the same per-client FIFO.
    pub bound_globals: HashMap<ObjectId, (ClientId, Box<str>)>,
    /// Interfaces withdrawn from the live advertisement set (Phase
    /// 44): a bind of one of these is accepted by the static server
    /// config and revoked immediately by the dispatcher (the
    /// dark-state pattern, now carrying `interface_removed`), and
    /// every withdrawal already told the live registries so.
    pub withdrawn_globals: std::collections::HashSet<Box<str>>,
    /// Desktop frames rendered so far (one per render pass — a pass
    /// renders every output with pending damage; test introspection).
    pub frames: u64,
    /// Device events serviced outside a session's pump (hotplug
    /// counts; the serve loop's own wake bookkeeping).
    pub device_events: u64,
    /// Re-arrangements executed so far — migrations, darks, relights,
    /// output additions and removals (test introspection; the serve
    /// loop's hotplug arm).
    pub rearranges: u64,
    /// Optional raw-frame dump sink (`--dump`), by frame index — the
    /// *primary* output's frames (the screenshot doctrine).
    pub dump: Option<std::path::PathBuf>,
    /// The renderer-selection report line ("renderer: …"), from
    /// bring-up — the honest one-liner the startup prints.
    pub renderer_report: String,
    /// The resolved Liquid quality tier (Phase 27): every surface the
    /// compositor serves gets its style from this — corners and
    /// shadows for opaque windows, the frosted material added for
    /// translucent ones. `Minimal` renders the plain Phase 26 pixels.
    pub effects: ldp_renderer::EffectTier,
    /// The positioning shell (Phase 28): the layout policy and the
    /// system dock — laid out over the *primary* output's geometry
    /// (the macOS doctrine: the dock owns the primary display). The
    /// library default keeps the dock off (plain pixels); the CLI's
    /// `--dock auto` serves the home dock.
    pub shell: crate::shell::Shell,
    /// The SSD chrome pass (Phase 52): the drawn band's raster cache
    /// — the title bar, the border ring, and the close affordance
    /// every server-decorated window wears (the dock's own ink
    /// model: CPU paint, cached per frame shape, no framebuffer).
    /// The render pass prepares and reads it; the input pump's ring
    /// hit test reads the same geometry truth.
    pub chrome: crate::shell::ChromePass,
    /// The operator's forced output size (`--resolution WxH`, Phase
    /// 30): every selection — bring-up and every hotplug migration —
    /// prefers a mode of exactly this size, and a display that does
    /// not offer it falls back to the unsized doctrine (never a
    /// silent nearest-neighbor guess). `None` is the plain doctrine.
    pub resolution: Option<ldp_display::serve::ModeSize>,
    /// The mode foundry's pour (`--synth WxH[@HZ]`, Phase 42): every
    /// selection — bring-up and every hotplug migration — pours the
    /// reduced-blanking timing instead of consulting the firmware's
    /// list, each connector's own EDID range limits gating its pour
    /// (a refusing connector falls back to the unsized doctrine,
    /// exactly the sized doctrine's honesty). `None` is the plain
    /// doctrine.
    pub synth: Option<ldp_display::timing::SynthRequest>,
    /// The multi-output doctrine (Phase 31): `false` (the library
    /// default) serves exactly one pipeline — the Phase 25/26
    /// behavior, byte-identical, so every equivalence oracle keeps
    /// its meaning; `true` serves every pipeline the allocator finds
    /// (several CRTCs, several scanout chains, one logical desktop).
    pub multi_output: bool,
    /// The arrangement of the served outputs (Phase 37): `Extended`
    /// (the default) appends them left-to-right into one desktop;
    /// `Mirrored` places them all at the layout origin — every
    /// display shows the same desktop (`--outputs mirror`). Survives
    /// re-arrangements: a display joining a mirrored desktop joins
    /// the mirror, not an extension.
    pub arrangement: OutputArrangement,
    /// The per-output scale doctrine (Phase 37, `--scale F1,F2,…`):
    /// the operator's per-output factors, in output order. Empty (the
    /// default) is the Phase 31 doctrine — every output advertises
    /// the one `--scale F` factor (the primary's). A hotplug newcomer
    /// takes the *last* entry (the stretch rule), so the doctrine
    /// never runs out of factors as the topology moves.
    pub output_scales: Vec<ScaleFactor>,
    /// The idle ladder (Phase 31, `--idle MS`): the ldp-power
    /// machine ticking at the device clock — `Dimmed` reports the
    /// backlight ramp, `Off` blanks through the DPMS property and
    /// parks the scheduler, activity lights it back. `None` (the
    /// default) never sleeps.
    pub idle: Option<ldp_power::idle::IdleMachine>,
    /// Whether every output is currently blanked (the `Off` rung).
    pub blanked: bool,
    /// The most recent injected input's arrival (the latency rig's
    /// open measurement).
    pub last_input: Option<Mono>,
    /// The most recent completed input→photon measurement (ns).
    pub input_photon_ns: Option<u64>,
    /// Whether panel self-refresh serves (Phase 35): `true` (the
    /// default, the macOS doctrine — the panel sleeps when the
    /// content is static) or `--no-psr` (the operator's escape, e.g.
    /// a panel whose PSR flickers — the real-world quirk table).
    pub psr_enabled: bool,
    /// The GPU clock governor (Phase 35): the asymmetric-hysteresis
    /// ladder over the landed-flip load signal — up instant, down
    /// patient. Pure decision; the report line and the residency
    /// histogram are what the operator reads.
    pub governor: ldp_power::governor::ClockGovernor,
    /// The energy ledger (Phase 35): the accumulated time-in-state
    /// account over the documented cost model — the static-scene
    /// ratio is the comparison table's power row.
    pub ledger: ldp_power::ledger::EnergyLedger,
    /// The ledger's last sample point (the interval boundary).
    pub power_sample_at: Mono,
    /// The governor's last interval point.
    pub governor_at: Mono,
    /// Landed flips since the governor's last interval (the load
    /// signal's window counter — the primary's flips, the pacing
    /// grid's own clock).
    pub governor_flips: u64,
    /// Whether flips carry the CRTC's VRR enablement (Phase 31,
    /// `--vrr`): every VRR-capable output's page-flip commits arm
    /// `VRR_ENABLED` — the panel stretches inside its advertised
    /// window (the mock's timeline clamps exactly like the kernel).
    pub vrr_enabled: bool,
    /// Whether the uniform collapse holds (Phase 41,
    /// `--vrr-uniform`): a mixed desktop — one output with a window,
    /// one without — serving one fixed sync across the seam (the
    /// `vrr-sibling-flicker` quirk's escape). While collapsed no
    /// output arms, whatever its own capability; the desktop is one
    /// clock or nothing.
    pub vrr_collapse: bool,
    /// The floor pass's audit trail (Phase 41, `--vrr-floor`), one
    /// entry per served output in output order: what the quirk
    /// ledger's first page did to each advertised window — the
    /// startup report prints this shape.
    pub vrr_floor_outcomes: Vec<ldp_vrr::FloorOutcome>,
    /// The HDR output-mode state (Phase 31): the stack summary's fold
    /// over the desktop (per-surface HDR metadata and color
    /// descriptions) feeding the mode controller's hysteresis — an
    /// HDR surface flips the panel's composite onto the PQ canvas (a
    /// lone popup cannot flap it; the dwell holds), all-SDR stacks
    /// keep the sRGB canvas. `None` (the default, no `--hdr`) is the
    /// honest SDR panel: the fold never runs.
    pub hdr: Option<ldp_hdr::policy::ModeController>,
    /// The negotiated canvas ceiling (Phase 38), in nits: the stack's
    /// brightest content clamped to the panel's *effective* peak —
    /// what the render pass tone-maps against. Recorded by the same
    /// fold that feeds the mode controller; `None` before the first
    /// HDR content (the static `pq_hdr()` canvas stands) and whenever
    /// the panel is not HDR.
    pub negotiated: Option<u32>,
    /// The data family's manager (Phase 32): devices, sources,
    /// offers, transfers — the clipboard and primary selections the
    /// data service routes. One manager, every seat: the compositor
    /// serves the one seat today ([`SEAT`]).
    pub clipboard: ldp_clipboard::ClipboardManager,
    /// The seat's interaction serial clock (Phase 32): every
    /// serial-bearing delivery the seat issues draws from this clock
    /// — today that is the shell's `configure` (the one serial a
    /// client can see and reference); the device-event path (the
    /// evdev roadmap line) will draw from the same clock. A
    /// `set_selection` serial must equal the clock's current value
    /// (the freshness doctrine the manager enforces).
    pub seat_serial: u32,
    /// The served input state (Phase 33): the seat router, the
    /// client device-object bindings, the keyboard focus, the real
    /// evdev devices. The headless default serves nothing (the
    /// byte-exactness doctrine); the scripted test feed and the real
    /// device pump share one routing path.
    pub input: crate::input::InputState,
    /// The data family's server-object counter: every routed
    /// `data_offer` announcement mints a world-unique server id
    /// (bit 31 set), so the receiving session's create can never
    /// collide with another client's object.
    pub next_data_object: u32,
    /// The buffer-import ledger (Phase 34): buffer object → the
    /// kernel framebuffer its pool's descriptor registered (the
    /// GEM-import walk's product), or `None` when the walk refused —
    /// the CPU-composition truth. `None` entries cache the refusal so
    /// the walk runs once per buffer, not once per frame.
    pub imports: HashMap<ObjectKey, Option<FbId>>,
    /// Desktop frames that composited zero renderer passes (the
    /// all-planes pages — test introspection and the perf report).
    pub zero_pass_frames: u64,
    /// The popup-role host (Phase 36): every live `ldp.shell.popup`
    /// object — the anchor/gravity machine and the constraint-solving
    /// serials. The dispatcher mints and drives it; the tree carries
    /// the solved positions.
    pub popups: crate::shell::PopupHost,
    /// The dialog-role host (Phase 48): every live `ldp.shell.dialog`
    /// object — the modality machine, the centered placement, and
    /// the parent pairing. The dispatcher mints and drives it; the
    /// tree carries the centered positions, exactly the popup
    /// doctrine.
    pub dialogs: crate::shell::DialogHost,
    /// The toplevel-role host (Phase 49, the states arm): every live
    /// `ldp.shell.toplevel` object — the states machine, the
    /// two-phase proposals, and the decoration choice. The dispatcher
    /// mints and drives it; the tree carries the realized geometry,
    /// exactly the popup/dialog doctrine.
    pub toplevels: crate::shell::ToplevelHost,
    /// The interactive-drag host (Phase 50, the operator's hand): the
    /// one live pointer grip — a move (server-truth geometry at the
    /// pump's cadence) or an edge-grip resize (proposals at the
    /// pointer's cadence, the client's ack+commit realizing). The
    /// dispatcher mints and retires it; the input pump advances it.
    pub drags: crate::shell::DragHost,
    /// The spaces machine (Phase 49, the states arm): the space
    /// count, every window's home space, stickiness, and the one
    /// seat's active space (`set_workspace` moves windows through
    /// it; the visibility sync feeds the scene's hidden set). The
    /// dispatcher drives it; the render path consumes its truth.
    pub spaces: ldp_shell::Spaces,
    /// Live shell-global binds (Phase 51, the view switch): every
    /// client that bound `ldp.shell.shell` — the `workspace_switched`
    /// broadcast set (the taskbar that asked, the wallpaper daemon
    /// that did not; the registry fan-out's own doctrine, one set
    /// per global). Dropped at the object's destroy and at session
    /// end.
    pub shell_binds: HashMap<ObjectId, ClientId>,
    /// Phase 51: the view switch's sweep is running — the per-hide
    /// focus promotion stands down (the sweep's *end* promotion is
    /// the authority: one transition, the space-preferred target,
    /// never the intermediate topmost). Private policy state.
    pub(crate) view_sweep: bool,
}

/// The one seat the compositor serves (Phase 32's single-seat
/// doctrine: the seat global is one, every device mints from it; the
/// multi-seat split is the seat registry's roadmap line).
pub const SEAT: ldp_clipboard::SeatKey = ldp_clipboard::SeatKey(0);

/// The spaces machine's seat id (Phase 49): the one seat, in
/// `ldp-shell`'s vocabulary — the shell addresses seats by index
/// (u32), the clipboard by key (u64); the two vocabularies agree on
/// the value, zero.
const SPACE_SEAT: u32 = 0;

impl World {
    /// The hardware plane arm's startup report line (Phase 34): the
    /// per-output inventory truth — the overlays the solver can
    /// offload onto, the reserved cursor, or the honest composite-only
    /// line when a pipeline carries no overlays.
    pub fn planes_report(&self) -> String {
        if self.outputs.is_empty() {
            return "off (no outputs)".to_owned();
        }
        let mut parts = Vec::with_capacity(self.outputs.len());
        for (i, slot) in self.outputs.iter().enumerate() {
            let overlays = slot.planes.overlay_slots();
            let cursor = slot.planes.cursor.is_some();
            parts.push(format!(
                "output {} (CRTC {}): {} overlay{}{}",
                i,
                slot.crtc.raw(),
                overlays,
                if overlays == 1 { "" } else { "s" },
                if cursor { ", cursor reserved" } else { "" }
            ));
        }
        parts.join("; ")
    }

    /// The input path's startup report line (Phase 33): what serves
    /// — or why not.
    pub fn input_report(&self) -> String {
        self.input.report()
    }

    /// The router's pointer position (the tests' oracle).
    pub fn input_pointer_position(&self) -> (f32, f32) {
        self.input.pointer_position()
    }

    /// The router's pointer focus (the routed surface).
    pub fn input_pointer_focus(&self) -> Option<SurfaceId> {
        self.input.pointer_focus()
    }

    /// The shell's keyboard focus (the click-to-target policy's
    /// state).
    pub fn input_keyboard_focus(&self) -> Option<SurfaceId> {
        self.input.keyboard_focus()
    }

    /// Draw the seat's next interaction serial (monotonic, wrapping
    /// at the wire's u32 — the same discipline the shell and the
    /// bridges keep).
    pub fn next_seat_serial(&mut self) -> u32 {
        self.seat_serial = self.seat_serial.wrapping_add(1);
        self.seat_serial
    }

    /// Mint the next data-family server object id (world-unique,
    /// server-owned bit set; the object-count ceiling guards runaway
    /// growth long before the counter's space could wrap).
    pub fn next_data_object(&mut self) -> ObjectId {
        self.next_data_object = self.next_data_object.wrapping_add(1);
        ObjectId::from_wire(ObjectId::SERVER_FLAG | self.next_data_object)
    }
}

/// The kernel objects a DRM serve session owns — released in order at
/// teardown or migration, after the disable commit stops referencing
/// them. Lives in the world so live re-arrangement can swap it (the
/// compositor's outer handle is gone in Phase 26).
#[derive(Debug, Clone, Copy)]
pub struct DrmLease {
    /// The two dumb buffers' GEM handles.
    pub handles: [u32; 2],
    /// Their registered framebuffer ids.
    pub fbs: [FbId; 2],
}

/// The shared handle session threads receive.
pub struct Shared {
    /// The one world lock (render and dispatch serialize: the v-slice
    /// single-pipeline doctrine).
    pub world: Mutex<World>,
}

static SHARED_SEQ: AtomicU64 = AtomicU64::new(0);

impl Shared {
    /// Wrap a world.
    #[must_use]
    pub fn new(world: World) -> Arc<Shared> {
        let _ = SHARED_SEQ.fetch_add(1, Ordering::Relaxed);
        Arc::new(Shared {
            world: Mutex::new(world),
        })
    }
}

impl World {
    /// A registry object was minted (Phase 44): it becomes a
    /// fan-out target for `global`/`global_remove`.
    pub fn track_registry(&mut self, client: ClientId, registry: ObjectId) {
        self.live_registries.insert(registry, client);
    }

    /// A registry object died (destroy or revocation): it leaves the
    /// fan-out set.
    pub fn untrack_registry(&mut self, registry: ObjectId) {
        self.live_registries.remove(&registry);
    }

    /// A global was bound (Phase 44): the object joins the withdrawal
    /// sweep's candidate set.
    pub fn track_global_bind(&mut self, client: ClientId, object: ObjectId, interface: &str) {
        self.bound_globals
            .insert(object, (client, interface.into()));
    }

    /// A global-bound object died (destroy or revocation): it leaves
    /// the sweep set.
    pub fn untrack_global_bind(&mut self, object: ObjectId) {
        self.bound_globals.remove(&object);
    }

    /// A client bound the shell global (Phase 51): the object joins
    /// the `workspace_switched` broadcast set.
    pub fn track_shell_bind(&mut self, client: ClientId, shell: ObjectId) {
        self.shell_binds.insert(shell, client);
    }

    /// A shell bind died (destroy): it leaves the broadcast set.
    pub fn untrack_shell_bind(&mut self, shell: ObjectId) {
        self.shell_binds.remove(&shell);
    }

    /// Withdraw `interface` from the live advertisement set (Phase 44,
    /// the dynamic-globals mechanism).
    ///
    /// The ordering is the spec's own sentence, served exactly:
    /// *every live object of that interface is revoked first* (each
    /// carrying `revoked_reason::interface_removed`), *then* every
    /// live registry object receives `registry.global_remove` — the
    /// removal barrier. Both ride the same per-client FIFO, so the
    /// revocations deliver ahead of the barrier; the client's
    /// Control-class lanes keep the wire order.
    ///
    /// Idempotent: withdrawing an already-withdrawn interface is a
    /// no-op (the sweep ran when the first withdrawal did).
    pub fn withdraw_global(&mut self, interface: &str) {
        if !self.withdrawn_globals.insert(interface.into()) {
            return;
        }
        let reason = ldp_protocol::generated::core::RevokedReason::InterfaceRemoved.to_wire();
        // 1. Revoke every live bound object of that interface.
        let targets: Vec<(ClientId, ObjectId)> = self
            .bound_globals
            .iter()
            .filter(|(_, (_, iface))| iface.as_ref() == interface)
            .map(|(object, (client, _))| (*client, *object))
            .collect();
        for (client, object) in targets {
            self.bound_globals.remove(&object);
            self.outboxes
                .push(crate::outbox::OutboxEntry::revoke(client, object, reason));
        }
        // 2. The removal barrier on every live registry, every client.
        let registries: Vec<(ClientId, ObjectId)> = self
            .live_registries
            .iter()
            .map(|(object, client)| (*client, *object))
            .collect();
        for (client, registry) in registries {
            self.outboxes.push(crate::outbox::OutboxEntry::event(
                client,
                registry,
                "global_remove",
                Vec::new(),
            ));
        }
    }

    /// Re-advertise `interface` into the live advertisement set (Phase
    /// 44): every live registry object receives `registry.global`
    /// (the interface's true version range, read off the compiled
    /// schema — never guessed), and fresh binds are served again.
    /// Idempotent: re-advertising a live global is a no-op.
    pub fn readvertise_global(&mut self, interface: &str) {
        if !self.withdrawn_globals.remove(interface) {
            return;
        }
        let Some(iface) = ldp_protocol::REGISTRY.interface(interface) else {
            return;
        };
        let args = vec![
            ldp_core::wire::Value::String(iface.name.into()),
            ldp_core::wire::Value::Uint32(iface.version_min),
            ldp_core::wire::Value::Uint32(iface.version_max),
        ];
        let registries: Vec<(ClientId, ObjectId)> = self
            .live_registries
            .iter()
            .map(|(object, client)| (*client, *object))
            .collect();
        for (client, registry) in registries {
            self.outboxes.push(crate::outbox::OutboxEntry::event(
                client,
                registry,
                "global",
                args.clone(),
            ));
        }
    }

    /// The primary output (slot 0) — the pacing grid's, the shell's,
    /// and the capture surface's owner. `None` while dark.
    #[must_use]
    pub fn primary(&self) -> Option<&OutputSlot> {
        self.outputs.first()
    }

    /// The primary output's protocol face — the single-output world's
    /// `output` field, as a method.
    #[must_use]
    pub fn output(&self) -> Option<&OutputGlobal> {
        self.outputs.first().map(|slot| &slot.output)
    }

    /// The CRTC the primary output scans out on.
    #[must_use]
    pub fn crtc(&self) -> Option<CrtcId> {
        self.outputs.first().map(|slot| slot.crtc)
    }

    /// The factor a *newly lit* output advertises (Phase 37): the
    /// per-output doctrine's last entry (the stretch rule — the
    /// operator's list never runs out), or the primary's own factor
    /// when the list is empty (the Phase 31 single-factor doctrine).
    /// This is the rule the hotplug arm applies; before Phase 37 a
    /// newcomer silently reset to identity, losing the operator's
    /// `--scale` mid-session (the bug the rule fixes).
    #[must_use]
    pub fn newcomer_scale(&self) -> ScaleFactor {
        self.output_scales.last().copied().unwrap_or_else(|| {
            self.primary()
                .map_or(ScaleFactor::IDENTITY, |s| s.output.scale)
        })
    }

    /// Begin a window-open transition on a freshly mapped surface
    /// (Phase 47, the compositor-owned choreography). The server's
    /// own invariants decide eligibility, never the client: the host
    /// must be enabled (the config switch), a popup never animates
    /// (the menu doctrine — instant appearance is the popup's
    /// contract), and a surface claiming an ephemeral role (tooltip,
    /// overlay) appears instantly whatever the catalog would say.
    pub fn begin_window_open(&mut self, surface: SurfaceId, now: Mono) {
        if !self.scene.transitions.enabled || self.popups.is_popup_surface(surface) {
            return;
        }
        let role = self
            .scene
            .semantics_of(surface)
            .role
            .unwrap_or(SemanticRole::Window);
        if !role.transitions_eligible() {
            return;
        }
        let now_ms = now.as_ns() / 1_000_000;
        self.scene
            .transitions
            .begin(surface, TransitionKind::WindowOpen, now_ms);
    }

    /// Begin a window-close fade on a surface about to leave the
    /// desktop (Phase 48): the owned ghost of its last raster fades
    /// out under the catalog's close spring. The server's own
    /// invariants decide eligibility, never the client — the same
    /// rules the open fade serves (popups dismiss instantly; the
    /// ephemeral roles leave no ghost), plus the one the close adds:
    /// there must be *content* to ghost (a mapped buffer). With
    /// transitions off (the library default) this is a no-op — the
    /// destroy behaves exactly as it always has, byte-identical.
    ///
    /// The capture owns a row-tight copy of the window's ink: the
    /// pool's mapping is the client's to reuse the moment the
    /// release lands, so the ghost's pixels must be the compositor's
    /// own (the macOS snapshot doctrine — the fade rides a copy, the
    /// client is already gone).
    ///
    /// Phase 57: a server-decorated window's capture also freezes its
    /// chrome's shape truth (the frame, the raster's shape key, the
    /// strip) — the band's ink is stateless and rides the desktop's
    /// own cache, so the whole frame leaves as one ghost. A window
    /// that wore no chrome ghosts content-only, exactly as before.
    ///
    /// # Panics
    ///
    /// Never: the tight geometry is a row-tight copy of a geometry
    /// the buffer's own creation validated (the expect mirrors every
    /// checked-then-built invariant in the stack).
    #[allow(clippy::too_many_lines)]
    pub fn begin_window_close(&mut self, surface: SurfaceId, now: Mono) {
        if !self.scene.transitions.enabled || self.popups.is_popup_surface(surface) {
            return;
        }
        // Phase 49: a states-arm-hidden window (minimized, off-space)
        // has no ink on the canvas — there is nothing to ghost, and
        // the vacate claims already own its rect.
        if self.scene.hidden.contains(&surface) {
            return;
        }
        let role = self
            .scene
            .semantics_of(surface)
            .role
            .unwrap_or(SemanticRole::Window);
        if !role.transitions_eligible() {
            return;
        }
        // The dying window's own truth: a route with a committed
        // buffer (cloned out — the Arc keeps the resource alive
        // through the capture), the buffer's pool, and the mapped
        // node.
        let Some((_, buf)) = self
            .scene
            .routes
            .get(&surface)
            .and_then(|route| route.buffer.clone())
        else {
            return;
        };
        let pool_key = ObjectKey {
            client: buf.pool().client,
            object: buf.pool().object,
        };
        // The scene's own snapshot: the node's geometry, transform,
        // and color — the truth the last frame rendered (an owned
        // value; the borrows below are its own, never the scene's).
        let snapshot = self.scene.snapshot();
        let Some(node) = snapshot.node(surface) else {
            return;
        };
        if !node.mapped {
            return;
        }
        // The z slot: how many *rendered* layers (mapped, routed —
        // the grade walk's own membership, hidden surfaces excluded)
        // stood before the dying window in the render order. The
        // ghost inserts there, keeping the window's own place in the
        // stack — a fading window never jumps above the windows that
        // were above it.
        let mut z = 0usize;
        for id in snapshot.render_order() {
            if *id == surface {
                break;
            }
            if snapshot.node(*id).is_some_and(|n| n.mapped)
                && self.scene.routes.contains_key(id)
                && !self.scene.hidden.contains(id)
            {
                z += 1;
            }
        }
        let (dest, transform, color, hdr) = (node.bounds, node.transform, node.color, node.hdr);
        // The style the last frame resolved, frozen at death: the
        // output the window lived on (its center's container — the
        // primary otherwise) is the resolution frame, exactly the
        // grade walk's own.
        let center =
            ldp_core::geometry::Point::new(dest.x + dest.w as i32 / 2, dest.y + dest.h as i32 / 2);
        let bounds = self
            .outputs
            .iter()
            .find(|slot| slot.output.bounds().contains_point(center))
            .map_or_else(
                || {
                    self.outputs
                        .first()
                        .map_or(Rect::new(0, 0, 0, 0), |slot| slot.output.bounds())
                },
                |slot| slot.output.bounds(),
            );
        let style = crate::frame_loop::surface_style(
            self.effects,
            node,
            bounds,
            false,
            self.scene.material_requests.get(&surface).copied(),
        );
        // Phase 57 — the chrome ghost's capture: a server-decorated
        // window's band rides the close fade. The chrome's truth is
        // frozen at death exactly as the style froze: the frame the
        // band wore, the shape the raster cache keys on (the insets,
        // the scaled close metrics, the Liquid variant), and the
        // strip if a title served. The ink itself is never copied —
        // the band's ink is stateless (the shape is its whole
        // truth), so the ghost renders from the same chrome cache
        // the living desktop reads; the capture only needs the keys.
        // `None` (plain, CSD, roleless, never-applied, fullscreen's
        // zero insets) leaves the ghost content-only — every
        // pre-Phase-57 byte stands.
        let chrome = {
            let scale = self
                .outputs
                .first()
                .map_or(ScaleFactor::IDENTITY, |slot| slot.output.scale);
            crate::shell::chrome_geometry(&self.toplevels, surface, dest).map(|(frame, insets)| {
                let liquid = self.effects != ldp_renderer::EffectTier::Minimal;
                GhostChrome {
                    strip: crate::shell::title_strip(
                        &self.toplevels,
                        surface,
                        frame,
                        insets,
                        scale,
                    ),
                    frame,
                    shape: crate::shell::ChromeShape::of(frame, insets, scale, liquid),
                }
            })
        };
        // The owned ink: a row-tight copy of the window's last
        // raster (the SHM family is 32-bit; the tight geometry is
        // the copy's own, validated against the copy's own size).
        // The pool's mapping is the client's to reuse the moment
        // the release lands — the copy is the only honest carrier.
        let Some(pool) = self.scene.pools.get(&pool_key) else {
            return;
        };
        let geometry = buf.geometry(pool.size());
        let plane = &geometry.planes()[0];
        let (offset, stride) = (plane.offset as usize, plane.stride as usize);
        let (w, h) = (buf.width() as usize, buf.height() as usize);
        let bytes = pool.bytes();
        if offset + stride * (h - 1) + w * 4 > bytes.len() {
            return; // the pool shrank under us (impossible by doctrine; honest exit)
        }
        let mut ink = Vec::with_capacity(w * h * 4);
        for row in 0..h {
            let start = offset + row * stride;
            ink.extend_from_slice(&bytes[start..start + w * 4]);
        }
        let tight = BufferGeometry::new(
            w as u32,
            h as u32,
            buf.format(),
            ldp_core::buffer::Modifier::LINEAR,
            &[PlaneLayout {
                offset: 0,
                stride: (w * 4) as u32,
            }],
            (w * h * 4) as u64,
        )
        .expect("a row-tight copy of a validated geometry is valid");
        let now_ms = now.as_ns() / 1_000_000;
        self.scene.ghosts.push(Ghost {
            id: 0, // the host assigns the identity
            dest,
            ink,
            geometry: tight,
            transform,
            color,
            hdr,
            style,
            z,
            chrome,
            transition: Transition::close(TransitionKind::WindowClose, now_ms),
        });
        self.scene.dirty = true;
    }

    /// The policy inputs for a surface's next toplevel proposal
    /// (Phase 49): the desktop's own truths — the usable area (the
    /// logical workspace), the primary output's logical size and
    /// scale, the LionOS chrome metrics, the surface's decoration
    /// choice, and its home space. The output pin is the caller's
    /// (the fullscreen arm resolves it; the other verbs propose
    /// unpinned).
    #[must_use]
    pub fn toplevel_policy(
        &self,
        surface: SurfaceId,
        output: Option<ObjectId>,
    ) -> ldp_shell::toplevel::PolicyInputs {
        let key = ldp_shell::WindowKey::new(surface.raw());
        let workspace = self.spaces.space_of(key).unwrap_or(0);
        let primary = self.outputs.first();
        ldp_shell::toplevel::PolicyInputs {
            workspace_area: self.shell.layout.usable,
            output_size: primary.map_or((0, 0), |slot| slot.output.logical_size()),
            metrics: ldp_shell::ssd::SsdMetrics::LION,
            decoration: self
                .toplevels
                .by_surface(surface)
                .map_or(ldp_shell::ssd::DecorationMode::Client, |t| t.decoration),
            scale: primary.map_or(ScaleFactor::IDENTITY, |slot| slot.output.scale),
            workspace,
            output,
        }
    }

    /// The states arm's visibility sync (Phase 49): recompute whether
    /// the surface belongs on the desktop — minimized, or homed on a
    /// space the seat is not viewing (sticky windows show everywhere)
    /// — and apply the change through [`World::set_surface_hidden`].
    /// The machine's flags and the spaces machine are the two truths;
    /// this is the one place they meet the render path. Phase 51: a
    /// *dialog's* space is its parent's (the sheet moves with its
    /// window — the spaces machine tracks toplevels only, so the
    /// parent's assignment answers for the role).
    pub fn sync_states_visibility(&mut self, surface: SurfaceId) {
        let key = ldp_shell::WindowKey::new(surface.raw());
        let minimized = self
            .toplevels
            .by_surface(surface)
            .is_some_and(|t| t.machine.wanted().minimized());
        let space_key = if self.toplevels.by_surface(surface).is_none() {
            self.dialogs
                .by_surface_ids(surface)
                .map(|(_, _, parent)| ldp_shell::WindowKey::new(parent.raw()))
        } else {
            None
        }
        .unwrap_or(key);
        let on_space = self
            .spaces
            .visible_to(SPACE_SEAT, space_key)
            .unwrap_or(true);
        let now_hidden = minimized || !on_space;
        let was_hidden = self.scene.hidden.contains(&surface);
        if now_hidden != was_hidden {
            self.set_surface_hidden(surface, now_hidden);
        }
    }

    /// Phase 51 — the view switch (the taskbar's arm): move the seat's
    /// viewed space and sync every window's visibility. Windows homed
    /// on the newly-viewed space show, the others hide (sticky
    /// windows show everywhere — the spaces machine's own query);
    /// the dialogs follow their parents (the visibility sync's own
    /// doctrine). No window's *own* state is proposed — the only
    /// configures the switch can carry are the focus truth's own
    /// (the frontmost window homed on the newly-viewed space takes
    /// the keys, the old holder releases them — the `activated` bit
    /// riding the transitions); the hidden spaces' frame requests
    /// park through the occlusion quiescer's own machinery. A switch
    /// to the space already viewed is a full no-op (the seat's truth
    /// did not change — nothing sweeps, nothing is emitted; `None`
    /// returns). Returns the *actual* space (the clamp's honest
    /// answer) and the shell binders that must learn it (the
    /// `workspace_switched` broadcast — every client that bound the
    /// shell global).
    pub fn switch_workspace(&mut self, index: u32) -> Option<(u32, Vec<(ClientId, ObjectId)>)> {
        let before = self.spaces.active_of(SPACE_SEAT).unwrap_or(0);
        let actual = self.spaces.switch(SPACE_SEAT, index).unwrap_or(0);
        if actual == before {
            // The view already shows this space: the seat's truth did
            // not change, and neither does anything else.
            return None;
        }
        // The visibility sweep: every toplevel by its own space
        // truth, every dialog by its parent's. The per-hide focus
        // promotion stands down for the sweep's duration (the
        // intermediate topmost is never the answer — the end
        // promotion below is: one transition, the honest target).
        self.view_sweep = true;
        let mut surfaces: Vec<SurfaceId> = self.toplevels.surface_ids();
        surfaces.extend(self.dialogs.surface_ids());
        for surface in surfaces {
            self.sync_states_visibility(surface);
        }
        self.view_sweep = false;
        // The keys land on the frontmost window homed on the
        // newly-viewed space (or the frontmost visible, when the
        // space is empty — a sticky window's own truth).
        self.promote_focus_on_space(actual);
        let binders = self
            .shell_binds
            .iter()
            .map(|(object, client)| (*client, *object))
            .collect();
        Some((actual, binders))
    }

    /// Apply one hide/unhide transition (Phase 49): membership, the
    /// repaint claim (the vacate doctrine — the ink a hidden window
    /// leaves behind is the compositor's to repaint, and the ink an
    /// unhidden window restores is equally ours), the render trigger,
    /// and the focus discipline (a hidden window never holds the
    /// keyboard — the leave rides the focus retarget, the same wire
    /// the modal gate uses). The enter/leave_output events and the
    /// frame-request parking are the visibility pass's own (the next
    /// pass clears the hidden surface's output set; App Nap's
    /// machinery does the rest).
    pub fn set_surface_hidden(&mut self, surface: SurfaceId, hidden: bool) {
        // The ink on the canvas at the last pass — what the hide
        // removes and the unhide restores. An unmapped window owns
        // no ink (the mapping commit's own pass claims it).
        let bounds = self
            .scene
            .tree
            .get(surface)
            .filter(|s| s.state().is_mapped())
            .map(Surface::last_bounds);
        if hidden {
            self.scene.hidden.insert(surface);
        } else {
            self.scene.hidden.remove(&surface);
        }
        // The claim: both directions repaint the window's own rect
        // (leaving it behind is stale ink; landing on it unseen is
        // worse).
        if let Some(rect) = bounds {
            if rect.w > 0 && rect.h > 0 {
                self.scene.hide_claims.push(rect);
            }
        }
        self.scene.dirty = true;
        // The focus discipline: a hidden window takes no keys, and
        // the keys land on the frontmost window that remains (the
        // desktop's own truth — Phase 51's promotion doctrine; the
        // retarget emits the leave over the wire, the dialog gate's
        // own seam). The view switch's sweep stands this down — its
        // own end-of-sweep promotion is the one authority.
        if hidden && !self.view_sweep && self.input.keyboard_focus() == Some(surface) {
            self.promote_focus();
        }
    }

    /// The states arm's realization (Phase 49): the commit that
    /// follows an ack. The machine's `commit()` returns the applied
    /// proposal; the geometry it carries lands *now* — the buffer
    /// this commit attached is the client's size answer, so the
    /// position completes the placement in the same breath (the
    /// migration arm's `set_position_now` doctrine: the damage
    /// engine's R2 rule repaints both ends).
    ///
    /// The restore point: the position the window held when it first
    /// engaged a server-geometry state, captured once and cleared
    /// when the last geometry state releases — a chain of geometry
    /// verbs returns to the one point where server geometry began.
    pub fn toplevel_realize(&mut self, surface: SurfaceId) {
        let Some(applied) = self
            .toplevels
            .by_surface_mut(surface)
            .and_then(|t| t.machine.commit())
        else {
            return;
        };
        let geometry_state = applied.states.fullscreen() || applied.states.maximized();
        let current = self.scene.tree.get(surface).map(Surface::position);
        if geometry_state {
            // Where server geometry puts the window: fullscreen at
            // the pinned output's origin (the client's own bound
            // object resolves the slot; the primary's otherwise);
            // maximized at the workspace area's origin (physical —
            // the tree's space).
            let (x, y) = if applied.states.fullscreen() {
                let slot = applied
                    .output
                    .and_then(|obj| {
                        self.scene
                            .output_binds
                            .iter()
                            .find(|(_, bound)| **bound == obj)
                            .and_then(|((_, crtc), _)| {
                                self.outputs.iter().find(|s| s.crtc == *crtc)
                            })
                    })
                    .or_else(|| self.outputs.first());
                slot.map_or((0, 0), |s| (s.output.layout.0, s.output.layout.1))
            } else {
                let usable = self.shell.usable_physical();
                // Phase 52: a server-decorated window maximizes with
                // its *frame* filling the workspace area — the
                // content sits inset by the chrome the machine
                // applied (the title band and the border ring stay
                // on-screen; a client-decorated window's buffer is
                // its own whole footprint, its reserved insets live
                // *inside* it, so the area's origin is its position
                // — every Phase 49 CSD pin unchanged).
                let chrome = self
                    .toplevels
                    .by_surface(surface)
                    .filter(|t| t.decoration == ldp_shell::ssd::DecorationMode::Server)
                    .map_or((0, 0), |_entry| {
                        (applied.insets.left as i32, applied.insets.top as i32)
                    });
                (usable.x + chrome.0, usable.y + chrome.1)
            };
            let restore = self
                .toplevels
                .by_surface(surface)
                .and_then(|t| t.restore)
                .or(current);
            if let Some(entry) = self.toplevels.by_surface_mut(surface) {
                entry.restore = restore;
                // The geometry regime begins: any drag-held position is
                // void (the verbs supersede the operator's hand).
                entry.drag_pos = None;
            }
            if current != Some((x, y)) {
                self.scene.tree.set_position_now(surface, x, y).ok();
                self.scene.dirty = true;
            }
        } else if let Some(entry) = self.toplevels.by_surface_mut(surface) {
            // Phase 50 — the operator's hand: a drag-minted proposal
            // realizes with the position the drag holds for that size
            // (the engaged left/top edges' anchor — the position and
            // the client's committed buffer land together, never
            // tearing); the restore point is *not* consumed (the
            // window never held a geometry state during the resize —
            // a later un-verb still returns there).
            if let Some((x, y)) = entry.drag_pos.take() {
                if current != Some((x, y)) {
                    self.scene.tree.set_position_now(surface, x, y).ok();
                    self.scene.dirty = true;
                }
            } else if let Some((x, y)) = entry.restore.take() {
                // The un-verb: return to the restore point, if one was
                // ever taken (a window that was never server-geometried
                // keeps its position — nothing to realize).
                if current != Some((x, y)) {
                    self.scene.tree.set_position_now(surface, x, y).ok();
                    self.scene.dirty = true;
                }
            }
        }
    }

    /// The security-aware capture's redaction pass (Phase 47): every
    /// *mapped* surface whose effective security class redacts paints
    /// its rectangle opaque black in the captured words — the display
    /// keeps showing the content (the user's own eyes), the
    /// client-visible frame does not. The primary's layout origin is
    /// the desktop's (the coordinate truth the render path shares),
    /// so the mapping is the render path's own translate-and-clip.
    ///
    /// Returns the number of redacted rectangles (the capture test's
    /// introspection oracle).
    pub fn redact_protected(&mut self, words: &mut [u32], w: u32, h: u32) -> usize {
        if self.scene.semantics.is_empty() {
            return 0;
        }
        let layout = self
            .outputs
            .first()
            .map_or((0, 0), |slot| slot.output.layout);
        let snapshot = self.scene.snapshot();
        let mut redacted = 0;
        for id in snapshot.render_order() {
            let Some(node) = snapshot.node(*id) else {
                continue;
            };
            if !node.mapped {
                continue;
            }
            if self
                .scene
                .semantics_of(*id)
                .effective_security()
                .capture_rule()
                != ldp_compositor::semantics::CaptureRule::Redact
            {
                continue;
            }
            let local = node.bounds.translate(-layout.0, -layout.1);
            let (w, h) = (
                i32::try_from(w).unwrap_or(i32::MAX),
                i32::try_from(h).unwrap_or(i32::MAX),
            );
            let x0 = local.x.max(0).min(w);
            let y0 = local.y.max(0).min(h);
            let x1 = (local.x + i32::try_from(local.w).unwrap_or(i32::MAX)).clamp(0, w);
            let y1 = (local.y + i32::try_from(local.h).unwrap_or(i32::MAX)).clamp(0, h);
            let w = w as u32 as usize;
            for y in y0..y1 {
                let row = y as usize * w;
                for x in x0..x1 {
                    words[row + x as usize] = 0xFF00_0000;
                }
            }
            redacted += 1;
        }
        redacted
    }

    /// The renderer-selection report ("gles (hardware: …)" or
    /// "software (GL unavailable: …)") — what the startup printed.
    #[must_use]
    pub fn renderer_decision_report(&self) -> &str {
        &self.renderer_report
    }

    /// The Liquid effects report ("minimal (plain pixels…)", "high
    /// (3-pass blur frost…)") — the resolved tier and what it serves,
    /// the startup's second line.
    #[must_use]
    pub fn effects_decision_report(&self) -> String {
        self.effects.report().to_owned()
    }

    /// The positioning shell's report ("phone layout, dock 84 px at
    /// the bottom") — the resolved layout doctrine, the startup's
    /// third line (Phase 28).
    #[must_use]
    pub fn shell_decision_report(&self) -> String {
        self.shell.report()
    }

    /// The adaptive-sync quirk report (Phase 41) — the startup's VRR
    /// line: the arming, the floor pass's audit trail (one fragment
    /// per output, in order), and the uniform collapse when it holds.
    /// The honest shape an operator reads after typing
    /// `--vrr --vrr-floor 57`.
    #[must_use]
    pub fn vrr_quirk_report(&self) -> String {
        if !self.vrr_enabled {
            return "off (the fixed nominal grid)".to_owned();
        }
        let mut parts: Vec<String> = Vec::new();
        for outcome in &self.vrr_floor_outcomes {
            parts.push(match outcome {
                ldp_vrr::FloorOutcome::Passthrough => "-".to_owned(),
                other => other.report(),
            });
        }
        let floors = if parts.is_empty() {
            String::new()
        } else {
            format!("; floors [{}]", parts.join(", "))
        };
        if self.vrr_collapse {
            format!("uniform fixed sync (the mixed-desktop collapse — no output arms){floors}")
        } else {
            format!("on (per-output VRR){floors}")
        }
    }
}
