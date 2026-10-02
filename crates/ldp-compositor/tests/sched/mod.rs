//! Shared harness for the Phase 7 scheduler test suites.
//!
//! (Different test binaries consume different subsets of this module,
//! so unused-item warnings are expected and silenced.)
//!
//! A deterministic discrete-event simulator:
//!
//! * [`Rng`] — splitmix64, seeded (the workspace has no `rand` policy
//!   exception to make here; the client crate already blazed the
//!   seeded-LCG trail),
//! * [`Panel`] — a simulated output generating vblank timestamps from a
//!   *true* period (which may drift from the nominal mode) plus bounded
//!   jitter, strictly increasing,
//! * [`SimClient`] — a reactive client: registers a frame, renders for
//!   `render_ns`, commits, re-registers on the terminal event,
//! * [`Driver`] — merges the flip stream and client reactions into one
//!   ordered input queue (BTreeMap, timestamp + sequence), feeds the
//!   real [`FrameScheduler`], and collects every input and emission.
//!
//! The recorded input stream is exactly what [`record`](ldp_compositor::replay::record)
//! consumes, so every property scenario doubles as a replay case.

#![allow(dead_code)]

use std::collections::BTreeMap;

use ldp_compositor::replay::SchedInput;
use ldp_compositor::scheduler::{FrameScheduler, SchedEvent, SchedulerConfig};
use ldp_compositor::SurfaceId;
use ldp_core::time::{
    FrameDropReason, Mono, PresentationMode, PresentationTiming, RefreshInterval,
};

/// Seeded splitmix64 generator (deterministic across runs/processes).
#[derive(Clone)]
pub struct Rng(u64);

impl Rng {
    /// Seed it.
    #[must_use]
    pub const fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// Next raw value.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform-ish value below `n` (0 when `n` is 0).
    pub fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next_u64() % n
        }
    }

    /// Symmetric jitter in `[-amp, +amp]`.
    pub fn jitter(&mut self, amp: u64) -> i64 {
        i64::try_from(self.below(2 * amp + 1)).map_or(0, |v| v - amp as i64)
    }
}

/// A simulated output timeline.
pub struct Panel {
    true_period_ns: u64,
    nominal_vblank_ns: u64,
    last_emitted_ns: u64,
    jitter_ns: u64,
    rng: Rng,
}

impl Panel {
    /// A panel running at `true_period_ns` with flip-timestamp jitter of
    /// ±`jitter_ns`, seeded.
    #[must_use]
    pub fn new(true_period_ns: u64, jitter_ns: u64, seed: u64) -> Self {
        Self {
            true_period_ns,
            nominal_vblank_ns: 0,
            last_emitted_ns: 0,
            jitter_ns,
            rng: Rng::new(seed),
        }
    }

    /// Advance the internal vblank clock by `ns` without emitting
    /// flips (an output stall: DPMS blip, modeset, starvation).
    pub fn stall(&mut self, ns: u64) {
        self.nominal_vblank_ns += ns;
    }

    /// Generate the next `count` flip timestamps (strictly increasing).
    pub fn flips(&mut self, count: usize) -> Vec<u64> {
        let mut out = Vec::with_capacity(count);
        for _ in 0..count {
            self.nominal_vblank_ns += self.true_period_ns;
            let ts = (self.nominal_vblank_ns as i128 + i128::from(self.rng.jitter(self.jitter_ns)))
                .max(i128::from(self.last_emitted_ns + 1));
            self.last_emitted_ns = u64::try_from(ts).unwrap_or(u64::MAX);
            out.push(self.last_emitted_ns);
        }
        out
    }
}

/// A reactive simulated client.
#[derive(Clone)]
pub struct SimClient {
    /// Surface it draws on.
    pub surface: SurfaceId,
    /// Presentation mode it requests.
    pub mode: PresentationMode,
    /// Render time after receiving a frame target.
    pub render_ns: u64,
    /// Wait after a terminal event before re-registering.
    pub requeue_delay_ns: u64,
    /// Commit-time jitter (±).
    pub commit_jitter_ns: u64,
    rng: Rng,
    next_frame: u64,
}

impl SimClient {
    /// A vsync client with sane defaults.
    #[must_use]
    pub fn new(surface: SurfaceId, render_ns: u64, seed: u64) -> Self {
        Self {
            surface,
            mode: PresentationMode::Vsync,
            render_ns,
            requeue_delay_ns: 1_000,
            commit_jitter_ns: 0,
            rng: Rng::new(seed),
            next_frame: 1,
        }
    }

    /// Override the presentation mode.
    #[must_use]
    pub fn with_mode(mut self, mode: PresentationMode) -> Self {
        self.mode = mode;
        self
    }

    /// Override the commit jitter.
    #[must_use]
    pub fn with_commit_jitter(mut self, amp: u64) -> Self {
        self.commit_jitter_ns = amp;
        self
    }

    fn next_frame_id(&mut self) -> u64 {
        let id = self.next_frame;
        self.next_frame += 1;
        id
    }
}

/// Terminal-event statistics over one driver run.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Stats {
    /// `presented` events seen.
    pub presented: usize,
    /// Drops with `DeadlineMissed`.
    pub deadline_missed: usize,
    /// Drops with `Superseded`.
    pub superseded: usize,
    /// Drops with `SurfaceHidden`.
    pub hidden: usize,
    /// Drops with `OutputOff`.
    pub output_off: usize,
}

impl Stats {
    /// Deadline hit rate: presented out of all deadline outcomes.
    #[must_use]
    pub fn hit_rate(&self) -> f64 {
        let total = self.presented + self.deadline_missed;
        if total == 0 {
            1.0
        } else {
            self.presented as f64 / total as f64
        }
    }
}

/// The discrete-event driver.
pub struct Driver {
    scheduler: FrameScheduler,
    /// Every pending input — pre-generated flips (low sequence
    /// numbers) and client reactions — merged by (timestamp, sequence).
    queue: BTreeMap<(u64, u64), SchedInput>,
    next_seq: u64,
    clients: BTreeMap<SurfaceId, SimClient>,
    /// Every input fed, in order (the recording stream).
    pub inputs: Vec<SchedInput>,
    /// Every emission, in order.
    pub events: Vec<SchedEvent>,
    /// Terminal-event statistics.
    pub stats: Stats,
}

impl Driver {
    /// Build a driver over a pre-generated flip stream.
    #[must_use]
    pub fn new(nominal: RefreshInterval, config: SchedulerConfig, flips: &[u64]) -> Self {
        let mut queue = BTreeMap::new();
        for (index, ts) in flips.iter().enumerate() {
            queue.insert(
                (*ts, index as u64),
                SchedInput::Flip {
                    ts: Mono::from_ns(*ts),
                },
            );
        }
        Self {
            scheduler: FrameScheduler::new(nominal, config).expect("valid config"),
            queue,
            next_seq: flips.len() as u64,
            clients: BTreeMap::new(),
            inputs: Vec::new(),
            events: Vec::new(),
            stats: Stats::default(),
        }
    }

    /// Register a client; its first frame request lands just after the
    /// first flip (a live timeline).
    pub fn add_client(&mut self, mut client: SimClient, first_request_ts: u64) {
        let frame = client.next_frame_id();
        let surface = client.surface;
        self.queue.insert(
            (first_request_ts, self.next_seq),
            SchedInput::SetMode {
                surface,
                mode: client.mode,
                ts: Mono::from_ns(first_request_ts),
            },
        );
        self.next_seq += 1;
        self.queue.insert(
            (first_request_ts, self.next_seq),
            SchedInput::FrameRequest {
                surface,
                frame,
                ts: Mono::from_ns(first_request_ts),
            },
        );
        self.next_seq += 1;
        self.clients.insert(surface, client);
    }

    /// Run to exhaustion (the input queue drains completely).
    pub fn run(&mut self) {
        while let Some(input) = self.next_input() {
            self.feed(input);
        }
    }

    fn next_input(&mut self) -> Option<SchedInput> {
        let key = *self.queue.keys().next()?;
        self.queue.remove(&key)
    }

    fn feed(&mut self, input: SchedInput) {
        let ts = Mono::from_ns(input_ts(&input));
        self.inputs.push(input);
        match input {
            SchedInput::Flip { ts } => self.scheduler.observe_flip(ts),
            SchedInput::FrameRequest { surface, frame, ts } => {
                self.scheduler.frame_request(surface, frame, ts);
            }
            SchedInput::Commit { surface, ts } => self.scheduler.commit(surface, ts),
            SchedInput::SetVisibility {
                surface,
                hidden,
                ts,
            } => {
                self.scheduler.set_visibility(surface, hidden, ts);
            }
            SchedInput::SetMode { surface, mode, ts } => self.scheduler.set_mode(surface, mode, ts),
            SchedInput::SetProfile {
                surface,
                profile,
                ts,
            } => {
                self.scheduler.set_profile(surface, profile, ts);
            }
            SchedInput::Park { ts } => self.scheduler.park(ts),
            SchedInput::Resume { ts } => self.scheduler.resume(ts),
        }
        let events = self.scheduler.drain();
        for event in events {
            self.react(event, ts.as_ns());
        }
    }

    fn react(&mut self, event: SchedEvent, now: u64) {
        self.events.push(event);
        match event {
            SchedEvent::FrameTarget { surface, .. } => {
                let (render_ns, jitter) = {
                    let Some(client) = self.clients.get_mut(&surface) else {
                        return;
                    };
                    (client.render_ns, client.rng.jitter(client.commit_jitter_ns))
                };
                let at =
                    (now as i128 + i128::from(render_ns) + i128::from(jitter)).max(i128::from(now));
                let at = u64::try_from(at).unwrap_or(u64::MAX);
                self.enqueue(
                    SchedInput::Commit {
                        surface,
                        ts: Mono::from_ns(at),
                    },
                    at,
                );
            }
            SchedEvent::Presented { surface, timing } => {
                self.stats.presented += 1;
                self.requeue(surface, now);
                let _ = timing;
            }
            SchedEvent::FrameDropped {
                surface,
                frame,
                reason,
            } => {
                match reason {
                    FrameDropReason::DeadlineMissed => self.stats.deadline_missed += 1,
                    FrameDropReason::Superseded => self.stats.superseded += 1,
                    FrameDropReason::SurfaceHidden => self.stats.hidden += 1,
                    FrameDropReason::OutputOff => self.stats.output_off += 1,
                    // Reserved for the Phase 15 VRR policy engine; the
                    // v1 scheduler never emits it.
                    _ => {}
                }
                self.requeue(surface, now);
                let _ = frame;
            }
        }
    }

    fn requeue(&mut self, surface: SurfaceId, now: u64) {
        let Some(client) = self.clients.get_mut(&surface) else {
            return;
        };
        let frame = client.next_frame_id();
        let at = now.saturating_add(client.requeue_delay_ns);
        self.enqueue(
            SchedInput::FrameRequest {
                surface,
                frame,
                ts: Mono::from_ns(at),
            },
            at,
        );
    }

    fn enqueue(&mut self, input: SchedInput, at: u64) {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.queue.insert((at, seq), input);
    }
}

fn input_ts(input: &SchedInput) -> u64 {
    match *input {
        SchedInput::Flip { ts }
        | SchedInput::FrameRequest { ts, .. }
        | SchedInput::Commit { ts, .. }
        | SchedInput::SetVisibility { ts, .. }
        | SchedInput::SetMode { ts, .. }
        | SchedInput::SetProfile { ts, .. }
        | SchedInput::Park { ts }
        | SchedInput::Resume { ts } => ts.as_ns(),
    }
}

// ---------------------------------------------------------------------------
// Golden-suite event constructors (exact expected values).
// ---------------------------------------------------------------------------

/// Build an expected `frame_target`.
#[must_use]
pub fn ft(
    surface: u64,
    frame: u64,
    deadline_ns: u64,
    target_ns: u64,
    refresh_ns: u64,
    budget_ns: u64,
    mode: PresentationMode,
) -> SchedEvent {
    SchedEvent::FrameTarget {
        surface: SurfaceId::from_raw(surface),
        frame,
        deadline: ldp_core::time::FrameDeadline {
            deadline: Mono::from_ns(deadline_ns),
            target_vblank: Mono::from_ns(target_ns),
            refresh: RefreshInterval::from_ns(refresh_ns).expect("positive refresh"),
            budget_ns,
            mode,
        },
    }
}

/// Build an expected `presented`.
#[must_use]
pub fn pr(surface: u64, frame: u64, ts_ns: u64, refresh_ns: u64, torn: bool) -> SchedEvent {
    SchedEvent::Presented {
        surface: SurfaceId::from_raw(surface),
        timing: PresentationTiming {
            frame,
            presented_at: Mono::from_ns(ts_ns),
            refresh: RefreshInterval::from_ns(refresh_ns).expect("positive refresh"),
            flags: if torn {
                ldp_core::time::PresentationFlags {
                    torn: true,
                    ..ldp_core::time::PresentationFlags::default()
                }
            } else {
                ldp_core::time::PresentationFlags {
                    vblank: true,
                    ..ldp_core::time::PresentationFlags::default()
                }
            },
        },
    }
}

/// Build an expected `frame_dropped`.
#[must_use]
pub fn fd(surface: u64, frame: u64, reason: FrameDropReason) -> SchedEvent {
    SchedEvent::FrameDropped {
        surface: SurfaceId::from_raw(surface),
        frame,
        reason,
    }
}

/// 60 Hz in nanoseconds.
pub const SIXTY: u64 = 16_666_666;

/// Default policy costs in ns (mirrors `SchedulerConfig::default`).
pub const COSTS: u64 = 1_000_000;

// ---------------------------------------------------------------------------
// Property/replay scenarios: each runs a full simulated timeline and
// captures the recording stream plus its outputs.
// ---------------------------------------------------------------------------

/// One recorded scenario run.
pub struct Scenario {
    /// Policy in force.
    pub config: SchedulerConfig,
    /// Nominal refresh (ns).
    pub nominal_ns: u64,
    /// The input stream (a recording body).
    pub inputs: Vec<SchedInput>,
    /// The emission stream.
    pub outputs: Vec<SchedEvent>,
    /// Terminal statistics.
    pub stats: Stats,
}

fn run_scenario(
    config: SchedulerConfig,
    nominal_ns: u64,
    flips: &[u64],
    clients: [(SurfaceId, SimClient); 1],
) -> Scenario {
    let first = flips.first().copied().unwrap_or(0) + 1;
    let mut driver = Driver::new(
        RefreshInterval::from_ns(nominal_ns).expect("positive nominal"),
        config,
        flips,
    );
    for (surface, client) in clients {
        debug_assert_eq!(surface, client.surface);
        driver.add_client(client, first);
    }
    driver.run();
    Scenario {
        config,
        nominal_ns,
        inputs: driver.inputs,
        outputs: driver.events,
        stats: driver.stats,
    }
}

/// Multi-client variant (shared timelines must not starve surfaces).
fn run_scenario_multi(
    config: SchedulerConfig,
    nominal_ns: u64,
    flips: &[u64],
    clients: Vec<SimClient>,
) -> Scenario {
    let first = flips.first().copied().unwrap_or(0) + 1;
    let mut driver = Driver::new(
        RefreshInterval::from_ns(nominal_ns).expect("positive nominal"),
        config,
        flips,
    );
    for client in clients {
        driver.add_client(client, first);
    }
    driver.run();
    Scenario {
        config,
        nominal_ns,
        inputs: driver.inputs,
        outputs: driver.events,
        stats: driver.stats,
    }
}

/// A comfortable client (97% of the render budget) on a jittered 60 Hz
/// panel: the deadline contract should almost always hold.
pub fn scenario_nominal_jitter(seed: u64) -> Scenario {
    let mut panel = Panel::new(SIXTY, 250_000, seed);
    let flips = panel.flips(500);
    let clients = [(
        SurfaceId::from_raw(1),
        SimClient::new(SurfaceId::from_raw(1), 15_200_000, seed ^ 0xA5A5)
            .with_commit_jitter(100_000),
    )];
    run_scenario(SchedulerConfig::default(), SIXTY, &flips, clients)
}

/// A panel truly running at ~56.8 Hz against a 60 Hz nominal mode: the
/// PLL must lock so the (drift-shifted) deadlines stop missing.
pub fn scenario_drift_lock(seed: u64) -> Scenario {
    let true_period = 17_600_000;
    let mut panel = Panel::new(true_period, 100_000, seed);
    let flips = panel.flips(400);
    let clients = [(
        SurfaceId::from_raw(1),
        SimClient::new(SurfaceId::from_raw(1), 16_200_000, seed ^ 0x1BAD)
            .with_commit_jitter(50_000),
    )];
    run_scenario(SchedulerConfig::default(), SIXTY, &flips, clients)
}

/// A systematically slow client (render 20 ms vs a 15.67 ms budget):
/// the escalation ladder must hand it deadlines it can meet.
pub fn scenario_slow_client(seed: u64) -> Scenario {
    let mut panel = Panel::new(SIXTY, 50_000, seed);
    let flips = panel.flips(400);
    let clients = [(
        SurfaceId::from_raw(1),
        SimClient::new(SurfaceId::from_raw(1), 20_000_000, seed ^ 0x5EED),
    )];
    run_scenario(SchedulerConfig::default(), SIXTY, &flips, clients)
}

/// A 200 ms output stall mid-run: the timeline re-anchors and the
/// client recovers without a miss storm.
pub fn scenario_stall_resync(seed: u64) -> Scenario {
    let mut panel = Panel::new(SIXTY, 100_000, seed);
    let mut flips = panel.flips(100);
    panel.stall(200_000_000);
    flips.extend(panel.flips(100));
    let clients = [(
        SurfaceId::from_raw(1),
        SimClient::new(SurfaceId::from_raw(1), 8_000_000, seed ^ 0xD00D),
    )];
    run_scenario(SchedulerConfig::default(), SIXTY, &flips, clients)
}

/// An immediate-mode client with wildly varying render times: every
/// frame presents, torn.
pub fn scenario_immediate(seed: u64) -> Scenario {
    let mut panel = Panel::new(SIXTY, 100_000, seed);
    let flips = panel.flips(200);
    let clients = [(
        SurfaceId::from_raw(1),
        SimClient::new(SurfaceId::from_raw(1), 10_000_000, seed ^ 0xFEED)
            .with_mode(PresentationMode::Immediate)
            .with_commit_jitter(8_000_000),
    )];
    run_scenario(SchedulerConfig::default(), SIXTY, &flips, clients)
}

/// An adaptive-mode client rendering past the plain deadline but inside
/// the widened window: no misses, one-vblank latency.
pub fn scenario_adaptive(seed: u64) -> Scenario {
    let config = SchedulerConfig {
        vrr_window_ns: 2_000_000,
        ..SchedulerConfig::default()
    };
    let mut panel = Panel::new(SIXTY, 50_000, seed);
    let flips = panel.flips(300);
    let clients = [(
        SurfaceId::from_raw(1),
        SimClient::new(SurfaceId::from_raw(1), 16_500_000, seed ^ 0xADBF)
            .with_mode(PresentationMode::Adaptive),
    )];
    run_scenario(config, SIXTY, &flips, clients)
}

/// The adaptive scenario's vsync control: same seed and render time,
/// no widened window — misses are unavoidable.
pub fn scenario_adaptive_vsync_control(seed: u64) -> Scenario {
    let mut panel = Panel::new(SIXTY, 50_000, seed);
    let flips = panel.flips(300);
    let clients = [(
        SurfaceId::from_raw(1),
        SimClient::new(SurfaceId::from_raw(1), 16_500_000, seed ^ 0xADBF),
    )];
    run_scenario(SchedulerConfig::default(), SIXTY, &flips, clients)
}

/// Three clients sharing one output: no starvation between surfaces.
pub fn scenario_multi_client(seed: u64) -> Scenario {
    let mut panel = Panel::new(SIXTY, 200_000, seed);
    let flips = panel.flips(300);
    let clients = vec![
        SimClient::new(SurfaceId::from_raw(1), 12_000_000, seed ^ 0x1111),
        SimClient::new(SurfaceId::from_raw(2), 12_000_000, seed ^ 0x2222),
        SimClient::new(SurfaceId::from_raw(3), 12_000_000, seed ^ 0x3333),
    ];
    run_scenario_multi(SchedulerConfig::default(), SIXTY, &flips, clients)
}
