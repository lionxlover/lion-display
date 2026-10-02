//! Per-opcode request handlers for the X11 server subset.
//!
//! `dispatch_request` routes one framed request. Every handler
//! decodes with the connection's byte order through the `Rd` reader,
//! validates
//! resource ids (emitting the matching X error), mutates the
//! [`Server`], and queues replies/events/errors on the client's
//! output. Short payloads answer `BadLength`; unknown or
//! unimplemented opcodes answer `BadImplementation` — never silence.
//!
//! The opcode numbers are the frozen core-protocol table (CreateWindow
//! = 1 … NoOperation = 116) plus the two served extensions at their
//! conventional opcodes (MIT-SHM 130, BIG-REQUESTS 131).

#![forbid(unsafe_code)]
// Handler signatures are uniform (`Result<(), OpErr>`) even where an
// individual handler cannot fail, and field names restate the wire's
// single-character geometry vocabulary.
#![allow(clippy::unnecessary_wraps, clippy::many_single_char_names)]

use ldp_core::geometry::Rect;

use crate::dispatch::{ClientId, FeedOutcome, Focus, Server};
use crate::events::{err, XEvent};
use crate::property::PropMode;
use crate::window::{ConfigureChanges, WinClass};
use crate::wire::Endian;

/// Handler failure: short payload (`BadLength`) or an X error event.
pub(crate) enum OpErr {
    /// The payload ended before the request's fields.
    Short,
    /// Emit this error (code, offending value).
    X(u8, u32),
}

/// Endian-aware payload reader.
pub(crate) struct Rd<'a> {
    e: Endian,
    p: &'a [u8],
    at: usize,
}

impl<'a> Rd<'a> {
    /// Bind a reader.
    pub(crate) fn new(e: Endian, p: &'a [u8]) -> Rd<'a> {
        Rd { e, p, at: 0 }
    }

    /// One byte.
    pub(crate) fn u8(&mut self) -> Result<u8, OpErr> {
        if self.at >= self.p.len() {
            return Err(OpErr::Short);
        }
        let v = self.p[self.at];
        self.at += 1;
        Ok(v)
    }

    /// One CARD16.
    pub(crate) fn u16(&mut self) -> Result<u16, OpErr> {
        if self.at + 2 > self.p.len() {
            return Err(OpErr::Short);
        }
        let v = self.e.get_u16(self.p, self.at);
        self.at += 2;
        Ok(v)
    }

    /// One INT16.
    pub(crate) fn i16(&mut self) -> Result<i16, OpErr> {
        Ok(self.u16()? as i16)
    }

    /// One CARD32.
    pub(crate) fn u32(&mut self) -> Result<u32, OpErr> {
        if self.at + 4 > self.p.len() {
            return Err(OpErr::Short);
        }
        let v = self.e.get_u32(self.p, self.at);
        self.at += 4;
        Ok(v)
    }

    /// One INT32.
    pub(crate) fn i32(&mut self) -> Result<i32, OpErr> {
        Ok(self.u32()? as i32)
    }

    /// Skip `n` bytes.
    pub(crate) fn skip(&mut self, n: usize) -> Result<(), OpErr> {
        if self.at + n > self.p.len() {
            return Err(OpErr::Short);
        }
        self.at += n;
        Ok(())
    }

    /// The remaining bytes (length-prefixed lists decode from here).
    pub(crate) fn rest(&self) -> &'a [u8] {
        &self.p[self.at.min(self.p.len())..]
    }

    /// Every remaining pair of INT16 (points).
    pub(crate) fn points_rest(&mut self) -> Result<Vec<(i32, i32)>, OpErr> {
        let n = self.rest().len() / 4;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let x = self.i16()?;
            let y = self.i16()?;
            out.push((i32::from(x), i32::from(y)));
        }
        Ok(out)
    }

    /// Every remaining rectangle (x, y, w, h).
    pub(crate) fn rects_rest(&mut self) -> Result<Vec<Rect>, OpErr> {
        let n = self.rest().len() / 8;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let x = self.i16()?;
            let y = self.i16()?;
            let w = self.u16()?;
            let h = self.u16()?;
            out.push(Rect::new(
                i32::from(x),
                i32::from(y),
                u32::from(w),
                u32::from(h),
            ));
        }
        Ok(out)
    }

    /// Every remaining arc.
    pub(crate) fn arcs_rest(&mut self) -> Result<Vec<crate::shape::Arc>, OpErr> {
        let n = self.rest().len() / 12;
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let x = self.i16()?;
            let y = self.i16()?;
            let w = self.u16()?;
            let h = self.u16()?;
            let a1 = self.i16()?;
            let a2 = self.i16()?;
            out.push(crate::shape::Arc {
                x: i32::from(x),
                y: i32::from(y),
                width: u32::from(w),
                height: u32::from(h),
                angle1: i32::from(a1),
                angle2: i32::from(a2),
            });
        }
        Ok(out)
    }
}

/// Route one framed request. `data` is the header's second byte
/// (minor opcode for extensions, else a request flag field).
pub(crate) fn dispatch_request(
    s: &mut Server,
    client: ClientId,
    opcode: u8,
    data: u8,
    payload: &[u8],
) -> FeedOutcome {
    let minor = if opcode >= 128 { data } else { 0 };
    let outcome = handle(s, client, opcode, data, payload);
    match outcome {
        Ok(()) => Ok(()),
        Err(OpErr::Short) => {
            s.error(client, err::LENGTH, 0, minor, opcode);
            Ok(())
        }
        Err(OpErr::X(code, value)) => {
            s.error(client, code, value, minor, opcode);
            Ok(())
        }
    }
}

/// Opcodes the subset serves inertly: the grab/ungrab/allow family
/// (input routes by geometry/focus), GrabServer pairs, and the
/// control/mapping requests recorded without effect (single-client
/// screen, frozen keymap), plus NoOperation.
const INERT_OPS: &[u8] = &[
    28, 29, 30, 31, 33, 34, 35, 36, 37, 38, 96, 98, 100, 101, 103, 105, 107, 108, 109, 111, 112,
    114, 116,
];

fn handle(
    s: &mut Server,
    client: ClientId,
    opcode: u8,
    data: u8,
    payload: &[u8],
) -> Result<(), OpErr> {
    if INERT_OPS.contains(&opcode) {
        return Ok(());
    }
    let e = s.endian_of(client);
    let mut rd = Rd::new(e, payload);
    match opcode {
        1 => create_window(s, client, data, &mut rd),
        2 => change_window_attributes(s, client, &mut rd),
        3 => get_window_attributes(s, client, &mut rd),
        4 => destroy_window(s, client, &mut rd),
        5 => destroy_subwindows(s, client, &mut rd),
        6 => change_save_set(s, client, &mut rd),
        8 => map_window(s, client, &mut rd),
        9 => map_subwindows(s, client, &mut rd),
        10 => unmap_window(s, client, &mut rd),
        11 => unmap_subwindows(s, client, &mut rd),
        12 => configure_window(s, client, &mut rd),
        13 => circulate_window(s, client, data, &mut rd),
        14 => get_geometry(s, client, &mut rd),
        15 => query_tree(s, client, &mut rd),
        16 => intern_atom(s, client, data, &mut rd),
        17 => get_atom_name(s, client, &mut rd),
        18 => change_property(s, client, data, &mut rd),
        19 => delete_property(s, client, &mut rd),
        20 => get_property(s, client, data, &mut rd),
        22 => list_properties(s, client, &mut rd),
        23 => set_selection_owner(s, client, &mut rd),
        24 => get_selection_owner(s, client, &mut rd),
        25 => convert_selection(s, client, &mut rd),
        26 => send_event(s, client, &mut rd),
        27 => grab_pointer(s, client, &mut rd),
        32 => grab_keyboard(s, client, &mut rd),
        39 => query_pointer(s, client, &mut rd),
        40 => translate_coordinates(s, client, &mut rd),
        41 => warp_pointer(s, client, &mut rd),
        42 => set_input_focus(s, client, data, &mut rd),
        43 => get_input_focus(s, client),
        44 => query_keymap(s, client),
        45 => open_font(s, client, &mut rd),
        46 => close_font(s, client, &mut rd),
        // The out-of-subset belt, answered honestly with
        // BadImplementation: ReparentWindow (the bridge is the WM),
        // the unused core opcode 21, and the text-extent pair (the
        // subset has no font rasterizer).
        7 | 21 | 47 | 48 => Err(OpErr::X(err::IMPLEMENTATION, 0)),
        49 => list_fonts(s, client),
        50 => list_fonts_with_info(s, client),
        51 => Ok(()), // SetFontPath: inert
        52 => get_font_path(s, client),
        53..=93 => crate::draw_ops::handle(s, client, opcode, data, &mut rd),
        94 => query_extension(s, client, &mut rd),
        95 => list_extensions(s, client),
        97 => get_keyboard_mapping(s, client, data, &mut rd),
        99 => get_keyboard_control(s, client),
        102 => get_pointer_control(s, client),
        104 => get_screen_saver(s, client),
        106 => list_hosts(s, client),
        110 => rotate_properties(s, client, &mut rd),
        113 => get_pointer_mapping(s, client),
        115 => get_modifier_mapping(s, client),
        crate::dispatch::ext::SHM => crate::draw_ops::shm(s, client, data, &mut rd),
        crate::dispatch::ext::BIGREQ => bigreq_enable(s, client, data),
        _ => Err(OpErr::X(err::REQUEST, u32::from(opcode))),
    }
}

// ---- window lifecycle ----

/// The window-attribute value list (CreateWindow /
/// ChangeWindowAttributes share the encoding; bits in wire order).
fn apply_window_attrs(
    s: &mut Server,
    client: ClientId,
    window: u32,
    mask: u32,
    rd: &mut Rd<'_>,
) -> Result<(), OpErr> {
    if mask >> 13 != 0 {
        return Err(OpErr::X(err::VALUE, mask));
    }
    let mut bg: Option<Option<u32>> = None; // Some(new value)
    let mut event_mask: Option<u32> = None;
    let mut dnp: Option<u32> = None;
    let mut or: Option<bool> = None;
    let mut bit_gravity: Option<u8> = None;
    let mut win_gravity: Option<u8> = None;
    let mut save_under: Option<bool> = None;
    let mut colormap: Option<u32> = None;
    for bit in 0..13u32 {
        if mask & (1 << bit) == 0 {
            continue;
        }
        match bit {
            0 | 2 | 3 | 12 => {
                // BackPixmap / BorderPixmap / BorderPixel / Cursor:
                // recorded via the reserved slots (unused by the subset).
                let v = rd.u32()?;
                if bit == 0 && v == 0 {
                    // ParentRelative: the subset records it as "none".
                }
                let _ = v;
            }
            1 => bg = Some(Some(rd.u32()?)),
            4 => {
                let v = rd.u8()?;
                if v > 10 {
                    return Err(OpErr::X(err::VALUE, u32::from(v)));
                }
                bit_gravity = Some(v);
            }
            5 => {
                let v = rd.u8()?;
                if v > 9 {
                    return Err(OpErr::X(err::VALUE, u32::from(v)));
                }
                win_gravity = Some(v);
            }
            6 => {
                rd.u8()?; // backing store: recorded, not used
            }
            7 => save_under = Some(rd.u8()? != 0),
            8 => event_mask = Some(rd.u32()?),
            9 => dnp = Some(rd.u32()?),
            10 => or = Some(rd.u8()? != 0),
            11 => colormap = Some(rd.u32()?),
            _ => unreachable!("bit range checked above"),
        }
    }
    let _ = client;
    if let Some(w) = s.tree_mut().get_mut(window) {
        if let Some(b) = bg {
            w.background_pixel = b;
        }
        if let Some(m) = event_mask {
            w.event_mask = m;
        }
        if let Some(d) = dnp {
            w.do_not_propagate_mask = d;
        }
        if let Some(o) = or {
            w.override_redirect = o;
        }
        if let Some(g) = bit_gravity {
            w.bit_gravity = crate::window::BitGravity::from_wire(g)
                .unwrap_or(crate::window::BitGravity::Forget);
        }
        if let Some(g) = win_gravity {
            w.win_gravity = crate::window::WinGravity::from_wire(g)
                .unwrap_or(crate::window::WinGravity::Pin(1));
        }
        if let Some(su) = save_under {
            w.save_under = su;
        }
        if let Some(c) = colormap {
            w.colormap = c;
        }
    }
    Ok(())
}

fn create_window(
    s: &mut Server,
    client: ClientId,
    depth: u8,
    rd: &mut Rd<'_>,
) -> Result<(), OpErr> {
    // Depth 0 means CopyFromParent; the subset has one depth (24).
    if depth > 24 {
        return Err(OpErr::X(err::MATCH, u32::from(depth)));
    }
    let wid = rd.u32()?;
    let parent = rd.u32()?;
    let x = i32::from(rd.i16()?);
    let y = i32::from(rd.i16()?);
    let width = u32::from(rd.u16()?);
    let height = u32::from(rd.u16()?);
    let _border = rd.u16()?;
    // class is a CARD16 enum; the depth rides the header's data byte
    // (derived from the screen's single visual — recorded, not used).
    let class = rd.u16()?;
    let _visual = rd.u32()?;
    let mask = rd.u32()?;
    if !matches!(class, 1 | 2) {
        return Err(OpErr::X(err::VALUE, u32::from(class)));
    }
    if s.tree().windows.contains_key(&wid) {
        return Err(OpErr::X(err::ID_CHOICE, wid));
    }
    let wc = if class == 2 {
        WinClass::InputOnly
    } else {
        WinClass::InputOutput
    };
    match s.tree_mut().create(wid, parent, x, y, width, height, wc) {
        Ok(()) => {}
        Err(crate::window::TreeError::BadWindow(w)) => return Err(OpErr::X(err::WINDOW, w)),
        Err(crate::window::TreeError::BadMatch) => return Err(OpErr::X(err::MATCH, parent)),
        Err(crate::window::TreeError::BadIdChoice(w)) => return Err(OpErr::X(err::ID_CHOICE, w)),
        Err(crate::window::TreeError::BadValue(v)) => return Err(OpErr::X(err::VALUE, v)),
    }
    s.owners_mut().insert(wid, client);
    apply_window_attrs(s, client, wid, mask, rd)?;
    s.create_notify(wid);
    Ok(())
}

fn change_window_attributes(
    s: &mut Server,
    client: ClientId,
    rd: &mut Rd<'_>,
) -> Result<(), OpErr> {
    let window = rd.u32()?;
    if !s.tree().windows.contains_key(&window) {
        return Err(OpErr::X(err::WINDOW, window));
    }
    let mask = rd.u32()?;
    apply_window_attrs(s, client, window, mask, rd)
}

fn get_window_attributes(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let window = rd.u32()?;
    let Some(w) = s.tree().get(window).cloned() else {
        return Err(OpErr::X(err::WINDOW, window));
    };
    let e = s.endian_of(client);
    let mut out = s.reply_prefix(client);
    out.extend_from_slice(&e.put_u32(s.visual()));
    out.push(if w.class == WinClass::InputOnly { 2 } else { 1 });
    out.push(match w.bit_gravity {
        crate::window::BitGravity::Forget => 0,
        crate::window::BitGravity::Pin(g) => g,
    });
    out.push(match w.win_gravity {
        crate::window::WinGravity::Unmap => 0,
        crate::window::WinGravity::Pin(g) => g,
    });
    out.push(0); // backing store: NotUseful
    out.extend_from_slice(&e.put_u32(0)); // backing planes
    out.extend_from_slice(&e.put_u32(0)); // backing pixel
    out.push(u8::from(w.save_under));
    out.push(0); // map installed (the screen colormap always is)
                 // Map state: 0 unmapped, 1 unviewable, 2 viewable. The subset
                 // reports mapped-ness (viewability of the whole chain is
                 // approximated by the window's own mapped flag).
    out.push(u8::from(w.mapped) * 2);
    out.push(u8::from(w.override_redirect));
    out.extend_from_slice(&e.put_u32(w.colormap));
    out.extend_from_slice(&e.put_u32(w.event_mask)); // all-event-masks
    out.extend_from_slice(&e.put_u32(w.event_mask)); // your-event-mask
    out.extend_from_slice(&e.put_u16(w.do_not_propagate_mask as u16));
    out.extend_from_slice(&[0; 2]);
    s.finish_reply(client, out);
    Ok(())
}

fn destroy_window(s: &mut Server, _client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let window = rd.u32()?;
    if window == s.root_id() {
        return Err(OpErr::X(err::ACCESS, window));
    }
    if !s.tree().windows.contains_key(&window) {
        return Err(OpErr::X(err::WINDOW, window));
    }
    s.destroy_window_internal(window);
    Ok(())
}

fn destroy_subwindows(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let window = rd.u32()?;
    let Some(w) = s.tree().get(window).cloned() else {
        return Err(OpErr::X(err::WINDOW, window));
    };
    let _ = client;
    for child in w.children.clone() {
        if s.tree().get(child).is_some() {
            s.destroy_window_internal(child);
        }
    }
    Ok(())
}

fn change_save_set(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let window = rd.u32()?;
    let _ = client;
    if !s.tree().windows.contains_key(&window) {
        return Err(OpErr::X(err::WINDOW, window));
    }
    Ok(())
}

fn map_window(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let window = rd.u32()?;
    let _ = client;
    if !s.tree().windows.contains_key(&window) {
        return Err(OpErr::X(err::WINDOW, window));
    }
    let effect = match s.tree_mut().map(window) {
        Ok(e) => e,
        Err(crate::window::TreeError::BadWindow(w)) => return Err(OpErr::X(err::WINDOW, w)),
        Err(_) => return Err(OpErr::X(err::WINDOW, window)),
    };
    s.map_notify(window, true);
    s.emit_exposures(&effect);
    s.recomposite_effect(&effect);
    Ok(())
}

fn map_subwindows(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let window = rd.u32()?;
    let _ = client;
    let Some(w) = s.tree().get(window).cloned() else {
        return Err(OpErr::X(err::WINDOW, window));
    };
    for child in w.children.clone() {
        let Ok(effect) = s.tree_mut().map(child) else {
            continue;
        };
        s.map_notify(child, true);
        s.emit_exposures(&effect);
        s.recomposite_effect(&effect);
    }
    Ok(())
}

fn unmap_window(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let window = rd.u32()?;
    let _ = client;
    if !s.tree().windows.contains_key(&window) {
        return Err(OpErr::X(err::WINDOW, window));
    }
    let effect = match s.tree_mut().unmap(window) {
        Ok(e) => e,
        Err(crate::window::TreeError::BadWindow(w)) => return Err(OpErr::X(err::WINDOW, w)),
        Err(_) => return Err(OpErr::X(err::WINDOW, window)),
    };
    s.map_notify(window, false);
    s.emit_exposures(&effect);
    s.recomposite_effect(&effect);
    // Unmapping the focus window resets focus to None.
    if s.focus() == Focus::Window(window) {
        s.set_focus(Focus::None, 0);
    }
    Ok(())
}

fn unmap_subwindows(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let window = rd.u32()?;
    let _ = client;
    let Some(w) = s.tree().get(window).cloned() else {
        return Err(OpErr::X(err::WINDOW, window));
    };
    for child in w.children.clone() {
        let Ok(effect) = s.tree_mut().unmap(child) else {
            continue;
        };
        s.map_notify(child, false);
        s.emit_exposures(&effect);
        s.recomposite_effect(&effect);
    }
    Ok(())
}

fn configure_window(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let window = rd.u32()?;
    let mask = rd.u16()?;
    rd.skip(2)?;
    if !s.tree().windows.contains_key(&window) {
        return Err(OpErr::X(err::WINDOW, window));
    }
    let _ = client;
    let mut ch = ConfigureChanges::default();
    for bit in 0..7u16 {
        if mask & (1 << bit) == 0 {
            continue;
        }
        let v = rd.u32()?;
        match bit {
            0 => ch.x = Some(v as i32),
            1 => ch.y = Some(v as i32),
            2 => ch.width = Some(v),
            3 => ch.height = Some(v),
            4 => ch.border_width = Some(v),
            5 => ch.sibling = Some(v),
            6 => ch.stack_mode = Some(v as u8),
            _ => unreachable!(),
        }
    }
    if let Some(mode) = ch.stack_mode {
        if mode > 4 {
            return Err(OpErr::X(err::VALUE, u32::from(mode)));
        }
    }
    if let Some(sib) = ch.sibling {
        if sib != 0 && !s.tree().windows.contains_key(&sib) {
            return Err(OpErr::X(err::MATCH, sib));
        }
    }
    match s.tree_mut().configure(window, ch) {
        Ok(effect) => {
            s.configure_notify(window);
            s.emit_exposures(&effect);
            s.recomposite_effect(&effect);
            Ok(())
        }
        Err(crate::window::TreeError::BadWindow(w)) => Err(OpErr::X(err::WINDOW, w)),
        Err(crate::window::TreeError::BadMatch) => Err(OpErr::X(err::MATCH, window)),
        Err(crate::window::TreeError::BadIdChoice(w)) => Err(OpErr::X(err::ID_CHOICE, w)),
        Err(crate::window::TreeError::BadValue(v)) => Err(OpErr::X(err::VALUE, v)),
    }
}

fn circulate_window(
    s: &mut Server,
    client: ClientId,
    direction: u8,
    rd: &mut Rd<'_>,
) -> Result<(), OpErr> {
    let window = rd.u32()?;
    let _ = client;
    if direction > 1 {
        return Err(OpErr::X(err::VALUE, u32::from(direction)));
    }
    let Some(parent) = s.tree().get(window).and_then(|w| w.parent) else {
        return Err(OpErr::X(err::WINDOW, window));
    };
    let kids = s
        .tree()
        .get(parent)
        .map_or_else(Vec::new, |w| w.children.clone());
    if kids.len() < 2 {
        return Ok(());
    }
    let moving = if direction == 0 {
        *kids.first().unwrap()
    } else {
        *kids.last().unwrap()
    };
    let effect = {
        let ch = ConfigureChanges {
            stack_mode: Some(u8::from(direction != 0)), // Above / Below
            ..Default::default()
        };
        match s.tree_mut().configure(moving, ch) {
            Ok(e) => e,
            Err(_) => return Ok(()),
        }
    };
    let place = u8::from(direction != 0);
    let ev = XEvent::CirculateNotify {
        event: parent,
        window: moving,
        place,
    };
    s.deliver_to_window_owner(parent, &ev);
    s.emit_exposures(&effect);
    s.recomposite_effect(&effect);
    Ok(())
}

fn get_geometry(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let drawable = rd.u32()?;
    let e = s.endian_of(client);
    let mut out = s.reply_prefix(client);
    out[1] = 24; // depth rides the reply prefix's second byte
    if let Some(w) = s.tree().get(drawable) {
        let (ox, oy) = s.tree().absolute_origin(drawable);
        out.extend_from_slice(&e.put_u32(s.root_id()));
        out.extend_from_slice(&e.put_u16(ox as u16));
        out.extend_from_slice(&e.put_u16(oy as u16));
        out.extend_from_slice(&e.put_u16(w.width as u16));
        out.extend_from_slice(&e.put_u16(w.height as u16));
        out.extend_from_slice(&e.put_u16(w.border_width as u16));
        s.finish_reply(client, out);
        return Ok(());
    }
    if let Some(p) = s.pixmaps().get(&drawable) {
        out.extend_from_slice(&e.put_u32(s.root_id()));
        out.extend_from_slice(&e.put_u16(0));
        out.extend_from_slice(&e.put_u16(0));
        out.extend_from_slice(&e.put_u16(p.width as u16));
        out.extend_from_slice(&e.put_u16(p.height as u16));
        out.extend_from_slice(&e.put_u16(0));
        s.finish_reply(client, out);
        return Ok(());
    }
    Err(OpErr::X(err::DRAWABLE, drawable))
}

fn query_tree(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let window = rd.u32()?;
    let Some(w) = s.tree().get(window).cloned() else {
        return Err(OpErr::X(err::WINDOW, window));
    };
    let e = s.endian_of(client);
    let children: Vec<u32> = w.children.clone();
    let mut out = s.reply_prefix(client);
    out.extend_from_slice(&e.put_u32(s.root_id()));
    out.extend_from_slice(&e.put_u32(w.parent.unwrap_or(0)));
    out.extend_from_slice(&e.put_u16(children.len() as u16));
    for &c in &children {
        out.extend_from_slice(&e.put_u32(c));
    }
    s.finish_reply(client, out);
    Ok(())
}

// ---- atoms ----

fn intern_atom(
    s: &mut Server,
    client: ClientId,
    only_if_exists: u8,
    rd: &mut Rd<'_>,
) -> Result<(), OpErr> {
    let name_len = rd.u16()? as usize;
    rd.skip(2)?;
    if rd.rest().len() < crate::wire::pad4(name_len) + name_len {
        return Err(OpErr::Short);
    }
    let name = rd.rest()[..name_len].to_vec();
    match s.atoms_mut().intern(&name, only_if_exists != 0) {
        Some(atom) => {
            let e = s.endian_of(client);
            let mut out = s.reply_prefix(client);
            out.extend_from_slice(&e.put_u32(atom));
            s.finish_reply(client, out);
            Ok(())
        }
        None => Err(OpErr::X(err::NAME, 0)),
    }
}

fn get_atom_name(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let atom = rd.u32()?;
    let name: Vec<u8> = match s.atoms().name(atom) {
        Some(n) => n.as_bytes().to_vec(),
        None => return Err(OpErr::X(err::ATOM, atom)),
    };
    let e = s.endian_of(client);
    let mut out = s.reply_prefix(client);
    out.extend_from_slice(&e.put_u16(name.len() as u16));
    out.extend_from_slice(&[0; 2]);
    out.extend_from_slice(&name);
    s.finish_reply(client, out);
    Ok(())
}

// ---- properties ----

fn change_property(
    s: &mut Server,
    client: ClientId,
    mode: u8,
    rd: &mut Rd<'_>,
) -> Result<(), OpErr> {
    let window = rd.u32()?;
    let property = rd.u32()?;
    let type_atom = rd.u32()?;
    let format = rd.u8()?;
    rd.skip(3)?;
    let data_len = rd.u32()? as usize; // in format units
    let byte_len = data_len * usize::from(format / 8);
    if rd.rest().len() < byte_len + crate::wire::pad4(byte_len) {
        return Err(OpErr::Short);
    }
    let data = rd.rest()[..byte_len].to_vec();
    if !s.tree().windows.contains_key(&window) {
        return Err(OpErr::X(err::WINDOW, window));
    }
    if !s.atoms().exists(property) || !s.atoms().exists(type_atom) {
        return Err(OpErr::X(
            err::ATOM,
            if s.atoms().exists(property) {
                type_atom
            } else {
                property
            },
        ));
    }
    let prop_mode = match mode {
        0 => PropMode::Replace,
        1 => PropMode::Append,
        2 => PropMode::Prepend,
        other => return Err(OpErr::X(err::VALUE, u32::from(other))),
    };
    let result = s.tree_mut().get_mut(window).map(|w| {
        w.props
            .change(property, type_atom, format, prop_mode, &data)
    });
    match result {
        Some(Ok(())) => {
            s.property_notify(window, property, 0, false);
            Ok(())
        }
        Some(Err(e)) => {
            let value = property;
            s.prop_error(client, e, value, 18);
            Ok(())
        }
        None => Err(OpErr::X(err::WINDOW, window)),
    }
}

fn delete_property(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let window = rd.u32()?;
    let property = rd.u32()?;
    let _ = client;
    if !s.tree().windows.contains_key(&window) {
        return Err(OpErr::X(err::WINDOW, window));
    }
    if !s.atoms().exists(property) {
        return Err(OpErr::X(err::ATOM, property));
    }
    if let Some(w) = s.tree_mut().get_mut(window) {
        w.props.delete(property);
    }
    s.property_notify(window, property, 0, true);
    Ok(())
}

fn get_property(
    s: &mut Server,
    client: ClientId,
    delete: u8,
    rd: &mut Rd<'_>,
) -> Result<(), OpErr> {
    let window = rd.u32()?;
    let property = rd.u32()?;
    let type_atom = rd.u32()?;
    let long_offset = rd.u32()?;
    let long_length = rd.u32()?;
    let Some(w) = s.tree().get(window).cloned() else {
        return Err(OpErr::X(err::WINDOW, window));
    };
    if !s.atoms().exists(property) {
        return Err(OpErr::X(err::ATOM, property));
    }
    let (ty, format, bytes_after, data) = w.props.read(property, long_offset, long_length);
    let e = s.endian_of(client);
    let mut out = s.reply_prefix(client);
    if let Some(actual_ty) = ty {
        if type_atom != 0 && type_atom != actual_ty {
            // Type mismatch: an empty reply with the actual type.
            out.push(format);
            out.push(0);
            out.extend_from_slice(&e.put_u32(actual_ty));
            out.extend_from_slice(&e.put_u32(0));
            out.extend_from_slice(&e.put_u16(0));
            s.finish_reply(client, out);
            return Ok(());
        }
        out.push(format);
        out.push(0);
        out.extend_from_slice(&e.put_u32(actual_ty));
        out.extend_from_slice(&e.put_u32(bytes_after));
        out.extend_from_slice(&e.put_u16(data.len() as u16));
        out.extend_from_slice(&[0; 2]);
        out.extend_from_slice(&data);
        s.finish_reply(client, out);
        if delete != 0 {
            if let Some(w) = s.tree_mut().get_mut(window) {
                w.props.delete(property);
            }
            s.property_notify(window, property, 0, true);
        }
        Ok(())
    } else {
        // No such property: format 0, all zero.
        out.push(0);
        out.push(0);
        out.extend_from_slice(&e.put_u32(0));
        out.extend_from_slice(&e.put_u32(0));
        out.extend_from_slice(&e.put_u16(0));
        s.finish_reply(client, out);
        Ok(())
    }
}

fn list_properties(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let window = rd.u32()?;
    let Some(w) = s.tree().get(window).cloned() else {
        return Err(OpErr::X(err::WINDOW, window));
    };
    let atoms = w.props.list();
    let e = s.endian_of(client);
    let mut out = s.reply_prefix(client);
    out.extend_from_slice(&e.put_u16(atoms.len() as u16));
    for &a in &atoms {
        out.extend_from_slice(&e.put_u32(a));
    }
    s.finish_reply(client, out);
    Ok(())
}

fn rotate_properties(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let window = rd.u32()?;
    let n = rd.u16()? as usize;
    let delta = rd.i16()?;
    let mut atoms = Vec::with_capacity(n);
    for _ in 0..n {
        atoms.push(rd.u32()?);
    }
    let _ = client;
    if !s.tree().windows.contains_key(&window) {
        return Err(OpErr::X(err::WINDOW, window));
    }
    for &a in &atoms {
        if !s.atoms().exists(a) {
            return Err(OpErr::X(err::ATOM, a));
        }
    }
    if n == 0 {
        return Err(OpErr::X(err::MATCH, 0));
    }
    // Snapshot the values (type+format+data), rotate by delta, write back.
    let snapshot: Vec<(u32, u8, Vec<u8>)> = {
        let w = s.tree().get(window).expect("checked above");
        atoms
            .iter()
            .map(|&a| {
                w.props.get(a).map_or((0, 0, Vec::new()), |v| {
                    (v.type_atom, v.format, v.data.clone())
                })
            })
            .collect()
    };
    let shift = i32::from(delta).rem_euclid(n as i32) as usize;
    // Rotate: value at position i moves to (i + shift) mod n.
    let mut perm: Vec<(u32, u8, Vec<u8>)> = vec![(0, 0, Vec::new()); n];
    for (i, v) in snapshot.into_iter().enumerate() {
        perm[(i + shift) % n] = v;
    }
    if let Some(w) = s.tree_mut().get_mut(window) {
        for (i, &a) in atoms.iter().enumerate() {
            let (ty, f, ref d) = perm[i];
            if f != 0 {
                let _ = w.props.change(a, ty, f, PropMode::Replace, d);
            }
        }
    }
    Ok(())
}

// ---- selections ----

fn set_selection_owner(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let window = rd.u32()?;
    let selection = rd.u32()?;
    let time = rd.u32()?;
    let _ = client;
    if window != 0 && !s.tree().windows.contains_key(&window) {
        return Err(OpErr::X(err::WINDOW, window));
    }
    if !s.atoms().exists(selection) {
        return Err(OpErr::X(err::ATOM, selection));
    }
    let previous = s.selection_mut().get(&selection).copied();
    s.selection_mut().insert(selection, (window, time));
    if let Some((prev_owner, _)) = previous {
        if prev_owner != 0 && prev_owner != window {
            let ev = XEvent::SelectionClear {
                time,
                owner: prev_owner,
                selection,
            };
            s.deliver_to_window_owner(prev_owner, &ev);
        }
    }
    Ok(())
}

fn get_selection_owner(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let selection = rd.u32()?;
    if !s.atoms().exists(selection) {
        return Err(OpErr::X(err::ATOM, selection));
    }
    let owner = s.selection_mut().get(&selection).map_or(0, |&(w, _)| w);
    let e = s.endian_of(client);
    let mut out = s.reply_prefix(client);
    out.extend_from_slice(&e.put_u32(owner));
    s.finish_reply(client, out);
    Ok(())
}

fn convert_selection(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let requestor = rd.u32()?;
    let selection = rd.u32()?;
    let target = rd.u32()?;
    let property = rd.u32()?;
    let time = rd.u32()?;
    if !s.tree().windows.contains_key(&requestor) {
        return Err(OpErr::X(err::WINDOW, requestor));
    }
    let _ = client;
    let owner = s.selection_mut().get(&selection).map_or(0, |&(w, _)| w);
    if owner == 0 {
        // No owner: the server answers SelectionNotify{property: None}.
        let ev = XEvent::SelectionNotify {
            time,
            requestor,
            selection,
            target,
            property: 0,
        };
        s.deliver_to_window_owner(requestor, &ev);
    } else {
        let ev = XEvent::SelectionRequest {
            time,
            owner,
            requestor,
            selection,
            target,
            property,
        };
        s.deliver_to_window_owner(owner, &ev);
    }
    Ok(())
}

// ---- send event / grabs / focus ----

fn send_event(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let destination = rd.u32()?;
    let event_mask = rd.u32()?;
    let mut event = Vec::with_capacity(32);
    for _ in 0..32 {
        event.push(rd.u8()?);
    }
    let _ = client;
    // Resolve the destination pseudo-windows.
    let (px, py) = s.pointer_position();
    let target = match destination {
        1 => s.tree().window_at(px, py),
        2 => match s.focus() {
            Focus::Window(w) => w,
            _ => 0,
        },
        w => w,
    };
    if target == 0 || !s.tree().windows.contains_key(&target) {
        return Ok(()); // Dropped: no destination.
    }
    let wants = s.tree().get(target).map_or(0, |w| w.event_mask);
    if wants & event_mask == 0 {
        return Ok(()); // Mask misses: dropped.
    }
    // The event travels with the send-event bit set.
    let mut ev = event;
    ev[0] |= 0x80;
    s.send_raw_to_window_owner(target, &ev);
    Ok(())
}

fn grab_pointer(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let _grab_window = rd.u32()?;
    let _event_mask = rd.u16()?;
    let _pointer_mode = rd.u8()?;
    let _keyboard_mode = rd.u8()?;
    let _confine_to = rd.u32()?;
    let _cursor = rd.u32()?;
    let _time = rd.u32()?;
    // Inert grab: reply GrabSuccess (status 0) — the bridge routes
    // input by geometry/focus, the documented Phase 17 model.
    let mut out = s.reply_prefix(client);
    out.push(0);
    s.finish_reply(client, out);
    Ok(())
}

fn grab_keyboard(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let _grab_window = rd.u32()?;
    let _time = rd.u32()?;
    let _pointer_mode = rd.u8()?;
    let _keyboard_mode = rd.u8()?;
    rd.skip(2)?;
    let mut out = s.reply_prefix(client);
    out.push(0);
    s.finish_reply(client, out);
    Ok(())
}

fn query_pointer(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let window = rd.u32()?;
    if !s.tree().windows.contains_key(&window) {
        return Err(OpErr::X(err::WINDOW, window));
    }
    let (px, py) = s.pointer_position();
    // The deepest window under the pointer, reported when it is a
    // descendant of the query window.
    let deep = s.tree().window_at(px, py);
    let child = if deep != window && s.tree().is_ancestor_or_self(window, deep) {
        deep
    } else {
        0
    };
    let root = s.root_id();
    let (wx, wy) = s.tree().absolute_origin(window);
    let state = s.input_state();
    let e = s.endian_of(client);
    let mut out = s.reply_prefix(client);
    out.push(1); // same screen
    out.push(0);
    out.extend_from_slice(&e.put_u32(root));
    out.extend_from_slice(&e.put_u32(child));
    out.extend_from_slice(&e.put_u16(px.clamp(0, 0xffff) as u16));
    out.extend_from_slice(&e.put_u16(py.clamp(0, 0xffff) as u16));
    out.extend_from_slice(&e.put_u16((px - wx).clamp(0, 0xffff) as u16));
    out.extend_from_slice(&e.put_u16((py - wy).clamp(0, 0xffff) as u16));
    out.extend_from_slice(&e.put_u16(state));
    s.finish_reply(client, out);
    Ok(())
}

fn translate_coordinates(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let src_window = rd.u32()?;
    let dst_window = rd.u32()?;
    let src_x = rd.i16()?;
    let src_y = rd.i16()?;
    for w in [src_window, dst_window] {
        if !s.tree().windows.contains_key(&w) {
            return Err(OpErr::X(err::WINDOW, w));
        }
    }
    let (sx, sy) = s.tree().absolute_origin(src_window);
    let (dx, dy) = s.tree().absolute_origin(dst_window);
    let abs_x = sx + i32::from(src_x);
    let abs_y = sy + i32::from(src_y);
    // The dst child under the translated point.
    let deep = s.tree().window_at(abs_x, abs_y);
    let child = if deep != dst_window && s.tree().is_ancestor_or_self(dst_window, deep) {
        deep
    } else {
        0
    };
    let e = s.endian_of(client);
    let mut out = s.reply_prefix(client);
    out.push(1); // same screen
    out.push(0);
    out.extend_from_slice(&e.put_u32(child));
    out.extend_from_slice(&e.put_u16((abs_x - dx) as i16 as u16));
    out.extend_from_slice(&e.put_u16((abs_y - dy) as i16 as u16));
    s.finish_reply(client, out);
    Ok(())
}

fn warp_pointer(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let _src_window = rd.u32()?;
    let dst_window = rd.u32()?;
    let _src_x = rd.i16()?;
    let _src_y = rd.i16()?;
    let _src_w = rd.u16()?;
    let _src_h = rd.u16()?;
    let dst_x = rd.i16()?;
    let dst_y = rd.i16()?;
    let _ = client;
    let (x, y) = if dst_window == 0 {
        (i32::from(dst_x), i32::from(dst_y))
    } else {
        // Relative to the destination window's origin.
        let (ox, oy) = s.tree().absolute_origin(dst_window);
        (ox + i32::from(dst_x), oy + i32::from(dst_y))
    };
    s.pointer_move(x, y, 0);
    Ok(())
}

fn set_input_focus(
    s: &mut Server,
    client: ClientId,
    revert_to: u8,
    rd: &mut Rd<'_>,
) -> Result<(), OpErr> {
    let focus = rd.u32()?;
    let _time = rd.u32()?;
    if revert_to > 2 {
        return Err(OpErr::X(err::VALUE, u32::from(revert_to)));
    }
    let _ = client;
    let target = match focus {
        0 => Focus::None,
        1 => Focus::PointerRoot,
        w => {
            if !s.tree().windows.contains_key(&w) {
                return Err(OpErr::X(err::WINDOW, w));
            }
            if s.tree()
                .get(w)
                .is_some_and(|win| win.class == WinClass::InputOnly)
            {
                return Err(OpErr::X(err::MATCH, w));
            }
            Focus::Window(w)
        }
    };
    s.set_focus(target, 0);
    Ok(())
}

fn get_input_focus(s: &mut Server, client: ClientId) -> Result<(), OpErr> {
    let (revert, focus) = match s.focus() {
        Focus::None => (1u8, 0u32),
        Focus::PointerRoot => (1, 1),
        Focus::Window(w) => (1, w),
    };
    let e = s.endian_of(client);
    let mut out = s.reply_prefix(client);
    out.push(revert);
    out.extend_from_slice(&e.put_u32(focus));
    s.finish_reply(client, out);
    Ok(())
}

fn query_keymap(s: &mut Server, client: ClientId) -> Result<(), OpErr> {
    let keys = s.keymap_bits();
    let mut out = s.reply_prefix(client);
    out.extend_from_slice(&keys);
    out.push(0);
    s.finish_reply(client, out);
    Ok(())
}

// ---- fonts (the inert subset) ----

fn open_font(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let fid = rd.u32()?;
    let _name_len = rd.u16()?;
    rd.skip(2)?;
    let _ = client;
    if s.fonts().contains(&fid) {
        return Err(OpErr::X(err::ID_CHOICE, fid));
    }
    s.fonts_mut().insert(fid);
    Ok(())
}

fn close_font(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let fid = rd.u32()?;
    let _ = client;
    if !s.fonts().contains(&fid) {
        return Err(OpErr::X(err::FONT, fid));
    }
    s.fonts_mut().remove(&fid);
    Ok(())
}

fn list_fonts(s: &mut Server, client: ClientId) -> Result<(), OpErr> {
    let e = s.endian_of(client);
    let mut out = s.reply_prefix(client);
    out.push(0); // no names
    out.extend_from_slice(&e.put_u16(0));
    s.finish_reply(client, out);
    Ok(())
}

fn list_fonts_with_info(s: &mut Server, client: ClientId) -> Result<(), OpErr> {
    // A single terminating reply (name length 0): "no fonts matched".
    // Every metric field is zero — the terminating marker.
    let e = s.endian_of(client);
    let mut out = s.reply_prefix(client);
    for _ in 0..8 {
        out.extend_from_slice(&e.put_u32(0));
    }
    for _ in 0..7 {
        out.extend_from_slice(&e.put_u16(0));
    }
    out.extend_from_slice(&[0; 4]); // min/max char-or-byte2, all-exist, name length 0
    s.finish_reply(client, out);
    Ok(())
}

fn get_font_path(s: &mut Server, client: ClientId) -> Result<(), OpErr> {
    let e = s.endian_of(client);
    let mut out = s.reply_prefix(client);
    out.push(0);
    out.extend_from_slice(&e.put_u16(0));
    s.finish_reply(client, out);
    Ok(())
}

// ---- extensions and misc ----

fn query_extension(s: &mut Server, client: ClientId, rd: &mut Rd<'_>) -> Result<(), OpErr> {
    let name_len = rd.u16()? as usize;
    rd.skip(2)?;
    if rd.rest().len() < name_len {
        return Err(OpErr::Short);
    }
    let name = rd.rest()[..name_len].to_vec();
    let (present, major): (u8, u16) = match name.as_slice() {
        b"MIT-SHM" => (1, u16::from(crate::dispatch::ext::SHM)),
        b"BIG-REQUESTS" => (1, u16::from(crate::dispatch::ext::BIGREQ)),
        _ => (0, 0),
    };
    let e = s.endian_of(client);
    let mut out = s.reply_prefix(client);
    out.push(present);
    if present == 1 {
        out.extend_from_slice(&e.put_u16(major));
        out.push(0); // first event
        out.push(0); // first error
    }
    s.finish_reply(client, out);
    Ok(())
}

fn list_extensions(s: &mut Server, client: ClientId) -> Result<(), OpErr> {
    let mut out = s.reply_prefix(client);
    out.push(2); // two names
    out.push(0);
    // STRING8 list: length-prefixed, padded.
    for name in [&b"MIT-SHM"[..], &b"BIG-REQUESTS"[..]] {
        out.push(name.len() as u8);
        out.extend_from_slice(name);
        let pad = crate::wire::pad4(name.len() + 1);
        out.resize(out.len() + pad, 0);
    }
    s.finish_reply(client, out);
    Ok(())
}

fn get_keyboard_mapping(
    s: &mut Server,
    client: ClientId,
    first_keycode: u8,
    rd: &mut Rd<'_>,
) -> Result<(), OpErr> {
    let count = rd.u8()?;
    if first_keycode < 8 {
        return Err(OpErr::X(err::VALUE, u32::from(first_keycode)));
    }
    if count == 0 {
        let mut out = s.reply_prefix(client);
        out.push(0);
        s.finish_reply(client, out);
        return Ok(());
    }
    let last = u32::from(first_keycode) + u32::from(count) - 1;
    if last > 255 {
        return Err(OpErr::X(err::VALUE, last));
    }
    let mut syms: Vec<u32> = Vec::new();
    for kc in first_keycode..(first_keycode + count) {
        let (base, shift) = keysyms_of(kc);
        syms.push(base);
        syms.push(shift);
    }
    let e = s.endian_of(client);
    let mut out = s.reply_prefix(client);
    out.push(2); // keysyms per keycode
    for sym in syms {
        out.extend_from_slice(&e.put_u32(sym));
    }
    s.finish_reply(client, out);
    Ok(())
}

/// The frozen keysym table: (base, shift) for one X keycode
/// (evdev + 8). Latin-1 letters/digits plus the editing keys; every
/// other keycode is `NoSymbol` (0).
pub(crate) fn keysyms_of(keycode: u8) -> (u32, u32) {
    const SPECIAL: &[(u8, u32)] = &[
        (9, 0xff1b),   // Escape
        (22, 0xff08),  // BackSpace
        (23, 0xff09),  // Tab
        (36, 0xff0d),  // Return
        (50, 0xffe1),  // Shift_L
        (62, 0xffe2),  // Shift_R
        (37, 0xffe3),  // Control_L
        (105, 0xffe4), // Control_R
        (64, 0xffe9),  // Alt_L
        (108, 0xffea), // Alt_R
        (65, 0x0020),  // space
        (133, 0xffeb), // Super_L
        (134, 0xffec), // Super_R
        (67, 0xffe5),  // Caps_Lock
    ];
    if let Some(&(_, sym)) = SPECIAL.iter().find(|&&(k, _)| k == keycode) {
        return (sym, 0);
    }
    // Letters: evdev 16..=25 → kc 24..=33 (q..p), 30..=38 → 38..=46
    // (a..l minus ;), 44..=50 → 52..=58 (z..m).
    let letters: &[(u8, char)] = &[
        (24, 'q'),
        (25, 'w'),
        (26, 'e'),
        (27, 'r'),
        (28, 't'),
        (29, 'y'),
        (30, 'u'),
        (31, 'i'),
        (32, 'o'),
        (33, 'p'),
        (38, 'a'),
        (39, 's'),
        (40, 'd'),
        (41, 'f'),
        (42, 'g'),
        (43, 'h'),
        (44, 'j'),
        (45, 'k'),
        (46, 'l'),
        (52, 'z'),
        (53, 'x'),
        (54, 'c'),
        (55, 'v'),
        (56, 'b'),
        (57, 'n'),
        (58, 'm'),
    ];
    if let Some(&(_, c)) = letters.iter().find(|&&(k, _)| k == keycode) {
        let lower = u32::from(c);
        let upper = u32::from(c.to_ascii_uppercase());
        return (lower, upper);
    }
    // Digits and symbols: evdev 2..=13 → kc 10..=21.
    let digits: &[(u8, char, char)] = &[
        (10, '1', '!'),
        (11, '2', '@'),
        (12, '3', '#'),
        (13, '4', '$'),
        (14, '5', '%'),
        (15, '6', '^'),
        (16, '7', '&'),
        (17, '8', '*'),
        (18, '9', '('),
        (19, '0', ')'),
        (20, '-', '_'),
        (21, '=', '+'),
    ];
    if let Some(&(_, a, b)) = digits.iter().find(|&&(k, _, _)| k == keycode) {
        return (u32::from(a), u32::from(b));
    }
    (0, 0)
}

fn get_keyboard_control(s: &mut Server, client: ClientId) -> Result<(), OpErr> {
    let e = s.endian_of(client);
    let mut out = s.reply_prefix(client);
    out.push(0); // key click percent
    out.extend_from_slice(&e.put_u32(0)); // led mask
    out.extend_from_slice(&[0; 3]);
    out.push(0); // auto repeat mode
                 // 32 bytes of per-key auto-repeat (all 0).
    out.extend_from_slice(&[0; 32]);
    s.finish_reply(client, out);
    Ok(())
}

fn get_pointer_control(s: &mut Server, client: ClientId) -> Result<(), OpErr> {
    let e = s.endian_of(client);
    let mut out = s.reply_prefix(client);
    out.extend_from_slice(&e.put_u16(1)); // accel numerator
    out.extend_from_slice(&e.put_u16(1)); // denominator
    out.extend_from_slice(&e.put_u16(0)); // threshold
    s.finish_reply(client, out);
    Ok(())
}

fn get_screen_saver(s: &mut Server, client: ClientId) -> Result<(), OpErr> {
    let e = s.endian_of(client);
    let mut out = s.reply_prefix(client);
    out.extend_from_slice(&e.put_u16(0));
    out.extend_from_slice(&e.put_u16(0));
    out.push(0);
    out.push(0);
    s.finish_reply(client, out);
    Ok(())
}

fn list_hosts(s: &mut Server, client: ClientId) -> Result<(), OpErr> {
    let mut out = s.reply_prefix(client);
    out.push(0);
    out.push(0); // allow accesses
    s.finish_reply(client, out);
    Ok(())
}

fn get_pointer_mapping(s: &mut Server, client: ClientId) -> Result<(), OpErr> {
    let mut out = s.reply_prefix(client);
    out.push(0);
    s.finish_reply(client, out);
    Ok(())
}

fn get_modifier_mapping(s: &mut Server, client: ClientId) -> Result<(), OpErr> {
    let mut out = s.reply_prefix(client);
    out.push(0); // keycodes per modifier
    s.finish_reply(client, out);
    Ok(())
}

fn bigreq_enable(s: &mut Server, client: ClientId, minor: u8) -> Result<(), OpErr> {
    if minor != 0 {
        return Err(OpErr::X(err::REQUEST, u32::from(minor)));
    }
    s.set_bigreq(client, true);
    let e = s.endian_of(client);
    let mut out = s.reply_prefix(client);
    out.extend_from_slice(&e.put_u32(crate::wire::max_request_bytes() as u32 / 4));
    s.finish_reply(client, out);
    Ok(())
}
