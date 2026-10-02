//! The CI-side GL backends: the reference evaluator and the recorder.
//!
//! [`RefGles`] executes the command vocabulary with the **reference
//! integer blend** — premultiplied source-over with `mul255` rounding
//! and the alpha-clamped channel sums, the exact rule the software
//! renderer and `ldp-gpu`'s mock EGL context share — so demanding
//! byte-equality between `GlesRenderer<RefGles>` and
//! `SoftwareRenderer` is a genuine cross-implementation oracle, not a
//! tautology.
//!
//! [`RecordingGles`] wraps any backend and records the command stream
//! as [`GlesCmd`] values; the stream goldens in `tests/gles_stream.rs`
//! pin exactly what the renderer emits for a known frame.

use ldp_core::geometry::Rect;

use super::api::{GlesApi, GlesError, GlesProgram, GlesTarget, GlesTexture};

/// One recorded command (the stream the renderer emits).
///
/// Values, not borrows: a recording is inspectable after the fact and
/// comparable with `assert_eq!` against a golden stream.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum GlesCmd {
    /// Program link.
    CreateProgram,
    /// Program teardown.
    DestroyProgram(u32),
    /// Target creation.
    CreateTarget(u32, u32, u32),
    /// Target teardown.
    DestroyTarget(u32),
    /// Texture upload (handle, width, height).
    CreateTexture(u32, u32, u32),
    /// Zero-copy texture from an imported EGLImage (handle, image):
    /// the DMA-BUF path — no CPU payload ever flows (Phase 31).
    CreateTextureFromImage(u32, u64),
    /// Texture teardown.
    DestroyTexture(u32),
    /// Pass open (target, scissor — `None` for the full target).
    BeginPass(u32, Option<Rect>),
    /// Clear to a premultiplied color.
    Clear(u8, u8, u8, u8),
    /// Layer draw (texture, destination, opacity).
    DrawLayer(u32, Rect, u8),
    /// Pass close.
    EndPass,
    /// Readback.
    Readback(u32),
}

/// One CPU-side texture: premultiplied ARGB words plus extent.
#[derive(Clone)]
struct RefTexture {
    width: u32,
    height: u32,
    words: Vec<u32>,
}

/// One CPU-side render target: premultiplied ARGB words.
#[derive(Clone)]
struct RefTarget {
    width: u32,
    height: u32,
    words: Vec<u32>,
}

/// The pass state a stream must respect.
struct PassState {
    target: GlesTarget,
    /// Scissor intersected with the target bounds, as pixel bounds.
    clip: (i64, i64, i64, i64), // x0, y0, x1, y1 (exclusive)
}

/// The reference GL evaluator: the command stream in, reference-blend
/// pixels out.
///
/// Handles are dense per-kind indices (`u32`, starting at 1) —
/// programs, textures, and targets number independently, exactly like
/// GL's separate object namespaces; destroyed slots are freed and
/// never reused (use-after-destroy is a typed error — the discipline
/// the real backend's object lifetime must keep too).
pub struct RefGles {
    programs: Vec<Option<()>>,
    textures: Vec<Option<RefTexture>>,
    targets: Vec<Option<RefTarget>>,
    pass: Option<PassState>,
    next_program: u32,
    next_texture: u32,
    next_target: u32,
}

impl core::fmt::Debug for RefGles {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RefGles")
            .field("targets", &self.targets.len())
            .field("textures", &self.textures.len())
            .field("pass_open", &self.pass.is_some())
            .finish_non_exhaustive()
    }
}

impl Default for RefGles {
    fn default() -> Self {
        Self::new()
    }
}

impl RefGles {
    /// A fresh evaluator with nothing allocated.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            programs: Vec::new(),
            textures: Vec::new(),
            targets: Vec::new(),
            pass: None,
            next_program: 1,
            next_texture: 1,
            next_target: 1,
        }
    }

    /// Mint the next dense handle for one object kind.
    fn mint(counter: &mut u32) -> u32 {
        let raw = *counter;
        *counter += 1;
        raw
    }

    fn slot_len_err(what: &'static str) -> GlesError {
        GlesError::Invalid(format!("payload does not match the {what} dimensions"))
    }

    /// Look up a live target slot by handle.
    fn target_of(&self, target: GlesTarget) -> Result<&RefTarget, GlesError> {
        self.targets
            .get(target.raw() as usize - 1)
            .and_then(Option::as_ref)
            .ok_or(GlesError::BadHandle("render-target handle"))
    }

    /// Look up a live target slot by handle, mutably.
    fn target_of_mut(&mut self, target: GlesTarget) -> Result<&mut RefTarget, GlesError> {
        self.targets
            .get_mut(target.raw() as usize - 1)
            .and_then(Option::as_mut)
            .ok_or(GlesError::BadHandle("render-target handle"))
    }
}

/// The reference blend: premultiplied source-over with the opacity
/// folded into the source alpha, `mul255` rounding, and channel sums
/// clamped to the result alpha — the pinned cross-implementation rule
/// (`ldp-renderer`'s composite rule; `ldp-gpu`'s mock EGL carries the
/// same function).
fn blend_word(src: u32, dst: u32, opacity: u8) -> u32 {
    let unpack = |w: u32| {
        [
            w & 0xFF,
            (w >> 8) & 0xFF,
            (w >> 16) & 0xFF,
            (w >> 24) & 0xFF,
        ]
    };
    let mul255 = |v: u32, a: u32| -> u32 { (v * a + 127) / 255 };
    let px = unpack(src);
    let d = unpack(dst);
    let q = u32::from(opacity);
    let a2 = mul255(px[3], q);
    let inv = 255 - a2;
    let oa = a2 + mul255(d[3], inv);
    let chan = |i: usize| (mul255(px[i], q) + mul255(d[i], inv)).min(oa);
    (oa << 24) | (chan(2) << 16) | (chan(1) << 8) | chan(0)
}

/// RGBA upload bytes (premultiplied, `[R, G, B, A]`) to a
/// premultiplied ARGB word (`A<<24 | R<<16 | G<<8 | B`).
fn word_from_rgba(bytes: [u8; 4]) -> u32 {
    u32::from(bytes[3]) << 24
        | u32::from(bytes[0]) << 16
        | u32::from(bytes[1]) << 8
        | u32::from(bytes[2])
}

/// A premultiplied ARGB word to RGBA bytes.
fn rgba_from_word(word: u32) -> [u8; 4] {
    [
        (word >> 16) as u8,
        (word >> 8) as u8,
        word as u8,
        (word >> 24) as u8,
    ]
}

/// The bounds a scissor rect clamps to on a `width` x `height` target.
fn clip_bounds(width: u32, height: u32, scissor: Rect) -> (i64, i64, i64, i64) {
    let x0 = i64::from(scissor.x).max(0);
    let y0 = i64::from(scissor.y).max(0);
    let x1 = i64::from(scissor.x)
        .saturating_add(i64::from(scissor.w))
        .min(i64::from(width));
    let y1 = i64::from(scissor.y)
        .saturating_add(i64::from(scissor.h))
        .min(i64::from(height));
    (x0, y0, x1.max(x0), y1.max(y0))
}

impl GlesApi for RefGles {
    fn create_program(&mut self, _vs: &str, _fs: &str) -> Result<GlesProgram, GlesError> {
        let raw = Self::mint(&mut self.next_program);
        self.programs.push(Some(()));
        Ok(GlesProgram::new(raw).expect("minted handles are nonzero"))
    }

    fn destroy_program(&mut self, program: GlesProgram) -> Result<(), GlesError> {
        let slot = self
            .programs
            .get_mut(program.raw() as usize - 1)
            .ok_or(GlesError::BadHandle("program handle"))?;
        if slot.take().is_none() {
            return Err(GlesError::BadHandle("program handle (already destroyed)"));
        }
        Ok(())
    }

    fn create_target(&mut self, width: u32, height: u32) -> Result<GlesTarget, GlesError> {
        if width == 0 || height == 0 {
            return Err(GlesError::Invalid("render target with a zero axis".into()));
        }
        let four = usize::try_from(
            width
                .checked_mul(height)
                .ok_or_else(|| GlesError::Invalid("render target extent overflows".into()))?,
        )
        .map_err(|_| GlesError::Invalid("render target extent overflows".into()))?;
        // Fresh targets start opaque black — the reference renderer's
        // own fresh-frame contract.
        let raw = Self::mint(&mut self.next_target);
        self.targets.push(Some(RefTarget {
            width,
            height,
            words: vec![0xFF00_0000; four],
        }));
        Ok(GlesTarget::new(raw).expect("minted handles are nonzero"))
    }

    fn destroy_target(&mut self, target: GlesTarget) -> Result<(), GlesError> {
        if self.pass.as_ref().is_some_and(|p| p.target == target) {
            return Err(GlesError::Invalid(
                "destroying the target an open pass renders into".into(),
            ));
        }
        let slot = self
            .targets
            .get_mut(target.raw() as usize - 1)
            .ok_or(GlesError::BadHandle("render-target handle"))?;
        if slot.take().is_none() {
            return Err(GlesError::BadHandle(
                "render-target handle (already destroyed)",
            ));
        }
        Ok(())
    }

    fn create_texture(
        &mut self,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<GlesTexture, GlesError> {
        if width == 0 || height == 0 {
            return Err(GlesError::Invalid("texture with a zero axis".into()));
        }
        let need = usize::try_from(width.saturating_mul(height).saturating_mul(4))
            .map_err(|_| GlesError::Invalid("texture extent overflows".into()))?;
        if rgba.len() != need {
            return Err(Self::slot_len_err("texture"));
        }
        let mut words = Vec::with_capacity(need / 4);
        for px in rgba.chunks_exact(4) {
            words.push(word_from_rgba([px[0], px[1], px[2], px[3]]));
        }
        let raw = Self::mint(&mut self.next_texture);
        self.textures.push(Some(RefTexture {
            width,
            height,
            words,
        }));
        Ok(GlesTexture::new(raw).expect("minted handles are nonzero"))
    }

    fn destroy_texture(&mut self, texture: GlesTexture) -> Result<(), GlesError> {
        let slot = self
            .textures
            .get_mut(texture.raw() as usize - 1)
            .ok_or(GlesError::BadHandle("texture handle"))?;
        if slot.take().is_none() {
            return Err(GlesError::BadHandle("texture handle (already destroyed)"));
        }
        Ok(())
    }

    fn create_texture_from_egl_image(&mut self, _image: u64) -> Result<GlesTexture, GlesError> {
        // The reference evaluator blends CPU-side words; an imported
        // image has none. The honest boundary: refuse — the command
        // stream (RecordingGles) and the real hardware carry the
        // zero-copy truth.
        Err(GlesError::Invalid(
            "the reference evaluator cannot blend an imported EGLImage".into(),
        ))
    }

    fn begin_pass(&mut self, target: GlesTarget, scissor: Option<Rect>) -> Result<(), GlesError> {
        if self.pass.is_some() {
            return Err(GlesError::Invalid(
                "begin_pass with a pass already open".into(),
            ));
        }
        let slot = self
            .targets
            .get(target.raw() as usize - 1)
            .ok_or(GlesError::BadHandle("render-target handle"))?;
        let (w, h) = match slot {
            Some(t) => (t.width, t.height),
            None => {
                return Err(GlesError::BadHandle(
                    "render-target handle (already destroyed)",
                ))
            }
        };
        let clip = match scissor {
            Some(rect) => clip_bounds(w, h, rect),
            None => (0, 0, i64::from(w), i64::from(h)),
        };
        self.pass = Some(PassState { target, clip });
        Ok(())
    }

    fn clear(&mut self, r: u8, g: u8, b: u8, a: u8) -> Result<(), GlesError> {
        let Some(pass) = self.pass.take() else {
            return Err(GlesError::Invalid("clear without an open pass".into()));
        };
        let word = u32::from(a) << 24 | u32::from(b) << 16 | u32::from(g) << 8 | u32::from(r);
        let result = (|| {
            let target = self.target_of_mut(pass.target)?;
            let (x0, y0, x1, y1) = pass.clip;
            for y in y0..y1 {
                for x in x0..x1 {
                    let di = (y * i64::from(target.width) + x) as usize;
                    target.words[di] = word;
                }
            }
            Ok(())
        })();
        self.pass = Some(pass);
        result
    }

    fn draw_layer(
        &mut self,
        texture: GlesTexture,
        dst: Rect,
        opacity: u8,
    ) -> Result<(), GlesError> {
        let Some(pass) = self.pass.take() else {
            return Err(GlesError::Invalid("draw_layer without an open pass".into()));
        };
        // The texture's pixels are read-only inputs; copy them out so
        // the target borrow below is the only live one (the reference
        // evaluator can afford the copy; the real backend has no such
        // conflict — the GPU owns both objects).
        let tex: Option<(u32, u32, Vec<u32>)> = self
            .textures
            .get(texture.raw() as usize - 1)
            .and_then(Option::as_ref)
            .map(|t| (t.width, t.height, t.words.clone()));
        let result = (|| {
            let Some((tw, th, words)) = tex else {
                return Err(GlesError::BadHandle("texture handle"));
            };
            let (tw, th) = (i64::from(tw), i64::from(th));
            let target = self.target_of_mut(pass.target)?;
            // Destination clipped to the target, then to the pass scissor.
            let (dx0, dy0, dx1, dy1) = clip_bounds(target.width, target.height, dst);
            let cx0 = dx0.max(pass.clip.0);
            let cy0 = dy0.max(pass.clip.1);
            let cx1 = dx1.min(pass.clip.2);
            let cy1 = dy1.min(pass.clip.3);
            // The texture's extent in f32 (the software path's own
            // precision — the same factors, the same floor, the same
            // ties).
            let (twf, thf) = (tw as f32, th as f32);
            let ax = twf / dst.w as f32;
            let ay = thf / dst.h as f32;
            for y in cy0..cy1 {
                // Phase 33: the continuous-nearest sampling rule — the
                // destination pixel's *center* maps through the inverse
                // scale onto the texture's continuous grid, then floors
                // to the sampled texel. Byte-identical to the 1:1 rule
                // at equal extents (a pixel center over its own texel
                // floors to it) and identical to the software path's
                // `Mapping::Scaled` math (the same f32 factors, the
                // same floor) — the scaled-upload oracle.
                let sy = (((y as f32) + 0.5 - dst.y as f32) * ay).floor();
                if sy < 0.0 || sy >= thf {
                    continue;
                }
                let sy = sy as usize;
                for x in cx0..cx1 {
                    let sx = (((x as f32) + 0.5 - dst.x as f32) * ax).floor();
                    if sx < 0.0 || sx >= twf {
                        continue;
                    }
                    let sx = sx as usize;
                    let src = words[sy * tw as usize + sx];
                    let di = (y * i64::from(target.width) + x) as usize;
                    target.words[di] = blend_word(src, target.words[di], opacity);
                }
            }
            Ok(())
        })();
        self.pass = Some(pass);
        result
    }

    fn end_pass(&mut self) -> Result<(), GlesError> {
        self.pass
            .take()
            .ok_or_else(|| GlesError::Invalid("end_pass without an open pass".into()))?;
        Ok(())
    }

    fn readback(&mut self, target: GlesTarget, rgba_out: &mut [u8]) -> Result<(), GlesError> {
        if self.pass.is_some() {
            return Err(GlesError::Invalid(
                "readback with a pass open (close it first)".into(),
            ));
        }
        let t = self.target_of(target)?;
        let need = t.words.len() * 4;
        if rgba_out.len() != need {
            return Err(GlesError::Invalid(format!(
                "readback buffer is {} bytes, the target holds {need}",
                rgba_out.len()
            )));
        }
        for (i, word) in t.words.iter().enumerate() {
            rgba_out[i * 4..i * 4 + 4].copy_from_slice(&rgba_from_word(*word));
        }
        Ok(())
    }
}

/// A stream recorder: every command logs, then delegates to the inner
/// backend.
///
/// The goldens assert on the log with the inner [`RefGles`] doing the
/// real evaluation — one artifact proving both *what* the renderer
/// emitted and *what it produced*.
///
/// The zero-copy exception (Phase 31): handles minted by
/// [`create_texture_from_egl_image`](GlesApi::create_texture_from_egl_image)
/// exist only in the log — the inner reference evaluator has no words
/// for them (the honest boundary), so their `draw_layer` and
/// `destroy_texture` record without evaluating. The command stream is
/// the recorder's truth for imported images; the real hardware carries
/// the pixels.
///
/// A shared handle onto a recorder's log — the recorder is usually
/// boxed inside a `GlesRenderer`, so the stream is read through this
/// handle after the run, not through the recorder itself. `Arc` +
/// `Mutex` (not `Rc`/`RefCell`) so the recorder satisfies the trait's
/// [`Send`](super::api::GlesApi) bound.
pub type SharedLog = std::sync::Arc<std::sync::Mutex<Vec<GlesCmd>>>;

/// A stream recorder: every command logs, then delegates to the inner
/// backend.
///
/// The goldens assert on the log with the inner [`RefGles`] doing the
/// real evaluation — one artifact proving both *what* the renderer
/// emitted and *what it produced*. Because the recorder is typically
/// boxed into the renderer, construct it with
/// [`RecordingGles::with_shared_log`] and keep the returned handle for
/// assertions after the frame.
pub struct RecordingGles {
    inner: RefGles,
    log: SharedLog,
    /// Texture handles minted for imported EGLImages — log-only
    /// objects the inner evaluator refuses (no CPU words), so their
    /// draws and destroys ride the record alone.
    image_textures: Vec<u32>,
}

impl core::fmt::Debug for RecordingGles {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RecordingGles")
            .field("commands", &self.log.lock().map_or(0, |l| l.len()))
            .finish_non_exhaustive()
    }
}

impl Default for RecordingGles {
    fn default() -> Self {
        Self::new()
    }
}

impl RecordingGles {
    /// A recorder over a fresh reference evaluator.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: RefGles::new(),
            log: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            image_textures: Vec::new(),
        }
    }

    /// A recorder whose log is shared with the caller: the returned
    /// handle keeps reading the stream after the recorder is boxed
    /// into a renderer.
    #[must_use]
    pub fn with_shared_log() -> (Self, SharedLog) {
        let log: SharedLog = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        (
            Self {
                inner: RefGles::new(),
                log: std::sync::Arc::clone(&log),
                image_textures: Vec::new(),
            },
            log,
        )
    }

    /// A recorder over a given evaluator.
    #[must_use]
    pub fn over(inner: RefGles) -> Self {
        Self {
            inner,
            log: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            image_textures: Vec::new(),
        }
    }

    /// The recorded command stream so far (cloned out under the
    /// lock; goldens compare the clone).
    #[must_use]
    pub fn log(&self) -> Vec<GlesCmd> {
        self.log.lock().map(|l| l.clone()).unwrap_or_default()
    }

    /// The inner evaluator (asserting pixels alongside the stream).
    #[must_use]
    pub const fn inner(&self) -> &RefGles {
        &self.inner
    }

    /// Record one command.
    fn record(&self, cmd: GlesCmd) {
        if let Ok(mut log) = self.log.lock() {
            log.push(cmd);
        }
    }
}

impl GlesApi for RecordingGles {
    fn create_program(&mut self, vs: &str, fs: &str) -> Result<GlesProgram, GlesError> {
        let out = self.inner.create_program(vs, fs)?;
        self.record(GlesCmd::CreateProgram);
        Ok(out)
    }

    fn destroy_program(&mut self, program: GlesProgram) -> Result<(), GlesError> {
        self.inner.destroy_program(program)?;
        self.record(GlesCmd::DestroyProgram(program.raw()));
        Ok(())
    }

    fn create_target(&mut self, width: u32, height: u32) -> Result<GlesTarget, GlesError> {
        let out = self.inner.create_target(width, height)?;
        self.record(GlesCmd::CreateTarget(out.raw(), width, height));
        Ok(out)
    }

    fn destroy_target(&mut self, target: GlesTarget) -> Result<(), GlesError> {
        self.inner.destroy_target(target)?;
        self.record(GlesCmd::DestroyTarget(target.raw()));
        Ok(())
    }

    fn create_texture(
        &mut self,
        width: u32,
        height: u32,
        rgba: &[u8],
    ) -> Result<GlesTexture, GlesError> {
        let out = self.inner.create_texture(width, height, rgba)?;
        self.record(GlesCmd::CreateTexture(out.raw(), width, height));
        Ok(out)
    }

    fn destroy_texture(&mut self, texture: GlesTexture) -> Result<(), GlesError> {
        // A minted image handle: log-only — the inner evaluator never
        // knew it (the honest boundary).
        if self.image_textures.contains(&texture.raw()) {
            self.image_textures.retain(|&raw| raw != texture.raw());
            self.record(GlesCmd::DestroyTexture(texture.raw()));
            return Ok(());
        }
        self.inner.destroy_texture(texture)?;
        self.record(GlesCmd::DestroyTexture(texture.raw()));
        Ok(())
    }

    fn create_texture_from_egl_image(&mut self, image: u64) -> Result<GlesTexture, GlesError> {
        // The recorder mints the handle itself (the inner reference
        // evaluator refuses imported images — the honest boundary
        // above; the *command stream* is this recorder's truth, and
        // the minted handle keeps the destroy bookkeeping honest).
        // The minted handle still consumes the evaluator's counter
        // and reserves its slot as `None`, so every later CPU
        // texture's handle keeps its dense index (handle N lives at
        // index N-1 — the recorder's invariant, now spanning both
        // kinds).
        let raw = self.inner.next_texture;
        self.inner.next_texture += 1;
        self.inner.textures.push(None);
        let out = GlesTexture::new(raw)
            .map_err(|_| GlesError::Failed("texture handle space exhausted".into()))?;
        self.image_textures.push(raw);
        self.record(GlesCmd::CreateTextureFromImage(out.raw(), image));
        Ok(out)
    }

    fn begin_pass(&mut self, target: GlesTarget, scissor: Option<Rect>) -> Result<(), GlesError> {
        self.inner.begin_pass(target, scissor)?;
        self.record(GlesCmd::BeginPass(target.raw(), scissor));
        Ok(())
    }

    fn clear(&mut self, r: u8, g: u8, b: u8, a: u8) -> Result<(), GlesError> {
        self.inner.clear(r, g, b, a)?;
        self.record(GlesCmd::Clear(r, g, b, a));
        Ok(())
    }

    fn draw_layer(
        &mut self,
        texture: GlesTexture,
        dst: Rect,
        opacity: u8,
    ) -> Result<(), GlesError> {
        // A minted image handle: the log-only draw (the inner
        // evaluator has no words for an imported image — recording
        // without evaluating is the recorder's honest zero-copy
        // truth; the real GPU owns the pixels).
        if !self.image_textures.contains(&texture.raw()) {
            self.inner.draw_layer(texture, dst, opacity)?;
        }
        self.record(GlesCmd::DrawLayer(texture.raw(), dst, opacity));
        Ok(())
    }

    fn end_pass(&mut self) -> Result<(), GlesError> {
        self.inner.end_pass()?;
        self.record(GlesCmd::EndPass);
        Ok(())
    }

    fn readback(&mut self, target: GlesTarget, rgba_out: &mut [u8]) -> Result<(), GlesError> {
        self.inner.readback(target, rgba_out)?;
        self.record(GlesCmd::Readback(target.raw()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: u32, h: u32) -> Rect {
        Rect::new(x, y, w, h)
    }

    /// RGBA pixels for a small solid-color texture.
    fn solid_rgba(w: u32, h: u32, px: [u8; 4]) -> Vec<u8> {
        px.repeat((w * h) as usize)
    }

    #[test]
    fn clear_and_blend_follow_the_reference_rule() {
        let mut gl = RefGles::new();
        let target = gl.create_target(2, 2).unwrap();
        // Opaque red texture (premultiplied: alpha 255, channels full).
        let tex = gl
            .create_texture(2, 2, &solid_rgba(2, 2, [255, 0, 0, 255]))
            .unwrap();
        gl.begin_pass(target, None).unwrap();
        gl.clear(0, 0, 0, 255).unwrap();
        // Half-opacity red over opaque black.
        gl.draw_layer(tex, rect(0, 0, 2, 2), 128).unwrap();
        gl.end_pass().unwrap();
        let mut out = vec![0u8; 16];
        gl.readback(target, &mut out).unwrap();
        // Half-opacity red over opaque black: color 128 red, alpha
        // 255 (premultiplied over: the destination alpha shows
        // through).
        let word = blend_word(0xFFFF_0000, 0xFF00_0000, 128);
        assert_eq!(&out[..4], &rgba_from_word(word));
        assert_eq!((word >> 24) & 0xFF, 255, "destination alpha survives");
        assert_eq!((word >> 16) & 0xFF, 128, "half-opacity red channel");
    }

    #[test]
    fn scissor_clips_both_clear_and_draw() {
        let mut gl = RefGles::new();
        let target = gl.create_target(4, 4).unwrap();
        let tex = gl
            .create_texture(4, 4, &solid_rgba(4, 4, [0, 255, 0, 255]))
            .unwrap();
        gl.begin_pass(target, Some(rect(2, 2, 2, 2))).unwrap();
        gl.clear(0, 0, 0, 255).unwrap();
        gl.draw_layer(tex, rect(0, 0, 4, 4), 255).unwrap();
        gl.end_pass().unwrap();
        let mut out = vec![0u8; 64];
        gl.readback(target, &mut out).unwrap();
        // (0,0) untouched: opaque black (fresh target).
        assert_eq!(&out[..4], &[0, 0, 0, 255]);
        // (3,3) inside scissor: green over black.
        assert_eq!(&out[3 * 4 * 4 + 3 * 4..][..4], &[0, 255, 0, 255]);
    }

    #[test]
    fn draws_clip_to_destination_and_target() {
        let mut gl = RefGles::new();
        let target = gl.create_target(3, 3).unwrap();
        // 2x2 texture, blue only in its bottom-right texel (1,1);
        // placed at (-1,-1) so exactly that texel lands on target
        // pixel (0,0) and the rest of the layer hangs off-target.
        let mut rgba = vec![0u8; 16];
        rgba[4 * (2 + 1)..4 * (2 + 1) + 4].copy_from_slice(&[0, 0, 255, 255]);
        let tex = gl.create_texture(2, 2, &rgba).unwrap();
        gl.begin_pass(target, None).unwrap();
        gl.draw_layer(tex, rect(-1, -1, 2, 2), 255).unwrap();
        gl.end_pass().unwrap();
        let mut out = vec![0u8; 36];
        gl.readback(target, &mut out).unwrap();
        // Only (0,0) lands inside the target, sampling the blue texel.
        assert_eq!(&out[..4], &[0, 0, 255, 255]);
        assert_eq!(&out[4..8], &[0, 0, 0, 255]);
    }

    #[test]
    fn handle_discipline_is_typed() {
        let mut gl = RefGles::new();
        let target = gl.create_target(1, 1).unwrap();
        // Destroying a live target succeeds; the second destroy and
        // any later use are typed errors.
        assert!(gl.destroy_target(target).is_ok());
        assert!(matches!(
            gl.destroy_target(target),
            Err(GlesError::BadHandle(_))
        ));
        assert!(matches!(
            gl.begin_pass(target, None),
            Err(GlesError::BadHandle(_))
        ));
        let t2 = gl.create_target(1, 1).unwrap();
        assert!(matches!(gl.clear(0, 0, 0, 0), Err(GlesError::Invalid(_))));
        gl.begin_pass(t2, Some(rect(0, 0, 1, 1))).unwrap();
        assert!(matches!(gl.end_pass(), Ok(())));
        // Double end_pass is invalid.
        assert!(matches!(gl.end_pass(), Err(GlesError::Invalid(_))));
    }

    #[test]
    fn zero_axes_and_size_mismatches_are_rejected() {
        let mut gl = RefGles::new();
        assert!(matches!(gl.create_target(0, 4), Err(GlesError::Invalid(_))));
        assert!(matches!(
            gl.create_texture(2, 2, &[0u8; 15]),
            Err(GlesError::Invalid(_))
        ));
    }

    #[test]
    fn recorder_logs_the_full_stream() {
        let mut gl = RecordingGles::new();
        let target = gl.create_target(2, 1).unwrap();
        let tex = gl
            .create_texture(1, 1, &solid_rgba(1, 1, [255, 255, 255, 255]))
            .unwrap();
        gl.begin_pass(target, Some(rect(0, 0, 1, 1))).unwrap();
        gl.clear(10, 20, 30, 255).unwrap();
        gl.draw_layer(tex, rect(0, 0, 1, 1), 255).unwrap();
        gl.end_pass().unwrap();
        let mut out = vec![0u8; 8];
        gl.readback(target, &mut out).unwrap();
        let stream: Vec<GlesCmd> = gl.log();
        assert_eq!(
            stream,
            vec![
                GlesCmd::CreateTarget(1, 2, 1),
                GlesCmd::CreateTexture(1, 1, 1),
                GlesCmd::BeginPass(1, Some(rect(0, 0, 1, 1))),
                GlesCmd::Clear(10, 20, 30, 255),
                GlesCmd::DrawLayer(1, rect(0, 0, 1, 1), 255),
                GlesCmd::EndPass,
                GlesCmd::Readback(1),
            ]
        );
    }
}
