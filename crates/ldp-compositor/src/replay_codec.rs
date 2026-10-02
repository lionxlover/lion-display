//! Binary codec for scheduler recordings.
//!
//! The format is self-describing and versioned: `LDP7REC` magic, a
//! version byte, the scheduler config and nominal refresh, the ordered
//! input stream, the emission stream, and a trailing FNV-1a checksum
//! over everything before it. Encoding is canonical (equal recordings
//! encode to equal bytes); decoding validates structure — magic,
//! version, checksum, non-decreasing input timestamps, known tags,
//! nonzero refresh — and rejects corruption with explicit errors.
//!
//! This module owns the byte level; [`crate::replay`] owns the
//! recording semantics (`run`/`record`/`replay`).

use ldp_core::time::{Mono, PresentationMode, RefreshInterval};

use crate::replay::{Recording, ReplayError};
use crate::sched_types::{SchedEvent, SchedInput};
use crate::surface::SurfaceId;

const MAGIC: &[u8; 7] = b"LDP7REC";
const VERSION: u8 = 1;

impl Recording {
    /// Encode to the self-describing binary format (checksummed).
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::default();
        w.raw(MAGIC);
        w.u8(VERSION);
        let c = &self.config;
        w.u64(self.nominal_ns);
        w.u64(c.submit_cost_ns);
        w.u64(c.flip_latency_ns);
        w.u64(c.min_commit_lead_ns);
        w.u64(c.arrival_slack_ns);
        w.u64(c.vrr_window_ns);
        w.u32(c.vsync_depth);
        w.u32(c.escalate_after);
        w.u32(c.max_extra_lead);
        w.u32(c.deescalate_hits);
        w.u64(self.inputs.len() as u64);
        for input in &self.inputs {
            encode_input(&mut w, input);
        }
        w.u64(self.outputs.len() as u64);
        for event in &self.outputs {
            encode_event(&mut w, event);
        }
        let checksum = fnv1a(&w.buf);
        w.u64(checksum);
        w.buf
    }

    /// Decode and structurally validate.
    ///
    /// # Errors
    /// Any [`ReplayError`] except [`ReplayError::Divergence`].
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ReplayError> {
        if bytes.len() < MAGIC.len() + 8 {
            return Err(ReplayError::Truncated);
        }
        let (body, checksum_bytes) = bytes.split_at(bytes.len() - 8);
        let mut r = Reader::new(body);
        if r.raw(MAGIC.len())? != MAGIC.as_slice() {
            return Err(ReplayError::BadMagic);
        }
        if r.u8()? != VERSION {
            return Err(ReplayError::BadVersion);
        }
        let stored = u64::from_le_bytes(
            checksum_bytes
                .try_into()
                .map_err(|_| ReplayError::Truncated)?,
        );
        if fnv1a(body) != stored {
            return Err(ReplayError::Checksum);
        }
        let nominal_ns = r.u64()?;
        if nominal_ns == 0 {
            return Err(ReplayError::BadValue);
        }
        let config = crate::scheduler::SchedulerConfig {
            submit_cost_ns: r.u64()?,
            flip_latency_ns: r.u64()?,
            min_commit_lead_ns: r.u64()?,
            arrival_slack_ns: r.u64()?,
            vrr_window_ns: r.u64()?,
            // The pinned v1 replay format does not carry the window's
            // min side (Phase 39's bring-up probe): a replay exercises
            // decision reproducibility, and the measurement band feeds
            // only the `presented` refresh reporting — never a
            // decision. Decoding keeps the legacy band, so the
            // existing corpora stay byte-valid.
            vrr_min_ns: 0,
            vsync_depth: r.u32()?,
            escalate_after: r.u32()?,
            max_extra_lead: r.u32()?,
            deescalate_hits: r.u32()?,
        };
        let input_count = r.u64()? as usize;
        // The §9 doctrine, applied: never size an allocation from an
        // untrusted count when the remaining bytes refute it. Every
        // encoded input is at least its tag plus a timestamp (9 bytes),
        // so a count the remaining bytes cannot carry is `Truncated`
        // — a typed refusal, not a `u64::MAX` capacity abort.
        if input_count > r.remaining() / 9 {
            return Err(ReplayError::Truncated);
        }
        let mut inputs = Vec::with_capacity(input_count);
        let mut last_ts = 0u64;
        for _ in 0..input_count {
            let input = decode_input(&mut r)?;
            let ts = input.ts().as_ns();
            if ts < last_ts {
                return Err(ReplayError::BadValue);
            }
            last_ts = ts;
            inputs.push(input);
        }
        let output_count = r.u64()? as usize;
        // Same guard: every encoded event is at least its tag, a
        // surface id, and a frame number (18 bytes).
        if output_count > r.remaining() / 18 {
            return Err(ReplayError::Truncated);
        }
        let mut outputs = Vec::with_capacity(output_count);
        for _ in 0..output_count {
            outputs.push(decode_event(&mut r)?);
        }
        r.done()?;
        Ok(Self {
            config,
            nominal_ns,
            inputs,
            outputs,
        })
    }
}

fn encode_input(w: &mut Writer, input: &SchedInput) {
    match *input {
        SchedInput::Flip { ts } => {
            w.u8(1);
            w.u64(ts.as_ns());
        }
        SchedInput::FrameRequest { surface, frame, ts } => {
            w.u8(2);
            w.u64(surface.raw());
            w.u64(frame);
            w.u64(ts.as_ns());
        }
        SchedInput::Commit { surface, ts } => {
            w.u8(3);
            w.u64(surface.raw());
            w.u64(ts.as_ns());
        }
        SchedInput::SetVisibility {
            surface,
            hidden,
            ts,
        } => {
            w.u8(4);
            w.u64(surface.raw());
            w.u8(u8::from(hidden));
            w.u64(ts.as_ns());
        }
        SchedInput::SetMode { surface, mode, ts } => {
            w.u8(5);
            w.u64(surface.raw());
            w.u8(mode.to_wire() as u8);
            w.u64(ts.as_ns());
        }
        SchedInput::SetProfile {
            surface,
            profile,
            ts,
        } => {
            // Phase 47: the semantic scene-profile input, appended as
            // tag 8 — the format stays backward-compatible (existing
            // corpora never carry the tag; a pre-Phase-47 decoder
            // rejects it as unknown, the honest version boundary).
            w.u8(8);
            w.u64(surface.raw());
            w.u8(profile.wire() as u8);
            w.u64(ts.as_ns());
        }
        SchedInput::Park { ts } => {
            w.u8(6);
            w.u64(ts.as_ns());
        }
        SchedInput::Resume { ts } => {
            w.u8(7);
            w.u64(ts.as_ns());
        }
    }
}

fn decode_input(r: &mut Reader<'_>) -> Result<SchedInput, ReplayError> {
    let tag = r.u8()?;
    Ok(match tag {
        1 => SchedInput::Flip {
            ts: Mono::from_ns(r.u64()?),
        },
        2 => SchedInput::FrameRequest {
            surface: SurfaceId::from_raw(r.u64()?),
            frame: r.u64()?,
            ts: Mono::from_ns(r.u64()?),
        },
        3 => SchedInput::Commit {
            surface: SurfaceId::from_raw(r.u64()?),
            ts: Mono::from_ns(r.u64()?),
        },
        4 => SchedInput::SetVisibility {
            surface: SurfaceId::from_raw(r.u64()?),
            hidden: r.u8()? != 0,
            ts: Mono::from_ns(r.u64()?),
        },
        5 => SchedInput::SetMode {
            surface: SurfaceId::from_raw(r.u64()?),
            mode: decode_mode(r.u8()?)?,
            ts: Mono::from_ns(r.u64()?),
        },
        8 => SchedInput::SetProfile {
            surface: SurfaceId::from_raw(r.u64()?),
            profile: crate::semantics::SceneProfile::from_wire(u32::from(r.u8()?))
                .ok_or(ReplayError::BadValue)?,
            ts: Mono::from_ns(r.u64()?),
        },
        6 => SchedInput::Park {
            ts: Mono::from_ns(r.u64()?),
        },
        7 => SchedInput::Resume {
            ts: Mono::from_ns(r.u64()?),
        },
        tag => return Err(ReplayError::UnknownInputTag(tag)),
    })
}

fn encode_event(w: &mut Writer, event: &SchedEvent) {
    match *event {
        SchedEvent::FrameTarget {
            surface,
            frame,
            deadline,
        } => {
            w.u8(1);
            w.u64(surface.raw());
            w.u64(frame);
            w.u64(deadline.deadline.as_ns());
            w.u64(deadline.target_vblank.as_ns());
            w.u64(deadline.refresh.as_ns());
            w.u64(deadline.budget_ns);
            w.u8(deadline.mode.to_wire() as u8);
        }
        SchedEvent::Presented { surface, timing } => {
            w.u8(2);
            w.u64(surface.raw());
            w.u64(timing.frame);
            w.u64(timing.presented_at.as_ns());
            w.u64(timing.refresh.as_ns());
            w.u8(timing.flags.to_wire() as u8);
        }
        SchedEvent::FrameDropped {
            surface,
            frame,
            reason,
        } => {
            w.u8(3);
            w.u64(surface.raw());
            w.u64(frame);
            w.u8(reason.to_wire() as u8);
        }
    }
}

fn decode_event(r: &mut Reader<'_>) -> Result<SchedEvent, ReplayError> {
    let tag = r.u8()?;
    Ok(match tag {
        1 => SchedEvent::FrameTarget {
            surface: SurfaceId::from_raw(r.u64()?),
            frame: r.u64()?,
            deadline: ldp_core::time::FrameDeadline {
                deadline: Mono::from_ns(r.u64()?),
                target_vblank: Mono::from_ns(r.u64()?),
                refresh: RefreshInterval::from_ns(r.u64()?).ok_or(ReplayError::BadValue)?,
                budget_ns: r.u64()?,
                mode: decode_mode(r.u8()?)?,
            },
        },
        2 => SchedEvent::Presented {
            surface: SurfaceId::from_raw(r.u64()?),
            timing: ldp_core::time::PresentationTiming {
                frame: r.u64()?,
                presented_at: Mono::from_ns(r.u64()?),
                refresh: RefreshInterval::from_ns(r.u64()?).ok_or(ReplayError::BadValue)?,
                flags: ldp_core::time::PresentationFlags::from_wire(u32::from(r.u8()?)),
            },
        },
        3 => SchedEvent::FrameDropped {
            surface: SurfaceId::from_raw(r.u64()?),
            frame: r.u64()?,
            reason: decode_reason(r.u8()?)?,
        },
        tag => return Err(ReplayError::UnknownEventTag(tag)),
    })
}

fn decode_mode(byte: u8) -> Result<PresentationMode, ReplayError> {
    PresentationMode::from_wire(u32::from(byte)).ok_or(ReplayError::UnknownMode(byte))
}

fn decode_reason(byte: u8) -> Result<ldp_core::time::FrameDropReason, ReplayError> {
    ldp_core::time::FrameDropReason::from_wire(u32::from(byte))
        .ok_or(ReplayError::UnknownReason(byte))
}

#[derive(Default)]
struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    fn raw(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }
    fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }
    /// Bytes not yet consumed — the honest upper bound on how many
    /// more fixed-width fields can follow (the allocation guard the
    /// codec's own §9 doctrine demands: never size an allocation from
    /// an untrusted count when the remaining bytes refute it).
    fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }
    fn raw(&mut self, n: usize) -> Result<&'a [u8], ReplayError> {
        let end = self.pos.checked_add(n).ok_or(ReplayError::Truncated)?;
        if end > self.bytes.len() {
            return Err(ReplayError::Truncated);
        }
        let out = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(out)
    }
    fn u8(&mut self) -> Result<u8, ReplayError> {
        Ok(self.raw(1)?[0])
    }
    fn u32(&mut self) -> Result<u32, ReplayError> {
        Ok(u32::from_le_bytes(
            self.raw(4)?
                .try_into()
                .map_err(|_| ReplayError::Truncated)?,
        ))
    }
    fn u64(&mut self) -> Result<u64, ReplayError> {
        Ok(u64::from_le_bytes(
            self.raw(8)?
                .try_into()
                .map_err(|_| ReplayError::Truncated)?,
        ))
    }
    fn done(&self) -> Result<(), ReplayError> {
        if self.pos == self.bytes.len() {
            Ok(())
        } else {
            Err(ReplayError::Trailing)
        }
    }
}

/// FNV-1a 64-bit over the payload.
///
/// Exposed for tooling and tests that construct or verify recording
/// checksums (`ldp-debug` will do exactly this).
#[must_use]
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}
