//! Game host core without I/O: the [`Worker`] drives a [`Core`] and answers the
//! clients' messages; the shell (the server's worker process) does the I/O.
//!
//! `core::Core` is the core the `Worker` asks (GH-D1). `real::RealCore` is the core on the gamecore brain;
//! `fixture::FixtureCore` is a deterministic test world without the brain. The fixture is a development
//! tool, not a game simplification.

mod audit;
mod checkpoint;
mod context;
pub mod core;
pub mod files;
pub mod fixture;
mod journal;
pub mod lobby_options;
pub mod map_scripts;
mod orders;
mod peace;
pub mod real;
pub mod replica;
pub mod saves;
mod seats;
mod slots;
mod stats;
mod timeline;
mod turn;
pub mod worker;
mod world_build;

pub use crate::core::{
    Core, CoreError, CoreErrorKind, CoreEvent, CorePhase, CorePump, CoreStart, CoreTurn,
    EventTarget, MapInfo, Refusal, RefusalCode,
};
pub use files::{GameFiles, LoadError, LoadReport, Progress, load_game_files};
pub use replica::{
    CHECK_EVERY_MS, PatchFn, ReplicaBody, ReplicaDone, ReplicaJob, ReplicaOps, ReplicaPacked,
    SnapFn,
};
pub use seats::CityMemo;
pub use worker::{Out, SaveDone, SaveFile, SaveJob, Worker};
