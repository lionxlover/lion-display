//! D-Bus wire codec — marshaling and parsing, little-endian, zero
//! dependencies (the ldp-input evdev philosophy: bytes from the spec,
//! not a binding).
//!
//! Subset: message framing (endianness/type/flags/version, body
//! length, serial, header `a(yv)` with fields 1-9) and the body types
//! `y b n q i u x t d s o g v a ( ) { }` with the full alignment
//! discipline — basics at 1/2/4/8, structs and dict entries at 8,
//! variants at 8 as array elements (their signature at 1), arrays
//! aligned to their element type with lengths that exclude the
//! pre-data padding but include inter-element padding.
//!
//! Every parse is bounds- and alignment-checked before allocation or
//! indexing: truncated frames, bad endianness/version, alignment
//! violations, and overlong length fields fail with typed errors.

/// A marshaled body value.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum DbusValue {
    /// `y`
    Byte(u8),
    /// `b` (wire u32, strictly 0/1)
    Bool(bool),
    /// `n`
    I16(i16),
    /// `q`
    U16(u16),
    /// `i`
    I32(i32),
    /// `u`
    U32(u32),
    /// `x`
    I64(i64),
    /// `t`
    U64(u64),
    /// `d`
    Double(f64),
    /// `s`
    Str(String),
    /// `o` (validated: starts '/', no trailing '/', `[A-Za-z0-9_/]`)
    Path(String),
    /// `g` (length ≤ 255)
    Signature(String),
    /// `v` — carries its own signature on the wire
    Variant(Box<DbusValue>),
    /// `a T` — homogeneous; the empty array loses its element type
    Array(Vec<DbusValue>),
    /// `( ... )`
    Struct(Vec<DbusValue>),
    /// `a { K : V }` — entry pairs
    Dict(Vec<(DbusValue, DbusValue)>),
    /// `h` — index into the message's unix-fd list
    Fd(u32),
}

/// Message kind (wire byte 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum MessageType {
    /// 1
    MethodCall,
    /// 2
    MethodReturn,
    /// 3
    Error,
    /// 4
    Signal,
}

impl MessageType {
    /// Wire byte.
    #[must_use]
    pub const fn to_wire(self) -> u8 {
        match self {
            Self::MethodCall => 1,
            Self::MethodReturn => 2,
            Self::Error => 3,
            Self::Signal => 4,
        }
    }

    /// Parse a wire byte.
    #[must_use]
    pub const fn from_wire(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::MethodCall),
            2 => Some(Self::MethodReturn),
            3 => Some(Self::Error),
            4 => Some(Self::Signal),
            _ => None,
        }
    }
}

/// Codec failures.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DbusError {
    /// Fewer bytes than the fixed 16-byte header.
    TooShort,
    /// Not little-endian.
    BadEndianness,
    /// Protocol version != 1.
    BadVersion,
    /// Unknown message type byte or serial 0.
    BadType,
    /// An alignment rule was violated.
    BadAlignment,
    /// A length field exceeds the remaining bytes.
    Overlong,
    /// Data ended mid-value.
    Truncated,
    /// A type character outside the subset.
    UnknownType,
    /// A body value disagrees with the signature.
    SignatureMismatch,
    /// Invalid UTF-8.
    InvalidUtf8,
    /// An invalid object path / signature string / bool word.
    InvalidString,
}

impl std::fmt::Display for DbusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::TooShort => "too short",
            Self::BadEndianness => "bad endianness",
            Self::BadVersion => "bad version",
            Self::BadType => "bad type",
            Self::BadAlignment => "bad alignment",
            Self::Overlong => "overlong length",
            Self::Truncated => "truncated",
            Self::UnknownType => "unknown type",
            Self::SignatureMismatch => "signature mismatch",
            Self::InvalidUtf8 => "invalid utf-8",
            Self::InvalidString => "invalid string",
        };
        f.write_str(s)
    }
}

impl std::error::Error for DbusError {}

/// One complete message.
#[derive(Clone, Debug, PartialEq)]
pub struct DbusMessage {
    /// Message kind.
    pub kind: MessageType,
    /// Flags (bit 0 = no-reply-expected, bit 1 = no-auto-start).
    pub flags: u8,
    /// Sender serial (never 0).
    pub serial: u32,
    /// Field 1: object path.
    pub path: Option<String>,
    /// Field 2: interface.
    pub interface: Option<String>,
    /// Field 3: member.
    pub member: Option<String>,
    /// Field 4: error name.
    pub error_name: Option<String>,
    /// Field 5: reply serial.
    pub reply_serial: Option<u32>,
    /// Field 6: destination.
    pub destination: Option<String>,
    /// Field 7: sender (bus-assigned).
    pub sender: Option<String>,
    /// Field 8: body signature (empty = no body).
    pub signature: String,
    /// Body values (must match `signature`).
    pub body: Vec<DbusValue>,
    /// Field 9: unix-fd count riding with the message.
    pub unix_fds: u32,
}

// ---------------------------------------------------------------------------
// Marshaling
// ---------------------------------------------------------------------------

fn align_up(pos: usize, align: usize) -> usize {
    if align <= 1 {
        pos
    } else {
        pos.div_ceil(align) * align
    }
}

fn pad_to(out: &mut Vec<u8>, align: usize) {
    if align > 1 {
        while out.len() % align != 0 {
            out.push(0);
        }
    }
}

/// Alignment of a value when it appears as a container element
/// (variants are 8 per the spec table; their inner signature is 1).
fn value_align(v: &DbusValue) -> usize {
    match v {
        DbusValue::Byte(_) | DbusValue::Signature(_) => 1,
        DbusValue::I16(_) | DbusValue::U16(_) => 2,
        DbusValue::Bool(_)
        | DbusValue::I32(_)
        | DbusValue::U32(_)
        | DbusValue::Fd(_)
        | DbusValue::Str(_)
        | DbusValue::Path(_) => 4,
        DbusValue::I64(_)
        | DbusValue::U64(_)
        | DbusValue::Double(_)
        | DbusValue::Struct(_)
        | DbusValue::Dict(_)
        | DbusValue::Variant(_) => 8,
        DbusValue::Array(els) => els.first().map_or(1, value_align),
    }
}

impl DbusValue {
    /// The complete-type signature (variant: `v` + inner signature;
    /// empty arrays lose their element type — a documented subset
    /// limit).
    #[must_use]
    pub fn signature(&self) -> String {
        match self {
            Self::Byte(_) => "y".into(),
            Self::Bool(_) => "b".into(),
            Self::I16(_) => "n".into(),
            Self::U16(_) => "q".into(),
            Self::I32(_) => "i".into(),
            Self::U32(_) => "u".into(),
            Self::I64(_) => "x".into(),
            Self::U64(_) => "t".into(),
            Self::Double(_) => "d".into(),
            Self::Str(_) => "s".into(),
            Self::Path(_) => "o".into(),
            Self::Signature(_) => "g".into(),
            Self::Variant(_) => "v".into(),
            Self::Array(els) => els
                .first()
                .map_or_else(String::new, |e| format!("a{}", e.signature())),
            Self::Struct(fields) => {
                format!(
                    "({})",
                    fields.iter().map(Self::signature).collect::<String>()
                )
            }
            Self::Dict(entries) => entries.first().map_or_else(String::new, |(k, v)| {
                format!("a{{{}:{}}}", k.signature(), v.signature())
            }),
            Self::Fd(_) => "h".into(),
        }
    }

    /// Marshal at the current end of `out` (self-aligning start).
    fn marshal(&self, out: &mut Vec<u8>) {
        match self {
            Self::Byte(b) => out.push(*b),
            Self::Bool(b) => {
                pad_to(out, 4);
                out.extend_from_slice(&u32::from(*b).to_le_bytes());
            }
            Self::I16(v) => {
                pad_to(out, 2);
                out.extend_from_slice(&v.to_le_bytes());
            }
            Self::U16(v) => {
                pad_to(out, 2);
                out.extend_from_slice(&v.to_le_bytes());
            }
            Self::I32(v) => {
                pad_to(out, 4);
                out.extend_from_slice(&v.to_le_bytes());
            }
            Self::U32(v) | Self::Fd(v) => {
                pad_to(out, 4);
                out.extend_from_slice(&v.to_le_bytes());
            }
            Self::I64(v) => {
                pad_to(out, 8);
                out.extend_from_slice(&v.to_le_bytes());
            }
            Self::U64(v) => {
                pad_to(out, 8);
                out.extend_from_slice(&v.to_le_bytes());
            }
            Self::Double(v) => {
                pad_to(out, 8);
                out.extend_from_slice(&v.to_le_bytes());
            }
            Self::Str(s) | Self::Path(s) => {
                pad_to(out, 4);
                out.extend_from_slice(&(s.len() as u32).to_le_bytes());
                out.extend_from_slice(s.as_bytes());
                out.push(0);
            }
            Self::Signature(s) => {
                out.push(s.len() as u8);
                out.extend_from_slice(s.as_bytes());
                out.push(0);
            }
            Self::Variant(inner) => {
                pad_to(out, 8);
                let sig = inner.signature();
                out.push(sig.len() as u8);
                out.extend_from_slice(sig.as_bytes());
                out.push(0);
                inner.marshal(out);
            }
            Self::Array(els) => {
                pad_to(out, 4);
                let len_at = out.len();
                out.extend_from_slice(&[0; 4]);
                let elem_align = els.first().map_or(1, value_align);
                let data_start = align_up(out.len(), elem_align);
                while out.len() < data_start {
                    out.push(0);
                }
                let before = out.len();
                for e in els {
                    pad_to(out, value_align(e));
                    e.marshal(out);
                }
                let arr_len = (out.len() - before) as u32;
                out[len_at..len_at + 4].copy_from_slice(&arr_len.to_le_bytes());
            }
            Self::Struct(fields) => {
                pad_to(out, 8);
                for f in fields {
                    f.marshal(out);
                }
            }
            Self::Dict(entries) => {
                pad_to(out, 4);
                let len_at = out.len();
                out.extend_from_slice(&[0; 4]);
                while out.len() % 8 != 0 {
                    out.push(0);
                }
                let before = out.len();
                for (k, v) in entries {
                    pad_to(out, 8);
                    k.marshal(out);
                    v.marshal(out);
                }
                let arr_len = (out.len() - before) as u32;
                out[len_at..len_at + 4].copy_from_slice(&arr_len.to_le_bytes());
            }
        }
    }
}

impl DbusMessage {
    /// Marshal the complete message, little-endian, header fields in
    /// ascending code order (canonical — golden tests pin it).
    #[must_use]
    pub fn marshal(&self) -> Vec<u8> {
        let mut body = Vec::new();
        for v in &self.body {
            pad_to(&mut body, value_align(v));
            v.marshal(&mut body);
        }
        pad_to(&mut body, 8);

        let mut array = Vec::new();
        // One a(yv) element: struct(byte code, variant sig+value). The
        // variant's signature rides unaligned right after the code
        // (the header-array convention), the contained value self-aligns.
        let mut field = |code: u8, sig: &str, value: &DbusValue| {
            pad_to(&mut array, 8);
            array.push(code);
            array.push(sig.len() as u8);
            array.extend_from_slice(sig.as_bytes());
            array.push(0);
            value.marshal(&mut array);
        };
        if let Some(p) = &self.path {
            field(1, "o", &DbusValue::Path(p.clone()));
        }
        if let Some(i) = &self.interface {
            field(2, "s", &DbusValue::Str(i.clone()));
        }
        if let Some(m) = &self.member {
            field(3, "s", &DbusValue::Str(m.clone()));
        }
        if let Some(e) = &self.error_name {
            field(4, "s", &DbusValue::Str(e.clone()));
        }
        if let Some(r) = self.reply_serial {
            field(5, "u", &DbusValue::U32(r));
        }
        if let Some(d) = &self.destination {
            field(6, "s", &DbusValue::Str(d.clone()));
        }
        if let Some(s) = &self.sender {
            field(7, "s", &DbusValue::Str(s.clone()));
        }
        if !self.signature.is_empty() {
            field(8, "g", &DbusValue::Signature(self.signature.clone()));
        }
        if self.unix_fds > 0 {
            field(9, "u", &DbusValue::U32(self.unix_fds));
        }

        let mut out = Vec::with_capacity(16 + array.len() + body.len());
        out.push(b'l');
        out.push(self.kind.to_wire());
        out.push(self.flags);
        out.push(1);
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.serial.to_le_bytes());
        out.extend_from_slice(&(array.len() as u32).to_le_bytes());
        out.extend_from_slice(&array);
        pad_to(&mut out, 8);
        out.extend_from_slice(&body);
        out
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Parse one message from `bytes`.
///
/// # Errors
///
/// [`DbusError`] for framing, alignment, signature, or UTF-8
/// violations. Never panics, never indexes out of bounds.
pub fn parse_message(bytes: &[u8]) -> Result<DbusMessage, DbusError> {
    if bytes.len() < 16 {
        return Err(DbusError::TooShort);
    }
    if bytes[0] != b'l' {
        return Err(DbusError::BadEndianness);
    }
    let kind = MessageType::from_wire(bytes[1]).ok_or(DbusError::BadType)?;
    let flags = bytes[2];
    if bytes[3] != 1 {
        return Err(DbusError::BadVersion);
    }
    let body_len = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    let serial = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    if serial == 0 {
        return Err(DbusError::BadType);
    }
    let array_len = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]) as usize;
    if array_len > bytes.len().saturating_sub(16) {
        return Err(DbusError::Overlong);
    }
    let array_end = 16 + array_len;
    if body_len > bytes.len().saturating_sub(array_end) {
        return Err(DbusError::Overlong);
    }
    let body_start = align_up(array_end, 8);
    if body_start + body_len > bytes.len() {
        return Err(DbusError::Overlong);
    }

    let mut msg = DbusMessage {
        kind,
        flags,
        serial,
        path: None,
        interface: None,
        member: None,
        error_name: None,
        reply_serial: None,
        destination: None,
        sender: None,
        signature: String::new(),
        body: Vec::new(),
        unix_fds: 0,
    };

    // Header array: struct(byte, variant) elements, 8-aligned.
    let mut cur = Cursor {
        bytes,
        pos: 16,
        limit: array_end,
    };
    while cur.pos < cur.limit {
        cur.skip_align(8)?;
        if cur.pos >= cur.limit {
            break;
        }
        let code = cur.take(1)?[0];
        let sig = cur.take_signature()?;
        let value = cur.parse_value(&mut sig.as_str())?;
        match code {
            1 => msg.path = Some(expect_string(value)?),
            2 => msg.interface = Some(expect_string(value)?),
            3 => msg.member = Some(expect_string(value)?),
            4 => msg.error_name = Some(expect_string(value)?),
            5 => {
                msg.reply_serial = Some(match value {
                    DbusValue::U32(v) => v,
                    _ => return Err(DbusError::SignatureMismatch),
                });
            }
            6 => msg.destination = Some(expect_string(value)?),
            7 => msg.sender = Some(expect_string(value)?),
            8 => msg.signature = expect_string(value)?,
            9 => {
                msg.unix_fds = match value {
                    DbusValue::U32(v) => v,
                    _ => return Err(DbusError::SignatureMismatch),
                }
            }
            _ => {} // unknown fields skipped (forward compat)
        }
    }

    // Body per the header signature.
    cur.pos = body_start;
    cur.limit = body_start + body_len;
    let mut sig_rest: &str = &msg.signature;
    while !sig_rest.is_empty() {
        msg.body.push(cur.parse_value(&mut sig_rest)?);
    }
    if cur.pos != cur.limit && align_up(cur.pos, 8) != cur.limit {
        return Err(DbusError::Truncated);
    }
    Ok(msg)
}

fn expect_string(v: DbusValue) -> Result<String, DbusError> {
    match v {
        DbusValue::Str(s) | DbusValue::Path(s) | DbusValue::Signature(s) => Ok(s),
        _ => Err(DbusError::SignatureMismatch),
    }
}

/// Bounds-checked cursor over one region.
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
    limit: usize,
}

impl<'a> Cursor<'a> {
    fn skip_align(&mut self, align: usize) -> Result<(), DbusError> {
        if align > 1 {
            self.pos = align_up(self.pos, align);
        }
        if self.pos > self.limit {
            return Err(DbusError::Overlong);
        }
        Ok(())
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], DbusError> {
        let end = self.pos.checked_add(n).ok_or(DbusError::Overlong)?;
        if end > self.limit {
            return Err(DbusError::Truncated);
        }
        let out = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    fn take_signature(&mut self) -> Result<String, DbusError> {
        let len = self.take(1)?[0] as usize;
        let raw = self.take(len + 1)?;
        if raw.last() != Some(&0) {
            return Err(DbusError::InvalidString);
        }
        String::from_utf8(raw[..raw.len() - 1].to_vec()).map_err(|_| DbusError::InvalidUtf8)
    }

    /// Parse one complete value from the front of `sig`, consuming
    /// the signature characters it used (basics one char, variants
    /// their own wire signature, arrays `a` + element type, structs
    /// the balanced group).
    fn parse_value(&mut self, sig: &mut &str) -> Result<DbusValue, DbusError> {
        self.parse_one(sig)
    }

    /// Parse one complete type from the front of `rest`, consuming it.
    ///
    /// One cohesive type-table match; splitting it would scatter the
    /// marshaling rules.
    #[allow(clippy::too_many_lines)]
    fn parse_one(&mut self, rest: &mut &str) -> Result<DbusValue, DbusError> {
        let c = rest.chars().next().ok_or(DbusError::UnknownType)?;
        *rest = &rest[1..];
        let value = match c {
            'y' => DbusValue::Byte(self.take(1)?[0]),
            'b' => {
                self.skip_align(4)?;
                let raw =
                    u32::from_le_bytes(self.take(4)?.try_into().map_err(|_| DbusError::Truncated)?);
                if raw > 1 {
                    return Err(DbusError::InvalidString);
                }
                DbusValue::Bool(raw == 1)
            }
            'n' => {
                self.skip_align(2)?;
                DbusValue::I16(i16::from_le_bytes(
                    self.take(2)?.try_into().map_err(|_| DbusError::Truncated)?,
                ))
            }
            'q' => {
                self.skip_align(2)?;
                DbusValue::U16(u16::from_le_bytes(
                    self.take(2)?.try_into().map_err(|_| DbusError::Truncated)?,
                ))
            }
            'i' => {
                self.skip_align(4)?;
                DbusValue::I32(i32::from_le_bytes(
                    self.take(4)?.try_into().map_err(|_| DbusError::Truncated)?,
                ))
            }
            'u' | 'h' => {
                self.skip_align(4)?;
                let v =
                    u32::from_le_bytes(self.take(4)?.try_into().map_err(|_| DbusError::Truncated)?);
                if c == 'u' {
                    DbusValue::U32(v)
                } else {
                    DbusValue::Fd(v)
                }
            }
            'x' => {
                self.skip_align(8)?;
                DbusValue::I64(i64::from_le_bytes(
                    self.take(8)?.try_into().map_err(|_| DbusError::Truncated)?,
                ))
            }
            't' => {
                self.skip_align(8)?;
                DbusValue::U64(u64::from_le_bytes(
                    self.take(8)?.try_into().map_err(|_| DbusError::Truncated)?,
                ))
            }
            'd' => {
                self.skip_align(8)?;
                DbusValue::Double(f64::from_le_bytes(
                    self.take(8)?.try_into().map_err(|_| DbusError::Truncated)?,
                ))
            }
            's' | 'o' | 'g' => {
                let len_bytes = if c == 'g' { 1 } else { 4 };
                self.skip_align(len_bytes)?;
                let len = if c == 'g' {
                    u32::from(self.take(1)?[0])
                } else {
                    u32::from_le_bytes(self.take(4)?.try_into().map_err(|_| DbusError::Truncated)?)
                };
                let raw = self.take(len as usize + 1)?;
                if raw.last() != Some(&0) {
                    return Err(DbusError::InvalidString);
                }
                let s = std::str::from_utf8(&raw[..raw.len() - 1])
                    .map_err(|_| DbusError::InvalidUtf8)?;
                match c {
                    's' => DbusValue::Str(s.to_owned()),
                    'o' => {
                        validate_path(s)?;
                        DbusValue::Path(s.to_owned())
                    }
                    _ => {
                        if s.len() > 255 {
                            return Err(DbusError::InvalidString);
                        }
                        DbusValue::Signature(s.to_owned())
                    }
                }
            }
            'v' => {
                self.skip_align(8)?;
                let inner_sig = self.take_signature()?;
                let inner = self.parse_value(&mut inner_sig.as_str())?;
                DbusValue::Variant(Box::new(inner))
            }
            'a' => {
                self.skip_align(4)?;
                let arr_len =
                    u32::from_le_bytes(self.take(4)?.try_into().map_err(|_| DbusError::Truncated)?)
                        as usize;
                let elem_sig = complete_type_prefix(rest).ok_or(DbusError::UnknownType)?;
                *rest = &rest[elem_sig.len()..];
                let elem_align = signature_alignment(&elem_sig);
                self.skip_align(elem_align)?;
                let data_start = self.pos;
                if data_start.checked_add(arr_len).ok_or(DbusError::Overlong)? > self.limit {
                    return Err(DbusError::Overlong);
                }
                let saved_limit = self.limit;
                self.limit = data_start + arr_len;
                let value = if let Some(inner) =
                    elem_sig.strip_prefix('{').and_then(|s| s.strip_suffix('}'))
                {
                    let (ksig, vsig) = split_dict(inner)?;
                    let mut entries = Vec::new();
                    while self.pos < self.limit {
                        self.skip_align(8)?;
                        if self.pos >= self.limit {
                            break;
                        }
                        let mut ks: &str = ksig;
                        let k = self.parse_value(&mut ks)?;
                        let mut vs: &str = vsig;
                        let v = self.parse_value(&mut vs)?;
                        entries.push((k, v));
                    }
                    DbusValue::Dict(entries)
                } else {
                    let mut elements = Vec::new();
                    while self.pos < self.limit {
                        self.skip_align(elem_align)?;
                        if self.pos >= self.limit {
                            break;
                        }
                        let mut es: &str = &elem_sig;
                        elements.push(self.parse_value(&mut es)?);
                    }
                    DbusValue::Array(elements)
                };
                self.limit = saved_limit;
                value
            }
            '(' => {
                self.skip_align(8)?;
                // The '(' is consumed; find the matching ')' in the rest.
                let mut depth = 0usize;
                let mut end = None;
                for (i, c) in rest.char_indices() {
                    match c {
                        '(' => depth += 1,
                        ')' => {
                            if depth == 0 {
                                end = Some(i);
                                break;
                            }
                            depth -= 1;
                        }
                        _ => {}
                    }
                }
                let i = end.ok_or(DbusError::UnknownType)?;
                let inner = rest[..i].to_owned();
                *rest = &rest[i + 1..];
                let mut fields = Vec::new();
                let mut inner_rest = inner.as_str();
                while !inner_rest.is_empty() {
                    fields.push(self.parse_one(&mut inner_rest)?);
                }
                DbusValue::Struct(fields)
            }
            _ => return Err(DbusError::UnknownType),
        };
        Ok(value)
    }
}

fn validate_path(s: &str) -> Result<(), DbusError> {
    let ok = !s.is_empty()
        && s.starts_with('/')
        && (s.len() == 1 || !s.ends_with('/'))
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'/' || b == b'_');
    if ok {
        Ok(())
    } else {
        Err(DbusError::InvalidString)
    }
}

/// The first complete type in `sig` (basic char, `a` + one complete
/// type, or a balanced bracket group), as a substring.
fn complete_type_prefix(sig: &str) -> Option<String> {
    let mut depth = 0i32;
    for (i, c) in sig.char_indices() {
        match c {
            '(' | '{' => depth += 1,
            ')' | '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(sig[..=i].to_owned());
                }
                if depth < 0 {
                    return None;
                }
            }
            _ => {
                if depth == 0 {
                    return Some(sig[..=i].to_owned());
                }
            }
        }
    }
    None
}

/// Alignment of a complete type's data (arrays: element alignment).
fn signature_alignment(sig: &str) -> usize {
    let stripped = sig.trim_start_matches('a');
    match stripped.chars().next() {
        Some('(' | '{' | 'v') => 8,
        Some(c) => match c {
            'y' | 'g' => 1,
            'n' | 'q' => 2,
            'b' | 'i' | 'u' | 's' | 'o' | 'h' => 4,
            _ => 8,
        },
        None => 1,
    }
}

fn split_dict(inner: &str) -> Result<(&str, &str), DbusError> {
    let idx = inner.find(':').ok_or(DbusError::UnknownType)?;
    Ok((&inner[..idx], &inner[idx + 1..]))
}
