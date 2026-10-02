//! EC: BIG-REQUESTS framing and MIT-SHM transfer through the host
//! seam, including the negative paths.

mod common;

use common::*;
use ldp_x11_bridge::shm::{ShmHost, ShmState};

/// An in-memory SHM host: segments are `Vec<u8>` slots.
#[derive(Default)]
struct MemShm {
    segments: Vec<(u32, Vec<u8>)>,
}

impl MemShm {
    fn add(&mut self, seg: u32, bytes: Vec<u8>) {
        self.segments.push((seg, bytes));
    }
}

impl ShmHost for MemShm {
    fn read_segment(&mut self, seg: u32, offset: usize, len: usize) -> Option<Vec<u8>> {
        let entry = self.segments.iter_mut().find(|(s, _)| *s == seg)?;
        let end = offset.checked_add(len)?;
        if end > entry.1.len() {
            return None;
        }
        Some(entry.1[offset..end].to_vec())
    }
    fn write_segment(&mut self, seg: u32, offset: usize, data: &[u8]) -> Option<()> {
        let entry = self.segments.iter_mut().find(|(s, _)| *s == seg)?;
        let end = offset.checked_add(data.len())?;
        if end > entry.1.len() {
            return None;
        }
        entry.1[offset..end].copy_from_slice(data);
        Some(())
    }
}

#[test]
fn shm_put_image_transfers_pixels() {
    use ldp_x11_bridge::dispatch::Server;
    let mut host = MemShm::default();
    // Segment 0x1000: a 8x8 32bpp image (256 bytes) with a red
    // diagonal.
    let mut seg = vec![0u8; 8 * 8 * 4];
    for i in 0..8 {
        let at = (i * 8 + i) * 4; // the 2-D diagonal in row-major
        seg[at..at + 4].copy_from_slice(&[0, 0, 0xff, 0xff]);
    }
    host.add(0x1000, seg);
    let mut server = Server::new(screen(), Box::new(host));
    let client = server.add_client();
    server.feed(client, &handshake()).unwrap();
    let _ = server.take_output(client);

    let mut cx = Vec::new();
    cx.extend_from_slice(&query_extension("MIT-SHM"));
    server.feed(client, &cx).unwrap();
    let out = server.take_output(client);
    let reply = envelopes(&out)[0];
    assert_eq!(reply[0], 1);
    assert_eq!(reply[8], 1, "present");
    assert_eq!(u16::from_le_bytes(reply[9..11].try_into().unwrap()), 130);

    // Window + GC + attach + PutImage from the segment.
    let mut cx = Vec::new();
    cx.extend_from_slice(&create_window(0x41, 0x40, 0, 0, 64, 64, 0, &[]));
    cx.extend_from_slice(&create_gc(0x50, 0x41, 0, &[]));
    cx.extend_from_slice(&map_window(0x41));
    cx.extend_from_slice(&shm_attach(0x1000, 0x2100, false));
    // Source rect (0,0,8,8) of the 8x8 segment, drawn at (4, 4).
    cx.extend_from_slice(&shm_put_image(
        0x41, 0x50, 8, 8, 0, 0, 8, 8, 4, 4, 0x1000, 0,
    ));
    server.feed(client, &cx).unwrap();
    let out = server.take_output(client);
    assert!(
        envelopes(&out).iter().all(|e| e[0] != 0),
        "no errors: {out:02x?}"
    );
    let store = server.root_store().expect("root store");
    // The diagonal: red pixels at (4+i, 4+i).
    assert_eq!(store.pixel(4, 4), Some(0x00ff_0000));
    assert_eq!(store.pixel(9, 9), Some(0x00ff_0000));
    // Off-diagonal stays background.
    assert_eq!(store.pixel(5, 4), Some(0));

    // Detach twice: the second is BadValue.
    let mut cx = Vec::new();
    cx.extend_from_slice(&request(130, 2, &0x1000u32.to_le_bytes()));
    cx.extend_from_slice(&request(130, 2, &0x1000u32.to_le_bytes()));
    server.feed(client, &cx).unwrap();
    let out = server.take_output(client);
    let errors: Vec<&[u8]> = envelopes(&out).into_iter().filter(|e| e[0] == 0).collect();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0][1], 2, "BadValue for the unknown segment");
}

#[test]
fn shm_out_of_bounds_read_is_bad_access() {
    use ldp_x11_bridge::dispatch::Server;
    let mut host = MemShm::default();
    host.add(0x1001, vec![0u8; 16]); // too small for any image
    let mut server = Server::new(screen(), Box::new(host));
    let client = server.add_client();
    server.feed(client, &handshake()).unwrap();
    let _ = server.take_output(client);
    let mut cx = Vec::new();
    cx.extend_from_slice(&create_window(0x41, 0x40, 0, 0, 64, 64, 0, &[]));
    cx.extend_from_slice(&create_gc(0x50, 0x41, 0, &[]));
    cx.extend_from_slice(&shm_attach(0x1001, 0x2101, false));
    // An 8x8 image needs 256 bytes; the segment has 16.
    cx.extend_from_slice(&shm_put_image(
        0x41, 0x50, 8, 8, 0, 0, 8, 8, 0, 0, 0x1001, 0,
    ));
    server.feed(client, &cx).unwrap();
    let out = server.take_output(client);
    let errors: Vec<&[u8]> = envelopes(&out).into_iter().filter(|e| e[0] == 0).collect();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0][1], 10, "BadAccess for the out-of-bounds read");
    assert_eq!(
        u32::from_le_bytes(errors[0][4..8].try_into().unwrap()),
        0x1001
    );
}

#[test]
fn bigreq_enables_and_frames_huge_requests() {
    let (mut server, client, _) = connect(screen());
    // Before enabling: a 0-length request is fatal.
    let mut evil = vec![20u8, 0, 0, 0];
    evil.extend_from_slice(&[0; 4]);
    evil.extend_from_slice(&100u64.to_le_bytes());
    let outcome = server.feed(client, &evil);
    assert!(outcome.is_err(), "0-length without BIG-REQUESTS is fatal");

    // A second client enables it.
    let client2 = server.add_client();
    server.feed(client2, &handshake()).unwrap();
    let _ = server.take_output(client2);
    server.feed(client2, &bigreq_enable()).unwrap();
    let out = server.take_output(client2);
    let reply = envelopes(&out)[0];
    assert_eq!(reply[0], 1);
    // The advertised ceiling: 64 MiB in 4-byte units.
    let max = u32::from_le_bytes(reply[8..12].try_into().unwrap());
    assert_eq!(
        u64::from(max) * 4,
        ldp_x11_bridge::wire::max_request_bytes()
    );

    // A BIG-REQUESTS-framed InternAtom now decodes.
    let name = "BIGREQ_WORKS";
    let payload_len = 2 + 2 + name.len();
    let total = 16 + payload_len + crate_pad(payload_len);
    let mut big = vec![16u8, 0, 0, 0];
    big.extend_from_slice(&[0; 4]);
    big.extend_from_slice(&((total / 4) as u64).to_le_bytes());
    big.extend_from_slice(&(name.len() as u16).to_le_bytes());
    big.extend_from_slice(&[0; 2]);
    big.extend_from_slice(name.as_bytes());
    while big.len() < total {
        big.push(0);
    }
    server.feed(client2, &big).unwrap();
    let out = server.take_output(client2);
    let reply = envelopes(&out)[0];
    assert_eq!(reply[0], 1);
    assert_eq!(u32::from_le_bytes(reply[8..12].try_into().unwrap()), 69);
}

fn crate_pad(n: usize) -> usize {
    (4 - (n & 3)) & 3
}

#[test]
fn shm_state_lifecycle() {
    let mut s = ShmState::default();
    s.attach(1, false).unwrap();
    assert!(s.writable(1));
    s.attach(1, true).unwrap_err();
    s.detach(1).unwrap();
    s.detach(1).unwrap_err();
}
