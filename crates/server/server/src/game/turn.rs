//! `TurnState` of a game: built from the worker's `TurnInfo`, the timers of
//! [`super::timer`] and the pause of the host. The game actor sends it to every human seat whenever
//! it changes.

use protocol::{SeatTurn, TurnInfo, TurnState};

use super::timer::{Timers, seat_groups};

/// `TurnState` of `info` with the running timers and the pause.
pub fn turn_state(info: &TurnInfo, timers: &Timers) -> TurnState {
    TurnState {
        turn: info.turn,
        phase: info.phase,
        mode: info.mode,
        seats: info
            .seats
            .iter()
            .zip(seat_groups(info))
            .map(|(s, (_, group))| SeatTurn {
                player: s.player,
                finished: s.finished,
                may_act: s.may_act,
                group,
            })
            .collect(),
        timers: timers.states(),
        paused: timers.paused(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{SeatTurnInfo, TimerMode, TurnMode, TurnPhase};

    #[test]
    fn the_state_has_the_groups_the_timers_and_the_pause() {
        let info = TurnInfo {
            turn: 4,
            phase: TurnPhase::Humans,
            mode: TurnMode::Hybrid,
            seats: vec![
                SeatTurnInfo {
                    player: 0,
                    finished: false,
                    may_act: true,
                    simultaneous: true,
                },
                SeatTurnInfo {
                    player: 2,
                    finished: false,
                    may_act: false,
                    simultaneous: false,
                },
            ],
            turn_len_s: 0,
            timer_base_s: 0,
            human_wars: vec![],
        };
        let mut timers = Timers::new(TimerMode::Fixed { seconds: 30 });
        timers.update(&info, &|_| true, 1_000);
        timers.set_paused(true, &|_| true, 2_000);
        let ts = turn_state(&info, &timers);
        assert_eq!((ts.turn, ts.phase, ts.mode), (4, info.phase, info.mode));
        assert_eq!(
            ts.seats
                .iter()
                .map(|s| (s.player, s.group))
                .collect::<Vec<_>>(),
            [(0, 0), (2, 1)]
        );
        assert!(ts.paused);
        assert_eq!(ts.timers.len(), 1);
        assert_eq!(ts.timers[0].len_ms, 30_000);
        assert_eq!(ts.timers[0].left_ms_when_stopped, Some(29_000));
        // No timer, no pause: the old, empty state.
        let ts = turn_state(&info, &Timers::new(TimerMode::Off));
        assert!(ts.timers.is_empty() && !ts.paused);
    }
}
