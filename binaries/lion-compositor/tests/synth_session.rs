//! Phase 42's exit criteria: the mode foundry (`--synth WxH[@HZ]`),
//! end to end through the real bring-up, the real protocol, and the
//! real mock engine's commit validation.
//!
//! * **THE pour criterion** — a size no display offers *serves*: the
//!   foundry pours the reduced-blanking timing, the engine's `USERDEF`
//!   gate passes the commit, the client binds and hears the poured
//!   geometry (the mode advertised with the current flag), and the
//!   desktop renders onto it — the scanout carries pixels at exactly
//!   the synthesized raster.
//! * **THE refresh criterion** — the pour serves a refresh the offered
//!   modes of that size lack (1080p at 144 Hz on a panel whose list
//!   carries only 60 and 59.94).
//! * **THE ceiling criterion** — a pour above the panel's declared
//!   pixel-clock ceiling refuses bring-up with the typed error naming
//!   the declaration (never a silent clamp, never a truncation).
//! * **THE audit criterion** — the default desktop's EDID audit
//!   diagnoses the stale-firmware fixture by name (the
//!   `edid-preferred-lie` finding) while the honest panel stays clean.
//! * **THE migration criterion** — a hotplug swap re-pours on the next
//!   display (the pour rides every selection, not just bring-up), and
//!   a connector whose declared ceiling refuses falls back to the
//!   unsized doctrine (the migration never dies on the operator's
//!   preference — the sized doctrine's honesty, verbatim).

mod testbench;

use std::io::Write;

use ldp_client::Proxy;
use ldp_core::geometry::Rect;
use ldp_core::wire::Value;
use testbench::*;

use lion_compositor::rearrange::Rearrange;
use lion_compositor::server::{Compositor, CompositorConfig};
use lion_compositor::World;

/// The connectors of the mock preset.
const EDP: u32 = 91;
const HDMI: u32 = 93;

fn connector(id: u32) -> ldp_display::ids::ConnectorId {
    ldp_display::ids::ConnectorId::new(id).expect("preset connector id")
}

/// Recorded events of one name.
fn events_of<'a>(c: &'a Collector, name: &str) -> Vec<&'a Recorded> {
    c.records.iter().filter(|r| r.event == name).collect()
}

/// A memfd pool carrying explicit pixel bytes.
fn pool_bytes(pixels: &[u8]) -> std::os::fd::OwnedFd {
    let fd = lion_compositor::sys::memfd("synth").expect("memfd");
    let mut file = std::fs::File::from(fd);
    file.write_all(pixels).expect("write pixels");
    file.into()
}

/// 2x2 XRGB8888 (stride 8): red, green / blue, white.
fn quad_pixels() -> Vec<u8> {
    let mut v = Vec::new();
    for word in [0x00FF_0000u32, 0x0000_FF00, 0x0000_00FF, 0x00FF_FFFF] {
        v.extend_from_slice(&word.to_le_bytes());
    }
    v
}

/// Drive one presentation cycle: frame request, pool, buffer, attach,
/// damage, commit, presented.
fn present_one_frame(
    client: &mut TestClient,
    shm: &Proxy,
    surface: &Proxy,
    frame_id: u64,
    cookie: u32,
) {
    frame(client, surface, frame_id);
    let pool = create_pool(client, shm, pool_bytes(&quad_pixels()), 16);
    let buffer = create_buffer(client, &pool, 0, 2, 2, 8, 0x3432_5258);
    attach(client, surface, &buffer);
    damage(client, surface, &[Rect::new(0, 0, 2, 2)]);
    commit(client, surface, cookie);
    client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "committed" && r.args[0] == Value::Uint32(cookie))
    });
    client.wait_until(|c| c.records.iter().any(|r| r.event == "presented"));
}

/// Inject a topology change and service it once — the summary.
fn service_hotplug(tb: &Testbench) -> Rearrange {
    let mut world = tb.shared.world.lock().expect("world lock");
    world
        .service_device_events()
        .expect("the service pass")
        .expect("a hotplug event was queued")
}

/// Unplug one connector (queues the event; the service pass acts).
fn unplug(world: &mut World, id: u32) {
    world
        .device
        .as_mock_mut()
        .expect("the mock driver")
        .hotplug_disconnect(connector(id));
}

fn unique_config(synth: Option<ldp_display::timing::SynthRequest>) -> CompositorConfig {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    CompositorConfig {
        socket: format!("lion-it-synth-{n}-{}", std::process::id()),
        synth,
        ..CompositorConfig::default()
    }
}
/// THE pour criterion: `--synth 2560x1440` on the preset panel (whose
/// list offers 1920x1080 and 1280x720 only) serves the synthesized
/// raster — the VESA reduced-blanking arithmetic of the timing
/// module's own anchor: htotal 2720, vtotal 1481, 241 700 kHz, a
/// user-defined mode on the wire. The protocol advertises the pour
/// (the mode list's current flag), and the desktop presents onto it.
#[test]
fn pour_serves_a_size_the_list_lacks() {
    let config = unique_config(Some(ldp_display::timing::SynthRequest {
        width: 2560,
        height: 1440,
        refresh_millihz: 60_000,
        family: ldp_display::timing::TimingFamily::Rb,
    }));
    let tb = Testbench::start_with("synth-pour", config);

    // The world's truth: the poured mode is active — the exact
    // reduced-blanking timing, marked user-defined.
    tb.world(|world| {
        let mode = world.output().expect("lit").mode();
        assert_eq!(
            (u32::from(mode.hdisplay), u32::from(mode.vdisplay)),
            (2560, 1440)
        );
        assert_eq!(mode.htotal, 2720, "the RB horizontal total");
        assert_eq!(mode.vtotal, 1481, "the RB vertical total");
        assert_eq!(mode.clock_khz, 241_700, "the exact-target clock");
        assert_eq!(mode.refresh_hz(), 60);
        assert!(
            mode.kind.0 & ldp_display::mode::ModeType::USERDEF.0 != 0,
            "the pour is a user-defined mode on the wire"
        );
        // The protocol face adopted the pour: it is IN the advertised
        // list (the xrandr --addmode story, protocol-side).
        assert!(world
            .output()
            .expect("lit")
            .connector
            .modes
            .iter()
            .any(|m| m == mode));
    });

    // The protocol truth: a bound client hears the poured geometry —
    // the mode list carries the synthesized size, flagged current.
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let compositor = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("create_surface");
    let _output = client.bind("ldp.core.output");
    client.wait_until(|c| !events_of(c, "name").is_empty());
    client.wait_until(|c| !events_of(c, "mode").is_empty());
    let modes = client.events_of("mode");
    let current: Vec<&Recorded> = modes
        .iter()
        .copied()
        .filter(|r| matches!(&r.args[1], Value::Bitset(b) if b.test(1)))
        .collect();
    assert_eq!(current.len(), 1, "exactly one flagged current mode");
    assert_eq!(current[0].args[2], Value::Uint32(2560), "the poured width");
    assert_eq!(current[0].args[3], Value::Uint32(1440), "the poured height");
    assert_eq!(
        current[0].args[4],
        Value::Uint32(60_000),
        "the poured refresh"
    );
    // The advertised list carries the pour among the firmware's modes.
    assert!(
        modes
            .iter()
            .any(|r| r.args[2] == Value::Uint32(2560) && r.args[3] == Value::Uint32(1440)),
        "the pour joins the advertised mode list"
    );

    // The desktop presents on the poured raster — the engine's USERDEF
    // gate passed the enable commit (the mock's declared envelope).
    present_one_frame(&mut client, &shm, &surface, 1, 0x5401);
    let scanout = tb.scanout();
    assert_eq!(scanout.len(), 2560 * 1440);
    assert_eq!(scanout[0], 0xFFFF_0000, "red, opaque");
    assert_eq!(scanout[1], 0xFF00_FF00, "green, opaque");
    assert_eq!(scanout[2560], 0xFF00_00FF, "blue, opaque");
    assert_eq!(scanout[2561], 0xFFFF_FFFF, "white, opaque");
    assert_eq!(scanout[2560 * 1439 + 2559], 0xFF00_0000, "black desktop");
}

/// THE refresh criterion: the foundry serves a refresh the offered
/// modes of that size lack — 1080p at 144 Hz on a panel whose list
/// carries 1080p60 and 1080p59.94 only. The pour's clock (346 545 kHz
/// over 2080x1157) lands at 144 Hz exactly under the kernel's rounded
/// convention.
#[test]
fn pour_serves_a_refresh_the_list_lacks() {
    let config = unique_config(Some(ldp_display::timing::SynthRequest {
        width: 1920,
        height: 1080,
        refresh_millihz: 144_000,
        family: ldp_display::timing::TimingFamily::Rb,
    }));
    let tb = Testbench::start_with("synth-refresh", config);
    tb.world(|world| {
        let mode = world.output().expect("lit").mode();
        assert_eq!(
            (u32::from(mode.hdisplay), u32::from(mode.vdisplay)),
            (1920, 1080)
        );
        assert_eq!(mode.clock_khz, 346_545);
        assert_eq!(mode.refresh_hz(), 144, "the 144 Hz the list never carried");
        assert!(mode.refresh_millihz() >= 144_000, "never under the ask");
        // The 60 Hz sibling stays in the list, unselected.
        assert!(world
            .output()
            .expect("lit")
            .connector
            .modes
            .iter()
            .any(|m| m.refresh_hz() == 60));
    });
    // The scheduler's nominal is the pour's own period.
    tb.world(|world| {
        assert_eq!(world.output().expect("lit").refresh().as_ns(), 6_944_444);
    });
}

/// THE ceiling criterion: a pour above the panel's declared
/// pixel-clock ceiling refuses bring-up with the typed error naming
/// the declaration. An 8K RB pour clocks ~2 089 988 kHz — above the
/// eDP panel's declared 600 MHz envelope (and the HDMI's 340 MHz).
#[test]
fn pour_above_the_declared_ceiling_refuses_honestly() {
    let config = unique_config(Some(ldp_display::timing::SynthRequest {
        width: 7680,
        height: 4320,
        refresh_millihz: 60_000,
        family: ldp_display::timing::TimingFamily::Rb,
    }));
    let Err(err) = Compositor::headless(config) else {
        panic!("the above-ceiling pour must fail bring-up")
    };
    let text = err.to_string();
    assert!(text.contains("7680x4320"), "names the ask: {text}");
    assert!(
        text.contains("600000 kHz"),
        "names the declared ceiling: {text}"
    );
    assert!(
        text.contains("2089988 kHz"),
        "names the pour's clock, never a silent clamp: {text}"
    );
}

/// THE audit criterion: the default desktop's EDID audit names the
/// stale-firmware fixture — the HDMI monitor's EDID prefers 720p while
/// its connector enumerates 1080p (the `edid-preferred-lie` finding)
/// — and the honest eDP panel stays clean (no findings). The audit
/// rides the *served* outputs (the multi-output desktop serves both
/// connectors).
#[test]
fn the_audit_diagnoses_the_stale_firmware_by_name() {
    let mut config = unique_config(None);
    config.multi_output = true;
    let tb = Testbench::start_with("synth-audit", config);
    tb.world(|world| {
        let findings: Vec<(String, String)> = world
            .outputs
            .iter()
            .flat_map(|slot| {
                slot.output
                    .edid_findings
                    .iter()
                    .map(move |f| (slot.output.name.clone(), f.to_string()))
            })
            .collect();
        assert_eq!(findings.len(), 1, "the HDMI fixture alone: {findings:?}");
        assert!(findings[0].0.contains("HDMI"), "on the HDMI monitor");
        assert!(
            findings[0].1.contains("edid-preferred-lie"),
            "the named finding: {}",
            findings[0].1
        );
        assert!(
            findings[0].1.contains("1280x720") && findings[0].1.contains("1920x1080"),
            "the lie's numbers: {}",
            findings[0].1
        );
        // The eDP panel (the first output) audits clean.
        assert!(world.outputs[0].output.edid_findings.is_empty());
    });
}

/// THE migration criterion: the pour rides every selection — a hotplug
/// swap re-pours on the next display (eDP dies, HDMI pours the same
/// synthesized raster), and a connector whose *declared ceiling*
/// refuses the pour falls back to the unsized doctrine (the
/// migration never dies on the operator's preference).
#[test]
fn migration_re_pours_and_falls_back_past_the_ceiling() {
    let config = unique_config(Some(ldp_display::timing::SynthRequest {
        width: 2560,
        height: 1440,
        refresh_millihz: 60_000,
        family: ldp_display::timing::TimingFamily::Rb,
    }));
    let tb = Testbench::start_with("synth-migrate", config);

    // The cable pulls: eDP dies. HDMI is next in line — the migration
    // re-pours on it (the 241 700 kHz ask sits under its declared
    // 340 MHz ceiling).
    {
        let mut world = tb.shared.world.lock().expect("world lock");
        unplug(&mut world, EDP);
    }
    match service_hotplug(&tb) {
        Rearrange::Migrated { from, to } => {
            assert_eq!(from.name, "eDP-1");
            assert_eq!(to.name, "HDMI-A-1");
            assert_eq!(
                (to.width, to.height),
                (2560, 1440),
                "the pour rides the migration"
            );
        }
        other => panic!("expected a migration, got {other:?}"),
    }
    tb.world(|world| {
        let mode = world.output().expect("lit").mode();
        assert_eq!(
            (u32::from(mode.hdisplay), u32::from(mode.vdisplay)),
            (2560, 1440)
        );
        assert!(mode.kind.0 & ldp_display::mode::ModeType::USERDEF.0 != 0);
    });

    // HDMI re-negotiates to a tightly-capped envelope: the EDID now
    // declares a 120 MHz ceiling — the pour (241 700 kHz) must be
    // refused for THIS connector, and the migration falls back to the
    // unsized doctrine (HDMI's preferred 1080p), never dying on the
    // operator's preference.
    {
        let mut world = tb.shared.world.lock().expect("world lock");
        let capped_edid = ldp_display::edid::EdidIdentity::synthesize_detailed(
            "BNQ",
            0x7D12,
            0x5566_7788,
            "Lion External",
            2025,
            None,
            Some(ldp_display::edid::RangeLimits {
                vrate_hz: (56, 75),
                hrate_khz: (15, 83),
                max_clock_khz: 120_000,
            }),
        );
        world
            .device
            .as_mock_mut()
            .expect("the mock driver")
            .hotplug_connect(
                connector(HDMI),
                vec![
                    ldp_display::mode::Mode::panel_1080p60(),
                    ldp_display::mode::Mode::panel_1080p_59_94(),
                ],
                capped_edid.to_vec(),
            );
    }
    match service_hotplug(&tb) {
        Rearrange::Migrated { to, .. } => {
            assert_eq!(
                (to.width, to.height),
                (1920, 1080),
                "the honest fallback past the ceiling"
            );
        }
        other => panic!("expected a mode migration, got {other:?}"),
    }
    tb.world(|world| {
        let mode = world.output().expect("lit").mode();
        assert_eq!(
            (u32::from(mode.hdisplay), u32::from(mode.vdisplay)),
            (1920, 1080)
        );
        // The world remembers the operator's pour for the next display
        // whose ceiling allows it.
        assert_eq!(
            world.synth,
            Some(ldp_display::timing::SynthRequest {
                width: 2560,
                height: 1440,
                refresh_millihz: 60_000,
                family: ldp_display::timing::TimingFamily::Rb,
            })
        );
    });
}

/// THE family criterion (Phase 46 — the foundry completes): every
/// VESA family pours end to end through the same bring-up — the
/// RB2 ask serves the §3.4.3 raster (2000×1111, 133 320 kHz — the
/// deep-color-era economy, RB's own vertical total with the
/// 80-pixel blank), the CVT standard-CRT ask serves the analog
/// raster (2576×1120, 173 108 kHz), and the GTF ask serves its own
/// (2576×1118, 172 799 kHz) — each a user-defined mode the engine's
/// commit gate passes, each advertised to a bound client, and the
/// GTF desktop *presents* onto the poured raster (the pixel truth).
#[test]
fn every_family_pours_end_to_end() {
    use ldp_display::timing::TimingFamily;
    for (name, family, htotal, vtotal, clock_khz) in [
        ("rb2", TimingFamily::Rb2, 2000, 1111, 133_320),
        ("cvt", TimingFamily::Cvt, 2576, 1120, 173_108),
        ("gtf", TimingFamily::Gtf, 2576, 1118, 172_799),
    ] {
        let config = unique_config(Some(ldp_display::timing::SynthRequest {
            width: 1920,
            height: 1080,
            refresh_millihz: 60_000,
            family,
        }));
        let tb = Testbench::start_with(&format!("synth-family-{name}"), config);

        // The world's truth: the family's own raster is active.
        tb.world(|world| {
            let mode = world.output().expect("lit").mode();
            assert_eq!(
                (u32::from(mode.hdisplay), u32::from(mode.vdisplay)),
                (1920, 1080),
                "{name}: the asked geometry"
            );
            assert_eq!(mode.htotal, htotal, "{name}: the family's htotal");
            assert_eq!(mode.vtotal, vtotal, "{name}: the family's vtotal");
            assert_eq!(mode.clock_khz, clock_khz, "{name}: the exact clock");
            assert_eq!(mode.refresh_hz(), 60, "{name}: never under the ask");
            assert!(
                mode.kind.0 & ldp_display::mode::ModeType::USERDEF.0 != 0,
                "{name}: a user-defined mode on the wire"
            );
        });

        // The protocol truth: a bound client hears the family's pour.
        let mut client = TestClient::connect(&tb.addr);
        let shm = client.bind("ldp.core.shm");
        let compositor = client.bind("ldp.core.compositor");
        let surface = client
            .conn
            .create_object(&compositor, "create_surface", vec![])
            .expect("create_surface");
        let _output = client.bind("ldp.core.output");
        client.wait_until(|c| !events_of(c, "mode").is_empty());
        let modes = client.events_of("mode");
        assert!(
            modes.iter().any(|r| r.args[2] == Value::Uint32(1920)
                && r.args[3] == Value::Uint32(1080)
                && r.args[4] == Value::Uint32(60_000)),
            "{name}: the pour joins the advertised mode list"
        );

        // The desktop truth — one presentation cycle on the analog
        // raster (the engine's USERDEF gate passed the commit): the
        // scanout carries pixels at exactly 1920×1080.
        if name == "gtf" {
            present_one_frame(&mut client, &shm, &surface, 1, 0x6401);
            let scanout = tb.scanout();
            assert_eq!(scanout.len(), 1920 * 1080);
            assert_eq!(scanout[0], 0xFFFF_0000, "red, opaque");
            assert_eq!(scanout[1920 * 1079 + 1919], 0xFF00_0000, "black desktop");
        }
    }
}

/// THE video-optimized criterion (Phase 46): the `:rb2v` multiplier
/// pours RB2's geometry with the 1000/1001 clock — the 59.94 Hz
/// class served over the real socket, the panel's advertised refresh
/// the video rate itself.
#[test]
fn the_video_optimized_rate_serves() {
    use ldp_display::timing::TimingFamily;
    let config = unique_config(Some(ldp_display::timing::SynthRequest {
        width: 1920,
        height: 1080,
        refresh_millihz: 60_000,
        family: TimingFamily::Rb2Video,
    }));
    let tb = Testbench::start_with("synth-family-rb2v", config);
    tb.world(|world| {
        let mode = world.output().expect("lit").mode();
        assert_eq!((mode.htotal, mode.vtotal), (2000, 1111), "RB2's own raster");
        assert_eq!(mode.clock_khz, 133_187, "the 1000/1001 clock");
        // 133 187 kHz over 2000×1111 = 59 940.06 mHz — the video
        // rate, never under the ×1000/1001 target.
        assert_eq!(mode.refresh_millihz(), 59_940);
    });
}
