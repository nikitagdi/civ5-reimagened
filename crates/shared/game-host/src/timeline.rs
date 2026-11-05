//! The timeline of the end of a turn (mp-latency-timeline spec, packet 1) and the progress of the AI phase.
//!
//! The worker records the spans of the catalog that happen on its threads (`w.queue`, `w.apply`, `w.check_batch`,
//! `w.ai`, `w.turn_change`, `w.save`, `w.pack_queue`, `w.pack`, `w.patch`) into a [`turntrace::Recorder`], keyed by the
//! turn being ended. When the snapshot of the next turn is packed, the trace of the turn goes to the gateway as
//! `WorkerMsg::Trace`.
//!
//! The AI phase is pumped in small slices ([`AI_STEP_SLICES`]) so the worker is free between two calls: it takes orders,
//! beats, and tells which civilization the core started and ended (`WorkerMsg::AiTurn`, SIMP-exe-820): the core's
//! `setTurnActive` of every non-human civilization, in order, with the frame of the call that did it (an Ended reaches the
//! clients within one pump call; nothing is held back or thinned out). The client lights the AI icons of the turn queue
//! with it and counts "N of M" (SIMP-exe-790).

use std::collections::BTreeMap;
use std::sync::Arc;

use protocol::{AiTurnEvent, CmdId, Codec, Command, PlayerId, Postcard, TurnPhase, WorkerMsg};
use turntrace::{Level, MonoClock, Recorder, Src, TurnTrace};

use crate::core::{Core, CoreError, CorePhase, CorePump};
use crate::worker::{Fb, Worker};

/// Slices of the core one call runs while the AI phase is pumped. The whole AI is a few hundred slices; one call of
/// 32 ran 3-9 s on a late Huge game (live game 936) with the worker deaf and silent.
pub(crate) const AI_STEP_SLICES: u32 = 4;
/// Turns of traces the worker keeps after they were sent.
const KEEP_TURNS: usize = 16;
/// Open turns that are sent at once if the snapshot never comes (a failed pack).
const MAX_OPEN_TURNS: usize = 4;

/// The AI phase being pumped.
#[derive(Debug)]
struct AiRun {
    /// The turn that is ended.
    turn: i32,
    t0_us: u64,
    pump_us: u64,
    slices: u32,
    calls: u32,
}

/// What the timeline remembers between calls.
pub(crate) struct Timeline {
    pub rec: Recorder,
    /// When the END_TURN orders were accepted: `(seat, order)` -> microseconds.
    cmd_in: BTreeMap<(PlayerId, CmdId), u64>,
    ai: Option<AiRun>,
    /// The AI phase ended in the step that is running: `(turn ended, end of the last pump)`.
    ai_done: Option<(i32, u64)>,
    /// The core's `setTurnActive` of the non-human civilizations since the last frame: `(turn ended, events)`.
    turn_status: Option<(i32, Vec<AiTurnEvent>)>,
    /// Traces to send with the next frame.
    out: Vec<(i32, Vec<u8>)>,
}

impl Timeline {
    pub fn new(game: u64) -> Self {
        Timeline {
            rec: Recorder::new(
                Arc::new(MonoClock::default()),
                Level::Coarse,
                Src::Worker,
                game,
                KEEP_TURNS,
            ),
            cmd_in: BTreeMap::new(),
            ai: None,
            ai_done: None,
            turn_status: None,
            out: Vec::new(),
        }
    }
}

impl<C: Core> Worker<C> {
    /// The level of the recorder (the shell reads `CIV5_TRACE`).
    pub fn set_trace_level(&mut self, level: Level) {
        self.tl.rec.set_level(level);
    }

    /// The phase the core thread is busy with, for the heartbeats of a long call: the AI when the next call is the
    /// pump after the last END_TURN (the worker is still in `Humans` then).
    pub fn watch_phase(&self) -> TurnPhase {
        match self.phase {
            TurnPhase::Humans if self.pump_due => TurnPhase::Ai { active: 0 },
            p => p,
        }
    }

    /// An END_TURN order was accepted.
    pub(crate) fn tl_cmd_accepted(&mut self, player: PlayerId, id: CmdId, cmd: &Command) {
        if matches!(cmd, Command::EndTurn { .. })
            && self.tl.rec.level() != Level::Off
            && self.tl.cmd_in.len() < 64
        {
            let now = self.tl.rec.now_us();
            self.tl.cmd_in.insert((player, id), now);
        }
    }

    /// The core took an END_TURN: the queue wait, the apply and the instant itself.
    pub(crate) fn tl_end_turn_applied(&mut self, player: PlayerId, id: CmdId, t0_us: u64) {
        let turn = self.turn;
        let now = self.tl.rec.now_us();
        let args = [("player", i64::from(player)), ("cmd", i64::from(id))];
        if let Some(q) = self.tl.cmd_in.remove(&(player, id)) {
            self.tl
                .rec
                .span(turn, "w.queue", "core", q, t0_us.saturating_sub(q), &args);
        }
        self.tl.rec.span(
            turn,
            "w.apply",
            "core",
            t0_us,
            now.saturating_sub(t0_us),
            &args,
        );
        self.tl.rec.mark(turn, "w.end_turn", "core", &args);
    }

    /// A snapshot job for the next turn was made: from `t0_us` (the step's end of work) to now.
    pub(crate) fn tl_snapshot(&mut self, t0_us: u64) {
        let now = self.tl.rec.now_us();
        let turn = self.turn - 1;
        self.tl.rec.span(
            turn,
            "w.snapshot",
            "core",
            t0_us,
            now.saturating_sub(t0_us),
            &[],
        );
    }

    /// The AI phase is being pumped (it began in a pump and has not ended).
    pub(crate) fn tl_ai_running(&self) -> bool {
        self.tl.ai.is_some()
    }

    /// Now on the recorder's clock.
    pub(crate) fn tl_now(&self) -> u64 {
        self.tl.rec.now_us()
    }

    /// The checked batch before the AI: a span with the batch number.
    pub(crate) fn tl_check_batch(&mut self, t0_us: u64, jseq_before: u64, jseq: u64) {
        let now = self.tl.rec.now_us();
        let args = [
            ("jseq", i64::try_from(jseq).unwrap_or(0)),
            ("new", i64::from(jseq != jseq_before)),
        ];
        let turn = self.turn;
        self.tl.rec.span(
            turn,
            "w.check_batch",
            "core",
            t0_us,
            now.saturating_sub(t0_us),
            &args,
        );
    }

    /// One call of the core's pump with the bookkeeping of the AI phase; `ends_turn`: the call follows an END_TURN.
    /// Books the AI time into `times.ai_us`, records `w.ai` when the phase ends and queues the progress.
    pub(crate) fn pump_core(&mut self, ends_turn: bool) -> Result<(CorePump, u32), CoreError> {
        let before_turn = self.turn;
        let before = self.phase;
        let ai_pump = self.tl.ai.is_some()
            || matches!(before, TurnPhase::Ai { .. })
            || (ends_turn && before == TurnPhase::Humans);
        // SIMPLIFIED(SIMP-exe-791): the AI phase is pumped in calls of at most 4 slices.
        let budget = if ai_pump {
            self.cfg.pump_budget.min(AI_STEP_SLICES)
        } else {
            self.cfg.pump_budget
        };
        let t0 = self.tl.rec.now_us();
        let t = std::time::Instant::now();
        let p = self.core.pump(budget)?;
        let us = crate::worker::micros(t);
        // SIMPLIFIED(SIMP-exe-821): what the core activated and deactivated in this call goes out with this call's frame,
        // not at the instant of the core's `setTurnActive` (SIMP-exe-820). The turn
        // is the one being ended: a pump that left the turn changed the counter already.
        let status = self.core.turn_status();
        if !status.is_empty() {
            let turn = self.tl.ai.as_ref().map_or(before_turn, |r| r.turn);
            match self.tl.turn_status.as_mut() {
                Some((t, v)) if *t == turn => v.extend(status),
                _ => self.tl.turn_status = Some((turn, status)),
            }
        }
        if !ai_pump {
            return Ok((p, us));
        }
        let now = self.tl.rec.now_us();
        let slices = self.core.last_pump_slices();
        // The AI phase begins with the first pump after the last END_TURN that leaves the humans.
        let began = self.tl.ai.is_some()
            || matches!(before, TurnPhase::Ai { .. })
            || p.turn != before_turn
            || !matches!(p.phase, CorePhase::Humans);
        if !began {
            return Ok((p, us));
        }
        self.times.ai_us += us;
        if self.tl.ai.is_none() {
            self.tl.ai = Some(AiRun {
                turn: before_turn,
                t0_us: t0,
                pump_us: 0,
                slices: 0,
                calls: 0,
            });
        }
        let Some(run) = self.tl.ai.as_mut() else {
            return Ok((p, us));
        };
        run.pump_us += u64::from(us);
        run.slices += slices;
        run.calls += 1;
        if !matches!(p.phase, CorePhase::Ai { .. })
            && let Some(run) = self.tl.ai.take()
        {
            let args = [
                ("pump_us", i64::try_from(run.pump_us).unwrap_or(i64::MAX)),
                ("slices", i64::from(run.slices)),
                ("calls", i64::from(run.calls)),
            ];
            let turn = run.turn;
            self.tl.rec.span(
                turn,
                "w.ai",
                "core",
                run.t0_us,
                now.saturating_sub(run.t0_us),
                &args,
            );
            self.tl.ai_done = Some((turn, now));
        }
        Ok((p, us))
    }

    /// After the work of a step: the turn change span and the `w.save` of the step; a trace is closed here when the
    /// core has no replica stream (otherwise the pack closes it, [`Self::tl_pack_done`]).
    pub(crate) fn tl_step_end(&mut self, save_us_before: u32) {
        if self.tl.rec.level() == Level::Off {
            return;
        }
        let now = self.tl.rec.now_us();
        if self.times.save_us > save_us_before {
            let dur = u64::from(self.times.save_us - save_us_before);
            let turn = self.tl.ai_done.map_or(self.turn - 1, |d| d.0);
            self.tl.rec.span(
                turn,
                "w.save",
                "core",
                now.saturating_sub(dur),
                dur,
                &[("turn", i64::from(self.turn))],
            );
        }
        if let Some((turn, ai_end)) = self.tl.ai_done.take() {
            let args = [
                ("pump_us", i64::from(self.times.pump_us)),
                ("views_us", i64::from(self.times.views_us)),
            ];
            self.tl.rec.span(
                turn,
                "w.turn_change",
                "core",
                ai_end,
                now.saturating_sub(ai_end),
                &args,
            );
            if self.core.replica().is_none() {
                self.tl_close(turn);
            }
        }
        // A pack that never came back must not keep the traces open for ever.
        let open = self.tl.rec.open_turns();
        if open.len() > MAX_OPEN_TURNS {
            for t in open.iter().take(open.len() - MAX_OPEN_TURNS) {
                self.tl_close(*t);
            }
        }
    }

    /// The pack thread is done with the snapshot of `turn` (the next turn): its spans, then the trace of the ended
    /// turn goes.
    pub(crate) fn tl_pack_done(&mut self, packed: &PackTimes, turn: i32) {
        if self.tl.rec.level() == Level::Off {
            return;
        }
        let now = self.tl.rec.now_us();
        let pack_t0 = now.saturating_sub(packed.pack_us + packed.patch_us);
        let ended = turn - 1;
        let rec = &mut self.tl.rec;
        if pack_t0 > packed.queued_us {
            rec.span(
                ended,
                "w.pack_queue",
                "pack",
                packed.queued_us,
                pack_t0 - packed.queued_us,
                &[],
            );
        }
        rec.span(
            ended,
            "w.pack",
            "pack",
            pack_t0,
            packed.pack_us,
            &[("bytes", i64::try_from(packed.snapshot_bytes).unwrap_or(0))],
        );
        if let Some(p) = packed.patch_bytes {
            rec.span(
                ended,
                "w.patch",
                "pack",
                pack_t0 + packed.pack_us,
                packed.patch_us,
                &[("bytes", i64::try_from(p).unwrap_or(0))],
            );
        }
        let old: Vec<i32> = rec.open_turns().into_iter().filter(|t| *t < turn).collect();
        for t in old {
            self.tl_close(t);
        }
    }

    fn tl_close(&mut self, turn: i32) {
        if let Some(tr) = self.tl.rec.close_turn(turn)
            && let Ok(bytes) = Postcard.encode(&tr)
        {
            self.tl.out.push((turn, bytes));
        }
    }

    /// Puts the traces and the progress that are due into the frame (`finish`).
    pub(crate) fn tl_flush(&mut self, fb: &mut Fb) {
        for (turn, bytes) in std::mem::take(&mut self.tl.out) {
            fb.tail.push(WorkerMsg::Trace { turn, bytes });
        }
        if let Some((turn, events)) = self.tl.turn_status.take() {
            fb.tail.push(WorkerMsg::AiTurn { turn, events });
        }
    }

    /// The traces of the closed turns kept by the worker, oldest first (tests).
    pub fn closed_traces(&self) -> Vec<TurnTrace> {
        self.tl.rec.closed().cloned().collect()
    }
}

/// What the pack of a snapshot took, for the spans.
pub(crate) struct PackTimes {
    /// When the job was made (recorder clock).
    pub queued_us: u64,
    pub pack_us: u64,
    pub patch_us: u64,
    pub snapshot_bytes: usize,
    pub patch_bytes: Option<usize>,
}
