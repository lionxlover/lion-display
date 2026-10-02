//! The deflate side of the PNG encoder: a fixed-Huffman, hash-chain LZ77
//! compressor and the test-only inflate that proves it round-trips.
//!
//! Scope is deliberately narrow — one zlib stream per image, emitted as a
//! single final fixed-Huffman block (BTYPE=01). Fixed Huffman forgoes the
//! dynamic-table machinery while keeping real match compression; the PNG
//! spec permits any conformant zlib stream. Determinism doctrine: the hash
//! chain is walked by strictly-decreasing positions with a bounded depth,
//! and ties break toward the *earlier* (smaller) distance symbol, so the
//! output is bit-identical across runs and platforms.

/// The LZ77 window (deflate's maximum, 32 KiB) minus one as a mask.
const WINDOW_MASK: usize = 32_768 - 1;
/// The match hash table size (15 bits, 32 KiB entries).
const HASH_BITS: u32 = 15;
const HASH_SIZE: usize = 1 << HASH_BITS;
/// How many hash-chain candidates the matcher visits per position.
const MAX_CHAIN: usize = 48;
/// Deflate's minimum and maximum match lengths.
const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;

/// Length-symbol table: `(symbol, base_length, extra_bits)`.
const LENGTH_TABLE: [(u16, u16, u32); 29] = [
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

/// Distance-symbol table: `(symbol, base_distance, extra_bits)`.
const DIST_TABLE: [(u16, u32, u32); 30] = [
    (0, 1, 0),
    (1, 2, 0),
    (2, 3, 0),
    (3, 4, 0),
    (4, 5, 1),
    (5, 7, 1),
    (6, 9, 2),
    (7, 13, 2),
    (8, 17, 3),
    (9, 25, 3),
    (10, 33, 4),
    (11, 49, 4),
    (12, 65, 5),
    (13, 97, 5),
    (14, 129, 6),
    (15, 193, 6),
    (16, 257, 7),
    (17, 385, 7),
    (18, 513, 8),
    (19, 769, 8),
    (20, 1025, 9),
    (21, 1537, 9),
    (22, 2049, 10),
    (23, 3073, 10),
    (24, 4097, 11),
    (25, 6145, 11),
    (26, 8193, 12),
    (27, 12_289, 12),
    (28, 16_385, 13),
    (29, 24_577, 13),
];

/// Reverse the low `n` bits of `value` (Huffman codes are bit-reversed on
/// the wire: the code's MSB is the stream's next bit).
#[must_use]
pub(crate) fn reverse_bits(value: u32, n: u32) -> u32 {
    let mut out = 0u32;
    let mut v = value;
    for _ in 0..n {
        out = (out << 1) | (v & 1);
        v >>= 1;
    }
    out
}

/// The deflate output accumulator: bytes materialize once 8 bits are
/// buffered; Huffman codes are pre-reversed so the accumulator's low bit
/// is always the stream's next bit.
#[derive(Default)]
pub(crate) struct BitWriter {
    out: Vec<u8>,
    acc: u64,
    nbits: u32,
}

impl BitWriter {
    /// Append `count` bits of `bits` (LSB = next stream bit). The caller
    /// guarantees `nbits + count <= 64`.
    pub(crate) fn write_bits(&mut self, bits: u32, count: u32) {
        self.acc |= u64::from(bits) << self.nbits;
        self.nbits += count;
        while self.nbits >= 8 {
            self.out.push((self.acc & 0xFF) as u8);
            self.acc >>= 8;
            self.nbits -= 8;
        }
    }

    /// Append a Huffman `code` of `len` bits (the code's MSB goes first on
    /// the wire, which is the bit-reversed integer).
    pub(crate) fn write_code(&mut self, code: u32, len: u32) {
        let rev = reverse_bits(code, len);
        self.write_bits(rev, len);
    }

    /// Pad the final byte with zeros and yield the stream.
    pub(crate) fn finish(mut self) -> Vec<u8> {
        if self.nbits > 0 {
            self.out.push((self.acc & 0xFF) as u8);
            self.acc = 0;
            self.nbits = 0;
        }
        self.out
    }
}

/// Fixed-Huffman code of a literal/length symbol: `(code, bit_length)`.
#[must_use]
pub(crate) fn fixed_lit_code(sym: u16) -> (u32, u32) {
    match sym {
        0..=143 => (0x30 + u32::from(sym), 8),
        144..=255 => (0x190 + u32::from(sym - 144), 9),
        256 => (0, 7),
        257..=279 => (1 + u32::from(sym - 257), 7),
        _ => (0xC0 + u32::from(sym - 280), 8),
    }
}

/// The symbol and extra bits encoding a match length.
#[must_use]
pub(crate) fn length_symbol(len: usize) -> (u16, u32, u32) {
    let len = len.min(MAX_MATCH);
    // The last row is exact for 258; scan otherwise (29 rows, top of the
    // table first would be faster; tables are small enough either way).
    let mut row = LENGTH_TABLE.len() - 1;
    while row > 0 && usize::from(LENGTH_TABLE[row].1) > len {
        row -= 1;
    }
    let (sym, base, extra) = LENGTH_TABLE[row];
    let extra_val = (len - usize::from(base)) as u32;
    (sym, extra_val, extra)
}

/// The symbol and extra bits encoding a match distance.
#[must_use]
pub(crate) fn dist_symbol(dist: usize) -> (u16, u32, u32) {
    let mut row = DIST_TABLE.len() - 1;
    while row > 0 && DIST_TABLE[row].1 as usize > dist {
        row -= 1;
    }
    let (sym, base, extra) = DIST_TABLE[row];
    let extra_val = (dist - base as usize) as u32;
    (sym, extra_val, extra)
}

/// The retained matcher state: one hash head per bucket plus the
/// position-linked chain (all reused across encodes — the steady state
/// allocates nothing).
#[derive(Default)]
pub(crate) struct Matcher {
    head: Vec<i32>,
    prev: Vec<i32>,
}

impl Matcher {
    fn reset(&mut self) {
        self.head.clear();
        self.head.resize(HASH_SIZE, -1);
        self.prev.clear();
        self.prev.resize(WINDOW_MASK + 1, -1);
    }

    #[allow(clippy::cast_possible_truncation)] // positions fit i32 by input limits
    fn hash3(data: &[u8], pos: usize) -> usize {
        let a = u32::from(data[pos]);
        let b = u32::from(data[pos + 1]);
        let c = u32::from(data[pos + 2]);
        let h = (a << 10) ^ (b << 5) ^ c;
        let h = h.wrapping_mul(0x9E37_79B1);
        ((h >> (32 - HASH_BITS)) & (HASH_SIZE as u32 - 1)) as usize
    }

    /// Insert `pos` into the chain and return the previous head.
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    fn insert(&mut self, data: &[u8], pos: usize) -> i32 {
        let h = Self::hash3(data, pos);
        let old = self.head[h];
        self.head[h] = pos as i32;
        self.prev[pos & WINDOW_MASK] = old;
        old
    }

    /// Find the longest match at `pos`, bounded by the chain depth.
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    fn find(&self, data: &[u8], pos: usize) -> (usize, usize) {
        let max_len = (data.len() - pos).clamp(MIN_MATCH, MAX_MATCH);
        if data.len() - pos < MIN_MATCH {
            return (0, 0);
        }
        let mut best_len = MIN_MATCH - 1;
        let mut best_dist = 0usize;
        let mut cand = self.head[Self::hash3(data, pos)];
        let mut chain = MAX_CHAIN;
        while cand >= 0 && chain > 0 {
            let c = cand as usize;
            let dist = pos - c;
            if dist == 0 || dist > WINDOW_MASK + 1 {
                break;
            }
            // Compare; a candidate only replaces the best on a strictly
            // longer match (ties keep the earlier-found = nearer chain
            // position, keeping distances small and the output stable).
            let mut l = 0usize;
            while l < max_len && data[c + l] == data[pos + l] {
                l += 1;
            }
            if l > best_len {
                best_len = l;
                best_dist = dist;
                if l == max_len {
                    break;
                }
            }
            cand = self.prev[c & WINDOW_MASK];
            chain -= 1;
        }
        if best_len >= MIN_MATCH {
            (best_len, best_dist)
        } else {
            (0, 0)
        }
    }
}

/// Compress `data` into one final fixed-Huffman deflate block (no zlib
/// wrapper — the caller adds it).
///
/// All scratch lives in `scratch` (retained across calls by the encoder);
/// the returned vector is freshly allocated per image, as is the output.
pub(crate) fn deflate_fixed(scratch: &mut Matcher, data: &[u8]) -> Vec<u8> {
    scratch.reset();
    let mut w = BitWriter::default();
    // BFINAL=1, BTYPE=01 (fixed Huffman).
    w.write_bits(1, 1);
    w.write_bits(1, 2);
    let mut pos = 0usize;
    while pos < data.len() {
        if data.len() - pos >= MIN_MATCH {
            let (len, dist) = scratch.find(data, pos);
            if len >= MIN_MATCH {
                // Emit the match, then insert the covered positions into
                // the chain (insertion happens after matching so the
                // first position's chain is queried before mutation).
                let (lsym, lextra, lbits) = length_symbol(len);
                let (sym_code, sym_bits) = fixed_lit_code(lsym);
                w.write_code(sym_code, sym_bits);
                if lbits > 0 {
                    w.write_bits(lextra, lbits);
                }
                let (dsym, dextra, dbits) = dist_symbol(dist);
                // Distance codes: 5-bit fixed codes, values 0..=29.
                w.write_code(u32::from(dsym), 5);
                if dbits > 0 {
                    w.write_bits(dextra, dbits);
                }
                for p in pos..pos + len {
                    if data.len() - p >= MIN_MATCH {
                        scratch.insert(data, p);
                    }
                }
                pos += len;
                continue;
            }
        }
        // Literal.
        let (code, len) = fixed_lit_code(u16::from(data[pos]));
        w.write_code(code, len);
        if data.len() - pos >= MIN_MATCH {
            scratch.insert(data, pos);
        }
        pos += 1;
    }
    // End of block.
    let (code, len) = fixed_lit_code(256);
    w.write_code(code, len);
    w.finish()
}

// ---- the test-only inflate: the round-trip oracle --------------------

/// A minimal fixed-Huffman inflate (test-only): decodes exactly what
/// [`deflate_fixed`] emits and nothing more. This is the round-trip proof
/// for the encoder — a second implementation of the format reading the
/// first one's bytes back.
#[cfg(test)]
pub(crate) mod inflate {
    use super::{DIST_TABLE, LENGTH_TABLE};

    struct BitReader<'a> {
        data: &'a [u8],
        pos: usize,
        bit: u32,
    }

    impl<'a> BitReader<'a> {
        fn new(data: &'a [u8]) -> Self {
            BitReader {
                data,
                pos: 0,
                bit: 0,
            }
        }

        /// Read one bit (LSB-first within bytes).
        fn bit(&mut self) -> Option<u32> {
            let byte = *self.data.get(self.pos)?;
            let b = (byte >> self.bit) & 1;
            self.bit += 1;
            if self.bit == 8 {
                self.bit = 0;
                self.pos += 1;
            }
            Some(u32::from(b))
        }

        /// Read `n` bits, LSB first.
        fn bits(&mut self, n: u32) -> Option<u32> {
            let mut v = 0u32;
            for i in 0..n {
                v |= self.bit()? << i;
            }
            Some(v)
        }
    }

    /// Decode one fixed-Huffman literal/length symbol. Reads bits as they
    /// come (first stream bit = the code's MSB) by accumulating the code
    /// prefix and checking the fixed table's ranges.
    fn read_lit(r: &mut BitReader<'_>) -> Option<u16> {
        // Build the code MSB-first: each stream bit extends the code.
        let mut code = 0u32;
        let mut len = 0u32;
        loop {
            code = (code << 1) | r.bit()?;
            len += 1;
            match len {
                7 => {
                    if (0..=23).contains(&code) {
                        // 0000000..0001011 -> 256..278
                        return Some(u16::try_from(256 + code).unwrap());
                    }
                }
                8 => {
                    if (0x30..=0xBF).contains(&code) {
                        return Some(u16::try_from(code - 0x30).unwrap());
                    }
                    if (0xC0..=0xC7).contains(&code) {
                        return Some(u16::try_from(280 + code - 0xC0).unwrap());
                    }
                }
                9 => {
                    if (0x190..=0x1FF).contains(&code) {
                        return Some(u16::try_from(144 + code - 0x190).unwrap());
                    }
                    return None; // Invalid code.
                }
                _ => {}
            }
        }
    }

    /// Inflate a single final fixed-Huffman block produced by
    /// [`super::deflate_fixed`].
    pub(crate) fn inflate_fixed(data: &[u8]) -> Option<Vec<u8>> {
        let mut r = BitReader::new(data);
        let bfinal = r.bit()?;
        let btype = r.bits(2)?;
        if bfinal != 1 || btype != 1 {
            return None; // The encoder emits exactly one fixed final block.
        }
        let mut out: Vec<u8> = Vec::new();
        loop {
            let sym = read_lit(&mut r)?;
            match sym {
                256 => break,
                0..=255 => out.push(sym as u8),
                _ => {
                    let row = usize::from(sym) - 257;
                    if row >= LENGTH_TABLE.len() {
                        return None;
                    }
                    let (_, base, extra) = LENGTH_TABLE[row];
                    let len = usize::from(base) + r.bits(extra)? as usize;
                    // Distance codes are Huffman codes: MSB-first.
                    let mut dsym = 0usize;
                    for _ in 0..5 {
                        dsym = (dsym << 1) | r.bit()? as usize;
                    }
                    if dsym >= DIST_TABLE.len() {
                        return None;
                    }
                    let (_, dbase, dextra) = DIST_TABLE[dsym];
                    let dist = dbase as usize + r.bits(dextra)? as usize;
                    if dist == 0 || dist > out.len() {
                        return None;
                    }
                    for _ in 0..len {
                        let b = out[out.len() - dist];
                        out.push(b);
                    }
                }
            }
        }
        // The rest must be zero padding to the byte boundary.
        if r.bit != 0 {
            let byte = *r.data.get(r.pos)?;
            let pad = byte >> r.bit;
            if pad != 0 {
                return None;
            }
            r.pos += 1;
        }
        if r.pos != r.data.len() {
            return None;
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverse_bits_is_an_involution_on_its_width() {
        assert_eq!(reverse_bits(0b101, 3), 0b101);
        assert_eq!(reverse_bits(0b1101, 4), 0b1011);
        assert_eq!(reverse_bits(0, 7), 0);
        assert_eq!(reverse_bits(0x7F, 7), 0x7F);
        // Reversing twice returns the original within the width.
        for v in [0u32, 1, 0x12, 0xABC, 0x12345] {
            for n in [3u32, 5, 9, 13] {
                let once = reverse_bits(v & ((1 << n) - 1), n);
                let twice = reverse_bits(once, n);
                assert_eq!(twice, v & ((1 << n) - 1));
            }
        }
    }

    #[test]
    fn fixed_lit_codes_match_the_rfc1951_table() {
        // Spot values from RFC 1951 §3.2.6.
        assert_eq!(fixed_lit_code(0), (0x30, 8));
        assert_eq!(fixed_lit_code(143), (0xBF, 8));
        assert_eq!(fixed_lit_code(144), (0x190, 9));
        assert_eq!(fixed_lit_code(255), (0x1FF, 9));
        assert_eq!(fixed_lit_code(256), (0, 7));
        assert_eq!(fixed_lit_code(257), (1, 7));
        assert_eq!(fixed_lit_code(279), (0b001_0111, 7));
        assert_eq!(fixed_lit_code(280), (0xC0, 8));
        assert_eq!(fixed_lit_code(287), (0xC7, 8));
    }

    #[test]
    fn length_and_distance_symbols_hit_table_edges() {
        assert_eq!(length_symbol(3), (257, 0, 0));
        assert_eq!(length_symbol(258), (285, 0, 0));
        assert_eq!(length_symbol(11), (265, 0, 1));
        assert_eq!(length_symbol(12), (265, 1, 1));
        assert_eq!(dist_symbol(1), (0, 0, 0));
        assert_eq!(dist_symbol(3), (2, 0, 0));
        assert_eq!(dist_symbol(4), (3, 0, 0));
        assert_eq!(dist_symbol(5), (4, 0, 1));
        assert_eq!(dist_symbol(6), (4, 1, 1));
        assert_eq!(dist_symbol(32_768), (29, 32_767 - 24_576, 13));
    }

    /// The literal-only deflate stream is hand-derivable: pin the exact
    /// bytes for a 5-literal payload plus end-of-block.
    #[test]
    fn literal_only_stream_is_the_hand_computed_bytes() {
        // Payload: the bytes 0x41, 0x42, 0x90, 0xFF, 0x00 (literals 65, 66,
        // 144, 255, 0), then symbol 256.
        //
        // Codes (RFC 1951 fixed table): 65 -> 8 bits 0x71... (0x30+65=0x71),
        // 66 -> 0x72 (8), 144 -> 0x190 (9), 255 -> 0x1FF (9), 0 -> 0x30 (8),
        // EOB -> 0 (7).
        //
        // Bit stream (each code MSB-first, packed LSB-first into bytes):
        // 1 (BFINAL), 1,0 (BTYPE=01 — note BTYPE's low bit is written first),
        // then the codes, then EOB, then zero padding.
        let mut w = BitWriter::default();
        w.write_bits(1, 1);
        w.write_bits(1, 2);
        for (sym, _) in [(65u16, 0), (66, 0), (144, 0), (255, 0), (0, 0)] {
            let (code, len) = fixed_lit_code(sym);
            w.write_code(code, len);
        }
        w.write_code(0, 7); // EOB
        let stream = w.finish();
        // The independent oracle: the test inflate reads it back.
        let back = inflate::inflate_fixed(&stream).expect("hand stream inflates");
        assert_eq!(back, vec![0x41, 0x42, 0x90, 0xFF, 0x00]);
    }

    #[test]
    fn deflate_round_trips_reference_corpora() {
        let mut m = Matcher::default();
        let cases: Vec<Vec<u8>> = vec![
            Vec::new(),
            vec![0],
            vec![0xFF; 1],
            vec![0xAB; 100_000],                        // one long run
            (0u8..=255).cycle().take(70_000).collect(), // cycling bytes
            {
                // Text-like with repeats.
                let mut v = Vec::new();
                for i in 0..20_000u32 {
                    v.push(b"the quick brown fox "[(i % 20) as usize]);
                }
                v
            },
            {
                // Pseudo-random (LCG) — matches are rare; mostly literals.
                let mut s = 0x1234_5678u64;
                (0..8_192)
                    .map(|_| {
                        s = s.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                        (s >> 33) as u8
                    })
                    .collect()
            },
        ];
        for (i, case) in cases.iter().enumerate() {
            let out = deflate_fixed(&mut m, case);
            let back =
                inflate::inflate_fixed(&out).unwrap_or_else(|| panic!("case {i} did not inflate"));
            assert_eq!(&back, case, "case {i} round-trip mismatch");
        }
    }

    #[test]
    fn runs_actually_compress() {
        let mut m = Matcher::default();
        let run = vec![0x5Au8; 100_000];
        let out = deflate_fixed(&mut m, &run);
        // 100 KB of one byte must compress far below 10 KB (a match every
        // 258 bytes plus headers).
        assert!(
            out.len() < 10_000,
            "run compression failed: {} bytes",
            out.len()
        );
        let text: Vec<u8> = {
            let mut v = Vec::new();
            for i in 0..20_000u32 {
                v.push(b"the quick brown fox "[(i % 20) as usize]);
            }
            v
        };
        let out = deflate_fixed(&mut m, &text);
        assert!(out.len() < text.len() / 2, "text should halve at least");
    }

    #[test]
    fn deflate_output_is_deterministic() {
        let mut a = Matcher::default();
        let mut b = Matcher::default();
        let data: Vec<u8> = (0..50_000u32).map(|i| (i % 251) as u8).collect();
        assert_eq!(deflate_fixed(&mut a, &data), deflate_fixed(&mut b, &data));
        // And across reuses of the same matcher (retained state must reset).
        let again = deflate_fixed(&mut a, &data);
        let third = deflate_fixed(&mut a, &data);
        assert_eq!(again, third);
    }

    /// A match longer than the 32 KiB window must still decode (distance
    /// capped, stream correct).
    #[test]
    fn long_repeats_respect_the_window() {
        let mut m = Matcher::default();
        let data = vec![0x11u8; 100_000];
        let out = deflate_fixed(&mut m, &data);
        assert_eq!(inflate::inflate_fixed(&out).as_deref(), Some(&data[..]));
    }
}
