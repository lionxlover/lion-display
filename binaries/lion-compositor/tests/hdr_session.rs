//! Phase 31 exit criteria — the HDR pipeline, CI-proven.
//!
//! The color machinery always existed (PQ/HLG transfers, BT.2020
//! primaries, per-surface `set_color`/`set_hdr_metadata`, the
//! BT.2390 tone mapper); Phase 31 wires it into the *running
//! compositor*: `--hdr` advertises the panel's truth over
//! `output.hdr_caps` (PQ, wide gamut, the peak), the render pass
//! folds the desktop's stack into the mode controller's hysteresis
//! (an HDR surface takes the composite onto the PQ canvas after the
//! dwell — a lone popup cannot flap the panel), and the renderer's
//! cross-description pipeline blends every layer scene-linear with
//! PQ encoding on the tail (SDR content rides at the BT.2408
//! reference white).

mod testbench;

use std::io::Write;

use ldp_core::color::{ColorDescription, HdrMetadata, Primaries, TransferFunction};
use ldp_core::geometry::Rect;
use ldp_core::wire::Value;
use testbench::*;

use lion_compositor::server::CompositorConfig;

/// A memfd pool carrying explicit pixel bytes.
fn pool_bytes(pixels: &[u8]) -> std::os::fd::OwnedFd {
    let fd = lion_compositor::sys::memfd("hdr").expect("memfd");
    let mut file = std::fs::File::from(fd);
    file.write_all(pixels).expect("write pixels");
    file.into()
}

fn start_hdr(tag: &str) -> Testbench {
    let config = CompositorConfig {
        socket: format!("lion-it-hdr-{tag}-{}", std::process::id()),
        hdr: true,
        ..CompositorConfig::default()
    };
    Testbench::start_with(tag, config)
}

/// `surface.set_color(primaries, transfer, range, min, max, white)`.
fn set_color(client: &mut TestClient, surface: &ldp_client::Proxy, color: ColorDescription) {
    client
        .conn
        .send_request(
            surface,
            "set_color",
            vec![
                Value::Enum(color.primaries.to_wire()),
                Value::Enum(color.transfer.to_wire()),
                Value::Enum(color.range.to_wire()),
                Value::Uint32(color.luminance_min.as_wire_units()),
                Value::Uint32(color.luminance_max.as_wire_units()),
                Value::Uint32(color.reference_white.as_wire_units()),
            ],
        )
        .expect("set_color");
}

/// `surface.set_hdr_metadata(primaries, min, max, cll, fall)`.
fn set_hdr_metadata(client: &mut TestClient, surface: &ldp_client::Proxy, meta: HdrMetadata) {
    client
        .conn
        .send_request(
            surface,
            "set_hdr_metadata",
            vec![
                Value::Enum(meta.mastering_primaries.to_wire()),
                Value::Uint32(meta.mastering_luminance_min.as_wire_units()),
                Value::Uint32(meta.mastering_luminance_max.as_wire_units()),
                Value::Uint32(meta.max_cll.as_wire_units()),
                Value::Uint32(meta.max_fall.as_wire_units()),
            ],
        )
        .expect("set_hdr_metadata");
}

#[test]
fn the_hdr_panel_advertises_its_caps() {
    let tb = start_hdr("caps");
    let mut client = TestClient::connect(&tb.addr);
    let _out = client.bind("ldp.core.output");
    let caps = client.last("hdr_caps");
    // The bitset: static metadata | PQ | wide gamut | tone mapping
    // (bits 0, 1, 3, 4).
    let Value::Bitset(bits) = &caps.args[0] else {
        panic!("hdr caps bitset");
    };
    let word = bits.to_words()[0];
    assert_eq!(word, 0b1_1011, "static | pq | wide | tone mapping");
    // The peak: 600 nits in 1e-4 cd/m2 wire units.
    assert_eq!(caps.args[1], Value::Uint32(600 * 10_000));
    assert_eq!(caps.args[2], Value::Uint32(600 * 10_000));
}

#[test]
fn hdr_content_takes_the_composite_onto_the_pq_canvas() {
    let tb = start_hdr("pq");
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let comp = client.bind("ldp.core.compositor");
    let _out = client.bind("ldp.core.output");
    // The HDR window (PQ color + mastering metadata): it flips the
    // output mode after the dwell. A 2x2 red quad at the origin.
    let hdr_surface = client
        .conn
        .create_object(&comp, "create_surface", vec![])
        .expect("surface");
    set_color(&mut client, &hdr_surface, ColorDescription::pq_hdr());
    set_hdr_metadata(&mut client, &hdr_surface, HdrMetadata::default());
    let red = [0x00FF_0000u32.to_le_bytes(); 4].concat();
    let pool = create_pool(&mut client, &shm, pool_bytes(&red), 16);
    let buffer = create_buffer(&mut client, &pool, 0, 2, 2, 8, 0x3432_5258);
    attach(&mut client, &hdr_surface, &buffer);
    damage(&mut client, &hdr_surface, &[Rect::new(0, 0, 2, 2)]);
    commit(&mut client, &hdr_surface, 0x3301);
    client.wait_until(|c| c.records.iter().any(|r| r.event == "committed"));
    // The dwell settles (two evaluations): drive another frame cycle
    // so the controller's hysteresis completes the switch.
    for n in 2..4u64 {
        frame(&mut client, &hdr_surface, n);
        let pool_n = create_pool(&mut client, &shm, pool_bytes(&red), 16);
        let buffer_n = create_buffer(&mut client, &pool_n, 0, 2, 2, 8, 0x3432_5258);
        attach(&mut client, &hdr_surface, &buffer_n);
        damage(&mut client, &hdr_surface, &[Rect::new(0, 0, 2, 2)]);
        commit(
            &mut client,
            &hdr_surface,
            0x3300 + u32::try_from(n).unwrap(),
        );
        client.wait_until(|c| {
            c.records
                .iter()
                .any(|r| r.event == "committed" && r.args[0] == Value::Uint32(0x3300 + n as u32))
        });
    }
    // The mode settled on PQ; now the SDR-in-HDR proof: a *classic
    // sRGB* window appears on the PQ canvas — its ink is decoded,
    // scaled to the BT.2408 reference white, and PQ-encoded on the
    // tail. Its scanout word is NOT the Phase 25 SDR word.
    let sdr_surface = client
        .conn
        .create_object(&comp, "create_surface", vec![])
        .expect("surface");
    // The legacy default keeps creation positions: place at (2, 0).
    let green = [0x0000_FF00u32.to_le_bytes(); 4].concat();
    let pool_g = create_pool(&mut client, &shm, pool_bytes(&green), 16);
    let buffer_g = create_buffer(&mut client, &pool_g, 0, 2, 2, 8, 0x3432_5258);
    attach(&mut client, &sdr_surface, &buffer_g);
    damage(&mut client, &sdr_surface, &[Rect::new(0, 0, 2, 2)]);
    commit(&mut client, &sdr_surface, 0x3320);
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "committed" && r.args[0] == Value::Uint32(0x3320))
    });
    tb.world(|world| {
        let mode = world
            .hdr
            .as_ref()
            .map(ldp_hdr::policy::ModeController::mode);
        assert_eq!(
            mode,
            Some(ldp_hdr::policy::OutputMode::HdrPq),
            "the dwell settled"
        );
        let scanout = world.scanout_words().expect("lit");
        // The sRGB green window's word: PQ-encoded — different from
        // the Phase 25 SDR bytes (0xFF00_FF00).
        assert_ne!(
            scanout[2], 0xFF00_FF00u32,
            "SDR content on the PQ canvas is re-encoded, not copied"
        );
        // And it is still green-ish: the G channel dominates (the
        // PQ encoding preserves the hue's channel ordering).
        let g = (scanout[2] >> 8) & 0xFF;
        let r = (scanout[2] >> 16) & 0xFF;
        let b = scanout[2] & 0xFF;
        let word = scanout[2];
        assert!(g >= r && g >= b, "green leads: {word:#010x}");
    });
    let _ = buffer;
    let _ = buffer_g;
}

#[test]
fn an_sdr_stack_stays_on_the_srgb_canvas() {
    // `--hdr` with no HDR content: the controller keeps SDR — the
    // Phase 25 bytes (the honest panel mode).
    let tb = start_hdr("sdr-stack");
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let comp = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&comp, "create_surface", vec![])
        .expect("surface");
    let pixels = [0x00FF_0000u32.to_le_bytes(); 4].concat();
    let pool = create_pool(&mut client, &shm, pool_bytes(&pixels), 16);
    let buffer = create_buffer(&mut client, &pool, 0, 2, 2, 8, 0x3432_5258);
    attach(&mut client, &surface, &buffer);
    damage(&mut client, &surface, &[Rect::new(0, 0, 2, 2)]);
    commit(&mut client, &surface, 0x3302);
    client.wait_until(|c| c.records.iter().any(|r| r.event == "committed"));
    tb.world(|world| {
        let scanout = world.scanout_words().expect("lit");
        assert_eq!(scanout[0], 0xFFFF_0000, "SDR content stays SDR-encoded");
    });
    let _ = buffer;
}

#[test]
fn the_default_panel_advertises_nothing() {
    // No `--hdr`: the honest empties (the Phase 25 bytes).
    let tb = Testbench::start("sdr-panel");
    let mut client = TestClient::connect(&tb.addr);
    let _out = client.bind("ldp.core.output");
    let caps = client.last("hdr_caps");
    let Value::Bitset(bits) = &caps.args[0] else {
        panic!("hdr caps bitset");
    };
    assert_eq!(bits.to_words()[0], 0);
    assert_eq!(caps.args[1], Value::Uint32(0));
}

#[test]
fn the_negotiated_peak_clamps_to_the_panel() {
    // The negotiation (ldp-hdr): content brighter than the panel maps
    // down; dimmer content negotiates to itself; SDR keeps the panel.
    use ldp_core::color::Luminance;
    use ldp_hdr::negotiation::negotiated_ceiling;
    let panel = Luminance::from_nits(600);
    assert_eq!(
        negotiated_ceiling(panel, Luminance::from_nits(4000)).as_nits(),
        600
    );
    assert_eq!(
        negotiated_ceiling(panel, Luminance::from_nits(400)).as_nits(),
        400
    );
    assert_eq!(
        negotiated_ceiling(panel, Luminance::from_nits(0)).as_nits(),
        600
    );
    let _ = (Primaries::Bt709, TransferFunction::Pq);
}

// ---- the honest peak (Phase 38) ------------------------------------
//
// The negotiation reaches the ink: the panel's effective peak and the
// content's declared mastering bounds conclude in the canvas's
// luminance ceiling and every HDR layer's tone policy, and the
// scanout carries the rolled ink — not the raw pass-through the
// panel would clip late and abruptly.

/// ST 2084 encode in f64 (the independent model; the tolerance
/// doctrine of the golden suites).
fn encode_pq(y_abs: f64) -> f32 {
    let m1 = 2610.0 / 16384.0;
    let m2 = 2523.0 / 4096.0 * 128.0;
    let c1 = 3424.0 / 4096.0;
    let c2 = 2413.0 / 4096.0 * 32.0;
    let c3 = 2392.0 / 4096.0 * 32.0;
    let yp = y_abs.powf(m1);
    ((c1 + c2 * yp) / (1.0 + c3 * yp)).powf(m2) as f32
}

/// Drive one HDR surface through the dwell (three stack evaluations:
/// the controller's hysteresis completes the SDR→PQ switch), with a
/// full-red PQ quad whose encoded channels sit at code 1.0 — a
/// 10 000-nit code value the metadata declares far below its master
/// peak, exactly the case the negotiation exists for.
fn drive_hdr_window(tag: &str, hdr_peak: Option<u32>, meta: HdrMetadata) -> (Testbench, Vec<u32>) {
    let config = CompositorConfig {
        socket: format!("lion-it-peak-{tag}-{}", std::process::id()),
        hdr: true,
        hdr_peak,
        ..CompositorConfig::default()
    };
    let tb = Testbench::start_with(tag, config);
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let comp = client.bind("ldp.core.compositor");
    let _out = client.bind("ldp.core.output");
    let surface = client
        .conn
        .create_object(&comp, "create_surface", vec![])
        .expect("surface");
    set_color(&mut client, &surface, ColorDescription::pq_hdr());
    set_hdr_metadata(&mut client, &surface, meta);
    let red = [0x00FF_0000u32.to_le_bytes(); 4].concat();
    let mut keep = Vec::new();
    for n in 1..5u64 {
        let pool = create_pool(&mut client, &shm, pool_bytes(&red), 16);
        keep.push(pool);
        let buffer = create_buffer(&mut client, &keep[n as usize - 1], 0, 2, 2, 8, 0x3432_5258);
        attach(&mut client, &surface, &buffer);
        damage(&mut client, &surface, &[Rect::new(0, 0, 2, 2)]);
        let cookie = 0x3300 + u32::try_from(n).unwrap();
        commit(&mut client, &surface, cookie);
        client.wait_until(|c| {
            c.records
                .iter()
                .any(|r| r.event == "committed" && r.args[0] == Value::Uint32(cookie))
        });
    }
    let scanout = {
        let mut words = None;
        tb.world(|world| {
            words = world.scanout_words();
        });
        words.expect("lit")
    };
    (tb, scanout)
}

#[test]
fn the_negotiated_ceiling_caps_the_pq_ink() {
    // The panel advertises 600; the content declares mastering
    // 0.005..1000 with CLL 1000: the negotiated ceiling is 600, and
    // the red quad's 10 000-nit code value rides the EETF — the
    // LUMINANCE lands at the ceiling, the channel at the ceiling's
    // luminance share. The scanout is the rolled ink, not the raw
    // pass-through the panel would clip late.
    let meta = HdrMetadata::default(); // mastering 0.005..1000, CLL 1000
    let (tb, scanout) = drive_hdr_window("negotiate", None, meta);
    tb.world(|world| {
        assert_eq!(world.negotiated, Some(600), "the negotiation recorded");
        let mode = world
            .hdr
            .as_ref()
            .map(ldp_hdr::policy::ModeController::mode);
        assert_eq!(mode, Some(ldp_hdr::policy::OutputMode::HdrPq));
    });
    // Model (f64): the red channel carries the BT.2020 red luminance
    // weight (0.2627) of the 10 000-nit code value → y = 2627 nits;
    // above the declared master peak (1000) the input-clip rule sends
    // it to the master peak, and the EETF maps the master peak onto
    // the display peak (600) — scale 600/2627, the channel at
    // 10 000 × 600/2627 nits, PQ-encoded.
    let y = 0.2627 * 10_000.0;
    let channel_nits = 10_000.0 * (600.0 / y);
    let want = (encode_pq(channel_nits / 10_000.0) * 255.0 + 0.5).floor() as u32;
    let r = (scanout[0] >> 16) & 0xFF;
    let g = (scanout[0] >> 8) & 0xFF;
    let b = scanout[0] & 0xFF;
    assert!(
        r.abs_diff(want) <= 3,
        "the rolled red channel: {r} want ~{want} (the EETF's ink)"
    );
    let word = scanout[0];
    assert!(g == 0 && b == 0, "the hue survives: {word:#010x}");
    assert!(r < 255, "not the raw pass-through (the pre-Phase-38 byte)");
}

#[test]
fn dimmer_content_negotiates_to_itself() {
    // The content declares mastering 0.005..400 (CLL 400): the
    // negotiated ceiling is 400 — dimmer content negotiates to itself
    // — and the ink rides at the content's own level (the 10 000-nit
    // code value input-clips at the declared master peak; the display
    // covers the master, the EETF is the identity there).
    let meta = HdrMetadata {
        mastering_luminance_max: ldp_core::color::Luminance::from_nits(400),
        max_cll: ldp_core::color::Luminance::from_nits(400),
        ..HdrMetadata::default()
    };
    let (tb, scanout) = drive_hdr_window("dimmer", None, meta);
    tb.world(|world| {
        assert_eq!(
            world.negotiated,
            Some(400),
            "dimmer content negotiates to itself"
        );
    });
    // Model: the code value input-clips at the 400-nit master peak;
    // scale 400/2627; the channel at 10 000 × 400/2627 nits.
    let y = 0.2627 * 10_000.0;
    let channel_nits = 10_000.0 * (400.0 / y);
    let want = (encode_pq(channel_nits / 10_000.0) * 255.0 + 0.5).floor() as u32;
    let r = (scanout[0] >> 16) & 0xFF;
    assert!(
        r.abs_diff(want) <= 3,
        "the content's own level: {r} want ~{want}"
    );
    let g = (scanout[0] >> 8) & 0xFF;
    let b = scanout[0] & 0xFF;
    let word = scanout[0];
    assert!(g == 0 && b == 0, "the hue survives: {word:#010x}");
}

#[test]
fn the_operator_cap_reaches_the_ink() {
    // The bloated-peak quirk's escape: `--hdr-peak 400` caps the
    // 600-nit advertisement — hdr_caps replies with the effective
    // peak, and the ink tone-maps against 400 whatever the content
    // declares (the quirk row's route, proven end to end).
    let meta = HdrMetadata::default(); // declares 1000: brighter than either cap
    let (tb, scanout) = drive_hdr_window("operator-cap", Some(400), meta);
    // The negotiation's reply side: the advertisement carries the
    // effective peak.
    tb.world(|world| {
        let caps = world
            .outputs
            .first()
            .and_then(|slot| slot.output.hdr)
            .expect("the forced pipeline");
        assert_eq!(caps.max_luminance.as_nits(), 400, "the operator cap");
        assert_eq!(
            world.negotiated,
            Some(400),
            "the negotiation sees the effective panel"
        );
    });
    // Model: the master peak (1000) maps onto the display peak (400);
    // scale 400/2627; the channel at 10 000 × 400/2627 nits — the
    // same honest ink the dimmer-content case serves.
    let y = 0.2627 * 10_000.0;
    let channel_nits = 10_000.0 * (400.0 / y);
    let want = (encode_pq(channel_nits / 10_000.0) * 255.0 + 0.5).floor() as u32;
    let r = (scanout[0] >> 16) & 0xFF;
    assert!(
        r.abs_diff(want) <= 3,
        "the operator's truth reaches the ink: {r} want ~{want}"
    );
}

#[test]
fn the_operator_peak_advertises_the_effective_peak() {
    // `--hdr-peak 400` alone (the wire side): hdr_caps carries the
    // effective peak both directions — what clients can rely on is
    // what the render pass will actually deliver.
    let config = CompositorConfig {
        socket: format!("lion-it-peak-reply-{}", std::process::id()),
        hdr: true,
        hdr_peak: Some(400),
        ..CompositorConfig::default()
    };
    let tb = Testbench::start_with("peak-reply", config);
    let mut client = TestClient::connect(&tb.addr);
    let _out = client.bind("ldp.core.output");
    let caps = client.last("hdr_caps");
    assert_eq!(
        caps.args[1],
        Value::Uint32(400 * 10_000),
        "the effective peak"
    );
    assert_eq!(caps.args[2], Value::Uint32(400 * 10_000));
}
