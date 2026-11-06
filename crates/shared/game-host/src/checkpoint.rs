//! Saves of the worker (game-host-mp 4.11, 4.12, GH-D9, GH-D10): when to save, what goes into the file, what
//! to do when the shell says the file is written, and how a game continues from a save.
//!
//! The worker does no I/O: it asks the core for its part (`Core::save`), adds the seats, the settings, the
//! journals, the last orders and the memory of cities, and hands an [`SaveJob`] to the shell as `Out::Save`.
//! The save thread of the shell packs and writes it (`saves::encode`), then the shell calls
//! `Worker::save_done`.
//!
//! When (4.11): at the start of every human phase (after the views, before the queued orders); in a safe point
//! when 60 s have passed since the last save and an order was applied; on `SaveNow` and `Shutdown { save }`
//! in the nearest safe point. A start-of-turn save of a turn that `keep_every` divides goes to `keep/`.
//! An empty `save_dir` in `Init` turns the saves off (unit tests of the worker).

use std::collections::{BTreeMap, VecDeque};
use std::time::Instant;

use protocol::{
    CivPick, CmdId, PlayerId, SaveKind, SaveMeta, SaveRecord, SlotControl, TurnPhase, WorkerMsg,
};

use crate::core::{Core, CoreError, CoreErrorKind, EventTarget};
use crate::journal;
use crate::saves::{self, Save, SaveHeader, SaveSections};
use crate::seats::TeamData;
use crate::worker::{Fb, Out, Worker};

/// Wait before the next look for a safe point for a save that waits.
const RETRY_MS: u64 = 50;
/// The longest wait between two tries of a save the core refused (`CoreErrorKind::NotSafe`): the wait doubles
/// from `RETRY_MS` up to this.
const MAX_BACKOFF_MS: u64 = 5_000;
/// How long `Shutdown { save: true }` waits for a safe point (game-host-mp 4.12).
const FINAL_WAIT_MS: u64 = 5_000;
/// How long it waits for the shell to write the final file.
const FINAL_WRITE_MS: u64 = 30_000;

/// Which file of the save directory a save is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaveFile {
    pub seq: u64,
    pub kind: SaveKind,
    pub turn: i32,
}

impl SaveFile {
    /// `ring/<seq:08>-t<turn:04>.c5sv` or `keep/...`, relative to the save directory.
    pub fn rel_path(&self) -> String {
        saves::rel_path(self.kind, self.seq, self.turn)
    }
}

/// A save for the save thread of the shell to pack and write: `saves::encode(&header, &gcs1, &sections)` with
/// `header.saved_at_ms` set from the wall clock, to `file.rel_path()` under the save directory, then the ring
/// trimmed to `ring` files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveJob {
    pub file: SaveFile,
    pub header: SaveHeader,
    /// The core's part, not packed.
    pub gcs1: Vec<u8>,
    pub sections: SaveSections,
    /// Files the ring keeps (`AutosaveCfg::ring`).
    pub ring: u32,
}

/// The answer of the shell to a [`SaveJob`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveDone {
    pub file: SaveFile,
    /// The wall clock time put into the header, Unix milliseconds.
    pub saved_at_ms: u64,
    /// Size of the file, or why it was not written.
    pub result: Result<u64, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Why {
    Start,
    Timer,
    Now,
    Final,
}

struct InFlight {
    record: SaveRecord,
    /// The last save before the process ends.
    last: bool,
}

/// What the worker knows about its saves.
pub(crate) struct SaveState {
    pub enabled: bool,
    next_seq: u64,
    last_ms: u64,
    /// An order was applied since the last save.
    pub dirty: bool,
    start_due: bool,
    now_due: bool,
    /// `Shutdown { save: true }` came: the time by which the save must be made, or written.
    final_by: Option<u64>,
    final_sent: bool,
    /// After a load: the first human phase is the one the save was made in.
    pub skip_start: bool,
    retry_ms: u64,
    /// The core refused the last save (`NotSafe`): no save is tried before this time.
    hold_until_ms: u64,
    /// The wait after the last refusal; 0 after a save that was made.
    backoff_ms: u64,
    inflight: BTreeMap<u64, InFlight>,
    jobs: Vec<SaveJob>,
}

impl SaveState {
    pub fn new(enabled: bool, now_ms: u64) -> Self {
        SaveState {
            enabled,
            next_seq: 1,
            last_ms: now_ms,
            dirty: false,
            start_due: false,
            now_due: false,
            final_by: None,
            final_sent: false,
            skip_start: false,
            retry_ms: 0,
            hold_until_ms: 0,
            backoff_ms: 0,
            inflight: BTreeMap::new(),
            jobs: Vec::new(),
        }
    }

    /// The jobs made in this call, for the shell.
    pub fn take_jobs(&mut self) -> Vec<Out> {
        self.jobs
            .drain(..)
            .map(|j| Out::Save(Box::new(j)))
            .collect()
    }
}

impl<C: Core> Worker<C> {
    /// The next save gets a number above `seq` at least (the shell saw files of that number in the save
    /// directory). Call it before the first step.
    pub fn set_seq_floor(&mut self, seq: u64) {
        self.saves.next_seq = self.saves.next_seq.max(seq.saturating_add(1));
    }

    /// The start of a human phase: a save is due (a game that was just loaded already is one).
    pub(crate) fn save_at_turn_start(&mut self) {
        if std::mem::take(&mut self.saves.skip_start) {
            return;
        }
        self.saves.start_due = true;
    }

    /// An order was applied: the 60-second save has a reason now.
    pub(crate) fn note_applied(&mut self) {
        self.saves.dirty = true;
    }

    /// `ToWorker::SaveNow`.
    pub(crate) fn save_now(&mut self, fb: &mut Fb) -> Result<(), CoreError> {
        if !self.saves.enabled {
            fb.tail.push(WorkerMsg::SaveFailed {
                reason: "saving is off (no save directory)".into(),
            });
            return Ok(());
        }
        self.saves.now_due = true;
        self.try_save(fb)
    }

    /// `ToWorker::Shutdown`: with `save` a last save first, then the exit.
    pub(crate) fn shutdown(&mut self, save: bool, fb: &mut Fb) -> Result<(), CoreError> {
        if !save || !self.saves.enabled {
            self.dead = true;
            return Ok(());
        }
        self.saves.final_by = Some(self.now_ms + FINAL_WAIT_MS);
        self.try_save(fb)
    }

    /// The shell gave up waiting for a safe point: the process ends without the save.
    pub(crate) fn final_expired(&mut self) {
        if let Some(by) = self.saves.final_by
            && self.now_ms >= by
        {
            tracing::warn!("the final save was not made in time, the worker stops without it");
            self.dead = true;
        }
    }

    /// The time the shell should wake the worker for the saves: a save that waits for a safe point, the
    /// 60-second save, the end of the wait of a shutdown.
    pub(crate) fn save_wake_ms(&self) -> Option<u64> {
        let s = &self.saves;
        if !s.enabled {
            return None;
        }
        let waiting = s.start_due || s.now_due || (s.final_by.is_some() && !s.final_sent);
        let mut at: Option<u64> = None;
        let mut soonest = |t: u64| at = Some(at.map_or(t, |a| a.min(t)));
        if waiting {
            soonest(s.retry_ms);
        }
        if let Some(by) = s.final_by {
            soonest(by);
        }
        if s.dirty && self.phase == TurnPhase::Humans {
            let interval = u64::from(self.cfg.autosave.min_interval_s) * 1000;
            soonest((s.last_ms + interval).max(s.hold_until_ms));
        }
        at
    }

    /// Makes the save that is due, if the core is at a safe point; otherwise it stays due and is tried again.
    pub(crate) fn try_save(&mut self, fb: &mut Fb) -> Result<(), CoreError> {
        if !self.saves.enabled {
            return Ok(());
        }
        let interval = u64::from(self.cfg.autosave.min_interval_s) * 1000;
        let s = &self.saves;
        let timer = self.phase == TurnPhase::Humans
            && s.dirty
            && self.now_ms.saturating_sub(s.last_ms) >= interval;
        let why = if s.final_by.is_some() && !s.final_sent {
            Why::Final
        } else if s.now_due {
            Why::Now
        } else if s.start_due {
            Why::Start
        } else if timer {
            Why::Timer
        } else {
            return Ok(());
        };
        if matches!(self.phase, TurnPhase::Starting | TurnPhase::Ai { .. }) {
            self.saves.retry_ms = self.now_ms + RETRY_MS;
            return Ok(());
        }
        if self.now_ms < self.saves.hold_until_ms {
            // The core refused a save a moment ago: the next try waits (no hot loop, live game 931).
            self.saves.retry_ms = self.saves.hold_until_ms;
            return Ok(());
        }
        if let Err(why_not) = self.core.safe_point() {
            self.note_no_safe_point("a save", &why_not);
            self.saves.retry_ms = self.now_ms + RETRY_MS;
            return Ok(());
        }
        self.note_safe_point();
        let last = why == Why::Final;
        let kind = if self.saves.start_due {
            saves::start_kind(self.turn, self.cfg.autosave.keep_every)
        } else {
            SaveKind::Ring
        };
        let t = Instant::now();
        let made = self.make_job(kind);
        self.times.save_us += crate::worker::micros(t);
        if let Err(e) = &made
            && e.kind == CoreErrorKind::NotSafe
        {
            // `Q_GAME.safe_point` said yes but `gc_save` says no: a human's unit or city is still busy (a move the
            // core times per pump slice). The save stays due; the core is pumped so that the unit settles, and
            // the save is tried again after a wait that doubles up to `MAX_BACKOFF_MS`.
            let s = &mut self.saves;
            s.backoff_ms = (s.backoff_ms * 2).clamp(RETRY_MS, MAX_BACKOFF_MS);
            s.hold_until_ms = self.now_ms + s.backoff_ms;
            s.retry_ms = s.hold_until_ms;
            tracing::info!(
                "turn {}: no save point yet, the save waits {} ms: {}",
                self.turn,
                s.backoff_ms,
                e.message
            );
            self.settle(fb)?;
            return Ok(());
        }
        self.saves.start_due = false;
        self.saves.now_due = false;
        self.saves.backoff_ms = 0;
        self.saves.hold_until_ms = 0;
        match made {
            Ok(job) => {
                let seq = job.file.seq;
                let cmds: Vec<(PlayerId, CmdId)> = job.sections.cmds.clone();
                let record = SaveRecord {
                    meta: SaveMeta {
                        seq,
                        build_id: job.header.build_id.clone(),
                        turn: job.header.turn,
                        civ_name: first_civ(&job.sections),
                        saved_at_ms: 0,
                    },
                    file: job.file.rel_path(),
                    kind: job.file.kind,
                    applied_upto: cmds,
                };
                self.saves.inflight.insert(seq, InFlight { record, last });
                self.saves.jobs.push(job);
                self.saves.last_ms = self.now_ms;
                self.saves.dirty = false;
                if last {
                    self.saves.final_sent = true;
                    self.saves.final_by = Some(self.now_ms + FINAL_WRITE_MS);
                }
                Ok(())
            }
            Err(e) if e.kind == CoreErrorKind::Failed => {
                self.save_failed(fb, &e.message, last);
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// One pump in the human phase so that a busy unit or city of a human settles (the core counts a move down
    /// one slice per pump, and a pump that ends because another human is up runs one slice only); the views
    /// show what the pump changed. A save or a snapshot the core refused (`NotSafe`) calls it.
    pub(crate) fn settle(&mut self, fb: &mut Fb) -> Result<(), CoreError> {
        if self.phase != TurnPhase::Humans {
            return Ok(());
        }
        self.pump_after_order(false)?;
        if self.phase == TurnPhase::Humans {
            self.build_round(fb)?;
        }
        self.refresh_turn(fb)
    }

    /// The core's part and the sections of the worker, as a job for the shell.
    fn make_job(&mut self, kind: SaveKind) -> Result<SaveJob, CoreError> {
        let settings = self
            .settings
            .clone()
            .ok_or_else(|| CoreError::failed("no game to save"))?;
        let gcs1 = self.core_save_shared()?;
        let seats = self.core.seats()?;
        let seq = self.saves.next_seq;
        self.saves.next_seq += 1;
        let journals = self
            .seats
            .iter()
            .filter(|(_, s)| !s.journal.is_empty())
            .map(|(p, s)| (*p, s.journal.clone()))
            .collect();
        let cmds = self
            .seats
            .iter()
            .filter_map(|(p, s)| s.applied_upto.map(|id| (*p, id)))
            .collect();
        let city_memo = self
            .teams
            .iter()
            .filter(|(_, d)| !d.memo.is_empty())
            .map(|(t, d)| (*t, d.memo.values().cloned().collect()))
            .collect();
        Ok(SaveJob {
            file: SaveFile {
                seq,
                kind,
                turn: self.turn,
            },
            header: SaveHeader {
                seq,
                game: self.game,
                turn: self.turn,
                saved_at_ms: 0,
                kind,
                build_id: self.core.build_id(),
            },
            gcs1,
            sections: SaveSections {
                seats,
                settings,
                journals,
                cmds,
                city_memo,
                // T15 fills this from the game's frozen mod set.
                mods: None,
            },
            ring: self.cfg.autosave.ring,
        })
    }

    /// A save that failed: the gateway hears `SaveFailed`, the seats a `System` line; the game goes on.
    fn save_failed(&mut self, fb: &mut Fb, why: &str, last: bool) {
        tracing::warn!("save failed: {why}");
        fb.tail.push(WorkerMsg::SaveFailed {
            reason: why.to_owned(),
        });
        let entry = journal::system_entry(self.turn, format!("The game could not be saved: {why}"));
        self.journal_pending
            .route(EventTarget::All, self.seats.keys().copied(), &entry);
        if last {
            self.dead = true;
        }
    }

    /// The shell wrote (or did not write) a save. A written save is told to the gateway (`Saved`); after the
    /// final one the worker ends.
    pub fn save_done(&mut self, done: SaveDone, now_ms: u64) -> Vec<Out> {
        if self.dead {
            return Vec::new();
        }
        self.now_ms = now_ms;
        let mut fb = Fb::default();
        if let Some(f) = self.saves.inflight.remove(&done.file.seq) {
            match done.result {
                Ok(_size) => {
                    let mut record = f.record;
                    record.meta.saved_at_ms = done.saved_at_ms;
                    fb.tail.push(WorkerMsg::Saved(record));
                    if f.last {
                        self.dead = true;
                    }
                }
                Err(why) => self.save_failed(&mut fb, &why, f.last),
            }
        }
        self.flush_journal(&mut fb);
        self.finish(fb, now_ms, Ok(()))
    }

    // ------------------------------------------------------------ loading

    /// Takes what a save holds that is not the core's: the last orders, the journals, the memory of cities,
    /// the number of the next save. Called before the first view.
    pub(crate) fn adopt_save(&mut self, save: &Save) {
        self.saves.next_seq = self
            .saves
            .next_seq
            .max(save.info.header.seq.saturating_add(1));
        self.saves.skip_start = true;
        for (p, id) in &save.cmds {
            if let Some(seat) = self.seats.get_mut(p) {
                seat.applied_upto = Some(*id);
            }
        }
        // The journals of the seats go out first, with the first views.
        for (p, entries) in &save.journals {
            for e in entries {
                self.journal_pending
                    .route(EventTarget::Player(*p), self.seats.keys().copied(), e);
            }
        }
        let line = journal::system_entry(
            save.info.header.turn,
            format!(
                "The game continues from the save of turn {}",
                save.info.header.turn
            ),
        );
        self.journal_pending
            .route(EventTarget::All, self.seats.keys().copied(), &line);
        for (team, memos) in save.city_memo.iter().flatten() {
            let mut data = TeamData::new(*team);
            data.memo = memos.iter().map(|m| ((m.owner, m.id), m.clone())).collect();
            self.teams.insert(*team, data);
        }
    }

    /// Hands the slots to the owners the gateway named, once, at the first safe point after the load
    /// (game-host-mp 4.12): a slot whose owner differs from the save's gets `Human` (in the core that is
    /// `CONNECT`), a slot the save held for a human and the gateway gave to the AI goes to the AI.
    ///
    /// A human the save already holds (the same nick) is NOT connected (decision 2026-10-27, `gc-abi-v2` 4.9):
    /// a connection does not survive a save, the human is off line only because of the load and stays so
    /// until the person logs in. The core does not play for an off-line human and the turn waits for its end
    /// of turn as before; `CvPlayer::reconnected` does not run for it.
    ///
    /// A `Kick` leaves the core at an unsafe point (the AI that took the slot still has its turn to play,
    /// `gc_slot_control` of `gc-abi-v2` 4.9) until the next pump, so every other change is made first (a
    /// `CONNECT` after a `Kick` was refused: a save whose human gave its slot to somebody else, or to the AI
    /// while another slot was taken, never loaded), and the kicks come last, one per safe point: after a kick
    /// the core is pumped (also after the last one: the save of the turn start needs a safe point), and when
    /// that is not the human phase again the rest waits (`load_controls` keeps it) for the next time the
    /// humans are up.
    ///
    /// `Ok(true)`: all done, the humans are up and the views may be built. `Ok(false)`: the AI is computing
    /// after a kick; the caller goes on pumping and calls this again at the next human phase.
    pub(crate) fn apply_load_controls(&mut self) -> Result<bool, CoreError> {
        let Some(list) = self.load_controls.take() else {
            return Ok(true);
        };
        let core_now = self.core.seats()?;
        let mut todo: Vec<(PlayerId, SlotControl)> = Vec::new();
        for (player, control) in list {
            let held = core_now
                .iter()
                .find(|s| s.player == player)
                .map(|s| &s.control);
            let needed = match (&control, held) {
                (SlotControl::Human { .. }, Some(h)) => *h != control,
                (SlotControl::Human { .. }, None) => true,
                (SlotControl::Ai, Some(SlotControl::Human { .. })) => true,
                (SlotControl::Ai, _) => false,
            };
            if needed {
                todo.push((player, control));
            }
        }
        // The stable sort keeps the order of the players within the connects and within the kicks.
        todo.sort_by_key(|(_, c)| matches!(c, SlotControl::Ai));
        let mut todo: VecDeque<(PlayerId, SlotControl)> = todo.into();
        let mut kicked = false;
        loop {
            if kicked {
                // The AI that took the slot plays its turn before anything else is done (or the views built).
                self.pump_after_order(false)?;
                match self.phase {
                    // Nobody moves any more: the views of the last state are all there is.
                    TurnPhase::GameOver { .. } => return Ok(true),
                    TurnPhase::Humans => {}
                    _ => {
                        self.load_controls = (!todo.is_empty()).then(|| todo.into_iter().collect());
                        return Ok(false);
                    }
                }
            }
            let Some((player, control)) = todo.pop_front() else {
                return Ok(true);
            };
            // A human of the save may be in auto moves (automated workers): the core is not at a safe point
            // until a pump has played them, and `gc_slot_control` refuses before that. Pump, do not die.
            let mut waited = 0;
            while let Err(why) = self.core.safe_point() {
                if waited == 0 {
                    // Pumping alone never leaves "auto moves": the human has nothing left to do and waits for
                    // its end of turn. A slot going to the AI has its turn ended for it; a slot going to a
                    // human of another nick (a save of the original's multiplayer game, taken after the end of
                    // its player's turn) is not played by anybody yet either, and the new owner starts at the
                    // next turn.
                    let _ = self.core.end_turn_timeout(player);
                }
                waited += 1;
                if waited > MAX_SAFE_POINT_PUMPS {
                    let seats = self
                        .core
                        .turn()
                        .map(|t| format!("{:?}", t.seats))
                        .unwrap_or_default();
                    return Err(CoreError::failed(format!(
                        "slot {player}: the core did not reach a safe point after {MAX_SAFE_POINT_PUMPS} pumps: {why} (phase {:?}, seats {seats})",
                        self.phase
                    )));
                }
                self.pump_after_order(false)?;
            }
            if !matches!(self.phase, TurnPhase::Humans) {
                todo.push_front((player, control));
                self.load_controls = Some(todo.into_iter().collect());
                return Ok(false);
            }
            self.core.set_control(player, &control)?;
            kicked = matches!(control, SlotControl::Ai);
        }
    }
}

/// Most pumps `apply_load_controls` waits for a safe point before it gives up.
const MAX_SAFE_POINT_PUMPS: u32 = 50;

/// The civilization of the first human seat, for the name of a save in lists.
fn first_civ(s: &SaveSections) -> String {
    s.seats
        .iter()
        .find(|x| matches!(x.control, SlotControl::Human { .. }))
        .or_else(|| s.seats.first())
        .and_then(|x| match &x.civ {
            CivPick::Civ(name) => Some(name.clone()),
            CivPick::Random => None,
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use protocol::{
        AutoChoices, GameSettings, GameSetup, IPC_VERSION, SaveReason, SlotSetup, Start, TimerMode,
        ToWorker, TurnMode, WorkerConfig, WorkerInit,
    };

    use super::*;
    use crate::fixture::FixtureCore;
    use crate::seats::CityMemo;

    fn init(start: Start) -> WorkerInit {
        WorkerInit {
            ipc: IPC_VERSION,
            game: 1,
            core: protocol::CoreKind::Fixture {
                huge: false,
                wrap_x: false,
            },
            start,
            save_dir: "saves".into(),
            seed: 5,
            cfg: WorkerConfig::default(),
        }
    }

    fn setup() -> GameSetup {
        let slot = |p: i8, control| SlotSetup {
            player: p,
            civ: CivPick::Random,
            control,
        };
        GameSetup {
            settings: GameSettings {
                name: "memo".into(),
                world_size: "WORLDSIZE_DUEL".into(),
                map_script: "fixture.lua".into(),
                map_options: Vec::new(),
                speed: "GAMESPEED_STANDARD".into(),
                start_era: "ERA_ANCIENT".into(),
                handicap: "HANDICAP_PRINCE".into(),
                minor_civs: 0,
                turn_mode: TurnMode::Simultaneous,
                timer: TimerMode::Off,
                game_options: Vec::new(),
                seed: Some(1),
                map_seed: None,
                auto_choices: AutoChoices::default(),
                diplo_auto_decline: true,
                mods: Vec::new(),
            },
            slots: vec![
                slot(0, SlotControl::Human { nick: "a".into() }),
                slot(1, SlotControl::Human { nick: "b".into() }),
                slot(2, SlotControl::Ai),
            ],
        }
    }

    fn jobs(outs: Vec<Out>) -> Vec<SaveJob> {
        outs.into_iter()
            .filter_map(|o| match o {
                Out::Save(j) => Some(*j),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_memory_of_cities_is_saved_and_read_before_the_first_view() {
        let (mut w, _) = Worker::new(FixtureCore::new(false, false), init(Start::New(setup())), 0);
        let mut now = 0;
        while w.wants_step() {
            now += 1;
            w.step(now);
        }
        let team = w.seats[&0].team;
        let memo = CityMemo {
            owner: 1,
            id: 9,
            population: 4,
            hp: 11,
            max_hp: 20,
            flags: protocol::CityFlags::CAPITAL,
            buildings: vec![0, 2],
        };
        w.teams
            .get_mut(&team)
            .expect("team data")
            .memo
            .insert((1, 9), memo.clone());
        let job = jobs(w.handle(
            ToWorker::SaveNow {
                reason: SaveReason::Admin,
            },
            now + 1,
        ))
        .pop()
        .expect("a job");
        assert_eq!(job.sections.city_memo, vec![(team, vec![memo.clone()])]);

        let file = saves::encode(&job.header, &job.gcs1, &job.sections);
        let save = saves::decode(&file).unwrap();
        let (w2, _) = Worker::resume(
            FixtureCore::new(false, false),
            init(Start::Load {
                file: "x".into(),
                seats: setup().slots,
            }),
            save,
            0,
        );
        assert_eq!(w2.teams[&team].memo.get(&(1, 9)), Some(&memo));
    }
}
