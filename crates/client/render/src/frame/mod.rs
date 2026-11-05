//! Frame skeleton of the original (`GameViewState::RenderGame`): the fixed pass list, the
//! subsystem trait with its draw slots, the render-target pool and the per-view constants
//! `renderer.rs` runs the plan; B1 draws through the adapters
//! of `b1_systems` (R0.10) in place of the stubs of `world/` until a `world-*` module moves its
//! subsystem. The main pass is MSAA into `Scene` (RC-2); passes into targets wait for their subsystems.

pub(crate) mod b1_systems;
pub mod graph;
pub mod mask;
pub mod present;
pub mod readback;
pub mod system;
pub mod systems;
pub mod targets;
pub mod timing;
pub mod views;

pub use graph::{FramePlan, MainSlot, PassId, PassMode, Step, plan, settled};
pub use present::Present;
pub use readback::{FrameSource, Image, Readback, ReadbackError};
pub use system::{
    AnimService, DrawContext, EncoderContext, InitContext, MaskContext, PassSlot, PrepareContext,
    RenderSystem, SystemId, WorldServices,
};
pub use systems::{WorldSystems, wire_build_stages};
pub use targets::{
    CLEAR_GREY, DEPTH_FORMAT, FOW_BASE, FOW_MASK, MINIMAP, MINIMAP_SIZE, OVERLAY_MAP,
    REFLECTION_COLOR, REFLECTION_DEPTH, ResourceDesc, ResourceId, SCENE, SCENE_DEPTH, SCENE_MSAA,
    SCREEN_OVERLAY, Target, TargetError, TargetPool, TargetSize, WORLD_FORMAT,
    declare_main_targets, declare_overlay_targets, fow_mask_side, multisample,
};
pub use timing::{CpuClock, GpuTimer, PassStat, PassTally, TimerSpan, pass_name};
pub use views::{MAX_MAP_COPIES, ViewBuffers, ViewGlobals, basis_rotation, copy_shifts};
