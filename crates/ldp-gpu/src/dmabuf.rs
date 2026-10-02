//! DMA-BUF import descriptors and their EGL attribute encoding.
//!
//! A [`DmaBufDescriptor`] is what crosses the EGL seam: geometry,
//! format, modifier, and per-plane (fd, offset, stride) triples. Two
//! invariants matter enough to enforce at construction:
//!
//! * **Plane arity** — the descriptor's plane count must equal the
//!   format's (`ldp-core`'s plane algebra), and every plane must carry
//!   a nonnegative fd and a stride at least wide enough for one row
//!   of that plane's blocks.
//! * **Explicit modifiers** — `INVALID` (the driver-default sentinel)
//!   is rejected: the LDP contract is explicit layouts, because an
//!   implicit one makes buffer sharing nondeterministic across
//!   drivers.
//!
//! The attribute codec is the kernel of the import path.
//! [`DmaBufDescriptor::attribs`] emits the exact
//! `EGL_EXT_image_dma_buf_import` (+`_modifiers`) list — width,
//! height, fourcc, then per plane: fd, offset, pitch, modifier
//! (lo/hi), terminated by `EGL_NONE` — and [`DmaBufDescriptor::decode`]
//! is its inverse. The mock EGL context imports by *decoding the
//! encoded list*, so the round trip is exercised on every import the
//! tests perform, not just in a codec unit test.

#![forbid(unsafe_code)]

use ldp_core::buffer::{FourCC, Modifier};

/// One EGL attribute: `(name, value)`.
pub type EglAttrib = (u32, u64);

/// The per-plane attribute name tables: (fd, offset, pitch, modifier
/// lo, modifier hi), each indexed by plane slot.
type PlaneNameTables = ([u32; 4], [u32; 4], [u32; 4], [u32; 4], [u32; 4]);

/// EGL constant names used by the import path (eglplatform/eglext
/// values, stable ABI).
mod egl {
    /// `EGL_NONE` — the list terminator.
    pub const NONE: u32 = 0x3058;
    /// `EGL_WIDTH`.
    pub const WIDTH: u32 = 0x3057;
    /// `EGL_HEIGHT`.
    pub const HEIGHT: u32 = 0x3056;
    /// `EGL_LINUX_DRM_FOURCC_EXT`.
    pub const FOURCC: u32 = 0x3271;
    /// `EGL_DMA_BUF_PLANE0_FD_EXT`.
    pub const PLANE0_FD: u32 = 0x3272;
    /// `EGL_DMA_BUF_PLANE0_OFFSET_EXT`.
    pub const PLANE0_OFFSET: u32 = 0x3273;
    /// `EGL_DMA_BUF_PLANE0_PITCH_EXT`.
    pub const PLANE0_PITCH: u32 = 0x3274;
    /// `EGL_DMA_BUF_PLANE1_FD_EXT`.
    pub const PLANE1_FD: u32 = 0x3275;
    /// `EGL_DMA_BUF_PLANE1_OFFSET_EXT`.
    pub const PLANE1_OFFSET: u32 = 0x3276;
    /// `EGL_DMA_BUF_PLANE1_PITCH_EXT`.
    pub const PLANE1_PITCH: u32 = 0x3277;
    /// `EGL_DMA_BUF_PLANE2_FD_EXT`.
    pub const PLANE2_FD: u32 = 0x3278;
    /// `EGL_DMA_BUF_PLANE2_OFFSET_EXT`.
    pub const PLANE2_OFFSET: u32 = 0x3279;
    /// `EGL_DMA_BUF_PLANE2_PITCH_EXT`.
    pub const PLANE2_PITCH: u32 = 0x327A;
    /// `EGL_DMA_BUF_PLANE3_FD_EXT` (modifiers extension).
    pub const PLANE3_FD: u32 = 0x3440;
    /// `EGL_DMA_BUF_PLANE3_OFFSET_EXT`.
    pub const PLANE3_OFFSET: u32 = 0x3441;
    /// `EGL_DMA_BUF_PLANE3_PITCH_EXT`.
    pub const PLANE3_PITCH: u32 = 0x3442;
    /// `EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT`.
    pub const PLANE0_MOD_LO: u32 = 0x3443;
    /// `EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT`.
    pub const PLANE0_MOD_HI: u32 = 0x3447;
    /// `EGL_DMA_BUF_PLANE1_MODIFIER_LO_EXT`.
    pub const PLANE1_MOD_LO: u32 = 0x3444;
    /// `EGL_DMA_BUF_PLANE1_MODIFIER_HI_EXT`.
    pub const PLANE1_MOD_HI: u32 = 0x3448;
    /// `EGL_DMA_BUF_PLANE2_MODIFIER_LO_EXT`.
    pub const PLANE2_MOD_LO: u32 = 0x3445;
    /// `EGL_DMA_BUF_PLANE2_MODIFIER_HI_EXT`.
    pub const PLANE2_MOD_HI: u32 = 0x3449;
    /// `EGL_DMA_BUF_PLANE3_MODIFIER_LO_EXT`.
    pub const PLANE3_MOD_LO: u32 = 0x3446;
    /// `EGL_DMA_BUF_PLANE3_MODIFIER_HI_EXT`.
    pub const PLANE3_MOD_HI: u32 = 0x344A;
}

/// Per-plane (fd, offset, stride) addressing into one dma-buf.
///
/// The fd is the *sync-file-free* plain dma-buf descriptor — mock
/// contexts run their own integer namespace.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DmaBufPlane {
    /// The dma-buf file descriptor.
    pub fd: i32,
    /// Byte offset of the plane's first row.
    pub offset: u32,
    /// Row stride in bytes.
    pub stride: u32,
}

/// One importable buffer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DmaBufDescriptor {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Pixel format.
    pub format: FourCC,
    /// Layout modifier (explicit; never `INVALID`).
    pub modifier: Modifier,
    /// Per-plane addressing, in format plane order.
    pub planes: Vec<DmaBufPlane>,
}

impl DmaBufDescriptor {
    /// Construct with kernel-grade validation.
    ///
    /// # Errors
    /// [`DescriptorError`] naming the first violated invariant: zero
    /// extent, unsupported format, implicit modifier, plane arity
    /// mismatch, negative fd, undersized stride, or a plane count
    /// beyond the ABI's four.
    pub fn new(
        width: u32,
        height: u32,
        format: FourCC,
        modifier: Modifier,
        planes: &[DmaBufPlane],
    ) -> Result<Self, DescriptorError> {
        if width == 0 || height == 0 {
            return Err(DescriptorError::ZeroExtent);
        }
        if !format.is_supported() {
            return Err(DescriptorError::FormatUnsupported { format });
        }
        if modifier.is_invalid() {
            return Err(DescriptorError::ImplicitModifier);
        }
        let expected = format.plane_count() as usize;
        if planes.len() != expected {
            return Err(DescriptorError::PlaneArity {
                expected,
                got: planes.len(),
            });
        }
        if planes.len() > 4 {
            // Unreachable given the arity check; kept for the ABI pin.
            return Err(DescriptorError::TooManyPlanes { got: planes.len() });
        }
        for (i, plane) in planes.iter().enumerate() {
            if plane.fd < 0 {
                return Err(DescriptorError::NegativeFd { plane: i });
            }
            let (block_bytes, sub_w, _) = format.plane_block(i as u32);
            let blocks_across = width.div_ceil(sub_w);
            let min_stride = block_bytes * blocks_across;
            if plane.stride < min_stride {
                return Err(DescriptorError::StrideTooSmall {
                    plane: i,
                    min_stride,
                });
            }
        }
        Ok(Self {
            width,
            height,
            format,
            modifier,
            planes: planes.to_vec(),
        })
    }

    /// Encode the EGL import attribute list (width, height, fourcc,
    /// per-plane fd/offset/pitch/modifier, `EGL_NONE`-terminated).
    #[must_use]
    pub fn attribs(&self) -> Vec<EglAttrib> {
        let mut list = Vec::with_capacity(6 + self.planes.len() * 5);
        list.push((egl::WIDTH, u64::from(self.width)));
        list.push((egl::HEIGHT, u64::from(self.height)));
        list.push((egl::FOURCC, u64::from(self.format.code())));
        let (fd_n, off_n, pitch_n, mod_lo_n, mod_hi_n) = Self::plane_name_arrays();
        let modifier = self.modifier.code();
        for (i, plane) in self.planes.iter().enumerate() {
            list.push((fd_n[i], i64::from(plane.fd) as u64));
            list.push((off_n[i], u64::from(plane.offset)));
            list.push((pitch_n[i], u64::from(plane.stride)));
            // The 64-bit modifier crosses as two 32-bit halves; the
            // low word carries the bits the PRIME structure cares
            // about, the high word the vendor slot.
            list.push((mod_lo_n[i], modifier & 0xFFFF_FFFF));
            list.push((mod_hi_n[i], modifier >> 32));
        }
        list.push((egl::NONE, 0));
        list
    }

    /// Decode an attribute list back into a descriptor — the codec's
    /// inverse. `None` on unknown names, missing mandatory pairs, or
    /// a list that fails descriptor validation.
    #[must_use]
    pub fn decode(attribs: &[EglAttrib]) -> Option<Self> {
        let mut width = None;
        let mut height = None;
        let mut format = None;
        let mut fds = [None; 4];
        let mut offsets = [None; 4];
        let mut strides = [None; 4];
        let mut mods_lo = [None; 4];
        let mut mods_hi = [None; 4];
        for &(name, value) in attribs {
            // The codec table: one arm per name in the import
            // vocabulary; anything else is undecodable.
            match name {
                egl::NONE => break,
                egl::WIDTH => width = Some(u32::try_from(value).ok()?),
                egl::HEIGHT => height = Some(u32::try_from(value).ok()?),
                egl::FOURCC => format = Some(FourCC::from_code(u32::try_from(value).ok()?)),
                egl::PLANE0_FD => fds[0] = Some(i32::try_from(value).ok()?),
                egl::PLANE1_FD => fds[1] = Some(i32::try_from(value).ok()?),
                egl::PLANE2_FD => fds[2] = Some(i32::try_from(value).ok()?),
                egl::PLANE3_FD => fds[3] = Some(i32::try_from(value).ok()?),
                egl::PLANE0_OFFSET => offsets[0] = Some(u32::try_from(value).ok()?),
                egl::PLANE1_OFFSET => offsets[1] = Some(u32::try_from(value).ok()?),
                egl::PLANE2_OFFSET => offsets[2] = Some(u32::try_from(value).ok()?),
                egl::PLANE3_OFFSET => offsets[3] = Some(u32::try_from(value).ok()?),
                egl::PLANE0_PITCH => strides[0] = Some(u32::try_from(value).ok()?),
                egl::PLANE1_PITCH => strides[1] = Some(u32::try_from(value).ok()?),
                egl::PLANE2_PITCH => strides[2] = Some(u32::try_from(value).ok()?),
                egl::PLANE3_PITCH => strides[3] = Some(u32::try_from(value).ok()?),
                egl::PLANE0_MOD_LO => mods_lo[0] = Some(value & 0xFFFF_FFFF),
                egl::PLANE1_MOD_LO => mods_lo[1] = Some(value & 0xFFFF_FFFF),
                egl::PLANE2_MOD_LO => mods_lo[2] = Some(value & 0xFFFF_FFFF),
                egl::PLANE3_MOD_LO => mods_lo[3] = Some(value & 0xFFFF_FFFF),
                egl::PLANE0_MOD_HI => mods_hi[0] = Some(value & 0xFFFF_FFFF),
                egl::PLANE1_MOD_HI => mods_hi[1] = Some(value & 0xFFFF_FFFF),
                egl::PLANE2_MOD_HI => mods_hi[2] = Some(value & 0xFFFF_FFFF),
                egl::PLANE3_MOD_HI => mods_hi[3] = Some(value & 0xFFFF_FFFF),
                _ => return None,
            }
        }
        let format = format?;
        let count = format.plane_count() as usize;
        // All four planes share one modifier in this model (the
        // per-plane attrib slots exist for future split layouts; a
        // descriptor that disagrees across planes is not decodable).
        let mut merged: [Option<u64>; 4] = [None; 4];
        for i in 0..4 {
            match (mods_lo[i], mods_hi[i]) {
                (Some(lo), Some(hi)) => merged[i] = Some(hi << 32 | lo),
                (None, None) => {}
                // Half a modifier is a malformed list.
                _ => return None,
            }
        }
        let modifier = merged[0];
        if merged[..count].iter().any(|m| *m != modifier) {
            return None;
        }
        let mut planes = Vec::with_capacity(count);
        for i in 0..count {
            planes.push(DmaBufPlane {
                fd: fds[i]?,
                offset: offsets[i]?,
                stride: strides[i]?,
            });
        }
        Self::new(
            width?,
            height?,
            format,
            Modifier::from_code(modifier?),
            &planes,
        )
        .ok()
    }

    /// Per-plane attribute name tables, indexed by plane slot.
    fn plane_name_arrays() -> PlaneNameTables {
        (
            [
                egl::PLANE0_FD,
                egl::PLANE1_FD,
                egl::PLANE2_FD,
                egl::PLANE3_FD,
            ],
            [
                egl::PLANE0_OFFSET,
                egl::PLANE1_OFFSET,
                egl::PLANE2_OFFSET,
                egl::PLANE3_OFFSET,
            ],
            [
                egl::PLANE0_PITCH,
                egl::PLANE1_PITCH,
                egl::PLANE2_PITCH,
                egl::PLANE3_PITCH,
            ],
            [
                egl::PLANE0_MOD_LO,
                egl::PLANE1_MOD_LO,
                egl::PLANE2_MOD_LO,
                egl::PLANE3_MOD_LO,
            ],
            [
                egl::PLANE0_MOD_HI,
                egl::PLANE1_MOD_HI,
                egl::PLANE2_MOD_HI,
                egl::PLANE3_MOD_HI,
            ],
        )
    }
}

/// Descriptor validation failures.
#[derive(Clone, PartialEq, Eq, Debug)]
#[non_exhaustive]
pub enum DescriptorError {
    /// Zero width or height.
    ZeroExtent,
    /// The format is outside LDP's supported set.
    FormatUnsupported {
        /// The rejected fourcc.
        format: FourCC,
    },
    /// `DRM_FORMAT_MOD_INVALID` — implicit layouts are refused.
    ImplicitModifier,
    /// Plane count does not match the format's arity.
    PlaneArity {
        /// The format's plane count.
        expected: usize,
        /// What the descriptor carried.
        got: usize,
    },
    /// Beyond the ABI's four plane slots.
    TooManyPlanes {
        /// What the descriptor carried.
        got: usize,
    },
    /// A negative fd in a plane.
    NegativeFd {
        /// The offending plane index.
        plane: usize,
    },
    /// A stride too narrow for one row of that plane's blocks.
    StrideTooSmall {
        /// The offending plane index.
        plane: usize,
        /// The minimum legal stride.
        min_stride: u32,
    },
}

impl core::fmt::Display for DescriptorError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ZeroExtent => write!(f, "zero extent"),
            Self::FormatUnsupported { format } => write!(f, "unsupported format {format:?}"),
            Self::ImplicitModifier => write!(f, "implicit (INVALID) modifier refused"),
            Self::PlaneArity { expected, got } => {
                write!(f, "plane arity {got} does not match format's {expected}")
            }
            Self::TooManyPlanes { got } => write!(f, "{got} planes beyond the ABI's four"),
            Self::NegativeFd { plane } => write!(f, "plane {plane} has a negative fd"),
            Self::StrideTooSmall { plane, min_stride } => {
                write!(f, "plane {plane} stride below the row minimum {min_stride}")
            }
        }
    }
}

impl std::error::Error for DescriptorError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(w: u32, h: u32) -> Result<DmaBufDescriptor, DescriptorError> {
        DmaBufDescriptor::new(
            w,
            h,
            FourCC::ARGB8888,
            Modifier::LINEAR,
            &[DmaBufPlane {
                fd: 4,
                offset: 0,
                stride: w * 4,
            }],
        )
    }

    #[test]
    fn validation_rejects_each_invariant() {
        assert!(matches!(
            rgb(0, 4).unwrap_err(),
            DescriptorError::ZeroExtent
        ));
        let narrow = DmaBufDescriptor::new(
            4,
            4,
            FourCC::ARGB8888,
            Modifier::LINEAR,
            &[DmaBufPlane {
                fd: 4,
                offset: 0,
                stride: 15,
            }],
        );
        assert!(matches!(
            narrow.unwrap_err(),
            DescriptorError::StrideTooSmall { min_stride: 16, .. }
        ));
        let bad_fmt = DmaBufDescriptor::new(
            4,
            4,
            FourCC::from_code(0xdead_beef),
            Modifier::LINEAR,
            &[DmaBufPlane {
                fd: 1,
                offset: 0,
                stride: 16,
            }],
        );
        assert!(matches!(
            bad_fmt.unwrap_err(),
            DescriptorError::FormatUnsupported { .. }
        ));
        let implicit = DmaBufDescriptor::new(
            4,
            4,
            FourCC::ARGB8888,
            Modifier::INVALID,
            &[DmaBufPlane {
                fd: 1,
                offset: 0,
                stride: 16,
            }],
        );
        assert!(matches!(
            implicit.unwrap_err(),
            DescriptorError::ImplicitModifier
        ));
        let negative = DmaBufDescriptor::new(
            4,
            4,
            FourCC::ARGB8888,
            Modifier::LINEAR,
            &[DmaBufPlane {
                fd: -1,
                offset: 0,
                stride: 16,
            }],
        );
        assert!(matches!(
            negative.unwrap_err(),
            DescriptorError::NegativeFd { plane: 0 }
        ));
    }

    #[test]
    fn planar_arity_is_enforced_per_format() {
        // NV12 needs two planes.
        let one = DmaBufDescriptor::new(
            64,
            64,
            FourCC::NV12,
            Modifier::LINEAR,
            &[DmaBufPlane {
                fd: 4,
                offset: 0,
                stride: 64,
            }],
        );
        assert!(matches!(
            one.unwrap_err(),
            DescriptorError::PlaneArity {
                expected: 2,
                got: 1
            }
        ));
        let two = DmaBufDescriptor::new(
            64,
            64,
            FourCC::NV12,
            Modifier::LINEAR,
            &[
                DmaBufPlane {
                    fd: 4,
                    offset: 0,
                    stride: 64,
                },
                // The chroma plane is subsampled: 32 bytes per row suffice.
                DmaBufPlane {
                    fd: 4,
                    offset: 64 * 64,
                    stride: 64,
                },
            ],
        );
        assert!(two.is_ok());
    }

    #[test]
    fn attrib_codec_round_trips() {
        let descriptor = rgb(37, 11).unwrap();
        let attribs = descriptor.attribs();
        assert_eq!(attribs.last(), Some(&(egl::NONE, 0)));
        let back = DmaBufDescriptor::decode(&attribs).expect("round trip");
        assert_eq!(back, descriptor);
    }

    #[test]
    fn attrib_codec_round_trips_planar_and_vendor_modifiers() {
        let descriptor = DmaBufDescriptor::new(
            64,
            64,
            FourCC::NV12,
            Modifier::INTEL_X,
            &[
                DmaBufPlane {
                    fd: 7,
                    offset: 0,
                    stride: 64,
                },
                DmaBufPlane {
                    fd: 8,
                    offset: 4096,
                    stride: 64,
                },
            ],
        )
        .unwrap();
        let back = DmaBufDescriptor::decode(&descriptor.attribs()).expect("round trip");
        assert_eq!(back, descriptor);
    }

    #[test]
    fn decode_rejects_malformed_lists() {
        // Unknown attribute name.
        assert!(DmaBufDescriptor::decode(&[(0xDEAD, 1), (egl::NONE, 0)]).is_none());
        // Missing mandatory pair (no stride).
        let list = vec![
            (egl::WIDTH, 4),
            (egl::HEIGHT, 4),
            (egl::FOURCC, u64::from(FourCC::ARGB8888.code())),
            (egl::PLANE0_FD, 4),
            (egl::PLANE0_OFFSET, 0),
            (egl::PLANE0_MOD_LO, 0),
            (egl::PLANE0_MOD_HI, 0),
            (egl::NONE, 0),
        ];
        assert!(DmaBufDescriptor::decode(&list).is_none());
        // Half a modifier (lo without hi).
        let half = vec![
            (egl::WIDTH, 4),
            (egl::HEIGHT, 4),
            (egl::FOURCC, u64::from(FourCC::ARGB8888.code())),
            (egl::PLANE0_FD, 4),
            (egl::PLANE0_OFFSET, 0),
            (egl::PLANE0_PITCH, 16),
            (egl::PLANE0_MOD_LO, 0),
            (egl::NONE, 0),
        ];
        assert!(DmaBufDescriptor::decode(&half).is_none());
    }

    #[test]
    fn attrib_names_match_the_extension_spec() {
        // The eglext.h values the import path is pinned against.
        assert_eq!(egl::NONE, 0x3058);
        assert_eq!(egl::WIDTH, 0x3057);
        assert_eq!(egl::HEIGHT, 0x3056);
        assert_eq!(egl::FOURCC, 0x3271);
        assert_eq!(egl::PLANE0_FD, 0x3272);
        assert_eq!(egl::PLANE3_PITCH, 0x3442);
        assert_eq!(egl::PLANE0_MOD_LO, 0x3443);
        assert_eq!(egl::PLANE3_MOD_HI, 0x344A);
    }
}
