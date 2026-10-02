//! GPU node model — devices, node kinds, and the selection policy.
//!
//! A DRM device exposes up to three character nodes: **primary**
//! (`cardN`, needs DRM-Master for modesetting), **control** (legacy
//! privileged ioctl node), and **render** (`renderN128`, no master
//! rights, the right node for GPU compute/texture work). A compositor
//! renders through a render node and hands buffers to a primary node
//! via the display backend — so the policy here is: *the render node
//! of the first device that has one; the primary node only as a
//! fallback*, because a primary fallback forces DRM-Master negotiation
//! with whatever else owns the device.
//!
//! The policy is a pure function over the discovered catalog — the
//! tests drive synthetic catalogs directly, and the real discovery
//! path (`drm_sys`) feeds the same types.

use crate::drm_sys::LibDrmNodes;

/// One device node kind.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NodeKind {
    /// `cardN` — modesetting, requires DRM-Master.
    Primary,
    /// `controlN` — legacy privileged ioctls.
    Control,
    /// `renderN` — unprivileged rendering.
    Render,
}

impl NodeKind {
    /// The kernel's `DRM_NODE_*` index.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Primary => 0,
            Self::Control => 1,
            Self::Render => 2,
        }
    }
}

/// One discovered DRM device and its node paths.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct GpuDevice {
    /// `/dev/dri/cardN`, when present.
    pub primary: Option<String>,
    /// `/dev/dri/controlN`, when present.
    pub control: Option<String>,
    /// `/dev/dri/renderN`, when present.
    pub render: Option<String>,
    /// The kernel reported the primary node available.
    pub has_primary: bool,
    /// The kernel reported the render node available.
    pub has_render: bool,
}

impl GpuDevice {
    /// The node path this device offers for rendering under the
    /// selection policy: its render node, else its primary node.
    #[must_use]
    pub fn best_render(&self) -> Option<&str> {
        self.render.as_deref().or(self.primary.as_deref())
    }

    /// Whether the policy's choice needs DRM-Master rights (true only
    /// for the primary-node fallback).
    #[must_use]
    pub fn needs_master(&self) -> bool {
        self.render.is_none() && self.primary.is_some()
    }
}

/// One selected render node: the device plus the path to open.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GpuNode {
    /// The device the node belongs to.
    pub device: GpuDevice,
    /// The chosen node kind.
    pub kind: NodeKind,
    /// The node path.
    pub path: String,
}

/// The discovered device catalog.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct NodeCatalog {
    devices: Vec<GpuDevice>,
}

impl NodeCatalog {
    /// An empty catalog (no GPU on the machine).
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            devices: Vec::new(),
        }
    }

    /// A catalog over the given devices, in discovery order.
    #[must_use]
    pub fn from_devices(devices: Vec<GpuDevice>) -> Self {
        Self { devices }
    }

    /// Discover through the runtime libdrm layer.
    ///
    /// # Errors
    /// Propagates [`crate::drm_sys::NodeError`] from the dlopen walk.
    pub fn discover(lib: &LibDrmNodes) -> Result<Self, crate::drm_sys::NodeError> {
        Ok(Self::from_devices(lib.devices()?))
    }

    /// The devices in discovery order.
    #[must_use]
    pub fn devices(&self) -> &[GpuDevice] {
        &self.devices
    }

    /// The selection policy: the **render node of the first device
    /// that has one**; otherwise the primary node of the first device
    /// that has one (a DRM-Master fallback); otherwise `None`.
    ///
    /// Deterministic by construction: discovery order is kernel device
    /// order, and the walk takes the first match in that order.
    #[must_use]
    pub fn select_render_node(&self) -> Option<GpuNode> {
        for device in &self.devices {
            if let Some(path) = &device.render {
                return Some(GpuNode {
                    device: device.clone(),
                    kind: NodeKind::Render,
                    path: path.clone(),
                });
            }
        }
        for device in &self.devices {
            if let Some(path) = &device.primary {
                return Some(GpuNode {
                    device: device.clone(),
                    kind: NodeKind::Primary,
                    path: path.clone(),
                });
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(render: Option<&str>, primary: Option<&str>) -> GpuDevice {
        GpuDevice {
            primary: primary.map(str::to_owned),
            control: None,
            render: render.map(str::to_owned),
            has_primary: primary.is_some(),
            has_render: render.is_some(),
        }
    }

    #[test]
    fn node_kind_indices_match_the_kernel() {
        assert_eq!(NodeKind::Primary.index(), 0);
        assert_eq!(NodeKind::Control.index(), 1);
        assert_eq!(NodeKind::Render.index(), 2);
    }

    #[test]
    fn selection_prefers_render_over_primary() {
        let catalog = NodeCatalog::from_devices(vec![
            dev(None, Some("/dev/dri/card0")),
            dev(Some("/dev/dri/renderD128"), Some("/dev/dri/card1")),
        ]);
        let chosen = catalog.select_render_node().expect("a node exists");
        assert_eq!(chosen.path, "/dev/dri/renderD128");
        assert_eq!(chosen.kind, NodeKind::Render);
        assert!(!chosen.device.needs_master());
    }

    #[test]
    fn selection_falls_back_to_primary_requiring_master() {
        let catalog = NodeCatalog::from_devices(vec![dev(None, Some("/dev/dri/card0"))]);
        let chosen = catalog.select_render_node().expect("a node exists");
        assert_eq!(chosen.path, "/dev/dri/card0");
        assert_eq!(chosen.kind, NodeKind::Primary);
        assert!(chosen.device.needs_master());
    }

    #[test]
    fn empty_catalog_selects_nothing() {
        assert!(NodeCatalog::empty().select_render_node().is_none());
        assert!(NodeCatalog::from_devices(vec![dev(None, None)])
            .select_render_node()
            .is_none());
    }

    #[test]
    fn selection_is_first_match_in_discovery_order() {
        // Two render-capable devices: the first one wins, always.
        let catalog = NodeCatalog::from_devices(vec![
            dev(Some("/dev/dri/renderD128"), Some("/dev/dri/card0")),
            dev(Some("/dev/dri/renderD129"), Some("/dev/dri/card1")),
        ]);
        assert_eq!(
            catalog.select_render_node().unwrap().path,
            "/dev/dri/renderD128"
        );
        // Same catalog, same answer — determinism.
        assert_eq!(
            catalog.select_render_node().unwrap().path,
            "/dev/dri/renderD128"
        );
    }
}
