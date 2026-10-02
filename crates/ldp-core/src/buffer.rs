//! Buffer formats and geometry.
//!
//! DRM fourcc codes, format modifiers, plane layouts, and the validated
//! geometry of a buffer (`docs/architecture.md` §2.3 resource ceilings,
//! `spec/core.toml` `shm_pool.create_buffer` / `dmabuf.create`).

use core::fmt;

/// A DRM fourcc format code.
///
/// Values use the standard DRM convention: the ASCII tag packed
/// little-endian, e.g. `XR24` (XRGB8888) = `0x3432_5258`. The constants
/// below are the formats the v1 renderer family supports; unknown fourccs
/// are rejected with `invalid_buffer` rather than guessed at.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
#[repr(transparent)]
pub struct FourCC {
    code: u32,
}

impl fmt::Display for FourCC {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let bytes = self.code.to_le_bytes();
        let tag: Vec<char> = bytes.iter().map(|b| *b as char).collect();
        write!(
            f,
            "{} ({:#010x})",
            tag.into_iter().collect::<String>(),
            self.code
        )
    }
}

impl FourCC {
    /// Build from a raw code.
    pub const fn from_code(code: u32) -> FourCC {
        FourCC { code }
    }

    /// Build from an ASCII tag like `"XR24"`.
    pub fn from_tag(tag: &str) -> Option<FourCC> {
        let bytes = tag.as_bytes();
        if bytes.len() != 4 {
            return None;
        }
        Some(FourCC {
            code: u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        })
    }

    /// Raw code.
    #[must_use]
    pub const fn code(self) -> u32 {
        self.code
    }

    // --- RGB formats (byte order as named: e.g. ARGB = A,R,G,B in memory
    // on little-endian, matching DRM's naming convention). ---

    /// 32-bit XRGB8888 (opaque, ignored alpha byte).
    pub const XRGB8888: FourCC = FourCC { code: 0x3432_5258 }; // 'XR24'
    /// 32-bit ARGB8888 (premultiplied-alpha capable).
    pub const ARGB8888: FourCC = FourCC { code: 0x3432_5241 }; // 'AR24'
    /// 32-bit XBGR8888.
    pub const XBGR8888: FourCC = FourCC { code: 0x3432_4258 }; // 'XB24'
    /// 32-bit ABGR8888 (the Vulkan-friendly RGBA byte order).
    pub const ABGR8888: FourCC = FourCC { code: 0x3432_4241 }; // 'AB24'
    /// 24-bit packed RGB888.
    pub const RGB888: FourCC = FourCC { code: 0x3432_4752 }; // 'RG24'
    /// 24-bit packed BGR888.
    pub const BGR888: FourCC = FourCC { code: 0x3432_4742 }; // 'BG24'
    /// 16-bit RGB565.
    pub const RGB565: FourCC = FourCC { code: 0x3631_4752 }; // 'RG16'

    // --- YUV packed/planar formats. ---

    /// 4:2:2 packed YUYV.
    pub const YUYV: FourCC = FourCC { code: 0x5659_5559 }; // 'YUYV'
    /// 4:2:0 planar NV12.
    pub const NV12: FourCC = FourCC { code: 0x3231_564e }; // 'NV12'
    /// 4:2:0 10-bit planar P010.
    pub const P010: FourCC = FourCC { code: 0x3031_3050 }; // 'P010'
    /// 4:2:0 planar YUV420.
    pub const YUV420: FourCC = FourCC { code: 0x3231_5559 }; // 'YU12'

    /// All formats a v1 LDP server must understand.
    pub const SUPPORTED: [FourCC; 11] = [
        Self::XRGB8888,
        Self::ARGB8888,
        Self::XBGR8888,
        Self::ABGR8888,
        Self::RGB888,
        Self::BGR888,
        Self::RGB565,
        Self::YUYV,
        Self::NV12,
        Self::P010,
        Self::YUV420,
    ];

    /// Whether the format is one of [`FourCC::SUPPORTED`].
    #[must_use]
    pub fn is_supported(self) -> bool {
        Self::SUPPORTED.contains(&self)
    }

    /// Bytes per horizontal *block* of plane 0. For single-plane formats a
    /// block spans `subsampling().0` pixels in width (e.g. YUYV packs 2
    /// pixels into 4 bytes). For planar formats this is the plane-0 (luma)
    /// *sample* size and plane 0 is full-resolution; use [`FourCC::plane_block`]
    /// for per-plane layout math — chroma planes have different block shapes
    /// (interleaved Cb+Cr pairs, 10-bit samples).
    #[must_use]
    pub const fn bytes_per_block(self) -> u32 {
        match self.code {
            // 32-bit RGB plus YUYV (2 pixels per 4-byte block).
            0x3432_5258 | 0x3432_5241 | 0x3432_4258 | 0x3432_4241 | 0x5659_5559 => 4,
            0x3432_4752 | 0x3432_4742 => 3, // 24-bit RGB
            // RGB565 pixel and P010 10-bit luma sample are both 2 bytes.
            0x3631_4752 | 0x3031_3050 => 2,
            0x3231_564e | 0x3231_5559 => 1, // 8-bit planar luma sample
            _ => 0,                         // unknown
        }
    }

    /// Per-plane block geometry: `(bytes_per_block, sub_w, sub_h)` of plane
    /// `plane`.
    ///
    /// A *block* is the unit the plane's rows are measured in: `ceil(width /
    /// sub_w) * bytes_per_block` bytes per row, `ceil(height / sub_h)` rows.
    /// Plane 0 (luma/RGB) is never subsampled horizontally except for packed
    /// multi-pixel blocks (YUYV). Chroma planes carry their real sample
    /// packing:
    ///
    /// * NV12 plane 1: interleaved `Cb, Cr` byte pair per 2×2 luma block —
    ///   `(2, 2, 2)`, so a 64-wide NV12 has 64-byte chroma rows,
    /// * P010 plane 1: interleaved 10-bit `Cb, Cr` u16 pair per 2×2 luma
    ///   block — `(4, 2, 2)`, and plane 0 samples are 2 bytes — `(2, 1, 1)`,
    /// * YUV420 planes 1/2: separate 1-byte U and V planes, `(1, 2, 2)`.
    ///
    /// Single-plane formats fold their pixel-grid block geometry in. An
    /// out-of-range plane index returns `(0, 1, 1)` — a zero block size that
    /// validation rejects.
    #[must_use]
    pub const fn plane_block(self, plane: u32) -> (u32, u32, u32) {
        match self.code {
            0x3231_564e => match plane {
                0 => (1, 1, 1),
                1 => (2, 2, 2),
                _ => (0, 1, 1),
            },
            0x3031_3050 => match plane {
                0 => (2, 1, 1),
                1 => (4, 2, 2),
                _ => (0, 1, 1),
            },
            0x3231_5559 => match plane {
                0 => (1, 1, 1),
                1 | 2 => (1, 2, 2),
                _ => (0, 1, 1),
            },
            _ => (
                self.bytes_per_block(),
                self.subsampling().0,
                self.subsampling().1,
            ),
        }
    }

    /// Horizontal/vertical subsampling factors of the block grid.
    /// `(1, 1)` for unpacked RGB; YUYV is `(2, 1)` (horizontal packing);
    /// 4:2:0 planars are `(2, 2)`.
    #[must_use]
    pub const fn subsampling(self) -> (u32, u32) {
        match self.code {
            0x3231_564e | 0x3231_5559 | 0x3031_3050 => (2, 2),
            0x5659_5559 => (2, 1),
            _ => (1, 1),
        }
    }

    /// Number of planes the format uses.
    #[must_use]
    pub const fn plane_count(self) -> u32 {
        match self.code {
            0x3231_564e | 0x3031_3050 => 2, // NV12 / P010: Y + interleaved UV
            0x3231_5559 => 3,               // YUV420: Y, U, V
            0x3432_5258 | 0x3432_5241 | 0x3432_4258 | 0x3432_4241 | 0x3432_4752 | 0x3432_4742
            | 0x3631_4752 | 0x5659_5559 => 1, // RGB + packed YUYV
            _ => 0,                         // unknown
        }
    }

    /// Whether this is a planar/semiplanar (multi-plane) format.
    #[must_use]
    pub const fn is_planar(self) -> bool {
        self.plane_count() > 1
    }
}

/// A DRM format modifier (vendor tiling/compression layout).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[repr(transparent)]
pub struct Modifier {
    code: u64,
}

impl Modifier {
    /// Linear (untiled, uncompressed) layout.
    pub const LINEAR: Modifier = Modifier { code: 0 };
    /// The "no modifier" sentinel used by legacy APIs — never a real
    /// allocation layout; importers must treat it as "vendor default".
    pub const INVALID: Modifier = Modifier {
        code: 0x00ff_ffff_ffff_ffff,
    };
    /// Intel X-tiling (legacy i915).
    pub const INTEL_X: Modifier = Modifier {
        code: fourcc_mod_code(b'I', 1),
    };
    /// Intel Y-tiling (legacy i915).
    pub const INTEL_Y: Modifier = Modifier {
        code: fourcc_mod_code(b'I', 2),
    };

    /// Build from a raw code.
    pub const fn from_code(code: u64) -> Modifier {
        Modifier { code }
    }

    /// Raw code.
    #[must_use]
    pub const fn code(self) -> u64 {
        self.code
    }

    /// Whether this is the linear layout.
    #[must_use]
    pub const fn is_linear(self) -> bool {
        self.code == 0
    }

    /// Whether this is the INVALID sentinel.
    #[must_use]
    pub const fn is_invalid(self) -> bool {
        self.code == Self::INVALID.code
    }
}

impl fmt::Display for Modifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_linear() {
            f.write_str("LINEAR")
        } else if self.is_invalid() {
            f.write_str("INVALID")
        } else {
            write!(f, "{:#018x}", self.code)
        }
    }
}

/// DRM vendor-modifier packing helper: `DRM_FORMAT_MOD_VENDOR(v) | param`.
const fn fourcc_mod_code(vendor: u8, param: u64) -> u64 {
    ((vendor as u64) << 56) | param
}

/// Ceil division used by the layout math.
const fn ceil_div(a: u32, b: u32) -> u64 {
    // `as` casts: `From` is not const-callable in const fns yet.
    (a as u64).div_ceil(b as u64)
}

/// Layout of one buffer plane.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PlaneLayout {
    /// Offset of the plane's first row within the storage object, in bytes.
    pub offset: u32,
    /// Row stride in bytes.
    pub stride: u32,
}

/// The validated geometry of a buffer.
///
/// Constructed from `shm_pool.create_buffer` (single plane, stride
/// supplied) or `dmabuf.create` (up to four planes); validation errors
/// map to the wire `invalid_buffer` code at the protocol layer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BufferGeometry {
    width: u32,
    height: u32,
    format: FourCC,
    modifier: Modifier,
    planes: Vec<PlaneLayout>,
}

impl BufferGeometry {
    /// Maximum supported planes (wire limit).
    pub const MAX_PLANES: usize = 4;

    /// Validate and build a geometry.
    ///
    /// Rules (all enforced here so the compositor never sees an unsafe
    /// layout):
    ///
    /// * width/height in `1..=16384` and `area <= 2^31` — bounded so the
    ///   per-client buffer-byte ceiling cannot be bypassed by stride
    ///   multiplication overflow,
    /// * plane count exactly matches the format's requirement,
    /// * each stride is a multiple of 4 and at least the minimum row size
    ///   implied by the format,
    /// * offsets/strides keep every plane inside the supplied storage size
    ///   (`storage_bytes` from the pool or DMA-BUF).
    ///
    /// # Errors
    ///
    /// Returns a static diagnostic naming the first violated rule; the
    /// protocol layer maps it to the wire `invalid_buffer` error.
    pub fn new(
        width: u32,
        height: u32,
        format: FourCC,
        modifier: Modifier,
        planes: &[PlaneLayout],
        storage_bytes: u64,
    ) -> Result<BufferGeometry, &'static str> {
        if width == 0 || height == 0 || width > 16384 || height > 16384 {
            return Err("width/height out of range");
        }
        if (width as u64) * (height as u64) > 1 << 31 {
            return Err("pixel count exceeds 2^31");
        }
        if !format.is_supported() {
            return Err("unsupported format");
        }
        let expected = format.plane_count() as usize;
        if planes.len() != expected {
            return Err("plane count mismatch");
        }
        if planes.len() > Self::MAX_PLANES {
            return Err("too many planes");
        }
        for (i, p) in planes.iter().enumerate() {
            if p.stride % 4 != 0 {
                return Err("stride not 4-byte aligned");
            }
            // Per-plane block geometry: luma/RGB planes are full resolution,
            // chroma planes carry their own (sub)sampling and packing.
            let (bytes_per_block, sub_w, sub_h) = format.plane_block(i as u32);
            if bytes_per_block == 0 {
                return Err("unknown block size");
            }
            let row_bytes = ceil_div(width, sub_w) * bytes_per_block as u64;
            if (p.stride as u64) < row_bytes {
                return Err("stride smaller than row size");
            }
            let plane_rows = ceil_div(height, sub_h).max(1);
            let end = p.offset as u64 + p.stride as u64 * (plane_rows - 1) + row_bytes;
            if end > storage_bytes {
                return Err("plane exceeds storage");
            }
            if i > 0 && p.offset <= planes[i - 1].offset {
                // Planes must be in ascending offset order; equal offsets
                // cannot both hold data at positive height.
                return Err("planes not in ascending offset order");
            }
        }
        Ok(BufferGeometry {
            width,
            height,
            format,
            modifier,
            planes: planes.to_vec(),
        })
    }

    /// Width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Pixel format.
    #[must_use]
    pub const fn format(&self) -> FourCC {
        self.format
    }

    /// Layout modifier.
    #[must_use]
    pub const fn modifier(&self) -> Modifier {
        self.modifier
    }

    /// Plane layouts, plane 0 (luma/RGB) first.
    #[must_use]
    pub fn planes(&self) -> &[PlaneLayout] {
        &self.planes
    }

    /// Estimated storage bytes actually spanned by the planes.
    #[must_use]
    pub fn spanned_bytes(&self) -> u64 {
        self.planes
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let (bytes_per_block, sub_w, sub_h) = self.format.plane_block(i as u32);
                let row_bytes = ceil_div(self.width, sub_w) * bytes_per_block as u64;
                let rows = ceil_div(self.height, sub_h).max(1);
                p.offset as u64 + p.stride as u64 * (rows - 1) + row_bytes.max(1)
            })
            .max()
            .unwrap_or(0)
    }

    /// A default tightly-packed plane layout for `format` at
    /// `width x height` (helper for shm buffer creation; strides are
    /// 4-byte padded row sizes, planes are stacked).
    #[must_use]
    pub fn simple_layout(width: u32, height: u32, format: FourCC) -> Vec<PlaneLayout> {
        let mut planes = Vec::with_capacity(format.plane_count() as usize);
        let mut offset = 0u32;
        for plane in 0..format.plane_count() {
            let (bytes_per_block, sub_w, sub_h) = format.plane_block(plane);
            let row_bytes = ceil_div(width, sub_w) * u64::from(bytes_per_block);
            let stride = (row_bytes.div_ceil(4) * 4).min(u64::from(u32::MAX)) as u32;
            let rows = ceil_div(height, sub_h).max(1);
            planes.push(PlaneLayout { offset, stride });
            offset = offset.saturating_add(stride * rows as u32);
        }
        planes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fourcc_tags_and_codes_agree() {
        assert_eq!(FourCC::from_tag("XR24").unwrap(), FourCC::XRGB8888);
        assert_eq!(FourCC::XRGB8888.code(), 0x3432_5258);
        assert_eq!(FourCC::from_tag("AR24").unwrap(), FourCC::ARGB8888);
        assert_eq!(FourCC::from_tag("NV12").unwrap(), FourCC::NV12);
        assert_eq!(FourCC::from_tag("RG16").unwrap(), FourCC::RGB565);
        assert!(FourCC::from_tag("XR2").is_none());
        assert!(FourCC::from_tag("XR244").is_none());
        assert_eq!(FourCC::XRGB8888.to_string(), "XR24 (0x34325258)");
    }

    #[test]
    fn format_families() {
        assert_eq!(FourCC::XRGB8888.plane_count(), 1);
        assert!(!FourCC::XRGB8888.is_planar());
        assert_eq!(FourCC::NV12.plane_count(), 2);
        assert_eq!(FourCC::YUV420.plane_count(), 3);
        assert!(FourCC::NV12.is_planar());
        assert_eq!(FourCC::NV12.subsampling(), (2, 2));
        assert_eq!(FourCC::YUYV.subsampling(), (2, 1));
        assert_eq!(FourCC::XRGB8888.subsampling(), (1, 1));
        assert_eq!(FourCC::XRGB8888.bytes_per_block(), 4);
        assert_eq!(FourCC::YUYV.bytes_per_block(), 4);
        assert!(FourCC::XRGB8888.is_supported());
        assert!(!FourCC::from_code(0xDEAD_BEEF).is_supported());
        // Unknown code: zero block size signals "unknown" to validators.
        assert_eq!(FourCC::from_code(0xDEAD_BEEF).bytes_per_block(), 0);
    }

    #[test]
    fn modifier_display_and_predicates() {
        assert!(Modifier::LINEAR.is_linear());
        assert!(!Modifier::LINEAR.is_invalid());
        assert!(Modifier::INVALID.is_invalid());
        assert_eq!(Modifier::LINEAR.to_string(), "LINEAR");
        assert_eq!(Modifier::INVALID.to_string(), "INVALID");
        assert_eq!(Modifier::from_code(42).to_string(), "0x000000000000002a");
    }

    #[test]
    fn geometry_validation_rejects_bad_input() {
        let layout = BufferGeometry::simple_layout(64, 64, FourCC::XRGB8888);
        let storage = 64 * 64 * 4;
        // Correct case.
        assert!(
            BufferGeometry::new(64, 64, FourCC::XRGB8888, Modifier::LINEAR, &layout, storage)
                .is_ok()
        );
        // Out of range dims.
        assert!(
            BufferGeometry::new(0, 64, FourCC::XRGB8888, Modifier::LINEAR, &layout, storage)
                .is_err()
        );
        assert!(BufferGeometry::new(
            20000,
            64,
            FourCC::XRGB8888,
            Modifier::LINEAR,
            &layout,
            storage
        )
        .is_err());
        // Pixel count ceiling.
        assert!(BufferGeometry::new(
            16384,
            16384,
            FourCC::XRGB8888,
            Modifier::LINEAR,
            &layout,
            u64::MAX
        )
        .is_err());
        // Plane count mismatch.
        assert!(
            BufferGeometry::new(64, 64, FourCC::NV12, Modifier::LINEAR, &layout, storage).is_err()
        );
        // Stride too small.
        let bad = vec![PlaneLayout {
            offset: 0,
            stride: 16,
        }];
        assert!(
            BufferGeometry::new(64, 64, FourCC::XRGB8888, Modifier::LINEAR, &bad, storage).is_err()
        );
        // Plane exceeds storage.
        let over = vec![PlaneLayout {
            offset: 0,
            stride: 4096,
        }];
        assert!(
            BufferGeometry::new(64, 64, FourCC::XRGB8888, Modifier::LINEAR, &over, 1024).is_err()
        );
        // Unsupported format.
        assert!(BufferGeometry::new(
            64,
            64,
            FourCC::from_code(0xDEAD_BEEF),
            Modifier::LINEAR,
            &layout,
            storage
        )
        .is_err());
    }

    #[test]
    fn simple_layout_is_always_valid() {
        for format in FourCC::SUPPORTED {
            for (w, h) in [
                (1u32, 1u32),
                (17, 9),
                (1920, 1080),
                (3840, 2160),
                (1919, 1079),
            ] {
                let layout = BufferGeometry::simple_layout(w, h, format);
                let storage: u64 = layout
                    .iter()
                    .enumerate()
                    .map(|(i, p)| {
                        let (_, _, sub_h) = format.plane_block(i as u32);
                        u64::from(p.offset)
                            + u64::from(p.stride) * u64::from(h.div_ceil(sub_h).max(1))
                    })
                    .max()
                    .unwrap_or(0);
                let g = BufferGeometry::new(w, h, format, Modifier::LINEAR, &layout, storage)
                    .unwrap_or_else(|e| panic!("{format} {w}x{h}: {e}"));
                assert_eq!(g.width(), w);
                assert_eq!(g.height(), h);
                assert_eq!(g.format(), format);
            }
        }
    }

    #[test]
    fn planar_layouts_have_real_strides_and_offsets() {
        // NV12 64x64: luma 64 B rows (64 samples), chroma rows hold 32
        // interleaved Cb+Cr pairs = 64 B, plane 1 starts after 64 luma rows.
        let nv12 = BufferGeometry::simple_layout(64, 64, FourCC::NV12);
        assert_eq!(nv12[0].offset, 0);
        assert_eq!(nv12[0].stride, 64);
        assert_eq!(nv12[1].offset, 64 * 64);
        assert_eq!(nv12[1].stride, 64);
        // YUV420 17x9: luma rows padded to 20 B (9 rows); each separate
        // U/V plane has ceil(17/2)=9 samples per row -> 12 B padded, 5 rows.
        let yu12 = BufferGeometry::simple_layout(17, 9, FourCC::YUV420);
        assert_eq!(yu12[0].stride, 20);
        assert_eq!(yu12[1].offset, 20 * 9);
        assert_eq!(yu12[1].stride, 12);
        assert_eq!(yu12[2].offset, 20 * 9 + 12 * 5);
        assert_eq!(yu12[2].stride, 12);
        // P010 32x16: 2-byte luma samples -> 64 B rows; chroma rows are
        // 16 interleaved u16 Cb+Cr pairs = 64 B.
        let p010 = BufferGeometry::simple_layout(32, 16, FourCC::P010);
        assert_eq!(p010[0].offset, 0);
        assert_eq!(p010[0].stride, 64);
        assert_eq!(p010[1].offset, 64 * 16);
        assert_eq!(p010[1].stride, 64);
    }

    #[test]
    fn planar_spanned_bytes_covers_chroma_planes() {
        let layout = BufferGeometry::simple_layout(64, 64, FourCC::NV12);
        let g = BufferGeometry::new(
            64,
            64,
            FourCC::NV12,
            Modifier::LINEAR,
            &layout,
            u32::MAX as u64,
        )
        .unwrap();
        // 4096 luma + 32 chroma rows of 64 B.
        assert_eq!(g.spanned_bytes(), 4096 + 32 * 64);
        let layout = BufferGeometry::simple_layout(32, 16, FourCC::P010);
        let g = BufferGeometry::new(
            32,
            16,
            FourCC::P010,
            Modifier::LINEAR,
            &layout,
            u32::MAX as u64,
        )
        .unwrap();
        assert_eq!(g.spanned_bytes(), 64 * 16 + 64 * 8);
    }

    #[test]
    fn spanned_bytes_covers_the_planes() {
        let layout = BufferGeometry::simple_layout(100, 50, FourCC::XRGB8888);
        let g = BufferGeometry::new(
            100,
            50,
            FourCC::XRGB8888,
            Modifier::LINEAR,
            &layout,
            u32::MAX as u64,
        )
        .unwrap();
        assert_eq!(g.spanned_bytes(), 100 * 4 * 50);
        // YUYV: rows = h, row = 2 bytes/pixel.
        let layout = BufferGeometry::simple_layout(100, 50, FourCC::YUYV);
        let g = BufferGeometry::new(
            100,
            50,
            FourCC::YUYV,
            Modifier::LINEAR,
            &layout,
            u32::MAX as u64,
        )
        .unwrap();
        assert_eq!(g.spanned_bytes(), 100 * 2 * 50);
    }

    #[test]
    fn yuyv_odd_width_uses_ceil_blocks() {
        // 17 pixels -> 9 blocks of 4 bytes = 36-byte rows.
        let layout = BufferGeometry::simple_layout(17, 3, FourCC::YUYV);
        assert_eq!(layout[0].stride, 36);
        let storage = 36u64 * 3;
        assert!(
            BufferGeometry::new(17, 3, FourCC::YUYV, Modifier::LINEAR, &layout, storage).is_ok()
        );
        assert!(
            BufferGeometry::new(17, 3, FourCC::YUYV, Modifier::LINEAR, &layout, storage - 1)
                .is_err()
        );
    }
}
