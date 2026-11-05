//! The client's replica of the server's game.
//!
//! `Replica` is a sans-IO state machine over the replica stream of protocol v4 (`GcBase`, `GcJournal`,
//! `GcTurn`) and a game [`Engine`] (the real one is `gamecore::ReplicaGame`): `Empty -> WaitBlob -> Loading ->
//! Live -> Resyncing`. It never moves the game by itself (R10): the game changes only by the server's journal
//! batches, snapshots and patches. The UI's own messages (`gDLL->send*`) leave through [`Replica::take_outgoing`].
//!
//! This is the only client crate with a direct dependency on `gamecore` (rule R10).

mod engine;
mod game_side;
mod host;
mod machine;
pub mod selftest;

pub use engine::{CoreEngine, Engine};
pub use game_side::{
    GameSide, NetMessage, PathNode, ReachFlags, TradePath, UiEvent, UiEventKind, UiOp, UiValue,
};
pub use host::{
    GAMEPLAY_DB_ID, GROUP_ID, GameDataError, GameDataHost, GameFiles, LOC_DB_ID, VFS_INDEX_ID,
    group_members,
};
pub use machine::{
    OWN_PENDING_MAX_MS, Outgoing, Replica, ReplicaConfig, ReplicaSpan, ReplicaStats, ReplicaStep,
    State,
};

pub use client_core::{
    GcBase, GcBlobRef, GcJournal, GcPatchRef, GcResyncReason, GcTurn, ReplicaMsg,
};

/// Why a replica could not be made.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReplicaError {
    /// The core did not boot (game data missing, a second instance, a wrong ABI).
    #[error("replica core: {0}")]
    Core(String),
}
