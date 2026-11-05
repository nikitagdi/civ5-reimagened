//! The turn timer of a game. Pure rules: the clock is an argument
//! (`Clock` milliseconds of the gateway), the actor feeds it every `TurnInfo`, every change of a
//! connection and every pause, and asks it what has run out.
//!
//! One timer per **group** (net-protocol 4.10): group 0 are the seats that move at once, every
//! sequential seat of hybrid mode is a group of its own whose timer starts when the seat may act. A
//! group lives while one of its seats `may_act` and is not `finished` in the human phase. Its clock
//! **stops** while the host pauses or a seat it waits for is not `Online` (N-D7), keeping what was
//! left. When it runs out the actor sends `ForceEndTurn` for the seats of the group; if the turn does
//! not show them finished in 5 s the request is repeated, up to three times.
//!
//! `Original { row }`: the length of the turn and the base time of the row come with `TurnInfo`
//! (`turn_len_s`, `timer_base_s`); 0 means no timer this turn. `Fixed { seconds }` has no base and no
//! extension. A war of one human on another extends the running timers by the Lekmod rule
//! (`CvTeam.cpp:1416-1471`): if the time left of the declarer's group is less than the base time,
//! every timer of the game is moved by the difference.

use std::collections::BTreeMap;

use protocol::{PlayerId, TimerMode, TimerState, TurnInfo, TurnPhase};

/// A request for `ForceEndTurn` that the turn does not obey is repeated after this long.
pub const RETRY_AFTER_MS: u64 = 5_000;
/// How many times `ForceEndTurn` is sent for one group before the gateway gives up.
pub const MAX_FORCE: u8 = 3;

/// Timer group of every seat of a `TurnInfo` (net-protocol 4.10): simultaneous seats are group 0,
/// every other seat gets the next number, in the order of the list.
pub fn seat_groups(info: &TurnInfo) -> Vec<(PlayerId, u8)> {
    let mut next = 1u8;
    info.seats
        .iter()
        .map(|s| {
            let group = if s.simultaneous {
                0
            } else {
                let g = next;
                next = next.saturating_add(1);
                g
            };
            (s.player, group)
        })
        .collect()
}

/// What `update` did, for the counters.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Update {
    /// Timers started.
    pub started: u32,
    /// Wars that moved the timers.
    pub extended: u32,
}

/// What `poll` found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// The timer of a group ran out: `ForceEndTurn` for these seats. `attempt` counts from 1.
    Force {
        group: u8,
        players: Vec<PlayerId>,
        attempt: u8,
    },
    /// Three requests did not end the turn for the group.
    GaveUp { group: u8, players: Vec<PlayerId> },
}

#[derive(Debug)]
struct Group {
    len_ms: u32,
    /// Gateway time at which it runs out; stale while the clock is stopped.
    deadline_ms: u64,
    /// `Some`: the clock is stopped and this is what was left.
    left_when_stopped: Option<u64>,
    /// Seats the group waits for: `may_act && !finished`.
    acting: Vec<PlayerId>,
    /// `ForceEndTurn` sent so far, and when last.
    forced: u8,
    last_force_ms: u64,
    gave_up: bool,
}

impl Group {
    fn left(&self, now: u64) -> u64 {
        self.left_when_stopped
            .unwrap_or_else(|| self.deadline_ms.saturating_sub(now))
    }
}

/// The timers of one game.
#[derive(Debug)]
pub struct Timers {
    mode: TimerMode,
    paused: bool,
    turn: Option<i32>,
    groups: BTreeMap<u8, Group>,
}

impl Timers {
    pub fn new(mode: TimerMode) -> Self {
        Self {
            mode,
            paused: false,
            turn: None,
            groups: BTreeMap::new(),
        }
    }

    /// Forgets the running timers (the worker restarted); the pause stays.
    pub fn reset(&mut self) {
        self.turn = None;
        self.groups.clear();
    }

    pub fn paused(&self) -> bool {
        self.paused
    }

    /// Some timer is running or waiting: the actor looks at the clock.
    pub fn active(&self) -> bool {
        !self.groups.is_empty()
    }

    /// A new `TurnInfo` of the worker: groups start and end, the war rule applies.
    pub fn update(
        &mut self,
        info: &TurnInfo,
        online: &dyn Fn(PlayerId) -> bool,
        now: u64,
    ) -> Update {
        let mut out = Update::default();
        if self.turn != Some(info.turn) {
            self.groups.clear();
            self.turn = Some(info.turn);
        }
        let len_ms = match &self.mode {
            TimerMode::Off => 0,
            TimerMode::Original { .. } => info.turn_len_s.saturating_mul(1000),
            TimerMode::Fixed { seconds } => u32::from(*seconds) * 1000,
        };
        if info.phase != TurnPhase::Humans || len_ms == 0 {
            self.groups.clear();
            return out;
        }
        let mut alive: BTreeMap<u8, Vec<PlayerId>> = BTreeMap::new();
        let by_seat: BTreeMap<PlayerId, u8> = seat_groups(info).into_iter().collect();
        for s in &info.seats {
            if s.may_act && !s.finished {
                alive.entry(by_seat[&s.player]).or_default().push(s.player);
            }
        }
        self.groups.retain(|g, _| alive.contains_key(g));
        for (g, acting) in alive {
            match self.groups.get_mut(&g) {
                Some(group) => group.acting = acting,
                None => {
                    self.groups.insert(
                        g,
                        Group {
                            len_ms,
                            deadline_ms: now + u64::from(len_ms),
                            left_when_stopped: None,
                            acting,
                            forced: 0,
                            last_force_ms: 0,
                            gave_up: false,
                        },
                    );
                    out.started += 1;
                }
            }
        }
        self.reconcile(online, now);
        // Wars of humans on humans (the worker lists only those the rule extends for).
        if matches!(self.mode, TimerMode::Original { .. }) && info.turn != 0 {
            for war in &info.human_wars {
                if let Some(g) = by_seat.get(&war.from)
                    && self.extend(*g, u64::from(info.timer_base_s) * 1000, now)
                {
                    out.extended += 1;
                }
            }
        }
        out
    }

    /// Lekmod: when the time left of the declarer's group is less than the base time, every timer
    /// moves by the difference. `false`: nothing moved.
    fn extend(&mut self, declarer_group: u8, base_ms: u64, now: u64) -> bool {
        let Some(g) = self.groups.get(&declarer_group) else {
            return false;
        };
        let left = g.left(now);
        if base_ms == 0 || left >= base_ms {
            return false;
        }
        let add = base_ms - left;
        for g in self.groups.values_mut() {
            match &mut g.left_when_stopped {
                Some(l) => *l += add,
                None => g.deadline_ms += add,
            }
        }
        true
    }

    /// The host pauses or resumes the timer. `true`: the state changed.
    pub fn set_paused(
        &mut self,
        paused: bool,
        online: &dyn Fn(PlayerId) -> bool,
        now: u64,
    ) -> bool {
        if self.paused == paused {
            return false;
        }
        self.paused = paused;
        self.reconcile(online, now);
        true
    }

    /// Stops the clocks that must stand and restarts those that may run (a seat went or came back,
    /// the pause changed). Returns `true` if a clock stopped or started.
    pub fn reconcile(&mut self, online: &dyn Fn(PlayerId) -> bool, now: u64) -> bool {
        let mut changed = false;
        for g in self.groups.values_mut() {
            let stop = self.paused || g.acting.iter().any(|p| !online(*p));
            match (stop, g.left_when_stopped) {
                (true, None) => {
                    g.left_when_stopped = Some(g.deadline_ms.saturating_sub(now));
                    changed = true;
                }
                (false, Some(left)) => {
                    g.deadline_ms = now + left;
                    g.left_when_stopped = None;
                    changed = true;
                }
                _ => {}
            }
        }
        changed
    }

    /// What has run out by `now`.
    pub fn poll(&mut self, now: u64) -> Vec<Action> {
        let mut out = Vec::new();
        for (group, g) in &mut self.groups {
            if g.left_when_stopped.is_some() || g.gave_up {
                continue;
            }
            let due = if g.forced == 0 {
                now >= g.deadline_ms
            } else {
                now.saturating_sub(g.last_force_ms) >= RETRY_AFTER_MS
            };
            if !due {
                continue;
            }
            if g.forced >= MAX_FORCE {
                g.gave_up = true;
                out.push(Action::GaveUp {
                    group: *group,
                    players: g.acting.clone(),
                });
            } else {
                g.forced += 1;
                g.last_force_ms = now;
                out.push(Action::Force {
                    group: *group,
                    players: g.acting.clone(),
                    attempt: g.forced,
                });
            }
        }
        out
    }

    /// `TurnState.timers`: one entry per running group.
    pub fn states(&self) -> Vec<TimerState> {
        self.groups
            .iter()
            .map(|(group, g)| TimerState {
                group: *group,
                deadline_ms: g.deadline_ms,
                len_ms: g.len_ms,
                left_ms_when_stopped: g
                    .left_when_stopped
                    .map(|l| u32::try_from(l).unwrap_or(u32::MAX)),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{HumanWar, SeatTurnInfo, TurnMode};

    fn seat(player: PlayerId, simultaneous: bool, may_act: bool, finished: bool) -> SeatTurnInfo {
        SeatTurnInfo {
            player,
            finished,
            may_act,
            simultaneous,
        }
    }

    fn info(turn: i32, seats: Vec<SeatTurnInfo>) -> TurnInfo {
        TurnInfo {
            turn,
            phase: TurnPhase::Humans,
            mode: TurnMode::Hybrid,
            seats,
            turn_len_s: 60,
            timer_base_s: 20,
            human_wars: vec![],
        }
    }

    fn fixed(seconds: u16) -> Timers {
        Timers::new(TimerMode::Fixed { seconds })
    }

    fn original() -> Timers {
        Timers::new(TimerMode::Original {
            row: "TURNTIMER_FAST".into(),
        })
    }

    fn everybody(_: PlayerId) -> bool {
        true
    }

    fn deadline(t: &Timers, group: u8) -> u64 {
        t.states()
            .iter()
            .find(|s| s.group == group)
            .expect("the group runs")
            .deadline_ms
    }

    #[test]
    fn groups_are_numbered_by_the_list() {
        let i = info(
            1,
            vec![
                seat(0, true, true, false),
                seat(1, false, true, false),
                seat(2, true, true, false),
                seat(3, false, false, false),
            ],
        );
        assert_eq!(seat_groups(&i), [(0, 0), (1, 1), (2, 0), (3, 2)]);
    }

    #[test]
    fn off_and_a_zero_length_have_no_timer() {
        let i = info(1, vec![seat(0, true, true, false)]);
        let mut t = Timers::new(TimerMode::Off);
        assert_eq!(t.update(&i, &everybody, 0), Update::default());
        assert!(!t.active());
        // `Original` with a zero length: no timer in this turn.
        let mut zero = info(1, vec![seat(0, true, true, false)]);
        zero.turn_len_s = 0;
        let mut t = original();
        t.update(&zero, &everybody, 0);
        assert!(!t.active());
        assert!(t.poll(1_000_000).is_empty());
    }

    #[test]
    fn a_timer_starts_with_the_turn_and_runs_out_for_the_seats_of_its_group() {
        let mut t = fixed(30);
        let i = info(
            1,
            vec![seat(0, true, true, false), seat(1, true, true, false)],
        );
        let u = t.update(&i, &everybody, 1_000);
        assert_eq!(u.started, 1);
        assert_eq!(deadline(&t, 0), 31_000);
        assert!(t.poll(30_999).is_empty());
        // Seat 0 finished before the end: only seat 1 is forced.
        let i = info(
            1,
            vec![seat(0, true, true, true), seat(1, true, true, false)],
        );
        assert_eq!(t.update(&i, &everybody, 20_000).started, 0);
        assert_eq!(
            t.poll(31_000),
            [Action::Force {
                group: 0,
                players: vec![1],
                attempt: 1
            }]
        );
    }

    #[test]
    fn force_is_repeated_after_five_seconds_up_to_three_times_then_given_up() {
        let mut t = fixed(10);
        let i = info(1, vec![seat(0, true, true, false)]);
        t.update(&i, &everybody, 0);
        let force = |attempt| Action::Force {
            group: 0,
            players: vec![0],
            attempt,
        };
        assert_eq!(t.poll(10_000), [force(1)]);
        assert!(t.poll(14_999).is_empty());
        assert_eq!(t.poll(15_000), [force(2)]);
        assert_eq!(t.poll(20_000), [force(3)]);
        assert_eq!(
            t.poll(25_000),
            [Action::GaveUp {
                group: 0,
                players: vec![0]
            }]
        );
        assert!(t.poll(60_000).is_empty());
        // The turn moves on: a new group, a new timer.
        let i = info(2, vec![seat(0, true, true, false)]);
        assert_eq!(t.update(&i, &everybody, 61_000).started, 1);
        assert_eq!(deadline(&t, 0), 71_000);
    }

    #[test]
    fn the_end_of_the_phase_of_the_turn_or_of_the_group_ends_the_timer() {
        let mut t = fixed(10);
        t.update(&info(1, vec![seat(0, true, true, false)]), &everybody, 0);
        assert!(t.active());
        // Everybody finished: no group.
        t.update(&info(1, vec![seat(0, true, true, true)]), &everybody, 1_000);
        assert!(!t.active());
        t.update(
            &info(2, vec![seat(0, true, true, false)]),
            &everybody,
            2_000,
        );
        assert!(t.active());
        // The AI phase.
        let mut ai = info(2, vec![seat(0, true, false, true)]);
        ai.phase = TurnPhase::Ai { active: 3 };
        t.update(&ai, &everybody, 3_000);
        assert!(!t.active());
        // A new turn starts anew, even if the group number is the same.
        t.update(
            &info(3, vec![seat(0, true, true, false)]),
            &everybody,
            4_000,
        );
        assert_eq!(deadline(&t, 0), 14_000);
    }

    #[test]
    fn the_pause_and_a_seat_that_is_not_online_stop_the_clock() {
        let mut t = fixed(60);
        let i = info(
            1,
            vec![seat(0, true, true, false), seat(1, true, true, false)],
        );
        t.update(&i, &everybody, 0);
        // The host pauses at 10 s: 50 s are left.
        assert!(t.set_paused(true, &everybody, 10_000));
        assert!(!t.set_paused(true, &everybody, 10_500));
        let s = t.states();
        assert_eq!(s[0].left_ms_when_stopped, Some(50_000));
        assert!(t.poll(500_000).is_empty());
        // Resumed at 100 s: the deadline is 150 s.
        t.set_paused(false, &everybody, 100_000);
        assert_eq!(deadline(&t, 0), 150_000);
        assert_eq!(t.states()[0].left_ms_when_stopped, None);
        // Seat 1 drops at 110 s (40 s left), comes back at 400 s.
        let seat1_gone = |p: PlayerId| p != 1;
        assert!(t.reconcile(&seat1_gone, 110_000));
        assert_eq!(t.states()[0].left_ms_when_stopped, Some(40_000));
        assert!(t.poll(399_000).is_empty());
        assert!(t.reconcile(&everybody, 400_000));
        assert_eq!(deadline(&t, 0), 440_000);
        // A pause during an absence does not start the clock when the seat returns.
        t.reconcile(&seat1_gone, 410_000);
        t.set_paused(true, &seat1_gone, 411_000);
        t.reconcile(&everybody, 412_000);
        assert_eq!(t.states()[0].left_ms_when_stopped, Some(30_000));
    }

    #[test]
    fn hybrid_groups_run_in_parallel_and_each_new_seat_starts_its_timer_anew() {
        let mut t = fixed(60);
        // Seat 0 moves at once, seat 1 and seat 2 one after the other.
        let seats = |a1, f1, a2| {
            vec![
                seat(0, true, true, false),
                seat(1, false, a1, f1),
                seat(2, false, a2, false),
            ]
        };
        let u = t.update(&info(1, seats(true, false, false)), &everybody, 0);
        assert_eq!(u.started, 2);
        assert_eq!((deadline(&t, 0), deadline(&t, 1)), (60_000, 60_000));
        // Seat 1 finished at 25 s: its group is gone, seat 2 (group 2) starts a fresh timer.
        let u = t.update(&info(1, seats(false, true, true)), &everybody, 25_000);
        assert_eq!(u.started, 1);
        let states = t.states();
        assert_eq!(
            states.iter().map(|s| s.group).collect::<Vec<_>>(),
            [0, 2],
            "groups 0 and 2 run"
        );
        assert_eq!((deadline(&t, 0), deadline(&t, 2)), (60_000, 85_000));
        // Group 0 runs out first: only its seat is forced.
        assert_eq!(
            t.poll(60_000),
            [Action::Force {
                group: 0,
                players: vec![0],
                attempt: 1
            }]
        );
        // Group 2 later.
        assert_eq!(
            t.poll(85_000)
                .into_iter()
                .filter(|a| matches!(a, Action::Force { group: 2, .. }))
                .count(),
            1
        );
    }

    fn war(from: PlayerId, to: PlayerId) -> HumanWar {
        HumanWar { from, to }
    }

    #[test]
    fn a_war_of_humans_moves_every_timer_by_the_missing_part_of_the_base_time() {
        let mut t = original();
        let seats = vec![seat(0, true, true, false), seat(1, false, true, false)];
        t.update(&info(5, seats.clone()), &everybody, 0);
        // Both groups run to 60 s. At 50 s player 0 declares war: 10 s are left, the base is 20 s:
        // everything moves by 10 s.
        let mut w = info(5, seats.clone());
        w.human_wars = vec![war(0, 1)];
        let u = t.update(&w, &everybody, 50_000);
        assert_eq!(u.extended, 1);
        assert_eq!((deadline(&t, 0), deadline(&t, 1)), (70_000, 70_000));
        // More than the base left: nothing moves (at 40 s there are 30 s left).
        let mut w = info(5, seats.clone());
        w.human_wars = vec![war(0, 1)];
        let u = t.update(&w, &everybody, 40_000);
        assert_eq!(u.extended, 0);
        assert_eq!(deadline(&t, 0), 70_000);
        // The group of the declarer decides, not the others: group 1 (declarer, 10 s left at 60 s
        // after the first move) is short, group 0 has the same deadline; the move is for both.
        let mut w = info(5, seats);
        w.human_wars = vec![war(1, 0)];
        assert_eq!(t.update(&w, &everybody, 60_000).extended, 1);
        assert_eq!(deadline(&t, 0), 80_000);
    }

    #[test]
    fn the_war_rule_uses_the_time_left_when_the_clock_stands() {
        let mut t = original();
        let seats = vec![seat(0, true, true, false)];
        t.update(&info(5, seats.clone()), &everybody, 0);
        t.set_paused(true, &everybody, 45_000); // 15 s left
        let mut w = info(5, seats);
        w.human_wars = vec![war(0, 1)];
        assert_eq!(t.update(&w, &everybody, 90_000).extended, 1);
        let s = t.states();
        assert_eq!(s[0].left_ms_when_stopped, Some(20_000), "moved to the base");
    }

    #[test]
    fn fixed_timers_and_the_turn_zero_get_no_extension() {
        let seats = vec![seat(0, true, true, false)];
        let mut w = info(5, seats.clone());
        w.human_wars = vec![war(0, 1)];
        let mut t = fixed(60);
        t.update(&info(5, seats.clone()), &everybody, 0);
        assert_eq!(t.update(&w, &everybody, 55_000).extended, 0);
        assert_eq!(deadline(&t, 0), 60_000);
        let mut t = original();
        let mut zero = info(0, seats);
        zero.human_wars = vec![war(0, 1)];
        t.update(&zero, &everybody, 0);
        assert_eq!(t.update(&zero, &everybody, 55_000).extended, 0);
    }

    #[test]
    fn a_reset_forgets_the_groups_but_not_the_pause() {
        let mut t = fixed(60);
        t.update(&info(1, vec![seat(0, true, true, false)]), &everybody, 0);
        t.set_paused(true, &everybody, 1);
        t.reset();
        assert!(!t.active());
        assert!(t.paused());
        t.update(
            &info(1, vec![seat(0, true, true, false)]),
            &everybody,
            100_000,
        );
        // Started stopped (the pause stands).
        assert_eq!(t.states()[0].left_ms_when_stopped, Some(60_000));
    }
}
