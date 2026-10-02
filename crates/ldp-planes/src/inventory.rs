//! The per-CRTC plane inventory: kinds, capability pairs, zpos slots.

use ldp_display::backend::KmsBackend;
use ldp_display::ids::{AnyId, PlaneId};
use ldp_display::plane::{PlaneInfo, PlaneType};

/// One plane's decision-relevant capabilities.
///
/// This is [`PlaneInfo`] plus the plane's zpos slot — the two facts
/// the assignment doctrine consumes. The zpos comes from the device's
/// property table (the `ZPOS` well-known property), not the info
/// struct, because drivers own it and overlay ordering is exactly the
/// stacking vocabulary the solver must respect.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaneCaps {
    /// The plane's snapshot (id, kind, CRTC mask, format caps).
    pub info: PlaneInfo,
    /// The plane's current zpos slot (the stacking order).
    pub zpos: u64,
}

impl PlaneCaps {
    /// Whether this plane scans out `(format, modifier)` — the
    /// `IN_FORMATS` (or legacy fallback) answer of the underlying
    /// [`PlaneInfo`].
    #[must_use]
    pub fn supports(
        &self,
        format: ldp_core::buffer::FourCC,
        modifier: ldp_core::buffer::Modifier,
    ) -> bool {
        self.info.supports(format, modifier)
    }

    /// Whether the plane can feed CRTC index `i`.
    #[must_use]
    pub fn feeds_crtc_index(&self, i: u32) -> bool {
        self.info.feeds_crtc_index(i)
    }
}

/// The plane set of one CRTC, in `(zpos, id)` order — the solver's
/// world.
///
/// Built once per pipeline at bring-up (and refreshed on topology
/// motion), the inventory is the *complete* hardware truth: the
/// primary plane that carries the canvas, the overlay planes the
/// doctrine stacks above it, and the cursor plane the engine
/// reserves. Planes whose zpos sits below the primary's are carried
/// but never assigned (below-canvas stacking is a documented future
/// line, not a silent behavior).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlaneInventory {
    /// The CRTC index this inventory serves.
    pub crtc_index: u32,
    /// The primary plane (the canvas carrier), when the driver has one.
    pub primary: Option<PlaneCaps>,
    /// Overlay planes at or above the primary's zpos, `(zpos, id)`
    /// ascending — the offload slots.
    pub overlays: Vec<PlaneCaps>,
    /// The cursor sprite plane — reserved, never assigned.
    pub cursor: Option<PlaneCaps>,
    /// Overlay planes strictly below the primary's zpos (carried for
    /// honesty, excluded from assignment).
    pub below_primary: Vec<PlaneCaps>,
}

impl PlaneInventory {
    /// Collect from a backend: every plane that can feed CRTC index
    /// `crtc_index`, classified by kind, ordered by `(zpos, id)`.
    ///
    /// The zpos of each plane is read through the object-property
    /// walk (the `ZPOS` well-known property) — the same table the
    /// atomic request writes into, so the solver's stacking model and
    /// the device's cannot drift.
    ///
    /// # Errors
    /// [`ldp_display::DisplayError`] when the topology or a plane's
    /// snapshot cannot be read — a bring-up failure, not a policy
    /// state (the caller fails loudly; there is no degraded
    /// inventory).
    pub fn collect(
        backend: &dyn KmsBackend,
        crtc_index: u32,
    ) -> Result<Self, ldp_display::DisplayError> {
        let topology = backend.topology()?;
        let mut planes = Vec::new();
        for id in &topology.planes {
            let info = backend.plane_info(*id)?;
            if !info.feeds_crtc_index(crtc_index) {
                continue;
            }
            let zpos = read_zpos(backend, *id);
            planes.push(PlaneCaps { info, zpos });
        }
        planes.sort_by_key(|p| (p.zpos, p.info.id.raw()));
        let primary_zpos = planes
            .iter()
            .find(|p| p.info.kind == PlaneType::Primary)
            .map_or(0, |p| p.zpos);
        let mut inventory = Self {
            crtc_index,
            primary: None,
            overlays: Vec::new(),
            cursor: None,
            below_primary: Vec::new(),
        };
        for plane in planes {
            match plane.info.kind {
                PlaneType::Primary => inventory.primary = Some(plane),
                PlaneType::Cursor => inventory.cursor = Some(plane),
                PlaneType::Overlay => {
                    if plane.zpos >= primary_zpos {
                        inventory.overlays.push(plane);
                    } else {
                        inventory.below_primary.push(plane);
                    }
                }
                _ => {
                    // Future plane classes (non-exhaustive enum): kept
                    // out of the overlay set — an unknown class has no
                    // documented stacking contract.
                    inventory.below_primary.push(plane);
                }
            }
        }
        Ok(inventory)
    }

    /// The overlay slot count available to the doctrine.
    #[must_use]
    pub fn overlay_slots(&self) -> usize {
        self.overlays.len()
    }

    /// Find an overlay plane by id (test convenience).
    #[must_use]
    pub fn overlay(&self, id: PlaneId) -> Option<&PlaneCaps> {
        self.overlays.iter().find(|p| p.info.id == id)
    }
}

/// Read one plane's `ZPOS` through the object-property walk.
fn read_zpos(backend: &dyn KmsBackend, plane: PlaneId) -> u64 {
    let props = backend.object_properties(AnyId::Plane(plane)).ok();
    let find = |props: &ldp_display::props::ObjectProperties| -> Option<u64> {
        for (pid, value) in &props.entries {
            if let Ok(info) = backend.property(*pid) {
                if info.name.as_str() == "ZPOS" {
                    return Some(*value);
                }
            }
        }
        None
    };
    props.as_ref().and_then(find).unwrap_or(0)
}
