//! Wgpu rendering (§5.6): WebGPU or WebGL2 on web, Vulkan/DX12/Metal native. No compute or storage
//! buffers, since WebGL2 lacks them. Platform-agnostic (P2): the app provides the surface.

pub mod anim;
pub mod blend;
mod camera;
pub mod camera_params;
pub mod cutout;
mod decal;
pub mod entities;
pub mod frame;
mod game_camera;
pub mod gfx_choice;
#[cfg(test)]
mod gpu_test;
pub mod hexgrid;
pub mod instancing;
pub mod layers;
pub mod lighting;
pub mod material;
pub mod mips;
mod model;
pub mod overlay;
pub mod perf;
pub mod picking;
mod pose;
pub mod reflection;
pub mod render_scale;
mod renderer;
pub mod scene;
pub mod settings;
pub mod shaders;
pub mod shadow;
pub mod skin;
pub mod static_model;
pub mod synthetic;
pub mod terrain;
pub mod terrain_detail;
mod terrain_pipeline;
pub mod texture;
pub mod ui_pass;
mod water_pipeline;
pub mod world;
pub mod wrap;

pub use camera::{
    Camera, CameraHints, CameraView, FirstPerson, FixedCamera, ModCamera, ModCameraView,
    ObjectsNearEye,
};
pub use frame::{FrameSource, Image, PassStat, Readback, ReadbackError, pass_name};
pub use game_camera::{GameCamera, MapBounds, Ray, pixels_to_ndc};
pub use layers::{LayerSwitches, LayerWarning};
pub use model::{GpuModel, InstanceId, MAX_BONES, ModelVertex};
pub use overlay::{DebugRectOverlay, MinimapInfo, Overlay, OverlayTarget, OverlayTextures};
pub use picking::{Pick, PickIndex};
pub use pose::PoseEvaluator;
pub use renderer::{
    BackendInfo, BackendKind, BackendPref, DeviceReport, FrameStats, RenderError, Renderer,
    SurfaceTargetSource, TextureCompression, set_device_lost_hook,
};
pub use scene::Scene;
pub use settings::{IniWarning, KeySet, Preset, RenderSettings, SettingsError};
pub use terrain::features::FeatureArtSet;
pub use terrain::texture_array::{LayerPlan, TerrainArtSlots};
pub use terrain::{TerrainMesh, TerrainSnapshot};
pub use ui_pass::{
    UiExternal, UiPass, UiStats, UiTarget, UiTextureData, UiTextureError, UiTextures, UiTimes,
};

/// Re-exported: model and texture blob types appear in the renderer API.
pub use asset_format;
/// Re-exported: `Camera` and `Scene` use its types.
pub use glam;
/// Re-exported so `client-app` needs no direct wgpu dependency.
pub use wgpu;
