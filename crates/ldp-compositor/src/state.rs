//! Surface state: the pending accumulator, the committed snapshot, and
//! the atomic commit with its damage accounting.
//!
//! A surface's state (buffer, transform, scale, input and opaque
//! regions, color description, presentation mode) accumulates in a
//! `PendingState` (see `pending.rs`) and applies atomically on
//! commit — the
//! `ldp.core.surface` contract. The committed form is an immutable
//! [`SurfaceState`] (cheap to clone and to share into snapshots).
//!
//! Commit damage rules (surface-local, feeding the engine's content
//! rule):
//!
//! * explicit `damage` / `damage_buffer` rects accumulate in the pending
//!   damage region;
//! * a *different* buffer attached with no explicit damage ⇒ the whole
//!   surface is damaged (content is unknowable);
//! * a buffer geometry change ⇒ old ∪ new bounds (coverage rule input);
//! * a detach ⇒ the old bounds (unmap damage);
//! * re-attaching the *same* buffer with no damage ⇒ nothing (a null
//!   re-attach is a no-op by design).

use ldp_core::color::{ColorDescription, HdrMetadata};
use ldp_core::geometry::{Rect, Region, Transform};
use ldp_core::scale::ScaleFactor;
use ldp_core::time::PresentationMode;

/// The attached buffer, as the scene graph needs it: geometry plus a
/// stable identity for "did the buffer change".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BufferAttachment {
    /// Buffer-space width in device pixels.
    pub width: u32,
    /// Buffer-space height in device pixels.
    pub height: u32,
    /// Monotonic identity of the underlying buffer object (attach slot);
    /// two attachments of the same id are the same content source.
    pub generation: u64,
}

impl BufferAttachment {
    /// The surface-space size of this buffer under `transform`.
    #[must_use]
    pub fn surface_size(self, transform: Transform, scale: ScaleFactor) -> (u32, u32) {
        let size = transform.transform_size(ldp_core::geometry::Size::new(self.width, self.height));
        // Logical size = device size inverse-scaled, rounded outward so
        // the logical surface never claims less area than the buffer
        // covers. One extra device pixel can appear at fractional scales;
        // the damage rules treat the surface bounds as authoritative.
        let lw = (u64::from(size.w) * 256)
            .div_ceil(u64::from(scale.to_q8()))
            .min(u64::from(u32::MAX)) as u32;
        let lh = (u64::from(size.h) * 256)
            .div_ceil(u64::from(scale.to_q8()))
            .min(u64::from(u32::MAX)) as u32;
        (lw, lh)
    }
}

/// The committed state of one surface — everything the scene graph and
/// damage engine read. Immutable by construction; commits replace it.
#[derive(Clone, Debug)]
pub struct SurfaceState {
    /// The attached buffer (`None` = unmapped).
    pub buffer: Option<BufferAttachment>,
    /// Buffer-to-surface orientation.
    pub transform: Transform,
    /// Buffer pixel ↔ logical unit scale.
    pub scale: ScaleFactor,
    /// Input region (surface coordinates); empty disables input.
    pub input_region: Region,
    /// Opaque region (surface coordinates); empty = fully translucent.
    pub opaque_region: Region,
    /// Parametric color description of the content.
    pub color: ColorDescription,
    /// Static HDR10 metadata (when the content is HDR).
    pub hdr: Option<HdrMetadata>,
    /// Presentation scheduling policy.
    pub presentation: PresentationMode,
}

impl Default for SurfaceState {
    fn default() -> Self {
        SurfaceState {
            buffer: None,
            transform: Transform::Normal,
            scale: ScaleFactor::IDENTITY,
            input_region: Region::new(),
            opaque_region: Region::new(),
            color: ColorDescription::srgb_sdr(),
            hdr: None,
            presentation: PresentationMode::Vsync,
        }
    }
}

impl SurfaceState {
    /// The surface-local bounds of the current mapping (empty when
    /// unmapped): the buffer's logical footprint at the origin.
    #[must_use]
    pub fn bounds(&self) -> Rect {
        self.buffer.map_or(Rect::EMPTY, |b| {
            let (w, h) = b.surface_size(self.transform, self.scale);
            Rect::new(0, 0, w, h)
        })
    }

    /// Whether the surface is mapped (has a buffer).
    #[must_use]
    pub fn is_mapped(&self) -> bool {
        self.buffer.is_some()
    }

    /// Map a buffer-coordinate rect into surface coordinates (the
    /// `damage_buffer` conversion): transform by the buffer-to-surface
    /// orientation, then inverse-scale, rounding **outward** so the
    /// damaged logical area never shrinks below the device pixels that
    /// changed.
    #[must_use]
    pub fn buffer_to_surface(&self, r: Rect) -> Rect {
        let Some(buf) = self.buffer else {
            return Rect::EMPTY;
        };
        // Transform in device space, using the *pending* buffer dims.
        let t = self.transform.transform_rect(r, buf.width, buf.height);
        // Inverse-scale outward: floor the origin, ceil the far corner.
        let q8 = u64::from(self.scale.to_q8());
        let x0 = (i64::from(t.x) * 256).div_euclid(q8 as i64);
        let y0 = (i64::from(t.y) * 256).div_euclid(q8 as i64);
        let x1 = ((i64::from(t.x) + i64::from(t.w)) * 256 + q8 as i64 - 1).div_euclid(q8 as i64);
        let y1 = ((i64::from(t.y) + i64::from(t.h)) * 256 + q8 as i64 - 1).div_euclid(q8 as i64);
        let w = (x1 - x0).clamp(0, i64::from(u32::MAX)) as u32;
        let h = (y1 - y0).clamp(0, i64::from(u32::MAX)) as u32;
        Rect::new(
            x0.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
            y0.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
            w,
            h,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_unmapped_srgb() {
        let s = SurfaceState::default();
        assert!(!s.is_mapped());
        assert_eq!(s.bounds(), Rect::EMPTY);
        assert!(s.opaque_region.is_empty());
        assert_eq!(s.scale.to_q8(), 256);
    }
}
