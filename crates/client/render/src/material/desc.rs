//! What a draw call needs to say about itself to the pipeline cache: the
//! kind of pass, the layout of the vertices and instances, and the blend and depth state taken
//! from the `MaterialDesc` of the model.

use std::collections::BTreeSet;

pub use asset_format::{MaterialDesc, TexKind, TexSource};

use crate::shaders::{MAX_VERTEX_ATTRIBUTES, Semantic, free_texcoord_locations};

/// The kind of pass a call is made in; the family picks its entries and `override` values by it
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PassKind {
    /// `Main.*`: the lit colour.
    Main,
    /// `ShadowMap`: the `*_Shadow` entries.
    Shadow,
    /// `Reflection`.
    Reflection,
    /// `Main.UnitZFail`: drawn where the depth test fails (the part of a unit behind a wall).
    ZFail,
    /// `ClassMask` (`golden`).
    Mask,
}

/// A vertex attribute: its semantic gives the `@location`, the format and offset say where the
/// data is in the vertex.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct VertexAttr {
    pub semantic: Semantic,
    pub format: wgpu::VertexFormat,
    pub offset: u64,
}

/// Why a layout cannot be used.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LayoutError {
    #[error("the semantic {0} is used twice")]
    Duplicate(String),
    #[error("{0} attributes, WebGL2 has {MAX_VERTEX_ATTRIBUTES}")]
    TooMany(usize),
    #[error("attribute {semantic} at offset {offset} ends at {end}, past the stride {stride}")]
    PastTheStride {
        semantic: String,
        offset: u64,
        end: u64,
        stride: u64,
    },
    #[error("the instance data needs {need} free TEXCOORD slots, the mesh leaves {free}")]
    NoFreeSlots { need: usize, free: usize },
}

/// The layout of the vertex buffer of a mesh, built from the semantics of `Mesh.layout`. The
/// `@location`s come from [`Semantic::location`], so any mesh and any ported vertex
/// shader agree without a table of their own.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct VertexLayout {
    stride: u64,
    attrs: Vec<VertexAttr>,
}

impl VertexLayout {
    /// A layout of `attrs` in a vertex of `stride` bytes.
    pub fn new(stride: u64, attrs: Vec<VertexAttr>) -> Result<Self, LayoutError> {
        if attrs.len() > MAX_VERTEX_ATTRIBUTES as usize {
            return Err(LayoutError::TooMany(attrs.len()));
        }
        let mut seen = BTreeSet::new();
        for a in &attrs {
            if !seen.insert(a.semantic.location()) {
                return Err(LayoutError::Duplicate(a.semantic.d3d_name()));
            }
            let end = a.offset + a.format.size();
            if end > stride {
                return Err(LayoutError::PastTheStride {
                    semantic: a.semantic.d3d_name(),
                    offset: a.offset,
                    end,
                    stride,
                });
            }
        }
        Ok(Self { stride, attrs })
    }

    /// No vertex buffer (a fullscreen triangle from the vertex index).
    pub fn empty() -> Self {
        Self {
            stride: 0,
            attrs: Vec::new(),
        }
    }

    pub fn stride(&self) -> u64 {
        self.stride
    }

    pub fn attrs(&self) -> &[VertexAttr] {
        &self.attrs
    }

    pub fn is_empty(&self) -> bool {
        self.attrs.is_empty()
    }

    pub fn semantics(&self) -> Vec<Semantic> {
        self.attrs.iter().map(|a| a.semantic).collect()
    }

    /// The attributes as wgpu wants them, at the `@location` of each semantic.
    pub fn wgpu_attributes(&self) -> Vec<wgpu::VertexAttribute> {
        self.attrs
            .iter()
            .map(|a| wgpu::VertexAttribute {
                format: a.format,
                offset: a.offset,
                shader_location: a.semantic.location(),
            })
            .collect()
    }
}

/// The per-instance data of a family: formats and offsets in the instance record. They take the
/// `TEXCOORD` slots the mesh leaves free, in order, so the same record fits any mesh.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct InstanceLayout {
    stride: u64,
    fields: Vec<(wgpu::VertexFormat, u64)>,
}

impl InstanceLayout {
    pub fn new(stride: u64, fields: Vec<(wgpu::VertexFormat, u64)>) -> Self {
        Self { stride, fields }
    }

    pub fn stride(&self) -> u64 {
        self.stride
    }

    /// The attributes at the free `TEXCOORD` locations of `vertex`.
    pub fn attributes(
        &self,
        vertex: &VertexLayout,
    ) -> Result<Vec<wgpu::VertexAttribute>, LayoutError> {
        let free = free_texcoord_locations(&vertex.semantics());
        if self.fields.len() > free.len() {
            return Err(LayoutError::NoFreeSlots {
                need: self.fields.len(),
                free: free.len(),
            });
        }
        Ok(self
            .fields
            .iter()
            .zip(free)
            .map(|(&(format, offset), location)| wgpu::VertexAttribute {
                format,
                offset,
                shader_location: location,
            })
            .collect())
    }
}

/// The package of the ground quads of the landmarks (the stone of a quarry, the base of a city).
pub const LANDMARK_PACKAGE: &str = "LandmarkShader";
/// The package of the meshes that only cut the trees (`Tree Cut` of an improvement): they write the
/// stencil the forest reads, never a colour.
pub const LANDMARK_STENCIL_PACKAGE: &str = "LandmarkShader_Stencil";

/// The material is drawn over what is behind it by its alpha: the ones sorted by distance and the
/// ground quads of the landmarks. Every landmark material has the same `alpha_z` ([8, 4, 128]) as
/// the opaque buildings, so the package decides; the quads are `BC3` with a soft alpha rim, and
/// drawn opaque they are a box around the stone. No capture of the main pass (S1a, S1b, S2, S4) has
/// a `LandmarkPS` call: the original lays them into the page of the terrain, a pass we do not have,
/// so they are drawn here as blended quads on the ground.
// TODO(RW-14): the ground quads into the bake of the terrain page, as the original does
pub fn blends(desc: &MaterialDesc) -> bool {
    desc.has(MaterialDesc::ALPHA_SORT) || desc.package == LANDMARK_PACKAGE || is_fx(desc)
}

/// The packages of the effect models (`FXM_*.gr2`): the smoke, fire and caustics. Their pixel shader
/// writes a premultiplied colour and what is left of the light behind it (`BlendMode::FxPremultiplied`),
/// so they are always blended, sorted by distance and never write depth.
pub const FX_PACKAGES: [&str; 2] = ["FXShader", "FXShader_Skinned"];

pub fn is_fx(desc: &MaterialDesc) -> bool {
    FX_PACKAGES.contains(&desc.package.as_str())
}

/// The prefix of the texture files of the god ray (`FXM_God_Ray`, the beam over the selected unit):
/// `FX_God_Ray_base_01.dds`, `..._UV_01.dds`, `..._alpha_01.dds` and so on, by the flat names of the set.
pub const GOD_RAY_TEXTURE_PREFIX: &str = "tx:fx_god_ray_";

/// The material is the god ray: an `FXShader` material whose layers read the `FX_God_Ray_*` textures.
/// It has the render state of the exe's "God Ray" command set ([`RenderState::of_material`]) and the
/// vertex entry that takes its clock from the instance (`fx_god_ray_vs`).
// re: re-b11-answers.txt 1.2 (the command set "God Ray" of cvMovementPathVisSystem::OnRenderView)
pub fn is_god_ray(desc: &MaterialDesc) -> bool {
    is_fx(desc)
        && desc
            .texture_files()
            .any(|id| id.as_str().starts_with(GOD_RAY_TEXTURE_PREFIX))
}

/// The mesh of a landmark is not drawn but laid into the page of the terrain as a decal (it drapes
/// over the relief): a `LandmarkShader` material with a texture of its own. The original's decal
/// container takes every such mesh (`IsDecalMaterial` is 1 for `LandmarkShader`) with its colour
/// texture; the stencil package has its own path (the `Tree Cut`).
// re: rw-w8-answers.txt q2 (DecalContainerBuilderContext::InitMeshDecal 0x088be424, IsDecalMaterial
// 0x088be93e)
pub fn ground_decal(desc: &MaterialDesc) -> bool {
    desc.package == LANDMARK_PACKAGE && desc.texture_files().next().is_some()
}

/// The mesh has no colour of its own in any pass (see [`LANDMARK_STENCIL_PACKAGE`]); drawn, the
/// `Tree Cut` of a quarry is a grey hexagon.
pub fn stencil_only(desc: &MaterialDesc) -> bool {
    desc.package == LANDMARK_STENCIL_PACKAGE
}

/// How the colour is combined with the target (`AlphaMode` of `CopyAlphaZStateData`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlendMode {
    Opaque,
    Alpha,
    /// The output of `FXShader`: `rgb` is the colour premultiplied by the alpha of the layers and `a`
    /// is what is left of the target behind them: colour `One`, destination `SrcAlpha`, the alpha
    /// channel the same.
    // SIMPLIFIED(SIMP-048): AlphaMode 8 of FXShader read as premultiplied over (One, SrcAlpha); the
    // original's state table is not recovered.
    FxPremultiplied,
    /// The "God Ray" command set of the exe (`CreateAlphaState(1, ONE, SRCALPHA, ADD, ZERO, SRCALPHA, ADD)`):
    /// colour `One`, `SrcAlpha`, and the alpha channel `Zero`, `SrcAlpha`. The shader writes
    /// `sqrt(colour)` and `sqrt(what is left)` and the target is the gamma space of the original
    /// (`Rgba8Unorm`, no sRGB write), so the equation runs on the same numbers as the original's.
    // re: re-b11-answers.txt 1.1 (cvMovementPathVisSystem::Startup 0x088bf6bc, CAlphaState::Apply 0x08d14040)
    GodRay,
    /// The main pass of the units' `_CODE` stages as S1a has it (ev 580, 2095): colour `SrcAlpha`,
    /// `InvSrcAlpha`, alpha `One`, `One`, with the depth written. An opaque unit has alpha 1 and
    /// covers what is behind it; a fading one or its part under the water shows the ground.
    UnitCode,
}

/// Blend, depth and culling of a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RenderState {
    pub blend: BlendMode,
    pub depth_write: bool,
    pub depth_compare: wgpu::CompareFunction,
    pub cull: Option<wgpu::Face>,
}

impl RenderState {
    /// Opaque, depth test `Less` and write, no culling (as the B1 pipelines: the winding of
    /// converted content is not guaranteed).
    pub const OPAQUE: Self = Self {
        blend: BlendMode::Opaque,
        depth_write: true,
        depth_compare: wgpu::CompareFunction::Less,
        cull: None,
    };

    /// The state of a material in a pass.
    ///
    /// The three numbers of `alpha_z` (`AlphaMode`, `ZMode`, `AlphaRef`) are stored but their
    /// meaning is not recovered yet (RW-14): the state follows the flags of the material instead.
    // TODO(RW-14): read `alpha_z` once the meaning of `AlphaMode` and `ZMode` is known.
    pub fn of_material(desc: &MaterialDesc, pass: PassKind) -> Self {
        let mut state = Self::OPAQUE;
        if blends(desc) {
            state.blend = if is_god_ray(desc) {
                BlendMode::GodRay
            } else if is_fx(desc) {
                BlendMode::FxPremultiplied
            } else {
                BlendMode::Alpha
            };
            state.depth_write = false;
        }
        if is_god_ray(desc) {
            // SIMPLIFIED(SIMP-exe-1032): see the row.
            // The exe clears the depth to 1.0 and draws the funnel with `LESSEQUAL` and `CULL_CW`
            // (z-state 4): on top of the world, one wall of the convex funnel a pixel. The faces of the
            // baked mesh are counter-clockwise from outside in our basis (a test of the asset
            // pipeline holds that), so the culled side is `Back`. A depth clear in the middle of the
            // pass is not possible; `Always` without a write is the same picture.
            state.depth_compare = wgpu::CompareFunction::Always;
            state.cull = Some(wgpu::Face::Back);
        }
        if pass == PassKind::ZFail {
            // The part of a unit hidden behind the terrain: where the depth test fails.
            state.depth_compare = wgpu::CompareFunction::Greater;
            state.depth_write = false;
        }
        state
    }

    pub fn is_transparent(&self) -> bool {
        self.blend != BlendMode::Opaque
    }
}

/// What the sort of a subsystem's calls looks at (`StateSort` of the original): the pipeline and
/// the material, and for the transparent ones the distance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrawKey {
    pub pipeline: u32,
    pub material: u32,
    /// Distance from the camera.
    pub distance: f32,
    pub transparent: bool,
}

/// Sorts calls in place: the opaque ones by (pipeline, material), so state changes are few, then
/// the transparent ones from the farthest to the nearest.
pub fn sort_calls<T>(calls: &mut [T], key: impl Fn(&T) -> DrawKey) {
    calls.sort_by(|a, b| {
        let (a, b) = (key(a), key(b));
        a.transparent.cmp(&b.transparent).then_with(|| {
            if a.transparent {
                b.distance.total_cmp(&a.distance)
            } else {
                (a.pipeline, a.material).cmp(&(b.pipeline, b.material))
            }
        })
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::VertexFormat as F;

    fn attr(semantic: Semantic, format: F, offset: u64) -> VertexAttr {
        VertexAttr {
            semantic,
            format,
            offset,
        }
    }

    #[test]
    fn a_layout_puts_each_semantic_at_its_location() {
        let l = VertexLayout::new(
            24,
            vec![
                attr(Semantic::Position0, F::Float32x3, 0),
                attr(Semantic::Normal0, F::Snorm16x4, 12),
                attr(Semantic::Texcoord(0), F::Float16x2, 20),
            ],
        )
        .unwrap();
        let locations: Vec<u32> = l
            .wgpu_attributes()
            .iter()
            .map(|a| a.shader_location)
            .collect();
        assert_eq!(locations, [0, 1, 2]);
        assert_eq!(l.stride(), 24);
        assert!(VertexLayout::empty().is_empty());
    }

    #[test]
    fn a_bad_layout_is_refused() {
        let twice = VertexLayout::new(
            32,
            vec![
                attr(Semantic::Position0, F::Float32x3, 0),
                attr(Semantic::Position0, F::Float32x3, 12),
            ],
        );
        assert!(matches!(twice, Err(LayoutError::Duplicate(_))));
        let past = VertexLayout::new(8, vec![attr(Semantic::Position0, F::Float32x3, 0)]);
        assert!(matches!(past, Err(LayoutError::PastTheStride { .. })));
        let many: Vec<_> = (0..17)
            .map(|i| attr(Semantic::Texcoord((i % 8) as u8), F::Float32, 0))
            .collect();
        assert!(matches!(
            VertexLayout::new(4, many),
            Err(LayoutError::TooMany(17))
        ));
    }

    #[test]
    fn instance_data_takes_the_texcoords_the_mesh_leaves_free() {
        let mesh = VertexLayout::new(
            20,
            vec![
                attr(Semantic::Position0, F::Float32x3, 0),
                attr(Semantic::Texcoord(0), F::Float32x2, 12),
            ],
        )
        .unwrap();
        let instance = InstanceLayout::new(32, vec![(F::Float32x4, 0), (F::Float32x4, 16)]);
        let a = instance.attributes(&mesh).unwrap();
        // TEXCOORD0 (location 2) is the mesh's; the instance gets TEXCOORD1 and TEXCOORD2.
        assert_eq!(
            a.iter().map(|a| a.shader_location).collect::<Vec<_>>(),
            [3, 4]
        );
        let wide = InstanceLayout::new(160, (0..9).map(|i| (F::Float32x4, i * 16)).collect());
        assert!(matches!(
            wide.attributes(&mesh),
            Err(LayoutError::NoFreeSlots { need: 9, free: 7 })
        ));
    }

    #[test]
    fn the_state_follows_the_flags_and_the_pass() {
        let mut d = MaterialDesc::default();
        assert_eq!(
            RenderState::of_material(&d, PassKind::Main),
            RenderState::OPAQUE
        );
        let z = RenderState::of_material(&d, PassKind::ZFail);
        assert_eq!(z.depth_compare, wgpu::CompareFunction::Greater);
        assert!(!z.depth_write);
        d.flags |= MaterialDesc::ALPHA_SORT;
        let a = RenderState::of_material(&d, PassKind::Main);
        assert!(a.is_transparent() && !a.depth_write);
    }

    /// The ground quads of the landmarks blend by their alpha; the tree cutters draw no colour.
    #[test]
    fn the_landmark_packages_blend_or_stay_out() {
        let quad = MaterialDesc {
            package: LANDMARK_PACKAGE.into(),
            ..MaterialDesc::default()
        };
        assert!(blends(&quad) && !stencil_only(&quad));
        let s = RenderState::of_material(&quad, PassKind::Main);
        assert!(s.is_transparent() && !s.depth_write);
        let cutter = MaterialDesc {
            package: LANDMARK_STENCIL_PACKAGE.into(),
            ..MaterialDesc::default()
        };
        assert!(stencil_only(&cutter) && !blends(&cutter));
        assert!(!blends(&MaterialDesc {
            package: "BuildingShader".into(),
            ..MaterialDesc::default()
        }));
    }

    /// The effect models blend by the premultiplied rule, never write depth, and are sorted.
    #[test]
    fn the_fx_packages_blend_premultiplied_and_are_transparent() {
        for package in FX_PACKAGES {
            let d = MaterialDesc {
                package: package.into(),
                ..MaterialDesc::default()
            };
            assert!(blends(&d), "{package}");
            let s = RenderState::of_material(&d, PassKind::Main);
            assert_eq!(s.blend, BlendMode::FxPremultiplied);
            assert!(s.is_transparent() && !s.depth_write);
        }
        let ordinary = MaterialDesc {
            flags: MaterialDesc::ALPHA_SORT,
            package: "UnitShader".into(),
            ..MaterialDesc::default()
        };
        assert_eq!(
            RenderState::of_material(&ordinary, PassKind::Main).blend,
            BlendMode::Alpha
        );
    }

    /// The god ray (an FX material on the `FX_God_Ray_*` textures) takes the state of the exe's "God Ray"
    /// set: its own blend, no depth write, no test against the world, the outside of the funnel; the
    /// other effects keep the plain premultiplied state.
    #[test]
    fn the_god_ray_material_has_the_state_of_the_exes_god_ray_set() {
        let param = |name: &str, file: &str| asset_format::TexParam {
            name: name.into(),
            kind: TexKind::Tex2D,
            src: TexSource::File(asset_format::LogicalId::new(file)),
            semantic: 0,
            reg: 0,
            usage: 0,
            flags: 0,
            active: true,
        };
        let with = |file: &str| MaterialDesc {
            package: "FXShader".into(),
            sets: vec![asset_format::ParamSet {
                blocks: vec![],
                textures: vec![param("BaseTextureMap0", file)],
                constants: vec![],
            }],
            ..MaterialDesc::default()
        };
        let ray = with("tx:fx_god_ray_base_01.dds");
        assert!(is_god_ray(&ray));
        let s = RenderState::of_material(&ray, PassKind::Main);
        assert_eq!(s.blend, BlendMode::GodRay);
        assert!(!s.depth_write);
        assert_eq!(s.depth_compare, wgpu::CompareFunction::Always);
        assert_eq!(s.cull, Some(wgpu::Face::Back));
        let smoke = with("tx:fx_smoke_01.dds");
        assert!(!is_god_ray(&smoke));
        let s = RenderState::of_material(&smoke, PassKind::Main);
        assert_eq!((s.blend, s.cull), (BlendMode::FxPremultiplied, None));
        // The blend of the exe: colour ONE/SRCALPHA, alpha ZERO/SRCALPHA.
        let b = crate::material::pipeline_cache::god_ray_blend();
        assert_eq!(
            (b.color.src_factor, b.color.dst_factor),
            (wgpu::BlendFactor::One, wgpu::BlendFactor::SrcAlpha)
        );
        assert_eq!(
            (b.alpha.src_factor, b.alpha.dst_factor),
            (wgpu::BlendFactor::Zero, wgpu::BlendFactor::SrcAlpha)
        );
    }

    #[test]
    fn calls_sort_by_state_then_the_transparent_from_far_to_near() {
        let call = |pipeline, material, distance, transparent| DrawKey {
            pipeline,
            material,
            distance,
            transparent,
        };
        let mut calls = vec![
            call(2, 1, 5.0, false),
            call(1, 9, 1.0, true),
            call(1, 2, 7.0, false),
            call(1, 1, 3.0, false),
            call(1, 8, 4.0, true),
        ];
        sort_calls(&mut calls, |k| *k);
        let order: Vec<_> = calls
            .iter()
            .map(|k| (k.pipeline, k.material, k.transparent))
            .collect();
        assert_eq!(
            order,
            [
                (1, 1, false),
                (1, 2, false),
                (2, 1, false),
                // The transparent ones: the one at 4.0 is farther than the one at 1.0.
                (1, 8, true),
                (1, 9, true),
            ]
        );
    }
}
