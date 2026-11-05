//! The input of the highlights and of the movement path: the
//! plots the game asks to show, the steps of a path, and the geometry of a path in one unwrapped
//! frame (a step to the neighbour across the seam continues in the same direction).
//!
//! The picture is not made here: the selection ring, the contours and the cells go through the
//! styles of `Highlights.xml` and the ring of `Civ5ArtDefines_Overlay.xml` (`world/overlay`), the
//! path through the models of the original (`world/path`). What this module still makes are the
//! labels with the number of the turn at the end of each turn of a path, for the UI.

use glam::{Vec2, Vec3};

use super::labels::{LabelData, PathTurn, WorldLabel};
use super::roads::dir_vector;
use crate::hexgrid::{Dir, GridShape, HEX_WIDTH, cell_center};
use crate::terrain::{TerrainSnapshot, terrain_height_at};

/// The path-turn label hangs this far over the ground.
// re: `PathHelpManager.lua:41` (plot position + 3 in height)
pub const PATH_LABEL_HEIGHT: f32 = 3.0;

/// One step of a path: the plot and the turn it is reached on, counted from 1 as the exe's
/// `UIPathFinderUpdate` counts (the start and the plots reached this turn are turn 1; 0 is read as 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathStep {
    pub x: i32,
    pub y: i32,
    pub turn: i32,
    /// The flags of the search node (`PathFlags`); 0 for none.
    pub flags: u16,
}

/// The flags of a path node, as the game core gives them (`GC_REACH_F_*`).
pub struct PathFlags;

impl PathFlags {
    pub const ATTACK: u16 = 0x0001;
    pub const EMBARK: u16 = 0x0002;
    pub const STOP: u16 = 0x0010;
    pub const UNREACHABLE: u16 = 0x0080;
}

/// What to highlight. Plots are map coordinates (`y` grows north).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Highlights {
    /// The selected unit's or city's plot: the pulsing ring.
    pub selected: Option<(i32, i32)>,
    /// Plots the selected unit reaches this turn.
    pub reachable: Vec<(i32, i32)>,
    /// Plots that can be shot at.
    pub targets: Vec<(i32, i32)>,
    /// The path, start first.
    pub path: Vec<PathStep>,
}

impl Highlights {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// Positions of the steps in one unwrapped frame: the first step at its cell centre, each next
/// one a cell away from the one before in the direction that leads to it (so a step over the seam
/// continues in the same direction). A step that is not a neighbour jumps to its own centre.
pub fn unrolled_path(shape: &GridShape, path: &[PathStep]) -> Vec<Vec2> {
    let mut out: Vec<Vec2> = Vec::with_capacity(path.len());
    for (i, s) in path.iter().enumerate() {
        let at = if i == 0 {
            cell_center(s.x, s.y)
        } else {
            let p = path[i - 1];
            let dir = Dir::ALL
                .into_iter()
                .find(|d| shape.neighbor(p.x, p.y, *d) == Some((s.x, s.y)));
            match dir {
                Some(d) => out[i - 1] + dir_vector(d) * HEX_WIDTH,
                None => cell_center(s.x, s.y),
            }
        };
        out.push(at);
    }
    out
}

/// Steps that get a marker and a turn label: the last step of each turn, never the start.
pub fn turn_markers(path: &[PathStep]) -> Vec<usize> {
    (1..path.len())
        .filter(|&i| i + 1 == path.len() || path[i + 1].turn != path[i].turn)
        .collect()
}

/// One label per turn marker of `path`, for the UI to number: the last step of each turn, at
/// `PATH_LABEL_HEIGHT` over the ground. A path of fewer than two steps has none.
pub fn path_labels(path: &[PathStep], terrain: &TerrainSnapshot) -> Vec<WorldLabel> {
    if path.len() < 2 {
        return Vec::new();
    }
    let shape = terrain.shape();
    let steps: Vec<PathStep> = path
        .iter()
        .filter_map(|s| {
            let (x, y) = shape.normalize(s.x, s.y)?;
            Some(PathStep { x, y, ..*s })
        })
        .collect();
    let at = unrolled_path(&shape, &steps);
    turn_markers(&steps)
        .into_iter()
        .filter(|&i| terrain.index(steps[i].x, steps[i].y).is_some())
        .map(|i| WorldLabel {
            cell: (steps[i].x, steps[i].y),
            position: Vec3::new(
                at[i].x,
                terrain_height_at(terrain, at[i]) + PATH_LABEL_HEIGHT,
                at[i].y,
            ),
            data: LabelData::PathTurn(PathTurn {
                turn: steps[i].turn,
            }),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::test_util::{land, map};

    fn wrap_shape() -> GridShape {
        GridShape {
            width: 12,
            height: 12,
            wrap_x: true,
        }
    }

    fn step(x: i32, y: i32, turn: i32) -> PathStep {
        PathStep {
            x,
            y,
            turn,
            flags: 0,
        }
    }

    #[test]
    fn path_turn_markers() {
        let steps = |turns: &[i32]| -> Vec<PathStep> {
            turns
                .iter()
                .enumerate()
                .map(|(i, &turn)| step(i as i32, 0, turn))
                .collect()
        };
        // Start (0), two steps in turn 1, one in turn 2: markers at the end of each turn.
        assert_eq!(turn_markers(&steps(&[0, 1, 1, 2])), [2, 3]);
        assert_eq!(turn_markers(&steps(&[0, 1])), [1]);
        assert!(turn_markers(&steps(&[0])).is_empty());
        // A start that already has the turn of the first step is not a marker.
        assert_eq!(turn_markers(&steps(&[1, 1, 2])), [1, 2]);
    }

    #[test]
    fn the_path_is_one_unwrapped_line_over_the_seam() {
        let shape = wrap_shape();
        // (10, 5) -> (11, 5) -> (0, 5): east, east over the seam.
        let steps = [step(10, 5, 0), step(11, 5, 1), step(0, 5, 1)];
        let at = unrolled_path(&shape, &steps);
        assert!((at[1] - at[0] - Vec2::new(64.0, 0.0)).length() < 1e-3);
        assert!((at[2] - at[1] - Vec2::new(64.0, 0.0)).length() < 1e-3);
        // The last step is past the east edge of the map in this frame.
        assert!(at[2].x > shape.world_width() - 1.0);
        // Not neighbours: jump to the cell's own centre.
        let jump = unrolled_path(&shape, &[step(1, 1, 0), step(6, 6, 1)]);
        assert_eq!(jump[1], cell_center(6, 6));
    }

    #[test]
    fn a_path_has_a_label_at_the_end_of_each_turn() {
        let t = map(12, 12, |_, _| land());
        let path = [step(4, 4, 0), step(5, 4, 1), step(6, 4, 2)];
        // Two turn markers: the end of turn 1 and of turn 2, at +3 over the ground.
        let labels = path_labels(&path, &t);
        assert_eq!(labels.len(), 2);
        assert_eq!(labels[0].cell, (5, 4));
        match &labels[1].data {
            LabelData::PathTurn(t) => assert_eq!(t.turn, 2),
            other => panic!("{other:?}"),
        }
        assert!(
            (labels[0].position.y - (crate::terrain::LAND_HEIGHT + PATH_LABEL_HEIGHT)).abs() < 1e-3
        );
    }

    #[test]
    fn short_and_off_map_paths_have_no_labels() {
        let t = map(4, 4, |_, _| land());
        assert!(path_labels(&[step(1, 1, 0)], &t).is_empty());
        assert!(path_labels(&[], &t).is_empty());
        assert!(path_labels(&[step(9, 9, 0), step(9, 8, 1)], &t).is_empty());
    }

    #[test]
    fn highlights_are_empty_by_default() {
        let h = Highlights {
            selected: Some((1, 1)),
            ..Highlights::default()
        };
        assert!(Highlights::default().is_empty() && !h.is_empty());
    }
}
