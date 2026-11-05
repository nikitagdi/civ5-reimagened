//! Features on cells: which art the art table gives each feature, and
//! the decals laid over the cells. CPU only.
//!
//! Every drawn feature has a decal: marsh, flood plains, oasis, ice and fallout the
//! `feature_*.dds` sticker, forests and jungles the overlay under their trees (`*_overlay_*.dds`,
//! faded with `overlay_alpha` of `forests.xml`).
//!
//! SIMPLIFIED(SIMP-009): a decal is stretched once over the hex bounding box and fades out to the
//! rim; the original decal rules (tiling, blending with the neighbour terrain) are not known.

use std::collections::HashMap;

use asset_format::{ArtTable, LogicalId};
use bytemuck::{Pod, Zeroable};
use glam::Vec2;

use super::height::ground_height;
use super::mesh::HILL_RING;
use super::texture_array::{STYLES, art_style};
use super::{FeatureKind, Relief, TerrainSnapshot};
use crate::hexgrid::{CORNER_RADIUS, HEX_WIDTH, cell_center, corner_offset};
use crate::world::forest::scatter::ForestParams;

/// Decal opacity of a feature without an `overlay_alpha` placement parameter.
// SIMPLIFIED(SIMP-009): own value
pub const DECAL_ALPHA: f32 = 0.85;
/// `overlay_alpha` of the shipped `forests.xml` (`<Forest ... overlay_alpha=".60">`).
pub const SHIPPED_OVERLAY_ALPHA: f32 = 0.6;
/// Name of the placement parameter that fades forest and jungle overlays.
pub const OVERLAY_ALPHA_KEY: &str = "overlay_alpha";
/// Lift of a decal over the ground, world units; the blend depth bias does the rest.
const DECAL_LIFT: f32 = 0.2;

/// The art of one feature in one continent style.
#[derive(Debug, Clone, PartialEq)]
pub struct FeatureEntry {
    pub tree_model: Option<LogicalId>,
    pub decal: Option<LogicalId>,
    /// Opacity of the decal.
    pub alpha: f32,
}

/// What `table:art-b1` says about every feature and style.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FeatureArtSet {
    entries: HashMap<(FeatureKind, usize), FeatureEntry>,
    /// Tree placement of `forests.xml`, shared by forests and jungles.
    forest: ForestParams,
}

impl FeatureArtSet {
    /// No art at all.
    pub fn none() -> Self {
        Self::default()
    }

    /// `feature/<type>/<style>` for each drawn feature and style.
    pub fn from_table(table: &ArtTable) -> Self {
        let mut entries = HashMap::new();
        let mut forest = ForestParams::default();
        for kind in FeatureKind::DRAWN {
            let Some(name) = kind.type_name() else {
                continue;
            };
            for style in 0..STYLES {
                let Some(rec) = table.feature(name, style as u8) else {
                    continue;
                };
                if FeatureKind::Forest == kind && style == 0 {
                    forest = ForestParams::from_placement(&rec.placement);
                }
                entries.insert(
                    (kind, style),
                    FeatureEntry {
                        tree_model: rec.tree_model.clone(),
                        decal: rec.texture.clone(),
                        alpha: rec
                            .placement
                            .get(OVERLAY_ALPHA_KEY)
                            .copied()
                            .unwrap_or(DECAL_ALPHA)
                            .clamp(0.0, 1.0),
                    },
                );
            }
        }
        Self { entries, forest }
    }

    /// The trees of `assets-b2` (`tree/<feature>/<style>` of `tb:art-b2`): the model and the overlay
    /// of every forest and jungle style. The set has no `forests.xml` placement, so the overlay fades
    /// with the shipped `overlay_alpha` and the trees take the shipped placement
    /// ([`ForestParams::default`]); the other features (marsh, flood plains, ice) are drawn by the
    /// water and the terrain of B2 and get no entry.
    pub fn from_b2_table(table: &ArtTable) -> Self {
        let mut entries = HashMap::new();
        for kind in [FeatureKind::Forest, FeatureKind::Jungle] {
            let Some(name) = kind.type_name() else {
                continue;
            };
            for style in 0..STYLES {
                let Some(t) = table.tree(name, style as u8) else {
                    continue;
                };
                entries.insert(
                    (kind, style),
                    FeatureEntry {
                        tree_model: Some(t.model.clone()),
                        decal: Some(t.overlay.clone()),
                        alpha: SHIPPED_OVERLAY_ALPHA,
                    },
                );
            }
        }
        Self {
            entries,
            forest: ForestParams::default(),
        }
    }

    /// No entry at all.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Placement parameters of the trees.
    pub fn forest_params(&self) -> &ForestParams {
        &self.forest
    }

    pub fn entry(&self, kind: FeatureKind, style: usize) -> Option<&FeatureEntry> {
        self.entries.get(&(kind, style))
    }

    fn ordered(&self) -> Vec<(&(FeatureKind, usize), &FeatureEntry)> {
        let mut v: Vec<_> = self.entries.iter().collect();
        v.sort_by_key(|((k, s), _)| (FeatureKind::DRAWN.iter().position(|d| d == k), *s));
        v
    }

    /// Distinct tree models, in kind and style order: what to fetch.
    pub fn tree_models(&self) -> Vec<LogicalId> {
        distinct(
            self.ordered()
                .into_iter()
                .filter_map(|(_, e)| e.tree_model.clone()),
        )
    }

    /// Distinct decal textures, in kind and style order: what to fetch.
    pub fn decal_textures(&self) -> Vec<LogicalId> {
        distinct(
            self.ordered()
                .into_iter()
                .filter_map(|(_, e)| e.decal.clone()),
        )
    }
}

fn distinct(ids: impl Iterator<Item = LogicalId>) -> Vec<LogicalId> {
    let mut out: Vec<LogicalId> = Vec::new();
    for id in ids {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

/// Vertex of the decal pipeline, 32 bytes: position (0), uv (12), array layer (20), alpha (24),
/// cell index into the fog-state texture (28).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct DecalVertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub layer: u32,
    pub alpha: f32,
    pub cell: u32,
}

/// Where a decal is in the decal texture array.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecalRef {
    pub layer: u32,
    /// UV multiplier (`1/k` when the texture is repeated `k` times to fill its layer).
    pub uv_scale: f32,
    pub alpha: f32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DecalMesh {
    pub vertices: Vec<DecalVertex>,
    pub indices: Vec<u32>,
}

impl DecalMesh {
    pub fn indices_valid(&self) -> bool {
        let n = self.vertices.len() as u32;
        self.indices.len().is_multiple_of(3) && self.indices.iter().all(|&i| i < n)
    }
}

/// Vertices per decal: the centre, an inner ring and the rim.
pub const DECAL_VERTICES: usize = 13;
/// Triangles per decal.
pub const DECAL_TRIANGLES: usize = 18;

/// Decals for every revealed cell with a drawn feature that `resolve` has art for. The fan
/// follows the ground (hill rings), is opaque to half the cell and fades to nothing at the rim.
pub fn decal_mesh(
    snapshot: &TerrainSnapshot,
    resolve: &dyn Fn(FeatureKind, usize) -> Option<DecalRef>,
) -> DecalMesh {
    let mut mesh = DecalMesh::default();
    let width = snapshot.width() as usize;
    for (index, cell) in snapshot.cells().iter().enumerate() {
        if !cell.is_revealed() || cell.relief == Relief::Mountain || cell.feat.type_name().is_none()
        {
            continue;
        }
        let Some(decal) = resolve(cell.feat, art_style(cell.art)) else {
            continue;
        };
        let (x, y) = ((index % width) as i32, (index / width) as i32);
        let centre = cell_center(x, y);
        let base = mesh.vertices.len() as u32;
        let mut push = |offset: Vec2, alpha: f32| {
            let uv = (Vec2::new(0.5, 0.5)
                + Vec2::new(offset.x / HEX_WIDTH, offset.y / (2.0 * CORNER_RADIUS)))
                * decal.uv_scale;
            mesh.vertices.push(DecalVertex {
                position: [
                    centre.x + offset.x,
                    ground_height(cell.relief, offset) + DECAL_LIFT,
                    centre.y + offset.y,
                ],
                uv: uv.to_array(),
                layer: decal.layer,
                alpha,
                cell: index as u32,
            });
        };
        push(Vec2::ZERO, decal.alpha);
        for i in 0..6 {
            push(corner_offset(i) * HILL_RING, decal.alpha);
        }
        for i in 0..6 {
            push(corner_offset(i), 0.0);
        }
        // Inner fan, then the ring between the two hexagons.
        for i in 0..6u32 {
            let (a, b) = (1 + i, 1 + (i + 1) % 6);
            mesh.indices.extend_from_slice(&[base, base + a, base + b]);
        }
        for i in 0..6u32 {
            let (a, b) = (1 + i, 1 + (i + 1) % 6);
            let (c, d) = (7 + i, 7 + (i + 1) % 6);
            mesh.indices.extend_from_slice(&[
                base + a,
                base + c,
                base + d,
                base + a,
                base + d,
                base + b,
            ]);
        }
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::test_util::{land, map};
    use crate::terrain::{Fog, TerrainCell};
    use asset_format::FeatureArt;
    use std::collections::BTreeMap;

    fn rec(texture: &str, model: Option<&str>, placement: &[(&str, f32)]) -> FeatureArt {
        FeatureArt {
            texture: Some(LogicalId::new(texture)),
            tree_model: model.map(LogicalId::new),
            placement: placement
                .iter()
                .map(|(k, v)| ((*k).to_owned(), *v))
                .collect::<BTreeMap<_, _>>(),
        }
    }

    fn table() -> ArtTable {
        let mut t = ArtTable::new();
        for style in 0..5u8 {
            let name = ["generic", "america", "asia", "africa", "europe"][usize::from(style)];
            t.insert_feature(
                "FEATURE_FOREST",
                style,
                rec(
                    &format!("tex:forest_overlay_{name}"),
                    Some(&format!("model:forest_{name}")),
                    &[("overlay_alpha", 0.6)],
                ),
            );
            t.insert_feature("FEATURE_MARSH", style, rec("tex:feature_marsh", None, &[]));
        }
        t
    }

    #[test]
    fn art_set_reads_models_decals_and_alpha() {
        let art = FeatureArtSet::from_table(&table());
        let f = art.entry(FeatureKind::Forest, 2).unwrap();
        assert_eq!(f.tree_model, Some(LogicalId::new("model:forest_asia")));
        assert_eq!(f.decal, Some(LogicalId::new("tex:forest_overlay_asia")));
        assert_eq!(f.alpha, 0.6);
        let m = art.entry(FeatureKind::Marsh, 0).unwrap();
        assert_eq!((m.tree_model.clone(), m.alpha), (None, DECAL_ALPHA));
        assert!(art.entry(FeatureKind::Jungle, 0).is_none());
        assert_eq!(art.tree_models().len(), 5);
        // Five overlays and one marsh decal shared by all styles.
        assert_eq!(art.decal_textures().len(), 6);
        assert_eq!(
            art.decal_textures()[0],
            LogicalId::new("tex:forest_overlay_generic")
        );
        assert!(FeatureArtSet::none().tree_models().is_empty());
    }

    #[test]
    fn feature_kind_names_round_trip() {
        for k in FeatureKind::DRAWN {
            assert_eq!(FeatureKind::from_type_name(k.type_name().unwrap()), k);
        }
        assert_eq!(
            FeatureKind::from_type_name("FEATURE_CRATER"),
            FeatureKind::Other
        );
    }

    fn resolve(kind: FeatureKind, _style: usize) -> Option<DecalRef> {
        (kind != FeatureKind::Oasis).then_some(DecalRef {
            layer: kind as u32,
            uv_scale: 0.5,
            alpha: 0.6,
        })
    }

    #[test]
    fn one_decal_per_revealed_cell_with_art() {
        let snap = map(5, 1, |x, _| match x {
            0 => TerrainCell {
                feat: FeatureKind::Marsh,
                ..land()
            },
            1 => TerrainCell {
                feat: FeatureKind::Marsh,
                fog: Fog::Unrevealed,
                ..land()
            },
            2 => TerrainCell {
                feat: FeatureKind::Oasis,
                ..land()
            },
            3 => TerrainCell {
                feat: FeatureKind::Marsh,
                relief: Relief::Mountain,
                ..land()
            },
            _ => TerrainCell {
                feat: FeatureKind::Ice,
                relief: Relief::Water,
                ..land()
            },
        });
        let m = decal_mesh(&snap, &resolve);
        // Marsh at 0 and ice at 4; unrevealed, no art and mountain cells have none.
        assert_eq!(m.vertices.len(), 2 * DECAL_VERTICES);
        assert_eq!(m.indices.len(), 2 * DECAL_TRIANGLES * 3);
        assert!(m.indices_valid());
        assert_eq!(m.vertices[0].cell, 0);
        assert_eq!(m.vertices[DECAL_VERTICES].cell, 4);
        assert_eq!(m.vertices[DECAL_VERTICES].layer, FeatureKind::Ice as u32);
    }

    #[test]
    fn a_decal_is_opaque_inside_and_fades_to_the_rim() {
        let snap = map(1, 1, |_, _| TerrainCell {
            feat: FeatureKind::Marsh,
            ..land()
        });
        let m = decal_mesh(&snap, &resolve);
        let alphas: Vec<f32> = m.vertices.iter().map(|v| v.alpha).collect();
        assert!(alphas[..7].iter().all(|&a| a == 0.6));
        assert!(alphas[7..].iter().all(|&a| a == 0.0));
        // UV spans the bounding box times the scale: the centre is at 0.5 * 0.5.
        assert_eq!(m.vertices[0].uv, [0.25, 0.25]);
        assert!(
            m.vertices
                .iter()
                .all(|v| v.uv.iter().all(|c| (0.0..=0.5).contains(c)))
        );
        assert!(m.vertices.iter().all(|v| v.position[1] > 12.0));
    }

    #[test]
    fn decals_follow_the_hill() {
        let snap = map(1, 1, |_, _| TerrainCell {
            feat: FeatureKind::Forest,
            relief: Relief::Hill,
            ..land()
        });
        let m = decal_mesh(&snap, &resolve);
        let top = m.vertices[0].position[1];
        let rim = m.vertices[7].position[1];
        assert!(top > rim + 10.0, "{top} {rim}");
    }

    #[test]
    fn vertex_layout_is_32_bytes() {
        assert_eq!(std::mem::size_of::<DecalVertex>(), 32);
    }
}
