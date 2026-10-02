//! YCbCr decode: coefficients from primaries, per-format sample fetch.
//!
//! The conversion matrix is *derived* from the surface's color primaries
//! (via [`ldp_core::color::Primaries::chromaticities`]) rather than
//! hard-coded per standard, so BT.709 / DCI-P3 / BT.2020 sources all decode
//! through one principled path. The math is pure IEEE f32 (mul/add/div/sqrt
//! only — no libm transcendentals), so results are bit-stable across
//! platforms. All arithmetic happens on *encoded-domain* components: range
//! expansion first, then the matrix, matching how the formats are defined.

use ldp_core::buffer::{BufferGeometry, FourCC};
use ldp_core::color::{ColorRange, Primaries};

/// Kr/Kb luminance coefficients of a primaries set.
///
/// `Kr + Kg + Kb = 1` by construction (the white-point solve). BT.709
/// yields the familiar `(0.2126, 0.0722)`; BT.2020 `(0.2627, 0.0593)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct YuvCoefficients {
    /// Red luminance weight (Kr).
    pub kr: f32,
    /// Blue luminance weight (Kb).
    pub kb: f32,
}

/// Solve the luminance weights from a primaries' chromaticities.
///
/// Each primary's XYZ direction is `(x/y, 1, z/y)`; scaling the three so
/// they sum to the white point makes the Y row of the solve the luminance
/// weights (the weights sum to 1 for any real white).
#[must_use]
pub fn ycbcr_coefficients(p: Primaries) -> YuvCoefficients {
    let c = p.chromaticities();
    let dirs = [
        xyz_dir(c[0], c[1]),
        xyz_dir(c[2], c[3]),
        xyz_dir(c[4], c[5]),
    ];
    let white = xyz_dir(c[6], c[7]);
    // Solve D · s = white for s (3×3 via adjugate); the weights are s
    // normalized so Y = 1 at white.
    let s = solve3(dirs, white);
    let total = s[0] + s[1] + s[2];
    YuvCoefficients {
        kr: s[0] / total,
        kb: s[2] / total,
    }
}

/// XYZ direction vector of a chromaticity `(x, y)` normalized to Y = 1.
fn xyz_dir(x: f32, y: f32) -> [f32; 3] {
    [x / y, 1.0, (1.0 - x - y) / y]
}

/// Solve a 3×3 system `columns · s = v` (columns of the basis) via scalar
/// triple products (Cramer) — correct by construction.
fn solve3(columns: [[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    let [c0, c1, c2] = columns;
    let det = dot(c0, cross(c1, c2));
    let inv_det = 1.0 / det;
    [
        dot(v, cross(c1, c2)) * inv_det,
        dot(c0, cross(v, c2)) * inv_det,
        dot(c0, cross(c1, v)) * inv_det,
    ]
}

/// Dot product.
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Cross product.
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

impl YuvCoefficients {
    /// Decode a range-expanded `(y, cb, cr)` sample into straight RGB in the
    /// `0..=1` encoded domain (clamped).
    #[must_use]
    pub fn to_rgb(self, y: f32, cb: f32, cr: f32) -> [f32; 3] {
        let r = y + 2.0 * (1.0 - self.kr) * cr;
        let b = y + 2.0 * (1.0 - self.kb) * cb;
        let kg = 1.0 - self.kr - self.kb;
        let g = (y - self.kr * r - self.kb * b) / kg;
        [r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0)]
    }
}

/// Range-expand a luma sample (encoded `0..=1`) per the color range.
#[must_use]
pub fn expand_luma(v: f32, range: ColorRange) -> f32 {
    match range {
        ColorRange::Studio => (v * 255.0 - 16.0) / 219.0,
        // Full range (and future variants) is the identity.
        _ => v,
    }
}

/// Range-expand a chroma sample to `-0.5..=0.5`-equivalent units.
///
/// The full-range pivot is the *integer* 128 (not 255/2): decoded neutral
/// chroma is exactly zero, which keeps gray/black/white anchors byte-exact.
#[must_use]
pub fn expand_chroma(v: f32, range: ColorRange) -> f32 {
    match range {
        ColorRange::Studio => (v * 255.0 - 128.0) / 224.0,
        // Full range (and future variants) pivots on the integer 128.
        _ => (v * 255.0 - 128.0) / 255.0,
    }
}

/// Fetch + range-expand one `(y, cb, cr)` sample of a YUV-family buffer at
/// buffer pixel `(bx, by)`.
///
/// The caller has validated `bx < width`, `by < height`; plane block
/// addressing is in-bounds by [`crate::view::BufferView`] construction.
// Sample plumbing carries luma/chroma triples; the short y/cb/cr names
// are the format standards' own.
#[allow(clippy::similar_names, clippy::many_single_char_names)]
pub(crate) fn fetch_sample(
    data: &[u8],
    geometry: &BufferGeometry,
    bx: u32,
    by: u32,
    range: ColorRange,
) -> (f32, f32, f32) {
    let planes = geometry.planes();
    match geometry.format() {
        FourCC::YUYV => {
            // Packed: [Y0 U Y1 V] per two pixels; chroma shared per pair.
            let layout = planes[0];
            let block = bx / 2;
            let base =
                layout.offset as usize + layout.stride as usize * by as usize + block as usize * 4;
            let y = u16::from(data[base + (bx as usize & 1) * 2]) as f32 / 255.0;
            let cb = data[base + 1] as f32 / 255.0;
            let cr = data[base + 3] as f32 / 255.0;
            (
                expand_luma(y, range),
                expand_chroma(cb, range),
                expand_chroma(cr, range),
            )
        }
        FourCC::NV12 => {
            let luma_layout = planes[0];
            let chroma_layout = planes[1];
            let y_addr = luma_layout.offset as usize
                + luma_layout.stride as usize * by as usize
                + bx as usize;
            let (cx, cy) = (bx / 2, by / 2);
            let c_addr = chroma_layout.offset as usize
                + chroma_layout.stride as usize * cy as usize
                + cx as usize * 2;
            let y = data[y_addr] as f32 / 255.0;
            let cb = data[c_addr] as f32 / 255.0;
            let cr = data[c_addr + 1] as f32 / 255.0;
            (
                expand_luma(y, range),
                expand_chroma(cb, range),
                expand_chroma(cr, range),
            )
        }
        FourCC::P010 => {
            // 10-bit samples in the high bits of u16 (value = raw >> 6).
            let luma_layout = planes[0];
            let chroma_layout = planes[1];
            let y_addr = luma_layout.offset as usize
                + luma_layout.stride as usize * by as usize
                + bx as usize * 2;
            let (cx, cy) = (bx / 2, by / 2);
            let c_addr = chroma_layout.offset as usize
                + chroma_layout.stride as usize * cy as usize
                + cx as usize * 4;
            let y10 = read_u16(data, y_addr) >> 6;
            let cb10 = read_u16(data, c_addr) >> 6;
            let cr10 = read_u16(data, c_addr + 2) >> 6;
            match range {
                // P010's studio excursion: Y 64..940, C 512±448.
                ColorRange::Studio => (
                    (f32::from(y10) - 64.0) / 876.0,
                    (f32::from(cb10) - 512.0) / 896.0,
                    (f32::from(cr10) - 512.0) / 896.0,
                ),
                // Full range (and future variants): integer pivot 512.
                _ => (
                    f32::from(y10) / 1023.0,
                    (f32::from(cb10) - 512.0) / 1023.0,
                    (f32::from(cr10) - 512.0) / 1023.0,
                ),
            }
        }
        FourCC::YUV420 => {
            let (y_layout, u_layout, v_layout) = (planes[0], planes[1], planes[2]);
            let (cx, cy) = (bx / 2, by / 2);
            let y_addr =
                y_layout.offset as usize + y_layout.stride as usize * by as usize + bx as usize;
            let u_addr =
                u_layout.offset as usize + u_layout.stride as usize * cy as usize + cx as usize;
            let v_addr =
                v_layout.offset as usize + v_layout.stride as usize * cy as usize + cx as usize;
            let y = data[y_addr] as f32 / 255.0;
            let cb = data[u_addr] as f32 / 255.0;
            let cr = data[v_addr] as f32 / 255.0;
            (
                expand_luma(y, range),
                expand_chroma(cb, range),
                expand_chroma(cr, range),
            )
        }
        _ => (0.0, 0.0, 0.0), // Not a YUV format; unreachable via dispatch.
    }
}

/// Little-endian u16 at a validated address.
pub(crate) fn read_u16(data: &[u8], addr: usize) -> u16 {
    u16::from_le_bytes([data[addr], data[addr + 1]])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn coefficients_match_the_standards() {
        let bt709 = ycbcr_coefficients(Primaries::Bt709);
        assert!(close(bt709.kr, 0.2126, 2e-4), "kr {}", bt709.kr);
        assert!(close(bt709.kb, 0.0722, 2e-4), "kb {}", bt709.kb);
        let bt2020 = ycbcr_coefficients(Primaries::Bt2020);
        assert!(close(bt2020.kr, 0.2627, 2e-4));
        assert!(close(bt2020.kb, 0.0593, 2e-4));
        // Weights sum to 1 for every primaries set.
        for p in [Primaries::Bt709, Primaries::DciP3, Primaries::Bt2020] {
            let c = ycbcr_coefficients(p);
            assert!(close(c.kr + c.kb + (1.0 - c.kr - c.kb), 1.0, 1e-6));
        }
    }

    #[test]
    fn gray_decodes_to_gray() {
        let c = ycbcr_coefficients(Primaries::Bt709);
        // Neutral chroma (0 after expansion) leaves Y untouched.
        let rgb = c.to_rgb(0.5, 0.0, 0.0);
        for v in rgb {
            assert!(close(v, 0.5, 1e-6));
        }
    }

    #[test]
    fn red_anchors_through_the_matrix() {
        // BT.709 pure red: Y = Kr; Cb = -Kr/(2(1-Kb)) (B - Y with B=0);
        // Cr = (1-Kr)/(2(1-Kr)) = 0.5.
        let c = ycbcr_coefficients(Primaries::Bt709);
        let y = c.kr;
        let cb = -c.kr / (2.0 * (1.0 - c.kb));
        let cr = 0.5;
        let rgb = c.to_rgb(y, cb, cr);
        assert!(close(rgb[0], 1.0, 1e-5), "r {}", rgb[0]);
        assert!(close(rgb[1], 0.0, 1e-5), "g {}", rgb[1]);
        assert!(close(rgb[2], 0.0, 1e-5), "b {}", rgb[2]);
    }

    #[test]
    fn range_expansion_bounds() {
        assert_eq!(expand_luma(16.0 / 255.0, ColorRange::Studio), 0.0);
        assert_eq!(expand_luma(235.0 / 255.0, ColorRange::Studio), 1.0);
        assert_eq!(expand_chroma(128.0 / 255.0, ColorRange::Studio), 0.0);
        assert_eq!(expand_chroma(240.0 / 255.0, ColorRange::Studio), 0.5);
        // Full range: integer pivot 128 and 1/255 scale.
        assert_eq!(expand_luma(0.0, ColorRange::Full), 0.0);
        assert_eq!(expand_chroma(128.0 / 255.0, ColorRange::Full), 0.0);
        assert!((expand_chroma(255.0 / 255.0, ColorRange::Full) - 127.0 / 255.0).abs() < 1e-6);
    }
}
