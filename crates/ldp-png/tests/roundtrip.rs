//! End-to-end round-trip conformance: a complete PNG *decoder* lives here,
//! written against the RFC and independent of the encoder's code paths
//! (its inflate is the one in `deflate.rs`'s test module — itself a
//! separate implementation from the compressor). Encoding an image and
//! decoding it back must return the exact input bytes, and the file's
//! structure must parse cleanly chunk by chunk.

use ldp_png::{ColorType, PngEncoder, PngError};

/// The signature.
const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// One parsed chunk.
struct Chunk<'a> {
    ctype: [u8; 4],
    data: &'a [u8],
}

/// Split a PNG into its chunks, verifying the signature, lengths, CRCs,
/// and the trailing IEND.
fn parse_chunks(png: &[u8]) -> Vec<Chunk<'_>> {
    assert_eq!(&png[..8], &SIGNATURE, "signature");
    let mut out = Vec::new();
    let mut pos = 8;
    while pos < png.len() {
        assert!(png.len() - pos >= 12, "truncated chunk at {pos}");
        let len = u32::from_be_bytes([png[pos], png[pos + 1], png[pos + 2], png[pos + 3]]) as usize;
        let mut ctype = [0u8; 4];
        ctype.copy_from_slice(&png[pos + 4..pos + 8]);
        let end = pos + 8 + len;
        assert!(end + 4 <= png.len(), "chunk body overruns the file");
        let crc = u32::from_be_bytes([png[end], png[end + 1], png[end + 2], png[end + 3]]);
        let mut crc_input = Vec::with_capacity(4 + len);
        crc_input.extend_from_slice(&ctype);
        crc_input.extend_from_slice(&png[pos + 8..end]);
        assert_eq!(
            crc,
            crc_ieee(&crc_input),
            "chunk {} CRC mismatch",
            String::from_utf8_lossy(&ctype)
        );
        out.push(Chunk {
            ctype,
            data: &png[pos + 8..end],
        });
        pos = end + 4;
        if &ctype == b"IEND" {
            assert_eq!(pos, png.len(), "bytes after IEND");
            return out;
        }
    }
    panic!("missing IEND");
}

/// CRC-32 (IEEE), re-implemented here so the decoder does not share the
/// encoder's table code.
fn crc_ieee(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    crc ^ 0xFFFF_FFFF
}

/// Adler-32, re-implemented here.
fn adler(data: &[u8]) -> u32 {
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for &byte in data {
        a = (a + u32::from(byte)) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

/// Inflate a fixed-Huffman deflate stream (re-implemented here, decoding
/// exactly what the encoder emits: one final fixed block).
/// The LSB-first bit reader (deflate's byte-level packing).
struct Reader<'a> {
    d: &'a [u8],
    p: usize,
    b: u32,
}

impl<'a> Reader<'a> {
    fn new(d: &'a [u8]) -> Self {
        Reader { d, p: 0, b: 0 }
    }
    fn bit(&mut self) -> u32 {
        let byte = self.d[self.p];
        let v = u32::from((byte >> self.b) & 1);
        self.b += 1;
        if self.b == 8 {
            self.b = 0;
            self.p += 1;
        }
        v
    }
    fn bits(&mut self, n: u32) -> u32 {
        let mut v = 0;
        for i in 0..n {
            v |= self.bit() << i;
        }
        v
    }
}

/// Inflate a fixed-Huffman deflate stream (decoding exactly what the
/// encoder emits: one final fixed block).
fn inflate_fixed(data: &[u8]) -> Vec<u8> {
    let mut r = Reader::new(data);
    assert_eq!(r.bit(), 1, "BFINAL");
    assert_eq!(r.bits(2), 1, "BTYPE fixed");
    let mut out = Vec::new();
    'outer: loop {
        // Read the code bit by bit, matching RFC 1951 fixed ranges.
        let mut code = 0u32;
        let mut len = 0u32;
        let sym = loop {
            code = (code << 1) | r.bit();
            len += 1;
            match len {
                7 if code <= 23 => break u16::try_from(256 + code).unwrap(),
                8 if (0x30..=0xBF).contains(&code) => break u16::try_from(code - 0x30).unwrap(),
                8 if (0xC0..=0xC7).contains(&code) => {
                    break u16::try_from(280 + code - 0xC0).unwrap()
                }
                9 if (0x190..=0x1FF).contains(&code) => {
                    break u16::try_from(144 + code - 0x190).unwrap()
                }
                9 => panic!("invalid literal code"),
                _ => {}
            }
        };
        match sym {
            256 => break 'outer,
            0..=255 => out.push(sym as u8),
            s => {
                let (mlen, dist) = read_match(&mut r, s);
                for _ in 0..mlen {
                    let byte = out[out.len() - dist];
                    out.push(byte);
                }
            }
        }
    }
    // Remaining bits of the current byte must be zero padding.
    if r.b != 0 {
        let rest = r.d[r.p] >> r.b;
        assert_eq!(rest, 0, "nonzero padding");
        r.p += 1;
    }
    assert_eq!(r.p, r.d.len(), "trailing deflate bytes");
    out
}

/// Decode one length/distance pair after symbol `s` (257..=285),
/// returning `(match_length, distance)`.
fn read_match(r: &mut Reader<'_>, s: u16) -> (usize, usize) {
    const LENS: [(u16, u16, u32); 29] = [
        (257, 3, 0),
        (258, 4, 0),
        (259, 5, 0),
        (260, 6, 0),
        (261, 7, 0),
        (262, 8, 0),
        (263, 9, 0),
        (264, 10, 0),
        (265, 11, 1),
        (266, 13, 1),
        (267, 15, 1),
        (268, 17, 1),
        (269, 19, 2),
        (270, 23, 2),
        (271, 27, 2),
        (272, 31, 2),
        (273, 35, 3),
        (274, 43, 3),
        (275, 51, 3),
        (276, 59, 3),
        (277, 67, 4),
        (278, 83, 4),
        (279, 99, 4),
        (280, 115, 4),
        (281, 131, 5),
        (282, 163, 5),
        (283, 195, 5),
        (284, 227, 5),
        (285, 258, 0),
    ];
    const DISTS: [(u32, u32); 30] = [
        (1, 0),
        (2, 0),
        (3, 0),
        (4, 0),
        (5, 1),
        (7, 1),
        (9, 2),
        (13, 2),
        (17, 3),
        (25, 3),
        (33, 4),
        (49, 4),
        (65, 5),
        (97, 5),
        (129, 6),
        (193, 6),
        (257, 7),
        (385, 7),
        (513, 8),
        (769, 8),
        (1025, 9),
        (1537, 9),
        (2049, 10),
        (3073, 10),
        (4097, 11),
        (6145, 11),
        (8193, 12),
        (12_289, 12),
        (16_385, 13),
        (24_577, 13),
    ];
    let row = usize::from(s) - 257;
    assert!(row < LENS.len(), "length symbol {s} out of range");
    let (_, base, extra) = LENS[row];
    let mlen = usize::from(base) + r.bits(extra) as usize;
    // Distance codes are Huffman codes: read MSB-first.
    let mut dsym = 0usize;
    for _ in 0..5 {
        dsym = (dsym << 1) | r.bit() as usize;
    }
    assert!(dsym < DISTS.len(), "distance symbol {dsym} out of range");
    let (dbase, dextra) = DISTS[dsym];
    let dist = dbase as usize + r.bits(dextra) as usize;
    (mlen, dist)
}

/// Undo one filter over a row (the decoder side of `filter.rs`).
fn unfilter_row(filter: u8, row: &mut [u8], prev: &[u8], bpp: usize) {
    for x in 0..row.len() {
        let left = if x >= bpp { row[x - bpp] } else { 0 };
        let up = prev[x];
        let upleft = if x >= bpp { prev[x - bpp] } else { 0 };
        // Paeth (decoder side re-derives the same predictor).
        let paeth = {
            let p = i32::from(left) + i32::from(up) - i32::from(upleft);
            let pa = (p - i32::from(left)).abs();
            let pb = (p - i32::from(up)).abs();
            let pc = (p - i32::from(upleft)).abs();
            if pa <= pb && pa <= pc {
                left
            } else if pb <= pc {
                up
            } else {
                upleft
            }
        };
        let v = match filter {
            1 => row[x].wrapping_add(left),
            2 => row[x].wrapping_add(up),
            3 => row[x].wrapping_add(((u16::from(left) + u16::from(up)) >> 1) as u8),
            4 => row[x].wrapping_add(paeth),
            _ => row[x],
        };
        row[x] = v;
    }
}

/// A fully decoded image: dimensions, color, pixel bytes.
struct Decoded {
    width: u32,
    height: u32,
    color: u8,
    data: Vec<u8>,
}

/// Decode a PNG produced by this encoder (8-bit RGB/RGBA, single IDAT).
fn decode(png: &[u8]) -> Decoded {
    let chunks = parse_chunks(png);
    assert_eq!(chunks[0].ctype, [b'I', b'H', b'D', b'R']);
    let ihdr = chunks[0].data;
    assert_eq!(ihdr.len(), 13);
    let width = u32::from_be_bytes([ihdr[0], ihdr[1], ihdr[2], ihdr[3]]);
    let height = u32::from_be_bytes([ihdr[4], ihdr[5], ihdr[6], ihdr[7]]);
    assert_eq!(ihdr[8], 8, "bit depth");
    assert_eq!(ihdr[10], 0, "compression method");
    assert_eq!(ihdr[11], 0, "filter method");
    assert_eq!(ihdr[12], 0, "no interlace");
    let color = ihdr[9];
    assert!(color == 2 || color == 6, "color type {color}");

    // The zlib stream: one IDAT.
    let idats: Vec<&Chunk<'_>> = chunks.iter().filter(|c| &c.ctype == b"IDAT").collect();
    assert_eq!(idats.len(), 1, "the encoder emits one IDAT");
    let z = idats[0].data;
    assert!(z.len() >= 6);
    assert_eq!(z[0] & 0x0F, 8, "deflate method");
    assert_eq!(z[0] >> 4, 7, "32 KiB window");
    // FLG check: (CMF*256 + FLG) % 31 == 0, FDICT clear.
    assert_eq!((u16::from(z[0]) * 256 + u16::from(z[1])) % 31, 0);
    assert_eq!(z[1] & 0x20, 0, "no preset dict");
    let deflated = &z[2..z.len() - 4];
    let stream = inflate_fixed(deflated);
    let checksum = u32::from_be_bytes([
        z[z.len() - 4],
        z[z.len() - 3],
        z[z.len() - 2],
        z[z.len() - 1],
    ]);
    assert_eq!(checksum, adler(&stream), "zlib Adler-32");

    // Unfilter.
    let bpp = if color == 2 { 3 } else { 4 };
    let stride = width as usize * bpp;
    assert_eq!(stream.len(), height as usize * (1 + stride));
    let mut data = Vec::with_capacity(stride * height as usize);
    let mut prev = vec![0u8; stride];
    let mut pos = 0usize;
    for _ in 0..height {
        let f = stream[pos];
        pos += 1;
        let mut row = stream[pos..pos + stride].to_vec();
        pos += stride;
        unfilter_row(f, &mut row, &prev, bpp);
        data.extend_from_slice(&row);
        prev.copy_from_slice(&row);
    }
    Decoded {
        width,
        height,
        color,
        data,
    }
}

/// A deterministic LCG (test-side; independent of the crates' generators).
fn lcg_bytes(seed: u64, n: usize) -> Vec<u8> {
    let mut s = seed;
    (0..n)
        .map(|_| {
            s = s
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (s >> 33) as u8
        })
        .collect()
}

#[test]
fn round_trips_exact_pixels_across_shapes() {
    let mut enc = PngEncoder::new();
    let shapes: [(u32, u32, ColorType); 7] = [
        (1, 1, ColorType::Rgba8),
        (1, 1, ColorType::Rgb8),
        (2, 3, ColorType::Rgb8),
        (7, 5, ColorType::Rgba8),
        (64, 64, ColorType::Rgb8),
        (13, 257, ColorType::Rgba8),
        (300, 40, ColorType::Rgb8),
    ];
    for (i, &(w, h, color)) in shapes.iter().enumerate() {
        let bpp = if color == ColorType::Rgb8 { 3 } else { 4 };
        let data = lcg_bytes(0x1000 + i as u64, w as usize * h as usize * bpp);
        let png = enc
            .encode(w, h, color, &data)
            .unwrap_or_else(|e| panic!("shape {i} ({w}x{h}) encode failed: {e}"));
        let dec = decode(&png);
        assert_eq!(dec.width, w, "shape {i}");
        assert_eq!(dec.height, h, "shape {i}");
        assert_eq!(
            dec.color,
            if color == ColorType::Rgb8 { 2 } else { 6 },
            "shape {i}"
        );
        assert_eq!(dec.data, data, "shape {i}: pixels differ after round-trip");
    }
}

#[test]
fn ui_shaped_content_round_trips_and_compresses() {
    let mut enc = PngEncoder::new();
    // A "window" shape: solid body, a title-bar band, a translucent
    // shadow ramp — the exact content screen capture produces.
    let (w, h) = (320u32, 200u32);
    let mut data = Vec::with_capacity(w as usize * h as usize * 4);
    for y in 0..h {
        for x in 0..w {
            let px = if y < 24 {
                [40u8, 44, 52, 255] // title bar
            } else if x < 4 || x >= w - 4 || y >= h - 4 {
                [20u8, 24, 28, 255] // border
            } else {
                let checker = ((x / 16) + (y / 16)) % 2 == 0;
                if checker {
                    [245u8, 245, 245, 255]
                } else {
                    [235u8, 235, 240, 255]
                }
            };
            data.extend_from_slice(&px);
        }
    }
    let png = enc.encode(w, h, ColorType::Rgba8, &data).unwrap();
    let dec = decode(&png);
    assert_eq!(dec.data, data);
    assert!(
        png.len() < data.len() / 3,
        "UI-shaped content should compress well: {} vs {}",
        png.len(),
        data.len()
    );
}

#[test]
fn error_surface_is_exact() {
    let mut enc = PngEncoder::new();
    assert_eq!(
        enc.encode(4, 4, ColorType::Rgb8, &[0; 47]),
        Err(PngError::DataLengthMismatch {
            expected: 48,
            got: 47
        })
    );
    // The failed call must not corrupt the encoder for the next one.
    let ok = enc.encode(2, 2, ColorType::Rgb8, &[7; 12]).unwrap();
    assert!(!ok.is_empty());
}

#[test]
fn large_image_round_trips() {
    // 1 MiB of pseudo-random RGBA: exercises window wraparound in the
    // matcher and multi-chunk filtering without being slow.
    let (w, h) = (512u32, 512u32);
    let data = lcg_bytes(0xFEED, w as usize * h as usize * 4);
    let mut enc = PngEncoder::new();
    let png = enc.encode(w, h, ColorType::Rgba8, &data).unwrap();
    let dec = decode(&png);
    assert_eq!(dec.data, data);
}
