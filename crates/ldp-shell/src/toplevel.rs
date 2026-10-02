//! The toplevel machine: state flags and the configure/ack commit.
//!
//! Two-phase commit (architecture §8): the server *proposes* state
//! with `configure(serial, states, size, insets, workspace, output)`;
//! the client `ack_configure(serial)`s; the client's next commit
//! *realizes* the last-acked proposal. At most one proposal is live at
//! a time — a new configure supersedes (and kills the serial of) the
//! previous one, which is what makes stale acks detectable exactly.
//!
//! Request handlers are *intents*: `maximize`/`fullscreen`/`minimize`
//! flip the wanted flags; the machine derives the proposal (geometry,
//! states, insets) from policy inputs (workspace area, output size,
//! SSD metrics at the output's scale) and hands it to the integrator
//! to emit. Geometry honors `min_size`/`max_size` clamps — the client
//! may shrink a proposal, the shell never proposes below the minimum
//! the client itself declared.

#![forbid(unsafe_code)]

use std::collections::VecDeque;

use ldp_core::bitset::Bitset128;
use ldp_core::geometry::Rect;
use ldp_core::ids::ObjectId;
use ldp_core::scale::ScaleFactor;

use crate::serial::{Serial, SerialClock};
use crate::ssd::{bounded_string, DecorationMode, Insets, SsdMetrics};
use crate::WindowKey;

/// The `toplevel_states` bitset (wire indices).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ToplevelStates(pub Bitset128);

impl ToplevelStates {
    /// Bit indices, frozen by the spec.
    pub const MAXIMIZED: u32 = 0;
    /// Fullscreen bit.
    pub const FULLSCREEN: u32 = 1;
    /// Minimized bit.
    pub const MINIMIZED: u32 = 2;
    /// Activated bit.
    pub const ACTIVATED: u32 = 3;
    /// Sticky bit.
    pub const STICKY: u32 = 4;
    /// Resizing bit.
    pub const RESIZING: u32 = 5;

    /// No flags set.
    #[must_use]
    pub const fn none() -> ToplevelStates {
        ToplevelStates(Bitset128::from_words([0, 0, 0, 0]))
    }

    /// Build from single flags.
    ///
    /// Six booleans, one per spec bit: the signature *is* the bitset
    /// (callers read better than a second nested struct).
    #[allow(clippy::fn_params_excessive_bools)]
    #[must_use]
    pub fn build(
        maximized: bool,
        fullscreen: bool,
        minimized: bool,
        activated: bool,
        sticky: bool,
        resizing: bool,
    ) -> ToplevelStates {
        let mut b = Bitset128::default();
        b.set(Self::MAXIMIZED, maximized);
        b.set(Self::FULLSCREEN, fullscreen);
        b.set(Self::MINIMIZED, minimized);
        b.set(Self::ACTIVATED, activated);
        b.set(Self::STICKY, sticky);
        b.set(Self::RESIZING, resizing);
        ToplevelStates(b)
    }

    /// Whether a flag is set.
    #[must_use]
    pub fn test(self, bit: u32) -> bool {
        self.0.test(bit)
    }

    /// Whether the window is maximized.
    #[must_use]
    pub fn maximized(self) -> bool {
        self.test(Self::MAXIMIZED)
    }

    /// Whether the window is fullscreen.
    #[must_use]
    pub fn fullscreen(self) -> bool {
        self.test(Self::FULLSCREEN)
    }

    /// Whether the window is minimized.
    #[must_use]
    pub fn minimized(self) -> bool {
        self.test(Self::MINIMIZED)
    }

    /// Whether the window is activated (has focus).
    #[must_use]
    pub fn activated(self) -> bool {
        self.test(Self::ACTIVATED)
    }

    /// Whether the window is sticky (on all spaces).
    #[must_use]
    pub fn sticky(self) -> bool {
        self.test(Self::STICKY)
    }

    /// Whether a user resize is in progress.
    #[must_use]
    pub fn resizing(self) -> bool {
        self.test(Self::RESIZING)
    }
}

/// One `configure` proposal (what the integrator emits and the client
/// acks).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Configure {
    /// The proposal serial.
    pub serial: Serial,
    /// The proposed state flags.
    pub states: ToplevelStates,
    /// Content size (0 = client chooses).
    pub width: u32,
    /// Content size (0 = client chooses).
    pub height: u32,
    /// Content insets (SSD chrome or CSD reservation).
    pub insets: Insets,
    /// The space the window is on.
    pub workspace: u32,
    /// The output the window is primarily on.
    pub output: Option<ObjectId>,
}

/// Toplevel misuse (protocol errors).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToplevelError {
    /// The acked serial is not the live proposal (stale or unknown).
    StaleAck,
    /// A string argument exceeded the protocol budget or embedded NUL.
    BadString,
}

impl std::fmt::Display for ToplevelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToplevelError::StaleAck => f.write_str("ack_configure serial is not the live proposal"),
            ToplevelError::BadString => f.write_str("string argument rejected"),
        }
    }
}

impl std::error::Error for ToplevelError {}

/// The interactive drag's acknowledgment grace window depth (Phase
/// 50): how many superseded *drag* proposals stay acknowledgeable. A
/// pointer-paced resize supersedes proposals at the operator's hand
/// speed; a client draining at frame cadence sits a handful behind —
/// 8 covers every honest lag (the device's own batching coalesces the
/// samples before the pump sees them), and anything 9-back is
/// genuinely stale.
const GRACE_WINDOW: usize = 8;

/// The `resize_edge` wire enum (Phase 50's `start_resize`): the border
/// or corner an interactive resize drives — the xdg_toplevel
/// resize_edges vocabulary, the WM's `_NET_WM_MOVERESIZE` edges before
/// it. A plain edge moves one border; a corner moves two; the
/// opposite corner anchors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResizeEdge {
    /// The top border follows the pointer; the bottom anchors.
    Top,
    /// The bottom border follows; the top anchors.
    Bottom,
    /// The left border follows; the right anchors.
    Left,
    /// The right border follows; the left anchors.
    Right,
    /// The top-left corner follows; the bottom-right anchors.
    TopLeft,
    /// The top-right corner follows; the bottom-left anchors.
    TopRight,
    /// The bottom-left corner follows; the top-right anchors.
    BottomLeft,
    /// The bottom-right corner follows; the top-left anchors.
    BottomRight,
}

impl ResizeEdge {
    /// From the wire value (the spec's `resize_edge` enum, 1..=8).
    /// `None` names every value the schema does not carry — the
    /// [`Material::from_wire`] convention verbatim.
    #[must_use]
    pub const fn from_wire(value: u32) -> Option<ResizeEdge> {
        Some(match value {
            1 => ResizeEdge::Top,
            2 => ResizeEdge::Bottom,
            3 => ResizeEdge::Left,
            4 => ResizeEdge::Right,
            5 => ResizeEdge::TopLeft,
            6 => ResizeEdge::TopRight,
            7 => ResizeEdge::BottomLeft,
            8 => ResizeEdge::BottomRight,
            _ => return None,
        })
    }

    /// Whether the horizontal axis engages (left or right borders
    /// follow the pointer).
    #[must_use]
    pub const fn horizontal(self) -> bool {
        matches!(
            self,
            ResizeEdge::Left
                | ResizeEdge::Right
                | ResizeEdge::TopLeft
                | ResizeEdge::TopRight
                | ResizeEdge::BottomLeft
                | ResizeEdge::BottomRight
        )
    }

    /// Whether the vertical axis engages (top or bottom borders
    /// follow the pointer).
    #[must_use]
    pub const fn vertical(self) -> bool {
        matches!(
            self,
            ResizeEdge::Top
                | ResizeEdge::Bottom
                | ResizeEdge::TopLeft
                | ResizeEdge::TopRight
                | ResizeEdge::BottomLeft
                | ResizeEdge::BottomRight
        )
    }

    /// Whether the *left* border follows (the window's origin moves
    /// with the pointer horizontally).
    #[must_use]
    pub const fn grabs_left(self) -> bool {
        matches!(
            self,
            ResizeEdge::Left | ResizeEdge::TopLeft | ResizeEdge::BottomLeft
        )
    }

    /// Whether the *top* border follows (the window's origin moves
    /// with the pointer vertically).
    #[must_use]
    pub const fn grabs_top(self) -> bool {
        matches!(
            self,
            ResizeEdge::Top | ResizeEdge::TopLeft | ResizeEdge::TopRight
        )
    }
}

impl std::fmt::Display for ResizeEdge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            ResizeEdge::Top => "top",
            ResizeEdge::Bottom => "bottom",
            ResizeEdge::Left => "left",
            ResizeEdge::Right => "right",
            ResizeEdge::TopLeft => "top_left",
            ResizeEdge::TopRight => "top_right",
            ResizeEdge::BottomLeft => "bottom_left",
            ResizeEdge::BottomRight => "bottom_right",
        };
        f.write_str(name)
    }
}

/// The policy inputs for deriving a proposal.
#[derive(Clone, Copy, Debug)]
pub struct PolicyInputs {
    /// The workspace area (usable region minus panels/dock), logical.
    pub workspace_area: Rect,
    /// The full output size, logical.
    pub output_size: (u32, u32),
    /// Decoration metrics + mode + the output's scale factor.
    pub metrics: SsdMetrics,
    /// Decoration mode chosen at creation.
    pub decoration: DecorationMode,
    /// Output scale (mixed-DPI: insets derive per output).
    pub scale: ScaleFactor,
    /// The space the window is on.
    pub workspace: u32,
    /// The output object.
    pub output: Option<ObjectId>,
}

/// A toplevel window: flags, strings, size hints, and the commit
/// lifecycle.
#[derive(Debug)]
pub struct Toplevel {
    key: WindowKey,
    serials: SerialClock,
    /// The live proposal (a new configure replaces it).
    pending: Option<Configure>,
    /// The last acked proposal (realized by the next commit).
    acked: Option<Configure>,
    /// The realized state (applied by the last commit).
    applied: Option<Configure>,
    /// The interactive drag's acknowledgment grace window (Phase 50):
    /// the last few *superseded* drag proposals, still acknowledgeable
    /// — a pointer-paced resize replaces proposals at the operator's
    /// hand speed, and a client that acks the configure it actually
    /// saw (already superseded by the time the ack arrives) is never
    /// at fault. The window is bounded (the drag's own cadence makes
    /// anything 8-back genuinely stale), cleared by every verb
    /// proposal — the operator's verbs are authoritative, the Phase 49
    /// strict-ack doctrine intact for them.
    superseded: VecDeque<Configure>,
    /// Wanted flags (intents; proposals carry them).
    wanted: ToplevelStates,
    /// Window title.
    title: Box<str>,
    /// Application identity.
    app_id: Box<str>,
    /// Minimum content size (0 = unconstrained per axis).
    min_size: (u32, u32),
    /// Maximum content size (0 = unconstrained per axis).
    max_size: (u32, u32),
    /// The requested material (Phase 45): the client's identity
    /// claim for its own chrome — `None` is the default (the server's
    /// own resolution rules dress the window).
    material: Option<Material>,
    /// The claimed semantic role (Phase 47): what this window *is* —
    /// `None` is no claim (the plain window doctrine).
    semantic_role: Option<SemanticRole>,
    /// The claimed security class (Phase 47): what this window may
    /// expose — `None` is no claim (the server treats it as normal).
    security_class: Option<SecurityClass>,
    /// The claimed scene profile (Phase 47): how the machine spends
    /// its frame budget here — `None` is no claim (the operator's
    /// configured policy stands).
    scene_profile: Option<SceneProfile>,
}

/// The per-surface material vocabulary (Phase 45's wire enum,
/// `toplevel.set_material`): the client's identity claim for its
/// window's chrome — the macOS NSVisualEffectView doctrine. The
/// machine carries the claim; the *server* owns the quality (the
/// effects tier dresses what the machine can afford).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Material {
    /// An opaque window: corners and a shadow.
    Panel,
    /// A translucent surface: the frosted sheet.
    Sheet,
    /// A menu or popup: the vibrant light glass with the hairline.
    Menu,
    /// A dark control-center panel: the vibrant dark glass.
    VibrantDark,
    /// The system chrome (the dock's material).
    Chrome,
}

impl Material {
    /// From a wire value (the spec's `material` enum, 2..=6 — the
    /// `default` clear at 1 is the *caller's* arm, the None-claim
    /// itself). `None` names every value the schema does not carry.
    /// The [`Anchor::from_wire`](crate::Anchor::from_wire)
    /// convention verbatim.
    #[must_use]
    pub const fn from_wire(value: u32) -> Option<Material> {
        Some(match value {
            2 => Material::Panel,
            3 => Material::Sheet,
            4 => Material::Menu,
            5 => Material::VibrantDark,
            6 => Material::Chrome,
            _ => return None,
        })
    }
}

/// The `semantic_role` wire enum (Phase 47's `set_semantic_role`):
/// what a surface *is* beyond its structure — the client's identity
/// claim, carried by the machine, enforced by the server (the
/// ephemeral roles never animate in; the lock role floors the
/// security class).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SemanticRole {
    /// A plain window — the default doctrine.
    Window,
    /// A transient dialog of the desktop's flow.
    Dialog,
    /// A tooltip — instant appearance, never transitions.
    Tooltip,
    /// A floating overlay — instant like the tooltip.
    Overlay,
    /// The lock surface — the security floor applies.
    Lock,
}

impl SemanticRole {
    /// From the wire value (the spec's `semantic_role` enum, 1..=5).
    /// `None` names every value the schema does not carry — the
    /// [`Material::from_wire`] convention verbatim.
    #[must_use]
    pub const fn from_wire(value: u32) -> Option<SemanticRole> {
        Some(match value {
            1 => SemanticRole::Window,
            2 => SemanticRole::Dialog,
            3 => SemanticRole::Tooltip,
            4 => SemanticRole::Overlay,
            5 => SemanticRole::Lock,
            _ => return None,
        })
    }
}

/// The `security_class` wire enum (Phase 47's `set_security_class`):
/// what a surface may *expose* — the capture-path ladder the
/// security-aware compositor enforces at the one seam it owns
/// outright (what becomes visible).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecurityClass {
    /// Ordinary content — captures whole (the default).
    Normal,
    /// Sensitive content — classifies for screen-share negotiation.
    Private,
    /// Protected content — never composited into a capture frame.
    Protected,
    /// System content — the server chrome's class, redacts like
    /// protected.
    System,
}

impl SecurityClass {
    /// From the wire value (the spec's `security_class` enum, 1..=4).
    /// `None` names every value the schema does not carry.
    #[must_use]
    pub const fn from_wire(value: u32) -> Option<SecurityClass> {
        Some(match value {
            1 => SecurityClass::Normal,
            2 => SecurityClass::Private,
            3 => SecurityClass::Protected,
            4 => SecurityClass::System,
            _ => return None,
        })
    }
}

/// The `scene_profile` wire enum (Phase 47's `set_scene_profile`):
/// how the machine spends its frame budget on one surface — the
/// per-surface admission floor the deadline walk demands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneProfile {
    /// The operator's configured policy — no added floor (the
    /// default).
    Desktop,
    /// A 4 ms floor — stable pacing over single-frame latency.
    Creative,
    /// A 1 ms floor — the tightest makeable admission.
    Gaming,
}

impl SceneProfile {
    /// From the wire value (the spec's `scene_profile` enum, 1..=3).
    /// `None` names every value the schema does not carry.
    #[must_use]
    pub const fn from_wire(value: u32) -> Option<SceneProfile> {
        Some(match value {
            1 => SceneProfile::Desktop,
            2 => SceneProfile::Creative,
            3 => SceneProfile::Gaming,
            _ => return None,
        })
    }
}

impl Toplevel {
    /// A new toplevel with default state.
    #[must_use]
    pub fn new(key: WindowKey) -> Toplevel {
        Toplevel {
            key,
            serials: SerialClock::new(),
            pending: None,
            acked: None,
            applied: None,
            superseded: VecDeque::new(),
            wanted: ToplevelStates::none(),
            title: Box::from(""),
            app_id: Box::from(""),
            min_size: (0, 0),
            max_size: (0, 0),
            material: None,
            semantic_role: None,
            security_class: None,
            scene_profile: None,
        }
    }

    /// The window key.
    #[must_use]
    pub const fn key(&self) -> WindowKey {
        self.key
    }

    /// The live proposal, if any.
    #[must_use]
    pub fn pending(&self) -> Option<Configure> {
        self.pending
    }

    /// The last acked (not yet realized) proposal.
    #[must_use]
    pub fn acked(&self) -> Option<Configure> {
        self.acked
    }

    /// The realized state (from the last applying commit).
    #[must_use]
    pub fn applied(&self) -> Option<Configure> {
        self.applied
    }

    /// The wanted flags.
    #[must_use]
    pub fn wanted(&self) -> ToplevelStates {
        self.wanted
    }

    /// The title.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    /// The app id.
    #[must_use]
    pub fn app_id(&self) -> &str {
        &self.app_id
    }

    /// The min size hint (0 = unconstrained per axis).
    #[must_use]
    pub fn min_size(&self) -> (u32, u32) {
        self.min_size
    }

    /// The max size hint (0 = unconstrained per axis).
    #[must_use]
    pub fn max_size(&self) -> (u32, u32) {
        self.max_size
    }

    /// `set_title`.
    ///
    /// # Errors
    /// [`ToplevelError::BadString`] when the title is rejected.
    pub fn set_title(&mut self, title: &str) -> Result<(), ToplevelError> {
        self.title = bounded_string(title).map_err(|_| ToplevelError::BadString)?;
        Ok(())
    }

    /// `set_app_id`.
    ///
    /// # Errors
    /// [`ToplevelError::BadString`] when the id is rejected.
    pub fn set_app_id(&mut self, app_id: &str) -> Result<(), ToplevelError> {
        self.app_id = bounded_string(app_id).map_err(|_| ToplevelError::BadString)?;
        Ok(())
    }

    /// `set_min_size`. A minimum above an established maximum
    /// saturates at that maximum (a window that must be ≤ 400 wide
    /// cannot simultaneously require > 400) — the hint pair stays
    /// consistent by construction, whatever the client sends.
    pub fn set_min_size(&mut self, width: u32, height: u32) {
        let (mw, mh) = self.max_size;
        self.min_size = (
            if mw > 0 { width.min(mw) } else { width },
            if mh > 0 { height.min(mh) } else { height },
        );
    }

    /// `set_max_size`. A maximum below an established minimum rises
    /// to that minimum; zero (unconstrained) passes through.
    pub fn set_max_size(&mut self, width: u32, height: u32) {
        let (mnw, mnh) = self.min_size;
        self.max_size = (
            if width > 0 { width.max(mnw) } else { width },
            if height > 0 { height.max(mnh) } else { height },
        );
    }

    /// Intent: `maximize`.
    pub fn maximize(&mut self) {
        self.wanted.0.set(ToplevelStates::MAXIMIZED, true);
    }

    /// Intent: `unmaximize`.
    pub fn unmaximize(&mut self) {
        self.wanted.0.set(ToplevelStates::MAXIMIZED, false);
    }

    /// Intent: `fullscreen`.
    pub fn fullscreen(&mut self) {
        self.wanted.0.set(ToplevelStates::FULLSCREEN, true);
    }

    /// Intent: `unfullscreen`.
    pub fn unfullscreen(&mut self) {
        self.wanted.0.set(ToplevelStates::FULLSCREEN, false);
    }

    /// Intent: `minimize`. Minimized clears activation: a hidden
    /// window never holds the keyboard.
    pub fn minimize(&mut self) {
        self.wanted.0.set(ToplevelStates::MINIMIZED, true);
        self.wanted.0.set(ToplevelStates::ACTIVATED, false);
    }

    /// Intent: `unminimize` (the shell decides when to restore).
    pub fn unminimize(&mut self) {
        self.wanted.0.set(ToplevelStates::MINIMIZED, false);
    }

    /// Intent: `set_sticky`.
    pub fn set_sticky(&mut self, sticky: bool) {
        self.wanted.0.set(ToplevelStates::STICKY, sticky);
    }

    /// `set_material` (Phase 45): the client's material claim for its
    /// own chrome. `None` is the clear — the server's own resolution
    /// rules dress the window again.
    pub fn set_material(&mut self, material: Option<Material>) {
        self.material = material;
    }

    /// The requested material (`None`: none requested).
    #[must_use]
    pub const fn material(&self) -> Option<Material> {
        self.material
    }

    /// `set_semantic_role` (Phase 47): the client's identity claim —
    /// what this window *is*. `None` is the clear (the plain window
    /// doctrine). The server keeps its own invariants over the claim
    /// (the ephemeral roles never animate in; the lock role floors
    /// the security class).
    pub fn set_semantic_role(&mut self, role: Option<SemanticRole>) {
        self.semantic_role = role;
    }

    /// The claimed semantic role (`None`: none claimed).
    #[must_use]
    pub const fn semantic_role(&self) -> Option<SemanticRole> {
        self.semantic_role
    }

    /// `set_security_class` (Phase 47): the client's exposure claim —
    /// what this window may show to captures. `None` is the clear
    /// (the server treats the surface as normal).
    pub fn set_security_class(&mut self, class: Option<SecurityClass>) {
        self.security_class = class;
    }

    /// The claimed security class (`None`: none claimed).
    #[must_use]
    pub const fn security_class(&self) -> Option<SecurityClass> {
        self.security_class
    }

    /// `set_scene_profile` (Phase 47): the client's frame-budget
    /// claim. `None` is the clear (the operator's configured policy
    /// stands).
    pub fn set_scene_profile(&mut self, profile: Option<SceneProfile>) {
        self.scene_profile = profile;
    }

    /// The claimed scene profile (`None`: none claimed).
    #[must_use]
    pub const fn scene_profile(&self) -> Option<SceneProfile> {
        self.scene_profile
    }

    /// Activation policy (the stacking layer calls this): set the
    /// activated flag intent.
    pub fn set_activated(&mut self, activated: bool) {
        self.wanted.0.set(ToplevelStates::ACTIVATED, activated);
        if activated {
            // An activated window is not hidden.
            self.wanted.0.set(ToplevelStates::MINIMIZED, false);
        }
    }

    /// Mark a user resize in progress (the `resizing` flag rides the
    /// next proposal).
    pub fn set_resizing(&mut self, resizing: bool) {
        self.wanted.0.set(ToplevelStates::RESIZING, resizing);
    }

    /// Derive the content size for the wanted flags under `inputs`,
    /// honoring the client's size hints. Fullscreen precedes maximized
    /// (fullscreen covers the whole output including chrome; maximized
    /// fills the workspace area minus decorations). Otherwise the
    /// client chooses (0, 0).
    #[must_use]
    pub fn derive_size(&self, inputs: &PolicyInputs) -> (u32, u32) {
        let (w, h) = if self.wanted.fullscreen() {
            (inputs.output_size.0, inputs.output_size.1)
        } else if self.wanted.maximized() {
            let insets = inputs.metrics.insets(inputs.decoration, inputs.scale);
            (
                inputs.workspace_area.w.saturating_sub(insets.width()),
                inputs.workspace_area.h.saturating_sub(insets.height()),
            )
        } else {
            return (0, 0);
        };
        self.clamp_size(w, h)
    }

    /// Clamp a content size to the min/max hints (0 stays 0: "client
    /// chooses" is not clamped away; a 0 hint means unconstrained).
    #[must_use]
    pub fn clamp_size(&self, w: u32, h: u32) -> (u32, u32) {
        if w == 0 || h == 0 {
            return (w, h);
        }
        let mut w = w;
        let mut h = h;
        if self.min_size.0 > 0 {
            w = w.max(self.min_size.0);
        }
        if self.min_size.1 > 0 {
            h = h.max(self.min_size.1);
        }
        if self.max_size.0 > 0 {
            w = w.min(self.max_size.0);
        }
        if self.max_size.1 > 0 {
            h = h.min(self.max_size.1);
        }
        (w, h)
    }

    /// Propose: build the next configure from the wanted flags and
    /// `inputs`, replacing any live proposal (its serial dies — stale
    /// acks become detectable). The verb's proposal is authoritative:
    /// the drag's grace window clears with it (Phase 50 — the Phase 49
    /// strict-ack doctrine holds whole for verb-superseded serials).
    /// Returns the proposal to emit.
    #[must_use]
    pub fn propose(&mut self, inputs: &PolicyInputs) -> Configure {
        let serial = self.serials.issue();
        self.superseded.clear();
        self.finish_propose_with(inputs, serial, None)
    }

    /// Propose a *server-side* state transition (Phase 51 — the focus
    /// truth): the same derivation as [`Toplevel::propose`], but the
    /// proposal it replaces parks in the acknowledgment grace window
    /// exactly as the drag's pointer-paced proposals do — a focus
    /// transition the client never asked for must never punish a
    /// client still draining a drag's serials (the same doctrine
    /// [`Toplevel::propose_drag`] serves, now for the state seams).
    /// The `activated` bit this carries is the frozen flag riding
    /// real proposals for the first time.
    #[must_use]
    pub fn propose_state(&mut self, inputs: &PolicyInputs) -> Configure {
        let serial = self.serials.issue();
        if let Some(old) = self.pending.take() {
            self.superseded.push_back(old);
            while self.superseded.len() > GRACE_WINDOW {
                self.superseded.pop_front();
            }
        }
        self.finish_propose_with(inputs, serial, None)
    }

    /// Propose under an *externally issued* serial — the mint arm's
    /// handshake path (Phase 49): the seat's interaction clock issued
    /// this serial (it is the one serial the data family's
    /// `set_selection` gate references), so the window's first
    /// proposal must carry it, not the machine's own next. The clock
    /// adopts the serial as its watermark and reserves it, so every
    /// later `propose()` continues the same domain — a machine serial
    /// can never collide with a serial the client has already seen.
    #[must_use]
    pub fn propose_at(&mut self, inputs: &PolicyInputs, serial: Serial) -> Configure {
        self.serials.resume_from(serial);
        self.finish_propose_with(inputs, serial, None)
    }

    /// Propose with an explicit content size (Phase 50 — the operator's
    /// hand): the drag-driven proposals that are *not* pointer-paced —
    /// the demotion (a maximized window's drag start restores the
    /// floating size) and the release's final proposal (the `resizing`
    /// bit clears with it). The size clamps against the client's own
    /// min/max grammar exactly as the verbs' derived sizes do; the
    /// replaced proposal's serial dies (the strict doctrine), and the
    /// drag's grace window survives — a client still draining the drag's
    /// last serials may ack them yet.
    #[must_use]
    pub fn propose_sized(&mut self, inputs: &PolicyInputs, size: (u32, u32)) -> Configure {
        let serial = self.serials.issue();
        self.finish_propose_with(inputs, serial, Some(size))
    }

    /// The live interactive resize's proposal (Phase 50 — the operator's
    /// hand): pointer-paced, so the proposal it replaces parks in the
    /// acknowledgment grace window (see [`Toplevel::ack_configure`]) —
    /// the client that acks the configure it actually *saw*, already
    /// superseded by the pointer's next move, is never at fault. The
    /// window holds the last 8 superseded proposals; the oldest
    /// leaves it (8 pointer-paced proposals behind is genuinely
    /// stale, whatever the device's rate).
    #[must_use]
    pub fn propose_drag(&mut self, inputs: &PolicyInputs, size: (u32, u32)) -> Configure {
        let serial = self.serials.issue();
        if let Some(old) = self.pending.take() {
            self.superseded.push_back(old);
            while self.superseded.len() > GRACE_WINDOW {
                self.superseded.pop_front();
            }
        }
        self.finish_propose_with(inputs, serial, Some(size))
    }

    /// The shared proposal construction: derive, reserve, install.
    /// The serial is the caller's — `propose` issues it, `propose_at`
    /// adopts it — and it becomes the live (reserved) proposal either
    /// way, so a superseding proposal kills exactly it. An explicit
    /// size (the drag's explicit-size arms) overrides the derivation,
    /// clamped by the client's own hints.
    fn finish_propose_with(
        &mut self,
        inputs: &PolicyInputs,
        serial: Serial,
        size: Option<(u32, u32)>,
    ) -> Configure {
        let (width, height) = match size {
            Some((w, h)) => self.clamp_size(w, h),
            None => self.derive_size(inputs),
        };
        let insets = if self.wanted.fullscreen() {
            Insets::ZERO
        } else {
            inputs.metrics.insets(inputs.decoration, inputs.scale)
        };
        let c = Configure {
            serial,
            states: self.wanted,
            width,
            height,
            insets,
            workspace: inputs.workspace,
            output: inputs.output,
        };
        self.serials.reserve(serial);
        self.pending = Some(c);
        c
    }

    /// `ack_configure`: accept the live proposal — or one of the
    /// drag's superseded proposals still inside the grace window
    /// (Phase 50: the client acks the configure it actually *saw*;
    /// the pointer's next move superseding it is the operator's
    /// doing, never the client's fault — the acked size realizes with
    /// the buffer the client actually commits, the fresher proposal
    /// stays live for the next ack). A serial that is neither (stale,
    /// unknown, or already realized) is a protocol violation.
    ///
    /// # Errors
    /// [`ToplevelError::StaleAck`] on any serial outside the live
    /// proposal and the grace window.
    pub fn ack_configure(&mut self, serial: Serial) -> Result<(), ToplevelError> {
        if let Some(p) = self.pending {
            if p.serial == serial {
                self.acked = Some(p);
                self.pending = None;
                return Ok(());
            }
        }
        if let Some(index) = self.superseded.iter().position(|c| c.serial == serial) {
            // The graceful ack: the seen-but-superseded proposal
            // becomes the acked one (the client's own size answer
            // realizes at its commit); the live proposal stands.
            if let Some(c) = self.superseded.remove(index) {
                self.acked = Some(c);
            }
            return Ok(());
        }
        Err(ToplevelError::StaleAck)
    }

    /// The client's commit: realize the last acked proposal. Returns
    /// the newly applied state (or `None` when nothing was pending —
    /// a commit without an ack changes nothing).
    ///
    /// # Panics
    ///
    /// Never; a commit is always accepted.
    #[must_use]
    pub fn commit(&mut self) -> Option<Configure> {
        if let Some(a) = self.acked {
            self.acked = None;
            self.applied = Some(a);
            Some(a)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs() -> PolicyInputs {
        PolicyInputs {
            workspace_area: Rect::new(0, 0, 1920, 1040),
            output_size: (1920, 1080),
            metrics: SsdMetrics::LION,
            decoration: DecorationMode::Server,
            scale: ScaleFactor::IDENTITY,
            workspace: 0,
            output: Some(ObjectId::from_wire(0x20)),
        }
    }

    fn k() -> WindowKey {
        WindowKey::new(1)
    }

    #[test]
    fn two_phase_commit_lifecycle() {
        let mut t = Toplevel::new(k());
        assert!(t.pending().is_none() && t.acked().is_none());
        let c1 = t.propose(&inputs());
        assert_eq!(c1.serial, Serial(1));
        assert_eq!(t.pending(), Some(c1));
        // Ack of an unknown serial is stale.
        assert_eq!(t.ack_configure(Serial(9)), Err(ToplevelError::StaleAck));
        t.ack_configure(c1.serial).unwrap();
        assert!(t.pending().is_none());
        assert_eq!(t.acked(), Some(c1));
        // Commit realizes it; a second commit is a no-op.
        assert_eq!(t.commit(), Some(c1));
        assert_eq!(t.applied(), Some(c1));
        assert_eq!(t.commit(), None);
        // The realized serial is dead: acking it again is stale.
        assert_eq!(t.ack_configure(c1.serial), Err(ToplevelError::StaleAck));
    }

    #[test]
    fn new_configure_supersedes_the_live_one() {
        let mut t = Toplevel::new(k());
        let c1 = t.propose(&inputs());
        t.maximize();
        let c2 = t.propose(&inputs());
        assert!(c2.serial.after(c1.serial));
        // c1 is dead.
        assert_eq!(t.ack_configure(c1.serial), Err(ToplevelError::StaleAck));
        t.ack_configure(c2.serial).unwrap();
        assert_eq!(t.commit(), Some(c2));
    }

    #[test]
    fn the_handshake_serial_is_adopted_and_the_domain_continues() {
        // The mint path: the seat's interaction clock issued serial 7;
        // the first proposal must carry exactly it.
        let mut t = Toplevel::new(k());
        let c1 = t.propose_at(&inputs(), Serial(7));
        assert_eq!(c1.serial, Serial(7));
        assert_eq!(t.pending(), Some(c1));
        t.ack_configure(Serial(7)).unwrap();
        assert_eq!(t.commit(), Some(c1));
        // The machine's own issues continue *after* the adopted
        // watermark — never colliding with serial 7, never reusing
        // any serial the client has seen.
        t.maximize();
        let c2 = t.propose(&inputs());
        assert_eq!(c2.serial, Serial(8));
        assert!(c2.width > 0);
    }

    #[test]
    fn two_machines_seed_from_successive_seat_serials() {
        // The deployment shape: the seat's interaction clock issues
        // one serial per mint (3, then 4); each machine adopts its
        // own and continues its own domain. The domains are
        // *object-scoped* — a value one machine issues after its seed
        // may equal another object's serial (4 here: A's second
        // proposal, B's handshake), exactly the dialog host's own
        // deployed shape (every dialog's first proposal is serial
        // 1): acks match per object, so the reuse is harmless by
        // construction.
        let mut a = Toplevel::new(WindowKey::new(1));
        let mut b = Toplevel::new(WindowKey::new(2));
        let ca = a.propose_at(&inputs(), Serial(3));
        let cb = b.propose_at(&inputs(), Serial(4));
        assert_eq!(ca.serial, Serial(3));
        assert_eq!(cb.serial, Serial(4));
        // Each machine's next proposal continues after its own seed.
        a.maximize();
        b.fullscreen();
        let ca2 = a.propose(&inputs());
        let cb2 = b.propose(&inputs());
        assert_eq!(ca2.serial, Serial(4));
        assert_eq!(cb2.serial, Serial(5));
        // And within one object, the domain never reuses a serial.
        assert_ne!(ca2.serial, ca.serial);
        assert_ne!(cb2.serial, cb.serial);
    }

    #[test]
    fn maximized_geometry_minus_insets() {
        let mut t = Toplevel::new(k());
        t.maximize();
        let c = t.propose(&inputs());
        // 1920×1040 area, insets 1/29/1/1 → 1918×1010.
        assert_eq!((c.width, c.height), (1918, 1010));
        assert!(c.states.maximized());
        assert_eq!((c.insets.left, c.insets.top), (1, 29));
    }

    #[test]
    fn fullscreen_covers_the_output_with_zero_insets() {
        let mut t = Toplevel::new(k());
        t.fullscreen();
        let c = t.propose(&inputs());
        assert_eq!((c.width, c.height), (1920, 1080));
        assert_eq!(c.insets, Insets::ZERO);
        assert!(c.states.fullscreen());
    }

    #[test]
    fn fullscreen_precedes_maximized() {
        let mut t = Toplevel::new(k());
        t.maximize();
        t.fullscreen();
        let c = t.propose(&inputs());
        assert_eq!((c.width, c.height), (1920, 1080));
        assert_eq!(c.insets, Insets::ZERO);
        assert!(c.states.maximized() && c.states.fullscreen());
    }

    #[test]
    fn contradictory_hints_saturate() {
        // Min above an established max saturates at the max.
        let mut t = Toplevel::new(k());
        t.set_max_size(400, 300);
        t.set_min_size(500, 50);
        assert_eq!(t.min_size(), (400, 50));
        // Max below an established min rises to the min; zero passes.
        let mut t2 = Toplevel::new(k());
        t2.set_min_size(200, 150);
        t2.set_max_size(100, 0);
        assert_eq!(t2.max_size(), (200, 0));
        // Unconstrained pairs stay untouched.
        let mut t3 = Toplevel::new(k());
        t3.set_min_size(500, 500);
        assert_eq!(t3.min_size(), (500, 500));
    }

    #[test]
    fn size_hints_clamp_proposals() {
        let mut t = Toplevel::new(k());
        t.set_min_size(500, 400);
        t.set_max_size(1000, 900);
        t.maximize();
        let c = t.propose(&inputs());
        // 1918×1010 clamped to 1000×900.
        assert_eq!((c.width, c.height), (1000, 900));
        // A tiny workspace with a big min hint: min wins.
        let mut small = inputs();
        small.workspace_area = Rect::new(0, 0, 300, 200);
        let c2 = t.propose(&small);
        assert_eq!((c2.width, c2.height), (500, 400));
    }

    #[test]
    fn unconstrained_proposal_is_client_choice() {
        let mut t = Toplevel::new(k());
        let c = t.propose(&inputs());
        assert_eq!((c.width, c.height), (0, 0));
        // 0 is never clamped away.
        assert_eq!(t.clamp_size(0, 0), (0, 0));
    }

    #[test]
    fn minimize_clears_activation_and_vice_versa() {
        let mut t = Toplevel::new(k());
        t.set_activated(true);
        t.minimize();
        assert!(t.wanted().minimized());
        assert!(!t.wanted().activated());
        t.set_activated(true);
        assert!(!t.wanted().minimized());
        assert!(t.wanted().activated());
    }

    #[test]
    fn strings_are_bounded_and_settable() {
        let mut t = Toplevel::new(k());
        t.set_title("Image Viewer").unwrap();
        t.set_app_id("io.lionos.viewer").unwrap();
        assert_eq!(t.title(), "Image Viewer");
        assert_eq!(t.app_id(), "io.lionos.viewer");
        assert_eq!(
            t.set_title(&"x".repeat(4097)),
            Err(ToplevelError::BadString)
        );
        assert_eq!(t.set_title("a\0b"), Err(ToplevelError::BadString));
        // Failed sets left the old value.
        assert_eq!(t.title(), "Image Viewer");
    }

    #[test]
    fn states_wire_bits_match_spec() {
        let s = ToplevelStates::build(true, true, true, true, true, true);
        assert_eq!(s.0, Bitset128::from_words([0b11_1111, 0, 0, 0]));
        assert_eq!(s.0.count(), 6);
    }

    #[test]
    fn semantic_claims_carry_and_clear() {
        // The Phase 47 claim triple: set, read back, clear — the
        // same carry-and-clear contract as the material claim.
        let mut t = Toplevel::new(k());
        assert_eq!(t.semantic_role(), None);
        assert_eq!(t.security_class(), None);
        assert_eq!(t.scene_profile(), None);
        t.set_semantic_role(Some(SemanticRole::Dialog));
        t.set_security_class(Some(SecurityClass::Protected));
        t.set_scene_profile(Some(SceneProfile::Gaming));
        assert_eq!(t.semantic_role(), Some(SemanticRole::Dialog));
        assert_eq!(t.security_class(), Some(SecurityClass::Protected));
        assert_eq!(t.scene_profile(), Some(SceneProfile::Gaming));
        t.set_semantic_role(None);
        t.set_security_class(None);
        t.set_scene_profile(None);
        assert_eq!(t.semantic_role(), None);
        assert_eq!(t.security_class(), None);
        assert_eq!(t.scene_profile(), None);
    }

    #[test]
    fn semantic_wire_domains() {
        use super::{SceneProfile, SecurityClass, SemanticRole};
        // Every in-domain wire value resolves; everything else is
        // None (the honest out-of-range refusal).
        for (wire, expect) in [
            (1, SemanticRole::Window),
            (2, SemanticRole::Dialog),
            (3, SemanticRole::Tooltip),
            (4, SemanticRole::Overlay),
            (5, SemanticRole::Lock),
        ] {
            assert_eq!(SemanticRole::from_wire(wire), Some(expect));
        }
        for (wire, expect) in [
            (1, SecurityClass::Normal),
            (2, SecurityClass::Private),
            (3, SecurityClass::Protected),
            (4, SecurityClass::System),
        ] {
            assert_eq!(SecurityClass::from_wire(wire), Some(expect));
        }
        for (wire, expect) in [
            (1, SceneProfile::Desktop),
            (2, SceneProfile::Creative),
            (3, SceneProfile::Gaming),
        ] {
            assert_eq!(SceneProfile::from_wire(wire), Some(expect));
        }
        assert_eq!(SemanticRole::from_wire(6), None);
        assert_eq!(SecurityClass::from_wire(5), None);
        assert_eq!(SceneProfile::from_wire(4), None);
    }

    // ---- Phase 50 — the operator's hand --------------------------------

    #[test]
    fn resize_edge_wire_domain_and_axis_algebra() {
        use super::ResizeEdge;
        // Every in-domain wire value resolves; everything else is
        // None (the honest out-of-range refusal).
        for (wire, expect) in [
            (1, ResizeEdge::Top),
            (2, ResizeEdge::Bottom),
            (3, ResizeEdge::Left),
            (4, ResizeEdge::Right),
            (5, ResizeEdge::TopLeft),
            (6, ResizeEdge::TopRight),
            (7, ResizeEdge::BottomLeft),
            (8, ResizeEdge::BottomRight),
        ] {
            assert_eq!(ResizeEdge::from_wire(wire), Some(expect));
        }
        assert_eq!(ResizeEdge::from_wire(0), None);
        assert_eq!(ResizeEdge::from_wire(9), None);
        // The axis algebra: a plain edge engages one axis, a corner
        // two; the left/top grabs name the borders whose following
        // moves the window's origin.
        assert!(ResizeEdge::Left.horizontal() && !ResizeEdge::Left.vertical());
        assert!(ResizeEdge::Top.vertical() && !ResizeEdge::Top.horizontal());
        assert!(ResizeEdge::TopRight.horizontal() && ResizeEdge::TopRight.vertical());
        assert!(ResizeEdge::BottomLeft.horizontal() && ResizeEdge::BottomLeft.vertical());
        assert!(ResizeEdge::Left.grabs_left() && !ResizeEdge::Right.grabs_left());
        assert!(ResizeEdge::TopLeft.grabs_left() && ResizeEdge::TopLeft.grabs_top());
        assert!(!ResizeEdge::BottomRight.grabs_top());
        assert!(!ResizeEdge::Top.grabs_left() && ResizeEdge::Top.grabs_top());
    }

    #[test]
    fn explicit_size_proposals_carry_the_clamped_size() {
        // propose_sized: the demotion and the final — the explicit
        // size lands clamped by the client's own hints, states as
        // wanted (the resizing bit rides when the host set it).
        let mut t = Toplevel::new(k());
        t.set_min_size(200, 150);
        let c = t.propose_sized(&inputs(), (640, 480));
        assert_eq!((c.width, c.height), (640, 480));
        assert!(!c.states.resizing());
        // The hint clamps: a 100-wide ask saturates at the min.
        t.set_resizing(true);
        let c2 = t.propose_sized(&inputs(), (100, 480));
        assert_eq!((c2.width, c2.height), (200, 480));
        assert!(c2.states.resizing());
        // The superseded explicit proposal's serial died (the strict
        // doctrine — the Phase 49 refusal holds for it).
        assert_eq!(t.ack_configure(c.serial), Err(ToplevelError::StaleAck));
    }

    #[test]
    fn the_grace_window_holds_the_superseded_drag_serials() {
        // The pointer-paced resize: each proposal supersedes the last
        // *into the grace window* — a client acking the configure it
        // actually saw is never at fault, and the realize applies the
        // client's own size answer while the fresher proposal stays
        // live.
        let mut t = Toplevel::new(k());
        t.set_resizing(true);
        let c1 = t.propose_drag(&inputs(), (400, 300));
        let c2 = t.propose_drag(&inputs(), (500, 380));
        let c3 = t.propose_drag(&inputs(), (600, 460));
        // c1 and c2 are superseded but acknowledgeable.
        t.ack_configure(c1.serial).expect("the graceful ack");
        // The acked one realizes at commit (the client's actual
        // buffer size); the live proposal (c3) stands.
        assert_eq!(t.commit().map(|c| (c.width, c.height)), Some((400, 300)));
        assert_eq!(t.pending(), Some(c3));
        // A ack of the live one still works, and realizes c3.
        t.ack_configure(c3.serial).expect("the live ack");
        assert_eq!(t.commit().map(|c| (c.width, c.height)), Some((600, 460)));
        // c2 sits in the window still — superseded but acknowledgeable
        // (the window's own doctrine: eviction, not realization, is
        // what makes a drag serial stale).
        t.ack_configure(c2.serial).expect("still inside the window");
    }

    #[test]
    fn the_grace_window_is_bounded_and_verb_cleared() {
        // Ten proposals: the first superseded right out of the window
        // (the depth is 8 — nine superseded, the tenth live).
        let mut t = Toplevel::new(k());
        t.set_resizing(true);
        let first = t.propose_drag(&inputs(), (100, 100));
        for i in 1..=9 {
            let _ = t.propose_drag(&inputs(), (100 + i * 10, 100 + i * 10));
        }
        assert_eq!(
            t.ack_configure(first.serial),
            Err(ToplevelError::StaleAck),
            "9-back is genuinely stale"
        );
        // A verb proposal clears the window — the operator's verbs
        // are authoritative (the Phase 49 strict-ack doctrine,
        // preserved whole).
        let last = t.propose_drag(&inputs(), (900, 900));
        t.maximize();
        let verb = t.propose(&inputs());
        assert_eq!(
            t.ack_configure(last.serial),
            Err(ToplevelError::StaleAck),
            "the verb cleared the window"
        );
        // The verb's own proposal acks normally.
        t.ack_configure(verb.serial).expect("the verb's ack");
        assert!(verb.states.maximized());
    }

    #[test]
    fn verb_superseded_serials_stay_strict() {
        // The Phase 49 doctrine, whole: a verb's superseded serial is
        // dead (the grace window only ever holds drag proposals).
        let mut t = Toplevel::new(k());
        let floating = t.propose(&inputs());
        t.maximize();
        let _maximized = t.propose(&inputs());
        assert_eq!(
            t.ack_configure(floating.serial),
            Err(ToplevelError::StaleAck)
        );
    }

    #[test]
    fn state_proposals_park_the_replaced_one_like_the_drag() {
        // Phase 51 — the focus truth: a server-side state transition
        // (the `activated` bit) never punishes a client still
        // draining a drag's serials. The live drag proposal the
        // focus transition replaces parks in the grace window,
        // acknowledgeable exactly as the drag's own.
        let mut t = Toplevel::new(k());
        t.set_resizing(true);
        let drag = t.propose_drag(&inputs(), (500, 380));
        t.set_activated(true);
        let focus = t.propose_state(&inputs());
        assert!(focus.states.activated(), "the bit rides the proposal");
        assert!(focus.states.resizing(), "the drag's bit rides along");
        // The drag's serial the focus proposal replaced is still
        // acknowledgeable — the grace window holds it.
        t.ack_configure(drag.serial).expect("the parked drag ack");
        assert_eq!(t.commit().map(|c| (c.width, c.height)), Some((500, 380)));
        assert_eq!(t.pending(), Some(focus));
        // The states interplay: activation clears minimized, minimize
        // clears activation (the machine's own law, unchanged).
        t.minimize();
        let hidden = t.propose_state(&inputs());
        assert!(hidden.states.minimized());
        assert!(!hidden.states.activated());
    }
}
