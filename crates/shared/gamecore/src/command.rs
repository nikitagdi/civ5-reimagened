//! Typed human commands and their `gc_command` encoding (`gamecore_abi.h`).

use gamecore_sys::ffi;

use crate::game::GameError;
use crate::play::OrderType;
use crate::types::RejectReason;

/// What a `CityPurchase` pays with (`GC_YIELD_GOLD` or `GC_YIELD_FAITH`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PurchaseWith {
    Gold,
    Faith,
}

impl PurchaseWith {
    fn abi(self) -> i32 {
        match self {
            PurchaseWith::Gold => ffi::GC_YIELD_GOLD,
            PurchaseWith::Faith => ffi::GC_YIELD_FAITH,
        }
    }
}

/// A human command. Units and cities are named by their id inside the player (`Q_UNITS`, `Q_CITIES`).
/// The player is given to [`crate::Game::command`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// 1: move along the path, continuing over turns.
    UnitMoveTo { unit: i32, x: i32, y: i32 },
    /// 2
    UnitRangeAttack { unit: i32, x: i32, y: i32 },
    /// 3
    UnitFoundCity { unit: i32 },
    /// 4
    UnitFortify { unit: i32 },
    /// 5
    UnitSleep { unit: i32 },
    /// 6
    UnitSkip { unit: i32 },
    /// 7
    UnitHeal { unit: i32 },
    /// 8
    UnitAutomateBuild { unit: i32 },
    /// 9
    UnitAutomateExplore { unit: i32 },
    /// 10: cancel the orders.
    UnitCancel { unit: i32 },
    /// 11
    UnitDisband { unit: i32 },
    /// 12
    UnitWake { unit: i32 },
    /// 20
    CitySetProduction {
        city: i32,
        order: OrderType,
        item: i32,
    },
    /// 21
    CityPurchase {
        city: i32,
        order: OrderType,
        item: i32,
        with: PurchaseWith,
    },
    /// 22
    CityRangeAttack { city: i32, x: i32, y: i32 },
    /// 30: also against a team of humans.
    DeclareWar { team: i32 },
    /// 31: white peace with a team of humans. Both sides agreed (the gateway checks that); the core
    /// checks that both are humans, the war is on and a peace treaty is possible.
    MakePeace { team: i32 },
    /// 40: refused while a blocker holds the turn.
    EndTurn,
    /// 41
    ForceEndTurn,
    /// 42: takes back the player's end of turn, while not every human has ended it yet.
    CancelEndTurn,
    /// 43: server only, when the turn timer runs out: ends the player's turn without the
    /// blocker checks.
    EndTurnTimeout,
}

impl Command {
    /// The `gc_command` kind (`GC_CMD_*`) and arguments.
    pub fn encode(&self) -> (u16, Vec<i32>) {
        use Command::*;
        match *self {
            UnitMoveTo { unit, x, y } => (ffi::GC_CMD_UNIT_MOVE_TO, vec![unit, x, y]),
            UnitRangeAttack { unit, x, y } => (ffi::GC_CMD_UNIT_RANGE_ATTACK, vec![unit, x, y]),
            UnitFoundCity { unit } => (ffi::GC_CMD_UNIT_FOUND_CITY, vec![unit]),
            UnitFortify { unit } => (ffi::GC_CMD_UNIT_FORTIFY, vec![unit]),
            UnitSleep { unit } => (ffi::GC_CMD_UNIT_SLEEP, vec![unit]),
            UnitSkip { unit } => (ffi::GC_CMD_UNIT_SKIP, vec![unit]),
            UnitHeal { unit } => (ffi::GC_CMD_UNIT_HEAL, vec![unit]),
            UnitAutomateBuild { unit } => (ffi::GC_CMD_UNIT_AUTOMATE_BUILD, vec![unit]),
            UnitAutomateExplore { unit } => (ffi::GC_CMD_UNIT_AUTOMATE_EXPLORE, vec![unit]),
            UnitCancel { unit } => (ffi::GC_CMD_UNIT_CANCEL, vec![unit]),
            UnitDisband { unit } => (ffi::GC_CMD_UNIT_DISBAND, vec![unit]),
            UnitWake { unit } => (ffi::GC_CMD_UNIT_WAKE, vec![unit]),
            CitySetProduction { city, order, item } => (
                ffi::GC_CMD_CITY_SET_PRODUCTION,
                vec![city, order.abi(), item],
            ),
            CityPurchase {
                city,
                order,
                item,
                with,
            } => (
                ffi::GC_CMD_CITY_PURCHASE,
                vec![city, order.abi(), item, with.abi()],
            ),
            CityRangeAttack { city, x, y } => (ffi::GC_CMD_CITY_RANGE_ATTACK, vec![city, x, y]),
            DeclareWar { team } => (ffi::GC_CMD_DECLARE_WAR, vec![team]),
            MakePeace { team } => (ffi::GC_CMD_MAKE_PEACE, vec![team]),
            EndTurn => (ffi::GC_CMD_END_TURN, vec![]),
            ForceEndTurn => (ffi::GC_CMD_FORCE_END_TURN, vec![]),
            CancelEndTurn => (ffi::GC_CMD_CANCEL_END_TURN, vec![]),
            EndTurnTimeout => (ffi::GC_CMD_END_TURN_TIMEOUT, vec![]),
        }
    }
}

/// A command in the raw `gc_command` shape, for kinds without a [`Command`] variant yet.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RawCommand {
    pub kind: u16,
    pub player: u8,
    pub args: Vec<i32>,
    pub text: Option<String>,
}

/// Why a command did not go through.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommandError {
    /// The game refused the command (nothing was queued). `reason` is the machine-readable cause,
    /// `message` the game's text.
    #[error("command rejected: {message}")]
    Rejected {
        reason: RejectReason,
        message: String,
    },
    /// Any other failure (bad arguments, game not started, ...).
    #[error(transparent)]
    Other(GameError),
}

impl From<GameError> for CommandError {
    fn from(e: GameError) -> Self {
        match e {
            GameError::Rejected { reason, message } => CommandError::Rejected { reason, message },
            other => CommandError::Other(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_table() {
        use Command::*;
        let cases: Vec<(Command, u16, Vec<i32>)> = vec![
            (
                UnitMoveTo {
                    unit: 7,
                    x: 3,
                    y: 4,
                },
                1,
                vec![7, 3, 4],
            ),
            (
                UnitRangeAttack {
                    unit: 7,
                    x: 5,
                    y: 6,
                },
                2,
                vec![7, 5, 6],
            ),
            (UnitFoundCity { unit: 1 }, 3, vec![1]),
            (UnitFortify { unit: 2 }, 4, vec![2]),
            (UnitSleep { unit: 3 }, 5, vec![3]),
            (UnitSkip { unit: 4 }, 6, vec![4]),
            (UnitHeal { unit: 5 }, 7, vec![5]),
            (UnitAutomateBuild { unit: 6 }, 8, vec![6]),
            (UnitAutomateExplore { unit: 7 }, 9, vec![7]),
            (UnitCancel { unit: 8 }, 10, vec![8]),
            (UnitDisband { unit: 9 }, 11, vec![9]),
            (UnitWake { unit: 10 }, 12, vec![10]),
            (
                CitySetProduction {
                    city: 2,
                    order: OrderType::Train,
                    item: 11,
                },
                20,
                vec![2, 0, 11],
            ),
            (
                CityPurchase {
                    city: 2,
                    order: OrderType::Construct,
                    item: 12,
                    with: PurchaseWith::Gold,
                },
                21,
                vec![2, 1, 12, 2],
            ),
            (
                CityPurchase {
                    city: 2,
                    order: OrderType::Create,
                    item: 1,
                    with: PurchaseWith::Faith,
                },
                21,
                vec![2, 2, 1, 5],
            ),
            (
                CitySetProduction {
                    city: 2,
                    order: OrderType::Maintain,
                    item: 0,
                },
                20,
                vec![2, 4, 0],
            ),
            (
                CityRangeAttack {
                    city: 2,
                    x: 8,
                    y: 9,
                },
                22,
                vec![2, 8, 9],
            ),
            (DeclareWar { team: 3 }, 30, vec![3]),
            (MakePeace { team: 4 }, 31, vec![4]),
            (EndTurn, 40, vec![]),
            (ForceEndTurn, 41, vec![]),
            (CancelEndTurn, 42, vec![]),
            (EndTurnTimeout, 43, vec![]),
        ];
        for (cmd, kind, args) in cases {
            assert_eq!(cmd.encode(), (kind, args), "{cmd:?}");
        }
    }

    #[test]
    fn rejection_is_typed() {
        let why = GameError::Rejected {
            reason: RejectReason::Blocked,
            message: "blocked by UNITS".into(),
        };
        assert_eq!(
            CommandError::from(why),
            CommandError::Rejected {
                reason: RejectReason::Blocked,
                message: "blocked by UNITS".into()
            }
        );
        assert_eq!(
            CommandError::Rejected {
                reason: RejectReason::Rule,
                message: "no money".into()
            }
            .to_string(),
            "command rejected: no money"
        );
        assert!(matches!(
            CommandError::from(GameError::NotStarted),
            CommandError::Other(GameError::NotStarted)
        ));
    }
}
