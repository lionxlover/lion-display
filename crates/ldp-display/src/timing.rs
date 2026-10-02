//! The mode foundry: VESA timing synthesis (Phase 42, completed Phase 46).
//!
//! This is the machine the "every display size" row's named remainder
//! priced — the giants' custom-resolution depth (`xrandr --newmode`,
//! Windows' custom-resolution form) — shipped as arithmetic instead of
//! a quirk table. A display server that can only serve the timings a
//! sink's EDID enumerates is a display server that stops at the
//! firmware's imagination; the foundry *pours* the timing the operator
//! asked for, per the VESA computation the family names, and every
//! constraint the pour must respect stays honest.
//!
//! **The four families** (Phase 46 — the foundry completes; every
//! family the VESA standards define, from the 1996 analog era to the
//! DisplayPort deep-color one):
//!
//! * **CVT-RB** ([`cvt_rb`], the default) — reduced blanking v1, the
//!   digital-panel arm of VESA CVT 1.2: the compact 160-pixel blank
//!   (48 front porch, 32 sync, 80 back porch), a 3-line vertical front
//!   porch, a 4-line vertical sync, and the 460 µs minimum vertical
//!   blanking period solved exactly in integers. Pinned by the
//!   canonical anchor: `cvt -r 1920 1080 60` pours horizontal total
//!   2080 and vertical total 1111 — bit-exact.
//! * **CVT-RB2** ([`cvt_rb2`], `:rb2`) — reduced blanking v2 (CVT 1.2
//!   §3.4.3, the 2013 amendment): the 80-pixel blank (8/32/40 — the
//!   sync's trailing edge at the blank's center), a fixed 8-line
//!   vertical sync (per CVT 1.2 Errata E2: 8 *regardless of aspect
//!   ratio*), a fixed 6-line vertical back porch, the front porch as
//!   the 460 µs remainder — and **1-pixel horizontal precision**
//!   (1366-class widths pour exactly; no character grid). The clock
//!   grid is 0.001 MHz (our exact-integer doctrine is finer and never
//!   under). **`:rb2v`** is the same family with the §3.4.3 item-1
//!   *video-optimized* 1000/1001 multiplier — the 59.94 Hz class,
//!   geometry identical, only the clock moved (the spec's own
//!   guarantee).
//! * **CVT standard "CRT" blanking** ([`cvt_standard`], `:cvt`) —
//!   CVT 1.2 §5.3: the GTF blanking machinery (the duty-cycle
//!   equation `C' − M'·H_PERIOD/1000` with the §5.2 mandated GTF
//!   defaults M = 600, C = 40, K = 128, J = 20 — C' = 30, M' = 300),
//!   the 550 µs vertical sync + back porch, a 3-line front porch, the
//!   20% blanking floor, and the vertical sync width from Table 3-2's
//!   *aspect* doctrine (4:3 → 4, 16:9 → 5, 16:10 → 6, the 1280×1024
//!   and 1280×768 special cases → 7, non-standard → 10).
//! * **GTF** ([`gtf`], `:gtf`) — VESA GTF 1.1 §7.3, the 1999
//!   generalized timing formula the CRT family descends from: a
//!   1-line front porch, a fixed 3-line vertical sync, `ROUND`
//!   semantics where CVT rounds down, and the period *refinement*
//!   step (the horizontal period re-solved so the estimated field
//!   rate lands exactly — which collapses algebraically to
//!   `1/(rate × vtotal)`, exact in integers).
//!
//! The families are **distinct by construction** and the tests pin
//! the distinctions: for 1920×1080@60, RB pours 2080×1111, RB2 pours
//! 2000×1111 (3.8% less clock than RB, ~23% less than CRT), CVT pours
//! 2576×1120 (vfp 3, vsync 5, vbp 32), GTF pours 2576×1118 (vfp 1,
//!   vsync 3, vbp 34) — the two analog families share the 656-pixel
//!   blank (120/208/328) the duty cycle pours for that geometry, and
//!   disagree everywhere else.
//!
//! **The clock doctrine is exactness, not the grid**: the VESA
//! spreadsheet and its tools quantize the pixel clock (CVT-CRT/GTF to
//! 0.25 MHz, RB to 0.25 MHz, RB2 to 0.001 MHz); the foundry instead
//! computes `ceil(htotal × vtotal × refresh / 1 MHz)` — integer
//! kilohertz, the finest unit the DRM wire carries — and never
//! *under*-serves the requested rate (a mode slower than the operator
//! asked for steals frame deadlines; faster is safe). The honest
//! residual is sub-millihertz.
//!
//! **The VESA-spreadsheet divergences, named** (the honesty the
//! module owes): the foundry follows the *spec text* — the CRT
//! family's vertical sync width is Table 3-2's aspect mapping (the
//! spreadsheet's own rule; the common open-source `cvt` tool
//! approximates it by height, which coincides for the classic
//! sub-1024-line formats and diverges above — ours carries the table
//! verbatim). RBv1 keeps its shipped 4-line sync (the de-facto
//! ecosystem convention every reduced-blanking modeline in the wild
//! carries; the spec's Table 3-2 would make it aspect-dependent — the
//! Phase 42 anchor chose the tool, and that choice stays). The
//! cell-granularity families round the horizontal ask to the 8-pixel
//! character grid (CVT down, GTF to nearest — the §5.2/§7.3 common
//! step; the 1-pixel families serve the exact ask).
//!
//! What the foundry does **not** pour — the honest remainder, named:
//! interlaced timings and margins (both families' spreadsheet
//! options; every modern panel is progressive, and the giants' custom
//! forms do not offer them either), and RBv3 / Optimized Video
//! Timings (CEA-861-H/I's successors to this standard's blanking
//! story — a roadmap line, accruing with the same discipline).
//!
//! The pour is a *user-defined mode* in the kernel's vocabulary:
//! the mode carries the `USERDEF` type bit (the `drmModeAddMode`
//! lineage), the KMS engine validates it against its own limits at
//! commit time (the mock's declared synthesis envelope; the real
//! driver's atomic check), and the connector's EDID range limits gate
//! the pour before it is ever proposed (see [`crate::serve`]` — two
//! independent gates, both honest).

#![forbid(unsafe_code)]

use crate::mode::{Mode, ModeFlags, ModeType};

/// The VESA timing family a pour follows (Phase 46 — the foundry
/// completes). Every family the standards define, one enum, one
/// grammar: `--synth WxH[@HZ][:family]`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TimingFamily {
    /// CVT 1.2 reduced blanking v1 — the digital-panel doctrine and
    /// the foundry's default (Phase 42's family, unchanged).
    #[default]
    Rb,
    /// CVT 1.2 reduced blanking v2 (§3.4.3): the 80-pixel blank, the
    /// 8-line vertical sync, 1-pixel horizontal precision.
    Rb2,
    /// CVT 1.2 RBv2 with the 1000/1001 *video-optimized* multiplier
    /// (§3.4.3 item 1): the 59.94 Hz class — geometry identical to
    /// [`TimingFamily::Rb2`], only the clock moved.
    Rb2Video,
    /// CVT 1.2 standard "CRT" blanking (§5.3): the GTF blanking
    /// machinery, the 550 µs sync + back porch, the aspect-mapped
    /// vertical sync width.
    Cvt,
    /// VESA GTF 1.1 (§7.3): the 1999 generalized timing formula —
    /// 1-line front porch, 3-line sync, the refined period.
    Gtf,
}

impl TimingFamily {
    /// The CLI spelling (the `:family` suffix of `--synth`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Rb => "rb",
            Self::Rb2 => "rb2",
            Self::Rb2Video => "rb2v",
            Self::Cvt => "cvt",
            Self::Gtf => "gtf",
        }
    }

    /// The spec's own name for the family (report lines).
    #[must_use]
    pub const fn spec_name(self) -> &'static str {
        match self {
            Self::Rb => "CVT 1.2 reduced blanking v1",
            Self::Rb2 => "CVT 1.2 reduced blanking v2",
            Self::Rb2Video => "CVT 1.2 reduced blanking v2, video-optimized",
            Self::Cvt => "CVT 1.2 standard CRT blanking",
            Self::Gtf => "VESA GTF 1.1",
        }
    }

    /// Parse the CLI spelling.
    ///
    /// # Errors
    /// `Err(String)` naming the valid spellings — the argv layer's
    /// error shape.
    pub fn parse(text: &str) -> Result<Self, String> {
        match text {
            "rb" => Ok(Self::Rb),
            "rb2" => Ok(Self::Rb2),
            "rb2v" => Ok(Self::Rb2Video),
            "cvt" => Ok(Self::Cvt),
            "gtf" => Ok(Self::Gtf),
            other => Err(format!(
                "unknown timing family '{other}' (one of: rb, rb2, rb2v, cvt, gtf)"
            )),
        }
    }
}

impl core::fmt::Display for TimingFamily {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.spec_name())
    }
}

/// The requested pour: an active area, a refresh, and the family to
/// pour it with.
///
/// Millihertz is the unit the whole timing stack already speaks
/// ([`Mode::refresh_millihz`]), so `--synth 2560x1440@143:rb2` and
/// the panel's advertised 143,999 mHz are the same arithmetic — the
/// foundry never rounds a rate before the clock makes it real.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SynthRequest {
    /// Active width in pixels.
    pub width: u32,
    /// Active height in pixels.
    pub height: u32,
    /// Target refresh in millihertz (60 Hz = 60_000).
    pub refresh_millihz: u64,
    /// The VESA family to pour (Phase 46). The grammar's default is
    /// [`TimingFamily::Rb`] — Phase 42's doctrine, unchanged.
    pub family: TimingFamily,
}

impl SynthRequest {
    /// Parse the CLI spelling `WxH[@HZ][:family]` (e.g.
    /// `1920x1080@60:rb2`, `2560x1440:gtf` — refresh defaults to
    /// 60 Hz, family defaults to `rb`).
    ///
    /// The refresh is whole hertz on the CLI (the operator's spelling;
    /// `59.94`-class rates arrive from EDID, from the `:rb2v`
    /// multiplier, or from the API — not from argv's whole hertz).
    ///
    /// # Errors
    /// `Err(String)` for malformed text — the argv layer's error shape,
    /// the same contract [`crate::serve::ModeSize::parse`] keeps.
    pub fn parse(text: &str) -> Result<Self, String> {
        // The family suffix rides last: `WxH[@HZ][:family]`.
        let (geometry, family_text) = match text.rsplit_once(':') {
            Some((g, f)) => (g, Some(f)),
            None => (text, None),
        };
        let family = match family_text {
            Some(f) => TimingFamily::parse(f)?,
            None => TimingFamily::Rb,
        };
        let (size_text, rate_text) = match geometry.split_once('@') {
            Some((s, r)) => (s, Some(r)),
            None => (geometry, None),
        };
        let size = crate::serve::ModeSize::parse(size_text)?;
        let refresh_hz: u64 = match rate_text {
            Some(r) => r.parse().map_err(|_| {
                format!("bad refresh '{r}' in '{text}' (a whole number of hertz, e.g. @60)")
            })?,
            None => 60,
        };
        if refresh_hz == 0 {
            return Err(format!("degenerate refresh in '{text}' (must be nonzero)"));
        }
        if refresh_hz > 1000 {
            // Every family's blanking solve degenerates in the
            // multi-kilohertz band (the CRT families' 550 µs
            // denominator collapses near 1.82 MHz, RB's near 2.17);
            // the foundry refuses absurd asks long before the
            // arithmetic does.
            return Err(format!(
                "refresh beyond 1000 Hz in '{text}' (the VESA timing families do not serve it)"
            ));
        }
        Ok(Self {
            width: size.width,
            height: size.height,
            refresh_millihz: refresh_hz * 1000,
            family,
        })
    }

    /// The `WxH@Hz[:family]` spelling (report lines) — the family
    /// suffix appears only for the non-default families, so Phase
    /// 42's pinned report strings stay byte-identical.
    #[must_use]
    pub fn as_str(&self) -> String {
        let base = format!(
            "{}x{}@{}.{}",
            self.width,
            self.height,
            self.refresh_millihz / 1000,
            self.refresh_millihz % 1000
        );
        if self.family == TimingFamily::Rb {
            base
        } else {
            format!("{base}:{}", self.family.as_str())
        }
    }
}

/// CVT-RB horizontal blanking: 160 pixels, fixed (the digital doctrine).
const RB_H_BLANK: u32 = 160;
/// CVT-RB horizontal front porch.
const RB_H_FPORCH: u32 = 48;
/// CVT-RB horizontal sync width.
const RB_H_SYNC: u32 = 32;
/// CVT-RB vertical front porch, lines.
const RB_V_FPORCH: u32 = 3;
/// CVT-RB vertical sync width, lines.
const RB_V_SYNC: u32 = 4;
/// CVT-RB minimum vertical blanking period, microseconds.
const RB_MIN_V_BLANK_US: u64 = 460;
/// CVT-RB minimum vertical back porch, lines (the floor that keeps the
/// blanking structurally sane at low refresh, where the 460 µs solve
/// alone would leave the back porch at zero).
const RB_MIN_V_BPORCH: u32 = RB_V_FPORCH + RB_V_SYNC + 6;

/// The character cell granularity (CVT §5.2 / GTF §3: the
/// cell-granularity families round the horizontal ask to this grid).
const CELL_GRAN: u32 = 8;
/// CVT-CRT / GTF minimum time of vertical sync + back porch,
/// microseconds (both standards' 550 µs).
const CRT_MIN_VSYNC_BP_US: u64 = 550;
/// CVT-CRT standard-timing vertical front porch, lines (§5.2's
/// `MIN_V_PORCH_RND`).
const CVT_MIN_V_PORCH: u32 = 3;
/// CVT-CRT minimum vertical back porch, lines (§5.2's `MIN_VBPORCH`).
const CVT_MIN_V_BPORCH: u32 = 6;
/// GTF minimum vertical front porch, lines (GTF §3's `MIN PORCH` —
/// GTF's own 1, where CVT grew it to 3).
const GTF_MIN_V_PORCH: u32 = 1;
/// GTF vertical sync width, lines (GTF §3's `V SYNC RQD` — fixed 3,
/// where CVT made it aspect-mapped).
const GTF_V_SYNC: u32 = 3;
/// The blanking-duty-cycle offset C' = ((C − J) × K/256) + J with the
/// GTF defaults C = 40, J = 20, K = 128 (percent). The gradient
/// M' = K/256 × M = 300 rides the formulas as the coefficient 3 over
/// the 10⁴ denominator (M'·H_µs/1000 = 3·H_ns/10⁴).
const DUTY_C_PRIME: u64 = 30;

/// CVT-RBv2 horizontal blanking: 80 pixels, fixed (§3.4.3 item 4).
const RB2_H_BLANK: u32 = 80;
/// CVT-RBv2 horizontal front porch (§3.4.3 item 5: the sync's
/// trailing edge at the blank's center — 80 − 40 − 32).
const RB2_H_FPORCH: u32 = 8;
/// CVT-RBv2 horizontal sync width (§3.4.3 item 5: 32 clocks, RB's
/// own width kept).
const RB2_H_SYNC: u32 = 32;
/// CVT-RBv2 horizontal back porch (§3.4.3 item 5: the blank's center
/// — 80 − 8 − 32).
const RB2_H_BPORCH: u32 = 40;
/// CVT-RBv2 vertical sync width, lines (§3.4.3 item 7 and Errata E2:
/// fixed 8 regardless of aspect ratio).
const RB2_V_SYNC: u32 = 8;
/// CVT-RBv2 vertical back porch, lines (§3.4.3 item 7: fixed 6).
const RB2_V_BPORCH: u32 = 6;
/// CVT-RBv2 minimum vertical blanking period, microseconds (§3.4.3
/// item 6: the first multiple of lines that *exceeds* 460 µs).
const RB2_MIN_V_BLANK_US: u64 = 460;
/// CVT-RBv2's video-optimized refresh multiplier (§3.4.3 item 1:
/// 1000/1001 — the 59.94 Hz class).
const RB2_VIDEO_NUM: u64 = 1000;
const RB2_VIDEO_DEN: u64 = 1001;

/// Pour one timing with the request's own family — the foundry's
/// front door (Phase 46). Every family returns a `USERDEF` mode whose
/// clock never under-serves the ask; the families' guarantees are the
/// individual functions' own.
#[must_use]
pub fn pour(request: SynthRequest) -> Option<Mode> {
    match request.family {
        TimingFamily::Rb => cvt_rb(request),
        TimingFamily::Rb2 => cvt_rb2(request, false),
        TimingFamily::Rb2Video => cvt_rb2(request, true),
        TimingFamily::Cvt => cvt_standard(request),
        TimingFamily::Gtf => gtf(request),
    }
}

/// Pour one reduced-blanking v1 timing for `request` (Phase 42's
/// family, the default — unchanged).
///
/// The computation is integer-exact and clock-deterministic (no
/// floats — the repo's bit-reproducibility doctrine):
///
/// 1. `vblank >= ceil(460 × refresh_mHz × vactive / (10⁹ − 460 ×
///    refresh_mHz))` — the 460 µs minimum vertical blanking period
///    solved for the blank line count (the spec's implicit inequality
///    `vblank / hfreq >= 460 µs` with `hfreq = refresh × vtotal`),
///    floored at the structural minimum `3 + 4 + 6`;
/// 2. the horizontal geometry is the fixed compact blank —
///    `htotal = hactive + 160`, sync edges at +48/+32;
/// 3. the pixel clock is `ceil(htotal × vtotal × refresh_mHz / 10⁶)`
///    kHz — integer kilohertz, never under the ask.
///
/// `None` is the honest degenerate refusal: a non-positive axis, an
/// axis beyond the 16-bit mode wire (`u16` totals), or a refresh
/// whose 460 µs denominator collapses (> 2,173,913 mHz — the API
/// layer refuses long before).
#[must_use]
pub fn cvt_rb(request: SynthRequest) -> Option<Mode> {
    let (w, h, f) = (
        u64::from(request.width),
        u64::from(request.height),
        request.refresh_millihz,
    );
    if w == 0 || h == 0 || f == 0 {
        return None;
    }
    // The 460 µs solve's denominator: 10⁹ − 460·f must stay positive.
    let per_frame = RB_MIN_V_BLANK_US.saturating_mul(f);
    let denominator = 1_000_000_000u64.checked_sub(per_frame)?;
    // vblank >= ceil(460·f·vactive / denominator), then the structural
    // floor. Numerator bound: 460 × 10⁶ mHz × 65535 lines < 2⁴⁸ — the
    // u64 arithmetic is exact.
    let numerator = per_frame.saturating_mul(h);
    let solved = numerator.div_ceil(denominator).max(RB_MIN_V_BPORCH.into());
    let vblank: u32 = u32::try_from(solved).ok()?;

    let htotal = w + u64::from(RB_H_BLANK);
    let vtotal = h + u64::from(vblank);
    let hsync_start = w + u64::from(RB_H_FPORCH);
    let hsync_end = hsync_start + u64::from(RB_H_SYNC);
    let vsync_start = h + u64::from(RB_V_FPORCH);
    let vsync_end = vsync_start + u64::from(RB_V_SYNC);
    // The 16-bit mode wire: totals and edges must fit u16 (a 65535-line
    // or -pixel raster is beyond every panel the foundry serves; the
    // honest refusal, not a truncation).
    let fit = |v: u64| u16::try_from(v).ok();
    let (htotal, vtotal) = (fit(htotal)?, fit(vtotal)?);
    let (hsync_start, hsync_end) = (fit(hsync_start)?, fit(hsync_end)?);
    let (vsync_start, vsync_end) = (fit(vsync_start)?, fit(vsync_end)?);
    let (hdisplay, vdisplay) = (fit(w)?, fit(h)?);

    // The exact clock: ceil over the integer-kilohertz wire, never
    // under the ask. Bound: 65535 × 65535 × 10⁶ mHz < 2⁴⁸ — exact.
    let ticks = u64::from(htotal) * u64::from(vtotal) * f;
    let clock_khz = u32::try_from(ticks.div_ceil(1_000_000)).ok()?;

    let mode = Mode::new(
        clock_khz,
        hdisplay,
        hsync_start,
        hsync_end,
        htotal,
        vdisplay,
        vsync_start,
        vsync_end,
        vtotal,
        // The digital compact-blank polarity: +hsync, −vsync (the
        // reduced-blanking convention every CVT-RB modeline carries).
        ModeFlags::PHSYNC | ModeFlags::NVSYNC,
        // The pour is a user-defined mode — the kernel's own vocabulary
        // for operator-minted timings (the `drmModeAddMode` lineage).
        ModeType::USERDEF,
    );
    mode.is_valid().then_some(mode)
}

/// Pour one reduced-blanking **v2** timing for `request` (CVT 1.2
/// §3.4.3 — Phase 46).
///
/// The v2 doctrine: the 80-pixel blank (front porch 8, sync 32, back
/// porch 40 — the sync's trailing edge parked at the blank's center),
/// the fixed 8-line vertical sync (Errata E2's rule, aspect-free),
/// the fixed 6-line vertical back porch, and the front porch as the
/// *remainder* of the 460 µs vertical blanking (`VBI_LINES =
/// ROUNDDOWN(460 µs / hperiod) + 1` — the first multiple of lines
/// that exceeds the minimum — floored at the structural `1 + 8 + 6`).
/// Horizontal counts carry **1-pixel precision** (§3.4.3 item 3: the
/// 1366-class widths pour exactly — no character grid).
///
/// With `video` set, the §3.4.3 item-1 *video-optimized* multiplier
/// applies: the geometry is poured from the **nominal** rate and only
/// the clock moves (the spec's own guarantee — the only difference
/// between a video-optimized and a non-optimized timing for a given
/// rate is the pixel clock), so `--synth 1920x1080@60:rb2v` serves
/// the 59.94 Hz class with RB2's raster bit-identical to
/// `@60:rb2`'s.
///
/// The clock is the foundry's exact doctrine (`ceil` to integer kHz,
/// never under the *target* rate — nominal for the plain families,
/// `rate × 1000/1001` for the video-optimized one), which is finer
/// than the spec's own 0.001 MHz grid. `None` is the honest
/// degenerate refusal, CVT-RB's own vocabulary.
#[must_use]
pub fn cvt_rb2(request: SynthRequest, video: bool) -> Option<Mode> {
    let (w, h, f) = (
        u64::from(request.width),
        u64::from(request.height),
        request.refresh_millihz,
    );
    if w == 0 || h == 0 || f == 0 {
        return None;
    }
    // The 460 µs solve's denominator: 10¹² − 460000·f > 0 (the RB2
    // arithmetic is carried in nanoseconds — the same shape as RB's
    // microsecond solve, one power of ten finer).
    let per_frame_ns = RB2_MIN_V_BLANK_US.saturating_mul(f).saturating_mul(1000);
    let denominator = 1_000_000_000_000u64.checked_sub(per_frame_ns)?;
    // VBI_LINES = ROUNDDOWN(460000·f·vactive / denominator) + 1, then
    // the structural floor 1 + 8 + 6. Bound: 460000 × 2.17×10⁶ mHz ×
    // 65535 < 2⁵⁷ — the u64 arithmetic is exact.
    let numerator = per_frame_ns.saturating_mul(h);
    let vbi = numerator / denominator + 1;
    let act_vbi = vbi.max(u64::from(RB2_V_SYNC + RB2_V_BPORCH + 1));

    let vtotal = h + act_vbi;
    // The vertical edges: front porch = the blanking remainder.
    let vfp = act_vbi - u64::from(RB2_V_SYNC + RB2_V_BPORCH);
    let hsync_start = w + u64::from(RB2_H_FPORCH);
    let hsync_end = hsync_start + u64::from(RB2_H_SYNC);
    let htotal = hsync_end + u64::from(RB2_H_BPORCH);
    debug_assert_eq!(htotal, w + u64::from(RB2_H_BLANK));
    let vsync_start = h + vfp;
    let vsync_end = vsync_start + u64::from(RB2_V_SYNC);
    let fit = |v: u64| u16::try_from(v).ok();
    let (htotal, vtotal) = (fit(htotal)?, fit(vtotal)?);
    let (hsync_start, hsync_end) = (fit(hsync_start)?, fit(hsync_end)?);
    let (vsync_start, vsync_end) = (fit(vsync_start)?, fit(vsync_end)?);
    let (hdisplay, vdisplay) = (fit(w)?, fit(h)?);

    // The exact clock, video-multiplied when asked: ceil to integer
    // kHz over htotal × vtotal × (f or f·1000/1001). The cross
    // product is u128 (65535 × 65535 × 2.17×10⁶ × 1000 approaches
    // 2⁶³ — one power too close to trust).
    let ticks = u128::from(htotal) * u128::from(vtotal) * u128::from(f);
    let clock_khz = if video {
        (ticks * u128::from(RB2_VIDEO_NUM)).div_ceil(u128::from(RB2_VIDEO_DEN) * 1_000_000)
    } else {
        ticks.div_ceil(1_000_000)
    };
    let clock_khz = u32::try_from(clock_khz).ok()?;

    let mode = Mode::new(
        clock_khz,
        hdisplay,
        hsync_start,
        hsync_end,
        htotal,
        vdisplay,
        vsync_start,
        vsync_end,
        vtotal,
        // The reduced-blanking polarity (Table 3-1's RB row): +hsync,
        // −vsync — v2's own row in the delta table keeps it.
        ModeFlags::PHSYNC | ModeFlags::NVSYNC,
        ModeType::USERDEF,
    );
    mode.is_valid().then_some(mode)
}

/// The CVT 1.2 Table 3-2 vertical sync width: the *aspect* doctrine.
///
/// 4:3 → 4, 16:9 → 5, 16:10 → 6 (ratios reduced — 8:5 *is* 16:10);
/// the two established non-standard-ratio formats the table names
/// (1280×1024, the 5:4 SXGA, and 1280×768, the 15:9 one) → 7; every
/// other aspect → 10 (the table's "non-standard" row —
/// manufacturer-specific timings). The foundry follows the spec's
/// table verbatim; the common open-source `cvt` tool approximates it
/// by *height* (4/5/6 at the 1024/1280 lines), which coincides for
/// the classic sub-1024 formats and diverges above — a named
/// divergence, the spec text wins.
#[must_use]
pub fn cvt_vsync_rnd(width: u32, height: u32) -> u32 {
    if (width, height) == (1280, 1024) || (width, height) == (1280, 768) {
        return 7;
    }
    let g = gcd(width, height);
    match (width / g, height / g) {
        (4, 3) => 4,
        (16, 9) => 5,
        (8, 5) => 6,
        _ => 10,
    }
}

/// `gcd` on `u32` (the aspect reduction's helper).
const fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let r = a % b;
        a = b;
        b = r;
    }
    a
}

/// Round a positive rational to the nearest integer, halves away
/// from zero (Excel's `ROUND` — the semantics the GTF sheet and the
/// CVT spreadsheet specify). `(2n + d) / (2d)` in exact integer
/// arithmetic: the half and everything above lands one up.
fn round_half_up(num: u128, den: u128) -> u128 {
    (2 * num + den) / (2 * den)
}

/// Pour one CVT standard "CRT" timing for `request` (CVT 1.2 §5.3 —
/// Phase 46).
///
/// The analog-era arithmetic, integer-exact (every step a rational
/// with explicit rounding — no floats, ever):
///
/// 1. §5.2's common step: the horizontal ask is rounded *down* to
///    the character grid (`H_PIXELS_RND = ROUNDDOWN(H/8)×8` — the
///    mode's own width is the rounded one, the VESA doctrine);
/// 2. `H_PERIOD_EST = (10¹² − 550000·f) / (f·(V+3))` ns — the
///    550 µs sync + back porch time taken off the frame period, the
///    remainder divided over the active lines plus the 3-line front
///    porch;
/// 3. `V_SYNC_BP = ROUNDDOWN(550000·f·(V+3) / (10¹² − 550000·f)) + 1`
///    lines, floored at `V_SYNC_RND + 6` (the aspect-mapped sync
///    width plus the minimum back porch);
/// 4. the ideal blanking duty cycle `C' − M'·H_PERIOD/1000` (the GTF
///    equation with the §5.2-mandated defaults: 30% − 300‰·period),
///    floored at 20% of the horizontal total (§5.3 step 13's limit);
///    the blanking `H_BLANK = ROUNDDOWN(H·duty/(100−duty)/(2·8))·16`
///    — a whole number of double character cells;
/// 5. the sync placement (GTF §7.5's own): sync width
///    `ROUND(8% × htotal / 8)·8`, the front porch `(H_BLANK/2) −
///    sync` (the sync's trailing edge lands at the blank's center),
///    the back porch the mirror of the front;
/// 6. the clock — the foundry's exact doctrine, `ceil` to integer
///    kHz, never under the ask (the VESA spreadsheet would round
///    *down* to the 0.25 MHz grid; the divergence is the doctrine's
///    own, documented above).
///
/// `None` is the honest degenerate refusal: CVT-RB's own vocabulary
/// (axes, the 16-bit wire), the 550 µs denominator's collapse
/// (f ≥ 1,818,182 mHz), an ask under one character cell, or a blank
/// the duty cycle could not pay (is_valid's own gate).
#[must_use]
pub fn cvt_standard(request: SynthRequest) -> Option<Mode> {
    let (w, h, f) = (
        u64::from(request.width),
        u64::from(request.height),
        request.refresh_millihz,
    );
    if w == 0 || h == 0 || f == 0 {
        return None;
    }
    // The 16-bit mode wire's own honesty, early: an axis beyond
    // 65535 is beyond every panel the foundry serves — and the gate
    // every later product's bound leans on.
    if w > u64::from(u16::MAX) || h > u64::from(u16::MAX) {
        return None;
    }
    // §5.2 step 2: the character grid, rounded DOWN (the CRT family
    // carries the grid; an ask under one cell is degenerate).
    let h_rnd = (w / u64::from(CELL_GRAN)) * u64::from(CELL_GRAN);
    if h_rnd == 0 {
        return None;
    }
    // The 550 µs solve: 10¹² − 550000·f > 0 (nanosecond·millihertz
    // units — the frame period is 10¹²/f ns). The product saturates
    // first so an absurd API refresh refuses instead of wrapping.
    let sync_bp_ns = CRT_MIN_VSYNC_BP_US.saturating_mul(1000).saturating_mul(f);
    let a = 1_000_000_000_000u64.checked_sub(sync_bp_ns)?;
    let vp = h + u64::from(CVT_MIN_V_PORCH);
    // V_SYNC_BP = ROUNDDOWN(550000·f·(V+3)/A) + 1, floored at the
    // aspect-mapped sync + 6. Bound: 550000 × 1.82×10⁶ × 65538 < 2⁵⁷.
    let vsync = u64::from(cvt_vsync_rnd(h_rnd as u32, request.height));
    let v_sync_bp = (sync_bp_ns * vp / a + 1).max(vsync + u64::from(CVT_MIN_V_BPORCH));
    let vtotal = h + v_sync_bp + u64::from(CVT_MIN_V_PORCH);

    // The duty cycle: 30 − 3·hperiod_ns/10⁴ with hperiod_ns = A/(f·Vp)
    // — as the exact rational d_num/d_den (percent). Bound:
    // 3×10⁶ × 1.82×10⁶ × 65538 < 2⁵⁹.
    let d_den = 10_000u64.saturating_mul(f).saturating_mul(vp);
    // §5.3 step 13: the 20% floor — blanking at least a fifth of the
    // horizontal total. The comparison is sign-free (duty < 20 ⟺
    // 10·d_den < 3a — negative duty included, the u64 subtraction
    // the general branch would underflow on never happens).
    let hblank: u64 = if 10 * d_den < 3 * a {
        // The floor branch is exact integer arithmetic
        // (H·20/80/16 = H/64).
        h_rnd / 64 * 16
    } else {
        // ROUNDDOWN(H·duty/(100−duty)/(2·CELL))·(2·CELL) — d_num is
        // at least 20·d_den here (positive by the branch guard); the
        // cross product is u128 (65535 × 3.6×10¹⁶ approaches 2⁷⁵).
        let d_num = DUTY_C_PRIME * d_den - 3 * a;
        let num = u128::from(h_rnd) * u128::from(d_num);
        let den = u128::from(16 * (100 * d_den - d_num));
        u64::try_from((num / den) * 16).ok()?
    };
    let htotal = h_rnd + hblank;

    // GTF §7.5 steps 17–19 (the sync placement): width ROUND(8% of
    // htotal to the cell), front porch H_BLANK/2 − sync (the trailing
    // edge at the blank's center), back porch the mirror.
    let hsync = round_half_up(u128::from(htotal), 100) * 8;
    let hsync = u64::try_from(hsync).ok()?;
    let hfp = hblank / 2;
    if hfp < hsync {
        // The sync swallows the front porch's half — a blank too thin
        // to carry the cell doctrine (the tiny-ask degenerate band).
        return None;
    }
    let hfp = hfp - hsync;

    let hsync_start = h_rnd + hfp;
    let hsync_end = hsync_start + hsync;
    let vsync_start = h + u64::from(CVT_MIN_V_PORCH);
    let vsync_end = vsync_start + vsync;
    let fit = |v: u64| u16::try_from(v).ok();
    let (htotal, vtotal) = (fit(htotal)?, fit(vtotal)?);
    let (hsync_start, hsync_end) = (fit(hsync_start)?, fit(hsync_end)?);
    let (vsync_start, vsync_end) = (fit(vsync_start)?, fit(vsync_end)?);
    let (hdisplay, vdisplay) = (fit(h_rnd)?, fit(h)?);

    // The exact clock: ceil to integer kHz, never under the ask.
    let ticks = u128::from(htotal) * u128::from(vtotal) * u128::from(f);
    let clock_khz = u32::try_from(ticks.div_ceil(1_000_000)).ok()?;

    let mode = Mode::new(
        clock_khz,
        hdisplay,
        hsync_start,
        hsync_end,
        htotal,
        vdisplay,
        vsync_start,
        vsync_end,
        vtotal,
        // Table 3-1's CRT row: −hsync, +vsync (the standard-timing
        // polarity pair the spec assigns so a display can tell the
        // CRT and reduced-blanking doctrines apart on the wire).
        ModeFlags::NHSYNC | ModeFlags::PVSYNC,
        ModeType::USERDEF,
    );
    mode.is_valid().then_some(mode)
}

/// Pour one GTF timing for `request` (VESA GTF 1.1 §7.3 — Phase 46).
///
/// The 1999 formula the CVT CRT family descends from — the
/// distinctions the tests pin:
///
/// * the horizontal ask rounds to the character grid *to nearest*
///   (GTF's `ROUND`, where CVT rounds down);
/// * the period estimate divides by `V + 1` (GTF's 1-line minimum
///   porch, where CVT grew it to 3);
/// * `V_SYNC_BP = ROUND(550 µs / H_PERIOD_EST)` — plain rounding,
///   where CVT is floor-plus-one (they agree whenever the fraction
///   rounds up, which the common formats do);
/// * the vertical sync is a fixed 3 lines (GTF §3's `V SYNC RQD`),
///   the front porch a fixed 1 — the pre-CVT vocabulary, no aspect
///   table exists to consult;
/// * the *refinement* step (§7.3 step 12): the horizontal period is
///   re-solved against the estimated field rate — algebraically
///   `H_PERIOD = 1/(rate × vtotal)`, which in integers is exact —
///   and the duty cycle is computed from the *refined* period
///   (where CVT uses the estimate);
/// * the blanking rounds *to nearest* (GTF's `ROUND`, where CVT
///   rounds down) and carries no floor (the 20% limit is CVT's
///   own addition, not GTF's).
///
/// `None` is the honest degenerate refusal: GTF's own two collapse
/// bands (a `V_SYNC_BP` the ROUND would leave under the sync width —
/// the back porch would go negative, the formula's honest domain
/// limit; and a duty cycle the refined period would drive to zero —
/// the absurd-low-refresh band), plus CVT-RB's shared vocabulary.
#[must_use]
pub fn gtf(request: SynthRequest) -> Option<Mode> {
    let (w, h, f) = (
        u64::from(request.width),
        u64::from(request.height),
        request.refresh_millihz,
    );
    if w == 0 || h == 0 || f == 0 {
        return None;
    }
    // The 16-bit mode wire's early gate (same honesty, same bound).
    if w > u64::from(u16::MAX) || h > u64::from(u16::MAX) {
        return None;
    }
    // GTF rounds the horizontal ask to the character grid TO NEAREST.
    let h_rnd = round_half_up(u128::from(w), u128::from(CELL_GRAN)) * u128::from(CELL_GRAN);
    let h_rnd = u64::try_from(h_rnd).ok()?;
    if h_rnd == 0 {
        return None;
    }
    // The 550 µs solve, GTF's divisor: V + 1 (the 1-line minimum
    // porch). The product saturates first (the absurd-band refusal).
    let sync_bp_ns = CRT_MIN_VSYNC_BP_US.saturating_mul(1000).saturating_mul(f);
    let a = 1_000_000_000_000u64.checked_sub(sync_bp_ns)?;
    let vp = h + u64::from(GTF_MIN_V_PORCH);
    // V_SYNC_BP = ROUND(550000·f·(V+1)/A) — Excel's ROUND, halves
    // away from zero (round_half_up).
    let v_sync_bp = round_half_up(u128::from(sync_bp_ns * vp), u128::from(a));
    let v_sync_bp = u64::try_from(v_sync_bp).ok()?;
    if v_sync_bp < u64::from(GTF_V_SYNC) {
        // The back porch would go negative — the formula's honest
        // domain limit, refused (CVT's floor saves its own family;
        // GTF has none).
        return None;
    }
    let vtotal = h + v_sync_bp + u64::from(GTF_MIN_V_PORCH);
    // The u16 mode wire's early gate: a V_SYNC_BP the ROUND blew up
    // (the near-collapse refresh band) makes the total astronomical —
    // refuse here, before the duty arithmetic would chase it (the
    // bound every later product leans on).
    if vtotal > u64::from(u16::MAX) {
        return None;
    }

    // The refined period: H_PERIOD = 1/(f·vtotal) exactly (§7.3's
    // step-12 refinement collapses to this in closed form) — the
    // duty cycle is computed from it: 30 − 3·H_ns/10⁴ over the
    // rational d_num/d_den.
    let d_den = 10_000u64.saturating_mul(f).saturating_mul(vtotal);
    // GTF has no floor: a duty cycle at or below zero (the
    // absurd-low-refresh band — the refined period past 100 µs) is
    // the honest refusal. The subtraction rides i128 so the band
    // refuses instead of underflowing.
    let d_num: i128 = i128::from(DUTY_C_PRIME * d_den) - 3_000_000_000_000i128;
    if d_num <= 0 {
        return None;
    }
    let diff = u64::try_from(100 * i128::from(d_den) - d_num).ok()?;
    // H_BLANK = ROUND(H·duty/(100−duty)/(2·CELL))·(2·CELL) — the
    // cross product in u128.
    let hblank = round_half_up(
        u128::from(h_rnd) * u128::try_from(d_num).ok()?,
        u128::from(16 * diff),
    ) * 16;
    let hblank = u64::try_from(hblank).ok()?;
    if hblank == 0 {
        return None;
    }
    let htotal = h_rnd + hblank;

    // The sync placement (§7.5 steps 17–19): the GTF sheet's own.
    let hsync = round_half_up(u128::from(htotal), 100) * 8;
    let hsync = u64::try_from(hsync).ok()?;
    let hfp = hblank / 2;
    if hfp < hsync {
        return None;
    }
    let hfp = hfp - hsync;

    let hsync_start = h_rnd + hfp;
    let hsync_end = hsync_start + hsync;
    let vsync_start = h + u64::from(GTF_MIN_V_PORCH);
    let vsync_end = vsync_start + u64::from(GTF_V_SYNC);
    let fit = |v: u64| u16::try_from(v).ok();
    let (htotal, vtotal) = (fit(htotal)?, fit(vtotal)?);
    let (hsync_start, hsync_end) = (fit(hsync_start)?, fit(hsync_end)?);
    let (vsync_start, vsync_end) = (fit(vsync_start)?, fit(vsync_end)?);
    let (hdisplay, vdisplay) = (fit(h_rnd)?, fit(h)?);

    let ticks = u128::from(htotal) * u128::from(vtotal) * u128::from(f);
    let clock_khz = u32::try_from(ticks.div_ceil(1_000_000)).ok()?;

    let mode = Mode::new(
        clock_khz,
        hdisplay,
        hsync_start,
        hsync_end,
        htotal,
        vdisplay,
        vsync_start,
        vsync_end,
        vtotal,
        // The analog-era convention: −hsync, +vsync (GTF predates
        // polarity signaling; the foundry assigns CVT's own CRT-row
        // pair so both analog families speak one polarity doctrine).
        ModeFlags::NHSYNC | ModeFlags::PVSYNC,
        ModeType::USERDEF,
    );
    mode.is_valid().then_some(mode)
}

// ===== tests =====

#[cfg(test)]
mod tests {
    use super::*;

    /// A request at the default family (the Phase 42 spelling).
    fn rb(w: u32, h: u32, hz: u64) -> SynthRequest {
        SynthRequest {
            width: w,
            height: h,
            refresh_millihz: hz * 1000,
            family: TimingFamily::Rb,
        }
    }

    /// A request for an explicit family.
    fn ask(w: u32, h: u32, hz: u64, family: TimingFamily) -> SynthRequest {
        SynthRequest {
            width: w,
            height: h,
            refresh_millihz: hz * 1000,
            family,
        }
    }

    /// The canonical anchor: `cvt -r 1920 1080 60` pours htotal 2080
    /// and vtotal 1111 — the foundry's reduced-blanking arithmetic
    /// reproduces both totals bit-exactly. Derived by hand:
    ///
    /// * vblank: `ceil(460 × 60000 × 1080 / (10⁹ − 460 × 60000))`
    ///   = `ceil(29 808 000 000 / 972 400 000)` = `ceil(30.6535)` = 31
    ///   → vtotal = 1111, vsync edges 1083/1087 (front porch 3, sync 4);
    /// * htotal = 1920 + 160 = 2080, hsync edges 1968/2000
    ///   (front porch 48, sync 32, back porch 80);
    /// * clock = `ceil(2080 × 1111 × 60000 / 10⁶)` = `ceil(138.6528 MHz)`
    ///   = 138 653 kHz — the exact doctrine (the VESA tool's 0.25 MHz
    ///   grid would print 138.50 and land at 59.93 Hz; ours lands at
    ///   60.000087 Hz — the 0.087 mHz integer-wire residual, over the
    ///   ask because the clock ceils, never under).
    #[test]
    fn rb_anchor_1080p60() {
        let m = cvt_rb(rb(1920, 1080, 60)).unwrap();
        assert_eq!((m.hdisplay, m.vdisplay), (1920, 1080));
        assert_eq!(m.htotal, 2080);
        assert_eq!(m.vtotal, 1111);
        assert_eq!((m.hsync_start, m.hsync_end), (1968, 2000));
        assert_eq!((m.vsync_start, m.vsync_end), (1083, 1087));
        assert_eq!(m.clock_khz, 138_653);
        // 138 653 kHz over 2080 × 1111: 138 653 000 000 / 2 310 880
        // = 60 000.087 mHz — the integer division lands at 60 000,
        // never under the ask.
        assert_eq!(m.refresh_millihz(), 60_000);
        assert!(m.refresh_millihz() >= 60_000);
        assert_eq!(m.kind, ModeType::USERDEF);
        assert_eq!(m.flags, ModeFlags::PHSYNC | ModeFlags::NVSYNC);
        assert_eq!(m.name, "1920x1080");
        assert!(m.is_valid());
    }

    /// The 4K pour: the desktop matrix's ceiling size, synthesized on
    /// a panel whose EDID never enumerated it. Derived by hand:
    ///
    /// * vblank: `ceil(460 × 60000 × 2160 / 972 400 000)`
    ///   = `ceil(61.3133)` = 62 → vtotal 2222, vsync edges 2163/2167;
    /// * htotal 4000 (3840 + 160), hsync edges 3888/3920;
    /// * clock `ceil(4000 × 2222 × 60000 / 10⁶)` = 533 280 kHz —
    ///   exact (the product is integral), 60.000 Hz precisely.
    #[test]
    fn rb_4k60() {
        let m = cvt_rb(rb(3840, 2160, 60)).unwrap();
        assert_eq!(m.htotal, 4000);
        assert_eq!(m.vtotal, 2222);
        assert_eq!((m.hsync_start, m.hsync_end), (3888, 3920));
        assert_eq!((m.vsync_start, m.vsync_end), (2163, 2167));
        assert_eq!(m.clock_khz, 533_280);
        assert_eq!(m.refresh_millihz(), 60_000);
        assert!(m.is_valid());
    }

    /// A 144 Hz pour (the VRR panel's fast end): the solve tightens
    /// with the rate. `ceil(460 × 144000 × 1080 / (10⁹ − 460 ×
    /// 144000))` = `ceil(71 551 200 000 / 933 760 000)` =
    /// `ceil(76.6382)` = 77 → vtotal 1157; clock `ceil(2080 × 1157 ×
    /// 144000 / 10⁶)` = `ceil(346.54464 MHz)` = 346 545 kHz.
    #[test]
    fn rb_144hz() {
        let m = cvt_rb(rb(1920, 1080, 144)).unwrap();
        assert_eq!(m.vtotal, 1080 + 77);
        assert_eq!(m.clock_khz, 346_545);
        // Never under: 346 545 000 000 / (2080 × 1157) = 144 000.15 mHz.
        assert_eq!(m.refresh_millihz(), 144_000);
        assert!(m.is_valid());
    }

    /// The low-refresh floor: at 30 Hz the 460 µs solve alone would
    /// leave the vertical blank at 7 lines (zero back porch) — the
    /// structural floor `3 + 4 + 6 = 13` keeps the pour sane.
    /// `ceil(0.0138 × 480 / 0.9862)` = `ceil(6.7217)` = 7 → floored
    /// to 13 → vtotal 493.
    #[test]
    fn rb_low_refresh_floor() {
        let m = cvt_rb(rb(640, 480, 30)).unwrap();
        assert_eq!(m.vtotal, 480 + 13);
        assert_eq!(m.htotal, 800);
        assert!(m.is_valid());
    }

    /// The clock never under-serves: for every pour in a sweep, the
    /// realized refresh is at least the ask (the ceiled clock), and
    /// at most one integer-kHz step of refresh over it.
    #[test]
    fn rb_clock_never_under_serves_materially() {
        for &(w, h, hz) in &[
            (640u32, 480u32, 60u64),
            (800, 600, 60),
            (1024, 768, 60),
            (1280, 720, 60),
            (1600, 900, 60),
            (1920, 1080, 60),
            (2560, 1440, 60),
            (3840, 2160, 60),
            (1920, 1080, 144),
            (1280, 1024, 75),
        ] {
            let m = cvt_rb(rb(w, h, hz)).unwrap();
            let ticks = u64::from(m.htotal) * u64::from(m.vtotal);
            // One kHz of clock, expressed in mHz of refresh at these
            // totals: the over-shoot bound.
            let step = 1_000_000_000 / ticks;
            let ask = hz * 1000;
            assert!(
                m.refresh_millihz() >= ask,
                "{w}x{h}@{hz}: realized {} mHz under ask {ask} mHz",
                m.refresh_millihz()
            );
            assert!(
                m.refresh_millihz() <= ask + step,
                "{w}x{h}@{hz}: realized {} mHz more than {step} mHz over ask",
                m.refresh_millihz()
            );
            assert!(m.is_valid(), "{w}x{h}@{hz} invalid");
        }
    }

    /// Degenerate asks are refused, never truncated: zero axes, an
    /// axis beyond the 16-bit wire, and the absurd-refresh band.
    #[test]
    fn rb_degenerate_asks_refused() {
        assert!(cvt_rb(rb(0, 1080, 60)).is_none());
        assert!(cvt_rb(rb(1920, 0, 60)).is_none());
        assert!(cvt_rb(rb(65_536, 1080, 60)).is_none());
        // 460 × f >= 10⁹: the solve's denominator collapses past
        // 2 173 913 mHz — the checked subtraction refuses.
        assert!(cvt_rb(rb(1920, 1080, 3000)).is_none());
    }

    /// The pour is a *user-defined* mode in the kernel's vocabulary —
    /// the bit the KMS engine's user-mode validation keys on.
    #[test]
    fn rb_pour_is_userdef() {
        let m = cvt_rb(rb(1280, 720, 60)).unwrap();
        assert_eq!(m.kind.0 & ModeType::USERDEF.0, ModeType::USERDEF.0);
        assert_eq!(m.kind.0 & ModeType::PREFERRED.0, 0);
        assert_eq!(m.kind.0 & ModeType::DRIVER.0, 0);
    }

    // ===== Phase 46: the families =====

    /// THE CRT anchor — CVT 1.2 §5.3 on 1920×1080@60, hand-derived
    /// with exact rationals (the spec's own spreadsheet arithmetic):
    ///
    /// * `H_PERIOD_EST = (10¹² − 550000·60000)/(60000·1083)` ns
    ///   = 9.967×10¹¹ / 6.498×10⁷ = 14 881.6 ns;
    /// * `V_SYNC_BP = ROUNDDOWN(550000·60000·1083 / 9.967×10¹¹) + 1`
    ///   = ROUNDDOWN(36.9321) + 1 = 37 (≥ 5 + 6 ✓), vtotal =
    ///   1080 + 37 + 3 = 1120, vfp 3 / vsync 5 (16:9, Table 3-2) /
    ///   vbp 32, vsync edges 1083/1088;
    /// * duty = 30 − 3·14 881.6/10⁴ = 25.535% (≥ 20, no floor);
    ///   `H_BLANK = ROUNDDOWN(1920·0.25535/0.74465/16)·16` = 656 →
    ///   htotal 2576;
    /// * sync `ROUND(8%·2576/8)·8` = 208, front porch 656/2 − 208 =
    ///   120, back porch 328 → edges 2040/2248 (the GTF §7.5
    ///   placement — the classic analog raster);
    /// * clock = `ceil(2576·1120·60000/10⁶)` = 173 108 kHz (the
    ///   VESA 0.25 MHz grid would print 173.00 and land at 59.96 Hz;
    ///   the exact doctrine lands at 60.000016 Hz).
    #[test]
    fn crt_anchor_1080p60() {
        let m = cvt_standard(ask(1920, 1080, 60, TimingFamily::Cvt)).unwrap();
        assert_eq!((m.hdisplay, m.vdisplay), (1920, 1080));
        assert_eq!(m.htotal, 2576);
        assert_eq!(m.vtotal, 1120);
        assert_eq!((m.hsync_start, m.hsync_end), (2040, 2248));
        assert_eq!((m.vsync_start, m.vsync_end), (1083, 1088));
        assert_eq!(m.clock_khz, 173_108);
        assert_eq!(m.refresh_millihz(), 60_000);
        // Table 3-1's CRT row: −hsync, +vsync.
        assert_eq!(m.flags, ModeFlags::NHSYNC | ModeFlags::PVSYNC);
        assert_eq!(m.kind, ModeType::USERDEF);
        assert!(m.is_valid());
    }

    /// The 20% floor branch + the classic raster: CVT-CRT on
    /// 640×480@60 pours the industry-famous 800×500 VGA-class raster.
    /// The duty cycle lands at 19.99% — *under* the floor — so
    /// §5.3 step 13's 20% limit fires: `H_BLANK = ROUNDDOWN(640·
    /// 20/80/16)·16 = 160`, htotal 800; `V_SYNC_BP =
    /// ROUNDDOWN(550000·60000·483/9.967×10¹¹)+1 = 17`, vtotal 500.
    /// Derived: duty = 30 − 3·33 368/10⁴ = 19.989% (< 20 → floor).
    #[test]
    fn crt_floor_branch_pours_the_classic_vga_raster() {
        let m = cvt_standard(ask(640, 480, 60, TimingFamily::Cvt)).unwrap();
        assert_eq!((m.htotal, m.vtotal), (800, 500));
        assert_eq!((m.hsync_start, m.hsync_end), (656, 720));
        assert_eq!((m.vsync_start, m.vsync_end), (483, 487));
        // 4:3 → vsync 4 (Table 3-2).
        assert_eq!(m.vsync_end - m.vsync_start, 4);
        assert_eq!(m.clock_khz, 24_000);
        assert_eq!(m.refresh_millihz(), 60_000);
        assert!(m.is_valid());
    }

    /// The Table 3-2 special case: 1280×1024 (the 5:4 SXGA format)
    /// carries a 7-line vertical sync — the spec's own named row, the
    /// one the height-approximating tooling forgets. Derived by hand:
    /// `V_SYNC_BP = ROUNDDOWN(550000·60000·1027/9.967×10¹¹)+1 = 39`
    /// (≥ 7 + 6 ✓), vtotal 1063, vfp 3 / vsync 7 / vbp 29; duty =
    /// 30 − 3·16 922/10⁴ = 24.92%, `H_BLANK = ROUNDDOWN(1280·0.2492/
    /// 0.7508/16)·16 = 432`? — the exact rational: 1280·d_num/(16·(100·
    /// d_den − d_num)) = 27.01 → 27 → 432 → htotal 1712; sync
    /// `ROUND(8%·1712/8)·8 = 136`, front porch 216−136 = 80 → edges
    /// 1360/1496; clock `ceil(1712·1063·60000/10⁶)` = 109 192 kHz.
    #[test]
    fn crt_1280x1024_carries_the_special_case_sync() {
        let m = cvt_standard(ask(1280, 1024, 60, TimingFamily::Cvt)).unwrap();
        assert_eq!((m.hdisplay, m.vdisplay), (1280, 1024));
        assert_eq!(m.htotal, 1712);
        assert_eq!(m.vtotal, 1063);
        assert_eq!((m.hsync_start, m.hsync_end), (1360, 1496));
        assert_eq!((m.vsync_start, m.vsync_end), (1027, 1034));
        assert_eq!(m.clock_khz, 109_192);
        assert!(m.is_valid());
    }

    /// The Table 3-2 mapping itself: 4:3 → 4, 16:9 → 5, 16:10 → 6
    /// (1920×1200 reduces to 8:5 — the test that guards the ratio
    /// reduction), the two special-case formats → 7, and a
    /// non-standard aspect → 10 (1360×768 is 85:48, *not* 16:9 —
    /// the industry's own "not quite 16:9" panel width).
    #[test]
    fn crt_aspect_table() {
        assert_eq!(cvt_vsync_rnd(640, 480), 4);
        assert_eq!(cvt_vsync_rnd(1920, 1080), 5);
        assert_eq!(cvt_vsync_rnd(2560, 1440), 5);
        assert_eq!(cvt_vsync_rnd(1920, 1200), 6);
        assert_eq!(cvt_vsync_rnd(1280, 1024), 7);
        assert_eq!(cvt_vsync_rnd(1280, 768), 7);
        assert_eq!(cvt_vsync_rnd(1360, 768), 10);
        assert_eq!(cvt_vsync_rnd(1366, 768), 10);
        // The poured mode carries the table's sync (16:10 → 6).
        let m = cvt_standard(ask(1920, 1200, 60, TimingFamily::Cvt)).unwrap();
        assert_eq!(m.vsync_end - m.vsync_start, 6);
        assert_eq!(m.vtotal, 1245);
    }

    /// The cell-granularity doctrine, CVT's own: the horizontal ask
    /// rounds *down* to the character grid (§5.2 step 2 — the
    /// 1366-class ask pours as 1360; the 1-pixel families are the
    /// ones that serve it exactly).
    #[test]
    fn crt_cell_floor_1366() {
        let m = cvt_standard(ask(1366, 768, 60, TimingFamily::Cvt)).unwrap();
        assert_eq!((m.hdisplay, m.vdisplay), (1360, 768));
        assert_eq!(m.htotal, 1776);
        assert!(m.is_valid());
    }

    /// THE GTF anchor — GTF 1.1 §7.3 on 1920×1080@60, hand-derived
    /// (the distinctions from CVT pinned):
    ///
    /// * the period divisor is `V + 1` (GTF's 1-line porch):
    ///   `H_PERIOD_EST = 9.967×10¹¹/(60000·1081) = 14 909 ns`;
    /// * `V_SYNC_BP = ROUND(550000·60000·1081/9.967×10¹¹) =
    ///   ROUND(36.892) = 37` — plain ROUND, no +1 — vtotal =
    ///   1080 + 37 + 1 = 1118, vfp 1 / vsync 3 / vbp 34 (GTF's own
    ///   vertical vocabulary: edges 1081/1084);
    /// * the *refined* period `10¹²/(60000·1118) = 14 902.2 ns`
    ///   (§7.3 step 12, exact in closed form), duty = 30 −
    ///   3·14 902.2/10⁴ = 25.529%, `H_BLANK = ROUND(1920·0.25529/
    ///   0.74471/16)·16 = 656` (ROUND — the same 656 the CVT pour
    ///   lands by ROUNDDOWN) → htotal 2576, edges 2040/2248;
    /// * clock = `ceil(2576·1118·60000/10⁶)` = 172 799 kHz.
    #[test]
    fn gtf_anchor_1080p60() {
        let m = gtf(ask(1920, 1080, 60, TimingFamily::Gtf)).unwrap();
        assert_eq!((m.hdisplay, m.vdisplay), (1920, 1080));
        assert_eq!(m.htotal, 2576);
        assert_eq!(m.vtotal, 1118);
        assert_eq!((m.hsync_start, m.hsync_end), (2040, 2248));
        assert_eq!((m.vsync_start, m.vsync_end), (1081, 1084));
        assert_eq!(m.clock_khz, 172_799);
        assert_eq!(m.refresh_millihz(), 60_000);
        assert_eq!(m.flags, ModeFlags::NHSYNC | ModeFlags::PVSYNC);
        assert!(m.is_valid());
    }

    /// The GTF classic: 1024×768@60 pours the raster the GTF sheet
    /// itself is famous for — 1344×795 (`1024 1080 1184 1344 768 769
    /// 772 795`, the modeline the `gtf` tool prints for this ask):
    /// `H_PERIOD_EST = 9.967×10¹¹/(60000·769) = 21 609 ns`;
    /// `V_SYNC_BP = ROUND(550000·60000·769/9.967×10¹¹) = ROUND(25.45)
    /// = 25`, vtotal 795; refined period `10¹²/(60000·795) =
    /// 20 942 ns`, duty = 30 − 3·20 942/10⁴ = 23.72%, `H_BLANK =
    /// ROUND(1024·0.2372/0.7628/16)·16 = 320`, htotal 1344, sync
    /// `ROUND(8%·1344/8)·8 = 104`, front porch 160 − 104 = 56 →
    /// edges 1080/1184; clock `ceil(1344·795·60000/10⁶)` = 64 109 kHz.
    #[test]
    fn gtf_classic_1024x768() {
        let m = gtf(ask(1024, 768, 60, TimingFamily::Gtf)).unwrap();
        assert_eq!(m.htotal, 1344);
        assert_eq!(m.vtotal, 795);
        assert_eq!((m.hsync_start, m.hsync_end), (1080, 1184));
        assert_eq!((m.vsync_start, m.vsync_end), (769, 772));
        assert_eq!(m.clock_khz, 64_109);
        assert!(m.is_valid());
    }

    /// GTF rounds the horizontal ask to the character grid *to
    /// nearest* (§7.3 step 1 — where CVT rounds down): 1366 → 1368.
    /// The two analog families disagree on the *same ask* — the
    /// distinction the cell doctrine owes a test.
    #[test]
    fn gtf_cell_rounds_to_nearest_1366() {
        let m = gtf(ask(1366, 768, 60, TimingFamily::Gtf)).unwrap();
        assert_eq!((m.hdisplay, m.vdisplay), (1368, 768));
        assert_eq!(m.htotal, 1800);
        // The CVT pour of the same ask is 1360-wide (ROUNDDOWN).
        let c = cvt_standard(ask(1366, 768, 60, TimingFamily::Cvt)).unwrap();
        assert_eq!(c.hdisplay, 1360);
    }

    /// GTF's honest domain limits, both refused (never wrapped):
    /// * the `V_SYNC_BP < 3` band — ROUND would leave the back porch
    ///   negative (a 100×100@24 ask: `ROUND(550000·24000·101/
    ///   (10¹²−1.32×10¹⁰)) = ROUND(1.351) = 1 < 3`);
    /// * the duty-collapse band — the refined period past 100 µs at
    ///   1 Hz on a 5000-line ask (v_sync_bp 3 passes, duty ≤ 0).
    #[test]
    fn gtf_domain_limits_refused() {
        assert!(gtf(ask(100, 100, 24, TimingFamily::Gtf)).is_none());
        assert!(gtf(ask(1920, 5000, 1, TimingFamily::Gtf)).is_none());
        // The CRT family serves the same ask — the 20% floor saves
        // what GTF's floorless formula refuses (the dividend the
        // floor exists to pay).
        assert!(cvt_standard(ask(1920, 5000, 1, TimingFamily::Cvt)).is_some());
    }

    /// THE RB2 anchor — CVT 1.2 §3.4.3 on 1920×1080@60, hand-derived:
    ///
    /// * the 80-pixel blank: hfp 8 / sync 32 / back 40 → edges
    ///   1928/1960, htotal 2000;
    /// * `VBI = ROUNDDOWN(460000·60000·1080/(10¹²−2.76×10¹⁰)) + 1 =
    ///   ROUNDDOWN(30.654) + 1 = 31` (> 15 structural) → vtotal 1111
    ///   — *RB's own total* (the 460 µs solve owns the vertical
    ///   geometry in both RB generations), vfp 17 / vsync 8 / vbp 6
    ///   → edges 1097/1105;
    /// * clock `ceil(2000·1111·60000/10⁶)` = 133 320 kHz — integral,
    ///   60.000 Hz exactly — **3.8% less clock than RB's 138 653**
    ///   at the same raster class (the §3.4.3 economy, measured).
    #[test]
    fn rb2_anchor_1080p60() {
        let m = cvt_rb2(ask(1920, 1080, 60, TimingFamily::Rb2), false).unwrap();
        assert_eq!((m.hdisplay, m.vdisplay), (1920, 1080));
        assert_eq!(m.htotal, 2000);
        assert_eq!(m.vtotal, 1111);
        assert_eq!((m.hsync_start, m.hsync_end), (1928, 1960));
        assert_eq!((m.vsync_start, m.vsync_end), (1097, 1105));
        assert_eq!(m.clock_khz, 133_320);
        assert_eq!(m.refresh_millihz(), 60_000);
        assert_eq!(m.flags, ModeFlags::PHSYNC | ModeFlags::NVSYNC);
        assert!(m.is_valid());
    }

    /// RB2 at 4K: htotal 3920 (3840 + 80), `VBI =
    /// ROUNDDOWN(460000·60000·2160/9.724×10¹¹)+1 = ROUNDDOWN(61.307)+1
    /// = 62` → vtotal 2222 (RB's own), edges 2208/2216 (vfp 48,
    /// vsync 8, vbp 6); clock `ceil(3920·2222·60000/10⁶)` =
    /// 522 615 kHz — RB poured 533 280 at the same size: the deep-
    /// color-era economy again, −2%.
    #[test]
    fn rb2_4k60() {
        let m = cvt_rb2(ask(3840, 2160, 60, TimingFamily::Rb2), false).unwrap();
        assert_eq!(m.htotal, 3920);
        assert_eq!(m.vtotal, 2222);
        assert_eq!((m.hsync_start, m.hsync_end), (3848, 3880));
        assert_eq!((m.vsync_start, m.vsync_end), (2208, 2216));
        assert_eq!(m.clock_khz, 522_615);
        assert!(m.is_valid());
    }

    /// The 1-pixel precision (§3.4.3 item 3): 1366 pours *exactly* —
    /// the width no cell-granularity family can serve — htotal 1446.
    #[test]
    fn rb2_pixel_precision_1366() {
        let m = cvt_rb2(ask(1366, 768, 60, TimingFamily::Rb2), false).unwrap();
        assert_eq!((m.hdisplay, m.vdisplay), (1366, 768));
        assert_eq!(m.htotal, 1446);
        assert!(m.is_valid());
    }

    /// The structural floor: a 30 Hz 480-line ask where the 460 µs
    /// solve alone would give 8 lines (`ROUNDDOWN(460000·30000·480/
    /// (10¹²−1.38×10¹⁰))+1 = 8`) — the §3.4.3 vocabulary floors at
    /// `1 + 8 + 6 = 15` (vfp 1, vsync 8, vbp 6), vtotal 495.
    #[test]
    fn rb2_structural_floor() {
        let m = cvt_rb2(ask(640, 480, 30, TimingFamily::Rb2), false).unwrap();
        assert_eq!(m.vtotal, 480 + 15);
        assert_eq!(m.vsync_end - m.vsync_start, 8);
        assert_eq!(m.vtotal - m.vsync_end, 6, "the fixed 6-line back porch");
        assert!(m.is_valid());
    }

    /// The video-optimized multiplier (§3.4.3 item 1): geometry
    /// bit-identical to the plain RB2 pour (the spec's own guarantee
    /// — "the only difference is in pixel clock"), the clock scaled
    /// by 1000/1001: `ceil(2000·1111·60000·1000/(1001·10⁶))` =
    /// 133 187 kHz, realizing 59 940 mHz — never under the
    /// ×1000/1001 target (the exact-rational bound, asserted as
    /// integers: 1001·clock·10⁶ ≥ 1000·f·ticks).
    #[test]
    fn rb2_video_optimized_moves_only_the_clock() {
        let plain = cvt_rb2(ask(1920, 1080, 60, TimingFamily::Rb2), false).unwrap();
        let video = cvt_rb2(ask(1920, 1080, 60, TimingFamily::Rb2Video), true).unwrap();
        assert_eq!(
            (
                video.htotal,
                video.vtotal,
                video.hsync_start,
                video.hsync_end,
                video.vsync_start,
                video.vsync_end
            ),
            (
                plain.htotal,
                plain.vtotal,
                plain.hsync_start,
                plain.hsync_end,
                plain.vsync_start,
                plain.vsync_end
            )
        );
        assert_eq!(video.clock_khz, 133_187);
        assert!(video.clock_khz < plain.clock_khz);
        assert_eq!(video.refresh_millihz(), 59_940);
        // Never under the video-optimized target, exactly:
        let ticks = u64::from(video.htotal) * u64::from(video.vtotal);
        assert!(u64::from(video.clock_khz) * 1001 * 1_000_000 >= 60_000 * 1000 * ticks);
        assert!(video.is_valid());
    }

    /// THE family distinctness: the same ask, four answers — the
    /// blanking economies the standards encode, measured on one
    /// geometry. RB2's 80-pixel blank beats RB's 160 (−3.8% clock),
    /// both beat the analog families' 656; the two analog families
    /// share the blank (the GTF duty machinery both carry) and
    /// disagree on the vertical vocabulary (1120 vs 1118). Every
    /// pour is a distinct, valid, user-defined mode.
    #[test]
    fn families_distinct_at_the_same_ask() {
        let r = cvt_rb(rb(1920, 1080, 60)).unwrap();
        let r2 = cvt_rb2(ask(1920, 1080, 60, TimingFamily::Rb2), false).unwrap();
        let c = cvt_standard(ask(1920, 1080, 60, TimingFamily::Cvt)).unwrap();
        let g = gtf(ask(1920, 1080, 60, TimingFamily::Gtf)).unwrap();
        // Four distinct rasters.
        let totals = [
            (r.htotal, r.vtotal),
            (r2.htotal, r2.vtotal),
            (c.htotal, c.vtotal),
            (g.htotal, g.vtotal),
        ];
        assert_eq!(totals[0], (2080, 1111), "RB");
        assert_eq!(totals[1], (2000, 1111), "RB2");
        assert_eq!(totals[2], (2576, 1120), "CVT");
        assert_eq!(totals[3], (2576, 1118), "GTF");
        // The clock economy, ordered.
        assert!(r2.clock_khz < r.clock_khz, "RB2 beats RB");
        assert!(r.clock_khz < g.clock_khz, "RB beats the analog pair");
        assert!(g.clock_khz < c.clock_khz, "GTF's shorter frame beats CVT's");
        // The analog pair share the blank, disagree vertically.
        assert_eq!(c.htotal - 1920, 656);
        assert_eq!(g.htotal - 1920, 656);
        // All four are user-defined, all valid.
        for m in [&r, &r2, &c, &g] {
            assert_eq!(m.kind, ModeType::USERDEF);
            assert!(m.is_valid());
            assert_eq!(m.refresh_millihz(), 60_000);
        }
    }

    /// The foundry's front door: `pour` dispatches on the family and
    /// reproduces each family function bit-exactly.
    #[test]
    fn pour_dispatches_on_the_family() {
        for (family, expect) in [
            (TimingFamily::Rb, cvt_rb(rb(1920, 1080, 60))),
            (
                TimingFamily::Rb2,
                cvt_rb2(ask(1920, 1080, 60, TimingFamily::Rb2), false),
            ),
            (
                TimingFamily::Rb2Video,
                cvt_rb2(ask(1920, 1080, 60, TimingFamily::Rb2Video), true),
            ),
            (
                TimingFamily::Cvt,
                cvt_standard(ask(1920, 1080, 60, TimingFamily::Cvt)),
            ),
            (
                TimingFamily::Gtf,
                gtf(ask(1920, 1080, 60, TimingFamily::Gtf)),
            ),
        ] {
            let via_pour = pour(ask(1920, 1080, 60, family));
            assert_eq!(via_pour, expect, "pour({family:?}) dispatches");
        }
    }

    /// The clock never under-serves in any family: a sweep across the
    /// sizes and rates, every family, the same two-sided bound as
    /// RB's own sweep (never under the ask, at most one integer-kHz
    /// step over it).
    #[test]
    fn every_family_never_under_serves() {
        for &(w, h, hz) in &[
            (640u32, 480u32, 60u64),
            (1024, 768, 60),
            (1280, 1024, 60),
            (1366, 768, 60),
            (1920, 1080, 60),
            (1920, 1080, 75),
            (1920, 1200, 60),
            (2560, 1440, 60),
            (3840, 2160, 60),
            (1920, 1080, 144),
        ] {
            for family in [
                TimingFamily::Rb,
                TimingFamily::Rb2,
                TimingFamily::Cvt,
                TimingFamily::Gtf,
            ] {
                // The cell families round the horizontal ask — the
                // realized raster is the rounded one; the refresh
                // doctrine is unchanged by it.
                let m = pour(ask(w, h, hz, family))
                    .unwrap_or_else(|| panic!("{w}x{h}@{hz} {family:?} refused"));
                let ticks = u64::from(m.htotal) * u64::from(m.vtotal);
                let step = 1_000_000_000 / ticks;
                let ask_hz = hz * 1000;
                assert!(
                    m.refresh_millihz() >= ask_hz,
                    "{w}x{h}@{hz} {family:?}: {} mHz under ask {ask_hz} mHz",
                    m.refresh_millihz()
                );
                assert!(
                    m.refresh_millihz() <= ask_hz + step,
                    "{w}x{h}@{hz} {family:?}: {} mHz over ask by more than {step}",
                    m.refresh_millihz()
                );
                assert!(m.is_valid(), "{w}x{h}@{hz} {family:?} invalid");
            }
        }
    }

    /// The degenerate vocabulary is every family's: zero axes and
    /// 16-bit-wire escapes refuse in all five (the early gate), and
    /// the absurd-refresh band refuses through each family's own
    /// denominator collapse.
    #[test]
    fn degenerate_asks_refuse_in_every_family() {
        for family in [
            TimingFamily::Rb,
            TimingFamily::Rb2,
            TimingFamily::Rb2Video,
            TimingFamily::Cvt,
            TimingFamily::Gtf,
        ] {
            assert!(pour(ask(0, 1080, 60, family)).is_none(), "{family:?}");
            assert!(pour(ask(1920, 0, 60, family)).is_none(), "{family:?}");
            assert!(pour(ask(65_536, 1080, 60, family)).is_none(), "{family:?}");
            assert!(pour(ask(1920, 1080, 3000, family)).is_none(), "{family:?}");
        }
    }

    /// The CLI grammar: `WxH`, `WxH@Hz`, the family suffix, the
    /// defaults, and the honest refusals (malformed size, malformed
    /// rate, zero rate, the beyond-1000 Hz band, the unknown family).
    #[test]
    fn cli_grammar() {
        let plain = SynthRequest::parse("1920x1080").unwrap();
        assert_eq!(
            (
                plain.width,
                plain.height,
                plain.refresh_millihz,
                plain.family
            ),
            (1920, 1080, 60_000, TimingFamily::Rb)
        );
        let rated = SynthRequest::parse("2560x1440@144").unwrap();
        assert_eq!(
            (
                rated.width,
                rated.height,
                rated.refresh_millihz,
                rated.family
            ),
            (2560, 1440, 144_000, TimingFamily::Rb)
        );
        assert_eq!(rated.as_str(), "2560x1440@144.0");
        // The families.
        let rb2 = SynthRequest::parse("1920x1080@60:rb2").unwrap();
        assert_eq!(
            (rb2.width, rb2.height, rb2.refresh_millihz, rb2.family),
            (1920, 1080, 60_000, TimingFamily::Rb2)
        );
        assert_eq!(rb2.as_str(), "1920x1080@60.0:rb2");
        assert_eq!(
            SynthRequest::parse("2560x1440:gtf").unwrap().family,
            TimingFamily::Gtf
        );
        assert_eq!(
            SynthRequest::parse("1920x1080@144:rb2v").unwrap().family,
            TimingFamily::Rb2Video
        );
        assert_eq!(
            SynthRequest::parse("1920x1200:cvt").unwrap().family,
            TimingFamily::Cvt
        );
        // The default family's report spelling is Phase 42's own
        // (no suffix — the pinned strings stay byte-identical).
        assert_eq!(
            SynthRequest::parse("1920x1080@60").unwrap().as_str(),
            "1920x1080@60.0"
        );
        // The honest refusals.
        assert!(SynthRequest::parse("1920").is_err());
        assert!(SynthRequest::parse("1920x1080@fast").is_err());
        assert!(SynthRequest::parse("1920x1080@0").is_err());
        assert!(SynthRequest::parse("1920x1080@1440").is_err());
        assert!(SynthRequest::parse("0x0@60").is_err());
        assert!(SynthRequest::parse("1920x1080:vesa").is_err());
        assert!(
            SynthRequest::parse("1920x1080@60:RB2").is_err(),
            "the family spelling is lowercase"
        );
        // The error names the valid families.
        let err = SynthRequest::parse("1920x1080:vesa").unwrap_err();
        assert!(err.contains("rb2v") && err.contains("gtf"), "{err}");
    }

    /// The family's own vocabulary: the CLI spelling round-trips and
    /// the spec names report.
    #[test]
    fn family_vocabulary() {
        for family in [
            TimingFamily::Rb,
            TimingFamily::Rb2,
            TimingFamily::Rb2Video,
            TimingFamily::Cvt,
            TimingFamily::Gtf,
        ] {
            assert_eq!(TimingFamily::parse(family.as_str()).unwrap(), family);
            assert!(!family.spec_name().is_empty());
            assert!(!family.to_string().is_empty());
        }
    }
}
