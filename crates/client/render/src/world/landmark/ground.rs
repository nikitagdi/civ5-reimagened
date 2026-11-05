//! The ground meshes of the landmarks as decals of the terrain (world-objects WOB4, RW-14): a mesh of
//! the `LandmarkShader` (the grass and rocks of Sri Pada, the stone of a quarry, the earth of a camp)
//! is not drawn as geometry but laid into the page of every cell it covers, so it follows the relief
//! the terrain build gave the plot (a natural wonder's own piece). The original's decal container
//! does this for every such mesh of a model (`DecalContainerBuilderContext::InitMeshDecal`): the
//! Position and TextureCoordinates0 of the vertices, the colour texture of slot 0 of the material and
//! its second texture, into `TerrainDecalLayer::CreateDecal` with a type of its own. No capture of
//! the main pass has a `LandmarkPS` call.
//!
//! The mesh is projected from above: the height of a vertex is dropped (the decal vertex is
//! `{x, y, u, v}`).
// re: rw-w8-answers.txt q2 (InitMeshDecal 0x088be424: two members Position, TextureCoordinates0; colour
// texture slot 0 -> +0x14, slot 1 -> +0x18), TerrainDecalLayer.cpp:160-285 (CreateDecal with a
// DecalType), :442-443 (the draw binds the albedo and the heightmap of the entry)

use asset_format::{LogicalId, ModelBlobV2, TexSource};
use glam::{Mat4, Vec2, Vec3};

use crate::material::ground_decal;
use crate::world::terrain::decals::{DecalVertex, MeshDecal};
use crate::world::unit::draw::shows_in;

/// A ground mesh of a model, in the space of the model.
#[derive(Debug, Clone, PartialEq)]
pub struct GroundMesh {
    pub part: u16,
    /// The node states the mesh shows in (`MeshV2::states`).
    pub states: u8,
    pub albedo: LogicalId,
    pub height: Option<LogicalId>,
    pub positions: Vec<Vec3>,
    pub uvs: Vec<Vec2>,
    pub tris: Vec<[u16; 3]>,
    /// The priority of the decal: the height of the first vertex of the mesh, in `0..=1`.
    pub priority: f32,
}

impl GroundMesh {
    /// The rigid `LandmarkShader` meshes of a model that have a texture (a mesh on a bone or with a
    /// skin stays a model: its place moves with the clip).
    pub fn of_model(blob: &ModelBlobV2) -> Vec<Self> {
        blob.meshes
            .iter()
            .filter(|m| m.bind.is_none() && m.joints().is_none_or(|j| j.is_empty()))
            .filter_map(|m| {
                let desc = blob.materials.get(usize::from(m.material))?;
                if !ground_decal(desc) {
                    return None;
                }
                // The samplers of `LandmarkShaderTextures`: `BaseTextureMap` (slot 0, the colour)
                // and `HeightTextureMap` (slot 1); `SpecTextureMap` is not read by the decal pass.
                let file = |name: &str| match desc.texture(name).map(|t| &t.src) {
                    Some(TexSource::File(id)) => Some(id.clone()),
                    _ => None,
                };
                let albedo =
                    file("BaseTextureMap").or_else(|| desc.texture_files().next().cloned())?;
                let height = file("HeightTextureMap");
                let positions: Vec<Vec3> = m.positions()?.into_iter().map(Vec3::from).collect();
                // The pipeline stores `1 - v`; the decal samples the D3D texture with the
                // original's `v` (as `unit_skinned.wgsl` does).
                let uvs: Vec<Vec2> = m
                    .uvs()?
                    .into_iter()
                    .map(|[u, v]| Vec2::new(u, 1.0 - v))
                    .collect();
                if positions.len() != uvs.len() || positions.len() > usize::from(u16::MAX) {
                    return None;
                }
                let idx = m.indices.to_u32();
                let tris: Vec<[u16; 3]> = idx
                    .chunks_exact(3)
                    .filter(|t| t.iter().all(|&i| (i as usize) < positions.len()))
                    .map(|t| [t[0] as u16, t[1] as u16, t[2] as u16])
                    .collect();
                // re: DecalContainerBuilderContext::InitMeshDecal 0x088be424 (RE-B12 item 2, 3.3): the
                // priority of a landmark decal is `clamp(z of the first vertex, 0, 1)`; the wheat's plane
                // lies at 0, so everything with a priority of its own (roads, fields) draws over it.
                // SIMPLIFIED(SIMP-exe-1073): the first vertex of the merged mesh, placement baked in
                let priority = positions.first().map_or(0.0, |p| p.y.clamp(0.0, 1.0));
                (!tris.is_empty()).then_some(Self {
                    priority,
                    part: m.part,
                    states: m.states,
                    albedo,
                    height,
                    positions,
                    uvs,
                    tris,
                })
            })
            .collect()
    }

    /// The decal of the mesh placed by `matrix` (model space to our world), for the hex `cell`.
    pub fn decal(&self, matrix: &Mat4, cell: u32) -> MeshDecal {
        let verts = self
            .positions
            .iter()
            .zip(&self.uvs)
            .map(|(p, uv)| {
                let w = matrix.transform_point3(*p);
                DecalVertex {
                    pos: Vec2::new(w.x, w.z),
                    uv: *uv,
                }
            })
            .collect();
        MeshDecal {
            albedo: self.albedo.clone(),
            height: self.height.clone(),
            priority: self.priority,
            verts,
            tris: self.tris.clone(),
            cell,
        }
    }
}

/// The decals of the ground meshes of one landmark: every piece (the whole model, or one part) puts
/// down the meshes of its part that show in the state `desired`.
pub fn landmark_decals(
    grounds: &[GroundMesh],
    pieces: &[(Option<u16>, Mat4)],
    desired: u8,
    cell: u32,
) -> Vec<MeshDecal> {
    let mut out = Vec::new();
    for (part, matrix) in pieces {
        for g in grounds
            .iter()
            .filter(|g| part.is_none_or(|p| p == g.part) && shows_in(g.states, desired))
        {
            out.push(g.decal(matrix, cell));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use asset_format::{
        MaterialDesc, MeshArrays, MeshV2, ParamSet, Part, STATES_ALL, TexKind, TexParam, TexSource,
        Transform,
    };

    use super::*;

    fn material(package: &str, files: &[&str]) -> MaterialDesc {
        MaterialDesc {
            package: package.into(),
            sets: vec![ParamSet {
                blocks: vec![],
                textures: files
                    .iter()
                    .zip(["BaseTextureMap", "HeightTextureMap"])
                    .map(|(f, name)| TexParam {
                        name: name.into(),
                        kind: TexKind::Tex2D,
                        src: TexSource::File(LogicalId::new(*f)),
                        semantic: 0,
                        reg: 0,
                        usage: 0,
                        flags: 0,
                        active: true,
                    })
                    .collect(),
                constants: vec![],
            }],
            ..MaterialDesc::default()
        }
    }

    fn quad(material: u16, y: f32) -> MeshV2 {
        let mut m = MeshV2::pack(
            "ground",
            0,
            material,
            &MeshArrays {
                positions: vec![
                    [-1.0, y, -1.0],
                    [1.0, y, -1.0],
                    [1.0, y + 5.0, 1.0],
                    [-1.0, y, 1.0],
                ],
                normals: vec![[0.0, 1.0, 0.0]; 4],
                uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                indices: vec![0, 1, 2, 0, 2, 3],
                ..MeshArrays::default()
            },
            false,
            false,
        )
        .expect("a quad packs");
        m.states = STATES_ALL;
        m
    }

    fn blob() -> ModelBlobV2 {
        ModelBlobV2 {
            parts: vec![Part {
                name: "root".into(),
                placement: Transform::IDENTITY,
                first_mesh: 0,
                mesh_count: 3,
            }],
            meshes: vec![quad(0, 0.0), quad(1, 0.0), quad(2, 0.0)],
            materials: vec![
                material("LandmarkShader", &["tx:ground_diff.dds", "tx:ground_h.dds"]),
                material("BuildingShader", &["tx:house.dds"]),
                material("LandmarkShader_Stencil", &[]),
            ],
            ..ModelBlobV2::default()
        }
    }

    #[test]
    fn only_the_textured_landmark_meshes_are_ground() {
        let g = GroundMesh::of_model(&blob());
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].albedo.as_str(), "tx:ground_diff.dds");
        assert_eq!(g[0].height.as_ref().unwrap().as_str(), "tx:ground_h.dds");
        assert_eq!(g[0].tris.len(), 2);
    }

    #[test]
    fn the_priority_of_a_decal_is_the_height_of_its_first_vertex_in_zero_to_one() {
        let mut b = blob();
        for (y, want) in [
            (0.0, 0.0),
            (0.0053, 0.0053),
            (0.4, 0.4),
            (3.0, 1.0),
            (-2.0, 0.0),
        ] {
            b.meshes[0] = quad(0, y);
            let g = GroundMesh::of_model(&b);
            assert!(
                (g[0].priority - want).abs() < 1e-3,
                "{y}: {}",
                g[0].priority
            );
            let d = g[0].decal(&Mat4::IDENTITY, 0);
            assert!((d.priority - want).abs() < 1e-3);
        }
    }

    #[test]
    fn the_decal_lies_where_the_piece_puts_the_mesh_seen_from_above() {
        let g = GroundMesh::of_model(&blob());
        let m = Mat4::from_translation(Vec3::new(100.0, 7.0, 50.0))
            * Mat4::from_scale(Vec3::splat(2.0));
        let d = landmark_decals(&g, &[(None, m)], 0x4, 12);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].cell, 12);
        // The height of the vertex (5 at the third) is dropped: x and z only.
        assert_eq!(d[0].verts[2].pos, Vec2::new(102.0, 52.0));
        assert_eq!(d[0].verts[0].pos, Vec2::new(98.0, 48.0));
        // The `v` of the blob is turned back to the original's.
        assert_eq!(d[0].verts[2].uv, Vec2::new(1.0, 0.0));
        // A piece of another part puts nothing down.
        assert!(landmark_decals(&g, &[(Some(3), m)], 0x4, 12).is_empty());
    }
}
