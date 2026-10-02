//! Malformed-message corpus: every case is a hand-built byte frame that
//! must be rejected at the right validation stage with the right error
//! code (`docs/protocol.md` §9; roadmap Phase 2 EC: "malformed corpus
//! rejected at the right stage"). Nothing here may panic.

use ldp_core::error::ErrorCode;
use ldp_core::limits::Limits;
use ldp_core::wire::{ArgType, Value};
use ldp_protocol::{decode, Message, ValidationMode};

/// Little-endian header assembly (bypasses the encoder on purpose: the
/// corpus simulates a hostile peer).
fn header(words: u32, object: u32, opcode: u32, flags: u16, fds: u16) -> Vec<u8> {
    let mut b = Vec::with_capacity(16);
    b.extend_from_slice(&words.to_le_bytes());
    b.extend_from_slice(&object.to_le_bytes());
    b.extend_from_slice(&opcode.to_le_bytes());
    b.extend_from_slice(&flags.to_le_bytes());
    b.extend_from_slice(&fds.to_le_bytes());
    b
}

fn unit_uint32(v: u32) -> Vec<u8> {
    let mut u = vec![0x02];
    u.extend_from_slice(&v.to_le_bytes());
    u.resize(8, 0);
    u
}

fn unit_string(s: &str) -> Vec<u8> {
    let mut u = vec![0x08];
    u.extend_from_slice(&(s.len() as u32).to_le_bytes());
    u.extend_from_slice(s.as_bytes());
    u.push(0);
    while u.len() % 8 != 0 {
        u.push(0);
    }
    u
}

fn frame_with(payload: &[u8], fds: u16) -> Vec<u8> {
    let mut f = header(payload.len() as u32 / 8, 1, 1, 0, fds);
    f.extend_from_slice(payload);
    f
}

fn expect(bytes: &[u8], ancillary: u32, limits: &Limits, code: ErrorCode) {
    let err = decode(bytes, ancillary, limits, ValidationMode::Tolerant)
        .map(|m| panic!("must be rejected, decoded: {m:?}"))
        .unwrap_err();
    assert_eq!(
        err.wire_code(),
        Some(code),
        "case failed with {err} (wanted {code})"
    );
}

#[test]
fn corpus_framing_stage() {
    let limits = Limits::DEFAULT;

    // 1. Short header.
    expect(&[0u8; 15], 0, &limits, ErrorCode::MalformedMessage);
    // 2. Header/payload length disagreement.
    let mut f = header(2, 1, 1, 0, 0);
    f.extend_from_slice(&unit_uint32(1));
    expect(&f, 0, &limits, ErrorCode::MalformedMessage);
    // 3. Trailing garbage.
    let mut f = frame_with(&unit_uint32(1), 0);
    f.push(0);
    expect(&f, 0, &limits, ErrorCode::MalformedMessage);
    // 4. Reserved flag bits.
    let mut f = header(1, 1, 1, 0x0004, 0);
    f.extend_from_slice(&unit_uint32(1));
    expect(&f, 0, &limits, ErrorCode::MalformedMessage);
    // 5. Message over the negotiated ceiling.
    let tight = Limits {
        message_bytes: 16,
        ..Limits::DEFAULT
    };
    let f = frame_with(&unit_uint32(1), 0);
    expect(&f, 0, &tight, ErrorCode::LimitExceeded);
}

#[test]
fn corpus_fd_stage() {
    let limits = Limits::DEFAULT;
    // Header fd_count vs ancillary disagreement.
    let mut f = header(1, 1, 1, 0, 3);
    f.extend_from_slice(&unit_uint32(1));
    expect(&f, 0, &limits, ErrorCode::FdMismatch);
    expect(&f, 2, &limits, ErrorCode::FdMismatch);
    // fd_count over the per-message ceiling.
    let tight = Limits {
        fds_per_message: 2,
        ..Limits::DEFAULT
    };
    let mut f = header(1, 1, 1, 0, 3);
    f.extend_from_slice(&unit_uint32(1));
    expect(&f, 3, &tight, ErrorCode::LimitExceeded);
    // fd argument index outside the table (structural stage).
    let mut payload = vec![0x0B];
    payload.extend_from_slice(&7u32.to_le_bytes());
    payload.resize(8, 0);
    let f = frame_with(&payload, 2);
    expect(&f, 2, &limits, ErrorCode::FdMismatch);
}

#[test]
fn corpus_structural_stage() {
    let limits = Limits::DEFAULT;

    // Unknown tag byte.
    let mut payload = vec![0x99];
    payload.extend_from_slice(&0u32.to_le_bytes());
    payload.resize(8, 0);
    expect(
        &frame_with(&payload, 0),
        0,
        &limits,
        ErrorCode::MalformedMessage,
    );

    // Bool wire byte 2.
    let mut payload = vec![0x07, 0x02];
    payload.resize(8, 0);
    expect(
        &frame_with(&payload, 0),
        0,
        &limits,
        ErrorCode::MalformedMessage,
    );

    // Non-finite float32 (NaN).
    let mut payload = vec![0x05];
    payload.extend_from_slice(&f32::NAN.to_bits().to_le_bytes());
    payload.resize(8, 0);
    expect(
        &frame_with(&payload, 0),
        0,
        &limits,
        ErrorCode::MalformedMessage,
    );

    // Non-UTF-8 string.
    let mut payload = vec![0x08];
    payload.extend_from_slice(&2u32.to_le_bytes());
    payload.extend_from_slice(&[0xFF, 0xFE]);
    payload.push(0);
    payload.resize(8, 0);
    expect(
        &frame_with(&payload, 0),
        0,
        &limits,
        ErrorCode::InvalidString,
    );

    // Missing NUL terminator.
    let mut payload = vec![0x08];
    payload.extend_from_slice(&2u32.to_le_bytes());
    payload.extend_from_slice(b"hi");
    payload.push(0x41); // should be NUL
    payload.resize(8, 0);
    expect(
        &frame_with(&payload, 0),
        0,
        &limits,
        ErrorCode::MalformedMessage,
    );

    // Over-length string against a tight limit.
    let tight = Limits {
        string_bytes: 4,
        ..Limits::DEFAULT
    };
    let f = frame_with(&unit_string("hello"), 0);
    expect(&f, 0, &tight, ErrorCode::LimitExceeded);
    // ... and the same string is fine under the default limits.
    assert!(decode(&f, 0, &limits, ValidationMode::Tolerant).is_ok());
}

#[test]
fn corpus_array_stage() {
    let limits = Limits::DEFAULT;

    // Count bomb: count claims u32::MAX with no elements present.
    let mut payload = vec![0x0C];
    payload.extend_from_slice(&(ArgType::Uint32.to_wire() as u32).to_le_bytes());
    payload.extend_from_slice(&u32::MAX.to_le_bytes());
    payload.resize(16, 0);
    expect(
        &frame_with(&payload, 0),
        0,
        &limits,
        ErrorCode::MalformedMessage,
    );

    // Element tag disagrees with the declared element type.
    let mut payload = vec![0x0C];
    payload.extend_from_slice(&(ArgType::Uint32.to_wire() as u32).to_le_bytes());
    payload.extend_from_slice(&1u32.to_le_bytes());
    payload.resize(16, 0);
    let mut wrong_elem = unit_uint32(5); // tag 0x02 = uint32, matches...
    wrong_elem[0] = 0x01; // ...force int32 tag mismatch
    payload.extend_from_slice(&wrong_elem);
    expect(
        &frame_with(&payload, 0),
        0,
        &limits,
        ErrorCode::MalformedMessage,
    );

    // Non-primitive element tag (string).
    let mut payload = vec![0x0C];
    payload.extend_from_slice(&(ArgType::String.to_wire() as u32).to_le_bytes());
    payload.extend_from_slice(&0u32.to_le_bytes());
    payload.resize(16, 0);
    expect(
        &frame_with(&payload, 0),
        0,
        &limits,
        ErrorCode::MalformedMessage,
    );

    // Element-tag field with high bits set (u32 > 0xFF).
    let mut payload = vec![0x0C];
    payload.extend_from_slice(&0x0000_0102u32.to_le_bytes());
    payload.extend_from_slice(&0u32.to_le_bytes());
    payload.resize(16, 0);
    expect(
        &frame_with(&payload, 0),
        0,
        &limits,
        ErrorCode::MalformedMessage,
    );

    // Region-rect ceiling: array<rect> with count over region_rects.
    let tight = Limits {
        region_rects: 2,
        ..Limits::DEFAULT
    };
    let mut payload = vec![0x0C];
    payload.extend_from_slice(&(ArgType::Rect.to_wire() as u32).to_le_bytes());
    payload.extend_from_slice(&3u32.to_le_bytes());
    payload.resize(16, 0);
    for _ in 0..3 {
        payload.extend_from_slice(&[
            0x0D, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ]);
    }
    expect(
        &frame_with(&payload, 0),
        0,
        &tight,
        ErrorCode::LimitExceeded,
    );
    // Empty rect array under the tight limit is fine.
    let mut payload = vec![0x0C];
    payload.extend_from_slice(&(ArgType::Rect.to_wire() as u32).to_le_bytes());
    payload.extend_from_slice(&0u32.to_le_bytes());
    payload.resize(16, 0);
    assert!(decode(
        &frame_with(&payload, 0),
        0,
        &tight,
        ValidationMode::Tolerant
    )
    .is_ok());
}

#[test]
fn corpus_padding_strict_vs_tolerant() {
    let limits = Limits::DEFAULT;
    let mut f = frame_with(&unit_uint32(1), 0);
    f[16 + 7] = 0xAA; // last pad byte of the first unit
                      // Tolerant: accepted (the wire contract does not require pad content).
    assert!(decode(&f, 0, &limits, ValidationMode::Tolerant).is_ok());
    // Strict: rejected.
    let err = decode(&f, 0, &limits, ValidationMode::Strict).unwrap_err();
    assert_eq!(err.wire_code(), Some(ErrorCode::MalformedMessage));
}

#[test]
fn corpus_forward_compatible_domains() {
    // Undeclared bitset bits and unknown enum values decode fine in both
    // modes (range policing is signature-stage strictness, never framing).
    let limits = Limits::DEFAULT;
    let bs = Message::new(1, 1)
        .arg(Value::Bitset(ldp_core::bitset::Bitset128::single(127)))
        .encode(&limits)
        .unwrap();
    assert!(decode(&bs, 0, &limits, ValidationMode::Strict).is_ok());
    let ev = Message::new(1, 1)
        .arg(Value::Enum(0xDEAD_BEEF))
        .encode(&limits)
        .unwrap();
    assert!(decode(&ev, 0, &limits, ValidationMode::Strict).is_ok());
}
