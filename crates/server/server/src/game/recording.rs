//! The recording of a game at the gateway (packet P2): what the game actor sees, in the order it sees it,
//! on disk in `recordings/g<game>-<created_ms>/` (`crates/shared/recording` has the format).
//!
//! [`Recorder`] is a cheap handle (`GameInfo.rec`). Every call stamps the time and queues the record for a thread of its
//! own that compresses and writes, so the actor never waits on the disk and the worker's core thread is not involved at
//! all: a worker that dies or hangs loses nothing that already crossed the pipe. A disabled recorder (`[recording]
//! enabled = false`, no `state_dir`) drops everything.
//!
//! SIMPLIFIED(SIMP-exe-810): a directory per game life, no `index.json`. The writer stops when the disk has less than
//! `[recording] min_free_mb` free (checked every 30 s with `df`; where `df` is not there the byte caps alone hold).
//!
//! The extra records (`BatchMeta`, `OrderSpan`) come out of a fixed budget per game life ([`META_BUDGET_BYTES`]); past
//! it they are dropped (and counted), so they cannot grow the journal without a limit. SIMPLIFIED(SIMP-exe-982).
//!
//! The thread flushes the zstd stream every two seconds, so a gateway that dies leaves a recording that decodes up to the
//! last flush. At the end of the game (the handle is dropped or [`Recorder::finish`] is called) the stream is closed with
//! its checksum and the manifest says when the game ended.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use protocol::GameId;
use recording::format::{
    Body, Entry, HeaderRec, JournalRec, KeyframeRec, KfKind, VERSION, WorkerEv, WorkerRec,
};
use recording::writer::{Config as WriterConfig, RecWriter, Stats};

use crate::clock::WallClock;
use crate::config::Recording as RecordingConfig;

enum Msg {
    Entry(Entry),
    Keyframe(Entry, Option<Bytes>),
    Finish(Option<u64>),
}

struct Inner {
    tx: Sender<Msg>,
    mono: Instant,
    wall: Arc<dyn WallClock>,
    dir_name: String,
    stats: Arc<Stats>,
    full_every: u32,
    /// Bytes (an estimate of the raw size) the extra records have used.
    meta_used: AtomicU64,
    meta_dropped: AtomicU64,
}

/// What the extra records (`BatchMeta`, `OrderSpan`) may add to one game life's journal (raw bytes, estimated).
pub const META_BUDGET_BYTES: u64 = 8 * 1024 * 1024;

/// The handle of a game's recording. Clones share the recording.
#[derive(Clone)]
pub struct Recorder {
    inner: Option<Arc<Inner>>,
}

impl std::fmt::Debug for Recorder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Recorder")
    }
}

/// The directory name of a recording: `g936-1793399061318`.
pub fn dir_name(game: GameId, created_ms: u64) -> String {
    format!("g{game}-{created_ms}")
}

/// The game and creation time in a recording directory name.
pub fn parse_dir_name(name: &str) -> Option<(GameId, u64)> {
    let rest = name.strip_prefix('g')?;
    let (g, ms) = rest.split_once('-')?;
    Some((g.parse().ok()?, ms.parse().ok()?))
}

/// What a snapshot announcement carries, for [`Recorder::keyframe`].
pub struct KeyframeArgs {
    pub turn: i32,
    pub epoch: u32,
    pub jseq: u64,
    pub raw_hash: u64,
    /// Hash and body of the full snapshot.
    pub snapshot: (u64, Bytes),
    /// Hash, `from_raw_hash` and body of the patch from the previous turn's snapshot, if there is one.
    pub patch: Option<(u64, u64, Bytes)>,
    /// The first snapshot of this worker: the chain begins here.
    pub base: bool,
}

impl Recorder {
    pub fn disabled() -> Self {
        Recorder { inner: None }
    }

    /// Opens `<state_dir>/recordings/g<game>-<now>`. A failure leaves a disabled recorder and a line in the log.
    pub fn open(
        state_dir: &Path,
        game: GameId,
        settings: &protocol::GameSettings,
        seed: u64,
        wall: Arc<dyn WallClock>,
        cfg: &RecordingConfig,
    ) -> Self {
        if !cfg.enabled {
            return Self::disabled();
        }
        let created_ms = wall.unix_ms();
        let dir_name = dir_name(game, created_ms);
        let dir = state_dir.join("recordings").join(&dir_name);
        let header = HeaderRec {
            format: VERSION,
            game,
            gateway: crate::BUILD.to_owned(),
            core_build: String::new(),
            seed,
            wall_at_mono0_ms: created_ms,
            created_ms,
            name: settings.name.clone(),
            settings: Some(settings.clone()),
            seats: Vec::new(),
        };
        let wcfg = WriterConfig {
            full_every: cfg.full_every.max(1),
            detail_turns: cfg.detail_turns,
            max_game_bytes: cfg.max_game_mb.saturating_mul(1024 * 1024),
            min_free_bytes: cfg.min_free_mb.saturating_mul(1024 * 1024),
            ..WriterConfig::default()
        };
        let stats = Arc::new(Stats::default());
        let writer = match RecWriter::create(&dir, wcfg, &header, Arc::clone(&stats)) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!(game, dir = %dir.display(), error = %e, "cannot open the game recording");
                return Self::disabled();
            }
        };
        let (tx, rx) = mpsc::channel();
        let thread_stats = Arc::clone(&stats);
        let spawned = std::thread::Builder::new()
            .name("game-rec".into())
            .spawn(move || run(writer, &rx, game, &thread_stats));
        if spawned.is_err() {
            return Self::disabled();
        }
        let rec = Recorder {
            inner: Some(Arc::new(Inner {
                tx,
                mono: Instant::now(),
                wall,
                dir_name,
                stats,
                full_every: cfg.full_every.max(1),
                meta_used: AtomicU64::new(0),
                meta_dropped: AtomicU64::new(0),
            })),
        };
        rec.put(Body::Header(header));
        rec
    }

    pub fn is_on(&self) -> bool {
        self.inner.is_some()
    }

    /// `g936-1793399061318`; `None` when disabled.
    pub fn dir_name(&self) -> Option<&str> {
        self.inner.as_ref().map(|i| i.dir_name.as_str())
    }

    /// The counters of the writer (records, bytes, time spent).
    pub fn stats(&self) -> Option<&Arc<Stats>> {
        self.inner.as_ref().map(|i| &i.stats)
    }

    fn entry(inner: &Inner, body: Body) -> Entry {
        Entry {
            mono_us: u64::try_from(inner.mono.elapsed().as_micros()).unwrap_or(u64::MAX),
            wall_ms: inner.wall.unix_ms(),
            body,
        }
    }

    /// One record, stamped now.
    pub fn put(&self, body: Body) {
        if let Some(i) = &self.inner {
            let _ = i.tx.send(Msg::Entry(Self::entry(i, body)));
        }
    }

    /// An extra record, if the meta budget has room (`cost`: its estimated raw size).
    fn put_meta(&self, cost: u64, body: Body) {
        let Some(i) = &self.inner else { return };
        if i.meta_used.fetch_add(cost, Ordering::Relaxed) + cost > META_BUDGET_BYTES {
            i.meta_dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let _ = i.tx.send(Msg::Entry(Self::entry(i, body)));
    }

    /// The size and tags of a `GameBatch` sent to a seat.
    pub fn batch_meta(&self, player: i8, seq: u64, bytes: u32, tags: Vec<u8>) {
        if !self.is_on() {
            return;
        }
        let cost = 24 + tags.len() as u64;
        self.put_meta(
            cost,
            Body::BatchMeta(recording::format::BatchMetaRec {
                player,
                seq,
                bytes,
                tags,
            }),
        );
    }

    /// How long the gateway had an order, from the worker's pipe to the final answer.
    pub fn order_span(&self, player: i8, id: u32, us: u64) {
        if !self.is_on() {
            return;
        }
        self.put_meta(
            32,
            Body::OrderSpan(recording::format::OrderSpanRec {
                player,
                id,
                stage: "cmd.worker".to_owned(),
                us,
            }),
        );
    }

    /// The extra records the budget refused.
    pub fn meta_dropped(&self) -> u64 {
        self.inner
            .as_ref()
            .map_or(0, |i| i.meta_dropped.load(Ordering::Relaxed))
    }

    /// A worker event.
    pub fn worker(&self, ev: WorkerEv) {
        self.put(Body::Worker(WorkerRec { ev }));
    }

    /// A seat event (`attach`, `detach`, `claim`, `kick`, `force_end_turn` ...).
    pub fn seat(&self, player: i8, event: &str, nick: Option<&str>, detail: String) {
        self.put(Body::Seat(recording::format::SeatRec {
            player,
            event: event.to_owned(),
            nick: nick.map(str::to_owned),
            detail,
        }));
    }

    /// A GCJ1 batch of the core.
    pub fn journal(&self, rec: JournalRec) {
        self.put(Body::Journal(rec));
    }

    /// The snapshot of the start of a human phase: it opens a new segment. A full body is kept for a base, when
    /// there is no patch and every `full_every` turns; the other turns keep the (much smaller) patch.
    pub fn keyframe(&self, a: KeyframeArgs) {
        let Some(i) = &self.inner else {
            return;
        };
        let full = a.base
            || a.patch.is_none()
            || a.turn.rem_euclid(i32::try_from(i.full_every).unwrap_or(10)) == 0;
        let (kind, hash, from, body) = match (full, a.patch) {
            (false, Some((hash, from, body))) => (KfKind::Patch, hash, Some(from), body),
            _ => (KfKind::Full, a.snapshot.0, None, a.snapshot.1),
        };
        let rec = KeyframeRec {
            turn: a.turn,
            epoch: a.epoch,
            jseq: a.jseq,
            raw_hash: a.raw_hash,
            hash,
            kind,
            from_raw_hash: from,
            file: None,
            bytes: u32::try_from(body.len()).unwrap_or(u32::MAX),
            base: a.base,
        };
        let e = Self::entry(i, Body::Keyframe(rec));
        let _ = i.tx.send(Msg::Keyframe(e, Some(body)));
    }

    /// The game ended: close the stream and write the manifest. The recorder keeps working for stragglers after it
    /// only until the handles are dropped.
    pub fn finish(&self) {
        if let Some(i) = &self.inner {
            let _ = i.tx.send(Msg::Finish(Some(i.wall.unix_ms())));
        }
    }
}

fn run(mut w: RecWriter, rx: &mpsc::Receiver<Msg>, game: GameId, stats: &Stats) {
    let mut failed = false;
    let mut finished = false;
    let mut note = |r: std::io::Result<()>| {
        if let Err(e) = r
            && !failed
        {
            failed = true;
            tracing::error!(game, error = %e, "the game recording stops: a write failed");
        }
    };
    loop {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(Msg::Entry(e)) => {
                if let Body::Header(h) = &e.body
                    && !h.core_build.is_empty()
                {
                    w.set_core_build(&h.core_build);
                }
                note(w.write(&e));
            }
            Ok(Msg::Keyframe(e, body)) => note(w.keyframe(e, body.as_deref())),
            Ok(Msg::Finish(at)) => {
                finished = true;
                note(w.finish(at));
                log_totals(game, stats);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                // Every handle is gone without a `finish` (a gateway that stops): close the stream, leave the end open.
                if !finished {
                    note(w.finish(None));
                    log_totals(game, stats);
                }
                return;
            }
        }
        w.tick(Instant::now());
    }
}

fn log_totals(game: GameId, s: &Stats) {
    use std::sync::atomic::Ordering::Relaxed;
    tracing::info!(
        game,
        records = s.records.load(Relaxed),
        raw_kb = s.raw_bytes.load(Relaxed) / 1024,
        segments_kb = s.seg_bytes.load(Relaxed) / 1024,
        keyframes_kb = s.kf_bytes.load(Relaxed) / 1024,
        flushes = s.flushes.load(Relaxed),
        busy_ms = s.busy_us.load(Relaxed) / 1000,
        "game recording closed"
    );
}

#[cfg(test)]
mod tests {
    use recording::Recording;
    use recording::reader::Tail;

    use super::*;
    use crate::clock::TestWallClock;

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("civ5-gwrec-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    fn settings() -> protocol::GameSettings {
        protocol::GameSettings {
            name: "t".into(),
            world_size: "WORLDSIZE_DUEL".into(),
            map_script: "m".into(),
            map_options: vec![],
            speed: "s".into(),
            start_era: "e".into(),
            handicap: "h".into(),
            minor_civs: 0,
            turn_mode: protocol::TurnMode::Hybrid,
            timer: protocol::TimerMode::Off,
            game_options: vec![],
            seed: Some(1),
            map_seed: None,
            auto_choices: protocol::AutoChoices::default(),
            diplo_auto_decline: true,
            mods: Vec::new(),
        }
    }

    #[test]
    fn names_round_trip() {
        assert_eq!(dir_name(936, 5), "g936-5");
        assert_eq!(
            parse_dir_name("g936-1793399061318"),
            Some((936, 1_793_399_061_318))
        );
        assert_eq!(parse_dir_name("index.json"), None);
        assert_eq!(parse_dir_name("g1"), None);
    }

    #[test]
    fn a_recorder_writes_segments_per_keyframe_and_closes_cleanly() {
        let dir = scratch("basic");
        let rec = Recorder::open(
            &dir,
            7,
            &settings(),
            1,
            Arc::new(TestWallClock::new(5000)),
            &RecordingConfig::default(),
        );
        let name = rec.dir_name().unwrap().to_owned();
        assert_eq!(name, "g7-5000");
        let snap = Bytes::from(vec![9u8; 200]);
        let patch = Bytes::from(vec![8u8; 20]);
        for turn in 1..=3 {
            rec.journal(JournalRec {
                phase: protocol::RecPhase::Ai,
                epoch: 1,
                jseq: 0,
                turn,
                bytes: vec![turn as u8; 30],
            });
            rec.keyframe(KeyframeArgs {
                turn,
                epoch: 1,
                jseq: 0,
                raw_hash: 77,
                snapshot: (protocol::xxh64(0, &snap), snap.clone()),
                patch: Some((protocol::xxh64(0, &patch), 66, patch.clone())),
                base: turn == 1,
            });
        }
        rec.worker(WorkerEv::Fault { why: "x".into() });
        rec.finish();
        drop(rec);
        // The writer thread ends when the handles are gone; wait for the manifest to say finished.
        let path = dir.join("recordings").join(&name);
        let mut r = None;
        for _ in 0..100 {
            if let Ok(x) = Recording::open(&path)
                && x.manifest.finished_ms.is_some()
            {
                r = Some(x);
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let r = r.expect("the recording is closed");
        let segs = r.read_all().unwrap();
        assert!(segs.iter().all(|s| s.tail == Tail::Clean));
        // seg 0: header + journal; seg 1..3: keyframe (+ journal / worker).
        assert_eq!(segs.len(), 4);
        let kinds: Vec<_> = r
            .manifest
            .keyframes
            .iter()
            .map(|k| (k.turn, k.kind))
            .collect();
        // Turn 1 is a base (full); 2 and 3 keep the patch.
        assert_eq!(
            kinds,
            [(1, KfKind::Full), (2, KfKind::Patch), (3, KfKind::Patch)]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn batch_meta_and_order_spans_are_written_within_a_fixed_budget() {
        let dir = scratch("meta");
        let rec = Recorder::open(
            &dir,
            8,
            &settings(),
            1,
            Arc::new(TestWallClock::new(7000)),
            &RecordingConfig::default(),
        );
        let name = rec.dir_name().unwrap().to_owned();
        rec.batch_meta(1, 5, 1234, vec![0, 3, 4]);
        rec.order_span(1, 9, 2500);
        assert_eq!(rec.meta_dropped(), 0);
        // A flood past the budget is dropped and counted, not written.
        let n = META_BUDGET_BYTES / 32 + 10;
        for i in 0..n {
            rec.order_span(0, i as u32, 1);
        }
        assert!(rec.meta_dropped() > 0);
        rec.finish();
        drop(rec);
        let path = dir.join("recordings").join(&name);
        let mut r = None;
        for _ in 0..200 {
            if let Ok(x) = Recording::open(&path)
                && x.manifest.finished_ms.is_some()
            {
                r = Some(x);
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let r = r.expect("closed");
        let all: Vec<_> = r
            .read_all()
            .unwrap()
            .into_iter()
            .flat_map(|s| s.entries)
            .collect();
        let meta = all
            .iter()
            .find_map(|e| match &e.body {
                Body::BatchMeta(m) => Some(m.clone()),
                _ => None,
            })
            .expect("a BatchMeta record");
        assert_eq!(
            (meta.player, meta.seq, meta.bytes, meta.tags),
            (1, 5, 1234, vec![0, 3, 4])
        );
        let spans = all
            .iter()
            .filter(|e| matches!(e.body, Body::OrderSpan(_)))
            .count() as u64;
        assert!(spans >= 1 && spans < n, "written {spans} of {n}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_disabled_recorder_costs_nothing() {
        let off = Recorder::disabled();
        off.worker(WorkerEv::Note("x".into()));
        off.finish();
        assert!(off.dir_name().is_none() && !off.is_on());
        let dir = scratch("off");
        let cfg = RecordingConfig {
            enabled: false,
            ..RecordingConfig::default()
        };
        let rec = Recorder::open(
            &dir,
            1,
            &settings(),
            1,
            Arc::new(TestWallClock::new(1)),
            &cfg,
        );
        assert!(!rec.is_on());
        assert!(!dir.exists());
    }
}
