//! Slots and the turn status of the humans: `SlotOp` (`gc_slot_control`) and `Q_TURN`
//! (`gamecore_abi.h`).

use gamecore_sys::ffi;

/// A change of a human slot's connection, made at a safe point (`Game::slot_control`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotOp {
    /// The human is on line: it comes back to its own slot, or takes a living AI major civ's
    /// slot (a nick is required then).
    Connect,
    /// The connection is lost: the slot stays human and the turn waits for it.
    Disconnect,
    /// The room host kicked the player: the AI plays the slot until a `Connect` takes it again.
    Kick,
}

impl SlotOp {
    /// The `GC_SLOT_*` number.
    pub fn abi(self) -> i32 {
        match self {
            SlotOp::Connect => ffi::GC_SLOT_CONNECT,
            SlotOp::Disconnect => ffi::GC_SLOT_DISCONNECT,
            SlotOp::Kick => ffi::GC_SLOT_KICK,
        }
    }
}

/// `flags` of a `Q_TURN` record (`GC_TURN_F_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TurnFlags(pub u16);

impl TurnFlags {
    fn has(self, bit: u16) -> bool {
        self.0 & bit != 0
    }

    /// The slot is a human now.
    pub fn human(self) -> bool {
        self.has(ffi::GC_TURN_F_HUMAN)
    }

    pub fn alive(self) -> bool {
        self.has(ffi::GC_TURN_F_ALIVE)
    }

    /// The human is on line (not saved: nobody is on line after a load until `Connect`).
    pub fn connected(self) -> bool {
        self.has(ffi::GC_TURN_F_CONNECTED)
    }

    /// Lekmod's autopause flag of a human who dropped while its turn was active.
    pub fn disconnected(self) -> bool {
        self.has(ffi::GC_TURN_F_DISCONNECTED)
    }

    /// The room host kicked the slot; an AI plays it.
    pub fn kicked(self) -> bool {
        self.has(ffi::GC_TURN_F_KICKED)
    }

    /// `isTurnActive()`.
    pub fn turn_active(self) -> bool {
        self.has(ffi::GC_TURN_F_TURN_ACTIVE)
    }

    /// The end of turn is sent.
    pub fn turn_ended(self) -> bool {
        self.has(ffi::GC_TURN_F_TURN_ENDED)
    }

    /// No blocker holds the end of turn.
    pub fn can_end_turn(self) -> bool {
        self.has(ffi::GC_TURN_F_CAN_END_TURN)
    }

    /// The human moves simultaneously in this turn.
    pub fn simultaneous(self) -> bool {
        self.has(ffi::GC_TURN_F_SIMULTANEOUS)
    }

    /// The human is in auto moves.
    pub fn automoves(self) -> bool {
        self.has(ffi::GC_TURN_F_AUTOMOVES)
    }

    /// The turn waits for this human.
    pub fn waiting_for(self) -> bool {
        self.has(ffi::GC_TURN_F_WAITING_FOR)
    }
}

/// One record of `Q_TURN`: the turn status of a slot that is or was a human.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HumanSlot {
    pub player: u8,
    pub team: i8,
    /// `SlotStatus` of the core (as `Player::slot`).
    pub slot_status: i8,
    /// `EndTurnBlockingTypes`, -1 = none.
    pub blocker: i8,
    pub flags: TurnFlags,
    /// Hybrid turn: the rank of the human among those who do not move simultaneously, by player
    /// id (0 = first); `None` = simultaneous turn.
    pub turn_order: Option<u8>,
    /// Unit id of the blocker's target, -1 = none.
    pub blocker_unit: i32,
    /// City id of the blocker's target, -1 = none.
    pub blocker_city: i32,
    /// Plot of the blocker's target, -1 = none.
    pub blocker_x: i16,
    pub blocker_y: i16,
    /// The human's nick; `None` when the slot has none.
    pub nick: Option<String>,
}

impl HumanSlot {
    /// Plot of the blocker's target.
    pub fn blocker_plot(&self) -> Option<(i16, i16)> {
        (self.blocker_x >= 0 && self.blocker_y >= 0).then_some((self.blocker_x, self.blocker_y))
    }
}
