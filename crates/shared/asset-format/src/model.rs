//! Model blob: skeleton, meshes, materials and animation references.

use glam::{Mat4, Quat, Vec3};
use serde::{Deserialize, Serialize};

use crate::{FormatError, LogicalId, blob};

pub const MODEL_MAGIC: [u8; 4] = *b"C5MD";

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    pub translation: [f32; 3],
    /// xyzw.
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

impl Transform {
    pub const IDENTITY: Self = Self {
        translation: [0.0; 3],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0; 3],
    };

    pub fn to_mat4(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(
            Vec3::from_array(self.scale),
            Quat::from_array(self.rotation),
            Vec3::from_array(self.translation),
        )
    }
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bone {
    pub name: String,
    /// Always an earlier bone.
    pub parent: Option<u16>,
    pub rest: Transform,
    /// Column-major.
    pub inverse_bind: [f32; 16],
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Skeleton {
    pub bones: Vec<Bone>,
}

impl Skeleton {
    pub fn rest_pose(&self) -> Vec<Transform> {
        self.bones.iter().map(|b| b.rest).collect()
    }

    /// Model-space matrices; relies on parents preceding children.
    /// Processes `min(bones, local, out)` bones.
    pub fn world_matrices(&self, local: &[Transform], out: &mut [Mat4]) {
        let n = self.bones.len().min(local.len()).min(out.len());
        for i in 0..n {
            let m = local[i].to_mat4();
            out[i] = match self.bones[i].parent {
                Some(p) if usize::from(p) < i => out[usize::from(p)] * m,
                _ => m,
            };
        }
    }

    /// `world * inverse_bind`, ready for the skinning palette.
    pub fn skin_matrices(&self, world: &[Mat4], out: &mut [Mat4]) {
        for ((bone, w), o) in self.bones.iter().zip(world).zip(out) {
            *o = *w * Mat4::from_cols_array(&bone.inverse_bind);
        }
    }

    fn validate(&self) -> Result<(), FormatError> {
        for (i, b) in self.bones.iter().enumerate() {
            if b.parent.is_some_and(|p| usize::from(p) >= i) {
                return Err(FormatError::invalid(format!(
                    "bone {i} ({}) has a parent that does not precede it",
                    b.name
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Indices {
    U16(Vec<u16>),
    U32(Vec<u32>),
}

impl Indices {
    pub fn len(&self) -> usize {
        match self {
            Self::U16(v) => v.len(),
            Self::U32(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    /// Octahedral snorm16.
    pub normals: Vec<[i16; 2]>,
    /// IEEE f16 bits.
    pub uvs: Vec<[u16; 2]>,
    /// Empty for rigid meshes.
    pub joints: Vec<[u8; 4]>,
    /// Unorm8, each vertex sums to 255. Empty for rigid meshes.
    pub weights: Vec<[u8; 4]>,
    pub indices: Indices,
    /// Index into [`ModelBlob::materials`].
    pub material: u16,
}

impl Mesh {
    fn validate(&self, index: usize, materials: usize, bones: usize) -> Result<(), FormatError> {
        let n = self.positions.len();
        let bad = |what: &str| Err(FormatError::invalid(format!("mesh {index}: {what}")));
        if self.normals.len() != n || self.uvs.len() != n {
            return bad("normals/uvs length differs from positions");
        }
        if self.joints.len() != self.weights.len()
            || (!self.joints.is_empty() && self.joints.len() != n)
        {
            return bad("joints/weights must be empty or match positions");
        }
        if self
            .joints
            .iter()
            .flatten()
            .any(|&j| usize::from(j) >= bones)
        {
            return bad("joint index out of range");
        }
        if usize::from(self.material) >= materials {
            return bad("material index out of range");
        }
        let in_range = match &self.indices {
            Indices::U16(v) => v.iter().all(|&i| usize::from(i) < n),
            Indices::U32(v) => v.iter().all(|&i| (i as usize) < n),
        };
        if !in_range {
            return bad("index out of range");
        }
        if !self.indices.len().is_multiple_of(3) {
            return bad("index count is not a multiple of 3");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Material {
    pub diffuse: Option<LogicalId>,
    pub specular: Option<LogicalId>,
}

/// Kind of a texture parameter (`ParamTypes` 6, 7, 10 of the FGX block).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TexKind {
    Tex2D,
    Cube,
    Tex3D,
}

/// Where a texture parameter gets its image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TexSource {
    /// A game texture, by logical id (`tx:<file name>`).
    File(LogicalId),
    /// Supplied by the engine by sampler name (`LightingEnv`, `ShadowMap`, `FowMask`, ...): the
    /// FGX path is empty.
    Engine,
    /// A path hash that matched no file. Never produced for unit models (the build fails
    /// instead); it lets other model sets build with the guess listed in the report.
    Unresolved(u32),
}

/// A texture sampler of a material. `name` is the CTAB sampler name; an unreadable one is
/// `#xxxxxxxx` (the hash).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TexParam {
    pub name: String,
    pub kind: TexKind,
    pub src: TexSource,
    /// `ParamSemantics` as stored (meaning not recovered, RW-9a).
    pub semantic: i32,
    /// `ParamRegs` as stored: a packed value, not a plain register number.
    pub reg: i32,
    /// `ParamUsage` as stored.
    pub usage: i32,
    /// `ParamFlags` as stored.
    pub flags: i32,
    /// `ParamActive` of the set.
    pub active: bool,
}

/// A shader constant of a material. `f` / `i` hold the stored values (`size / 4` entries) and
/// are empty when the engine fills the constant (no stored data).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConstParam {
    pub name: String,
    pub semantic: i32,
    pub reg: i32,
    /// Bytes.
    pub size: u16,
    pub usage: i32,
    pub flags: i32,
    pub f: Vec<f32>,
    pub i: Vec<i32>,
    pub active: bool,
}

/// One `ParameterSetN`: a constant buffer or a texture group.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ParamSet {
    /// `ParamBlock` name (`BuildingGlobal`, `BuildingTextures`, ...); `#xxxxxxxx` if unreadable.
    pub blocks: Vec<String>,
    pub textures: Vec<TexParam>,
    pub constants: Vec<ConstParam>,
}

/// The material of a mesh, as the FGX block of the Granny material describes it.
/// `render-core` picks its shader family by `package`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MaterialDesc {
    /// `ShaderSet`: `UnitShader_Skinned`, `BuildingShader`, ...; `#xxxxxxxx` if unreadable.
    pub package: String,
    /// FGX version (0, 2, 3 or 4).
    pub version: u8,
    /// `MaterialDesc::SKINNED` and the other flag bits.
    pub flags: u16,
    /// `AlphaMode`, `ZMode`, `AlphaRef`: the three numbers of `CopyAlphaZStateData`.
    pub alpha_z: [u32; 3],
    /// `SkinBoneCount`.
    pub skin_bones: u8,
    /// `FallbackMtl`.
    pub fallback: Option<String>,
    /// `ParameterSet0…3` by index; a missing one in between is empty, missing trailing ones are
    /// dropped.
    pub sets: Vec<ParamSet>,
    /// Name of the material in the gr2, when it is not stripped.
    pub source_name: Option<String>,
}

impl MaterialDesc {
    pub const SKINNED: u16 = 1 << 0;
    pub const SHADOWS: u16 = 1 << 1;
    pub const REFLECTIONS: u16 = 1 << 2;
    pub const ALPHA_SORT: u16 = 1 << 3;
    pub const INVISIBLE: u16 = 1 << 4;
    pub const ZFAIL: u16 = 1 << 5;
    pub const USE_FALLBACK: u16 = 1 << 6;
    pub const HAS_PARAMS: u16 = 1 << 7;

    pub fn has(&self, flag: u16) -> bool {
        self.flags & flag != 0
    }

    /// The texture with the sampler `name` in any set.
    pub fn texture(&self, name: &str) -> Option<&TexParam> {
        self.sets
            .iter()
            .flat_map(|s| &s.textures)
            .find(|t| t.name == name)
    }

    /// The constant `name` in any set.
    pub fn constant(&self, name: &str) -> Option<&ConstParam> {
        self.sets
            .iter()
            .flat_map(|s| &s.constants)
            .find(|c| c.name == name)
    }

    /// Every `File` texture id, in set and parameter order (repeats kept).
    pub fn texture_files(&self) -> impl Iterator<Item = &LogicalId> {
        self.sets
            .iter()
            .flat_map(|s| &s.textures)
            .filter_map(|t| match &t.src {
                TexSource::File(id) => Some(id),
                _ => None,
            })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnimEvent {
    /// Seconds from clip start.
    pub time: f32,
    pub code: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnimRef {
    pub name: String,
    /// Logical id of the [`crate::AnimationBlob`].
    pub blob: LogicalId,
    pub events: Vec<AnimEvent>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelBlob {
    pub skeleton: Skeleton,
    pub meshes: Vec<Mesh>,
    pub materials: Vec<Material>,
    pub animations: Vec<AnimRef>,
}

impl ModelBlob {
    pub const VERSION: u16 = 1;
    pub const MIN_VERSION: u16 = 1;

    pub fn encode(&self) -> Vec<u8> {
        blob::encode_v(MODEL_MAGIC, Self::VERSION, self)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let (_, m): (u16, Self) =
            blob::decode_range(MODEL_MAGIC, Self::MIN_VERSION, Self::VERSION, bytes)?;
        m.skeleton.validate()?;
        for (i, mesh) in m.meshes.iter().enumerate() {
            mesh.validate(i, m.materials.len(), m.skeleton.bones.len())?;
        }
        Ok(m)
    }
}

// ---------------------------------------------------------------- C5MD v2
//
// The v1 types above keep their names: the B1 client reads them. The v2 model is `ModelBlobV2`
// (the spec's `ModelBlob`); it reads v1 bytes too and converts them in memory (AB-D1).

/// A D3D vertex semantic; the same table as `render-core` (`shaders/common/semantics.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Semantic {
    Position0,
    Normal0,
    /// `TEXCOORD0..7`.
    Texcoord(u8),
    /// `COLOR0..1`.
    Color(u8),
    Tangent0,
    Binormal0,
    BlendIndices0,
    BlendWeight0,
}

/// Storage of one vertex attribute (little endian).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AttrFormat {
    F32x3,
    /// Octahedral normal, snorm16 x2 ([`crate::oct_encode`]).
    OctSnorm16x2,
    /// IEEE binary16 x2.
    F16x2,
    F32x2,
    U8x4,
    U16x4,
    Unorm8x4,
    Snorm16x4,
}

impl AttrFormat {
    /// Bytes in the vertex row.
    pub const fn size(self) -> usize {
        match self {
            Self::F32x3 => 12,
            Self::OctSnorm16x2 | Self::F16x2 | Self::U8x4 | Self::Unorm8x4 => 4,
            Self::F32x2 | Self::U16x4 | Self::Snorm16x4 => 8,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VertexAttr {
    pub semantic: Semantic,
    pub format: AttrFormat,
}

/// Triangle indices as little-endian bytes: u16 each, or u32 each when `wide`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackedIndices {
    pub wide: bool,
    pub data: Vec<u8>,
}

impl PackedIndices {
    /// Packs `indices`; u16 when every index fits.
    pub fn from_u32(indices: &[u32]) -> Self {
        let wide = indices.iter().any(|&i| i > u32::from(u16::MAX));
        let mut data = Vec::with_capacity(indices.len() * if wide { 4 } else { 2 });
        for &i in indices {
            if wide {
                data.extend_from_slice(&i.to_le_bytes());
            } else {
                data.extend_from_slice(&(i as u16).to_le_bytes());
            }
        }
        Self { wide, data }
    }

    pub fn width(&self) -> usize {
        if self.wide { 4 } else { 2 }
    }

    /// Number of indices (a partial trailing element is not counted; `validate` rejects it).
    pub fn len(&self) -> usize {
        self.data.len() / self.width()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = u32> + '_ {
        let w = self.width();
        self.data.chunks_exact(w).map(move |c| {
            if w == 4 {
                u32::from_le_bytes([c[0], c[1], c[2], c[3]])
            } else {
                u32::from(u16::from_le_bytes([c[0], c[1]]))
            }
        })
    }

    pub fn to_u32(&self) -> Vec<u32> {
        self.iter().collect()
    }
}

/// A model of the gr2 file (`Model` object): meshes `first_mesh..first_mesh + mesh_count` of the
/// blob. City catalogs keep one part per building.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Part {
    pub name: String,
    pub first_mesh: u16,
    pub mesh_count: u16,
    /// `InitialPlacement` of the gr2 model in blob axes (identity when the file has none).
    pub placement: Transform,
}

/// [`MeshV2::states`] of a mesh drawn in every node state.
pub const STATES_ALL: u8 = 0xff;
/// The intact state (the desired-state byte of a unit with no damage, and of every other object).
pub const STATE_INTACT: u8 = 1;

/// The state bits of a v2 (version 2) damage label, for the files written before the masks:
/// 0 drawn always, 1 intact hull, 2 light damage (levels 1 to 3), 3 heavy damage (levels 2, 3).
fn states_of_v2_label(label: u8) -> u8 {
    match label {
        1 => STATE_INTACT,
        2 => 0b1110,
        3 => 0b1100,
        _ => STATES_ALL,
    }
}

/// A mesh with packed vertices.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeshV2 {
    pub name: String,
    /// Index into [`ModelBlobV2::parts`].
    pub part: u16,
    /// Index into [`ModelBlobV2::materials`].
    pub material: u16,
    /// Attributes of a vertex row, in order; the row has no padding.
    pub layout: Vec<VertexAttr>,
    pub vertex_count: u32,
    /// `vertex_count` rows of the layout, little endian.
    pub vertices: Vec<u8>,
    pub indices: PackedIndices,
    /// Rigid mesh on an animated bone: skin every vertex to bone `k` (no joints / weights).
    pub bind: Option<u16>,
    /// The node states the mesh is drawn in: bit `k` is the desired-state bit `k` of the engine
    /// (`FGXModelNodeStateSorter`: a node shows when `desired & mask != 0`). A ship's intact hull
    /// has bit 0, its damaged hull the bits of the damage levels; [`STATES_ALL`] for the rest.
    // re: FGXModelNode +0x04 (answers-b update 9, RW-14 b); read from the gr2 by `granny::mesh::state_mask`.
    pub states: u8,
    /// Only `Raw` is read by this build.
    pub codec: crate::Codec,
}

/// The unpacked arrays [`MeshV2::pack`] takes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MeshArrays {
    pub positions: Vec<[f32; 3]>,
    /// Unit vectors.
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Empty for rigid meshes.
    pub joints: Vec<[u16; 4]>,
    /// Unorm8, empty for rigid meshes.
    pub weights: Vec<[u8; 4]>,
    pub indices: Vec<u32>,
}

/// UVs with an absolute value below this are stored as f16: the rounding error of f16 is
/// `2^-11` per unit step below 2, i.e. at most 1/2048 (card R2.1a); larger ones stay f32.
// The format note says "outside +-4"; the requirement asks for a 1/2048 bound, which f16 gives only
// below 2.
pub const UV_F16_LIMIT: f32 = 2.0;

impl MeshV2 {
    /// Row size in bytes, `None` on overflow.
    pub fn stride(&self) -> Option<usize> {
        self.layout
            .iter()
            .try_fold(0usize, |a, at| a.checked_add(at.format.size()))
    }

    /// Byte offset of the attribute `semantic` in a row.
    pub fn offset_of(&self, semantic: Semantic) -> Option<(usize, AttrFormat)> {
        let mut off = 0;
        for a in &self.layout {
            if a.semantic == semantic {
                return Some((off, a.format));
            }
            off += a.format.size();
        }
        None
    }

    pub fn has(&self, semantic: Semantic) -> bool {
        self.layout.iter().any(|a| a.semantic == semantic)
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// Packs arrays into the default layout (4.8): position f32x3, normal oct16, UV f16 (f32 when
    /// a UV is not below [`UV_F16_LIMIT`]), and for skinned meshes joints u8x4 (u16x4 when
    /// `wide_joints`) and unorm8 weights. Lengths must agree; the caller owns `bind`, `damage`.
    pub fn pack(
        name: impl Into<String>,
        part: u16,
        material: u16,
        a: &MeshArrays,
        wide_joints: bool,
        uv_f32: bool,
    ) -> Result<Self, FormatError> {
        let n = a.positions.len();
        let skinned = !a.joints.is_empty();
        if a.normals.len() != n
            || a.uvs.len() != n
            || (skinned && (a.joints.len() != n || a.weights.len() != n))
            || (!skinned && !a.weights.is_empty())
        {
            return Err(FormatError::invalid("mesh arrays differ in length"));
        }
        let vertex_count =
            u32::try_from(n).map_err(|_| FormatError::invalid("mesh has too many vertices"))?;
        let mut layout = vec![
            VertexAttr {
                semantic: Semantic::Position0,
                format: AttrFormat::F32x3,
            },
            VertexAttr {
                semantic: Semantic::Normal0,
                format: AttrFormat::OctSnorm16x2,
            },
            VertexAttr {
                semantic: Semantic::Texcoord(0),
                format: if uv_f32 {
                    AttrFormat::F32x2
                } else {
                    AttrFormat::F16x2
                },
            },
        ];
        if skinned {
            layout.push(VertexAttr {
                semantic: Semantic::BlendIndices0,
                format: if wide_joints {
                    AttrFormat::U16x4
                } else {
                    AttrFormat::U8x4
                },
            });
            layout.push(VertexAttr {
                semantic: Semantic::BlendWeight0,
                format: AttrFormat::Unorm8x4,
            });
        }
        let stride: usize = layout.iter().map(|l| l.format.size()).sum();
        let mut vertices = Vec::with_capacity(stride * n);
        for i in 0..n {
            for c in a.positions[i] {
                vertices.extend_from_slice(&c.to_le_bytes());
            }
            for c in crate::oct_encode(a.normals[i]) {
                vertices.extend_from_slice(&c.to_le_bytes());
            }
            for c in a.uvs[i] {
                if uv_f32 {
                    vertices.extend_from_slice(&c.to_le_bytes());
                } else {
                    vertices.extend_from_slice(&crate::f32_to_f16_bits(c).to_le_bytes());
                }
            }
            if skinned {
                for j in a.joints[i] {
                    if wide_joints {
                        vertices.extend_from_slice(&j.to_le_bytes());
                    } else {
                        vertices.push(
                            u8::try_from(j).map_err(|_| {
                                FormatError::invalid("joint above 255 in a u8 layout")
                            })?,
                        );
                    }
                }
                vertices.extend_from_slice(&a.weights[i]);
            }
        }
        Ok(Self {
            name: name.into(),
            part,
            material,
            layout,
            vertex_count,
            vertices,
            indices: PackedIndices::from_u32(&a.indices),
            bind: None,
            states: STATES_ALL,
            codec: crate::Codec::Raw,
        })
    }

    /// Whether UVs in `uvs` need the f32 layout.
    pub fn uvs_need_f32(uvs: &[[f32; 2]]) -> bool {
        uvs.iter()
            .flatten()
            .any(|&c| !c.is_finite() || c.abs() >= UV_F16_LIMIT)
    }

    /// Rows of the vertex bytes; none when the row size is zero or overflows.
    fn rows(&self) -> impl Iterator<Item = &[u8]> {
        let stride = self.stride().filter(|&s| s > 0);
        self.vertices
            .chunks_exact(stride.unwrap_or(usize::MAX).max(1))
            .take(if stride.is_some() { usize::MAX } else { 0 })
    }

    fn column<T>(
        &self,
        semantic: Semantic,
        want: &[AttrFormat],
        read: impl Fn(AttrFormat, &[u8]) -> T,
    ) -> Option<Vec<T>> {
        let (off, fmt) = self.offset_of(semantic)?;
        if !want.contains(&fmt) {
            return None;
        }
        let size = fmt.size();
        Some(
            self.rows()
                .map(|r| read(fmt, &r[off..off + size]))
                .collect(),
        )
    }

    pub fn positions(&self) -> Option<Vec<[f32; 3]>> {
        self.column(Semantic::Position0, &[AttrFormat::F32x3], |_, b| {
            std::array::from_fn(|k| {
                f32::from_le_bytes([b[4 * k], b[4 * k + 1], b[4 * k + 2], b[4 * k + 3]])
            })
        })
    }

    /// Decoded unit normals.
    pub fn normals(&self) -> Option<Vec<[f32; 3]>> {
        self.column(Semantic::Normal0, &[AttrFormat::OctSnorm16x2], |_, b| {
            crate::oct_decode([
                i16::from_le_bytes([b[0], b[1]]),
                i16::from_le_bytes([b[2], b[3]]),
            ])
        })
    }

    /// `TEXCOORD0` as f32 whatever the storage.
    pub fn uvs(&self) -> Option<Vec<[f32; 2]>> {
        self.column(
            Semantic::Texcoord(0),
            &[AttrFormat::F16x2, AttrFormat::F32x2],
            |f, b| {
                if f == AttrFormat::F32x2 {
                    std::array::from_fn(|k| {
                        f32::from_le_bytes([b[4 * k], b[4 * k + 1], b[4 * k + 2], b[4 * k + 3]])
                    })
                } else {
                    std::array::from_fn(|k| {
                        crate::f16_bits_to_f32(u16::from_le_bytes([b[2 * k], b[2 * k + 1]]))
                    })
                }
            },
        )
    }

    pub fn joints(&self) -> Option<Vec<[u16; 4]>> {
        self.column(
            Semantic::BlendIndices0,
            &[AttrFormat::U8x4, AttrFormat::U16x4],
            |f, b| {
                if f == AttrFormat::U16x4 {
                    std::array::from_fn(|k| u16::from_le_bytes([b[2 * k], b[2 * k + 1]]))
                } else {
                    std::array::from_fn(|k| u16::from(b[k]))
                }
            },
        )
    }

    pub fn weights(&self) -> Option<Vec<[u8; 4]>> {
        self.column(Semantic::BlendWeight0, &[AttrFormat::Unorm8x4], |_, b| {
            [b[0], b[1], b[2], b[3]]
        })
    }

    /// Checks the sizes and ranges a reader relies on (no allocation proportional to claimed
    /// sizes, checked arithmetic).
    fn validate(
        &self,
        index: usize,
        parts: usize,
        materials: usize,
        bones: usize,
    ) -> Result<(), FormatError> {
        let bad = |what: &str| Err(FormatError::invalid(format!("mesh {index}: {what}")));
        if self.codec != crate::Codec::Raw {
            return bad("only Raw vertex sections are read by this build");
        }
        let Some(stride) = self.stride() else {
            return bad("row size overflows");
        };
        let Some(total) = stride.checked_mul(self.vertex_count as usize) else {
            return bad("vertex bytes overflow");
        };
        if self.vertices.len() != total {
            return bad("vertex bytes do not match the layout and the vertex count");
        }
        if usize::from(self.part) >= parts {
            return bad("part out of range");
        }
        if usize::from(self.material) >= materials {
            return bad("material out of range");
        }
        if self.bind.is_some_and(|b| usize::from(b) >= bones) {
            return bad("bind bone out of range");
        }
        if self.bind.is_some() && self.has(Semantic::BlendIndices0) {
            return bad("a bound mesh has no joints");
        }
        let w = self.indices.width();
        if !self.indices.data.len().is_multiple_of(w) || !self.indices.len().is_multiple_of(3) {
            return bad("index bytes are not whole triangles");
        }
        if self.indices.iter().any(|i| i >= self.vertex_count) {
            return bad("index out of range");
        }
        if let Some(j) = self.joints() {
            if bones == 0 && !j.is_empty() {
                return bad("joints without a skeleton");
            }
            if j.iter().flatten().any(|&j| usize::from(j) >= bones) {
                return bad("joint index out of range");
            }
        }
        Ok(())
    }
}

/// Which clip a [`StateClip`] plays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClipKey {
    Ec(u32),
    Name(String),
}

/// One row of a landmark decision tree (`.ddtxml`): in `state` play `clip`, `repeat` times, then
/// wait `pause` seconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateClip {
    pub state: String,
    pub clip: ClipKey,
    pub repeat: u16,
    pub pause: f32,
}

/// A clip of the model's `an:` pack.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClipRef {
    pub name: String,
    /// Event code of the fxsxml (`1000` IdleA, ...); `0` when the fxsxml gives none.
    pub ec: u32,
    /// Index in the [`crate::AnimPack`] of [`ModelBlobV2::anim_pack`].
    pub index: u16,
    pub events: Vec<AnimEvent>,
    /// Only on a model converted from v1: the clip is the separate `C5AN` blob with this id.
    pub legacy_blob: Option<LogicalId>,
}

/// Reserved for the `.ftsxml` visual-effect timers (filled with particles stage 1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectRef {
    pub name: String,
}

/// `C5MD` v2: one blob per `.fxsxml`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelBlobV2 {
    /// `.fxsxml` name without extension.
    pub source: String,
    pub skeleton: Skeleton,
    pub parts: Vec<Part>,
    pub meshes: Vec<MeshV2>,
    pub materials: Vec<MaterialDesc>,
    pub clips: Vec<ClipRef>,
    pub anim_pack: Option<LogicalId>,
    pub fsm: Option<LogicalId>,
    pub state_clips: Vec<StateClip>,
    pub effects: Vec<EffectRef>,
    /// Model-space box of all vertices: min xyz, max xyz.
    pub bounds: [f32; 6],
}

impl ModelBlobV2 {
    /// 3: [`MeshV2::states`] is the node state mask of the gr2 (2 had a damage label there).
    pub const VERSION: u16 = 3;
    pub const MIN_VERSION: u16 = 1;

    pub fn encode(&self) -> Vec<u8> {
        blob::encode_v(MODEL_MAGIC, Self::VERSION, self)
    }

    /// Reads v3, v2 (its damage labels as state masks) and v1 converted in memory
    /// ([`Self::from_v1`]).
    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let (version, body) =
            blob::split_range(MODEL_MAGIC, Self::MIN_VERSION, Self::VERSION, bytes)?;
        let m = if version == 1 {
            let v1: ModelBlob = blob::payload(body)?;
            v1.skeleton.validate()?;
            for (i, mesh) in v1.meshes.iter().enumerate() {
                mesh.validate(i, v1.materials.len(), v1.skeleton.bones.len())?;
            }
            Self::from_v1(&v1)?
        } else {
            let mut m = blob::payload::<Self>(body)?;
            if version == 2 {
                for mesh in &mut m.meshes {
                    mesh.states = states_of_v2_label(mesh.states);
                }
            }
            m
        };
        m.validate()?;
        Ok(m)
    }

    /// A v1 model as v2: one part, default layouts, `MaterialDesc` with the two v1 textures,
    /// clips by `legacy_blob`.
    pub fn from_v1(v1: &ModelBlob) -> Result<Self, FormatError> {
        let mut meshes = Vec::with_capacity(v1.meshes.len());
        for (i, m) in v1.meshes.iter().enumerate() {
            let skinned = !m.joints.is_empty();
            let mut vertices = Vec::new();
            let mut layout = vec![
                VertexAttr {
                    semantic: Semantic::Position0,
                    format: AttrFormat::F32x3,
                },
                VertexAttr {
                    semantic: Semantic::Normal0,
                    format: AttrFormat::OctSnorm16x2,
                },
                VertexAttr {
                    semantic: Semantic::Texcoord(0),
                    format: AttrFormat::F16x2,
                },
            ];
            if skinned {
                layout.push(VertexAttr {
                    semantic: Semantic::BlendIndices0,
                    format: AttrFormat::U8x4,
                });
                layout.push(VertexAttr {
                    semantic: Semantic::BlendWeight0,
                    format: AttrFormat::Unorm8x4,
                });
            }
            for v in 0..m.positions.len() {
                for c in m.positions[v] {
                    vertices.extend_from_slice(&c.to_le_bytes());
                }
                for c in m.normals[v] {
                    vertices.extend_from_slice(&c.to_le_bytes());
                }
                for c in m.uvs[v] {
                    vertices.extend_from_slice(&c.to_le_bytes());
                }
                if skinned {
                    vertices.extend_from_slice(&m.joints[v]);
                    vertices.extend_from_slice(&m.weights[v]);
                }
            }
            let idx: Vec<u32> = match &m.indices {
                Indices::U16(v) => v.iter().map(|&i| u32::from(i)).collect(),
                Indices::U32(v) => v.clone(),
            };
            meshes.push(MeshV2 {
                name: format!("mesh{i}"),
                part: 0,
                material: m.material,
                layout,
                vertex_count: u32::try_from(m.positions.len())
                    .map_err(|_| FormatError::invalid("mesh has too many vertices"))?,
                vertices,
                indices: PackedIndices::from_u32(&idx),
                bind: None,
                states: STATES_ALL,
                codec: crate::Codec::Raw,
            });
        }
        let tex = |name: &str, id: &Option<LogicalId>| {
            id.clone().map(|id| TexParam {
                name: name.to_owned(),
                kind: TexKind::Tex2D,
                src: TexSource::File(id),
                semantic: -1,
                reg: 0,
                usage: 0,
                flags: 0,
                active: true,
            })
        };
        let materials = v1
            .materials
            .iter()
            .map(|m| MaterialDesc {
                package: "legacy-b1".to_owned(),
                sets: vec![ParamSet {
                    blocks: vec!["legacy-b1".to_owned()],
                    textures: [tex("Diffuse", &m.diffuse), tex("Specular", &m.specular)]
                        .into_iter()
                        .flatten()
                        .collect(),
                    constants: Vec::new(),
                }],
                ..MaterialDesc::default()
            })
            .collect();
        let clips = v1
            .animations
            .iter()
            .enumerate()
            .map(|(i, a)| ClipRef {
                name: a.name.clone(),
                ec: a.events.first().map_or(0, |e| e.code),
                index: i as u16,
                events: a.events.clone(),
                legacy_blob: Some(a.blob.clone()),
            })
            .collect();
        let part = Part {
            name: "model".to_owned(),
            first_mesh: 0,
            mesh_count: u16::try_from(meshes.len())
                .map_err(|_| FormatError::invalid("too many meshes"))?,
            placement: Transform::IDENTITY,
        };
        let mut out = Self {
            source: String::new(),
            skeleton: v1.skeleton.clone(),
            parts: vec![part],
            meshes,
            materials,
            clips,
            ..Self::default()
        };
        out.bounds = out.compute_bounds();
        Ok(out)
    }

    /// The model as a rigid v1 model, for the readers of B1's static models (the trees of `tb:art-b2`
    /// go through B1's forest): one v1 mesh per v2 mesh, in order, with the placement of its part
    /// applied; the diffuse of a material is its `BaseTextureMap`, `BaseSampler` or `Diffuse`, else
    /// its first file. The skeleton, the joints and the clips are left out. A mesh without positions,
    /// normals or uvs in a layout this reads is an error.
    pub fn to_static_v1(&self) -> Result<ModelBlob, FormatError> {
        let mut meshes = Vec::with_capacity(self.meshes.len());
        for (i, m) in self.meshes.iter().enumerate() {
            let bad = |what: &str| FormatError::invalid(format!("mesh {i}: {what}"));
            let positions = m.positions().ok_or_else(|| bad("no f32 positions"))?;
            let normals = m.normals().ok_or_else(|| bad("no octahedral normals"))?;
            let uvs = m.uvs().ok_or_else(|| bad("no uvs"))?;
            let place = self
                .parts
                .get(usize::from(m.part))
                .map_or(Mat4::IDENTITY, |p| p.placement.to_mat4());
            let turn = place.inverse().transpose();
            meshes.push(Mesh {
                positions: positions
                    .iter()
                    .map(|p| place.transform_point3(Vec3::from(*p)).to_array())
                    .collect(),
                normals: normals
                    .iter()
                    .map(|n| {
                        crate::oct_encode(
                            turn.transform_vector3(Vec3::from(*n))
                                .normalize_or_zero()
                                .to_array(),
                        )
                    })
                    .collect(),
                uvs: uvs
                    .iter()
                    .map(|uv| [crate::f32_to_f16_bits(uv[0]), crate::f32_to_f16_bits(uv[1])])
                    .collect(),
                joints: Vec::new(),
                weights: Vec::new(),
                indices: Indices::U32(m.indices.to_u32()),
                material: m.material,
            });
        }
        let file = |desc: &MaterialDesc, names: &[&str]| {
            names
                .iter()
                .find_map(|n| match desc.texture(n).map(|t| &t.src) {
                    Some(TexSource::File(id)) => Some(id.clone()),
                    _ => None,
                })
        };
        let materials = self
            .materials
            .iter()
            .map(|d| Material {
                diffuse: file(d, &["BaseTextureMap", "BaseSampler", "Diffuse"])
                    .or_else(|| d.texture_files().next().cloned()),
                specular: file(d, &["Specular", "SREFMap", "BuildingSREF"]),
            })
            .collect();
        Ok(ModelBlob {
            skeleton: Skeleton::default(),
            meshes,
            materials,
            animations: Vec::new(),
        })
    }

    /// Box of all vertex positions; zeros without vertices.
    pub fn compute_bounds(&self) -> [f32; 6] {
        let mut b = [
            f32::INFINITY,
            f32::INFINITY,
            f32::INFINITY,
            -f32::INFINITY,
            -f32::INFINITY,
            -f32::INFINITY,
        ];
        let mut any = false;
        for m in &self.meshes {
            for p in m.positions().unwrap_or_default() {
                any = true;
                for k in 0..3 {
                    b[k] = b[k].min(p[k]);
                    b[3 + k] = b[3 + k].max(p[k]);
                }
            }
        }
        if any { b } else { [0.0; 6] }
    }

    pub fn triangle_count(&self) -> usize {
        self.meshes.iter().map(MeshV2::triangle_count).sum()
    }

    /// The structural checks of [`Self::decode`].
    pub fn validate(&self) -> Result<(), FormatError> {
        self.skeleton.validate()?;
        if self.bounds.iter().any(|b| !b.is_finite()) {
            return Err(FormatError::invalid("bounds are not finite"));
        }
        for (i, m) in self.meshes.iter().enumerate() {
            m.validate(
                i,
                self.parts.len(),
                self.materials.len(),
                self.skeleton.bones.len(),
            )?;
        }
        for (p, part) in self.parts.iter().enumerate() {
            let end = usize::from(part.first_mesh) + usize::from(part.mesh_count);
            if end > self.meshes.len() {
                return Err(FormatError::invalid(format!(
                    "part {p} reaches past the meshes"
                )));
            }
            if self.meshes[usize::from(part.first_mesh)..end]
                .iter()
                .any(|m| usize::from(m.part) != p)
            {
                return Err(FormatError::invalid(format!(
                    "part {p} covers a mesh of another part"
                )));
            }
        }
        if let Some(m) = self.meshes.iter().enumerate().find(|(i, m)| {
            let p = &self.parts[usize::from(m.part)];
            !(usize::from(p.first_mesh)..usize::from(p.first_mesh) + usize::from(p.mesh_count))
                .contains(i)
        }) {
            return Err(FormatError::invalid(format!(
                "mesh {} is outside the range of its part",
                m.0
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bone(name: &str, parent: Option<u16>, t: [f32; 3]) -> Bone {
        Bone {
            name: name.into(),
            parent,
            rest: Transform {
                translation: t,
                ..Transform::IDENTITY
            },
            inverse_bind: Mat4::from_translation(Vec3::from_array(t).recip()).to_cols_array(),
        }
    }

    pub(crate) fn sample_model() -> ModelBlob {
        ModelBlob {
            skeleton: Skeleton {
                bones: vec![
                    bone("root", None, [0.0, 1.0, 0.0]),
                    bone("spine", Some(0), [0.0, 2.0, 0.0]),
                ],
            },
            meshes: vec![
                Mesh {
                    positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                    normals: vec![[0, 0], [100, -100], [-32767, 32767]],
                    uvs: vec![[0x3c00, 0], [0, 0x3c00], [0x3800, 0x3800]],
                    joints: vec![[0, 1, 0, 0]; 3],
                    weights: vec![[200, 55, 0, 0]; 3],
                    indices: Indices::U16(vec![0, 1, 2]),
                    material: 0,
                },
                Mesh {
                    positions: vec![[0.0; 3]; 4],
                    normals: vec![[0, 0]; 4],
                    uvs: vec![[0, 0]; 4],
                    joints: vec![],
                    weights: vec![],
                    indices: Indices::U32(vec![0, 1, 2, 2, 1, 3]),
                    material: 1,
                },
            ],
            materials: vec![
                Material {
                    diffuse: Some(LogicalId::new("tex.diffuse")),
                    specular: None,
                },
                Material::default(),
            ],
            animations: vec![AnimRef {
                name: "idle".into(),
                blob: LogicalId::new("anim.idle"),
                events: vec![AnimEvent { time: 0.5, code: 7 }],
            }],
        }
    }

    #[test]
    fn roundtrip() {
        let m = sample_model();
        assert_eq!(ModelBlob::decode(&m.encode()).unwrap(), m);
    }

    fn sample_desc() -> MaterialDesc {
        MaterialDesc {
            package: "BuildingShader_Skinned".into(),
            version: 4,
            flags: MaterialDesc::SKINNED | MaterialDesc::SHADOWS | MaterialDesc::HAS_PARAMS,
            alpha_z: [8, 4, 128],
            skin_bones: 32,
            fallback: None,
            sets: vec![
                ParamSet {
                    blocks: vec!["BuildingGlobal".into()],
                    textures: vec![],
                    constants: vec![ConstParam {
                        name: "f3FogColor".into(),
                        semantic: -1,
                        reg: -65531,
                        size: 16,
                        usage: 0,
                        flags: 8,
                        f: vec![0.5, 0.25, 0.0, 1.0],
                        i: vec![],
                        active: true,
                    }],
                },
                ParamSet {
                    blocks: vec!["BuildingTextures".into()],
                    textures: vec![
                        TexParam {
                            name: "Diffuse".into(),
                            kind: TexKind::Tex2D,
                            src: TexSource::File(LogicalId::new("tx:cow_diff.dds")),
                            semantic: 42,
                            reg: -65536,
                            usage: 0,
                            flags: 8,
                            active: true,
                        },
                        TexParam {
                            name: "ColorCorrection".into(),
                            kind: TexKind::Tex3D,
                            src: TexSource::Engine,
                            semantic: -1,
                            reg: -65530,
                            usage: 0,
                            flags: 8,
                            active: true,
                        },
                        TexParam {
                            name: "#12345678".into(),
                            kind: TexKind::Cube,
                            src: TexSource::Unresolved(0xdead_beef),
                            semantic: -1,
                            reg: 0,
                            usage: 0,
                            flags: 0,
                            active: false,
                        },
                    ],
                    constants: vec![],
                },
            ],
            source_name: Some("Game Cow 01".into()),
        }
    }

    #[test]
    fn material_desc_roundtrips_and_looks_up_by_name() {
        let d = sample_desc();
        let bytes = blob::encode_v(*b"TEST", 1, &d);
        let (_, back): (u16, MaterialDesc) = blob::decode_range(*b"TEST", 1, 1, &bytes).unwrap();
        assert_eq!(back, d);
        assert!(d.has(MaterialDesc::SKINNED) && !d.has(MaterialDesc::ZFAIL));
        assert_eq!(d.texture("ColorCorrection").unwrap().src, TexSource::Engine);
        assert_eq!(d.constant("f3FogColor").unwrap().f.len(), 4);
        assert!(d.texture("Missing").is_none());
        let files: Vec<_> = d.texture_files().map(LogicalId::as_str).collect();
        assert_eq!(files, ["tx:cow_diff.dds"]);
    }

    #[test]
    fn rejects_bad_magic_and_version() {
        let bytes = sample_model().encode();
        let mut bad = bytes.clone();
        bad[0] = b'X';
        assert!(matches!(
            ModelBlob::decode(&bad),
            Err(FormatError::BadMagic { .. })
        ));
        let mut bad = bytes.clone();
        bad[4] = 0xff;
        assert!(matches!(
            ModelBlob::decode(&bad),
            Err(FormatError::UnsupportedVersion { .. })
        ));
        assert_eq!(ModelBlob::decode(&bytes[..3]), Err(FormatError::Truncated));
        let mut trailing = bytes;
        trailing.push(0);
        assert!(matches!(
            ModelBlob::decode(&trailing),
            Err(FormatError::TrailingBytes(1))
        ));
    }

    #[test]
    fn rejects_parent_after_child() {
        let mut m = sample_model();
        m.skeleton.bones[0].parent = Some(1);
        assert!(matches!(
            ModelBlob::decode(&m.encode()),
            Err(FormatError::Invalid(_))
        ));
    }

    #[test]
    fn rejects_bad_mesh() {
        let mut m = sample_model();
        m.meshes[0].indices = Indices::U16(vec![0, 1, 9]);
        assert!(ModelBlob::decode(&m.encode()).is_err());
        let mut m = sample_model();
        m.meshes[0].material = 5;
        assert!(ModelBlob::decode(&m.encode()).is_err());
        let mut m = sample_model();
        m.meshes[0].joints[0][0] = 9;
        assert!(ModelBlob::decode(&m.encode()).is_err());
        let mut m = sample_model();
        m.meshes[0].uvs.pop();
        assert!(ModelBlob::decode(&m.encode()).is_err());
    }

    #[test]
    fn world_and_skin_matrices_on_chain() {
        let t = |x: f32| Transform {
            translation: [x, 0.0, 0.0],
            ..Transform::IDENTITY
        };
        let bones = (0..3u16)
            .map(|i| Bone {
                name: format!("b{i}"),
                parent: i.checked_sub(1),
                rest: t(1.0),
                inverse_bind: Mat4::from_translation(Vec3::new(-(f32::from(i) + 1.0), 0.0, 0.0))
                    .to_cols_array(),
            })
            .collect();
        let sk = Skeleton { bones };
        let local = sk.rest_pose();
        let mut world = [Mat4::IDENTITY; 3];
        sk.world_matrices(&local, &mut world);
        for (i, w) in world.iter().enumerate() {
            assert_eq!(w.w_axis.truncate(), Vec3::new(i as f32 + 1.0, 0.0, 0.0));
        }
        let mut skin = [Mat4::ZERO; 3];
        sk.skin_matrices(&world, &mut skin);
        for m in skin {
            assert!(m.abs_diff_eq(Mat4::IDENTITY, 1e-6));
        }
    }

    #[test]
    fn world_matrices_apply_parent_rotation() {
        let sk = Skeleton {
            bones: vec![
                Bone {
                    name: "a".into(),
                    parent: None,
                    rest: Transform::IDENTITY,
                    inverse_bind: Mat4::IDENTITY.to_cols_array(),
                },
                Bone {
                    name: "b".into(),
                    parent: Some(0),
                    rest: Transform::IDENTITY,
                    inverse_bind: Mat4::IDENTITY.to_cols_array(),
                },
            ],
        };
        let q = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let local = [
            Transform {
                rotation: q.to_array(),
                ..Transform::IDENTITY
            },
            Transform {
                translation: [1.0, 0.0, 0.0],
                ..Transform::IDENTITY
            },
        ];
        let mut world = [Mat4::IDENTITY; 2];
        sk.world_matrices(&local, &mut world);
        assert!(
            world[1]
                .w_axis
                .truncate()
                .abs_diff_eq(Vec3::new(0.0, 1.0, 0.0), 1e-6)
        );
    }
}

#[cfg(test)]
#[path = "model_v2_tests.rs"]
mod v2_tests;
