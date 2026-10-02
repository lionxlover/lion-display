//! Safe read-only views over buffer storage.
//!
//! [`BufferView`] pairs a byte slice with a validated [`BufferGeometry`] and
//! a stable identity key (the future GL backend's texture-cache handle).
//! Construction is the *only* bounds gate: the geometry's spanned extent must
//! fit the supplied data, after which every per-pixel plane address is
//! in-bounds by construction (block arithmetic derived from
//! [`ldp_core::buffer::FourCC::plane_block`]).

use ldp_core::buffer::BufferGeometry;

use crate::errors::RendererError;

/// A borrowed buffer: data + validated geometry + identity.
#[derive(Clone, Debug)]
pub struct BufferView<'a> {
    key: u64,
    data: &'a [u8],
    geometry: BufferGeometry,
}

impl<'a> BufferView<'a> {
    /// Validate and build.
    ///
    /// # Errors
    ///
    /// [`RendererError::BufferDataTooSmall`] when the data slice is shorter
    /// than the geometry's spanned extent — i.e. the geometry was validated
    /// against a larger storage object than the one supplied here.
    pub fn new(
        key: u64,
        data: &'a [u8],
        geometry: BufferGeometry,
    ) -> Result<BufferView<'a>, RendererError> {
        let needed = geometry.spanned_bytes();
        if needed > data.len() as u64 {
            return Err(RendererError::BufferDataTooSmall {
                needed,
                have: data.len() as u64,
            });
        }
        Ok(BufferView {
            key,
            data,
            geometry,
        })
    }

    /// The stable buffer identity (attach-slot id in the compositor; the
    /// texture-cache key in the GL backend).
    #[must_use]
    pub fn key(&self) -> u64 {
        self.key
    }

    /// The backing bytes.
    #[must_use]
    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    /// The validated geometry.
    #[must_use]
    pub fn geometry(&self) -> &BufferGeometry {
        &self.geometry
    }

    /// Byte slice of one full plane row (stride-wide), starting at the
    /// plane's offset. `row` must be `< ceil(height / sub_h)` of the plane —
    /// callers derive rows from validated buffer coordinates.
    #[cfg(test)]
    pub(crate) fn plane_row(&self, plane: usize, row: u32) -> &[u8] {
        let layout = self.geometry.planes()[plane];
        let start = layout.offset as usize + layout.stride as usize * row as usize;
        &self.data[start..start + layout.stride as usize]
    }

    /// Address of a block within a plane row: the byte index into `data`
    /// where the block covering pixel `block_x * sub_w` of `row` lives.
    /// `block_x` is block-granular (callers pass `pixel_x / sub_w`). The
    /// returned index is always `< data.len()` for in-range coordinates
    /// (construction proved the spanned extent fits).
    #[cfg(test)]
    pub(crate) fn block_addr(&self, plane: usize, block_x: u32, row: u32) -> usize {
        let layout = self.geometry.planes()[plane];
        let (block_bytes, sub_w, sub_h) = self.geometry.format().plane_block(plane as u32);
        debug_assert!(block_x < self.geometry.width().div_ceil(sub_w));
        debug_assert!(row < self.geometry.height().div_ceil(sub_h));
        layout.offset as usize
            + layout.stride as usize * row as usize
            + block_x as usize * block_bytes as usize
    }

    /// Rows in a plane (ceil of height over the plane's vertical sampling).
    #[cfg(test)]
    pub(crate) fn plane_rows(&self, plane: usize) -> u32 {
        let (_, _, sub_h) = self.geometry.format().plane_block(plane as u32);
        self.geometry.height().div_ceil(sub_h).max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ldp_core::buffer::{FourCC, Modifier, PlaneLayout};

    fn geometry(w: u32, h: u32, format: FourCC) -> BufferGeometry {
        let layout = BufferGeometry::simple_layout(w, h, format);
        let storage: u64 = layout
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let (_, _, sub_h) = format.plane_block(i as u32);
                u64::from(p.offset) + u64::from(p.stride) * u64::from(h.div_ceil(sub_h))
            })
            .max()
            .unwrap_or(0);
        BufferGeometry::new(w, h, format, Modifier::LINEAR, &layout, storage).unwrap()
    }

    #[test]
    fn rejects_short_data() {
        let g = geometry(64, 64, FourCC::XRGB8888);
        let short = vec![0u8; 1024];
        assert!(matches!(
            BufferView::new(1, &short, g),
            Err(RendererError::BufferDataTooSmall { needed: 16_384, .. })
        ));
    }

    #[test]
    fn plane_rows_and_strides() {
        let data = vec![0u8; 6144];
        let g = geometry(64, 64, FourCC::NV12);
        let v = BufferView::new(7, &data, g).unwrap();
        assert_eq!(v.key(), 7);
        assert_eq!(v.plane_rows(0), 64);
        assert_eq!(v.plane_rows(1), 32);
        // Chroma row 0 starts right after 64 luma rows.
        let luma = v.plane_row(0, 0);
        let chroma = v.plane_row(1, 0);
        let offset = chroma.as_ptr() as usize - luma.as_ptr() as usize;
        assert_eq!(offset, 64 * 64);
        assert_eq!(chroma.len(), 64);
        // The last luma row is stride-addressable.
        assert_eq!(v.plane_row(0, 63).len(), 64);
    }

    #[test]
    fn block_addresses_stay_inside_data() {
        for format in FourCC::SUPPORTED {
            let g = geometry(19, 7, format);
            let data = vec![0u8; g.spanned_bytes() as usize];
            let v = BufferView::new(0, &data, g).unwrap();
            for plane in 0..format.plane_count() as usize {
                let (_, sub_w, _) = format.plane_block(plane as u32);
                for row in 0..v.plane_rows(plane) {
                    for bx in (0..19u32).step_by(sub_w as usize) {
                        let addr = v.block_addr(plane, bx / sub_w, row);
                        assert!(addr < data.len(), "{format} out of bounds");
                    }
                }
            }
        }
    }

    #[test]
    fn custom_offsets_are_respected() {
        // Plane 1 at a nonzero offset with padding before it.
        let planes = [
            PlaneLayout {
                offset: 0,
                stride: 64,
            },
            PlaneLayout {
                offset: 4096,
                stride: 64,
            },
        ];
        let g = BufferGeometry::new(64, 64, FourCC::NV12, Modifier::LINEAR, &planes, 8192).unwrap();
        let data = vec![0u8; 8192];
        let v = BufferView::new(0, &data, g).unwrap();
        let luma = v.plane_row(0, 0);
        let chroma = v.plane_row(1, 0);
        assert_eq!(chroma.as_ptr() as usize - luma.as_ptr() as usize, 4096);
    }
}
