//! Pixel truth: what the client draws is what the display scans out —
//! subsurface stacking, premultiplied blending, incremental damage,
//! and detach/unmap repaint.

mod testbench;

use std::io::Write;

use ldp_core::geometry::Rect;
use ldp_core::wire::Value;
use testbench::*;

/// A memfd pool with explicit content.
fn pool_bytes(pixels: &[u8]) -> std::os::fd::OwnedFd {
    let fd = lion_compositor::sys::memfd("pixels").expect("memfd");
    let mut file = std::fs::File::from(fd);
    file.write_all(pixels).expect("write pixels");
    file.into()
}

struct Rig {
    #[allow(dead_code)]
    tb: Testbench,
    client: TestClient,
    surface: ldp_client::Proxy,
    compositor: ldp_client::Proxy,
    #[allow(dead_code)]
    pool: ldp_client::Proxy,
}

fn rig(tag: &str, first_pixels: &[u8], format: u32) -> Rig {
    let tb = Testbench::start(tag);
    let mut client = TestClient::connect(&tb.addr);
    let shm = client.bind("ldp.core.shm");
    let pool = create_pool(
        &mut client,
        &shm,
        pool_bytes(first_pixels),
        first_pixels.len() as i64,
    );
    let buffer = create_buffer(&mut client, &pool, 0, 2, 2, 8, format);
    let compositor = client.bind("ldp.core.compositor");
    let surface = client
        .conn
        .create_object(&compositor, "create_surface", vec![])
        .expect("surface");
    client.bind("ldp.core.output");
    attach(&mut client, &surface, &buffer);
    damage(&mut client, &surface, &[Rect::new(0, 0, 2, 2)]);
    commit(&mut client, &surface, 1);
    client.wait_until(|c| c.records.iter().any(|r| r.event == "committed"));
    Rig {
        tb,
        client,
        surface,
        compositor,
        pool,
    }
}

/// Opaque red / green / blue / white quad as XRGB words.
fn quad() -> Vec<u8> {
    let mut v = Vec::new();
    for word in [0x00FF_0000u32, 0x0000_FF00, 0x0000_00FF, 0x00FF_FFFF] {
        v.extend_from_slice(&word.to_le_bytes());
    }
    v
}

#[test]
fn subsurface_draws_above_its_parent() {
    let mut rig = rig("subsurface", &quad(), 0x3432_5258);
    // A second surface becomes a subsurface of the first; its 1x1
    // green pixel lands at (1,1) over the parent's quad.
    let child = rig
        .client
        .conn
        .create_object(&rig.compositor, "create_surface", vec![])
        .expect("child surface");
    let shm = rig.client.bind("ldp.core.shm");
    let green_pool = create_pool(
        &mut rig.client,
        &shm,
        pool_bytes(&0x0000_FF00u32.to_le_bytes()),
        4,
    );
    let green = create_buffer(&mut rig.client, &green_pool, 0, 1, 1, 4, 0x3432_5258);
    let role = rig
        .client
        .conn
        .create_object(
            &rig.compositor,
            "create_subsurface",
            vec![
                Value::Object(Some(child.id())),
                Value::Object(Some(rig.surface.id())),
            ],
        )
        .expect("subsurface role");

    // Sync mode (default): child state applies at the parent's commit.
    attach(&mut rig.client, &child, &green);
    damage(&mut rig.client, &child, &[Rect::new(0, 0, 1, 1)]);
    commit(&mut rig.client, &child, 2); // stashed until the parent commits
    rig.client
        .conn
        .send_request(
            &role,
            "set_position",
            vec![Value::Int32(1), Value::Int32(1)],
        )
        .expect("set_position");
    commit(&mut rig.client, &rig.surface, 3);
    rig.client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "committed" && r.args[0] == Value::Uint32(3))
    });

    let scanout = rig.tb.scanout();
    // The child's green pixel covers the parent's white at (1,1).
    assert_eq!(scanout[0], 0xFFFF_0000);
    assert_eq!(scanout[1], 0xFF00_FF00);
    assert_eq!(scanout[1920], 0xFF00_00FF);
    assert_eq!(scanout[1921], 0xFF00_FF00, "the subsurface draws above");
}

#[test]
fn premultiplied_argb_blends_over_the_opaque_parent() {
    // Parent: opaque red quad. Child subsurface: 2x2 ARGB at 50%
    // premultiplied green — the blend composites over the parent's
    // live pixels (buffer swaps re-fold from the background; overlays
    // blend over what is below).
    let parent = quad();
    let mut child = Vec::new();
    for _ in 0..4 {
        child.extend_from_slice(&0x8000_8000u32.to_le_bytes()); // a=128, g=128 premul
    }
    let mut rig = rig("argb", &parent, 0x3432_5258);
    let shm = rig.client.bind("ldp.core.shm");
    let pool2 = create_pool(&mut rig.client, &shm, pool_bytes(&child), 16);
    let overlay = create_buffer(&mut rig.client, &pool2, 0, 2, 2, 8, 0x3432_5241);
    let child_surface = rig
        .client
        .conn
        .create_object(&rig.compositor, "create_surface", vec![])
        .expect("child surface");
    let role = rig
        .client
        .conn
        .create_object(
            &rig.compositor,
            "create_subsurface",
            vec![
                Value::Object(Some(child_surface.id())),
                Value::Object(Some(rig.surface.id())),
            ],
        )
        .expect("subsurface role");
    let _ = role;
    // Sync mode: the child's state applies at the parent's commit.
    attach(&mut rig.client, &child_surface, &overlay);
    damage(&mut rig.client, &child_surface, &[Rect::new(0, 0, 2, 2)]);
    commit(&mut rig.client, &child_surface, 2); // stashed
    commit(&mut rig.client, &rig.surface, 3); // applies the cascade
    rig.client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "committed" && r.args[0] == Value::Uint32(3))
    });
    let scanout = rig.tb.scanout();
    // Over red: r = 0 + 255*127/255 = 127, g = 128 → 0xFF7F8000.
    // Over green: g = 128 + 255*127/255 = 255 → the alpha saturates.
    assert_eq!(scanout[0], 0xFF7F_8000, "50% premultiplied green over red");
    assert_eq!(
        scanout[1], 0xFF00_FF00,
        "over green the same layer saturates"
    );
    // Rows below the quad are untouched background.
    assert_eq!(scanout[1920 * 2], 0xFF00_0000);
}

#[test]
fn incremental_damage_updates_one_pixel() {
    let mut rig = rig("damage", &quad(), 0x3432_5258);
    // A second pool whose bytes the test rewrites through a kept
    // duplicate handle: 1x1 damage re-composites just that pixel.
    let fd = lion_compositor::sys::memfd("damage-write").expect("memfd");
    let writer = fd.try_clone().expect("duplicate pool handle");
    {
        use std::io::Write;
        let mut file = std::fs::File::from(writer.try_clone().unwrap());
        file.write_all(&quad()).unwrap();
    }
    let shm = rig.client.bind("ldp.core.shm");
    let pool2 = create_pool(&mut rig.client, &shm, fd, 16);
    let buffer2 = create_buffer(&mut rig.client, &pool2, 0, 2, 2, 8, 0x3432_5258);
    // Write yellow at pixel 0 through the duplicate handle.
    {
        use std::io::{Seek, SeekFrom, Write};
        let mut file = std::fs::File::from(writer);
        file.seek(SeekFrom::Start(0)).unwrap();
        file.write_all(&0x00FF_FF00u32.to_le_bytes()).unwrap();
    }
    attach(&mut rig.client, &rig.surface, &buffer2);
    damage(&mut rig.client, &rig.surface, &[Rect::new(0, 0, 1, 1)]);
    commit(&mut rig.client, &rig.surface, 2);
    rig.client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "committed" && r.args[0] == Value::Uint32(2))
    });
    let scanout = rig.tb.scanout();
    assert_eq!(scanout[0], 0xFFFF_FF00, "only the damaged pixel changed");
    assert_eq!(scanout[1], 0xFF00_FF00, "the undamaged pixel persists");
    assert_eq!(scanout[1920], 0xFF00_00FF);
}

#[test]
fn detaching_repaints_the_desktop_behind() {
    let mut rig = rig("detach", &quad(), 0x3432_5258);
    assert_eq!(rig.tb.scanout()[0], 0xFFFF_0000);
    rig.client
        .conn
        .send_request(&rig.surface, "attach", vec![Value::Object(None)])
        .expect("detach");
    damage(&mut rig.client, &rig.surface, &[Rect::new(0, 0, 2, 2)]);
    commit(&mut rig.client, &rig.surface, 2);
    rig.client.wait_until(|c| {
        c.records
            .iter()
            .any(|r| r.event == "committed" && r.args[0] == Value::Uint32(2))
    });
    // The pump after the next wake repaints; poll the scanout.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if rig.tb.scanout()[0] == 0xFF00_0000 {
            break;
        }
        rig.client.sync();
    }
    assert_eq!(rig.tb.scanout()[0], 0xFF00_0000, "the desktop is back");
    assert_eq!(rig.tb.scanout()[1921], 0xFF00_0000);
    // The detached buffer was released with a signalled fence.
    rig.client
        .wait_until(|c| c.records.iter().any(|r| r.event == "release"));
}
