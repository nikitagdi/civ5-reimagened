//! Decals of resources, improvements, barbarian encampments and ruins (spec §3.4, AC-2.7, AC-2.8,
//! AC-4.6): the strategic-view icon of the plot's art define, laid flat on the ground.
//!
//! | What | Where on the cell | Size |
//! |---|---|---|
//! | resource | south-east part | 24 |
//! | improvement | south-west part | 24 |
//! | encampment, ruins | centre | 32 |
//!
//! Fog: the brain sends the last seen improvement of a revealed plot, and the resource only as the
//! human knows it (`getResourceType(team)`), so the plot data decides *what* is shown. Here only
//! unrevealed cells show nothing; on revealed cells the decal pipeline darkens the cells that are
//! not visible. A pillaged improvement is drawn half transparent.
//!
//! SIMPLIFIED(SIMP-012): the SV icon of the art define instead of the model of the original.

use std::collections::BTreeSet;

use glam::Vec2;

use super::{EntitySnapshot, PlotDecor};
use crate::terrain::features::{DecalMesh, DecalRef, DecalVertex};
use crate::terrain::{Fog, TerrainSnapshot, terrain_height_at};

/// Edge of the resource icon.
// re: (24 units)
pub const RESOURCE_SIZE: f32 = 24.0;
/// Edge of the improvement icon.
// re: (24 units)
pub const IMPROVEMENT_SIZE: f32 = 24.0;
/// Edge of the encampment and ruins icon.
// re: (32 units)
pub const SPECIAL_SIZE: f32 = 32.0;
/// Centre of the resource icon from the cell centre (X east, Z south): the south-east part.
pub const RESOURCE_OFFSET: Vec2 = Vec2::new(11.0, 12.0);
/// Centre of the improvement icon: the south-west part.
pub const IMPROVEMENT_OFFSET: Vec2 = Vec2::new(-11.0, 12.0);
/// Opacity factor of the icon of a pillaged improvement.
pub const PILLAGED_ALPHA: f32 = 0.5;
/// Lift over the ground, above the feature decals (0.2).
const ICON_LIFT: f32 = 0.6;

/// Which part of the cell an icon takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Resource,
    Improvement,
    /// Encampment or ruins: the middle of the cell.
    Special,
}

impl Slot {
    /// Offset of the icon centre from the cell centre, and the edge of the icon.
    pub fn placement(self) -> (Vec2, f32) {
        match self {
            Self::Resource => (RESOURCE_OFFSET, RESOURCE_SIZE),
            Self::Improvement => (IMPROVEMENT_OFFSET, IMPROVEMENT_SIZE),
            Self::Special => (Vec2::ZERO, SPECIAL_SIZE),
        }
    }
}

/// Opacity factor of the icons of a cell in `fog`, `None` when nothing is drawn. Revealed cells
/// that are not visible keep their icons: the pipeline darkens them.
pub fn fog_alpha(fog: Fog) -> Option<f32> {
    match fog {
        Fog::Unrevealed => None,
        Fog::Hidden | Fog::Visible => Some(1.0),
    }
}

/// The icons of one plot: slot, art define tag, opacity factor.
pub fn plot_icons(plot: &PlotDecor) -> Vec<(Slot, &str, f32)> {
    let mut out = Vec::new();
    if let Some(tag) = plot.resource.as_deref() {
        out.push((Slot::Resource, tag, 1.0));
    }
    if let Some(tag) = plot.improvement.as_deref() {
        let slot = if plot.special {
            Slot::Special
        } else {
            Slot::Improvement
        };
        out.push((slot, tag, if plot.pillaged { PILLAGED_ALPHA } else { 1.0 }));
    }
    out
}

/// Art define tags the snapshot needs icons for, in order: what to look up in the art table.
pub fn wanted_tags(snapshot: &EntitySnapshot) -> BTreeSet<String> {
    wanted_tags_of(snapshot.plots.iter())
}

/// [`wanted_tags`] of the plots of the keyed inputs (WOB0).
pub fn wanted_tags_of<'a>(plots: impl IntoIterator<Item = &'a PlotDecor>) -> BTreeSet<String> {
    plots
        .into_iter()
        .flat_map(|p| plot_icons(p).into_iter().map(|(_, t, _)| t.to_owned()))
        .collect()
}

/// One decal quad per icon on a revealed cell for which `resolve` finds the icon of its tag.
pub fn build(
    snapshot: &EntitySnapshot,
    terrain: &TerrainSnapshot,
    resolve: &dyn Fn(&str) -> Option<DecalRef>,
) -> DecalMesh {
    build_plots(snapshot.plots.iter(), terrain, resolve)
}

/// [`build`] of the plots of the keyed inputs (WOB0).
pub fn build_plots<'a>(
    plots: impl IntoIterator<Item = &'a PlotDecor>,
    terrain: &TerrainSnapshot,
    resolve: &dyn Fn(&str) -> Option<DecalRef>,
) -> DecalMesh {
    let mut mesh = DecalMesh::default();
    let mut plots: Vec<&PlotDecor> = plots.into_iter().collect();
    plots.sort_by_key(|p| (p.y, p.x));
    for plot in plots {
        let Some(index) = terrain.index(plot.x, plot.y) else {
            continue;
        };
        let Some(fog) = fog_alpha(terrain.cells()[index].fog) else {
            continue;
        };
        let centre = crate::hexgrid::cell_center(plot.x, plot.y);
        for (slot, tag, factor) in plot_icons(plot) {
            let Some(icon) = resolve(tag) else {
                continue;
            };
            let (offset, size) = slot.placement();
            let half = size * 0.5;
            let base = mesh.vertices.len() as u32;
            // Image up is north (-Z); the camera looks north.
            for (cx, cz) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                let p = centre + offset + Vec2::new(cx * half, cz * half);
                mesh.vertices.push(DecalVertex {
                    position: [p.x, terrain_height_at(terrain, p) + ICON_LIFT, p.y],
                    uv: [
                        (cx * 0.5 + 0.5) * icon.uv_scale,
                        (cz * 0.5 + 0.5) * icon.uv_scale,
                    ],
                    layer: icon.layer,
                    alpha: icon.alpha * factor * fog,
                    cell: index as u32,
                });
            }
            mesh.indices
                .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hexgrid::cell_center;
    use crate::terrain::TerrainCell;
    use crate::terrain::test_util::{land, map};

    fn plot(x: i32, y: i32) -> PlotDecor {
        PlotDecor {
            x,
            y,
            ..PlotDecor::default()
        }
    }

    fn resolve(tag: &str) -> Option<DecalRef> {
        (tag != "ART_DEF_NONE").then_some(DecalRef {
            layer: tag.len() as u32,
            uv_scale: 0.5,
            alpha: 1.0,
        })
    }

    fn snapshot(plots: Vec<PlotDecor>) -> EntitySnapshot {
        EntitySnapshot {
            plots,
            ..EntitySnapshot::default()
        }
    }

    #[test]
    fn slots_have_the_spec_sizes_and_sit_in_their_parts() {
        assert_eq!(Slot::Resource.placement().1, 24.0);
        assert_eq!(Slot::Improvement.placement().1, 24.0);
        assert_eq!(Slot::Special.placement(), (Vec2::ZERO, 32.0));
        // South-east: east and south (+Z) of the centre; south-west: west and south.
        let (r, i) = (
            Slot::Resource.placement().0,
            Slot::Improvement.placement().0,
        );
        assert!(r.x > 0.0 && r.y > 0.0, "{r:?}");
        assert!(i.x < 0.0 && i.y > 0.0, "{i:?}");
    }

    #[test]
    fn fog_rules() {
        assert_eq!(fog_alpha(Fog::Unrevealed), None);
        assert_eq!(fog_alpha(Fog::Hidden), Some(1.0));
        assert_eq!(fog_alpha(Fog::Visible), Some(1.0));
    }

    #[test]
    fn unrevealed_cells_have_no_icons_hidden_ones_keep_theirs() {
        let t = map(3, 1, |x, _| match x {
            0 => land(),
            1 => TerrainCell {
                fog: Fog::Hidden,
                ..land()
            },
            _ => TerrainCell::default(),
        });
        let mk = |x| PlotDecor {
            resource: Some("ART_DEF_RESOURCE_IRON".into()),
            ..plot(x, 0)
        };
        let s = snapshot(vec![mk(0), mk(1), mk(2), mk(7)]);
        let m = build(&s, &t, &resolve);
        // Cells 0 and 1; the unrevealed one and the one off the map give nothing.
        assert_eq!(m.vertices.len(), 8);
        assert_eq!(m.vertices[0].cell, 0);
        assert_eq!(m.vertices[4].cell, 1);
        assert!(m.indices_valid());
    }

    #[test]
    fn resource_improvement_and_special_take_their_parts_of_the_cell() {
        let t = map(3, 3, |_, _| land());
        let both = PlotDecor {
            resource: Some("ART_DEF_RESOURCE_IRON".into()),
            improvement: Some("ART_DEF_IMPROVEMENT_MINE".into()),
            ..plot(1, 1)
        };
        let camp = PlotDecor {
            improvement: Some("ART_DEF_IMPROVEMENT_BARBARIAN_CAMP".into()),
            special: true,
            ..plot(2, 2)
        };
        let m = build(&snapshot(vec![both, camp]), &t, &resolve);
        assert_eq!(m.vertices.len(), 12);
        let mean = |range: std::ops::Range<usize>| {
            let v = &m.vertices[range];
            let n = v.len() as f32;
            Vec2::new(
                v.iter().map(|v| v.position[0]).sum::<f32>() / n,
                v.iter().map(|v| v.position[2]).sum::<f32>() / n,
            )
        };
        let c = cell_center(1, 1);
        // Resource first (south-east), then the improvement (south-west).
        assert!((mean(0..4) - (c + RESOURCE_OFFSET)).length() < 1e-3);
        assert!((mean(4..8) - (c + IMPROVEMENT_OFFSET)).length() < 1e-3);
        // The encampment is in the middle, 32 wide.
        let cc = cell_center(2, 2);
        assert!((mean(8..12) - cc).length() < 1e-3);
        let w = m.vertices[9].position[0] - m.vertices[8].position[0];
        assert!((w - 32.0).abs() < 1e-3);
        // Every icon lies inside its own hex (corner within the corner radius).
        for v in &m.vertices[0..8] {
            let p = Vec2::new(v.position[0], v.position[2]);
            assert!(crate::terrain::height::hex_norm(p - c) < 1.05, "{p:?}");
        }
    }

    #[test]
    fn uv_covers_the_layer_part_and_the_ground_is_followed() {
        let t = map(2, 2, |x, _| TerrainCell {
            relief: if x == 1 {
                crate::terrain::Relief::Hill
            } else {
                crate::terrain::Relief::Flat
            },
            ..land()
        });
        let mk = |x| PlotDecor {
            resource: Some("ART_DEF_RESOURCE_IRON".into()),
            ..plot(x, 0)
        };
        let m = build(&snapshot(vec![mk(0), mk(1)]), &t, &resolve);
        assert_eq!(m.vertices[0].uv, [0.0, 0.0]);
        assert_eq!(m.vertices[2].uv, [0.5, 0.5]);
        // Same icon on a hill sits higher than on flat land.
        assert!(m.vertices[4].position[1] > m.vertices[0].position[1] + 3.0);
    }

    #[test]
    fn missing_icons_pillage_and_wanted_tags() {
        let t = map(3, 1, |_, _| land());
        let pillaged = PlotDecor {
            improvement: Some("ART_DEF_IMPROVEMENT_FARM".into()),
            pillaged: true,
            ..plot(0, 0)
        };
        let missing = PlotDecor {
            resource: Some("ART_DEF_NONE".into()),
            ..plot(1, 0)
        };
        let s = snapshot(vec![pillaged, missing]);
        let m = build(&s, &t, &resolve);
        assert_eq!(m.vertices.len(), 4, "the missing icon is skipped");
        assert_eq!(m.vertices[0].alpha, PILLAGED_ALPHA);
        let tags = wanted_tags(&s);
        assert_eq!(tags.len(), 2);
        assert!(tags.contains("ART_DEF_NONE") && tags.contains("ART_DEF_IMPROVEMENT_FARM"));
    }
}
