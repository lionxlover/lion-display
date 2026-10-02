//! Connection setup: the pre-request handshake both directions.
//!
//! An X client opens with a fixed 12-byte prefix: byte-order byte
//! (`'l'`/`'B'`), one pad, protocol major/minor (11 / 0 for core), then
//! optional authorization blobs. The server answers with the full
//! setup reply: release number, resource-id base/mask the client
//! allocates XIDs from, motion buffer size, vendor string, the
//! maximum request length, one SCREEN per attached LDP output (the
//! Phase 17 rootful bridge advertises exactly one), the pixmap
//! FORMATs, and the depth/visual tree clients draw with.
//!
//! The bridge's fixed screen: depth 24, one TrueColor visual
//! (`0xFF0000 / 0x00FF00 / 0x0000FF` masks, 8 bits per RGB), ZPixmap
//! scanlines padded to 32 bits at 32 bits per pixel — the exact shape
//! of LDP's XRGB8888 shm format, which is why the rasterizer and the
//! LDP buffer views share pixels without conversion.

#![forbid(unsafe_code)]

use crate::wire::{put_string8, Endian};

/// Fixed geometry/identity of the one advertised screen.
#[derive(Clone, Copy, Debug)]
pub struct ScreenParams {
    /// Root window XID (the bridge's own resource; 0x40 by default).
    pub root: u32,
    /// Width in pixels.
    pub width: u16,
    /// Height in pixels.
    pub height: u16,
    /// Physical width in millimeters (0 unknown).
    pub mm_width: u16,
    /// Physical height in millimeters (0 unknown).
    pub mm_height: u16,
    /// The single visual's ID.
    pub visual: u32,
    /// The default colormap's ID.
    pub colormap: u32,
    /// Black pixel value (RGB 0x000000).
    pub black_pixel: u32,
    /// White pixel value (RGB 0xFFFFFF).
    pub white_pixel: u32,
}

impl Default for ScreenParams {
    fn default() -> Self {
        ScreenParams {
            root: 0x40,
            width: 1024,
            height: 768,
            mm_width: 340,
            mm_height: 255,
            visual: 0x21,
            colormap: 0x22,
            black_pixel: 0x0000_0000,
            white_pixel: 0x00ff_ffff,
        }
    }
}

/// The client's parsed setup prefix.
#[derive(Clone, Debug)]
pub struct ClientSetup {
    /// Negotiated byte order for the whole connection.
    pub endian: Endian,
    /// Protocol major version (11 for core X11).
    pub major: u16,
    /// Protocol minor version (0 for core).
    pub minor: u16,
    /// Authorization protocol name (unused by the bridge; recorded).
    pub auth_name: Vec<u8>,
    /// Authorization protocol data (unused; recorded).
    pub auth_data: Vec<u8>,
}

/// Outcome of parsing a (possibly partial) setup prefix.
#[derive(Clone, Debug)]
pub enum Handshake {
    /// Not enough bytes yet; the count still required.
    Incomplete(usize),
    /// The parsed prefix and total bytes consumed.
    Ready(Box<ClientSetup>, usize),
}

/// Parse the setup prefix from `buf` (endian agnostic — the order is
/// itself the first byte).
///
/// The fixed part is 12 bytes: order, pad, major, minor, auth-name-len,
/// auth-data-len, pad(2). Then name, pad, data, pad.
#[must_use]
pub fn parse_handshake(buf: &[u8]) -> Handshake {
    if buf.len() < 12 {
        return Handshake::Incomplete(12 - buf.len());
    }
    // Unparseable order byte: a malformed 12-byte prefix so the
    // dispatcher can kill the connection (major version 0 marks it).
    let Ok(endian) = Endian::from_setup_byte(buf[0]) else {
        return Handshake::Ready(
            Box::new(ClientSetup {
                endian: Endian::Lsb,
                major: 0,
                minor: 0,
                auth_name: Vec::new(),
                auth_data: Vec::new(),
            }),
            12,
        );
    };
    let major = endian.get_u16(buf, 2);
    let minor = endian.get_u16(buf, 4);
    let name_len = endian.get_u16(buf, 6) as usize;
    let data_len = endian.get_u16(buf, 8) as usize;
    let fixed = 12usize;
    let name_end = fixed + name_len;
    let name_padded = name_end + ((4 - (name_len & 3)) & 3);
    let data_end = name_padded + data_len;
    let total = data_end + ((4 - (data_len & 3)) & 3);
    if buf.len() < total {
        return Handshake::Incomplete(total - buf.len());
    }
    let auth_name = buf[fixed..name_end].to_vec();
    let auth_data = buf[name_padded..data_end].to_vec();
    Handshake::Ready(
        Box::new(ClientSetup {
            endian,
            major,
            minor,
            auth_name,
            auth_data,
        }),
        total,
    )
}

/// Encode the server's successful setup reply.
///
/// One screen, one depth (24), one TrueColor visual, one FORMAT
/// (depth 24 / 32 bpp / 32 pad); byte order of the reply itself follows
/// the negotiated `endian`. `resource_id_base`/`mask` carve the XID
/// space the client allocates from; the bridge does not police that
/// clients stay inside the carve (it validates existence per class).
#[allow(clippy::too_many_arguments)] // one flat wire record; a struct
// would just restate the field list
#[must_use]
pub fn encode_setup_reply(
    endian: Endian,
    screen: &ScreenParams,
    release: u32,
    resource_id_base: u32,
    resource_id_mask: u32,
    max_request_len: u32,
    vendor: &[u8],
    min_keycode: u8,
    max_keycode: u8,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(160);
    out.push(1); // success
    out.push(0);
    out.extend_from_slice(&endian.put_u16(11));
    out.extend_from_slice(&endian.put_u16(0));
    // Additional data length in 4-byte units counting everything after
    // the 8-byte prefix: the 32-byte fixed fields, padded vendor, the
    // FORMAT list, and every SCREEN.
    let vendor_padded = (vendor.len() + 3) & !3;
    let formats_len = 8usize; // one FORMAT structure
    let visual = 24usize; // one VISUALTYPE
    let depth = 8 + visual; // DEPTH header + visuals
    let screen_len = 44 + depth; // SCREEN header + depths
    let additional = 32 + vendor_padded + formats_len + screen_len;
    out.extend_from_slice(&endian.put_u16((additional / 4) as u16));
    // --- additional data ---
    out.extend_from_slice(&endian.put_u32(release));
    out.extend_from_slice(&endian.put_u32(resource_id_base));
    out.extend_from_slice(&endian.put_u32(resource_id_mask));
    out.extend_from_slice(&endian.put_u32(0)); // motion buffer size
    out.extend_from_slice(&endian.put_u16(vendor.len() as u16));
    out.extend_from_slice(&endian.put_u16(max_request_len as u16));
    out.push(1); // number of screens
    out.push(1); // number of formats
    out.push(0); // image byte order: LSB first (rasterizer native)
    out.push(0); // bitmap bit order: LSB first
    out.push(32); // bitmap scanline unit
    out.push(32); // bitmap scanline pad
    out.push(min_keycode);
    out.push(max_keycode);
    out.extend_from_slice(&[0; 4]);
    put_string8(&mut out, vendor);
    // FORMAT (8 bytes): depth 24, bpp 32, scanline pad 32.
    out.push(24);
    out.push(32);
    out.push(32);
    out.extend_from_slice(&[0; 5]);
    // SCREEN.
    out.extend_from_slice(&endian.put_u32(screen.root));
    out.extend_from_slice(&endian.put_u32(screen.colormap));
    out.extend_from_slice(&endian.put_u32(screen.white_pixel));
    out.extend_from_slice(&endian.put_u32(screen.black_pixel));
    out.extend_from_slice(&endian.put_u32(0)); // current input masks
    out.extend_from_slice(&endian.put_u16(screen.width));
    out.extend_from_slice(&endian.put_u16(screen.height));
    out.extend_from_slice(&endian.put_u16(screen.mm_width));
    out.extend_from_slice(&endian.put_u16(screen.mm_height));
    out.extend_from_slice(&endian.put_u16(1)); // min installed maps
    out.extend_from_slice(&endian.put_u16(1)); // max installed maps
    out.extend_from_slice(&endian.put_u32(screen.visual));
    out.push(0); // backing store: not useful (we still retain content)
    out.push(0); // save unders: no
    out.push(24); // root depth
    out.push(1); // number of depths
    out.extend_from_slice(&[0; 4]);
    // DEPTH 24 with one TrueColor visual.
    out.push(24);
    out.push(0);
    out.extend_from_slice(&endian.put_u16(1));
    out.extend_from_slice(&[0; 4]);
    // VISUALTYPE: TrueColor (class 4), 8 bits per RGB, 256 entries.
    out.extend_from_slice(&endian.put_u32(screen.visual));
    out.push(4);
    out.push(8);
    out.extend_from_slice(&endian.put_u16(256));
    out.extend_from_slice(&endian.put_u32(0x00ff_0000));
    out.extend_from_slice(&endian.put_u32(0x0000_ff00));
    out.extend_from_slice(&endian.put_u32(0x0000_00ff));
    out.extend_from_slice(&endian.put_u32(0));
    out
}

/// Encode a setup failure (version not understood): reason string.
#[must_use]
pub fn encode_setup_failed(endian: Endian, reason: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    out.push(0);
    out.push(reason.len() as u8);
    out.extend_from_slice(&endian.put_u16(11));
    out.extend_from_slice(&endian.put_u16(0));
    // Additional length in 4-byte units: reason padded.
    let padded = (reason.len() + 3) & !3;
    out.extend_from_slice(&endian.put_u16((padded / 4) as u16));
    put_string8(&mut out, reason);
    out
}

/// Encode a setup authenticate challenge (not used by the Phase 17
/// bridge — no auth policy — but part of the wire vocabulary).
#[must_use]
pub fn encode_setup_authenticate(endian: Endian, reason: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    out.push(2);
    out.extend_from_slice(&[0; 5]);
    let padded = (reason.len() + 3) & !3;
    out.extend_from_slice(&endian.put_u16((padded / 4) as u16));
    put_string8(&mut out, reason);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a client setup prefix in the given order.
    fn setup_bytes(endian: Endian) -> Vec<u8> {
        let mut b = Vec::new();
        b.push(match endian {
            Endian::Lsb => 0x6c,
            Endian::Msb => 0x42,
        });
        b.push(0);
        b.extend_from_slice(&endian.put_u16(11));
        b.extend_from_slice(&endian.put_u16(0));
        b.extend_from_slice(&endian.put_u16(0));
        b.extend_from_slice(&endian.put_u16(0));
        b.extend_from_slice(&[0; 2]);
        b
    }

    #[test]
    fn handshake_parses_both_orders() {
        for e in [Endian::Lsb, Endian::Msb] {
            let bytes = setup_bytes(e);
            match parse_handshake(&bytes) {
                Handshake::Ready(setup, used) => {
                    assert_eq!(setup.endian, e);
                    assert_eq!(setup.major, 11);
                    assert_eq!(setup.minor, 0);
                    assert_eq!(used, 12);
                }
                other @ Handshake::Incomplete(_) => panic!("expected ready, got {other:?}"),
            }
        }
    }

    #[test]
    fn handshake_with_auth_rounds_lengths() {
        let e = Endian::Lsb;
        // Full prefix: order, pad, major, minor, name-len, data-len, pad.
        let mut b = vec![0x6c, 0];
        b.extend_from_slice(&e.put_u16(11));
        b.extend_from_slice(&e.put_u16(0));
        b.extend_from_slice(&e.put_u16(4)); // auth name len
        b.extend_from_slice(&e.put_u16(3)); // auth data len
        b.extend_from_slice(&[0; 2]);
        b.extend_from_slice(b"MIT-"); // 4 bytes, no pad
        b.extend_from_slice(b"abc\0"); // 3 bytes + 1 pad
        assert_eq!(b.len(), 20);
        match parse_handshake(&b) {
            Handshake::Ready(setup, used) => {
                assert_eq!(setup.auth_name, b"MIT-");
                assert_eq!(setup.auth_data, b"abc");
                assert_eq!(used, 20);
            }
            other @ Handshake::Incomplete(_) => panic!("expected ready, got {other:?}"),
        }
        // Truncated at 16 bytes: needs 4 more.
        match parse_handshake(&b[..16]) {
            Handshake::Incomplete(n) => assert_eq!(n, 4),
            other @ Handshake::Ready(..) => panic!("expected incomplete, got {other:?}"),
        }
    }

    #[test]
    fn setup_reply_layout_is_byte_exact() {
        let e = Endian::Lsb;
        let screen = ScreenParams::default();
        let bytes = encode_setup_reply(
            e,
            &screen,
            11_805_000,
            0x0040_0000,
            0x001f_ffff,
            4096,
            b"LionOS X Bridge",
            8,
            255,
        );
        assert_eq!(bytes[0], 1);
        assert_eq!(&bytes[2..4], &11u16.to_le_bytes());
        assert_eq!(e.get_u16(&bytes, 24), 15); // vendor length
        assert_eq!(e.get_u16(&bytes, 26), 4096); // max request length
        assert_eq!(bytes[28], 1); // screens
        assert_eq!(bytes[29], 1); // formats
        assert_eq!(bytes[34], 8); // min keycode
        assert_eq!(bytes[35], 255); // max keycode
                                    // Vendor starts at 40, padded to 16 bytes.
        assert_eq!(&bytes[40..47], b"LionOS ");
        // FORMAT at 56: depth 24, bpp 32, scanline pad 32.
        assert_eq!(bytes[56], 24);
        assert_eq!(bytes[57], 32);
        assert_eq!(bytes[58], 32);
        // SCREEN at 64: root, then geometry at 84.
        assert_eq!(e.get_u32(&bytes, 64), 0x40);
        assert_eq!(e.get_u16(&bytes, 84), 1024);
        assert_eq!(e.get_u16(&bytes, 86), 768);
        // DEPTH at 108 with one visual.
        assert_eq!(bytes[108], 24);
        // VISUALTYPE at 116: id, class 4 (TrueColor), masks.
        assert_eq!(e.get_u32(&bytes, 116), 0x21);
        assert_eq!(bytes[120], 4);
        assert_eq!(bytes[121], 8);
        assert_eq!(e.get_u32(&bytes, 124), 0x00ff_0000);
        assert_eq!(e.get_u32(&bytes, 128), 0x0000_ff00);
        assert_eq!(e.get_u32(&bytes, 132), 0x0000_00ff);
        // Additional-data length accounts for every byte after byte 8.
        let extra = e.get_u16(&bytes, 6) as usize * 4;
        assert_eq!(8 + extra, bytes.len());
        assert_eq!(bytes.len(), 140);
    }

    #[test]
    fn setup_failed_layout() {
        let e = Endian::Msb;
        let bytes = encode_setup_failed(e, b"nope");
        assert_eq!(bytes[0], 0);
        assert_eq!(bytes[1], 4);
        assert_eq!(bytes.len(), 12);
    }

    #[test]
    fn bad_byte_order_reports_ready_with_zero_version() {
        // A zero order byte is fatal; the dispatcher distinguishes it by
        // the major version 0 marker.
        let mut b = vec![0u8; 12];
        b[2] = 11;
        match parse_handshake(&b) {
            Handshake::Ready(s, _) => assert_eq!(s.major, 0),
            other @ Handshake::Incomplete(_) => panic!("expected ready, got {other:?}"),
        }
    }
}
