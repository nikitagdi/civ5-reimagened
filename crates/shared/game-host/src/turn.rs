//! `TurnInfo` of the worker (game-host-mp 4.7): the turn, the phase, the mode, who has finished and who may
//! act, the timer data of the core. The gateway runs the timers and builds `TurnState` from it.
//!
//! `human_wars` is what the core reports, as it is: the core names only the wars that extend the timer (the
//! protocol `HumanWar` has no `extends_timer`).

use protocol::{TurnInfo, TurnPhase};

use crate::core::{Core, CoreError};
use crate::worker::{Fb, Worker};

impl<C: Core> Worker<C> {
    /// Asks the core for the turn state and puts a `Turn` into the frame when it differs from the last one
    /// sent, or when humans declared war on humans.
    pub(crate) fn refresh_turn(&mut self, fb: &mut Fb) -> Result<(), CoreError> {
        let info = if matches!(self.phase, TurnPhase::Starting) {
            TurnInfo {
                turn: self.turn,
                phase: self.phase,
                mode: self.mode,
                seats: self.starting_seats(),
                turn_len_s: 0,
                timer_base_s: 0,
                human_wars: Vec::new(),
            }
        } else {
            let ct = self.core.turn()?;
            TurnInfo {
                turn: self.turn,
                phase: self.phase,
                mode: ct.mode,
                seats: ct.seats,
                turn_len_s: ct.turn_len_s,
                timer_base_s: ct.timer_base_s,
                human_wars: ct.human_wars,
            }
        };
        for s in &info.seats {
            if let Some(seat) = self.seats.get_mut(&s.player) {
                seat.finished = s.finished;
            }
        }
        let mut stored = info.clone();
        stored.human_wars.clear();
        let changed = self.last_turn.as_ref() != Some(&stored) || !info.human_wars.is_empty();
        if changed {
            self.last_turn = Some(stored);
            fb.turn = Some(info);
        }
        Ok(())
    }
}
