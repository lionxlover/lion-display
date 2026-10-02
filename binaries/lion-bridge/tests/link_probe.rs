//! Scratch diagnostic (not a gate): the link's bootstrap against the
//! live server, printing every inbound event.
#![allow(dead_code)]

mod common;

#[test]
fn probe_the_bootstrap() {
    let tb = common::Testbench::start("link-probe");
    use ldp_wayland_bridge::dispatch::Host;
    use ldp_wayland_bridge::driver::WlBridgeDriver;
    struct NHost;
    impl Host for NHost {
        fn read_pool(&mut self, _: u32, _: usize, _: usize) -> Option<Vec<u8>> {
            None
        }
        fn keymap_fd(&mut self) -> u32 {
            0
        }
        fn keymap(&mut self) -> Vec<u8> {
            Vec::new()
        }
    }
    struct AllowAll;
    impl ldp_wayland_bridge::driver::TokenCheck for AllowAll {
        fn check(&mut self, _: &str, _: [u32; 8]) -> bool {
            true
        }
    }
    let mut driver = WlBridgeDriver::new(Box::new(NHost));
    driver
        .authenticate("lion-bridge", [1, 2, 3, 4, 5, 6, 7, 8], &mut AllowAll)
        .expect("auth");
    let bootstrap = driver.take_ldp_messages();
    println!("bootstrap: {} messages", bootstrap.len());
    for m in &bootstrap {
        println!("  obj={} op={} args={:?}", m.object_id, m.opcode, m.args);
    }
    let mut link = lion_bridge::ldp::LdpLink::connect(&tb.addr, &bootstrap, None).expect("connect");
    let factories = link.seat_factories();
    println!("factories: {} messages", factories.len());
    match link.send(&factories, &mut ldp_transport::fd::FdList::new()) {
        Ok(()) => println!("sent"),
        Err(e) => println!("send failed: {e}"),
    }
    for _ in 0..50 {
        match link.recv_one() {
            Ok(Some(ev)) => println!("event: {ev:?}"),
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
            Err(e) => {
                println!("recv error: {e}");
                break;
            }
        }
    }
    println!("welcomed: {}", link.is_welcomed());

    // ---- bisect: replay the export sequence message by message ----
    use ldp_core::wire::Value;
    use ldp_protocol::generated::core;
    use ldp_protocol::generated::shell;
    use ldp_protocol::Message;
    let oid = |raw: u32| ldp_core::ids::ObjectId::client(raw).expect("id");
    let msgs: Vec<Message> =
        vec![Message::new(3, core::compositor::request::CREATE_SURFACE).arg(Value::NewId(oid(10)))];
    for m in &msgs {
        println!(">> send obj={} op={}", m.object_id, m.opcode);
        match link.send(
            std::slice::from_ref(m),
            &mut ldp_transport::fd::FdList::new(),
        ) {
            Ok(()) => println!("   sent"),
            Err(e) => println!("   send err: {e}"),
        }
        for _ in 0..10 {
            match link.recv_one() {
                Ok(Some(ev)) => println!("   event: {ev:?}"),
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(10)),
                Err(e) => {
                    println!("   RECV ERR: {e} (death: {:?})", link.death());
                    break;
                }
            }
        }
    }
    // The pool with a real memfd.
    let fd = lion_bridge::sys::memfd(16).expect("memfd");
    let pool_msgs: Vec<Message> = vec![
        Message::new(4, core::shm::request::CREATE_POOL)
            .arg(Value::Fd(0))
            .arg(Value::Int64(16))
            .arg(Value::NewId(oid(11))),
        Message::new(11, core::shm_pool::request::CREATE_BUFFER)
            .arg(Value::Int32(0))
            .arg(Value::Int32(2))
            .arg(Value::Int32(2))
            .arg(Value::Int32(8))
            .arg(Value::Uint32(0x34325258))
            .arg(Value::NewId(oid(12))),
        Message::new(6, shell::shell::request::GET_TOPLEVEL)
            .arg(Value::Object(Some(oid(10))))
            .arg(Value::Enum(2))
            .arg(Value::NewId(oid(13))),
        Message::new(10, core::surface::request::ATTACH).arg(Value::Object(Some(oid(12)))),
        Message::new(10, core::surface::request::DAMAGE).arg(
            Value::array(
                ldp_core::wire::ArgType::Rect,
                vec![ldp_core::wire::Primitive::Rect(
                    ldp_core::geometry::Rect::new(0, 0, 2, 2),
                )],
            )
            .expect("array"),
        ),
        Message::new(10, core::surface::request::COMMIT).arg(Value::Uint32(1)),
    ];
    for m in &pool_msgs {
        println!(">> send obj={} op={}", m.object_id, m.opcode);
        let carries_fd = m.args.iter().any(|a| matches!(a, Value::Fd(_)));
        let res = if carries_fd {
            link.send_with_fd(std::slice::from_ref(m), fd.try_clone().expect("dup"))
        } else {
            link.send(
                std::slice::from_ref(m),
                &mut ldp_transport::fd::FdList::new(),
            )
        };
        match res {
            Ok(()) => println!("   sent"),
            Err(e) => println!("   send err: {e}"),
        }
        for _ in 0..20 {
            match link.recv_one() {
                Ok(Some(ev)) => println!("   event: {ev:?}"),
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(10)),
                Err(e) => {
                    println!("   RECV ERR: {e} (death: {:?})", link.death());
                    break;
                }
            }
        }
    }
    println!("DONE. death: {:?}", link.death());
}
