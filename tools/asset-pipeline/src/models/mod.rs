//! models: Granny meshes, skeletons and animations to engine model blobs, the Warrior asset set
//! build, and the glTF debug export.
//!
//! # Conventions (see [`Basis`])
//! - Civ5 (3ds Max / Granny) is Z up, right-handed; the engine is Y up, right-handed with a hex of
//!   radius 1.0 on the XZ plane. [`Basis::CIV5_TO_ENGINE`] is applied to everything in model
//!   space, so blobs are already in engine axes. The scale is not baked: `fScale` of the unit
//!   member and the Civ5-to-engine unit factor are applied by the client when placing the model.
//! - Texture V is flipped (`v' = 1 - v`) to wgpu's top-left origin; see [`convert::UV_FLIP_V`].
//! - glTF is Y up, right-handed, meters: the export needs no axis change. Only the armature root
//!   node carries a scale (the client's total model scale) so the model has a sensible size.
//!
//! # Materials
//! Each mesh group draws with the engine material of its Granny material; the textures of each
//! Granny material are recovered from the stripped gr2 and the fxsxml by [`materials`], which
//! also lists every guess in [`MaterialNote`]s. Files with several models (city building sets)
//! are converted whole, see [`convert_model`].
//!
//! # B2 materials
//! The FGX block of a Granny material becomes an `asset_format::MaterialDesc` ([`fgx`]); texture
//! paths, the shader catalog names and the check of all unit models are in [`material_desc`].
//!
//! # C5MD v2
//! [`bake_v2::bake_model`] builds the B2 model of a Granny scene: all meshes with the node states of
//! the gr2 (`granny::mesh::state_mask`), `MaterialDesc` materials, parts, rigid meshes with `bind`,
//! meshes merged by material ([`merge`]) and packed into little-endian vertex rows.
//!
//! # Static models
//! B1 draws units without animations: [`bake_static`] applies frame 0 of the idle clip to the
//! vertices and drops damage-state hulls, see [`bake`].
//!
//! # Animation events
//! `.fxsxml` lists an event code list per animation but no per-event timing, so every event is
//! placed at time 0; the client only uses the codes to pick clips.

pub mod bake;
pub mod bake_v2;
pub mod basis;
pub mod convert;
pub mod fgx;
pub mod glb;
pub mod material_desc;
pub mod materials;
pub mod merge;
pub mod png;
pub mod resample;
pub mod set;

pub use bake::{DroppedMesh, IDLE_EVENT_CODE, StaticModel, bake_pose, bake_static};
pub use basis::Basis;
pub use convert::{ConvertedAnimation, ModelMaterials, convert_animation, convert_model};
pub use fgx::{NameDict, NameRole, Unresolved, fgx_to_desc};
pub use glb::write_glb;
pub use material_desc::{
    FgxReport, PathResolver, build_descs, check_unit_models, load_shader_catalog,
};
pub use materials::{MaterialNote, MaterialPlan, NoteKind, plan_materials};
pub use set::{SETS, SetDef, build_set};

use crate::art::ArtError;
use crate::game::GameError;
use crate::store::StoreError;
use crate::textures::TextureError;

#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    #[error(transparent)]
    Game(#[from] GameError),
    #[error(transparent)]
    Art(#[from] ArtError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error("texture {name}: {source}")]
    Texture {
        name: String,
        #[source]
        source: TextureError,
    },
    #[error("{what}: {source}")]
    Granny {
        what: String,
        #[source]
        source: granny::GrannyError,
    },
    #[error("{0}")]
    Invalid(String),
    #[error("glTF export: {0}")]
    Gltf(String),
}

impl ModelError {
    pub(crate) fn invalid(msg: impl Into<String>) -> Self {
        Self::Invalid(msg.into())
    }
}
