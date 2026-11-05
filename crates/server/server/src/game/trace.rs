//! The turn timeline of a game at the gateway (mp-latency-timeline spec, packet 1): the traces the worker sends
//! (`WorkerMsg::Trace`) merged with the gateway's own spans (`g.cmd`, `g.turn_fwd`, `g.blob_serve`) in a ring of the
//! last turns, the `span` and `turn_summary` lines of the game journal, and the exports of
//! `GET /admin/games/{id}/trace` (Chrome/Perfetto JSON, journal lines, summaries).
//!
//! `CIV5_TRACE=off|coarse|fine` (default `coarse`) switches it; `off` records nothing and the endpoint answers an empty
//! trace. The worker and the gateway run on one machine: both clocks are unix time at their start plus the monotonic
//! time since, which agree to well under a millisecond.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

use protocol::{Codec, GameId, Postcard};
use serde_json::{Value, json};
use turntrace::{Clock, Kind, Level, MonoClock, Rec, Src, TraceStore, TurnTrace, summarize};

use super::gamelog::GameLog;
use super::recording::Recorder;

/// Turns the gateway keeps per game (`[trace] keep_turns`).
pub const KEEP_TURNS: usize = 64;
/// Bodies whose hash maps to a turn (for `g.blob_serve`).
const KEEP_BLOBS: usize = 16;

#[derive(Debug, Default)]
struct Inner {
    store: Option<TraceStore>,
    /// Body hash -> the turn that was ended.
    blobs: HashMap<u64, i32>,
    blob_order: Vec<u64>,
    next_id: u32,
}

/// The journal writer puts `ts` and `kind` itself.
fn strip(mut v: Value) -> Value {
    if let Value::Object(m) = &mut v {
        m.remove("ts");
        m.remove("kind");
    }
    v
}

/// The timeline of one game.
pub struct GameTrace {
    game: GameId,
    level: Level,
    clock: MonoClock,
    log: GameLog,
    rec: Recorder,
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for GameTrace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GameTrace")
    }
}

impl GameTrace {
    pub fn new(game: GameId, log: GameLog, rec: Recorder) -> Self {
        Self::with_level(game, log, rec, Level::from_env())
    }

    pub fn with_level(game: GameId, log: GameLog, rec: Recorder, level: Level) -> Self {
        GameTrace {
            game,
            level,
            clock: MonoClock::default(),
            log,
            rec,
            inner: Mutex::new(Inner {
                store: Some(TraceStore::new(KEEP_TURNS)),
                ..Inner::default()
            }),
        }
    }

    pub fn level(&self) -> Level {
        self.level
    }

    /// Now, unix microseconds (the axis of the worker's spans).
    pub fn now_us(&self) -> u64 {
        self.clock.now_us()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self, t: &TurnTrace) {
        // The recording keeps the whole trace of the end of a turn in the shared envelope (postcard `TurnTrace`).
        // SIMPLIFIED(SIMP-exe-813): the timing in the recording is these traces and the clock on every record.
        if self.rec.is_on()
            && let Ok(bytes) = Postcard.encode(t)
        {
            self.rec
                .put(recording::Body::Trace(recording::format::TraceRec {
                    turn: t.turn,
                    bytes,
                }));
        }
        for line in t.lines(0) {
            let kind = line["kind"].as_str().unwrap_or("span").to_owned();
            self.log.event(&kind, strip(line));
        }
    }

    /// A span of the gateway for the end of `turn`, from `t0_us` to now.
    pub fn span(&self, turn: i32, name: &str, lane: &str, t0_us: u64, args: &[(&str, i64)]) {
        if self.level == Level::Off {
            return;
        }
        let dur = self.now_us().saturating_sub(t0_us);
        self.record(turn, name, lane, t0_us, dur, args);
    }

    /// A span of the gateway with its duration known.
    pub fn record(
        &self,
        turn: i32,
        name: &str,
        lane: &str,
        t0_us: u64,
        dur_us: u64,
        args: &[(&str, i64)],
    ) {
        if self.level == Level::Off {
            return;
        }
        let rec = {
            let mut g = self.lock();
            g.next_id = g.next_id.wrapping_add(1).max(1);
            Rec {
                kind: Kind::Span,
                src: Src::Gateway,
                seat: -1,
                span: name.to_owned(),
                lane: lane.to_owned(),
                id: g.next_id,
                parent: 0,
                t0_us,
                dur_us,
                args: args.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect(),
            }
        };
        let t = TurnTrace {
            game: u64::from(self.game),
            turn,
            recs: vec![rec],
        };
        self.write(&t);
        if let Some(s) = self.lock().store.as_mut() {
            s.add(t);
        }
    }

    /// The hash of a body of the turn that follows `ended` (`g.blob_serve` names its turn by it).
    pub fn note_blob(&self, hash: u64, ended: i32) {
        let mut g = self.lock();
        if g.blobs.insert(hash, ended).is_none() {
            g.blob_order.push(hash);
            while g.blob_order.len() > KEEP_BLOBS {
                let old = g.blob_order.remove(0);
                g.blobs.remove(&old);
            }
        }
    }

    /// The turn a body belongs to.
    pub fn blob_turn(&self, hash: u64) -> Option<i32> {
        self.lock().blobs.get(&hash).copied()
    }

    /// The worker's trace of the end of `turn`: merged, written to the journal with its summary. Returns the summary.
    pub fn on_worker_trace(&self, turn: i32, bytes: &[u8]) -> Option<turntrace::Summary> {
        if self.level == Level::Off {
            return None;
        }
        let t: TurnTrace = match Postcard.decode(bytes) {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(
                    game = self.game,
                    turn,
                    "a worker trace does not decode: {e}"
                );
                return None;
            }
        };
        self.write(&t);
        let s = summarize(&t);
        self.log.event("turn_summary", strip(s.line(0)));
        if let Some(st) = self.lock().store.as_mut() {
            st.add(t);
        }
        Some(s)
    }

    /// The traces of turns `lo..=hi`.
    pub fn traces(&self, lo: i32, hi: i32) -> Vec<TurnTrace> {
        self.lock()
            .store
            .as_ref()
            .map(|s| s.range(lo, hi).into_iter().cloned().collect())
            .unwrap_or_default()
    }

    /// The Chrome/Perfetto JSON of turns `lo..=hi`.
    pub fn chrome(&self, lo: i32, hi: i32) -> Value {
        let ts = self.traces(lo, hi);
        turntrace::chrome(&ts.iter().collect::<Vec<_>>())
    }

    /// The journal lines of turns `lo..=hi`, one per line.
    pub fn jsonl(&self, lo: i32, hi: i32) -> String {
        let mut out = String::new();
        for t in self.traces(lo, hi) {
            for l in t.lines(0) {
                out.push_str(&l.to_string());
                out.push('\n');
            }
        }
        out
    }

    /// One summary per turn of `lo..=hi` that has a worker trace.
    pub fn summaries(&self, lo: i32, hi: i32) -> Vec<turntrace::Summary> {
        self.traces(lo, hi)
            .iter()
            .filter(|t| t.find("w.end_turn").is_some() || t.find("w.apply").is_some())
            .map(summarize)
            .collect()
    }

    /// The summaries as text, a line each.
    pub fn summary_text(&self, lo: i32, hi: i32) -> String {
        let mut out = String::new();
        for s in self.summaries(lo, hi) {
            out.push_str(&s.text());
            out.push('\n');
        }
        out
    }

    /// The turns held.
    pub fn turns(&self) -> Vec<i32> {
        self.lock()
            .store
            .as_ref()
            .map(TraceStore::turns)
            .unwrap_or_default()
    }

    /// For the listing of the endpoint.
    pub fn info(&self) -> Value {
        json!({"level": format!("{:?}", self.level).to_lowercase(), "turns": self.turns()})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trace_with(turn: i32, span: &str, t0: u64, dur: u64) -> TurnTrace {
        TurnTrace {
            game: 936,
            turn,
            recs: vec![Rec {
                kind: Kind::Span,
                src: Src::Worker,
                seat: -1,
                span: span.into(),
                lane: "core".into(),
                id: 1,
                parent: 0,
                t0_us: t0,
                dur_us: dur,
                args: vec![("pump_us".into(), dur as i64)],
            }],
        }
    }

    #[test]
    fn a_worker_trace_and_a_gateway_span_meet_in_the_store() {
        let g = GameTrace::with_level(
            936,
            GameLog::disabled(),
            Recorder::disabled(),
            Level::Coarse,
        );
        let mut t = trace_with(126, "w.end_turn", 1_000, 0);
        t.recs[0].kind = Kind::Mark;
        t.recs
            .push(trace_with(126, "w.ai", 2_000, 3_000_000).recs.remove(0));
        // The pack after the AI closes the turn's window.
        t.recs
            .push(trace_with(126, "w.pack", 3_002_000, 100_000).recs.remove(0));
        let bytes = Postcard.encode(&t).unwrap();
        let s = g.on_worker_trace(126, &bytes).expect("a summary");
        assert_eq!(s.server_ai_us, 3_000_000);
        g.record(126, "g.cmd", "io", 900, 50, &[("cmd", 41)]);
        let all = g.traces(126, 126);
        assert_eq!(all.len(), 1);
        let names: Vec<&str> = all[0].recs.iter().map(|r| r.span.as_str()).collect();
        assert_eq!(
            names,
            ["g.cmd", "w.end_turn", "w.ai", "w.pack"],
            "sorted by start"
        );
        // The Chrome export has a process per source.
        let j = g.chrome(126, 126);
        let procs: Vec<&str> = j["traceEvents"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["name"] == "process_name")
            .filter_map(|e| e["args"]["name"].as_str())
            .collect();
        assert_eq!(procs, ["gateway", "worker"]);
        assert!(g.jsonl(126, 126).lines().count() == 4);
        assert!(g.summary_text(126, 126).contains("turn 126"));
    }

    #[test]
    fn off_records_nothing() {
        let g = GameTrace::with_level(1, GameLog::disabled(), Recorder::disabled(), Level::Off);
        g.record(1, "g.cmd", "io", 0, 1, &[]);
        let t = trace_with(1, "w.ai", 0, 1);
        assert!(
            g.on_worker_trace(1, &Postcard.encode(&t).unwrap())
                .is_none()
        );
        assert!(g.traces(0, 9).is_empty());
    }

    #[test]
    fn a_blob_names_its_turn_and_the_list_is_bounded() {
        let g = GameTrace::with_level(1, GameLog::disabled(), Recorder::disabled(), Level::Coarse);
        for h in 0..(KEEP_BLOBS as u64 + 5) {
            g.note_blob(h, h as i32);
        }
        assert_eq!(g.blob_turn(0), None);
        assert_eq!(
            g.blob_turn(KEEP_BLOBS as u64 + 4),
            Some(KEEP_BLOBS as i32 + 4)
        );
    }
}
