//! Game input logic of the client: selection, orders, hotkeys.
//! No egui and no GPU: [`PlayController`] maps input and the latest `WorldView` to [`Outputs`].

mod controller;
pub mod highlight_calls;
mod highlights;
pub mod hotkeys;
pub mod selection;

use client_core::OrderSink;
use protocol::{Blocker, BlockerTarget, Command, Query, TeamId, UnitFlags, WorldView};

pub use controller::{Hints, Mode, PATH_DWELL_MS, PlayController};

/// `EndTurnBlockingTypes::ENDTURN_BLOCKING_PRODUCTION` (CvEnums.h).
const BLOCKING_PRODUCTION: i32 = 2;

/// The blocker of the end of turn of `world`. On a view with the optimistic layer (`layered`) a blocker whose
/// target the layer already settled is not one: the unit no longer waits for orders, the city has a
/// production (NC-D13). The server decides in the end; a button pressed too early gets a refusal that does no
/// harm. The `blocker` of the header itself is not predicted (4.4).
pub fn blocker_of(world: &WorldView, layered: bool) -> Option<Blocker> {
    let b = world.blocker?;
    if !layered {
        return Some(b);
    }
    let settled = match b.target {
        BlockerTarget::Unit(id) => world
            .units
            .iter()
            .find(|u| u.id == id && u.owner == world.human)
            .is_some_and(|u| !u.flags.contains(UnitFlags::NEEDS_ORDERS)),
        BlockerTarget::City(id) if b.kind == BLOCKING_PRODUCTION => world
            .cities
            .iter()
            .find(|c| c.id == id && c.owner == world.human)
            .is_some_and(|c| c.production_item >= 0),
        _ => false,
    };
    (!settled).then_some(b)
}

/// What the shell or the UI has to do besides sending protocol messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiEffect {
    /// Move the camera over the plot.
    CenterOn {
        x: i16,
        y: i16,
    },
    OpenCity {
        city: i32,
    },
    CloseCityWindow,
    ToggleGrid,
    /// Ask "Declare war on <civ>?"; the answer goes to `PlayController::answer_war`.
    ConfirmWar {
        team: TeamId,
        civ: String,
        /// The order is a ranged attack, not a move.
        ranged: bool,
    },
}

/// The result of one input event, in the order to be applied.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outputs {
    pub commands: Vec<Command>,
    pub queries: Vec<Query>,
    pub effects: Vec<UiEffect>,
}

impl Outputs {
    /// Sends commands and queries into the sink (queries are registered with `ctl` to match their
    /// answers) and returns the UI effects. An order the sink refuses (no connection, too many in
    /// flight; NC-D5) is not sent and does not advance the selection.
    pub fn send(self, ctl: &mut PlayController, sink: &mut dyn OrderSink) -> Vec<UiEffect> {
        for cmd in self.commands {
            tracing::info!("command sent: {cmd:?}");
            match sink.order(cmd) {
                Ok(id) => ctl.on_order_sent(id, &cmd),
                Err(why) => {
                    tracing::info!("order refused: {why:?}");
                    ctl.on_command_rejected();
                }
            }
        }
        for q in self.queries {
            let id = sink.query(q);
            ctl.query_sent(id, &q);
        }
        self.effects
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The test world of the fixture host (a dev-dependency: the client itself has no host).
    fn fixture_world() -> WorldView {
        crate::test_world::world()
    }

    /// On a view with the layer a blocker whose target the layer already settled is not one (NC-D13); on a
    /// plain view it always is.
    #[test]
    fn the_layer_settles_a_blocker_before_the_server_does() {
        use protocol::{Blocker, BlockerTarget, UnitFlags};
        let mut w = fixture_world();
        w.blocker = Some(Blocker {
            kind: 3,
            target: BlockerTarget::Unit(4),
        });
        assert!(blocker_of(&w, true).is_some(), "the unit waits for orders");
        let u = w.units.iter_mut().find(|u| u.id == 4).unwrap();
        u.flags = UnitFlags(u.flags.0 & !UnitFlags::NEEDS_ORDERS);
        assert_eq!(blocker_of(&w, true), None, "the layer took its orders");
        assert!(blocker_of(&w, false).is_some(), "a plain view has no layer");
        w.blocker = Some(Blocker {
            kind: BLOCKING_PRODUCTION,
            target: BlockerTarget::City(1),
        });
        w.cities
            .iter_mut()
            .find(|c| c.id == 1)
            .unwrap()
            .production_item = -1;
        assert!(blocker_of(&w, true).is_some(), "nothing is built");
        w.cities
            .iter_mut()
            .find(|c| c.id == 1)
            .unwrap()
            .production_item = 3;
        assert_eq!(blocker_of(&w, true), None, "the layer chose a build");
        // A blocker of another kind on a city is not settled by a production.
        w.blocker = Some(Blocker {
            kind: 10,
            target: BlockerTarget::City(1),
        });
        assert!(blocker_of(&w, true).is_some());
    }
}
