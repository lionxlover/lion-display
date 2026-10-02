//! Live output re-arrangement — Phase 26, extended by Phase 31.
//!
//! The serve loop of Phase 25 selects its pipeline *once* and serves
//! it until exit. Real topologies do not sit still: cables pull,
//! docks land, sinks re-negotiate their mode lists. This module is
//! the compositor's answer — the serve loop's hotplug arm:
//!
//! * [`World::rearrange`] — the decision. A hotplug signal means
//!   "re-probe the whole topology" (one udev event can mean several
//!   connector changes — an MST hub rewires its entire tree), so the
//!   world snapshots every connector's status and re-asks the
//!   selection question: *which pipelines should be served?* The
//!   single-output doctrine (the library default) asks Phase 26's one
//!   question and maps to exactly one of its four transitions:
//!
//!   | live | re-probe | transition                                        |
//!   |------|----------|---------------------------------------------------|
//!   | yes  | same     | [`Rearrange::Spurious`] — status noise, no action |
//!   | yes  | other    | [`Rearrange::Migrated`] — swap the live pipeline  |
//!   | yes  | none     | [`Rearrange::Dark`] — the honest dark state       |
//!   | no   | some     | [`Rearrange::Relit`] — first light after dark     |
//!
//!   The multi-output doctrine (Phase 31) asks the allocator's
//!   question — *every* pipeline — and diffs the sets, so a second
//!   monitor joining is [`Rearrange::OutputAdded`] (the desktop
//!   extends; nothing restarts), a non-primary leaving is
//!   [`Rearrange::OutputRemoved`] (the survivors re-flow), and a
//!   many-at-once rewiring is [`Rearrange::Rewired`].
//!
//! * The *stability rule*: the served set changes only when it must.
//!   In single-output mode a connector appearing while the served one
//!   still lives is topology noise (no migration to a "better"
//!   output); in multi-output mode the living pipelines never move —
//!   additions append, removals subtract, and a survivor keeps its
//!   CRTC, its plane, and its scanout chain untouched.
//!
//! * The migration choreography is Phase 25's vocabulary, replayed:
//!   the applied disable (its teardown mirror), the scanout objects
//!   released (framebuffers, dumb buffers, mappings dropped), the new
//!   chain allocated, the applied enable, the bring-up flip latching
//!   the scheduler's fresh timeline
//!   ([`FrameScheduler::reanchor`](ldp_compositor::scheduler::FrameScheduler::reanchor)),
//!   and a full-scene re-render — the desktop *survives* the swap
//!   with every client session intact.
//!
//! * The client story is the protocol's revocation vocabulary: the
//!   output object a client holds mirrored a pipeline that no longer
//!   exists, so it is revoked (`capability_revoked`) through the
//!   outbox — the same wake-point delivery every cross-client event
//!   uses. Clients re-bind `ldp.core.output` and receive the new
//!   cascade (geometry, modes, identity). Uniform on purpose: a mode
//!   renegotiation and a monitor swap are the same event to a client
//!   — "the output you knew is gone; here is the one that exists".
//!
//! * The dark state is honest, not empty: the pipelines are disabled,
//!   every kernel object released, pending buffer fences flushed
//!   (nothing will ever flip again, so nothing is still being read),
//!   the scheduler parks (pending frame requests died with
//!   `output_off`; new ones defer until light returns), and the
//!   protocol keeps serving — clients connect, bind, commit; their
//!   content waits in the tree for the first re-plug. When it comes,
//!   [`Rearrange::Relit`] re-runs the bring-up and the whole desktop
//!   paints onto the new display without a single session lost.

#![forbid(unsafe_code)]

use ldp_compositor::surface::SurfaceId;
use ldp_core::buffer::{FourCC, Modifier};
use ldp_core::geometry::Rect;
use ldp_core::ids::{ClientId, ObjectId};
use ldp_core::wire::Value;
use ldp_display::backend::KmsBackend;
use ldp_display::events::DeviceEvent;
use ldp_display::fb::FbSpec;
use ldp_display::hotplug::topology_statuses;
use ldp_display::ids::{ConnectorId, CrtcId, PlaneId};
use ldp_display::serve::{self, Pipeline};
use ldp_display::ConnectorStatus;
use ldp_protocol::generated::core::RevokedReason;

use crate::frame_loop::FrameError;
use crate::outbox::OutboxEntry;
use crate::output::OutputGlobal;
use crate::scene::{DrmLease, OutputSlot, PendingRelease, ScanoutChain, World};
use crate::sys;

/// What one re-arrangement did. Carries the before/after output
/// identity (connector, kernel-style name, mode) for the serve loop's
/// report line and the tests' assertions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rearrange {
    /// A signal without consequence for the served pipeline: the
    /// topology diff was empty, or the re-probe selected the very
    /// pipeline already being served (status noise), or the world was
    /// dark and stayed dark.
    Spurious,
    /// The live pipeline was replaced: monitor swap, or a mode
    /// renegotiation on the same connector.
    Migrated {
        /// The output that was being served.
        from: Served,
        /// The output that is being served now.
        to: Served,
    },
    /// The last output went away: the pipeline is disabled, every
    /// object released, the protocol still serving.
    Dark {
        /// The output that was being served.
        from: Served,
    },
    /// The first output came back after a dark stretch.
    Relit {
        /// The output that is being served now.
        to: Served,
    },
    /// A display joined the desktop while others live (Phase 31's
    /// multi-output doctrine): its pipeline lights alongside the
    /// survivors — the logical desktop extends, no session restarts.
    OutputAdded {
        /// The output that joined.
        to: Served,
    },
    /// A display left the desktop while others live: its pipeline
    /// stops, its binds revoke, the desktop re-flows onto the
    /// survivors.
    OutputRemoved {
        /// The output that left.
        from: Served,
    },
    /// Several changes at once (an MST hub rewiring its tree):
    /// additions, removals, and migrations applied in one re-probe,
    /// reported together.
    Rewired {
        /// The outputs that joined.
        added: Vec<Served>,
        /// The outputs that left.
        removed: Vec<Served>,
        /// The outputs that re-negotiated (same connector, new truth).
        migrated: Vec<(Served, Served)>,
    },
}

/// One served output's identity, as a re-arrangement reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Served {
    /// The connector behind the output.
    pub connector: ConnectorId,
    /// The kernel-style output name (`eDP-1`).
    pub name: String,
    /// The active mode's width in pixels.
    pub width: u32,
    /// The active mode's height in pixels.
    pub height: u32,
}

impl World {
    /// The re-arrangement decision: re-probe, diff, act. Called by the
    /// serve loop's device service whenever a hotplug event was seen
    /// (the whole-topology re-probe is the policy — one signal, every
    /// consequence).
    ///
    /// The topology snapshot is *always* refreshed (even a Spurious
    /// verdict remembers what the world just looked at), so the next
    /// diff is against current truth.
    ///
    /// # Errors
    ///
    /// [`FrameError`] — the disable, allocation, or enable of a new
    /// pipeline failed (fatal for the serve loop: display pipelines
    /// do not degrade silently). The one tolerated arm: a disable the
    /// device rejects *for a connector that is no longer connected*
    /// — the kernel tore that pipeline down with the connector; the
    /// mock's unbind-shaped commits never reach it.
    ///
    /// # Panics
    ///
    /// On the impossible arms of construction (`CrtcId::new` with the
    /// compile-time nonzero literal) — the same never-panics rule the
    /// rest of the module carries.
    pub fn rearrange(&mut self) -> Result<Rearrange, FrameError> {
        // The re-probe and selection question (the split helper —
        // the reasoning lives there).
        let (live, selection) = self.reselect();
        let (removed, added, migrated) = pipeline_diff(&live, &selection);
        let changes = removed.len() + added.len() + migrated.len();
        if changes == 0 {
            return Ok(Rearrange::Spurious);
        }
        // ---- the choreography (order is the doctrine) ----------------
        // Removals first: dying pipelines release their objects before
        // anything re-allocates (a mode change on the same CRTC needs
        // the stop-then-light order; an MST rewiring may hand a freed
        // CRTC to a new connector).
        let mut removed_served = Vec::new();
        for pipeline in &removed {
            removed_served.push(self.stop_output(pipeline)?);
            self.forget_output(pipeline.crtc);
            self.revoke_output_binds(Some(pipeline.crtc));
        }
        // Migrations: stop-and-light per re-negotiated connector.
        let mut migrated_served = Vec::new();
        for (from_pipeline, to_pipeline) in &migrated {
            let from = self.stop_output(from_pipeline)?;
            self.forget_output(from_pipeline.crtc);
            self.revoke_output_binds(Some(from_pipeline.crtc));
            let slot = self.light_output(to_pipeline)?;
            self.outputs.push(slot);
            let to = self.served_of(to_pipeline);
            migrated_served.push((from, to));
        }
        // Additions: light the newcomers alongside the survivors.
        let mut added_served = Vec::new();
        for pipeline in &added {
            let slot = self.light_output(pipeline)?;
            self.outputs.push(slot);
            added_served.push(self.served_of(pipeline));
        }
        // The layout always re-resolves after a set change (the
        // desktop's shape changed); binds whose outputs moved are
        // stale (geometry rode their cascade) — revoke them all, the
        // Phase 26 vocabulary, and let the clients re-bind.
        self.resolve_layouts();
        self.revoke_output_binds(None);
        // The primary's identity decides the re-anchoring: a new
        // primary (the old one left, or a fresh relight) re-runs the
        // positioning shell and re-anchors the scheduler at the new
        // nominal; a surviving primary keeps its grid.
        let primary_connector = self.outputs.first().map(|slot| slot.output.connector.id);
        let primary_changed =
            primary_connector.is_some_and(|c| live.first().map_or(true, |l| l.connector != c));
        let primary_geometry = self.outputs.first().map(|slot| {
            (
                slot.output.mode().clone(),
                slot.output.refresh(),
                slot.output.scale,
            )
        });
        if primary_changed || live.is_empty() {
            if let Some((mode, refresh, scale)) = primary_geometry {
                let (w, h) = (u32::from(mode.hdisplay), u32::from(mode.vdisplay));
                if self.shell.is_active() {
                    self.shell.relayout((w, h), scale, true);
                    self.replace_roots();
                }
                self.scene.scheduler.reanchor(refresh);
            }
        } else if !added.is_empty() || !migrated.is_empty() {
            // The primary survived: the desktop gained or lost area.
            // Full re-render everywhere (the survivors' frames are
            // stale against the new desktop damage space).
            for slot in &mut self.outputs {
                slot.pending.add(slot.output.bounds());
            }
        }
        // Land the bring-up flips of everything newly lit, and the
        // dark state's own choreography when nothing remains: the
        // scheduler parks (pending frame requests die with
        // `output_off`; new ones defer until light returns) and every
        // owed buffer release flushes now (nothing is being read
        // anymore).
        if self.outputs.is_empty() {
            let now = self.device.now();
            self.scene.scheduler.park(now);
            let entries = self.fire_releases()?;
            self.outboxes.extend(entries);
            // Phase 44: the dark state withdraws the output global —
            // the registry tells the truth ("no display right now")
            // instead of letting a bind learn it by revocation. Every
            // live output bind is already revoked above (the
            // `revoke_output_binds` sweeps ran); this adds the
            // registry-level removal barrier after them, in the spec's
            // own order.
            self.withdraw_global("ldp.core.output");
        } else {
            self.land_bringup_flip();
            // Phase 44: a relight (or any dark-to-lit transition)
            // re-advertises the output global — every live registry
            // object learns the displays are back, with the schema's
            // own version range. Idempotent: a world that was never
            // dark pays nothing (the withdrawn set was empty).
            self.readvertise_global("ldp.core.output");
        }
        self.scene.dirty = true;
        self.rearranges += 1;
        Ok(self.rearrange_verdict(&live, removed_served, added_served, migrated_served))
    }

    /// The verdict of one completed choreography (Phase 31's split of
    /// `rearrange`'s tail): the dark state outranks the counting, the
    /// first relight names the desktop's return, single-change turns
    /// map to the named transitions, and multi-change turns report
    /// the aggregate.
    fn rearrange_verdict(
        &self,
        live: &[Pipeline],
        mut removed_served: Vec<Served>,
        mut added_served: Vec<Served>,
        mut migrated_served: Vec<(Served, Served)>,
    ) -> Rearrange {
        // The dark state outranks the counting: no output remains.
        if self.outputs.is_empty() {
            let from = removed_served
                .first()
                .cloned()
                .unwrap_or_else(|| self.served().expect("the dark state started lit"));
            return Rearrange::Dark { from };
        }
        // First light after a dark stretch, however many outputs
        // came back at once: the desktop returned.
        if live.is_empty() && !added_served.is_empty() {
            return Rearrange::Relit {
                to: added_served.remove(0),
            };
        }
        // Single-change turns map to the named transitions (every
        // Phase 26 test keeps its meaning); multi-change turns report
        // together.
        match (
            removed_served.len(),
            added_served.len(),
            migrated_served.len(),
        ) {
            (1, 1, 0) => Rearrange::Migrated {
                from: removed_served.remove(0),
                to: added_served.remove(0),
            },
            (0, 0, 1) => {
                let (from, to) = migrated_served.remove(0);
                Rearrange::Migrated { from, to }
            }
            (0, 1, 0) => Rearrange::OutputAdded {
                to: added_served.remove(0),
            },
            (1, 0, 0) => Rearrange::OutputRemoved {
                from: removed_served.remove(0),
            },
            _ => Rearrange::Rewired {
                added: added_served,
                removed: removed_served,
                migrated: migrated_served,
            },
        }
    }

    /// Stop one output's pipeline and release everything it owned: the
    /// applied disable commit (the Phase 25 teardown mirror), the
    /// in-flight flip abandoned (it will never land), the
    /// framebuffers removed, the DRM lease's dumb buffers destroyed —
    /// the mappings drop with the chain. Returns the output's served
    /// identity for the verdict.
    ///
    /// # Errors
    ///
    /// The disable's typed rejection — fatal for the caller — *unless*
    /// the connector is no longer connected (the kernel tore that
    /// pipeline down with the connector; the disable is history
    /// repeating itself, not a failure to act on).
    /// The re-probe and the selection question (Phase 31's split of
    /// `rearrange`'s head): snapshot the topology (the "before" for
    /// the next signal — always refreshed, even a Spurious verdict
    /// remembers what the world just looked at), re-ask the selection
    /// question under the doctrine in force, and reconcile the
    /// allocator's fresh walk against the living pipelines (a living
    /// connector keeps its CRTC and plane; only a mode change moves
    /// it; newcomers avoid the taken resources). Returns `(live,
    /// selection)` — the diff's two sides.
    fn reselect(&mut self) -> (Vec<Pipeline>, Vec<Pipeline>) {
        // The *pipeline comparison* decides — not the status diff,
        // because a sink can re-negotiate its mode list without any
        // status change at all (same connector, same Connected,
        // different modes: the diff would be empty while the pipeline
        // is stale). The diff vocabulary itself lives in
        // `ldp_display::hotplug` for the registry layer's future use.
        self.topology = topology_statuses(self.device.as_ref());
        // The selection: the doctrine decides the question. Single
        // (the library default) asks Phase 26's one question; multi
        // (Phase 31) asks the allocator's — every pipeline the device
        // can serve at once. The sized doctrine (Phase 30) rides both:
        // a forced size is preferred per-connector, and a total miss
        // falls back to the unsized doctrine (never a silent
        // nearest-neighbor guess). Phase 42: the foundry rides the
        // same way — every connector pours its own user-defined
        // timing, each gated by its own EDID range limits, and a
        // refusing connector (or a total miss) falls back to the
        // unsized doctrine (the migration never dies on the
        // operator's preference).
        let selection: Vec<Pipeline> = if self.multi_output {
            match (self.resolution, self.synth) {
                (_, Some(synth)) => serve::select_pipelines_synth(self.device.as_ref(), synth)
                    .or_else(|_| serve::select_pipelines(self.device.as_ref(), None))
                    .unwrap_or_default(),
                (Some(size), None) => serve::select_pipelines(self.device.as_ref(), Some(size))
                    .or_else(|_| serve::select_pipelines(self.device.as_ref(), None))
                    .unwrap_or_default(),
                (None, None) => {
                    serve::select_pipelines(self.device.as_ref(), None).unwrap_or_default()
                }
            }
        } else {
            match (self.resolution, self.synth) {
                (_, Some(synth)) => serve::select_pipeline_synth(self.device.as_ref(), synth)
                    .ok()
                    .or_else(|| serve::select_pipeline(self.device.as_ref())),
                (Some(size), None) => {
                    serve::select_pipeline_sized(self.device.as_ref(), Some(size))
                        .ok()
                        .or_else(|| serve::select_pipeline(self.device.as_ref()))
                }
                (None, None) => serve::select_pipeline(self.device.as_ref()),
            }
            .into_iter()
            .collect()
        };
        // The live pipelines, as the diff's "before" — remembered before
        // the choreography touches anything.
        let live: Vec<Pipeline> = self
            .outputs
            .iter()
            .map(|slot| Pipeline {
                connector: slot.output.connector.id,
                crtc: slot.crtc,
                plane: slot.plane,
                mode: slot.output.mode().clone(),
            })
            .collect();
        // The stability reconciliation: a living connector keeps its
        // CRTC and its plane whatever the allocator's fresh walk says
        // (a freed CRTC earlier in resource order may look "better" to
        // the allocator — the desktop must not notice; only a *mode*
        // change moves a living pipeline). New connectors then avoid
        // the resources the living ones hold.
        let mut selection = selection;
        for p in &mut selection {
            if let Some(l) = live.iter().find(|l| l.connector == p.connector) {
                p.crtc = l.crtc;
                p.plane = l.plane;
            }
        }
        for index in 0..selection.len() {
            if live
                .iter()
                .any(|l| l.connector == selection[index].connector)
            {
                continue; // living pipelines keep their resources
            }
            let taken: Vec<CrtcId> = live
                .iter()
                .map(|l| l.crtc)
                .chain(
                    selection
                        .iter()
                        .enumerate()
                        .filter(|(i, _q)| *i != index)
                        .map(|(_, q)| q.crtc),
                )
                .collect();
            if taken.contains(&selection[index].crtc) {
                // The allocator handed this newcomer a CRTC a living (or
                // already-planned) pipeline holds: reallocate honestly —
                // the first unused CRTC with an unused primary plane.
                let reallocated = self.reallocate_around(&taken);
                match reallocated {
                    Some((crtc, plane)) => {
                        selection[index].crtc = crtc;
                        selection[index].plane = plane;
                    }
                    None => {
                        // Nothing free: the newcomer stays unserved (the
                        // honest cap).
                        selection[index].crtc = CrtcId::new(0).expect("nonzero id");
                    }
                }
            }
        }
        selection.retain(|p| p.crtc.raw() != 0);
        (live, selection)
    }

    fn stop_output(&mut self, pipeline: &Pipeline) -> Result<Served, FrameError> {
        let served = self
            .outputs
            .iter()
            .find(|slot| slot.output.connector.id == pipeline.connector)
            .map_or_else(
                || Served {
                    connector: pipeline.connector,
                    name: format!("connector {}", pipeline.connector.raw()),
                    width: pipeline.width(),
                    height: pipeline.height(),
                },
                |slot| Served {
                    connector: slot.output.connector.id,
                    name: slot.output.name.clone(),
                    width: u32::from(slot.output.mode().hdisplay),
                    height: u32::from(slot.output.mode().vdisplay),
                },
            );
        // A connector that is no longer connected was torn down by the
        // kernel together with its pipeline; a disable the device then
        // rejects is history repeating itself, not a failure to act on.
        let connector_alive = self
            .topology
            .iter()
            .any(|(id, status)| *id == pipeline.connector && *status == ConnectorStatus::Connected);
        match serve::disable(self.device.as_mut(), pipeline) {
            Ok(()) => {}
            Err(_) if !connector_alive => {
                // The connector died under us; its pipeline died with it.
            }
            Err(e) => return Err(e.into()),
        }
        if let Some(index) = self
            .outputs
            .iter()
            .position(|slot| slot.output.connector.id == pipeline.connector)
        {
            let slot = self.outputs.remove(index);
            self.release_slot_objects(&slot);
        }
        Ok(served)
    }

    /// Release one slot's scanout chain kernel objects: framebuffers
    /// removed on every backend, dumb buffers destroyed on the DRM
    /// lease, the lease itself retired. The mappings (and the shadow
    /// buffers) drop with the chain.
    fn release_slot_objects(&mut self, slot: &OutputSlot) {
        for fb in slot.scanout.fbs {
            let _ = self.device.rm_fb(fb);
        }
        if let Some(DrmLease { handles, .. }) = slot.lease {
            if let Some(drm) = self.device.as_drm_mut() {
                for handle in handles {
                    let _ = drm.backend_mut().destroy_dumb(handle);
                }
            }
        }
    }

    /// Drop every trace of one output from the scene's per-output
    /// bookkeeping: entered flags and visibility sets (its binds are
    /// revoked separately; its release gates satisfy themselves — a
    /// CRTC that no longer exists has stopped reading).
    fn forget_output(&mut self, crtc: CrtcId) {
        for route in self.scene.routes.values_mut() {
            route.entered.retain(|c| *c != crtc);
            route.visible_on.retain(|c| *c != crtc);
        }
    }

    /// Bring one pipeline up as a served output (addition, migration,
    /// or relight): allocate the scanout chain, enable applied, build
    /// the protocol face. The caller places the slot, resolves the
    /// layouts, and re-anchors what the primary change asks for.
    fn light_output(&mut self, pipeline: &Pipeline) -> Result<OutputSlot, FrameError> {
        let backend: &dyn KmsBackend = self.device.as_ref();
        let mut output =
            OutputGlobal::from_backend(backend, pipeline, (0, 0)).ok_or_else(|| {
                FrameError::Rearrange(format!(
                    "connector {} vanished between re-probe and bring-up",
                    pipeline.connector.raw()
                ))
            })?;
        // The scale doctrine rides the newcomer (Phase 37): the
        // per-output list's last entry (the stretch rule), or the
        // primary's own factor when the list is empty — the operator's
        // `--scale` answer. Before this rule a hotplug newcomer reset
        // to identity and advertised a lie (a latent Phase 31 bug:
        // `--scale 2` + a monitor joining mid-session = the newcomer
        // at 1x while the shell laid out at 2x).
        output.scale = self.newcomer_scale();
        // The forced size re-points the protocol face at the mode the
        // sized selection picked (a miss means the migration fell
        // back to the unsized doctrine — the preferred mode stands);
        // a foundry pour is *adopted* instead (Phase 42 — the
        // user-defined mode joins the advertised list).
        if pipeline.mode.kind.0 & ldp_display::mode::ModeType::USERDEF.0 != 0 {
            output.adopt_mode(&pipeline.mode);
        } else if let Some(size) = self.resolution {
            let _ = output.select_size(size);
        }
        let (w, h) = (pipeline.width(), pipeline.height());
        // The scanout store kind follows the world's configuration:
        // real mappings on the DRM driver, the configured store
        // (shadow by default, mappings for the equivalence suites)
        // on the mock.
        let (scanout, lease) = if self.device.as_drm().is_some() {
            let (handles, fbs, mappings, pitch) = allocate_drm_scanout(
                self.device
                    .as_drm_mut()
                    .expect("checked drm above")
                    .backend_mut(),
                w,
                h,
            )?;
            (
                ScanoutChain::mapped(mappings, pitch as usize, w, h, fbs),
                Some(DrmLease { handles, fbs }),
            )
        } else if self.mapped_scanout {
            let (fbs, maps, pitch) = allocate_mock_mapped_scanout(self.device.as_mut(), w, h)?;
            (ScanoutChain::mapped(maps, pitch as usize, w, h, fbs), None)
        } else {
            let fbs = allocate_mock_shadow_fbs(self.device.as_mut(), w, h)?;
            let pixels = usize::try_from(w * h).unwrap_or(0);
            (ScanoutChain::shadow(pixels, fbs), None)
        };
        serve::enable(self.device.as_mut(), pipeline, scanout.fbs[0])
            .map_err(|e| FrameError::Rearrange(format!("enable rejected: {e}")))?;
        // The scene learns the output's ledger entry (release gates
        // queue against it from now on).
        self.scene.flips.entry(pipeline.crtc).or_insert(0);
        // The re-formed pipeline re-walks its plane inventory (Phase
        // 34): a new CRTC may carry different plane capabilities than
        // the one it replaces.
        let planes = match self.device.topology() {
            Ok(top) => match top.crtc_index(pipeline.crtc) {
                Some(i) => ldp_planes::PlaneInventory::collect(self.device.as_ref(), i)
                    .map_err(|e| FrameError::Rearrange(format!("plane inventory: {e}")))?,
                None => ldp_planes::PlaneInventory::default(),
            },
            Err(_) => ldp_planes::PlaneInventory::default(),
        };
        let psr_capable = self
            .device
            .object_properties(ldp_display::ids::AnyId::Connector(pipeline.connector))
            .is_ok_and(|props| {
                props.entries.iter().any(|(id, _)| {
                    self.device.property(*id).is_ok_and(|def| {
                        def.name.as_str() == ldp_display::props::prop::PANEL_SELF_REFRESH
                    })
                })
            });
        let mut slot = OutputSlot {
            output,
            crtc: pipeline.crtc,
            plane: pipeline.plane,
            scanout,
            lease,
            flips: 0,
            pending: ldp_core::geometry::Region::new(),
            owes: false,
            planes,
            active_planes: Vec::new(),
            all_planes: false,
            display: Vec::new(),
            psr: ldp_power::psr::PsrMachine::default_machine(),
            psr_capable,
            psr_live: false,
        };
        // The mirror newcomer owes its whole first frame (Phase 37):
        // the desktop damage alone may not cover it (a display larger
        // than the desktop letterboxes in background its damage never
        // reaches — the region beyond the primary's bounds dirties
        // nowhere else). The full-bounds seed paints it whole from its
        // first flip; an extended-desktop newcomer keeps the Phase 31
        // doctrine (it renders what the distributed damage gives it,
        // the survivors re-render arm seeding the whole desktop
        // anyway).
        if self.arrangement == crate::scene::OutputArrangement::Mirrored {
            slot.pending.add(slot.output.bounds());
        }
        Ok(slot)
    }

    /// Resolve the logical layout after any slot-set change: the
    /// primary stays at the origin, the others append to the right in
    /// slot order (top-aligned — the deterministic default; the
    /// operator's arrangement surface is a roadmap line). The mirror
    /// doctrine (Phase 37) keeps *every* output at the origin — a
    /// display joining a mirrored desktop joins the mirror, not an
    /// extension, and a newcomer owes its whole first frame (the
    /// full-bounds seed, the bring-up doctrine — its fb-init edges
    /// never reach the panel).
    fn resolve_layouts(&mut self) {
        match self.arrangement {
            crate::scene::OutputArrangement::Extended => {
                let mut x = 0i32;
                for slot in &mut self.outputs {
                    slot.output.layout = (x, 0);
                    x += i32::try_from(u32::from(slot.output.mode().hdisplay)).unwrap_or(i32::MAX);
                }
            }
            crate::scene::OutputArrangement::Mirrored => {
                for slot in &mut self.outputs {
                    slot.output.layout = (0, 0);
                }
            }
        }
    }

    /// Find the first `(crtc, primary plane)` pair outside `taken` (the
    /// resources living and already-planned pipelines hold) — the
    /// newcomer's honest reallocation. `None` when the device is out.
    fn reallocate_around(&self, taken: &[CrtcId]) -> Option<(CrtcId, PlaneId)> {
        let top = self.device.topology().ok()?;
        let taken_planes: Vec<PlaneId> = self.outputs.iter().map(|slot| slot.plane).collect();
        top.crtcs.iter().find_map(|crtc| {
            if taken.contains(crtc) {
                return None;
            }
            let index = top.crtc_index(*crtc)?;
            let plane = top.planes.iter().find(|p| {
                !taken_planes.contains(p)
                    && self.device.plane_info(**p).is_ok_and(|info| {
                        info.kind == ldp_display::plane::PlaneType::Primary
                            && info.feeds_crtc_index(index)
                    })
            })?;
            Some((*crtc, *plane))
        })
    }

    /// Re-place every mapped root at the current shell layout (the
    /// Phase 28 migration arm — a new primary is a fresh boot to the
    /// eye).
    fn replace_roots(&mut self) {
        let roots: Vec<SurfaceId> = self.scene.tree.children_of(None).to_vec();
        for id in roots {
            let Some(surface) = self.scene.tree.get(id) else {
                continue;
            };
            if !surface.state().is_mapped() {
                continue;
            }
            let root_bounds = if surface.last_bounds().is_empty() {
                // Mapped but never rendered (the migration raced the
                // first commit): the live position and size still
                // re-place off the current geometry.
                let (origin_x, origin_y) = surface.position();
                match surface.state().buffer {
                    Some(ref attach) => {
                        let size = surface.state().transform.transform_size(
                            ldp_core::geometry::Size::new(attach.width, attach.height),
                        );
                        Rect::new(origin_x, origin_y, size.w, size.h)
                    }
                    None => Rect::new(origin_x, origin_y, 0, 0),
                }
            } else {
                surface.last_bounds()
            };
            let (next_x, next_y) = self.shell.replace_root(root_bounds);
            self.scene.tree.set_position_now(id, next_x, next_y).ok();
        }
    }

    /// Land the bring-up flips (shared by every bring-up and
    /// re-arrangement): wait for the newly-lit outputs' enable flips —
    /// the driver's wait, so the mock's clock advances exactly to the
    /// landings — and feed the scheduler its anchor (the primary's
    /// landing; sibling outputs keep their own ledgers). Idle vblanks
    /// from already-live outputs ride along and are consumed.
    pub fn land_bringup_flip(&mut self) {
        let mut patience = 16;
        while self.outputs.iter().any(|slot| slot.scanout.flip_pending) {
            let Ok(events) = self.device.wait_events(None) else {
                break;
            };
            if events.is_empty() {
                // Nothing is due at all (no live timeline): the
                // remaining pendings can never land — the honest
                // degradation.
                for slot in &mut self.outputs {
                    slot.scanout.flip_pending = false;
                }
                break;
            }
            let has_flip = events.iter().any(|e| matches!(e, DeviceEvent::PageFlip(_)));
            self.land_bringup_events(events);
            if has_flip {
                patience = 16;
            } else {
                patience -= 1;
                if patience == 0 {
                    for slot in &mut self.outputs {
                        slot.scanout.flip_pending = false;
                    }
                    break;
                }
            }
        }
    }

    /// The bring-up flip landing's event application (the ledger
    /// updates without the release-gate pass — nothing is owed yet).
    fn land_bringup_events(&mut self, events: Vec<DeviceEvent>) {
        let primary_crtc = self.outputs.first().map(|slot| slot.crtc);
        for event in events {
            if let DeviceEvent::PageFlip(flip) = event {
                if let Some(slot) = self.outputs.iter_mut().find(|s| s.crtc == flip.crtc) {
                    slot.scanout.flip_pending = false;
                    slot.flips += 1;
                    self.scene.flips.insert(slot.crtc, slot.flips);
                    if Some(slot.crtc) == primary_crtc {
                        self.scene.scheduler.observe_flip(flip.timestamp);
                    }
                }
            }
        }
    }

    /// Mint the release fences for a batch of due releases — shared
    /// by the flip-gated path (`land_events`), the dark flush, and
    /// the dark state's pump (nothing is being read anymore, so the
    /// fences do not wait for a flip that will never land).
    pub(crate) fn due_releases(due: Vec<PendingRelease>) -> Result<Vec<OutboxEntry>, FrameError> {
        let mut entries = Vec::with_capacity(due.len());
        for r in due {
            // The headless release fence: an already-signalled eventfd —
            // readable means "the compositor is done reading" (the DRM
            // path hands out the kernel's out-fence sync-file instead).
            let fd = sys::eventfd_signalled()?;
            entries.push(OutboxEntry::with_fd(
                r.client,
                r.buffer_obj,
                "release",
                vec![Value::Fd(0)],
                fd,
            ));
        }
        Ok(entries)
    }

    /// Flush every pending release now — the dark-state pump's arm.
    pub(crate) fn fire_releases(&mut self) -> Result<Vec<OutboxEntry>, FrameError> {
        let due = self.scene.take_pending_releases();
        Self::due_releases(due)
    }

    /// Revoke bound output objects (`capability_revoked` — the
    /// display they mirrored is gone) and clear the binds. `crtc`
    /// scopes the revocation to one output's binds (that output is
    /// gone); `None` revokes them all (the layout moved under the
    /// survivors — geometry rode their cascades, so they are stale
    /// too). Delivery rides the outboxes: each client learns at its
    /// next wake.
    fn revoke_output_binds(&mut self, crtc: Option<CrtcId>) {
        let binds: Vec<((u32, CrtcId), ObjectId)> = self
            .scene
            .output_binds
            .iter()
            .filter(|((_, c), _)| crtc.map_or(true, |want| *c == want))
            .map(|(k, v)| (*k, *v))
            .collect();
        for ((raw, crtc_key), object) in binds {
            self.scene.output_binds.remove(&(raw, crtc_key));
            let Some(client) = ClientId::new(raw) else {
                continue;
            };
            self.outboxes.push(OutboxEntry::revoke(
                client,
                object,
                RevokedReason::CapabilityRevoked.to_wire(),
            ));
        }
    }

    /// The primary output's summary identity, for reports and tests.
    #[must_use]
    pub fn served(&self) -> Option<Served> {
        self.outputs.first().map(|slot| Served {
            connector: slot.output.connector.id,
            name: slot.output.name.clone(),
            width: u32::from(slot.output.mode().hdisplay),
            height: u32::from(slot.output.mode().vdisplay),
        })
    }

    /// Every served output's identity, in slot order (the
    /// multi-output report).
    #[must_use]
    pub fn served_all(&self) -> Vec<Served> {
        self.outputs
            .iter()
            .map(|slot| Served {
                connector: slot.output.connector.id,
                name: slot.output.name.clone(),
                width: u32::from(slot.output.mode().hdisplay),
                height: u32::from(slot.output.mode().vdisplay),
            })
            .collect()
    }

    /// The served identity of one pipeline (verdict construction).
    fn served_of(&self, pipeline: &Pipeline) -> Served {
        Served {
            connector: pipeline.connector,
            name: self
                .outputs
                .iter()
                .find(|slot| slot.output.connector.id == pipeline.connector)
                .map_or_else(
                    || format!("connector {}", pipeline.connector.raw()),
                    |slot| slot.output.name.clone(),
                ),
            width: pipeline.width(),
            height: pipeline.height(),
        }
    }
}

/// The DRM allocation: two kernel dumb buffers at the mode's
/// geometry, each registered as a framebuffer, each mapped and zeroed
/// opaque black before anything scans out. Returns
/// `(handles, fbs, mappings, pitch)`. The bring-up twin lives in
/// `server.rs`; the re-arrangement replays it live.
///
/// # Errors
///
/// Each step fails typed with context; whatever was already created
/// is unwound before the error escapes.
#[allow(clippy::type_complexity)]
pub(crate) fn allocate_drm_scanout(
    backend: &mut ldp_display::drm::DrmBackend,
    w: u32,
    h: u32,
) -> Result<
    (
        [u32; 2],
        [ldp_display::ids::FbId; 2],
        [ldp_display::drm::sys::DumbMapping; 2],
        u32,
    ),
    FrameError,
> {
    let mut handles = [0u32; 2];
    let mut pitches = [0u32; 2];
    let mut sizes = [0u64; 2];
    // Placeholder FB ids (1) stand in until registration replaces
    // them — the unwind path ignores releases that were never made.
    let mut fbs = [ldp_display::ids::FbId::new(1).expect("nonzero id"); 2];
    for i in 0..2 {
        let (handle, pitch) = backend
            .create_dumb(w, h, 32)
            .map_err(|e| FrameError::Rearrange(format!("CREATE_DUMB at {w}x{h}: {e}")))?;
        if pitch < w * 4 {
            return Err(FrameError::Rearrange(format!(
                "dumb pitch {pitch} underflows XRGB8888 at {w} wide"
            )));
        }
        handles[i] = handle;
        pitches[i] = pitch;
        sizes[i] = u64::from(pitch) * u64::from(h);
    }
    for i in 0..2 {
        let fb = backend
            .add_fb(
                &FbSpec::single(w, h, FourCC::XRGB8888, handles[i], pitches[i], 0),
                Some(Modifier::LINEAR),
            )
            .map_err(|e| {
                unwind_drm(backend, handles, fbs);
                FrameError::Rearrange(format!("drmModeAddFB2: {e}"))
            })?;
        fbs[i] = fb;
    }
    let map0 = backend.map_dumb(handles[0], sizes[0]).map_err(|e| {
        unwind_drm(backend, handles, fbs);
        FrameError::Rearrange(format!("MAP_DUMB + mmap: {e}"))
    })?;
    let map1 = backend.map_dumb(handles[1], sizes[1]).map_err(|e| {
        unwind_drm(backend, handles, fbs);
        FrameError::Rearrange(format!("MAP_DUMB + mmap: {e}"))
    })?;
    // Opaque black before anything scans out: the panel shows a clean
    // black frame until the first commit lands (explicit over trusting
    // kernel page zeroing).
    let mut mappings = [map0, map1];
    for map in &mut mappings {
        for byte in map.as_mut_slice() {
            *byte = 0x00;
        }
    }
    Ok((handles, fbs, mappings, pitches[0]))
}

/// The mock's mapped store (the CI equivalence path): two fresh
/// framebuffers over anonymous mappings at the mode's pitch — the
/// same lifetime discipline and pitch-honoring writes as the DRM
/// chain, without the kernel objects.
#[allow(clippy::type_complexity)]
fn allocate_mock_mapped_scanout(
    device: &mut dyn KmsBackend,
    w: u32,
    h: u32,
) -> Result<
    (
        [ldp_display::ids::FbId; 2],
        [ldp_display::drm::sys::DumbMapping; 2],
        u32,
    ),
    FrameError,
> {
    let pitch = w * 4;
    let mut fbs = [ldp_display::ids::FbId::new(1).expect("nonzero id"); 2];
    for (i, fb) in fbs.iter_mut().enumerate() {
        let handle = u32::try_from(i + 1).expect("handle fits");
        *fb = device
            .add_fb(
                &FbSpec::single(w, h, FourCC::XRGB8888, handle, pitch, 0),
                Some(Modifier::LINEAR),
            )
            .map_err(|e| FrameError::Rearrange(format!("framebuffer registration: {e}")))?;
    }
    let byte_len = usize::try_from(u64::from(pitch) * u64::from(h)).unwrap_or(0);
    let maps = [
        ldp_display::drm::sys::anon_mapping(byte_len).map_err(FrameError::Display)?,
        ldp_display::drm::sys::anon_mapping(byte_len).map_err(FrameError::Display)?,
    ];
    // Fresh anonymous mappings read as zero — opaque black (XRGB's
    // reserved byte reads back through the pitch-honoring writer) —
    // the same bring-up contract as the DRM chain.
    Ok((fbs, maps, pitch))
}

/// The mock's shadow store: two fresh framebuffers; the shadow
/// buffers arrive opaque black by construction.
fn allocate_mock_shadow_fbs(
    device: &mut dyn KmsBackend,
    w: u32,
    h: u32,
) -> Result<[ldp_display::ids::FbId; 2], FrameError> {
    let pitch = w * 4;
    let mut fbs = [ldp_display::ids::FbId::new(1).expect("nonzero id"); 2];
    for (i, fb) in fbs.iter_mut().enumerate() {
        let handle = u32::try_from(i + 1).expect("handle fits");
        *fb = device
            .add_fb(
                &FbSpec::single(w, h, FourCC::XRGB8888, handle, pitch, 0),
                Some(Modifier::LINEAR),
            )
            .map_err(|e| FrameError::Rearrange(format!("framebuffer registration: {e}")))?;
    }
    Ok(fbs)
}

/// Release re-arrangement objects after a failed DRM allocation
/// (best-effort, in reverse order; unmade releases fail harmlessly).
fn unwind_drm(
    backend: &mut ldp_display::drm::DrmBackend,
    handles: [u32; 2],
    fbs: [ldp_display::ids::FbId; 2],
) {
    for fb in fbs {
        let _ = backend.rm_fb(fb);
    }
    for handle in handles {
        if handle != 0 {
            let _ = backend.destroy_dumb(handle);
        }
    }
}

/// The live-vs-selection diff, keyed by connector (a connector is one
/// display whatever mode it serves): the removals, the additions, and
/// the migrations (same connector, changed pipeline).
fn pipeline_diff(
    live: &[Pipeline],
    selection: &[Pipeline],
) -> (Vec<Pipeline>, Vec<Pipeline>, Vec<(Pipeline, Pipeline)>) {
    let mut removed: Vec<Pipeline> = Vec::new();
    let mut added: Vec<Pipeline> = Vec::new();
    let mut migrated: Vec<(Pipeline, Pipeline)> = Vec::new();
    for l in live {
        match selection.iter().find(|p| p.connector == l.connector) {
            Some(p) if p != l => migrated.push((l.clone(), p.clone())),
            Some(_) => {}
            None => removed.push(l.clone()),
        }
    }
    for p in selection {
        if !live.iter().any(|l| l.connector == p.connector) {
            added.push(p.clone());
        }
    }
    (removed, added, migrated)
}
