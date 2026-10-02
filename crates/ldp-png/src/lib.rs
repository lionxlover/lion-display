//! A zero-dependency, deterministic PNG encoder.
//!
//! Scope: 8-bit RGB and RGBA input (the exact shapes LDP frame dumps and
//! screen captures produce), adaptive per-row filtering, one fixed-Huffman
//! deflate stream, single IDAT. Output is **bit-identical across platforms
//! and runs** for identical input — the determinism doctrine of the
//! renderer and scheduler applies to on-disk artifacts too, so a dumped
//! frame is diffable in CI.
//!
//! Layout: [`PngEncoder`] is the public surface; `deflate.rs` is the
//! compressor; `filter.rs` is the adaptive row filter. Everything is pure
//! integer code over caller buffers: `#![forbid(unsafe_code)]`, no
//! dependencies, no clock, no threads, allocation only at output and at
//! first use (all scratch is retained across encodes).

#![forbid(unsafe_code)]

mod deflate;
mod filter;

/// The PNG signature (RFC 2083 §`magic`).
const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// Errors of the encoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PngError {
    /// Width or height is zero, or exceeds the format's 2^31-1 limit.
    InvalidDimensions,
    /// The input slice's length is not `width * height * bytes_per_pixel`.
    DataLengthMismatch {
        /// The expected length.
        expected: usize,
        /// The length received.
        got: usize,
    },
}

impl core::fmt::Display for PngError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            PngError::InvalidDimensions => {
                write!(f, "png: invalid dimensions (must be 1..=2^31-1)")
            }
            PngError::DataLengthMismatch { expected, got } => {
                write!(f, "png: data length {got} != expected {expected}")
            }
        }
    }
}

impl std::error::Error for PngError {}

/// CRC-32 (IEEE 802.3, the PNG chunk polynomial), with the table built
/// once per process.
fn crc32(bytes: &[u8]) -> u32 {
    use std::sync::OnceLock;
    static TABLE: OnceLock<[u32; 256]> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (n, entry) in t.iter_mut().enumerate() {
            let mut c = n as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *entry = c;
        }
        t
    });
    let mut crc = 0xFFFF_FFFFu32;
    for &b in bytes {
        // Index cast: the table is indexed by a byte-informed value.
        crc = table[((crc ^ u32::from(b)) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

/// Adler-32 (RFC 1950 §8.2) of `data`.
fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65_521;
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    // Process in blocks so `a` cannot overflow before the modulo: with
    // 5552 bytes per block, 255 * 5552 < 2^32 (the classic zlib bound).
    for block in data.chunks(5552) {
        for &byte in block {
            a += u32::from(byte);
            b += a;
        }
        a %= MOD;
        b %= MOD;
    }
    (b << 16) | a
}

/// Append one PNG chunk (`length + type + data + crc`) to `out`.
fn write_chunk(out: &mut Vec<u8>, ctype: [u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(&ctype);
    out.extend_from_slice(data);
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(&ctype);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

/// The color types the encoder emits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorType {
    /// 8-bit truecolor RGB, 3 bytes per pixel.
    Rgb8,
    /// 8-bit truecolor RGBA, 4 bytes per pixel.
    Rgba8,
}

impl ColorType {
    /// The PNG IHDR color-type byte.
    #[must_use]
    pub(crate) fn ihdr_value(self) -> u8 {
        match self {
            ColorType::Rgb8 => 2,
            ColorType::Rgba8 => 6,
        }
    }

    /// Bytes per pixel.
    #[must_use]
    pub(crate) fn bytes_per_pixel(self) -> usize {
        match self {
            ColorType::Rgb8 => 3,
            ColorType::Rgba8 => 4,
        }
    }
}

/// A PNG encoder with retained scratch (the steady state allocates only
/// the output image).
#[derive(Default)]
pub struct PngEncoder {
    matcher: deflate::Matcher,
    /// The filter-stage scratch (filtered rows and trial buffers).
    filter_scratch: filter::FilterScratch,
    /// The deflate input assembly (filtered stream) and its zlib output.
    stream: Vec<u8>,
    zlib_out: Vec<u8>,
}

impl PngEncoder {
    /// A fresh encoder (empty; first encode allocates).
    #[must_use]
    pub fn new() -> PngEncoder {
        PngEncoder::default()
    }

    /// Encode an 8-bit image into a complete PNG file.
    ///
    /// `data` is `width * height * bytes_per_pixel` bytes, row-major,
    /// top-to-bottom. The output is self-contained and deterministic.
    ///
    /// # Errors
    ///
    /// [`PngError::InvalidDimensions`] or [`PngError::DataLengthMismatch`].
    pub fn encode(
        &mut self,
        width: u32,
        height: u32,
        color: ColorType,
        data: &[u8],
    ) -> Result<Vec<u8>, PngError> {
        if width == 0 || height == 0 || width > i32::MAX as u32 || height > i32::MAX as u32 {
            return Err(PngError::InvalidDimensions);
        }
        let expected = width as usize * height as usize * color.bytes_per_pixel();
        if data.len() != expected {
            return Err(PngError::DataLengthMismatch {
                expected,
                got: data.len(),
            });
        }

        // ---- IHDR ----
        let mut ihdr = Vec::with_capacity(13);
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.push(8); // bit depth
        ihdr.push(color.ihdr_value());
        ihdr.push(0); // compression: deflate
        ihdr.push(0); // filter: adaptive per row
        ihdr.push(0); // interlace: none

        // ---- the filtered scanline stream ----
        self.stream.clear();
        filter::filter_image(
            &mut self.filter_scratch,
            &mut self.stream,
            width as usize,
            height as usize,
            color.bytes_per_pixel(),
            data,
        );

        // ---- the zlib stream: header + deflate + adler ----
        let deflated = deflate::deflate_fixed(&mut self.matcher, &self.stream);
        self.zlib_out.clear();
        self.zlib_out.reserve(deflated.len() + 6);
        self.zlib_out.push(0x78); // CMF: deflate, 32 KiB window
        self.zlib_out.push(0x01); // FLG: check bits valid, no dict, fastest
        self.zlib_out.extend_from_slice(&deflated);
        self.zlib_out
            .extend_from_slice(&adler32(&self.stream).to_be_bytes());

        // ---- assemble ----
        let mut out = Vec::with_capacity(SIGNATURE.len() + 12 * 3 + 13 + self.zlib_out.len());
        out.extend_from_slice(&SIGNATURE);
        write_chunk(&mut out, *b"IHDR", &ihdr);
        write_chunk(&mut out, *b"IDAT", &self.zlib_out);
        write_chunk(&mut out, *b"IEND", &[]);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_known_vectors() {
        // The classic check value (IEEE 802.3 / PNG).
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
        assert_eq!(crc32(b"a"), 0xE8B7_BE43);
        assert_eq!(crc32(b"abc"), 0x3524_41C2);
        assert_eq!(crc32(b"1234567890"), 0x261D_AEE5);
    }

    #[test]
    fn adler32_known_vectors() {
        assert_eq!(adler32(b""), 1);
        assert_eq!(adler32(b"a"), 0x0062_0062);
        assert_eq!(adler32(b"abc"), 0x024D_0127);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        // The block bound: a large uniform input still lands exactly on
        // the per-byte-mod result (verified against an independent
        // implementation; a = 12332, b = 5262).
        let big = vec![0xFFu8; 100_000];
        assert_eq!(adler32(&big), 0x149A_302C);
    }

    #[test]
    fn chunk_writing_is_self_describing() {
        let mut out = Vec::new();
        write_chunk(&mut out, *b"ABCD", &[1, 2, 3]);
        assert_eq!(&out[..4], &[0, 0, 0, 3]);
        assert_eq!(&out[4..8], b"ABCD");
        assert_eq!(&out[8..11], &[1, 2, 3]);
        // CRC over type+data, big-endian.
        let mut crc_in = Vec::new();
        crc_in.extend_from_slice(b"ABCD");
        crc_in.extend_from_slice(&[1, 2, 3]);
        assert_eq!(&out[11..15], &crc32(&crc_in).to_be_bytes());
        assert_eq!(out.len(), 15);
    }

    #[test]
    fn rejects_invalid_dimensions_and_lengths() {
        let mut enc = PngEncoder::new();
        assert_eq!(
            enc.encode(0, 4, ColorType::Rgb8, &[0; 12]),
            Err(PngError::InvalidDimensions)
        );
        assert_eq!(
            enc.encode(2, 0, ColorType::Rgb8, &[0; 12]),
            Err(PngError::InvalidDimensions)
        );
        assert_eq!(
            enc.encode(2, 2, ColorType::Rgb8, &[0; 11]),
            Err(PngError::DataLengthMismatch {
                expected: 12,
                got: 11
            })
        );
        // u32::MAX dims fail before any multiplication overflow.
        assert_eq!(
            enc.encode(u32::MAX, u32::MAX, ColorType::Rgba8, &[]),
            Err(PngError::InvalidDimensions)
        );
    }

    #[test]
    fn output_is_structurally_valid_png() {
        let mut enc = PngEncoder::new();
        // 3x2 RGBA with distinct pixels.
        let data: Vec<u8> = (0..24).map(|i| (i * 11) as u8).collect();
        let png = enc.encode(3, 2, ColorType::Rgba8, &data).unwrap();
        assert_eq!(&png[..8], &SIGNATURE);
        // Walk chunks: 12 (len+type) + body + 4 crc each.
        let mut pos = 8;
        let mut seen = Vec::new();
        while pos < png.len() {
            assert!(png.len() - pos >= 12, "truncated chunk header");
            let len =
                u32::from_be_bytes([png[pos], png[pos + 1], png[pos + 2], png[pos + 3]]) as usize;
            let ctype = &png[pos + 4..pos + 8];
            let body = &png[pos + 8..pos + 8 + len];
            let crc = u32::from_be_bytes([
                png[pos + 8 + len],
                png[pos + 9 + len],
                png[pos + 10 + len],
                png[pos + 11 + len],
            ]);
            let mut crc_in = Vec::new();
            crc_in.extend_from_slice(ctype);
            crc_in.extend_from_slice(body);
            assert_eq!(
                crc,
                crc32(&crc_in),
                "chunk {} crc",
                String::from_utf8_lossy(ctype)
            );
            seen.push(([ctype[0], ctype[1], ctype[2], ctype[3]], len));
            pos += 12 + len;
        }
        assert_eq!(pos, png.len());
        assert_eq!(seen[0], (*b"IHDR", 13));
        assert_eq!(seen.last().copied(), Some((*b"IEND", 0)));
        // IHDR fields.
        let ihdr = &png[16..29];
        assert_eq!(&ihdr[0..4], &3u32.to_be_bytes());
        assert_eq!(&ihdr[4..8], &2u32.to_be_bytes());
        assert_eq!(ihdr[8], 8);
        assert_eq!(ihdr[9], 6); // RGBA
        assert_eq!(&ihdr[10..13], &[0, 0, 0]);
    }

    #[test]
    fn encoding_is_deterministic_and_scratch_reuse_is_stable() {
        let mut a = PngEncoder::new();
        let data: Vec<u8> = (0..450).map(|i| (i % 7 * 30) as u8).collect();
        let first = a.encode(10, 15, ColorType::Rgb8, &data).unwrap();
        let second = a.encode(10, 15, ColorType::Rgb8, &data).unwrap();
        assert_eq!(first, second);
        // A different image in between must not poison the scratch.
        let other: Vec<u8> = vec![9u8; 450];
        let _ = a.encode(10, 15, ColorType::Rgb8, &other).unwrap();
        let third = a.encode(10, 15, ColorType::Rgb8, &data).unwrap();
        assert_eq!(first, third);
        // And a second encoder agrees bit-for-bit.
        let mut b = PngEncoder::new();
        assert_eq!(b.encode(10, 15, ColorType::Rgb8, &data).unwrap(), first);
    }

    #[test]
    fn solid_and_gradient_inputs_compress_below_raw() {
        let mut enc = PngEncoder::new();
        let solid = vec![0x30u8; 64 * 64 * 3];
        let png = enc.encode(64, 64, ColorType::Rgb8, &solid).unwrap();
        assert!(
            png.len() < solid.len(),
            "solid 64x64 must compress below raw ({})",
            png.len()
        );
        // A vertical gradient compresses too (rows repeat after filtering).
        let mut grad = Vec::with_capacity(64 * 64 * 3);
        for y in 0..64u16 {
            for _ in 0..64 {
                grad.extend_from_slice(&[0, (y * 4) as u8, 0xFF]);
            }
        }
        let png = enc.encode(64, 64, ColorType::Rgb8, &grad).unwrap();
        assert!(png.len() < grad.len(), "gradient must compress below raw");
    }
}
