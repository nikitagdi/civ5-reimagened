//! Safe API over `gamecore-sys` (architecture §5.4). One game per process.

mod command;
mod event;
mod game;
mod host;
mod play;
pub mod query;
pub mod replica;
mod save;
mod selftest;
mod setup;
mod slots;
mod types;
mod ui;

pub use command::{Command, CommandError, PurchaseWith, RawCommand};
pub use event::{AutoChoiceKind, GameEvent, MessageSource, MessageType};
pub use game::{Game, GameError, PumpResult};
/// The core's error, the payload of [`GameError::Core`]: re-exported so that crates which may not link
/// `gamecore-sys` (rule R10) can tell "not implemented in this core" from other failures.
pub use gamecore_sys::GcError;
pub use gamecore_sys::IMPLEMENTATION;
pub use host::{HostServices, LogLevel};
pub use play::{
    Activity, Attacker, BuildFlags, City, CityArt, CityBuild, CityFlags, MOVE_DENOMINATOR,
    Notification, OrderType, PathNode, RangeShooter, RangeTarget, RangeTargetFlags, ReachFlags,
    Reachable, TopBar, TopBarFlags, TradePath, TradeRoute, TreeNode, Unit, UnitAction, UnitFlags,
};
pub use query::QueryError;
pub use replica::{Applied, NetMessage, ReplicaGame, Role};
pub use save::SaveInfo;
pub use selftest::{SelftestCase, SelftestReport, selftest};
pub use setup::{AutoChoices, GameType, MinorCivs, Setup, SlotKind, SlotSetup};
pub use slots::{HumanSlot, SlotOp, TurnFlags};
pub use types::{
    BasisFlags, CatalogEntry, CatalogTable, CivInfo, CombatFlags, CombatForecast, DiploFlags,
    DiploRelation, Direction, GameInfo, GameOptionEntry, GameStatus, MapOptionEntry,
    MapOptionValueEntry, NamedEntry, Phase, Player, PlayerFlags, Plot, PlotBasis, PlotFlags,
    PlotType, RejectReason, SetupDescription, TimerEntry, TurnMode, UnsafeReason, WorldSizeEntry,
    same_script,
};
pub use ui::{UiEvent, UiEventKind, UiOp, UiValue};

/// ABI version of the linked gamecore.
pub fn abi_version() -> u32 {
    gamecore_sys::abi_version()
}

/// Sets the clocks of the C/C++ runtime (ns: wall time since the Unix epoch, and monotonic).
/// The host must call it at start-up, otherwise the C++ side sees 1970 (wasm only; no-op natively).
pub fn set_clock(realtime_ns: u64, monotonic_ns: u64) {
    lua_sys::wasi::set_clock(realtime_ns, monotonic_ns);
}

/// Forwards buffered C/C++ output to `tracing`.
pub(crate) fn flush_cpp_output() {
    let out = lua_sys::wasi::take_output();
    for line in out.lines().filter(|l| !l.is_empty()) {
        tracing::info!(target: "cpp", "{line}");
    }
}
