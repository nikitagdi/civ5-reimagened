//! Counters of one game and of its seats. The writers are the game actor and the seat
//! pipelines; the admin API only reads. Names follow the spec because `net-test` reads them.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use protocol::{GameCtlReason, KeyframeReason, PlayerId, RejectCode, StepKind};

/// The time of one AI phase, added up from the steps of the worker that make it: the steps in the middle of the phase
/// are `AiPhase`, and the one that gives the turn back to the humans is a `TurnChange` (which is also how a phase that
/// took a single pump ends). The phase is one sample of `hist.ai_phase_ms`, taken when it ends.
#[derive(Debug, Default)]
pub struct AiPhaseAcc(u64);

impl AiPhaseAcc {
    /// A step with `ai_us` microseconds of AI work; `Some(total)` when the step closed an AI phase.
    pub fn step(&mut self, kind: StepKind, ai_us: u64) -> Option<u64> {
        self.0 += ai_us;
        if kind != StepKind::AiPhase && self.0 > 0 {
            return Some(std::mem::take(&mut self.0));
        }
        None
    }
}

/// Adds one.
pub fn bump(c: &AtomicU64) {
    c.fetch_add(1, Ordering::Relaxed);
}

/// Reads a counter.
pub fn get(c: &AtomicU64) -> u64 {
    c.load(Ordering::Relaxed)
}

/// Raises a maximum.
pub fn max(c: &AtomicU64, v: usize) {
    c.fetch_max(v as u64, Ordering::Relaxed);
}

/// Index of a `NeedKeyframe` reason in [`SeatStats::need_keyframe`].
pub fn reason_index(r: KeyframeReason) -> usize {
    match r {
        KeyframeReason::HashMismatch => 0,
        KeyframeReason::BaseMismatch => 1,
        KeyframeReason::ApplyError => 2,
        KeyframeReason::Decode => 3,
        KeyframeReason::BasisMismatch => 4,
    }
}

/// Names of the reasons, in index order.
pub const REASON_NAMES: [&str; 5] = [
    "HashMismatch",
    "BaseMismatch",
    "ApplyError",
    "Decode",
    "BasisMismatch",
];

/// Index of a reject code in [`GameStats::rejected`].
pub fn reject_index(c: RejectCode) -> usize {
    match c {
        RejectCode::Core => 0,
        RejectCode::TurnChanged => 1,
        RejectCode::TurnEnded => 2,
        RejectCode::NotYourTurn => 3,
        RejectCode::NoGame => 4,
        RejectCode::Restarting => 5,
        RejectCode::TooMany => 6,
        RejectCode::Invalid => 7,
        RejectCode::Expired => 8,
        RejectCode::NotSeated => 9,
    }
}

/// Names of the reject codes, in index order.
pub const REJECT_NAMES: [&str; 10] = [
    "Core",
    "TurnChanged",
    "TurnEnded",
    "NotYourTurn",
    "NoGame",
    "Restarting",
    "TooMany",
    "Invalid",
    "Expired",
    "NotSeated",
];

/// Index of a `GameCtl` refusal in [`GameStats::gamectl_rejected`].
pub fn ctl_reason_index(r: GameCtlReason) -> usize {
    match r {
        GameCtlReason::NotHost => 0,
        GameCtlReason::NoSuchPlayer => 1,
        GameCtlReason::NotHuman => 2,
        GameCtlReason::SelfKick => 3,
        GameCtlReason::LastHuman => 4,
        GameCtlReason::NoGame => 5,
    }
}

/// Names of the `GameCtl` refusals, in index order.
pub const CTL_REASON_NAMES: [&str; 6] = [
    "NotHost",
    "NoSuchPlayer",
    "NotHuman",
    "SelfKick",
    "LastHuman",
    "NoGame",
];

/// Buckets of a [`Hist`]: four per power of two of a microsecond, up to about a minute.
const BUCKETS: usize = 96;

/// A histogram on logarithmic buckets, written from any task without a lock. Values
/// are microseconds; [`Hist::json`] shows milliseconds.
#[derive(Debug)]
pub struct Hist {
    n: AtomicU64,
    sum: AtomicU64,
    min: AtomicU64,
    max: AtomicU64,
    buckets: [AtomicU64; BUCKETS],
}

impl Default for Hist {
    fn default() -> Self {
        Self {
            n: AtomicU64::new(0),
            sum: AtomicU64::new(0),
            min: AtomicU64::new(u64::MAX),
            max: AtomicU64::new(0),
            buckets: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}

fn bucket_of(us: u64) -> usize {
    // `log2(us + 1)` in quarters.
    (((us + 1) as f64).log2() * 4.0) as usize
}

fn bucket_upper(i: usize) -> u64 {
    (2f64.powf((i + 1) as f64 / 4.0) - 1.0) as u64
}

impl Hist {
    pub fn record_us(&self, us: u64) {
        self.n.fetch_add(1, Ordering::Relaxed);
        self.sum.fetch_add(us, Ordering::Relaxed);
        self.min.fetch_min(us, Ordering::Relaxed);
        self.max.fetch_max(us, Ordering::Relaxed);
        self.buckets[bucket_of(us).min(BUCKETS - 1)].fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_ms(&self, ms: u64) {
        self.record_us(ms.saturating_mul(1000));
    }

    pub fn record(&self, d: std::time::Duration) {
        self.record_us(u64::try_from(d.as_micros()).unwrap_or(u64::MAX));
    }

    pub fn count(&self) -> u64 {
        self.n.load(Ordering::Relaxed)
    }

    /// Adds this histogram into a sum of several (the seats of a game).
    pub fn add_to(&self, acc: &mut HistSum) {
        acc.n += self.n.load(Ordering::Relaxed);
        acc.sum += self.sum.load(Ordering::Relaxed);
        acc.min = acc.min.min(self.min.load(Ordering::Relaxed));
        acc.max = acc.max.max(self.max.load(Ordering::Relaxed));
        for (a, b) in acc.buckets.iter_mut().zip(&self.buckets) {
            *a += b.load(Ordering::Relaxed);
        }
    }

    pub fn json(&self) -> serde_json::Value {
        let mut acc = HistSum::default();
        self.add_to(&mut acc);
        acc.json()
    }
}

/// Several histograms added up.
#[derive(Debug)]
pub struct HistSum {
    n: u64,
    sum: u64,
    min: u64,
    max: u64,
    buckets: [u64; BUCKETS],
}

impl Default for HistSum {
    fn default() -> Self {
        Self {
            n: 0,
            sum: 0,
            min: u64::MAX,
            max: 0,
            buckets: [0; BUCKETS],
        }
    }
}

impl HistSum {
    /// The value below which `p` of the samples lie, microseconds (the upper edge of its bucket, at most
    /// the maximum seen).
    fn quantile(&self, p: f64) -> u64 {
        if self.n == 0 {
            return 0;
        }
        let want = ((self.n as f64) * p).ceil().max(1.0) as u64;
        let mut seen = 0;
        for (i, c) in self.buckets.iter().enumerate() {
            seen += c;
            if seen >= want {
                return bucket_upper(i).clamp(self.min, self.max);
            }
        }
        self.max
    }

    /// `{n, min, p50, p95, p99, max, mean}` in milliseconds.
    pub fn json(&self) -> serde_json::Value {
        let ms = |us: u64| (us as f64).round() / 1000.0;
        if self.n == 0 {
            return serde_json::json!({"n": 0, "min": 0, "p50": 0, "p95": 0, "p99": 0, "max": 0, "mean": 0});
        }
        serde_json::json!({
            "n": self.n,
            "min": ms(self.min),
            "p50": ms(self.quantile(0.50)),
            "p95": ms(self.quantile(0.95)),
            "p99": ms(self.quantile(0.99)),
            "max": ms(self.max),
            "mean": ms(self.sum / self.n),
        })
    }
}

/// What one seat received in one turn (`tx_by_turn`, the last 400 turns).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurnTx {
    pub turn: i32,
    pub batches: u64,
    pub body_bytes: u64,
    pub keyframes: u64,
}

/// How many turns `tx_by_turn` keeps.
pub const TX_TURNS: usize = 400;

/// The newest save of a game as the metrics show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveLast {
    pub seq: u64,
    pub turn: i32,
    pub kind: protocol::SaveKind,
    pub bytes: u64,
    pub applied_upto: Vec<(PlayerId, u32)>,
}

/// The state of a seat's stream at the last step: what the admin API shows under `seats.<p>`.
#[derive(Debug, Clone, Copy, Default)]
pub struct StreamSnapshot {
    pub stream: u64,
    pub seq: u64,
    pub view_seq: u64,
    pub view_hash: u64,
    pub basis_hash: u64,
    pub rev: u32,
    pub ring_batches: usize,
    pub ring_bytes: usize,
}

/// Counters of one seat pipeline.
#[derive(Debug, Default)]
pub struct SeatStats {
    /// Keyframes and diffs sent to the stream.
    pub keyframes: AtomicU64,
    pub diffs: AtomicU64,
    /// `MapBasis` messages sent, and skipped because the client held the same basis (NP-D22).
    pub basis_sent: AtomicU64,
    pub basis_skipped: AtomicU64,
    /// `WantFull` asked of the worker.
    pub wantfull: AtomicU64,
    /// Worker updates that did not check out (base, apply, hash of the view or of the basis).
    pub hash_fail_worker: AtomicU64,
    /// Deltas dropped while waiting for a `Full`.
    pub deltas_dropped: AtomicU64,
    /// Contexts dropped because their `rev` is not the current one.
    pub contexts_dropped: AtomicU64,
    /// `NeedKeyframe` by reason (`REASON_NAMES`).
    pub need_keyframe: [AtomicU64; 5],
    /// Batches, messages and bytes sent to the stream.
    pub tx_batches: AtomicU64,
    pub tx_msgs: AtomicU64,
    pub tx_body_bytes: AtomicU64,
    pub tx_raw_bytes: AtomicU64,
    /// Largest compressed keyframe without the basis (the AC-N3.5 threshold), largest compressed
    /// `MapBasis`, largest compressed batch holding both (cold entry).
    pub keyframe_bytes_max: AtomicU64,
    pub basis_bytes_max: AtomicU64,
    pub cold_batch_bytes_max: AtomicU64,
    pub resume_replay: AtomicU64,
    pub resume_fresh: AtomicU64,
    /// What the client sent: messages and bytes of the game stream.
    pub rx_msgs: AtomicU64,
    pub rx_bytes: AtomicU64,
    /// Time to build one batch of this seat (its share of `batch_build_ms`).
    pub batch_build: Hist,
    /// Round trips of the seat: the application `Ping`/`Pong` (the protocol gives the gateway no way to
    /// measure it, the histogram stays empty) and the WS ping.
    pub rtt: Hist,
    pub ws_rtt: Hist,
    tx_turns: Mutex<VecDeque<TurnTx>>,
    /// Batches pushed out of the replay ring.
    pub ring_overflow_total: AtomicU64,
    state: Mutex<StreamSnapshot>,
}

impl SeatStats {
    /// A batch of `bytes` went to the stream in `turn`.
    pub fn note_tx(&self, turn: i32, bytes: usize, keyframe: bool) {
        let mut t = self.tx_turns.lock().unwrap_or_else(PoisonError::into_inner);
        if t.back().is_none_or(|b| b.turn != turn) {
            t.push_back(TurnTx {
                turn,
                batches: 0,
                body_bytes: 0,
                keyframes: 0,
            });
            while t.len() > TX_TURNS {
                t.pop_front();
            }
        }
        if let Some(b) = t.back_mut() {
            b.batches += 1;
            b.body_bytes += bytes as u64;
            b.keyframes += u64::from(keyframe);
        }
    }

    pub fn tx_by_turn(&self) -> Vec<TurnTx> {
        self.tx_turns
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .copied()
            .collect()
    }

    pub fn set_snapshot(&self, s: StreamSnapshot) {
        *self.state.lock().unwrap_or_else(PoisonError::into_inner) = s;
    }

    pub fn snapshot(&self) -> StreamSnapshot {
        *self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Counters of one game (`games.<id>.cmds`, `.views`, `.saves`, ...).
#[derive(Debug, Default)]
pub struct GameStats {
    pub received: AtomicU64,
    pub forwarded: AtomicU64,
    pub dup_answered: AtomicU64,
    pub dup_pending_dropped: AtomicU64,
    pub expired: AtomicU64,
    pub id_gap: AtomicU64,
    pub applied: AtomicU64,
    pub queued: AtomicU64,
    pub lost: AtomicU64,
    /// By `RejectCode` (`REJECT_NAMES`).
    pub rejected: [AtomicU64; 10],
    pub saves_ok: AtomicU64,
    pub saves_failed: AtomicU64,
    /// Sum of `StepStats.fog_violations`.
    pub fog_violations: AtomicU64,
    /// Worker processes started and failures seen by the actor.
    pub worker_starts: AtomicU64,
    pub worker_crashes: AtomicU64,
    pub queries: AtomicU64,
    pub queries_dropped: AtomicU64,
    /// Turn timers: groups started, `ForceEndTurn` rounds (first request per group), wars that moved
    /// the timers, and how long the host's pause lasted, milliseconds.
    pub timer_started: AtomicU64,
    pub timer_expired: AtomicU64,
    pub timer_extended: AtomicU64,
    pub timer_paused_ms: AtomicU64,
    /// `GameCtl` refusals by reason (`CTL_REASON_NAMES`).
    pub gamectl_rejected: [AtomicU64; 6],
    /// Worker kills for silence, and the pid of the worker now (0: none).
    pub hang_kills: AtomicU64,
    pub worker_pid: AtomicU32,
    /// From the start of the game to the first ready worker, ms.
    pub boot_ms: AtomicU64,
    restart_ms: Mutex<Vec<u64>>,
    saves_last: Mutex<Option<SaveLast>>,
    /// Orders: from the gateway taking an order to its answer reaching the pipeline of the seat.
    pub cmd_gateway: Hist,
    /// Worker steps (everything one step took), AI phases, safe-point waits of seat changes, time to
    /// restore a game after a failure.
    pub step: Hist,
    pub ai_phase: Hist,
    pub safe_point_wait: Hist,
    pub restore: Hist,
    /// Chat lines sent, and lines dropped by the rate limit.
    pub chat_lines: AtomicU64,
    pub chat_dropped_rate: AtomicU64,
    /// Host changes (GS-D21) and how long the host had been in `Conn::Lost` at the last one, ms.
    pub host_changes: AtomicU64,
    pub host_last_change_after_lost_ms: AtomicU64,
    /// The replica stream.
    pub replica: super::replica::ReplicaStats,
}

impl GameStats {
    /// A restart took `ms` from the failure to the next ready worker.
    pub fn note_restart(&self, ms: u64) {
        let mut v = self
            .restart_ms
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        v.push(ms);
        if v.len() > 20 {
            v.remove(0);
        }
        self.restore.record_ms(ms);
    }

    pub fn restart_ms(&self) -> Vec<u64> {
        self.restart_ms
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn set_last_save(&self, s: SaveLast) {
        *self
            .saves_last
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(s);
    }

    pub fn last_save(&self) -> Option<SaveLast> {
        self.saves_last
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// Seat stats of a game by player.
#[derive(Debug, Default)]
pub struct SeatStatsMap {
    map: Mutex<BTreeMap<PlayerId, Arc<SeatStats>>>,
}

impl SeatStatsMap {
    /// The stats of a seat, created on first use.
    pub fn seat(&self, player: PlayerId) -> Arc<SeatStats> {
        Arc::clone(
            self.map
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entry(player)
                .or_default(),
        )
    }

    pub fn all(&self) -> Vec<(PlayerId, Arc<SeatStats>)> {
        self.map
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(p, s)| (*p, Arc::clone(s)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_ai_phase_is_one_sample_taken_when_it_ends() {
        use protocol::StepKind;
        let mut acc = AiPhaseAcc::default();
        // A phase of three pumps: two in the middle, the last one hands the turn to the humans.
        assert_eq!(acc.step(StepKind::AiPhase, 400), None);
        assert_eq!(acc.step(StepKind::AiPhase, 600), None);
        assert_eq!(acc.step(StepKind::TurnChange, 250), Some(1250));
        // A phase of a single pump ends at once.
        assert_eq!(acc.step(StepKind::TurnChange, 90), Some(90));
        // Steps with no AI work (orders, the first pump of the game) make no sample.
        assert_eq!(acc.step(StepKind::Orders, 0), None);
        assert_eq!(acc.step(StepKind::TurnChange, 0), None);
    }

    use super::*;

    #[test]
    fn an_empty_histogram_is_all_zero() {
        let h = Hist::default();
        assert_eq!(h.json()["n"], 0);
        assert_eq!(h.json()["p99"], 0);
    }

    #[test]
    fn quantiles_land_in_the_right_bucket_and_never_leave_min_and_max() {
        let h = Hist::default();
        for ms in 1..=100u64 {
            h.record_ms(ms);
        }
        let j = h.json();
        assert_eq!(j["n"], 100);
        assert_eq!(j["min"], 1.0);
        assert_eq!(j["max"], 100.0);
        let p50 = j["p50"].as_f64().unwrap();
        let p95 = j["p95"].as_f64().unwrap();
        let p99 = j["p99"].as_f64().unwrap();
        // Quarter-octave buckets: within 20 % of the exact value.
        assert!((40.0..=62.0).contains(&p50), "{p50}");
        assert!((90.0..=100.0).contains(&p95), "{p95}");
        assert!(p95 <= p99 && p99 <= 100.0, "{p99}");
        assert_eq!(j["mean"], 50.5);
    }

    #[test]
    fn several_histograms_add_up() {
        let (a, b) = (Hist::default(), Hist::default());
        a.record_ms(2);
        b.record_ms(10);
        let mut sum = HistSum::default();
        a.add_to(&mut sum);
        b.add_to(&mut sum);
        let j = sum.json();
        assert_eq!(
            (j["n"].as_u64(), j["min"].as_f64(), j["max"].as_f64()),
            (Some(2), Some(2.0), Some(10.0))
        );
    }

    #[test]
    fn tx_by_turn_keeps_the_last_four_hundred_turns() {
        let s = SeatStats::default();
        for turn in 0..450 {
            s.note_tx(turn, 10, turn % 2 == 0);
            s.note_tx(turn, 5, false);
        }
        let t = s.tx_by_turn();
        assert_eq!(t.len(), TX_TURNS);
        assert_eq!(t[0].turn, 50);
        assert_eq!((t[0].batches, t[0].body_bytes, t[0].keyframes), (2, 15, 1));
    }
}
