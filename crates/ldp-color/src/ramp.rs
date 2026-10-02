//! Exact integer PQ ramps.
//!
//! `docs/architecture.md` §12 precision rule: "PQ encodes via the exact
//! integer 10/12-bit ramps (no float rounding drift)". This module owns
//! those ramps. A [`PqRamp`] is built once (in f64, through the
//! [`pq_decode_f64`] reference) as the table of every representable
//! quantized PQ code → linear value; the **decode** direction is a pure
//! table lookup, and the **encode** direction is a nearest-code search
//! over the monotone table — no `powf` on either path, and
//! `decode(encode(l))` returns exactly the nearest table entry, so
//! quantization round trips are drift-free and bit-stable everywhere.
//!
//! ```ignore
//! let ramp = ldp_color::PqRamp::new(10)?;
//! let code = ramp.encode_linear(0.01);      // 100 nits → code 520-ish
//! let linear = ramp.decode(code);           // exact table entry
//! assert_eq!(ramp.encode_linear(linear), code); // always
//! ```

use crate::transfer::pq_decode_f64;

/// An integer PQ ramp for one bit depth (8..=16).
///
/// The table stores absolute linear values in the PQ domain's units
/// (nits/10000) so it composes directly with the f32 curve pair.
#[derive(Clone, Debug)]
pub struct PqRamp {
    bits: u8,
    /// `decode[code]` = linear value (nits/10000) of that code.
    table: Vec<f32>,
}

impl PqRamp {
    /// Build the ramp for `bits` (8..=16) codes.
    ///
    /// # Errors
    /// [`RampError::Bits`] when `bits` is outside 8..=16 (v1 buffer
    /// formats only ever quantize PQ at 10 or 12 bits, but the table is
    /// depth-generic; beyond 16 bits the 4096→65536-entry growth and
    /// f32 table precision stop making sense).
    pub fn new(bits: u8) -> Result<PqRamp, RampError> {
        if !(8..=16).contains(&bits) {
            return Err(RampError::Bits { bits });
        }
        let codes = 1usize << bits;
        let max_code = f64::from(codes as u32 - 1);
        let mut table = Vec::with_capacity(codes);
        for code in 0..codes {
            let v = f64::from(code as u32) / max_code;
            // Single f64 evaluation, cast once at the end: the table
            // content never depends on intermediate f32 rounding.
            table.push(pq_decode_f64(v) as f32);
        }
        Ok(PqRamp { bits, table })
    }

    /// The ramp's bit depth.
    #[must_use]
    pub fn bits(&self) -> u8 {
        self.bits
    }

    /// Number of codes (`2^bits`).
    #[must_use]
    pub fn len(&self) -> usize {
        self.table.len()
    }

    /// Whether the ramp is empty — always `false` (a valid ramp has at
    /// least 256 entries); the method exists for clippy's len contract.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    /// Decode an integer code → exact linear value (nits/10000).
    ///
    /// # Errors
    /// [`RampError::Code`] when `code >= 2^bits`.
    pub fn decode(&self, code: u32) -> Result<f32, RampError> {
        let code_u = code as usize;
        if code_u >= self.table.len() {
            return Err(RampError::Code {
                code,
                bits: self.bits,
            });
        }
        Ok(self.table[code_u])
    }

    /// Encode a linear value (nits/10000) → the nearest code.
    ///
    /// Nearest by table value; ties resolve to the **lower** code (the
    /// documented rule — deterministic across platforms). The input is
    /// clamped into the table's domain first.
    #[must_use]
    pub fn encode_linear(&self, linear: f32) -> u32 {
        let target = linear.clamp(0.0, self.table[self.table.len() - 1]);
        // Binary search for the first table entry >= target.
        let mut lo = 0usize;
        let mut hi = self.table.len(); // exclusive
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.table[mid] < target {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        // `lo` is the first entry >= target (or len-1 at the top).
        let upper = lo.min(self.table.len() - 1);
        if upper == 0 {
            return 0;
        }
        let lower = upper - 1;
        // Tie rule: strictly closer lower wins; equal distance → lower.
        let d_up = self.table[upper] - target;
        let d_lo = target - self.table[lower];
        if d_up < d_lo {
            upper as u32
        } else {
            lower as u32
        }
    }
}

/// Ramp construction/lookup failures.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RampError {
    /// Bit depth outside 8..=16.
    Bits {
        /// The rejected depth.
        bits: u8,
    },
    /// Code larger than the ramp's maximum.
    Code {
        /// The offending code.
        code: u32,
        /// The ramp's bit depth.
        bits: u8,
    },
}

impl core::fmt::Display for RampError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Self::Bits { bits } => write!(f, "PQ ramp bit depth {bits} outside 8..=16"),
            Self::Code { code, bits } => {
                write!(f, "code {code} exceeds the {bits}-bit ramp maximum")
            }
        }
    }
}

impl std::error::Error for RampError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transfer::decode_transfer;
    use ldp_core::color::TransferFunction;

    #[test]
    fn rejects_bad_depths() {
        assert!(matches!(PqRamp::new(7), Err(RampError::Bits { bits: 7 })));
        assert!(matches!(PqRamp::new(17), Err(RampError::Bits { bits: 17 })));
        assert!(PqRamp::new(8).is_ok());
        assert!(PqRamp::new(16).is_ok());
    }

    #[test]
    fn endpoints_are_exact() {
        for bits in [10u8, 12] {
            let ramp = PqRamp::new(bits).unwrap();
            assert_eq!(ramp.len(), 1 << bits);
            assert_eq!(ramp.decode(0).unwrap(), 0.0);
            // PQ(1.0) = 1.0 exactly (closed-form collapse) — so the top
            // code is exactly 1.0 = 10 000 nits, no drift.
            assert_eq!(ramp.decode((1 << bits) - 1).unwrap(), 1.0);
            assert!(matches!(
                ramp.decode(1 << bits),
                Err(RampError::Code { .. })
            ));
        }
    }

    #[test]
    fn integer_round_trip_is_exact() {
        // THE property: for every code, decode → encode returns the same
        // code. Zero float drift, by construction.
        for bits in [8u8, 10, 12] {
            let ramp = PqRamp::new(bits).unwrap();
            for code in 0..ramp.len() as u32 {
                assert_eq!(
                    ramp.encode_linear(ramp.decode(code).unwrap()),
                    code,
                    "{bits}-bit round trip broke at {code}"
                );
            }
        }
    }

    #[test]
    fn tables_are_strictly_monotone() {
        for bits in [10u8, 12, 16] {
            let ramp = PqRamp::new(bits).unwrap();
            for w in ramp.table.windows(2) {
                assert!(w[0] < w[1], "non-monotone step in the {bits}-bit ramp");
            }
        }
    }

    #[test]
    fn encode_is_monotone_and_sane() {
        let ramp = PqRamp::new(10).unwrap();
        let mut prev_code = 0;
        for i in 0..=1000 {
            let l = i as f32 / 1000.0;
            let code = ramp.encode_linear(l);
            assert!(code < 1024);
            assert!(code >= prev_code, "encode not monotone at {l}");
            prev_code = code;
            // Decoded result is within half a table step of the input
            // (nearest-code contract): |out - in| <= the local neighbor
            // gap (the gap on the side with a neighbor; at the table
            // edges only one side exists).
            let out = ramp.decode(code).unwrap();
            let gap = if code < 1023 {
                ramp.decode(code + 1).unwrap() - out
            } else {
                out - ramp.decode(code - 1).unwrap()
            };
            assert!(
                (out - l).abs() <= gap.max(1e-7),
                "nearest-code violated at {l}"
            );
        }
        // Clamping on both sides.
        assert_eq!(ramp.encode_linear(-1.0), 0);
        assert_eq!(ramp.encode_linear(2.0), 1023);
    }

    #[test]
    fn table_agrees_with_the_f32_curve() {
        // The f64-built table and the on-the-fly f32 decode agree within
        // the f32 curve's intrinsic noise: near the top of the range the
        // decode's C2 - C3·vp cancellation amplifies f32 powf error
        // (~60×) and the ^(1/m1) exponent amplifies it again — up to
        // ~2e-5 absolute. That noise is precisely why the exact ramps
        // exist (the architecture's no-drift doctrine): the TABLE is
        // the authority, the curve is the approximation.
        let ramp = PqRamp::new(10).unwrap();
        for code in (0..1024u32).step_by(7) {
            let v = code as f32 / 1023.0;
            let curve = decode_transfer(TransferFunction::Pq, v);
            let table = ramp.decode(code).unwrap();
            assert!(
                (curve - table).abs() <= 5e-5,
                "table drift at code {code}: {curve} vs {table}"
            );
        }
        // Known spot: 100 nits is near code 520 (PQ(100 nits) ≈ 0.508).
        let code_100 = ramp.encode_linear(0.01);
        let lin = ramp.decode(code_100).unwrap();
        assert!((lin - 0.01).abs() < 3e-4, "100-nit code {code_100} → {lin}");
        assert!(
            (code_100 as i32 - 520).abs() <= 2,
            "100-nit code {code_100}"
        );
    }
}
