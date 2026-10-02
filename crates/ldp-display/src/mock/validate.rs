//! Atomic commit planning, validation, and application on the mock.
//!
//! The rules mirror `drm_atomic_check_only` and its callees, in order:
//! request shape, object/property existence, type fit, then the semantic
//! pass over the *effective* state (current values overlaid with the
//! request): plane/CRTC feasibility, framebuffer bounds, connector
//! binding, mode support, modeset detection, busy detection. Only after
//! every check passes does anything mutate — commits are all-or-nothing.
//!
//! Blob payloads (`MODE_ID`, gamma LUTs, HDR metadata) are staged in the
//! plan and minted into the store only on application, so `TEST_ONLY`
//! leaves no residue — matching the kernel, where a TEST_ONLY commit
//! allocates nothing persistent.

#![forbid(unsafe_code)]

use std::collections::BTreeSet;

use crate::atomic::AtomicRequest;
use crate::backend::CommitOutcome;
use crate::error::{DisplayError, RejectReason, Result};
use crate::ids::{AnyId, BlobId, CrtcId, PropId};
use crate::mode::{Mode, ModeType};
use crate::props::{prop, PropValue};

use super::device::MockDevice;
use super::timeline::CrtcTimeline;

/// Reject helper.
fn reject(reason: RejectReason, detail: impl Into<String>) -> DisplayError {
    DisplayError::AtomicReject {
        reason,
        detail: detail.into(),
    }
}

/// Effective-state marker for a MODE_ID set through an inline blob
/// payload: nonzero like any real blob id (blob ids are u32, so the
/// full-64 sentinel can never collide), replaced by the minted id at
/// application time.
const STAGED_MODE: u64 = u64::MAX;

/// A property's name by id (properties are globally unique in the mock).
fn name_of<'a>(names: &[(PropId, &'a str)], pid: PropId) -> &'a str {
    names
        .iter()
        .find(|(id, _)| *id == pid)
        .map_or("", |(_, n)| *n)
}

/// Effective CRTC state after the request overlays current state.
#[derive(Clone, Copy, Debug)]
struct EffectiveCrtc {
    id: CrtcId,
    index: usize,
    active: bool,
    mode_blob: u64,
    vrr: bool,
}

/// The resolved commit.
struct Plan {
    /// Resolved (object, property id, wire value) operations.
    ops: Vec<(AnyId, PropId, u64)>,
    /// Blobs staged for minting: (object, prop, payload).
    staged_blobs: Vec<(AnyId, PropId, Vec<u8>)>,
    /// CRTCs to schedule flips for (active + event flag).
    event_crtcs: Vec<CrtcId>,
    /// CRTCs with OUT_FENCE_PTR=1.
    out_fence_crtcs: Vec<CrtcId>,
}

/// Run the full commit path: validate, then apply (unless TEST_ONLY).
pub(super) fn commit(dev: &mut MockDevice, request: &AtomicRequest) -> Result<CommitOutcome> {
    let (plan, mut effective) = plan(dev, request)?;
    if request.flags.is_test_only() {
        return Ok(CommitOutcome {
            applied: false,
            out_fences: Vec::new(),
        });
    }
    Ok(apply(dev, request, plan, &mut effective))
}

/// Resolve ops (phase 1-2): existence, immutability, type fit.
fn resolve_ops(
    dev: &MockDevice,
    request: &AtomicRequest,
) -> Result<Vec<(AnyId, PropId, PropValue)>> {
    request.validate_shape()?;
    let mut resolved = Vec::with_capacity(request.ops.len());
    for op in &request.ops {
        let object_exists = match op.obj {
            AnyId::Crtc(id) => dev.crtcs.iter().any(|c| c.id == id),
            AnyId::Plane(id) => dev.planes.iter().any(|p| p.info.id == id),
            AnyId::Connector(id) => dev.connectors.iter().any(|c| c.info.id == id),
        };
        if !object_exists {
            return Err(reject(RejectReason::UnknownObject, op.obj.to_string()));
        }
        let Some(catalog) = dev.catalogs.get(&op.obj) else {
            return Err(reject(RejectReason::UnknownObject, op.obj.to_string()));
        };
        let prop_def = dev
            .prop_defs
            .iter()
            .find(|p| catalog.contains(&p.id) && p.name == op.name)
            .ok_or_else(|| {
                reject(
                    RejectReason::UnknownProperty,
                    format!("{} on {}", op.name, op.obj),
                )
            })?;
        if prop_def.immutable {
            return Err(reject(RejectReason::Immutable, prop_def.name.to_string()));
        }
        if !op.value.fits(prop_def) {
            return Err(reject(
                RejectReason::PropertyType,
                format!("{} value {:?}", prop_def.name, op.value),
            ));
        }
        resolved.push((op.obj, prop_def.id, op.value.clone()));
    }
    Ok(resolved)
}

/// Full validation and planning. Returns the plan and the effective CRTC
/// states (needed by apply to avoid recomputing).
///
/// The length mirrors `drm_atomic_check_only` on purpose: the checks
/// are the kernel's *sequence*, and scattering them across helpers
/// would hide exactly the ordering this module exists to prove.
#[allow(clippy::too_many_lines)]
fn plan(dev: &MockDevice, request: &AtomicRequest) -> Result<(Plan, Vec<EffectiveCrtc>)> {
    let resolved = resolve_ops(dev, request)?;
    let names: Vec<(PropId, &str)> = dev
        .prop_defs
        .iter()
        .map(|p| (p.id, p.name.as_str()))
        .collect();

    // Stage blob payloads.
    let staged_blobs: Vec<(AnyId, PropId, Vec<u8>)> = resolved
        .iter()
        .filter_map(|(obj, pid, value)| {
            if let PropValue::Blob(data) = value {
                Some((*obj, *pid, data.clone()))
            } else {
                None
            }
        })
        .collect();

    // ---- Effective states ----------------------------------------------
    let mut plane_states: Vec<(usize, super::device::PlaneState)> = dev
        .planes
        .iter()
        .enumerate()
        .map(|(i, p)| (i, p.state))
        .collect();
    for (i, state) in &mut plane_states {
        let obj = AnyId::Plane(dev.planes[*i].info.id);
        for (target, pid, value) in &resolved {
            if *target != obj {
                continue;
            }
            overlay_plane(state, name_of(&names, *pid), value.wire_u64());
        }
    }
    let mut crtcs: Vec<EffectiveCrtc> = dev
        .crtcs
        .iter()
        .enumerate()
        .map(|(index, c)| EffectiveCrtc {
            id: c.id,
            index,
            active: c.active,
            mode_blob: c.mode_blob,
            vrr: c.vrr_enabled,
        })
        .collect();
    for crtc in &mut crtcs {
        let obj = AnyId::Crtc(crtc.id);
        for (target, pid, value) in &resolved {
            if *target != obj {
                continue;
            }
            match name_of(&names, *pid) {
                prop::ACTIVE => crtc.active = value.wire_u64() != 0,
                prop::MODE_ID => {
                    crtc.mode_blob = match value {
                        PropValue::Blob(_) => STAGED_MODE,
                        other => other.wire_u64(),
                    };
                }
                prop::VRR_ENABLED => crtc.vrr = value.wire_u64() != 0,
                _ => {}
            }
        }
    }
    let mut bindings: Vec<(crate::ids::ConnectorId, u64)> = dev
        .connectors
        .iter()
        .map(|c| {
            (
                c.info.id,
                dev.connector_binding(c.info.id)
                    .map_or(0, |id| u64::from(id.raw())),
            )
        })
        .collect();
    for (cid, binding) in &mut bindings {
        let obj = AnyId::Connector(*cid);
        for (target, pid, value) in &resolved {
            if *target != obj {
                continue;
            }
            if name_of(&names, *pid) == prop::CRTC_ID {
                *binding = value.wire_u64();
            }
        }
    }

    // ---- Referenced CRTC set -------------------------------------------
    // The CRTCs *this commit* re-programs — the kernel's page-flip
    // scope: the CRTC ops, the planes whose ops ride this request (by
    // their new assignments), and the connectors whose bindings this
    // request changes. Untouched-but-active CRTCs (the other monitor
    // mid-desktop) are deliberately absent: a commit that does not
    // touch their planes does not flip them (the Phase 31 multi-output
    // doctrine caught the over-broad set — every PAGE_FLIP_EVENT
    // commit was flipping every active CRTC).
    let mut touched: BTreeSet<CrtcId> = BTreeSet::new();
    for (obj, _, _) in &resolved {
        if let AnyId::Crtc(id) = obj {
            touched.insert(*id);
        }
    }
    for (i, state) in &plane_states {
        let plane_obj = AnyId::Plane(dev.planes[*i].info.id);
        if resolved.iter().any(|(o, _, _)| *o == plane_obj) {
            if let Some(id) = CrtcId::new(u32::try_from(state.crtc).unwrap_or(0)) {
                touched.insert(id);
            }
        }
    }
    for (cid, binding) in &bindings {
        let conn_obj = AnyId::Connector(*cid);
        if resolved.iter().any(|(o, _, _)| *o == conn_obj) {
            if let Some(id) = CrtcId::new(u32::try_from(*binding).unwrap_or(0)) {
                touched.insert(id);
            }
        }
    }

    // ---- Semantic checks ------------------------------------------------
    for (i, state) in &plane_states {
        if state.fb == 0 {
            continue;
        }
        let plane = &dev.planes[*i].info;
        let Some(crtc) = CrtcId::new(u32::try_from(state.crtc).unwrap_or(0)) else {
            return Err(reject(
                RejectReason::Malformed,
                format!("plane {} on without CRTC", plane.id),
            ));
        };
        let Some(idx) = dev.crtcs.iter().position(|c| c.id == crtc) else {
            return Err(reject(RejectReason::UnknownObject, crtc.to_string()));
        };
        if !plane
            .possible_crtcs
            .contains(u32::try_from(idx).unwrap_or(u32::MAX))
        {
            return Err(reject(
                RejectReason::PlaneNotPossible,
                format!("{} on {}", plane.id, crtc),
            ));
        }
        let Some((_, spec, modifier)) = dev
            .fbs
            .iter()
            .find(|(id, _, _)| u64::from(id.raw()) == state.fb)
        else {
            return Err(reject(RejectReason::FbNotFound, format!("fb {}", state.fb)));
        };
        // FBs registered without a modifier are linear by AddFB2 contract.
        let modifier = modifier.unwrap_or(ldp_core::buffer::Modifier::LINEAR);
        if !plane.supports(spec.format, modifier) {
            if plane.formats.contains(&spec.format) {
                return Err(reject(
                    RejectReason::ModifierUnsupported,
                    format!("{:?}+{} on {}", spec.format, modifier, plane.id),
                ));
            }
            return Err(reject(
                RejectReason::FormatUnsupported,
                format!("{:?} on {}", spec.format, plane.id),
            ));
        }
        if state.src_w == 0 || state.src_h == 0 || state.crtc_w == 0 || state.crtc_h == 0 {
            return Err(reject(RejectReason::ZeroSize, plane.id.to_string()));
        }
        let fw = u64::from(spec.width) << 16;
        let fh = u64::from(spec.height) << 16;
        if state
            .src_x
            .checked_add(state.src_w)
            .map_or(true, |end| end > fw)
            || state
                .src_y
                .checked_add(state.src_h)
                .map_or(true, |end| end > fh)
        {
            return Err(reject(RejectReason::SrcOutOfBounds, plane.id.to_string()));
        }
        if state.in_fence != -1 && !dev.fences.contains_key(&state.in_fence) {
            return Err(reject(
                RejectReason::InvalidInFence,
                format!("fd {}", state.in_fence),
            ));
        }
    }

    // Active CRTCs need a live mode offered by a bound connected
    // connector — the CRTCs *this commit* re-programs (the kernel's
    // commit scope). An untouched-but-active CRTC whose connector
    // vanished mid-desktop (the other monitor's cable pulled, its
    // teardown not yet serviced) is torn down by its own disable,
    // never by every other commit: Phase 31's multi-output desktop
    // caught the over-broad walk — one monitor's death was rejecting
    // the survivor's cleanup with a phantom mode check.
    for crtc in &crtcs {
        if !crtc.active {
            continue;
        }
        let crtc_obj = AnyId::Crtc(crtc.id);
        if !resolved.iter().any(|(o, _, _)| *o == crtc_obj) {
            // Untouched by this request: out of the commit's scope.
            continue;
        }
        if crtc.mode_blob == 0 {
            return Err(reject(
                RejectReason::Malformed,
                format!("{} active without MODE_ID", crtc.id),
            ));
        }
        let mode = resolve_mode(dev, &resolved, &names, crtc)?;
        let bound: Vec<_> = bindings
            .iter()
            .filter(|(_, b)| CrtcId::new(u32::try_from(*b).unwrap_or(0)) == Some(crtc.id))
            .collect();
        if bound.is_empty() {
            return Err(reject(RejectReason::MissingConnector, crtc.id.to_string()));
        }
        let offered = bound.iter().any(|(cid, _)| {
            dev.connectors
                .iter()
                .any(|c| c.info.id == *cid && c.info.modes.contains(&mode))
        });
        // Phase 42 — the mode foundry's second gate: a *user-defined*
        // mode (the `USERDEF` type bit a pour mints — the kernel's
        // `drmModeAddMode` lineage) rides the display engine's
        // declared synthesis envelope instead of the sink's offered
        // list. The engine's envelope is its own truth, checked here
        // at commit time exactly as a real driver's atomic check
        // validates user modes: the mode must be structurally valid
        // and clock within the bound connector's envelope. No
        // envelope, no user modes — the honest refusal.
        let userdef = mode.kind.0 & ModeType::USERDEF.0 != 0
            && mode.is_valid()
            && bound.iter().any(|(cid, _)| {
                dev.connectors.iter().any(|c| {
                    c.info.id == *cid
                        && c.synth_max_clock_khz
                            .is_some_and(|cap| mode.clock_khz <= cap)
                })
            });
        if !offered && !userdef {
            return Err(reject(RejectReason::ModeNotSupported, mode.name.clone()));
        }
    }

    // Connector bindings must target existing, connected CRTCs — for
    // the connectors *this commit* re-binds (the kernel's commit
    // scope). A previously-bound connector that vanished mid-desktop
    // (the other monitor's cable pulled) is torn down by its own
    // disable, never by every other commit: Phase 31's multi-output
    // desktop caught the over-broad walk — one monitor's death was
    // rejecting the survivor's cleanup.
    for (cid, binding) in &bindings {
        if *binding == 0 {
            continue;
        }
        let conn_obj = AnyId::Connector(*cid);
        if !resolved.iter().any(|(o, _, _)| *o == conn_obj) {
            // Not re-bound by this request: out of the commit's scope.
            continue;
        }
        let Some(crtc) = CrtcId::new(u32::try_from(*binding).unwrap_or(0)) else {
            return Err(reject(
                RejectReason::Malformed,
                format!("{cid} bound to invalid id"),
            ));
        };
        if dev.crtcs.iter().all(|c| c.id != crtc) {
            return Err(reject(RejectReason::UnknownObject, crtc.to_string()));
        }
        let conn = dev.connectors.iter().find(|c| c.info.id == *cid).unwrap();
        if conn.info.status != crate::connector::ConnectorStatus::Connected {
            return Err(reject(RejectReason::ConnectorDisconnected, cid.to_string()));
        }
    }

    // Modeset detection: diff effective vs current.
    let mut needs_modeset: Option<String> = None;
    for crtc in &crtcs {
        let current = &dev.crtcs[crtc.index];
        if crtc.active != current.active || (crtc.active && crtc.mode_blob != current.mode_blob) {
            needs_modeset = Some(format!("crtc {} active/mode change", crtc.id.raw()));
        }
    }
    for (cid, binding) in &bindings {
        let current = dev
            .connector_binding(*cid)
            .map_or(0, |id| u64::from(id.raw()));
        if *binding != current {
            needs_modeset = Some(format!("connector {} rebind", cid.raw()));
        }
    }
    for (i, state) in &plane_states {
        let current = &dev.planes[*i].state;
        // A *live* plane reassigned to another CRTC is a pipeline
        // change (the scanout moves between pipes); a plane this
        // commit DISABLES (`fb == 0`) is the normal overlay-retire
        // shape — the kernel takes it in a plain page-flip commit
        // (Phase 34's multi-plane frames retire overlays every time
        // the plan shrinks).
        if state.crtc != current.crtc && current.fb != 0 && state.fb != 0 {
            let old_still_active = CrtcId::new(u32::try_from(current.crtc).unwrap_or(0))
                .and_then(|id| dev.crtcs.iter().find(|c| c.id == id))
                .is_some_and(|c| c.active);
            if old_still_active {
                needs_modeset = Some(format!(
                    "plane {} moves off an active CRTC",
                    dev.planes[*i].info.id.raw()
                ));
            }
        }
    }
    if let Some(why) = needs_modeset {
        if !request.flags.allows_modeset() {
            return Err(reject(
                RejectReason::NeedsModeset,
                format!("pipeline change without ALLOW_MODESET ({why})"),
            ));
        }
    }

    // Busy: nonblock + pending flip on a touched CRTC.
    if request.flags.is_nonblock() {
        for crtc in &touched {
            if dev.pending_flips(*crtc) > 0 {
                return Err(reject(RejectReason::Busy, crtc.to_string()));
            }
        }
    }

    // Event scheduling: touched + effectively active CRTCs.
    let event_crtcs: Vec<CrtcId> = if request.flags.wants_flip_event() {
        touched
            .into_iter()
            .filter(|id| crtcs.iter().any(|c| c.id == *id && c.active))
            .collect()
    } else {
        Vec::new()
    };

    // Out-fence requests.
    let mut out_fence_crtcs = Vec::new();
    for (obj, pid, value) in &resolved {
        if let (AnyId::Crtc(crtc), PropValue::U64(1)) = (obj, value) {
            if name_of(&names, *pid) == prop::OUT_FENCE_PTR {
                out_fence_crtcs.push(*crtc);
            }
        }
    }

    Ok((
        Plan {
            ops: resolved
                .iter()
                .map(|(o, p, v)| (*o, *p, v.wire_u64()))
                .collect(),
            staged_blobs,
            event_crtcs,
            out_fence_crtcs,
        },
        crtcs,
    ))
}

/// Resolve the effective mode of an active CRTC (staged or stored blob).
fn resolve_mode(
    dev: &MockDevice,
    resolved: &[(AnyId, PropId, PropValue)],
    names: &[(PropId, &str)],
    crtc: &EffectiveCrtc,
) -> Result<Mode> {
    for (obj, pid, value) in resolved {
        if *obj == AnyId::Crtc(crtc.id) && name_of(names, *pid) == prop::MODE_ID {
            if let PropValue::Blob(data) = value {
                return Mode::from_blob(data)
                    .map_err(|_| reject(RejectReason::ModeNotSupported, "mode blob unparsable"));
            }
        }
    }
    let data = dev
        .blobs
        .iter()
        .find(|b| u64::from(b.id.raw()) == crtc.mode_blob)
        .map(|b| b.data.clone())
        .ok_or_else(|| {
            reject(
                RejectReason::BlobNotFound,
                format!("blob {}", crtc.mode_blob),
            )
        })?;
    Mode::from_blob(&data)
        .map_err(|_| reject(RejectReason::ModeNotSupported, "mode blob unparsable"))
}

/// Overlay one wire value onto a plane state by property name.
fn overlay_plane(state: &mut super::device::PlaneState, name: &str, wire: u64) {
    let signed = i64::from_ne_bytes(wire.to_ne_bytes());
    match name {
        prop::CRTC_ID => state.crtc = wire,
        prop::FB_ID => state.fb = wire,
        prop::SRC_X => state.src_x = wire,
        prop::SRC_Y => state.src_y = wire,
        prop::SRC_W => state.src_w = wire,
        prop::SRC_H => state.src_h = wire,
        prop::CRTC_X => state.crtc_x = signed,
        prop::CRTC_Y => state.crtc_y = signed,
        prop::CRTC_W => state.crtc_w = wire,
        prop::CRTC_H => state.crtc_h = wire,
        prop::IN_FENCE_FD => state.in_fence = signed,
        prop::ZPOS => state.zpos = wire,
        _ => {}
    }
}

/// Phase 3: apply a validated plan. Validation already passed, so
/// application cannot fail — blob ids come from the device's own
/// allocator.
///
/// One pass, in kernel application order (blobs, values, planes,
/// CRTCs, fences); splitting it would only obscure that order.
#[allow(clippy::too_many_lines)]
fn apply(
    dev: &mut MockDevice,
    request: &AtomicRequest,
    plan: Plan,
    effective: &mut [EffectiveCrtc],
) -> CommitOutcome {
    // Mint staged blobs, rewriting the ops' blob placeholders.
    let mut minted: Vec<(AnyId, PropId, BlobId)> = Vec::new();
    for (obj, pid, data) in plan.staged_blobs {
        let id = BlobId::new(dev.next_blob).unwrap();
        dev.next_blob += 1;
        dev.blobs.push(crate::backend::Blob { id, data });
        minted.push((obj, pid, id));
    }
    let mut ops = plan.ops;
    for (obj, pid, value) in &mut ops {
        if let Some((_, _, blob)) = minted.iter().find(|(o, p, _)| o == obj && p == pid) {
            *value = u64::from(blob.raw());
        }
    }
    // Effective CRTC states carried the staged-blob sentinel; the minted
    // ids exist now, so patch them in.
    for crtc in effective.iter_mut() {
        if crtc.mode_blob == STAGED_MODE {
            let staged = minted
                .iter()
                .find(|(o, _, _)| *o == AnyId::Crtc(crtc.id))
                .map(|(_, _, id)| u64::from(id.raw()));
            if let Some(real) = staged {
                crtc.mode_blob = real;
            }
        }
    }

    // The PSR release transitions (Phase 35): a connector's
    // self-refresh property written from on to off releases the
    // sleeping panel — the driven timeline re-anchors at this
    // commit's time (the rescan: the display engine restarts the
    // scan; the next flip lands a full nominal later). Captured
    // BEFORE the store so the before-value is still readable.
    let psr_prop_id = dev
        .prop_defs
        .iter()
        .find(|p| p.name.as_str() == prop::PANEL_SELF_REFRESH)
        .map(|p| p.id);
    let mut psr_released: Vec<CrtcId> = Vec::new();
    if let Some(psr_prop_id) = psr_prop_id {
        for (obj, pid, value) in &ops {
            if *pid == psr_prop_id && *value == 0 {
                if let AnyId::Connector(conn) = obj {
                    if dev.psr_enabled(*conn) {
                        if let Some(crtc) = dev.connector_binding(*conn) {
                            psr_released.push(crtc);
                        }
                    }
                }
            }
        }
    }

    // Store new property values.
    for (obj, pid, value) in &ops {
        dev.values.insert((*obj, *pid), *value);
    }

    // Apply the release re-anchors (the timeline lifecycle block
    // above has run, so the driven timelines exist).
    for crtc in &psr_released {
        if let Some(entry) = dev.crtcs.iter_mut().find(|c| c.id == *crtc) {
            if let Some(timeline) = entry.timeline.as_mut() {
                timeline.reanchor(dev.now);
            }
        }
    }

    // Materialize plane states from the property values.
    for i in 0..dev.planes.len() {
        let obj = AnyId::Plane(dev.planes[i].info.id);
        let mut state = dev.planes[i].state;
        for (target, pid, value) in &ops {
            if *target == obj {
                let names: Vec<(PropId, &str)> = dev
                    .prop_defs
                    .iter()
                    .map(|p| (p.id, p.name.as_str()))
                    .collect();
                overlay_plane(&mut state, name_of(&names, *pid), *value);
            }
        }
        dev.planes[i].state = state;
        dev.planes[i].info.current_crtc = CrtcId::new(u32::try_from(state.crtc).unwrap_or(0));
    }

    // CRTC state + timeline lifecycle.
    for crtc in effective {
        let index = crtc.index;
        let was_active = dev.crtcs[index].active;
        let old_vrr = dev.crtcs[index].vrr_enabled;
        dev.crtcs[index].active = crtc.active;
        dev.crtcs[index].mode_blob = crtc.mode_blob;
        dev.crtcs[index].vrr_enabled = crtc.vrr;
        if crtc.active {
            let period = dev
                .blobs
                .iter()
                .find(|b| u64::from(b.id.raw()) == crtc.mode_blob)
                .and_then(|b| Mode::from_blob(&b.data).ok())
                .and_then(|m| m.period().map(ldp_core::time::RefreshInterval::as_ns))
                .unwrap_or(16_666_666);
            let vrr_window = dev.crtcs[index].vrr_capable.filter(|_| crtc.vrr);
            if let Some(timeline) = dev.crtcs[index].timeline.as_mut() {
                timeline.set_nominal(period);
                if old_vrr != crtc.vrr {
                    timeline.set_vrr(vrr_window);
                }
            } else {
                let mut timeline = CrtcTimeline::new(dev.now, period);
                timeline.set_vrr(vrr_window);
                dev.crtcs[index].timeline = Some(timeline);
            }
        } else if was_active {
            // Deactivation cancels pending flips (the kernel drops
            // queued events on disable).
            dev.crtcs[index].timeline = None;
        }
    }

    // Out-fences + flip scheduling.
    let now = dev.now;
    // The sleeping set (Phase 35): CRTCs whose driven connector is in
    // self-refresh *right now* (after this commit's property store),
    // with the connector objects that hold the property — the flips
    // below release them implicitly.
    let sleepers: Vec<(CrtcId, crate::ids::ConnectorId)> = psr_prop_id
        .map(|psr_prop_id| {
            dev.connectors
                .iter()
                .filter_map(|conn| {
                    let obj = AnyId::Connector(conn.info.id);
                    let psr = dev.values.get(&(obj, psr_prop_id)).copied().unwrap_or(0);
                    if psr == 0 {
                        return None;
                    }
                    dev.connector_binding(conn.info.id)
                        .map(|crtc| (crtc, conn.info.id))
                })
                .collect()
        })
        .unwrap_or_default();
    let frozen: Vec<CrtcId> = sleepers.iter().map(|(crtc, _)| *crtc).collect();
    let mut scheduling: Vec<CrtcId> = plan.event_crtcs.clone();
    for crtc in &plan.out_fence_crtcs {
        if !scheduling.contains(crtc) {
            scheduling.push(*crtc);
        }
    }
    let mut out_fences: Vec<(CrtcId, crate::events::OutFence)> = Vec::new();
    for crtc in scheduling {
        let wants_event = plan.event_crtcs.contains(&crtc);
        let out_token = plan.out_fence_crtcs.contains(&crtc).then(|| {
            let token = dev.next_token;
            dev.next_token += 1;
            token
        });
        // The latest in-fence among this CRTC's enabled planes gates the
        // flip.
        let fence_ready = dev
            .planes
            .iter()
            .filter(|p| {
                p.state.crtc == u64::from(crtc.raw()) && p.state.fb != 0 && p.state.in_fence != -1
            })
            .filter_map(|p| dev.fences.get(&p.state.in_fence).copied())
            .max();
        let Some(entry) = dev.crtcs.iter_mut().find(|c| c.id == crtc) else {
            continue;
        };
        if let Some(timeline) = &mut entry.timeline {
            // The implicit release (Phase 35): submitting a flip to a
            // CRTC whose connector is in self-refresh wakes the panel
            // — the kernel's own semantics (a page flip implies a
            // rescan; the property reads back off). The grid
            // re-anchors at `now`, so this flip lands one full
            // nominal period later: the honest exit cost.
            if frozen.contains(&crtc) {
                if let Some(psr_prop_id) = psr_prop_id {
                    for (_, conn) in &sleepers {
                        dev.values.insert((AnyId::Connector(*conn), psr_prop_id), 0);
                    }
                }
                timeline.reanchor(now);
            }
            let target = timeline.submit_flip(now, wants_event, out_token);
            if let Some(ready) = fence_ready {
                timeline.set_fence_ready(ready);
            }
            if let Some(token) = out_token {
                dev.out_fences
                    .insert(token, (target.max(fence_ready.unwrap_or(now)), false));
                out_fences.push((crtc, crate::events::OutFence::Token(token)));
            }
        } else if let Some(token) = out_token {
            // No live scanout to wait for: the fence signals at commit.
            dev.out_fences.insert(token, (now, false));
            out_fences.push((crtc, crate::events::OutFence::Token(token)));
        }
    }

    CommitOutcome {
        applied: !request.flags.is_test_only(),
        out_fences,
    }
}
