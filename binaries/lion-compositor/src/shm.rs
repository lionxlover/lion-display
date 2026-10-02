//! Shared-memory pools and buffers: the server side of `ldp.core.shm`.
//!
//! [`ShmPool`] owns one received pool descriptor and its read-only
//! `MAP_SHARED` mapping; [`ShmBuffer`] is a validated window into a
//! pool — the geometry checks the spec's `create_buffer` contract
//! (stride at least `width * bytes-per-pixel`, the whole span inside
//! the pool) run eagerly so the render path never re-validates. The
//! buffer's stable [`identity`](ShmBuffer::identity) doubles as the
//! renderer's texture-cache key and the tree's buffer generation.
//!
//! `shm_pool.resize` grows the mapping by re-mapping the same
//! descriptor (MAP_SHARED content is the file, so a fresh mapping is
//! the same bytes); buffers validate against the pool's *current* size
//! when they build views, matching the spec's "the server re-maps".

#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicU64, Ordering};

use ldp_core::buffer::{BufferGeometry, FourCC, Modifier, PlaneLayout};
use ldp_renderer::{BufferView, RendererError};

use crate::sys;

/// The formats this compositor advertises on `ldp.core.shm`: exactly
/// the single-plane RGB formats the software render path samples
/// bit-exactly. YUV and multi-plane shm arrive with a later phase.
pub const SHM_FORMATS: [FourCC; 2] = [FourCC::XRGB8888, FourCC::ARGB8888];

/// Bytes per pixel of every advertised shm format.
const BPP: u32 = 4;

/// A pool validation failure.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShmError {
    /// The fourcc is not one of the advertised shm formats.
    UnsupportedFormat(u32),
    /// Width or height is zero (or beyond the geometry ceiling).
    BadExtent,
    /// The stride is smaller than one full row.
    StrideTooSmall {
        /// The offered stride.
        stride: u64,
        /// The minimum for this width.
        needed: u64,
    },
    /// The buffer's span does not fit inside the pool.
    SpanOutsidePool {
        /// The span's last byte + 1.
        needed: u64,
        /// The pool size.
        have: u64,
    },
    /// A shrink was requested (`resize` may only grow).
    Shrink,
    /// The underlying syscall failed (mapping).
    Sys(sys::SysError),
}

impl std::fmt::Display for ShmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedFormat(c) => {
                write!(f, "fourcc {c:#010x} is not an advertised shm format")
            }
            Self::BadExtent => f.write_str("zero or oversized width/height"),
            Self::StrideTooSmall { stride, needed } => {
                write!(f, "stride {stride} < width*bpp {needed}")
            }
            Self::SpanOutsidePool { needed, have } => {
                write!(f, "buffer span {needed} exceeds pool size {have}")
            }
            Self::Shrink => f.write_str("shm pools may only grow"),
            Self::Sys(e) => write!(f, "pool mapping failed: {e}"),
        }
    }
}

impl std::error::Error for ShmError {}

impl From<sys::SysError> for ShmError {
    fn from(e: sys::SysError) -> Self {
        Self::Sys(e)
    }
}

/// One `shm_pool` object: the descriptor plus its live mapping.
pub struct ShmPool {
    fd: std::os::fd::OwnedFd,
    size: u64,
    mapping: sys::Mapping,
}

impl ShmPool {
    /// Map a freshly received pool of `size` bytes.
    ///
    /// # Errors
    /// [`ShmError::Sys`] when the mapping fails; zero sizes are
    /// rejected by the syscall layer.
    pub fn new(fd: std::os::fd::OwnedFd, size: u64) -> Result<ShmPool, ShmError> {
        let mapping = sys::map_read_only(&fd, size)?;
        Ok(ShmPool { fd, size, mapping })
    }

    /// The pool's backing descriptor — the import walk's input
    /// (Phase 34: a PRIME/dma-buf-class fd is what GEM imports; a
    /// memfd refuses cleanly on real hardware and the layer composites
    /// on the CPU — the honest degradation, never silent).
    #[must_use]
    pub fn as_fd(&self) -> &std::os::fd::OwnedFd {
        &self.fd
    }

    /// The committed size in bytes.
    #[must_use]
    pub fn size(&self) -> u64 {
        self.size
    }

    /// Grow (never shrink) the pool; the mapping is replaced with a
    /// fresh one of the same descriptor.
    ///
    /// # Errors
    /// [`ShmError::Shrink`] when `size` is not larger; [`ShmError::Sys`]
    /// when the re-map fails (the pool keeps its old mapping then).
    pub fn resize(&mut self, size: u64) -> Result<(), ShmError> {
        if size < self.size {
            return Err(ShmError::Shrink);
        }
        if size == self.size {
            return Ok(());
        }
        // Map first: on failure the old mapping stays intact.
        let mapping = sys::map_read_only(&self.fd, size)?;
        self.mapping = mapping;
        self.size = size;
        Ok(())
    }

    /// The pool's bytes through the live mapping.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        self.mapping.bytes()
    }
}

impl std::fmt::Debug for ShmPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The descriptor and mapping are deliberately omitted: they do
        // not render usefully and never appear in logs.
        f.debug_struct("ShmPool")
            .field("size", &self.size)
            .finish_non_exhaustive()
    }
}

static NEXT_IDENTITY: AtomicU64 = AtomicU64::new(1);

/// One `buffer` object: a validated window into a pool.
///
/// The buffer references its pool by the pool object's wire key —
/// pools are owned (and grown) by the scene's pool map, so a resize
/// is a plain `&mut` operation on the map entry while every buffer's
/// view builds against the pool's *current* mapping at render time.
#[derive(Debug)]
pub struct ShmBuffer {
    pool: ObjectKeyRef,
    offset: u32,
    width: u32,
    height: u32,
    stride: u32,
    format: FourCC,
    identity: u64,
}

/// The pool reference inside a buffer (a debug-friendly key).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ObjectKeyRef {
    /// The owning client's wire id.
    pub client: u32,
    /// The pool object's wire id.
    pub object: u32,
}

impl ObjectKeyRef {
    /// Build from the wire pair.
    #[must_use]
    pub fn new(client: u32, object: u32) -> Self {
        ObjectKeyRef { client, object }
    }
}

impl ShmBuffer {
    /// Validate and create a buffer view into `pool`.
    ///
    /// # Errors
    /// Every [`ShmError`] variant except `Shrink`/`Sys`: the format must
    /// be advertised, extents positive, the stride a full row, and the
    /// whole span inside the pool's current size.
    pub fn new(
        pool: ObjectKeyRef,
        pool_size: u64,
        offset: i32,
        width: i32,
        height: i32,
        stride: i32,
        format: u32,
    ) -> Result<ShmBuffer, ShmError> {
        let fourcc = FourCC::from_code(format);
        if !SHM_FORMATS.contains(&fourcc) {
            return Err(ShmError::UnsupportedFormat(format));
        }
        let (width, height, stride, offset) = (
            u32::try_from(width).ok(),
            u32::try_from(height).ok(),
            u32::try_from(stride).ok(),
            u32::try_from(offset.max(0)).ok(),
        );
        let (Some(width), Some(height), Some(stride), Some(offset)) =
            (width, height, stride, offset)
        else {
            return Err(ShmError::BadExtent);
        };
        if width == 0 || height == 0 {
            return Err(ShmError::BadExtent);
        }
        let needed_row = u64::from(width) * u64::from(BPP);
        if u64::from(stride) < needed_row {
            return Err(ShmError::StrideTooSmall {
                stride: u64::from(stride),
                needed: needed_row,
            });
        }
        let span = span_bytes(offset, stride, width, height);
        if span > pool_size {
            return Err(ShmError::SpanOutsidePool {
                needed: span,
                have: pool_size,
            });
        }
        Ok(ShmBuffer {
            pool,
            offset,
            width,
            height,
            stride,
            format: fourcc,
            identity: NEXT_IDENTITY.fetch_add(1, Ordering::Relaxed),
        })
    }

    /// The pool this buffer windows into.
    #[must_use]
    pub fn pool(&self) -> ObjectKeyRef {
        self.pool
    }

    /// The stable buffer identity (renderer cache key, tree generation).
    #[must_use]
    pub fn identity(&self) -> u64 {
        self.identity
    }

    /// Width in pixels.
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height in pixels.
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The fourcc.
    #[must_use]
    pub fn format(&self) -> FourCC {
        self.format
    }

    /// The validated geometry against the pool's current size.
    ///
    /// # Panics
    ///
    /// Never: the geometry was validated at creation and pools only
    /// grow, so the checked constructor's failure is unreachable.
    #[must_use]
    pub fn geometry(&self, pool_size: u64) -> BufferGeometry {
        // Construction-time checks guarantee this cannot fail: extents
        // were bounded, the plane layout matches, and the pool only
        // grows. The expect mirrors every other checked-then-built
        // invariant in the stack.
        BufferGeometry::new(
            self.width,
            self.height,
            self.format,
            Modifier::LINEAR,
            &[PlaneLayout {
                offset: self.offset,
                stride: self.stride,
            }],
            pool_size,
        )
        .expect("shm geometry was validated at creation and pools only grow")
    }

    /// A render-path view into the pool's live mapping.
    ///
    /// # Errors
    /// [`RendererError::BufferDataTooSmall`] if the pool somehow shrank
    /// under the buffer (impossible by construction; kept honest).
    pub fn view<'a>(&self, pool: &'a ShmPool) -> Result<BufferView<'a>, RendererError> {
        BufferView::new(self.identity, pool.bytes(), self.geometry(pool.size()))
    }
}

/// The exclusive end of the buffer's byte span.
fn span_bytes(offset: u32, stride: u32, width: u32, height: u32) -> u64 {
    u64::from(offset)
        + u64::from(stride) * u64::from(height.saturating_sub(1))
        + u64::from(width) * u64::from(BPP)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sys;
    use std::io::Write;

    fn make_pool(len: u64, fill: u8) -> ShmPool {
        let mem = sys::memfd("shm-test").unwrap();
        let mut sink = std::fs::File::from(mem);
        sink.write_all(&vec![fill; len as usize]).unwrap();
        ShmPool::new(sink.into(), len).unwrap()
    }

    #[test]
    fn valid_buffer_round_trips() {
        let pool = make_pool(4096, 0xAB);
        let key = ObjectKeyRef::new(1, 9);
        let buf = ShmBuffer::new(key, pool.size(), 0, 4, 4, 16, 0x3432_5258).unwrap();
        assert_eq!(buf.width(), 4);
        assert_eq!(buf.identity(), buf.identity());
        let view = buf.view(&pool).unwrap();
        assert_eq!(&view.data()[..16], &[0xAB; 16]);
    }

    #[test]
    fn rejects_small_stride_and_outside_span() {
        let pool = make_pool(64, 0);
        let key = ObjectKeyRef::new(1, 9);
        assert_eq!(
            ShmBuffer::new(key, pool.size(), 0, 4, 4, 15, 0x3432_5258).unwrap_err(),
            ShmError::StrideTooSmall {
                stride: 15,
                needed: 16
            }
        );
        assert_eq!(
            ShmBuffer::new(key, pool.size(), 0, 16, 16, 64, 0x3432_5241).unwrap_err(),
            ShmError::SpanOutsidePool {
                needed: 1024,
                have: 64
            }
        );
    }

    #[test]
    fn rejects_unadvertised_format_and_bad_extent() {
        let pool = make_pool(4096, 0);
        let key = ObjectKeyRef::new(1, 9);
        assert_eq!(
            ShmBuffer::new(key, pool.size(), 0, 1, 1, 4, 0x2036_3135).unwrap_err(),
            ShmError::UnsupportedFormat(0x2036_3135)
        );
        assert_eq!(
            ShmBuffer::new(key, pool.size(), 0, 0, 1, 4, 0x3432_5258).unwrap_err(),
            ShmError::BadExtent
        );
    }

    #[test]
    fn resize_grows_and_keeps_content() {
        let fd = sys::memfd("shm-grow").unwrap();
        let mut file = std::fs::File::from(fd);
        file.write_all(&[1u8; 256]).unwrap();
        let mut pool = ShmPool::new(file.into(), 256).unwrap();
        assert!(pool.resize(128).is_err(), "shrink rejected");
        pool.resize(1024).unwrap();
        assert_eq!(pool.size(), 1024);
        assert_eq!(&pool.bytes()[..256], &[1u8; 256]);
        // A buffer spanning the grown region now fits.
        let key = ObjectKeyRef::new(1, 9);
        assert!(ShmBuffer::new(key, pool.size(), 0, 64, 4, 256, 0x3432_5258).is_ok());
    }
}
