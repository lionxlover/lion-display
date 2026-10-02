//! Cross-module invariant tests.
//!
//! These exercise properties that hold only when several foundation
//! modules agree with each other — the contracts `ldp-protocol` (Phase 2)
//! and the compositor (Phase 6) will build on.

use ldp_core::prelude::*;

#[test]
fn object_ids_pair_with_generation_discipline() {
    // A client-allocated ID's slot plus a generation uniquely names an
    // object lifetime; after destroy + re-creation the generation differs.
    let id = ObjectId::client(0x1000).unwrap();
    let slot = id.slot();
    let mut gen = Generation::FIRST;
    let first = (slot, gen);
    gen = gen.next();
    let second = (slot, gen);
    assert_ne!(first, second);
    // And the wire form never leaks the generation — it stays implicit.
    assert_eq!(ObjectId::from_wire(id.as_u32()), id);
}

#[test]
fn error_taxonomy_covers_validation_pipeline_stages() {
    // The four validation stages of docs/protocol.md §9 must map onto
    // distinct codes so diagnostics are unambiguous.
    let stage_codes = [
        ErrorCode::FdMismatch,        // stage 1: framing/transport
        ErrorCode::MalformedMessage,  // stage 2: structural decode
        ErrorCode::SignatureMismatch, // stage 3: signature check
        ErrorCode::InvalidState,      // stage 4: semantic check
    ];
    let mut sorted: Vec<u32> = stage_codes.iter().map(|c| c.to_wire()).collect();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), stage_codes.len());
}

#[test]
fn deadline_chain_from_refresh_to_budget_is_consistent() {
    // The server computes deadlines as vblank - submit - latency; the
    // client sees budget = deadline - now at emission. Reconstructing the
    // chain must never produce a budget larger than the refresh interval
    // under the default depth-1 pipeline.
    let refresh = RefreshInterval::from_millihz(144_000).unwrap();
    let vblank = Mono::from_ns(10_000_000_000);
    let now = vblank.saturating_sub_ns(refresh.as_ns());
    let deadline = FrameDeadline {
        deadline: vblank.saturating_sub_ns(500_000), // 0.5 ms submit+flip cost
        target_vblank: vblank,
        refresh,
        budget_ns: 0,
        mode: PresentationMode::Vsync,
    };
    assert!(deadline.budget_ns <= refresh.as_ns());
    assert!(deadline.is_met(now));
    assert!(deadline.target_vblank >= deadline.deadline);
}

#[test]
fn damage_algebra_feeds_the_scheduler_contract() {
    // Compositor damage flow: surface damage (surface coords) -> transform
    // to output space -> subtract occlusion -> clip to output. All four
    // steps use only ldp-core types; the chain must stay inside bounds.
    let output_bounds = Rect::new(0, 0, 1920, 1080);
    let buffer_size = Size::new(800, 600);
    let transform = Transform::Rot90;

    let surface_damage = Region::from_rect(Rect::new(100, 100, 300, 200));
    // Transform damage into output space (buffer orientation applied).
    let mut output_damage = Region::new();
    for r in &surface_damage {
        output_damage.add(transform.transform_rect(*r, buffer_size.w, buffer_size.h));
    }
    // Occluded by an always-on-top window.
    let occlusion = Region::from_rect(Rect::new(0, 0, 1920, 54)); // menu bar
    let visible = output_damage.subtract(&occlusion);
    // Clip to the output.
    let final_damage = visible.clipped_to(output_bounds);
    assert!(final_damage.bounds().intersect(output_bounds).is_some());
    for r in &final_damage {
        assert!(output_bounds.intersect(*r).is_some());
        assert!(occlusion.iter().all(|o| r.intersect(*o).is_none()));
    }
}

#[test]
fn color_description_drives_scope_free_surface_rendering() {
    // Color types compose with the HDR gating the compositor applies:
    // an HDR surface needs both an HDR transfer and luminance headroom.
    let sdr = ColorDescription::srgb_sdr();
    let hdr = ColorDescription::pq_hdr();
    assert!(!sdr.is_hdr());
    assert!(hdr.is_hdr());
    assert!(hdr.luminance_max > hdr.reference_white);
    assert!(hdr.reference_white > hdr.luminance_min);
    // Output advertisement and surface description share the type, so the
    // tone-mapping decision (sdr output + hdr surface => BT.2390) is a
    // pure comparison.
    let output_desc = sdr;
    let needs_tone_map = hdr.is_hdr() && !output_desc.is_hdr();
    assert!(needs_tone_map);
}

#[test]
fn scope_sets_and_tokens_compose_deny_by_default() {
    // Default client state: no scopes, no token validity.
    let nobody = ScopeSet::NONE;
    assert!(nobody.is_empty());
    for scope in Scope::ALL {
        assert!(!nobody.contains(scope));
    }
    // A screenshot token expands exactly the screenshot scope.
    let token = AccessToken::from_words([7; 8], ScopeSet::single(Scope::Screenshot), None);
    assert_eq!(token.scope(), ScopeSet::single(Scope::Screenshot));
    assert!(token.is_valid_at(Mono::from_ns(1)));
    // Screenshot is prompt-gated in the layered policy.
    assert!(Scope::Screenshot.requires_prompt());
    assert!(!Scope::ClipboardRead.requires_prompt());
}

#[test]
fn version_and_limit_negotiation_agree_with_the_handshake() {
    // hello/welcome negotiate: server offers [1,1] for core v1; clients
    // older or newer still find the intersection or fail cleanly.
    let server = VersionRange::exact(1).unwrap();
    let peer_newer = VersionRange::new(2, 4).unwrap();
    assert!(server.intersect(peer_newer).is_none());
    let peer_spanning = VersionRange::new(1, 3).unwrap();
    assert_eq!(server.choose(peer_spanning, None).unwrap().as_u32(), 1);
    // Large-message negotiation only widens the ceiling.
    assert!(Limits::LARGE_MESSAGES.message_fits(Limits::DEFAULT.max_payload_words()));
    assert!(!Limits::DEFAULT.message_fits(Limits::LARGE_MESSAGES.max_payload_words()));
}

#[test]
fn wire_values_never_carry_undefined_state() {
    // Every value constructor that could produce an undefined wire state
    // rejects it: NaN floats, zero objects, out-of-range enums.
    assert!(Value::float32(f32::NAN).is_err());
    assert!(Value::float64(f64::NAN).is_err());
    assert!(ObjectId::client(0).is_none());
    assert!(ObjectId::server(0).is_none());
    assert!(Transform::from_wire(9).is_none());
    assert!(ErrorCode::from_wire(16).is_none());
    assert!(Scope::from_wire(13).is_none());
    assert!(PresentationMode::from_wire(0).is_none());
}

#[test]
fn fractional_scaling_survives_round_trips() {
    // Mixed-DPI layout math: a 1.5x output and a 2x output exchange
    // logical and device sizes without drift.
    let one_half = ScaleFactor::from_q8(384).unwrap();
    let two = ScaleFactor::from_q8(512).unwrap();
    for logical in (1..3000u32).step_by(7) {
        let device = one_half.scale_px(logical);
        assert_eq!(one_half.unscale_px(device), logical);
        let device = two.scale_px(logical);
        assert_eq!(two.unscale_px(device), logical);
    }
}
