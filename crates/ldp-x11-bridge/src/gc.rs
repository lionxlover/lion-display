//! Graphics contexts: the shared drawable state ChangeGC/CreateGC edit.
//!
//! A GC is a bundle of drawing parameters referenced by id from the
//! drawing requests. The bridge models exactly the fields its
//! rasterizer consumes plus the bookkeeping fields recorded for wire
//! fidelity. CreateGC and ChangeGC share one value-list decoder driven
//! by the attribute bitmask (fields appear in ascending bit order —
//! the protocol's fixed rule, pinned by tests).
//!
//! Subset doctrine (documented, error-pinned): `fill-style` values
//! other than `Solid` and a non-null `clip-mask` refer to state the
//! Phase 17 rasterizer does not implement; the *drawing* requests
//! reject them with `BadImplementation` (code 17) rather than silently
//! rendering something else. Fonts, tiles and stipples are recorded so
//! the GC round-trips, but the subset never rasterizes them.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use crate::render::Gx;

/// GC value-mask bits, in wire (ascending) order.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[allow(missing_docs)] // the names are the protocol's
pub enum GcAttr {
    Function = 0,
    PlaneMask = 1,
    Foreground = 2,
    Background = 3,
    LineWidth = 4,
    LineStyle = 5,
    CapStyle = 6,
    JoinStyle = 7,
    FillStyle = 8,
    FillRule = 9,
    Tile = 10,
    Stipple = 11,
    TileStippleXOrigin = 12,
    TileStippleYOrigin = 13,
    Font = 14,
    SubwindowMode = 15,
    GraphicsExposures = 16,
    ClipXOrigin = 17,
    ClipYOrigin = 18,
    ClipMask = 19,
    DashOffset = 20,
    Dashes = 21,
    ArcMode = 22,
}

/// Line style.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LineStyle {
    /// Solid path.
    Solid,
    /// Dashes; gaps untouched.
    OnOffDash,
    /// Dashes; gaps drawn with the background.
    DoubleDash,
}

impl LineStyle {
    /// Wire value 0..=2.
    #[must_use]
    pub const fn to_wire(self) -> u8 {
        match self {
            LineStyle::Solid => 0,
            LineStyle::OnOffDash => 1,
            LineStyle::DoubleDash => 2,
        }
    }

    /// Parse a wire value.
    #[must_use]
    pub const fn from_wire(v: u8) -> Option<LineStyle> {
        match v {
            0 => Some(LineStyle::Solid),
            1 => Some(LineStyle::OnOffDash),
            2 => Some(LineStyle::DoubleDash),
            _ => None,
        }
    }
}

/// Fill style.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FillStyle {
    /// Single color (the subset's implemented style).
    Solid,
    /// Tiled with a pixmap.
    Tiled,
    /// Stippled (transparent bitmap).
    Stippled,
    /// Stippled over a background fill.
    OpaqueStippled,
}

impl FillStyle {
    /// Wire value 0..=3.
    #[must_use]
    pub const fn to_wire(self) -> u8 {
        match self {
            FillStyle::Solid => 0,
            FillStyle::Tiled => 1,
            FillStyle::Stippled => 2,
            FillStyle::OpaqueStippled => 3,
        }
    }

    /// Parse a wire value.
    #[must_use]
    pub const fn from_wire(v: u8) -> Option<FillStyle> {
        match v {
            0 => Some(FillStyle::Solid),
            1 => Some(FillStyle::Tiled),
            2 => Some(FillStyle::Stippled),
            3 => Some(FillStyle::OpaqueStippled),
            _ => None,
        }
    }
}

/// Subwindow clipping mode for drawing on windows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SubwindowMode {
    /// Drawing is clipped by mapped children.
    ClipByChildren,
    /// Drawing passes over children.
    IncludeInferiors,
}

impl SubwindowMode {
    /// Wire value 0/1.
    #[must_use]
    pub const fn to_wire(self) -> u8 {
        match self {
            SubwindowMode::ClipByChildren => 0,
            SubwindowMode::IncludeInferiors => 1,
        }
    }

    /// Parse a wire value.
    #[must_use]
    pub const fn from_wire(v: u8) -> Option<SubwindowMode> {
        match v {
            0 => Some(SubwindowMode::ClipByChildren),
            1 => Some(SubwindowMode::IncludeInferiors),
            _ => None,
        }
    }
}

/// One graphics context.
#[derive(Clone, Debug)]
pub struct Gc {
    /// The XID.
    pub id: u32,
    /// Raster operation.
    pub function: Gx,
    /// Plane mask (depth-24 space).
    pub plane_mask: u32,
    /// Foreground pixel.
    pub foreground: u32,
    /// Background pixel.
    pub background: u32,
    /// Line width (0 = thin; > 0 uses the wide rasterizer).
    pub line_width: u32,
    /// Line style.
    pub line_style: LineStyle,
    /// Cap style (recorded).
    pub cap_style: u8,
    /// Join style (recorded).
    pub join_style: u8,
    /// Fill style (Solid implemented).
    pub fill_style: FillStyle,
    /// Fill rule.
    pub fill_rule: u8,
    /// Tile pixmap (recorded).
    pub tile: u32,
    /// Stipple pixmap (recorded).
    pub stipple: u32,
    /// Tile/stipple origin.
    pub ts_origin: (i32, i32),
    /// Font (recorded).
    pub font: u32,
    /// Subwindow clipping.
    pub subwindow_mode: SubwindowMode,
    /// Whether CopyArea reports exposures.
    pub graphics_exposures: bool,
    /// Clip origin.
    pub clip_origin: (i32, i32),
    /// Clip-mask pixmap (0 = none).
    pub clip_mask: u32,
    /// The SetClipRectangles list (GC-local coordinates, offset by
    /// `clip_origin` at draw time; empty = no restriction).
    pub clip_rects: Vec<ldp_core::geometry::Rect>,
    /// Dash offset.
    pub dash_offset: u32,
    /// Dash pattern (default [4, 4]).
    pub dashes: Vec<u8>,
    /// Arc fill mode: 0 chord, 1 pie slice.
    pub arc_mode: u8,
}

impl Gc {
    /// The protocol defaults.
    #[must_use]
    pub fn new(id: u32) -> Gc {
        Gc {
            id,
            function: Gx::Copy,
            plane_mask: 0x00ff_ffff,
            foreground: 0,
            background: 1,
            line_width: 0,
            line_style: LineStyle::Solid,
            cap_style: 1, // CapButt
            join_style: 0,
            fill_style: FillStyle::Solid,
            fill_rule: 0,
            tile: 0,
            stipple: 0,
            ts_origin: (0, 0),
            font: 0,
            subwindow_mode: SubwindowMode::ClipByChildren,
            graphics_exposures: true,
            clip_origin: (0, 0),
            clip_mask: 0,
            clip_rects: Vec::new(),
            dash_offset: 0,
            dashes: vec![4, 4],
            arc_mode: 1, // ArcPieSlice is the protocol's default
        }
    }

    /// Whether drawing with this GC hits unimplemented rasterizer
    /// state (the subset rejects rather than misrender).
    #[must_use]
    pub fn unsupported(&self) -> bool {
        self.fill_style != FillStyle::Solid || self.clip_mask != 0
    }

    /// The effective clip rectangle list for a drawable of the given
    /// bounds: clip rects (offset by the origin) intersected with the
    /// bounds; an empty stored list clips nothing.
    #[must_use]
    pub fn effective_clip(
        &self,
        bounds: ldp_core::geometry::Rect,
    ) -> Vec<ldp_core::geometry::Rect> {
        if self.clip_rects.is_empty() {
            return vec![bounds];
        }
        let mut out = Vec::new();
        for r in &self.clip_rects {
            let shifted = r.translate(self.clip_origin.0, self.clip_origin.1);
            if let Some(i) = shifted.intersect(bounds) {
                out.push(i);
            }
        }
        out
    }

    /// The dash state for the thin rasterizer: `Some((offset, pattern))`
    /// when the line style is dashed.
    #[must_use]
    pub fn dash_state(&self) -> Option<(u32, &[u8])> {
        match self.line_style {
            LineStyle::Solid => None,
            LineStyle::OnOffDash | LineStyle::DoubleDash => {
                Some((self.dash_offset, self.dashes.as_slice()))
            }
        }
    }
}

/// Why a value-list decode failed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum GcError {
    /// The payload ended before the mask's fields.
    Short,
    /// A field's value is out of range (bit index, value).
    BadValue(u8, u32),
    /// Unknown mask bits set (the protocol reserves them).
    UnknownBits(u32),
}

/// Apply a CreateGC/ChangeGC value list to a GC.
///
/// The payload starts at the value-list's first byte (after the
/// 12/16-byte request header consumed by the dispatcher). One flat
/// decoder over the whole attribute vocabulary.
///
/// # Errors
/// [`GcError`] per the decoder's contract.
#[allow(clippy::too_many_lines)]
///
/// # Panics
/// Never: `GcAttr::from` total-matches the 0..23 bit range the mask
/// check above bounds.
pub fn apply_value_list(gc: &mut Gc, mask: u32, payload: &[u8]) -> Result<(), GcError> {
    let mut at = 0usize;
    let read_u32 = |at: &mut usize| -> Result<u32, GcError> {
        if *at + 4 > payload.len() {
            return Err(GcError::Short);
        }
        let v = u32::from(payload[*at])
            | (u32::from(payload[*at + 1]) << 8)
            | (u32::from(payload[*at + 2]) << 16)
            | (u32::from(payload[*at + 3]) << 24);
        *at += 4;
        Ok(v)
    };
    let read_u16 = |at: &mut usize| -> Result<u32, GcError> {
        if *at + 4 > payload.len() {
            return Err(GcError::Short);
        }
        let v = u32::from(payload[*at]) | (u32::from(payload[*at + 1]) << 8);
        *at += 4;
        Ok(v)
    };
    // Value-list fields occupy 4-byte slots each (the protocol's
    // LISTofVALUE rule): CARD8/BOOL values ride the slot's low byte.
    let read_u8 = |at: &mut usize| -> Result<u32, GcError> {
        if *at + 4 > payload.len() {
            return Err(GcError::Short);
        }
        let v = u32::from(payload[*at]);
        *at += 4;
        Ok(v)
    };
    if mask >> 23 != 0 {
        return Err(GcError::UnknownBits(mask));
    }
    for bit in 0..23u32 {
        if mask & (1 << bit) == 0 {
            continue;
        }
        // Field widths per the protocol (most pack into 4-byte slots;
        // the small ones occupy their slot's low bytes).
        let bit_u8 = bit as u8;
        match GcAttr::from(bit) {
            GcAttr::Function => {
                let v = read_u8(&mut at)?;
                let Some(gx) = Gx::from_wire(v as u8) else {
                    return Err(GcError::BadValue(bit_u8, v));
                };
                gc.function = gx;
            }
            GcAttr::PlaneMask => gc.plane_mask = read_u32(&mut at)?,
            GcAttr::Foreground => gc.foreground = read_u32(&mut at)?,
            GcAttr::Background => gc.background = read_u32(&mut at)?,
            GcAttr::LineWidth => gc.line_width = read_u16(&mut at)?,
            GcAttr::LineStyle => {
                let v = read_u8(&mut at)?;
                let Some(ls) = LineStyle::from_wire(v as u8) else {
                    return Err(GcError::BadValue(bit_u8, v));
                };
                gc.line_style = ls;
            }
            GcAttr::CapStyle => gc.cap_style = read_u8(&mut at)? as u8,
            GcAttr::JoinStyle => gc.join_style = read_u8(&mut at)? as u8,
            GcAttr::FillStyle => {
                let v = read_u8(&mut at)?;
                let Some(fs) = FillStyle::from_wire(v as u8) else {
                    return Err(GcError::BadValue(bit_u8, v));
                };
                gc.fill_style = fs;
            }
            GcAttr::FillRule => {
                let v = read_u8(&mut at)?;
                if v > 1 {
                    return Err(GcError::BadValue(bit_u8, v));
                }
                gc.fill_rule = v as u8;
            }
            GcAttr::Tile => gc.tile = read_u32(&mut at)?,
            GcAttr::Stipple => gc.stipple = read_u32(&mut at)?,
            GcAttr::TileStippleXOrigin => {
                let v = read_u16(&mut at)?;
                gc.ts_origin.0 = v as u16 as i16 as i32;
            }
            GcAttr::TileStippleYOrigin => {
                let v = read_u16(&mut at)?;
                gc.ts_origin.1 = v as u16 as i16 as i32;
            }
            GcAttr::Font => gc.font = read_u32(&mut at)?,
            GcAttr::SubwindowMode => {
                let v = read_u8(&mut at)?;
                let Some(m) = SubwindowMode::from_wire(v as u8) else {
                    return Err(GcError::BadValue(bit_u8, v));
                };
                gc.subwindow_mode = m;
            }
            GcAttr::GraphicsExposures => gc.graphics_exposures = read_u8(&mut at)? != 0,
            GcAttr::ClipXOrigin => {
                let v = read_u16(&mut at)?;
                gc.clip_origin.0 = v as u16 as i16 as i32;
            }
            GcAttr::ClipYOrigin => {
                let v = read_u16(&mut at)?;
                gc.clip_origin.1 = v as u16 as i16 as i32;
            }
            GcAttr::ClipMask => gc.clip_mask = read_u32(&mut at)?,
            GcAttr::DashOffset => gc.dash_offset = read_u16(&mut at)?,
            GcAttr::Dashes => {
                // LISTofCARD8: the length rides on the request itself;
                // the caller pre-slices this field (the dispatcher
                // passes the value list only when Dashes is the last
                // field, which the protocol guarantees).
                let len = payload.len().saturating_sub(at);
                if len == 0 {
                    return Err(GcError::BadValue(bit_u8, 0));
                }
                gc.dashes = payload[at..at + len].to_vec();
                at += len;
            }
            GcAttr::ArcMode => {
                let v = read_u8(&mut at)?;
                if v > 1 {
                    return Err(GcError::BadValue(bit_u8, v));
                }
                gc.arc_mode = v as u8;
            }
        }
    }
    Ok(())
}

impl GcAttr {
    /// The bit's mask constant.
    #[must_use]
    pub const fn mask(self) -> u32 {
        1u32 << (self as u32)
    }

    /// Parse a bit index.
    #[must_use]
    pub const fn from(bit: u32) -> GcAttr {
        match bit {
            0 => GcAttr::Function,
            1 => GcAttr::PlaneMask,
            2 => GcAttr::Foreground,
            3 => GcAttr::Background,
            4 => GcAttr::LineWidth,
            5 => GcAttr::LineStyle,
            6 => GcAttr::CapStyle,
            7 => GcAttr::JoinStyle,
            8 => GcAttr::FillStyle,
            9 => GcAttr::FillRule,
            10 => GcAttr::Tile,
            11 => GcAttr::Stipple,
            12 => GcAttr::TileStippleXOrigin,
            13 => GcAttr::TileStippleYOrigin,
            14 => GcAttr::Font,
            15 => GcAttr::SubwindowMode,
            16 => GcAttr::GraphicsExposures,
            17 => GcAttr::ClipXOrigin,
            18 => GcAttr::ClipYOrigin,
            19 => GcAttr::ClipMask,
            20 => GcAttr::DashOffset,
            21 => GcAttr::Dashes,
            _ => GcAttr::ArcMode,
        }
    }
}

/// The connection's GC table.
#[derive(Clone, Debug, Default)]
pub struct GcStore {
    gcs: BTreeMap<u32, Gc>,
}

impl GcStore {
    /// Create a GC with defaults (id collisions reject).
    ///
    /// # Errors
    /// The XID back when it is already in use.
    ///
    /// # Panics
    /// Never in practice: the `expect` follows an insert that cannot
    /// fail.
    pub fn create(&mut self, id: u32) -> Result<&mut Gc, u32> {
        if self.gcs.contains_key(&id) {
            return Err(id);
        }
        self.gcs.insert(id, Gc::new(id));
        Ok(self.gcs.get_mut(&id).expect("just inserted"))
    }

    /// Look one up for mutation.
    pub fn get_mut(&mut self, id: u32) -> Option<&mut Gc> {
        self.gcs.get_mut(&id)
    }

    /// Look one up.
    #[must_use]
    pub fn get(&self, id: u32) -> Option<&Gc> {
        self.gcs.get(&id)
    }

    /// Free one; true when it existed.
    pub fn free(&mut self, id: u32) -> bool {
        self.gcs.remove(&id).is_some()
    }

    /// Copy `src`'s selected fields into `dst` (CopyGC). The mask bits
    /// pick fields; an empty mask copies nothing.
    pub fn copy_fields(dst: &mut Gc, src: &Gc, mask: u32) {
        if mask & GcAttr::Function.mask() != 0 {
            dst.function = src.function;
        }
        if mask & GcAttr::PlaneMask.mask() != 0 {
            dst.plane_mask = src.plane_mask;
        }
        if mask & GcAttr::Foreground.mask() != 0 {
            dst.foreground = src.foreground;
        }
        if mask & GcAttr::Background.mask() != 0 {
            dst.background = src.background;
        }
        if mask & GcAttr::LineWidth.mask() != 0 {
            dst.line_width = src.line_width;
        }
        if mask & GcAttr::LineStyle.mask() != 0 {
            dst.line_style = src.line_style;
        }
        if mask & GcAttr::CapStyle.mask() != 0 {
            dst.cap_style = src.cap_style;
        }
        if mask & GcAttr::JoinStyle.mask() != 0 {
            dst.join_style = src.join_style;
        }
        if mask & GcAttr::FillStyle.mask() != 0 {
            dst.fill_style = src.fill_style;
        }
        if mask & GcAttr::FillRule.mask() != 0 {
            dst.fill_rule = src.fill_rule;
        }
        if mask & GcAttr::Tile.mask() != 0 {
            dst.tile = src.tile;
        }
        if mask & GcAttr::Stipple.mask() != 0 {
            dst.stipple = src.stipple;
        }
        if mask & (GcAttr::TileStippleXOrigin.mask() | GcAttr::TileStippleYOrigin.mask()) != 0 {
            dst.ts_origin = src.ts_origin;
        }
        if mask & GcAttr::Font.mask() != 0 {
            dst.font = src.font;
        }
        if mask & GcAttr::SubwindowMode.mask() != 0 {
            dst.subwindow_mode = src.subwindow_mode;
        }
        if mask & GcAttr::GraphicsExposures.mask() != 0 {
            dst.graphics_exposures = src.graphics_exposures;
        }
        if mask & (GcAttr::ClipXOrigin.mask() | GcAttr::ClipYOrigin.mask()) != 0 {
            dst.clip_origin = src.clip_origin;
        }
        if mask & GcAttr::ClipMask.mask() != 0 {
            dst.clip_mask = src.clip_mask;
        }
        if mask & GcAttr::DashOffset.mask() != 0 {
            dst.dash_offset = src.dash_offset;
        }
        if mask & GcAttr::Dashes.mask() != 0 {
            dst.dashes.clone_from(&src.dashes);
        }
        if mask & GcAttr::ArcMode.mask() != 0 {
            dst.arc_mode = src.arc_mode;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(words: &[u32]) -> Vec<u8> {
        let mut v = Vec::new();
        for w in words {
            v.extend_from_slice(&w.to_le_bytes());
        }
        v
    }

    #[test]
    fn defaults_match_the_protocol() {
        let gc = Gc::new(7);
        assert_eq!(gc.function, Gx::Copy);
        assert_eq!(gc.plane_mask, 0x00ff_ffff);
        assert_eq!(gc.line_width, 0);
        assert_eq!(gc.line_style, LineStyle::Solid);
        assert_eq!(gc.dashes, vec![4, 4]);
        assert_eq!(gc.subwindow_mode, SubwindowMode::ClipByChildren);
        assert_eq!(gc.arc_mode, 1, "ArcPieSlice is the default");
        assert!(gc.graphics_exposures);
        assert!(!gc.unsupported());
    }

    #[test]
    fn value_list_applies_fields_in_bit_order() {
        // function=GXxor(6), foreground=0xff0000, line-width=3 — each
        // field a 4-byte slot (function/line-width in the low bytes).
        let mask = GcAttr::Function.mask() | GcAttr::Foreground.mask() | GcAttr::LineWidth.mask();
        let p = payload(&[6, 0x00ff_0000, 3]);
        let mut gc = Gc::new(1);
        apply_value_list(&mut gc, mask, &p).unwrap();
        assert_eq!(gc.function, Gx::Xor);
        assert_eq!(gc.foreground, 0x00ff_0000);
        assert_eq!(gc.line_width, 3);
    }

    #[test]
    fn short_payload_rejects() {
        let mask = GcAttr::Foreground.mask() | GcAttr::Background.mask();
        let p = payload(&[1]);
        let mut gc = Gc::new(1);
        assert_eq!(apply_value_list(&mut gc, mask, &p), Err(GcError::Short));
    }

    #[test]
    fn unknown_bits_reject() {
        let mut gc = Gc::new(1);
        assert_eq!(
            apply_value_list(&mut gc, 1 << 23, &[]),
            Err(GcError::UnknownBits(1 << 23))
        );
    }

    #[test]
    fn bad_enum_values_reject() {
        // function=16 is not a GX.
        let mut gc = Gc::new(1);
        let p = payload(&[16]);
        assert_eq!(
            apply_value_list(&mut gc, GcAttr::Function.mask(), &p),
            Err(GcError::BadValue(0, 16))
        );
        // fill-style=4.
        let p = payload(&[4]);
        assert_eq!(
            apply_value_list(&mut gc, GcAttr::FillStyle.mask(), &p),
            Err(GcError::BadValue(8, 4))
        );
        // subwindow-mode=2.
        let p = payload(&[2]);
        assert_eq!(
            apply_value_list(&mut gc, GcAttr::SubwindowMode.mask(), &p),
            Err(GcError::BadValue(15, 2))
        );
    }

    #[test]
    fn clip_mask_marks_unsupported() {
        let mut gc = Gc::new(1);
        gc.clip_mask = 0x99;
        assert!(gc.unsupported());
        gc.clip_mask = 0;
        gc.fill_style = FillStyle::Tiled;
        assert!(gc.unsupported());
    }

    #[test]
    fn dashes_take_the_remaining_bytes() {
        // Dashes is the last field: the rest of the payload is the list.
        // (DashOffset occupies its own 4-byte slot first.)
        let mask = GcAttr::DashOffset.mask() | GcAttr::Dashes.mask();
        let mut p = Vec::new();
        p.extend_from_slice(&2u32.to_le_bytes());
        p.extend_from_slice(&[1, 2, 3, 4]); // list + implicit pad
        let mut gc = Gc::new(1);
        apply_value_list(&mut gc, mask, &p).unwrap();
        assert_eq!(gc.dash_offset, 2);
        assert_eq!(gc.dashes, vec![1, 2, 3, 4]);
        // Empty dash list is invalid.
        let p2 = Vec::new();
        let mut gc2 = Gc::new(1);
        assert_eq!(
            apply_value_list(&mut gc2, GcAttr::Dashes.mask(), &p2),
            Err(GcError::BadValue(21, 0))
        );
    }

    #[test]
    fn copy_fields_is_selective() {
        let mut src = Gc::new(1);
        src.foreground = 0x11;
        src.background = 0x22;
        let mut dst = Gc::new(2);
        GcStore::copy_fields(&mut dst, &src, GcAttr::Foreground.mask());
        assert_eq!(dst.foreground, 0x11);
        assert_eq!(dst.background, 1); // untouched default
    }

    #[test]
    fn store_create_and_free() {
        let mut s = GcStore::default();
        assert!(s.create(5).is_ok());
        assert!(s.create(5).is_err());
        assert!(s.get(5).is_some());
        assert!(s.free(5));
        assert!(!s.free(5));
        assert!(s.get(5).is_none());
    }
}
