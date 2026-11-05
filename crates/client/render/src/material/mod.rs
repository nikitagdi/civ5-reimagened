//! Materials of the ported shader families: `MaterialDesc` to a
//! `ShaderFamily` by the package string, the layout of the vertices from `Mesh.layout`, the cache
//! of pipelines, and the table of sampler states.

pub mod desc;
pub mod families;
pub mod pipeline_cache;
pub mod sampler;

pub use desc::{
    BlendMode, DrawKey, InstanceLayout, LayoutError, MaterialDesc, PassKind, RenderState, TexKind,
    TexSource, VertexAttr, VertexLayout, blends, ground_decal, sort_calls, stencil_only,
};
pub use families::{
    FAMILIES, Selection, ShaderFamily, Skip, UNIT_OBJECT, family_for, family_in, resolve,
    resolve_in, resolve_object,
};
pub use pipeline_cache::{CacheStats, PipelineCache, PipelineKey, PipelineRequest};
