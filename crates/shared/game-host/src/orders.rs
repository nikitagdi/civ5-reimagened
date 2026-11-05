//! Orders of the seats (game-host-mp GH-D7, 4.5): the queue, the batch the worker applies in one step and the
//! answer to every order number.
//!
//! An order arrives with `ToWorker::Cmd`. A seat that is no human gets `NotSeated`, one with 64 orders waiting
//! `TooMany`; while the core is busy (the AI moves) the order waits and the seat hears `Queued`. In the human
//! phase `step` applies up to `batch_max_cmds` orders (or until `batch_max_ms` has passed): an order of an
//! earlier turn is `TurnChanged`; an order to a unit or a city after the own "end turn" of the seat first takes
//! the end of turn back (NP-D23, `// re: CvPlayer::CancelActivePlayerEndTurn`); the refusal of the core is
//! translated (`RefusalCode`); an order the core took is `Applied`, after the views that show what it did.
//! Peace between humans is `peace.rs`. After the views the seats that gave an order get their urgent
//! contexts (`context.rs`).

use std::collections::{BTreeMap, VecDeque};
use std::time::Instant;

use protocol::{Cmd, Command, CommandOutcome, PlayerId, RejectCode, StepKind, TurnPhase};

use crate::core::{Core, CoreError, Refusal, RefusalCode};
use crate::worker::{Fb, Worker, micros};

/// Orders a seat may have waiting (game-host-mp 4.5 item 2).
pub(crate) const QUEUE_MAX: usize = 64;

/// What waits in the queue, in the order it came.
#[derive(Debug, Clone)]
pub(crate) enum Item {
    Cmd {
        player: PlayerId,
        cmd: Cmd,
        /// The seat has been told `Queued`.
        announced: bool,
    },
    /// The turn timer ran out for the seat (`ForceEndTurn`): no number, no answer. Stale in another turn.
    Timeout { player: PlayerId, turn: i32 },
}

/// The orders waiting for the core, in arrival order, with a count per seat.
#[derive(Debug, Default)]
pub(crate) struct OrderQueue {
    items: VecDeque<Item>,
    per_seat: BTreeMap<PlayerId, usize>,
}

impl OrderQueue {
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Orders of `player` waiting.
    pub fn count(&self, player: PlayerId) -> usize {
        self.per_seat.get(&player).copied().unwrap_or(0)
    }

    /// Queues an order; `false` when the seat has too many waiting.
    pub fn push_cmd(&mut self, player: PlayerId, cmd: Cmd, announced: bool) -> bool {
        if self.count(player) >= QUEUE_MAX {
            return false;
        }
        *self.per_seat.entry(player).or_default() += 1;
        self.items.push_back(Item::Cmd {
            player,
            cmd,
            announced,
        });
        true
    }

    pub fn push_timeout(&mut self, player: PlayerId, turn: i32) {
        self.items.push_back(Item::Timeout { player, turn });
    }

    pub fn pop_front(&mut self) -> Option<Item> {
        let item = self.items.pop_front()?;
        if let Item::Cmd { player, .. } = &item
            && let Some(n) = self.per_seat.get_mut(player)
        {
            *n = n.saturating_sub(1);
        }
        Some(item)
    }

    /// Puts an order back at the front (the core was busy).
    pub fn push_front(&mut self, item: Item) {
        if let Item::Cmd { player, .. } = &item {
            *self.per_seat.entry(*player).or_default() += 1;
        }
        self.items.push_front(item);
    }

    /// Marks the waiting orders as announced and returns `(player, id)` of those that were not.
    pub fn announce_all(&mut self) -> Vec<(PlayerId, protocol::CmdId)> {
        let mut out = Vec::new();
        for item in &mut self.items {
            if let Item::Cmd {
                player,
                cmd,
                announced,
            } = item
                && !*announced
            {
                *announced = true;
                out.push((*player, cmd.id));
            }
        }
        out
    }
}

/// The order is for a unit or a city: it takes back the end of turn of the seat first.
fn is_unit_or_city_order(c: &Command) -> bool {
    matches!(
        c,
        Command::UnitMoveTo { .. }
            | Command::UnitRangeAttack { .. }
            | Command::UnitFoundCity { .. }
            | Command::UnitFortify { .. }
            | Command::UnitSleep { .. }
            | Command::UnitSkip { .. }
            | Command::UnitHeal { .. }
            | Command::UnitAutomateBuild { .. }
            | Command::UnitAutomateExplore { .. }
            | Command::UnitCancel { .. }
            | Command::UnitDisband { .. }
            | Command::UnitWake { .. }
            | Command::CitySetProduction { .. }
            | Command::CityPurchase { .. }
            | Command::CityRangeAttack { .. }
    )
}

/// The reject code of a refusal that is not "the core is busy".
fn reject_code(r: &Refusal) -> RejectCode {
    match r.code {
        RefusalCode::NotHuman => RejectCode::NotSeated,
        RefusalCode::NotYourTurn => RejectCode::NotYourTurn,
        RefusalCode::TurnEnded => RejectCode::TurnEnded,
        RefusalCode::AiRunning | RefusalCode::PumpFirst | RefusalCode::Other => RejectCode::Core,
    }
}

/// Test tooling: `CIV5_DEV_AI_PHASE_DELAY_MS` makes the core call that ends the human phase that much longer (the
/// worker sleeps in it, as in a late game whose AI turns take 10-20 s in one call), so a playtest sees the end of a
/// turn of a big game on a small map (`m5-turn-flow`). Unset (every real server): no delay.
fn dev_ai_phase_delay() {
    static MS: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let ms = *MS.get_or_init(|| {
        std::env::var("CIV5_DEV_AI_PHASE_DELAY_MS")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0)
    });
    if ms > 0 {
        tracing::info!("dev: the end of the human phase is held {ms} ms");
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }
}

/// Most pumps an order refused with `PUMP_FIRST` waits for before it is refused for good. A pump that stops
/// because another human is up runs one slice, and a move holds its unit busy for one slice per plot it went
/// (`CvUnitMission::CalculateMissionTimer`), so a long move of a fast unit needs a dozen.
pub(crate) const MAX_SETTLE_PUMPS: u32 = 32;

/// How one order ended in a batch.
enum Done {
    /// The core changed (an order taken, an end of turn taken back).
    Changed,
    /// Nothing changed.
    Same,
    /// The core is busy: the order went back to the queue and the batch ends.
    Busy,
}

impl<C: Core> Worker<C> {
    /// `ToWorker::Cmd`: the order is answered at once, or waits for the next step.
    pub(crate) fn accept_cmd(&mut self, player: PlayerId, cmd: Cmd, fb: &mut Fb) {
        let refuse = match self.phase {
            TurnPhase::Starting => Some((RejectCode::NoGame, "the game is starting")),
            TurnPhase::GameOver { .. } => Some((RejectCode::NoGame, "the game is over")),
            _ if !self.seats.contains_key(&player) => {
                Some((RejectCode::NotSeated, "the player has no seat"))
            }
            _ if self.queue.count(player) >= QUEUE_MAX => {
                Some((RejectCode::TooMany, "too many orders are waiting"))
            }
            _ => None,
        };
        if let Some((code, text)) = refuse {
            fb.reject(player, cmd.id, code, text);
            return;
        }
        self.tl_cmd_accepted(player, cmd.id, &cmd.cmd);
        // While the core is busy the seat hears `Queued` at once.
        let busy = self.phase != TurnPhase::Humans;
        self.queue.push_cmd(player, cmd, busy);
        if busy {
            fb.result(player, cmd.id, CommandOutcome::Queued);
        }
    }

    /// One batch of the human phase (game-host-mp 4.5).
    pub(crate) fn run_batch(&mut self, fb: &mut Fb) -> Result<(), CoreError> {
        fb.kind = Some(StepKind::Orders);
        self.ordered.clear();
        let t = Instant::now();
        let mut changed = false;
        let mut n = 0u32;
        while n < self.cfg.batch_max_cmds.max(1)
            && self.phase == TurnPhase::Humans
            && !self.pump_due
        {
            if n > 0 && t.elapsed().as_millis() >= u128::from(self.cfg.batch_max_ms) {
                break;
            }
            let Some(item) = self.queue.pop_front() else {
                break;
            };
            n += 1;
            let done = match item {
                Item::Timeout { player, turn } => self.apply_timeout(player, turn)?,
                Item::Cmd {
                    player,
                    cmd,
                    announced,
                } => self.apply_cmd(player, cmd, announced, fb)?,
            };
            match done {
                Done::Changed => {
                    changed = true;
                    self.query_noise();
                }
                Done::Same => {}
                Done::Busy => break,
            }
        }
        // What is left waits for the next human phase; its seats hear `Queued` once.
        if self.phase != TurnPhase::Humans {
            for (player, id) in self.queue.announce_all() {
                fb.result(player, id, CommandOutcome::Queued);
            }
        }
        self.times.apply_us = micros(t);
        if changed {
            self.build_round(fb)?;
            if self.phase == TurnPhase::Humans {
                self.ctx_urgent(fb)?;
            }
        }
        self.ordered.clear();
        self.ai_step_kind(fb);
        self.refresh_turn(fb)
    }

    /// An end of turn was applied with the replica stream on: the checked batch that holds it is made now
    /// (`replica_before_pump`) and goes out with this step; the pump waits for the next step (`pump_due`). The pump
    /// after the last human's end of turn is the AI phase, one core call of 10-20 s in a late game, and the worker
    /// sends nothing while it runs: the clients did not learn that every human had ended the turn (the last one's
    /// "Waiting for players" stayed clickable and the others listed him as not done), and what they clicked meanwhile
    /// came back refused, flipping the END TURN button (live report 2026-10-31). The exe sends NetTurnComplete to
    /// every machine before any of them runs the turn. `false`: no replica batch, pump now as before.
    fn defer_end_turn_pump(&mut self) -> Result<bool, CoreError> {
        if self.core.replica().is_none() {
            return Ok(false);
        }
        self.replica_before_pump(true)?;
        if !self.rep.has_out() {
            return Ok(false);
        }
        self.pump_due = true;
        Ok(true)
    }

    /// The pump an end of turn left for this step ([`Self::defer_end_turn_pump`]); then what `run_batch` does after a
    /// changed batch: the views, the urgent contexts, the turn state.
    pub(crate) fn deferred_pump(&mut self, fb: &mut Fb) -> Result<(), CoreError> {
        fb.kind = Some(StepKind::Orders);
        self.pump_due = false;
        self.ordered.clear();
        let t = Instant::now();
        self.pump_after_order(true)?;
        if self.phase != TurnPhase::Humans {
            for (player, id) in self.queue.announce_all() {
                fb.result(player, id, CommandOutcome::Queued);
            }
        }
        self.times.apply_us = micros(t);
        self.build_round(fb)?;
        if self.phase == TurnPhase::Humans {
            self.ctx_urgent(fb)?;
        }
        self.ai_step_kind(fb);
        self.refresh_turn(fb)
    }

    /// A step whose pump began the AI phase (the AI phase runs inside the step of the last END_TURN) is an AI step for
    /// the statistics when the phase goes on: `ai_us` holds its time and the step that gives the turn back to the
    /// humans closes the sample (`AiPhaseAcc`). A phase that fit into one pump stays an order step.
    fn ai_step_kind(&mut self, fb: &mut Fb) {
        if self.tl_ai_running() && matches!(self.phase, TurnPhase::Ai { .. }) {
            fb.kind = Some(StepKind::AiPhase);
        }
    }

    /// The core took an order of `player`: its contexts are the urgent ones (`city`: the city the order was
    /// for, if it was a city order).
    pub(crate) fn note_order(&mut self, player: PlayerId, cmd: &Command) {
        self.last_order_seat = Some(player);
        let city = match *cmd {
            Command::CitySetProduction { city, .. }
            | Command::CityPurchase { city, .. }
            | Command::CityRangeAttack { city, .. } => Some(city),
            _ => None,
        };
        let entry = self.ordered.entry(player).or_insert(None);
        if city.is_some() {
            *entry = city;
        }
    }

    /// The core took something: it runs on until the humans or the AI are to move, the phase follows.
    ///
    /// `ends_turn`: the order was an end of turn, which may end the human phase (the replica stream sends its
    /// last, checked batch before the core runs on).
    pub(crate) fn pump_after_order(&mut self, ends_turn: bool) -> Result<(), CoreError> {
        self.replica_before_pump(ends_turn)?;
        let before = self.turn;
        // The pump that follows an END_TURN is the AI phase: `pump_core` books its time into `ai_us` (4.17) whether
        // the phase fits into the call or goes on, and pumps it in small slices.
        let (p, us) = self.pump_core(ends_turn)?;
        if ends_turn && (p.turn != before || !matches!(p.phase, crate::core::CorePhase::Humans)) {
            dev_ai_phase_delay();
        }
        self.times.pump_us += us;
        self.apply_pump(p);
        self.collect_events();
        // The AI phase fit into this pump (the real core can do that): the humans of a new turn are up, and
        // that is the start of the turn for the saves and for the digest of the turn end.
        if self.phase == TurnPhase::Humans && self.turn != before {
            self.save_at_turn_start();
            if self.cfg.digests {
                self.times.digest = self.core.digest(-1).ok();
            }
        }
        Ok(())
    }

    fn apply_timeout(&mut self, player: PlayerId, turn: i32) -> Result<Done, CoreError> {
        if turn != self.turn || !self.seats.contains_key(&player) {
            return Ok(Done::Same);
        }
        let t0 = self.tl_now();
        let r = self.core.end_turn_timeout(player);
        self.collect_events();
        match r {
            Ok(()) => {
                // The timer's end of turn has no order number: 0.
                self.tl_end_turn_applied(player, 0, t0);
                self.note_applied();
                self.pump_after_order(true)?;
                Ok(Done::Changed)
            }
            Err(r) => {
                tracing::info!("end of turn by timer of seat {player} refused: {}", r.text);
                Ok(Done::Same)
            }
        }
    }

    fn apply_cmd(
        &mut self,
        player: PlayerId,
        cmd: Cmd,
        announced: bool,
        fb: &mut Fb,
    ) -> Result<Done, CoreError> {
        let id = cmd.id;
        // An order queued while the AI moved is for the turn that follows: the client names that turn.
        if cmd.turn != self.turn {
            let text = format!(
                "the order is for turn {}, the turn is {}",
                cmd.turn, self.turn
            );
            fb.reject(player, id, RejectCode::TurnChanged, &text);
            return Ok(Done::Same);
        }
        let Some(seat) = self.seats.get(&player) else {
            fb.reject(player, id, RejectCode::NotSeated, "the player has no seat");
            return Ok(Done::Same);
        };
        let mut changed = false;
        if seat.finished && is_unit_or_city_order(&cmd.cmd) {
            // re: CvPlayer::CancelActivePlayerEndTurn (Lekmod CvPlayer.cpp:31713)
            let r = self.core.command(player, &Command::CancelEndTurn);
            self.collect_events();
            match r {
                Ok(()) => {
                    if let Some(seat) = self.seats.get_mut(&player) {
                        seat.finished = false;
                    }
                    changed = true;
                }
                Err(r) => {
                    fb.reject(player, id, reject_code(&r), &r.text);
                    return Ok(Done::Same);
                }
            }
        }
        if matches!(
            cmd.cmd,
            Command::ProposePeace { .. } | Command::AnswerPeace { .. }
        ) {
            // Peace between humans is the job of the worker itself (GH-D13, 4.9).
            let done = self.peace_cmd(player, id, &cmd.cmd, fb)?;
            return Ok(if done || changed {
                Done::Changed
            } else {
                Done::Same
            });
        }
        let apply_t0 = self.tl_now();
        let mut r = self.core.command(player, &cmd.cmd);
        self.collect_events();
        // Live game 931 (2026-10-30): an end of turn right after a move was refused with "a unit / city is busy;
        // pump first" and nothing pumped any more (the humans' phase waits for orders), so the turn hung. The
        // core asks for a pump: the worker pumps and gives the order again, as the exe's frame loop would.
        let mut settle = 0;
        while matches!(&r, Err(e) if e.code == RefusalCode::PumpFirst)
            && settle < MAX_SETTLE_PUMPS
            && self.phase == TurnPhase::Humans
        {
            settle += 1;
            self.pump_after_order(false)?;
            changed = true;
            if self.phase != TurnPhase::Humans {
                break;
            }
            r = self.core.command(player, &cmd.cmd);
            self.collect_events();
        }
        if settle > 0 {
            tracing::info!(
                "turn {}: an order of seat {player} waited {settle} pump(s) for a busy unit or city: {}",
                self.turn,
                if r.is_ok() { "taken" } else { "still refused" }
            );
        }
        let same = if changed { Done::Changed } else { Done::Same };
        match r {
            Ok(()) => {
                fb.result(player, id, CommandOutcome::Applied);
                if let Some(seat) = self.seats.get_mut(&player) {
                    seat.applied_upto = Some(id);
                }
                self.note_applied();
                self.note_order(player, &cmd.cmd);
                let ends_turn = matches!(cmd.cmd, Command::EndTurn { .. });
                if ends_turn {
                    self.tl_end_turn_applied(player, id, apply_t0);
                }
                if !(ends_turn && self.defer_end_turn_pump()?) {
                    self.pump_after_order(ends_turn)?;
                }
                Ok(Done::Changed)
            }
            Err(_) if self.phase != TurnPhase::Humans => {
                // The settling pumps left the human phase: the order waits for the next one.
                self.queue.push_front(Item::Cmd {
                    player,
                    cmd,
                    announced,
                });
                Ok(Done::Changed)
            }
            Err(r) if r.code == RefusalCode::AiRunning => {
                // The core is busy: find out where it stands, the order waits for the human phase.
                self.pump_after_order(false)?;
                if self.phase == TurnPhase::Humans {
                    fb.reject(player, id, RejectCode::Core, &r.text);
                    return Ok(same);
                }
                self.queue.push_front(Item::Cmd {
                    player,
                    cmd,
                    announced,
                });
                Ok(if changed { Done::Changed } else { Done::Busy })
            }
            Err(r) => {
                if matches!(cmd.cmd, Command::EndTurn { .. }) {
                    // A refused end of turn holds the seat in its turn: the core's text names the blocker (NP28).
                    tracing::info!(
                        "turn {}: the end of turn of seat {player} was refused: {}",
                        self.turn,
                        r.text
                    );
                }
                // The end of turn stays taken back even if the core refuses the order itself.
                fb.reject(player, id, reject_code(&r), &r.text);
                Ok(same)
            }
        }
    }
}
