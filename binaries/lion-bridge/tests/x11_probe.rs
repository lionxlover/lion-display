//! Scratch diagnostic: the X11 driver's bootstrap message shapes.
#![allow(dead_code)]

mod common;

#[test]
fn probe_x11_bootstrap() {
    use ldp_x11_bridge::driver::XBridgeDriver;
    struct AllowAll;
    impl ldp_x11_bridge::driver::TokenCheck for AllowAll {
        fn check(&mut self, _: &str, _: [u32; 8]) -> bool {
            true
        }
    }
    let mut driver = XBridgeDriver::new(ldp_x11_bridge::setup::ScreenParams::default());
    driver
        .authenticate("lion-bridge", [1, 2, 3, 4, 5, 6, 7, 8], &mut AllowAll)
        .expect("auth");
    let msgs = driver.take_ldp_messages();
    println!("bootstrap: {} messages", msgs.len());
    for m in &msgs {
        let carries = m
            .args
            .iter()
            .any(|a| matches!(a, ldp_core::wire::Value::Fd(_)));
        println!(
            "  obj={} op={} fd={} args={:?}",
            m.object_id, m.opcode, carries, m.args
        );
    }
    // Send against the live server.
    let tb = common::Testbench::start("x11-probe");
    let fd = lion_bridge::sys::memfd(3_145_728).expect("memfd");
    let mut link =
        lion_bridge::ldp::LdpLink::connect(&tb.addr, &msgs, Some(fd.try_clone().expect("dup")))
            .expect("connect");
    println!("connect survived");
    for _ in 0..30 {
        match link.recv_one() {
            Ok(Some(ev)) => println!("event: {ev:?}"),
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(10)),
            Err(e) => {
                println!("recv err: {e} death={:?}", link.death());
                break;
            }
        }
    }
}
