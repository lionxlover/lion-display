//! Bounded byte primitives shared by the codec.
//!
//! The `Reader` and `Writer` types are the codec's only contact with raw
//! bytes: little-endian, bounds-checked, 8-byte unit padding, never
//! panicking on untrusted input. Keeping them here separates "byte
//! mechanics" from the argument-type semantics of `codec`.

use ldp_core::error::{ErrorCode, LdpError, Result};

use crate::message::ValidationMode;

/// Bounded little-endian reader; never panics, never indexes past the end.
pub(crate) struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// Wrap a payload slice.
    pub(crate) fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    /// Bytes left to read.
    pub(crate) fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub(crate) fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.remaining() < n {
            return Err(LdpError::malformed(
                ErrorCode::MalformedMessage,
                "argument unit overruns the payload",
            ));
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    pub(crate) fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    pub(crate) fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub(crate) fn u64(&mut self) -> Result<u64> {
        let b = self.take(8)?;
        let mut w = [0u8; 8];
        w.copy_from_slice(b);
        Ok(u64::from_le_bytes(w))
    }

    /// Read `n` padding bytes; strict mode requires them to be zero
    /// (tolerant mode must not require it, per the wire contract).
    pub(crate) fn pad(&mut self, n: usize, mode: ValidationMode) -> Result<()> {
        if n == 0 {
            return Ok(());
        }
        let p = self.take(n)?;
        if mode == ValidationMode::Strict && p.iter().any(|&b| b != 0) {
            return Err(LdpError::malformed(
                ErrorCode::MalformedMessage,
                "nonzero padding (strict mode)",
            ));
        }
        Ok(())
    }
}

/// Append-only little-endian writer with 8-byte unit padding.
pub(crate) struct Writer {
    out: Vec<u8>,
}

impl Writer {
    /// New empty writer.
    pub(crate) fn new() -> Self {
        Writer { out: Vec::new() }
    }

    /// Finish and take the encoded bytes.
    pub(crate) fn into_bytes(self) -> Vec<u8> {
        self.out
    }

    /// Push the argument type tag byte.
    pub(crate) fn tag(&mut self, t: ldp_core::wire::ArgType) {
        self.out.push(t.to_wire());
    }

    /// Push a raw byte (bool payload).
    pub(crate) fn u8(&mut self, v: u8) {
        self.out.push(v);
    }

    /// Push a little-endian u32.
    pub(crate) fn u32(&mut self, v: u32) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    /// Push a little-endian u64.
    pub(crate) fn u64(&mut self, v: u64) {
        self.out.extend_from_slice(&v.to_le_bytes());
    }

    /// Push raw bytes (string contents).
    pub(crate) fn bytes(&mut self, v: &[u8]) {
        self.out.extend_from_slice(v);
    }

    /// Zero-pad to the next multiple of 8.
    pub(crate) fn pad_to8(&mut self) {
        while self.out.len() % 8 != 0 {
            self.out.push(0);
        }
    }
}

/// Round `n` up to the next multiple of 8.
pub(crate) fn round_up8(n: usize) -> usize {
    n.div_ceil(8) * 8
}
