//! The watch of the core thread: heartbeats while one core call runs long, and a log of what it is.
//!
//! The worker's own heartbeat goes out between calls (`Worker::finish`), so one long call is silence, and the
//! gateway kills a worker that is silent for `hang_s` (30 s). Live game 936 (2026-10-31, turn 126): the END TURN
//! of the last human ran the whole AI phase of a Huge map with ten AI in one `gc_pump` (14 s on a desktop, past
//! 30 s on a small hosted server); the worker was killed three times, each restart came back to the same turn, and the game was
//! faulted. A busy core is not a hung one: while a call runs, this thread sends `Heartbeat` every second, logs
//! what the call is every [`WARN_EVERY`], and after `budget` (`CIV5_CORE_CALL_BUDGET_S`, default
//! [`DEFAULT_BUDGET`]) stops beating and logs that it gave up, so a core that really loops is still ended by the
//! gateway's silence rule and the log names the call.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use protocol::{Codec, Postcard, TurnPhase, WorkerFrame, WorkerMsg};

/// How long a call may run with heartbeats; past it the worker falls silent.
pub const DEFAULT_BUDGET: Duration = Duration::from_secs(300);
/// A long call is logged at warn level this often while it runs.
pub const WARN_EVERY: Duration = Duration::from_secs(10);
/// A call that took longer than this is logged when it ends.
const SLOW: Duration = Duration::from_secs(5);

/// The budget of the environment (`CIV5_CORE_CALL_BUDGET_S`, seconds), else [`DEFAULT_BUDGET`].
pub fn budget_from_env() -> Duration {
    std::env::var("CIV5_CORE_CALL_BUDGET_S")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .filter(|&s| s > 0)
        .map_or(DEFAULT_BUDGET, Duration::from_secs)
}

/// The clock of the watch (tests drive it by hand).
type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// The call the core thread is in.
struct Call {
    since: Instant,
    what: String,
    phase: TurnPhase,
    /// The next warn line is due at this run time.
    warn_at: Duration,
    /// The budget is spent: no more heartbeats for this call.
    given_up: bool,
    /// A `RecMark` went out when the call began.
    marked: bool,
    turn: i32,
}

#[derive(Default)]
struct State {
    call: Option<Call>,
    /// A number per call, so the watch thread's view of "the same call" survives a quick end and start.
    seq: u64,
}

/// The watch thread and the state it shares with the core thread.
pub struct Watch {
    state: Arc<Mutex<State>>,
    now: Clock,
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
    game: u64,
    /// The recording is on (`WorkerConfig.record`): the AI-phase calls announce themselves (`WorkerMsg::RecMark`).
    record: Arc<AtomicBool>,
    frames: Option<Sender<Vec<u8>>>,
    last_mark: Mutex<Option<Instant>>,
}

/// An AI-phase call is announced at most this often (the phase is pumped in small calls; the text is the same).
const MARK_EVERY: Duration = Duration::from_millis(500);
/// A marked call that ran at least this long is closed with an end mark.
const MARK_END_AFTER: Duration = Duration::from_secs(1);

impl Watch {
    /// Starts the thread; `frames` is the queue of the stdout thread (`Writer::queue`). `every`: the heartbeat
    /// period, `budget`: how long one call may run with heartbeats.
    pub fn spawn(
        game: u64,
        frames: Option<Sender<Vec<u8>>>,
        every: Duration,
        budget: Duration,
    ) -> Self {
        Self::spawn_with_clock(game, frames, every, budget, Arc::new(Instant::now))
    }

    fn spawn_with_clock(
        game: u64,
        frames: Option<Sender<Vec<u8>>>,
        every: Duration,
        budget: Duration,
        now: Clock,
    ) -> Self {
        let state = Arc::new(Mutex::new(State::default()));
        let beat_frames = frames.clone();
        let (stop, rx) = mpsc::channel::<()>();
        let shared = Arc::clone(&state);
        let clock = Arc::clone(&now);
        let record = Arc::new(AtomicBool::new(false));
        let thread_record = Arc::clone(&record);
        let thread = super::io::logged_spawn(
            "core-watch",
            std::thread::Builder::new()
                .name("core-watch".into())
                .spawn(move || {
                    loop {
                        match rx.recv_timeout(every) {
                            Err(RecvTimeoutError::Timeout) => {}
                            _ => return,
                        }
                        let mut late_mark = None;
                        let beat = {
                            let mut s = shared.lock().unwrap_or_else(PoisonError::into_inner);
                            let Some(call) = s.call.as_mut() else {
                                continue;
                            };
                            let now = clock();
                            // A call of any phase that has run past a second and was not marked at its start (only the AI
                            // phase marks then): named in the recording now, closed at its end (SIMP-exe-815).
                            if thread_record.load(Ordering::Relaxed)
                                && !call.marked
                                && now.saturating_duration_since(call.since) >= MARK_END_AFTER
                            {
                                call.marked = true;
                                late_mark = Some((
                                    call.turn,
                                    call.what.chars().take(240).collect::<String>(),
                                ));
                            }
                            tick(game, call, every, budget, now)
                        };
                        if let (Some((turn, what)), Some(frames)) =
                            (late_mark, beat_frames.as_ref())
                        {
                            let frame = WorkerFrame {
                                msgs: vec![WorkerMsg::RecMark {
                                    turn,
                                    what,
                                    begin: true,
                                }],
                            };
                            if let Ok(body) = Postcard.encode(&frame) {
                                let _ = frames.send(body);
                            }
                        }
                        if let (Some(phase), Some(frames)) = (beat, beat_frames.as_ref()) {
                            let frame = WorkerFrame {
                                msgs: vec![WorkerMsg::Heartbeat { phase, queued: 0 }],
                            };
                            if let Ok(body) = Postcard.encode(&frame)
                                && frames.send(body).is_err()
                            {
                                return;
                            }
                        }
                    }
                }),
        );
        Watch {
            state,
            now,
            stop: Some(stop),
            thread,
            game,
            record,
            frames,
            last_mark: Mutex::new(None),
        }
    }

    /// Turns the recording marks on (`WorkerConfig.record`).
    #[must_use]
    pub fn with_record(self, on: bool) -> Self {
        self.record.store(on, Ordering::Relaxed);
        self
    }

    fn send_mark(&self, turn: i32, what: &str, begin: bool) {
        let Some(frames) = &self.frames else {
            return;
        };
        let what: String = what.chars().take(240).collect();
        let frame = WorkerFrame {
            msgs: vec![WorkerMsg::RecMark { turn, what, begin }],
        };
        if let Ok(body) = Postcard.encode(&frame) {
            let _ = frames.send(body);
        }
    }

    /// The core thread enters a call in game turn `turn`; it ends when the guard is dropped. With the recording on, a
    /// call in the AI phase tells the gateway what it is before it runs (the AI phase is the call that can run for
    /// minutes), so a call that never returns is named in the recording.
    pub fn enter(&self, what: impl FnOnce() -> String, phase: TurnPhase, turn: i32) -> Guard<'_> {
        let what = what();
        let mut marked = false;
        // SIMPLIFIED(SIMP-exe-815): a call of the AI phase is marked at its start, at most twice a second; a call of
        // any other phase is marked by the watch thread once it has run a second (a call shorter than that leaves no
        // mark).
        if self.record.load(Ordering::Relaxed) && matches!(phase, TurnPhase::Ai { .. }) {
            let now = (self.now)();
            let mut last = self
                .last_mark
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if last.is_none_or(|t| now.saturating_duration_since(t) >= MARK_EVERY) {
                *last = Some(now);
                drop(last);
                self.send_mark(turn, &what, true);
                marked = true;
            }
        }
        let mut s = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        s.seq += 1;
        s.call = Some(Call {
            since: (self.now)(),
            what,
            phase,
            warn_at: WARN_EVERY,
            given_up: false,
            marked,
            turn,
        });
        Guard { watch: self }
    }

    fn leave(&self) {
        let call = self
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .call
            .take();
        if let Some(c) = call {
            let took = (self.now)().saturating_duration_since(c.since);
            if c.marked && took >= MARK_END_AFTER {
                self.send_mark(c.turn, &c.what, false);
            }
            if took >= SLOW {
                tracing::warn!(
                    game = self.game,
                    took_ms = u64::try_from(took.as_millis()).unwrap_or(u64::MAX),
                    "a slow core call ended: {}",
                    c.what
                );
            }
        }
    }

    fn halt(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.halt();
    }
}

/// One second of the watch over `call`: `Some(phase)` when a heartbeat is due.
fn tick(
    game: u64,
    call: &mut Call,
    every: Duration,
    budget: Duration,
    now: Instant,
) -> Option<TurnPhase> {
    if call.given_up {
        return None;
    }
    let ran = now.saturating_duration_since(call.since);
    if ran >= budget {
        call.given_up = true;
        tracing::error!(
            game,
            ran_s = ran.as_secs(),
            budget_s = budget.as_secs(),
            "a core call runs past its budget, no more heartbeats (the gateway will end the worker): {}",
            call.what
        );
        return None;
    }
    if ran >= call.warn_at {
        call.warn_at += WARN_EVERY;
        tracing::warn!(
            game,
            ran_s = ran.as_secs(),
            "a core call is still running: {}",
            call.what
        );
    }
    // A call shorter than a period needs no beat: the worker's own comes after it.
    (ran >= every).then_some(call.phase)
}

/// A call of the core thread; dropping it ends the call.
pub struct Guard<'a> {
    watch: &'a Watch,
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        self.watch.leave();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::Receiver;

    use super::*;

    fn marks(rx: &Receiver<Vec<u8>>) -> Vec<(String, bool)> {
        rx.try_iter()
            .filter_map(|b| {
                let f = Postcard.decode::<WorkerFrame>(&b).ok()?;
                match f.msgs.as_slice() {
                    [WorkerMsg::RecMark { what, begin, .. }] => Some((what.clone(), *begin)),
                    _ => None,
                }
            })
            .collect()
    }

    #[test]
    fn a_long_call_outside_the_ai_phase_is_marked_once_it_runs_a_second() {
        use std::sync::atomic::AtomicU64;
        let (tx, rx) = mpsc::channel();
        let base = Instant::now();
        let ms = Arc::new(AtomicU64::new(0));
        let clock_ms = Arc::clone(&ms);
        let mut w = Watch::spawn_with_clock(
            1,
            Some(tx),
            Duration::from_millis(10),
            Duration::from_secs(60),
            Arc::new(move || base + Duration::from_millis(clock_ms.load(Ordering::SeqCst))),
        )
        .with_record(true);
        let deadline = Instant::now() + Duration::from_secs(30);
        // A short call: no mark at all.
        drop(w.enter(|| "quick order".into(), TurnPhase::Humans, 3));
        assert!(marks(&rx).is_empty());
        let g = w.enter(|| "order: found city".into(), TurnPhase::Humans, 3);
        let mut seen = Vec::new();
        ms.store(1500, Ordering::SeqCst);
        while seen.is_empty() {
            assert!(Instant::now() < deadline, "no mark for a long call");
            std::thread::sleep(Duration::from_millis(1));
            seen.extend(marks(&rx));
        }
        assert_eq!(seen, [("order: found city".to_owned(), true)]);
        // Not marked twice; the end mark comes when it returns.
        std::thread::sleep(Duration::from_millis(40));
        assert!(marks(&rx).is_empty());
        drop(g);
        w.halt();
        assert_eq!(marks(&rx), [("order: found city".to_owned(), false)]);
    }

    fn beats(rx: &Receiver<Vec<u8>>) -> usize {
        rx.try_iter()
            .filter(|b| {
                Postcard
                    .decode::<WorkerFrame>(b)
                    .is_ok_and(|f| matches!(f.msgs.as_slice(), [WorkerMsg::Heartbeat { .. }]))
            })
            .count()
    }

    #[test]
    fn a_long_call_beats_and_an_idle_worker_does_not() {
        use std::sync::atomic::{AtomicU64, Ordering};
        let (tx, rx) = mpsc::channel();
        let base = Instant::now();
        let ms = Arc::new(AtomicU64::new(0));
        let clock_ms = Arc::clone(&ms);
        let mut w = Watch::spawn_with_clock(
            1,
            Some(tx),
            Duration::from_millis(10),
            Duration::from_secs(60),
            Arc::new(move || base + Duration::from_millis(clock_ms.load(Ordering::SeqCst))),
        );
        // The clock is far past a period, but no call runs: no beats however many ticks come.
        ms.store(1000, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(beats(&rx), 0, "no call, no beats");
        let deadline = Instant::now() + Duration::from_secs(30);
        {
            let _g = w.enter(|| "step".into(), TurnPhase::Humans, 1);
            ms.store(1120, Ordering::SeqCst);
            let mut n = 0;
            while n < 3 {
                assert!(Instant::now() < deadline, "{n} heartbeats in a call");
                std::thread::sleep(Duration::from_millis(1));
                n += beats(&rx);
            }
        }
        // The thread is joined: at most one beat was in flight when the call ended, and none comes after.
        w.halt();
        assert!(beats(&rx) <= 1, "the call ended, no more beats");
    }
    #[test]
    fn a_call_past_its_budget_falls_silent() {
        use std::sync::atomic::{AtomicU64, Ordering};
        let (tx, rx) = mpsc::channel();
        // The clock is the test's: milliseconds since `base`, moved by hand.
        let base = Instant::now();
        let ms = Arc::new(AtomicU64::new(0));
        let clock_ms = Arc::clone(&ms);
        let mut w = Watch::spawn_with_clock(
            1,
            Some(tx),
            Duration::from_millis(10),
            Duration::from_millis(50),
            Arc::new(move || base + Duration::from_millis(clock_ms.load(Ordering::SeqCst))),
        );
        let given_up = |w: &Watch| {
            w.state
                .lock()
                .unwrap()
                .call
                .as_ref()
                .is_some_and(|c| c.given_up)
        };
        let deadline = Instant::now() + Duration::from_secs(30);
        let g = w.enter(|| "a loop".into(), TurnPhase::Ai { active: 3 }, 1);
        // Inside the budget (20 ms of 50): the thread beats.
        ms.store(20, Ordering::SeqCst);
        let mut seen = 0;
        while seen == 0 {
            assert!(Instant::now() < deadline, "no beat inside the budget");
            std::thread::sleep(Duration::from_millis(1));
            seen += beats(&rx);
            assert!(!given_up(&w), "given up inside the budget");
        }
        // Past the budget: the next tick gives up.
        ms.store(60, Ordering::SeqCst);
        while !given_up(&w) {
            assert!(Instant::now() < deadline, "the watch never gave up");
            std::thread::sleep(Duration::from_millis(1));
        }
        // What was queued before the give-up is drained here; after it, nothing comes while the call goes on.
        beats(&rx);
        drop(g);
        w.halt();
        assert_eq!(beats(&rx), 0, "silent after the budget");
    }
}
