//! Typed meshes (`granny_mesh`): vertex buffers decoded by the vertex type's member layout,
//! triangle indices and material groups.

use crate::{GrannyError, MemberKind, Object};

/// Resolves a bone binding's `(name, id)` to a skeleton bone index.
pub type BoneLookup<'a> = dyn Fn(Option<&str>, Option<u32>) -> Option<usize> + 'a;

/// A run of triangles drawn with one material.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Group {
    pub material_index: u32,
    pub tri_first: u32,
    pub tri_count: u32,
}

/// One `BoneBindings` entry. Skinned vertices index this table, not the skeleton directly.
#[derive(Debug, Clone, PartialEq)]
pub struct BoneBinding {
    /// `None` in packed game files.
    pub name: Option<String>,
    /// Raw ID stored in place of the name in packed game files.
    pub name_id: Option<u32>,
    /// Skeleton bone this entry refers to (by name, else by ID).
    pub bone: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Mesh {
    pub name: Option<String>,
    pub positions: Vec<[f32; 3]>,
    pub normals: Option<Vec<[f32; 3]>>,
    pub tangents: Option<Vec<[f32; 3]>>,
    pub binormals: Option<Vec<[f32; 3]>>,
    /// Only the first two components of `TextureCoordinates0..` (Civ5 stores three).
    pub uv: Vec<Vec<[f32; 2]>>,
    pub bone_indices: Option<Vec<[u8; 4]>>,
    /// Raw u8 weights (sum to 255 for skinned vertices).
    pub bone_weights: Option<Vec<[u8; 4]>>,
    /// Triangle list.
    pub indices: Vec<u32>,
    pub groups: Vec<Group>,
    /// Index into the file's `Materials` for each `MaterialBindings` entry.
    pub material_bindings: Vec<Option<usize>>,
    pub bone_bindings: Vec<BoneBinding>,
    /// Vertex members that were skipped (unknown names or layouts).
    pub skipped_members: Vec<String>,
    /// The node states the mesh is drawn in ([`state_mask`]): bit `k` is the desired-state bit `k`
    /// of the engine's `FGXModelNodeStateSorter`. [`ALL_STATES`] for a mesh without the block.
    pub state_mask: u8,
}

impl Default for Mesh {
    fn default() -> Self {
        Self {
            name: None,
            positions: Vec::new(),
            normals: None,
            tangents: None,
            binormals: None,
            uv: Vec::new(),
            bone_indices: None,
            bone_weights: None,
            indices: Vec::new(),
            groups: Vec::new(),
            material_bindings: Vec::new(),
            bone_bindings: Vec::new(),
            skipped_members: Vec::new(),
            state_mask: ALL_STATES,
        }
    }
}

/// The state mask of a mesh drawn in every state.
pub const ALL_STATES: u8 = 0xff;

/// The member of a mesh's `ExtendedData` that lists the material of each node state (the name is
/// stripped in the game files; this is its hash).
// re: FGXModelNode +0x04 stateMask, tested by FGXModelNodeStateSorter::IsInCorrectState 0x08cf4532
// (answers-b update 9, RW-14 b); the exporter writes one `Material` slot per state bit.
pub const STATE_MATERIALS_HASH: u32 = 0x693a_e546;

/// The state mask of a mesh object: its `ExtendedData` holds an object with one `Material` slot per
/// state bit (eight in the files), and a slot that references a material shows the mesh in that
/// state; a null slot hides it. A ship's intact hull has bit 0 only, the damaged hull bits 1..7.
/// A mesh without the block is drawn in every state.
pub fn state_mask(mesh: &Object<'_>) -> Result<u8, GrannyError> {
    if mesh.type_def().member("ExtendedData").is_none() {
        return Ok(ALL_STATES);
    }
    let Some(ext) = mesh.reference("ExtendedData")? else {
        return Ok(ALL_STATES);
    };
    let block = ext
        .fields()
        .find(|(m, _)| m.name_hash == STATE_MATERIALS_HASH)
        .map(|(_, v)| v);
    let Some(crate::Value::Reference(r)) = block else {
        return Ok(ALL_STATES);
    };
    let Some(slots) = r.get()? else {
        return Ok(ALL_STATES);
    };
    let mut mask = 0u8;
    for (bit, (_, slot)) in slots.fields().take(8).enumerate() {
        let crate::Value::Reference(slot) = slot else {
            continue;
        };
        let Some(slot) = slot.get()? else { continue };
        let shown = slot
            .fields()
            .any(|(_, v)| matches!(v, crate::Value::Reference(m) if m.target().is_some()));
        if shown {
            mask |= 1 << bit;
        }
    }
    Ok(mask)
}

impl Mesh {
    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn is_skinned(&self) -> bool {
        self.bone_indices.is_some() && self.bone_weights.is_some()
    }

    /// Largest vertex bone index in use among vertices with non-zero weight.
    pub fn max_bone_index(&self) -> Option<u8> {
        let (i, w) = (self.bone_indices.as_ref()?, self.bone_weights.as_ref()?);
        i.iter()
            .zip(w)
            .flat_map(|(i, w)| i.iter().zip(w).filter(|(_, w)| **w > 0).map(|(i, _)| *i))
            .max()
    }
}

fn half_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = i32::from((h >> 10) & 0x1f);
    let man = f32::from(h & 0x3ff);
    match exp {
        0 => sign * man * 2f32.powi(-24),
        31 => {
            if man == 0.0 {
                sign * f32::INFINITY
            } else {
                f32::NAN
            }
        }
        _ => sign * (1.0 + man / 1024.0) * 2f32.powi(exp - 15),
    }
}

/// One scalar of a vertex member as f32; integers are normalised for `Normal*`/`Binormal*` kinds.
fn component(kind: MemberKind, b: &[u8]) -> Option<f32> {
    use MemberKind::*;
    let b1 = |i: usize| b.get(i).copied();
    Some(match kind {
        Real32 => f32::from_le_bytes([b1(0)?, b1(1)?, b1(2)?, b1(3)?]),
        Real16 => half_to_f32(u16::from_le_bytes([b1(0)?, b1(1)?])),
        UInt8 => f32::from(b1(0)?),
        Int8 => f32::from(b1(0)? as i8),
        NormalUInt8 => f32::from(b1(0)?) / 255.0,
        BinormalInt8 => (f32::from(b1(0)? as i8) / 127.0).max(-1.0),
        UInt16 => f32::from(u16::from_le_bytes([b1(0)?, b1(1)?])),
        Int16 => f32::from(i16::from_le_bytes([b1(0)?, b1(1)?])),
        NormalUInt16 => f32::from(u16::from_le_bytes([b1(0)?, b1(1)?])) / 65535.0,
        BinormalInt16 => (f32::from(i16::from_le_bytes([b1(0)?, b1(1)?])) / 32767.0).max(-1.0),
        _ => return None,
    })
}

fn is_float_kind(k: MemberKind) -> bool {
    use MemberKind::*;
    matches!(
        k,
        Real32 | Real16 | NormalUInt8 | BinormalInt8 | NormalUInt16 | BinormalInt16
    )
}

fn vec_n<const N: usize>(
    kind: MemberKind,
    elem: usize,
    width: usize,
    b: &[u8],
) -> Option<[f32; N]> {
    let mut out = [0.0f32; N];
    if width < N {
        return None;
    }
    for (i, o) in out.iter_mut().enumerate() {
        *o = component(kind, b.get(i * elem..)?)?;
    }
    Some(out)
}

fn u8x4(kind: MemberKind, elem: usize, width: usize, b: &[u8]) -> Option<[u8; 4]> {
    if width < 4 {
        return None;
    }
    let mut out = [0u8; 4];
    for (i, o) in out.iter_mut().enumerate() {
        let v = component(kind, b.get(i * elem..)?)?;
        *o = match kind {
            MemberKind::NormalUInt8 => (v * 255.0).round() as u8,
            _ => v as u8,
        };
    }
    Some(out)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Position,
    Normal,
    Tangent,
    Binormal,
    Uv(usize),
    Weights,
    Indices,
}

fn role_of(name: &str) -> Option<Role> {
    Some(match name {
        "Position" => Role::Position,
        "Normal" => Role::Normal,
        "Tangent" => Role::Tangent,
        "Binormal" => Role::Binormal,
        "BoneWeights" => Role::Weights,
        "BoneIndices" => Role::Indices,
        _ => Role::Uv(name.strip_prefix("TextureCoordinates")?.parse().ok()?),
    })
}

impl Mesh {
    /// Reads a mesh. `materials` are the file's material objects, used to resolve bindings by
    /// identity; `bone_lookup` maps a binding's name or ID to a skeleton bone.
    pub fn read(
        o: &Object<'_>,
        materials: &[Object<'_>],
        bone_lookup: &BoneLookup<'_>,
    ) -> Result<Mesh, GrannyError> {
        let mut mesh = Mesh {
            name: o.string("Name")?,
            state_mask: state_mask(o)?,
            ..Mesh::default()
        };
        // Meshes without vertex data exist (light placeholders); they stay empty.
        let Some(vd) = o.reference("PrimaryVertexData")? else {
            return Ok(mesh);
        };
        read_vertices(&vd, &mut mesh)?;
        if let Some(topo) = o.reference("PrimaryTopology")? {
            read_topology(&topo, &mut mesh)?;
        }
        if let Ok(mb) = o.array("MaterialBindings") {
            for b in mb.iter() {
                let target = b.reference("Material")?.map(|m| m.at());
                mesh.material_bindings
                    .push(target.and_then(|t| materials.iter().position(|m| m.at() == t)));
            }
        }
        if let Ok(bb) = o.array("BoneBindings") {
            for b in bb.iter() {
                let (name, name_id) = b.name_and_id("BoneName")?;
                let bone = bone_lookup(name.as_deref(), name_id);
                mesh.bone_bindings.push(BoneBinding {
                    name,
                    name_id,
                    bone,
                });
            }
        }
        Ok(mesh)
    }
}

/// Texture-coordinate set number and its values.
type UvSet = (usize, Vec<[f32; 2]>);

fn read_vertices(vd: &Object<'_>, mesh: &mut Mesh) -> Result<(), GrannyError> {
    let verts = vd.array("Vertices")?;
    let Some(ty) = verts.element_type().cloned() else {
        return Ok(());
    };
    let stride = ty.size as usize;
    let n = verts.len();
    let data = verts.bytes();
    let mut uv: Vec<UvSet> = Vec::new();
    let mut normals = None;
    let mut tangents = None;
    let mut binormals = None;
    let mut weights = None;
    let mut indices = None;
    let mut positions = None;
    // Per member: its role and output buffer, validated against the layout once.
    for m in &ty.members {
        let width = m.count() as usize;
        let elem = m.element_size as usize;
        let at = m.offset as usize;
        let supported = match role_of(&m.name) {
            Some(Role::Weights | Role::Indices) => {
                width >= 4
                    && matches!(
                        m.kind,
                        MemberKind::UInt8 | MemberKind::NormalUInt8 | MemberKind::Int8
                    )
            }
            Some(Role::Uv(_)) => width >= 2 && is_float_kind(m.kind),
            Some(_) => width >= 3 && is_float_kind(m.kind),
            None => false,
        };
        let Some(role) = role_of(&m.name).filter(|_| supported) else {
            mesh.skipped_members.push(m.name.to_string());
            continue;
        };
        let rows = || {
            data.chunks_exact(stride.max(1))
                .take(n)
                .map(move |r| r.get(at..).unwrap_or(&[]))
        };
        match role {
            Role::Position => {
                positions = Some(
                    rows()
                        .map(|r| vec_n::<3>(m.kind, elem, width, r).unwrap_or([0.0; 3]))
                        .collect::<Vec<_>>(),
                )
            }
            Role::Normal => {
                normals = Some(
                    rows()
                        .map(|r| vec_n::<3>(m.kind, elem, width, r).unwrap_or([0.0; 3]))
                        .collect::<Vec<_>>(),
                )
            }
            Role::Tangent => {
                tangents = Some(
                    rows()
                        .map(|r| vec_n::<3>(m.kind, elem, width, r).unwrap_or([0.0; 3]))
                        .collect::<Vec<_>>(),
                )
            }
            Role::Binormal => {
                binormals = Some(
                    rows()
                        .map(|r| vec_n::<3>(m.kind, elem, width, r).unwrap_or([0.0; 3]))
                        .collect::<Vec<_>>(),
                )
            }
            Role::Uv(k) => uv.push((
                k,
                rows()
                    .map(|r| vec_n::<2>(m.kind, elem, width, r).unwrap_or([0.0; 2]))
                    .collect(),
            )),
            Role::Weights => {
                weights = Some(
                    rows()
                        .map(|r| u8x4(m.kind, elem, width, r).unwrap_or([0; 4]))
                        .collect::<Vec<_>>(),
                )
            }
            Role::Indices => {
                indices = Some(
                    rows()
                        .map(|r| u8x4(m.kind, elem, width, r).unwrap_or([0; 4]))
                        .collect::<Vec<_>>(),
                )
            }
        }
    }
    mesh.positions =
        positions.ok_or_else(|| GrannyError::Invalid("vertex type has no Position".into()))?;
    mesh.normals = normals;
    mesh.tangents = tangents;
    mesh.binormals = binormals;
    mesh.bone_weights = weights;
    mesh.bone_indices = indices;
    uv.sort_by_key(|(k, _)| *k);
    mesh.uv = uv.into_iter().map(|(_, v)| v).collect();
    Ok(())
}

fn read_topology(t: &Object<'_>, mesh: &mut Mesh) -> Result<(), GrannyError> {
    let i32s = t.array("Indices")?;
    let i16s = t.array("Indices16")?;
    mesh.indices = if !i32s.is_empty() {
        i32s.bytes()
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect()
    } else {
        i16s.bytes()
            .chunks_exact(2)
            .map(|c| u32::from(u16::from_le_bytes([c[0], c[1]])))
            .collect()
    };
    if !mesh.indices.len().is_multiple_of(3) {
        return Err(GrannyError::Invalid(format!(
            "index count {} is not a multiple of 3",
            mesh.indices.len()
        )));
    }
    let nv = mesh.positions.len();
    if mesh.indices.iter().any(|&i| i as usize >= nv) {
        return Err(GrannyError::Invalid("triangle index out of range".into()));
    }
    let tris = mesh.indices.len() / 3;
    for g in t.array("Groups")?.iter() {
        let g = Group {
            material_index: g.u32("MaterialIndex")?,
            tri_first: g.u32("TriFirst")?,
            tri_count: g.u32("TriCount")?,
        };
        if (g.tri_first as usize).saturating_add(g.tri_count as usize) > tris {
            return Err(GrannyError::Invalid(
                "material group exceeds triangles".into(),
            ));
        }
        mesh.groups.push(g);
    }
    Ok(())
}
