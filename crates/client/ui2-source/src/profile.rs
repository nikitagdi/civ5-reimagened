//! Where a frame of the source went: the cost of each root it built and of each kind of read it made, in cost
//! units and, when the host gave the source a clock ([`UiSource::set_profile_clock`](crate::UiSource::set_profile_clock)), in µs.
//! For the debug overlay, the performance report and the tests that hold the source to its budget.

use crate::cost::ReadKind;
use crate::trigger::Roots;

/// The read kinds as the profile counts them: one slot per `GameRead` method, the catalog tables in one.
pub const READ_SLOTS: usize = 14;

/// The name of each read slot ([`ReadKind::slot`]).
pub const READ_NAMES: [&str; READ_SLOTS] = [
    "game_info",
    "top_bar",
    "players",
    "notifications",
    "techs",
    "player_resources",
    "policy_branches",
    "units",
    "cities",
    "unit_actions",
    "unit_promotions",
    "city_queue",
    "city_builds",
    "catalog",
];

/// The counter of each read slot's µs in a frame (the host publishes them).
pub const READ_COUNTERS: [&str; READ_SLOTS] = [
    "ui2.source.read_us.game_info",
    "ui2.source.read_us.top_bar",
    "ui2.source.read_us.players",
    "ui2.source.read_us.notifications",
    "ui2.source.read_us.techs",
    "ui2.source.read_us.player_resources",
    "ui2.source.read_us.policy_branches",
    "ui2.source.read_us.units",
    "ui2.source.read_us.cities",
    "ui2.source.read_us.unit_actions",
    "ui2.source.read_us.unit_promotions",
    "ui2.source.read_us.city_queue",
    "ui2.source.read_us.city_builds",
    "ui2.source.read_us.catalog",
];

/// The roots of the game, with the counter of each one's µs in a frame (the host publishes them).
pub const ROOT_COUNTERS: [(Roots, &str); 9] = [
    (Roots::TURN, "ui2.source.root_us.turn"),
    (Roots::GAME, "ui2.source.root_us.game"),
    (Roots::TOP, "ui2.source.root_us.top"),
    (Roots::UNIT, "ui2.source.root_us.unit"),
    (Roots::RESEARCH, "ui2.source.root_us.research"),
    (Roots::NOTES, "ui2.source.root_us.notes"),
    (Roots::MAP, "ui2.source.root_us.map"),
    (Roots::PLAYERS, "ui2.source.root_us.players"),
    (Roots::PRODUCTION, "ui2.source.root_us.production"),
];

impl ReadKind {
    /// The profile slot of this read.
    #[must_use]
    pub const fn slot(self) -> usize {
        match self {
            ReadKind::GameInfo => 0,
            ReadKind::TopBar => 1,
            ReadKind::Players => 2,
            ReadKind::Notifications => 3,
            ReadKind::Techs => 4,
            ReadKind::PlayerResources => 5,
            ReadKind::PolicyBranches => 6,
            ReadKind::Units => 7,
            ReadKind::Cities => 8,
            ReadKind::UnitActions => 9,
            ReadKind::UnitPromotions => 10,
            ReadKind::CityQueue => 11,
            ReadKind::CityBuilds => 12,
            ReadKind::Catalog(_) => 13,
        }
    }
}

/// The reads of one kind in a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ReadCost {
    pub calls: u32,
    /// Cost units (the weights of [`crate::cost`]).
    pub units: u32,
    /// Measured time, µs (0 without a clock).
    pub us: f64,
}

/// One root a frame worked on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RootCost {
    pub root: Roots,
    /// Cost units of the reads made for it (reads another root of the frame already made cost nothing).
    pub units: u32,
    /// Measured time of the build (reads and assembly), µs (0 without a clock).
    pub us: f64,
    /// The root reached the model (false: a read was refused and it waits).
    pub written: bool,
}

/// What the last frame (or warm-up) of the source spent.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceProfile {
    /// The roots built or tried, in the order the frame took them.
    pub roots: Vec<RootCost>,
    /// The reads by kind ([`ReadKind::slot`]).
    pub reads: [ReadCost; READ_SLOTS],
    /// The game could be read in this frame (a live replica with no batch waiting).
    pub readable: bool,
    /// The roots still waiting after the frame.
    pub pending: Roots,
    /// The turn of the game record read since the game last changed, if any.
    pub game_turn: Option<i32>,
    /// The turn of the session's turn state in the frame, if any.
    pub session_turn: Option<i32>,
}

impl SourceProfile {
    /// The measured µs of `root` in the frame (0 when it was not built or there is no clock).
    #[must_use]
    pub fn root_us(&self, root: Roots) -> f64 {
        self.roots
            .iter()
            .filter(|r| r.root == root)
            .map(|r| r.us)
            .sum()
    }

    /// The cost units of `root` in the frame.
    #[must_use]
    pub fn root_units(&self, root: Roots) -> u32 {
        self.roots
            .iter()
            .filter(|r| r.root == root)
            .map(|r| r.units)
            .sum()
    }

    /// `root` was written in the frame.
    #[must_use]
    pub fn wrote(&self, root: Roots) -> bool {
        self.roots.iter().any(|r| r.root == root && r.written)
    }

    /// The profile as named counters (`ui2.source.*`): every root's and every read kind's µs (0 when absent), the
    /// readable flag, the pending roots' bits, the game turn read and the session's turn (-1 when none).
    #[must_use]
    pub fn counters(&self) -> Vec<(&'static str, f64)> {
        let mut out = Vec::with_capacity(ROOT_COUNTERS.len() + READ_SLOTS + 3);
        for (root, name) in ROOT_COUNTERS {
            out.push((name, self.root_us(root)));
        }
        for (i, name) in READ_COUNTERS.into_iter().enumerate() {
            out.push((name, self.reads[i].us));
        }
        out.push(("ui2.source.readable", f64::from(u8::from(self.readable))));
        out.push(("ui2.source.pending", f64::from(self.pending.bits())));
        out.push((
            "ui2.source.game_turn",
            f64::from(self.game_turn.unwrap_or(-1)),
        ));
        out.push((
            "ui2.source.session_turn",
            f64::from(self.session_turn.unwrap_or(-1)),
        ));
        out
    }
}
