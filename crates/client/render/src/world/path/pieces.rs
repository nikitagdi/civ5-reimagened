//! The pieces of a path (world-overlays 4.5, WOV6): which model of the original stands where.
//!
//! The original draws the path with six looks of three kinds of model (`Assets/UI/Overlays/`):
//! `Center*` on a node, `Connector*` between two nodes, `SelectedAni*` on the last node (it pulses,
//! `AnimService`). The look follows the node's flags and its turn: `OneTurn`, `TwoTurns`,
//! `ThreeTurns` (the third and every later turn), `Embark`, `Attack`, `Inaccessible` (an
//! unreachable target).
//!
//! The layout is the exe's (`cvMovementPathVisSystem::UpdateFromHandle(DisplayPath)` 0x088c08c6 and
//! `LoadConnectorModelsIntoRenderList` 0x088c1e8e, re-ui-live-answers item 3, 3.2): the unit's own
//! plot gets `CenterOneTurn` unscaled; every later node a `Center*` of its look, scaled 1.5 on the
//! last node of a turn and on the destination, 0.75 elsewhere; the destination also gets the
//! unscaled `SelectedAni*`; and the edge into every node but the first carries six `Connector*` of
//! that node's look at `k / 7` of the way (`k = 1..=6`; three on the last edge), turned so that
//! the model's north axis runs along the edge, unscaled. The positions are the cells' centres at
//! the relief's height; a connector lies on the straight line between the two node positions.
//! This matches the capture `S4-path` (a node of turn 1 at scale 1.0 is the unit's own plot, the
//! one at 1.5 the end of a turn; the relief's height with no lift).

use asset_format::LogicalId;
use glam::{Vec2, Vec3};

use super::PathInput;
use crate::entities::highlight::{PathFlags, PathStep, turn_markers, unrolled_path};
use crate::hexgrid::GridShape;
use crate::world::overlay::highlights::model::model_id;

/// What a piece is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PieceKind {
    Center,
    Connector,
    /// The pulsing mark on the last node.
    Selected,
}

/// The look of a piece.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Look {
    OneTurn,
    TwoTurns,
    ThreeTurns,
    Embark,
    Attack,
    Inaccessible,
}

impl Look {
    /// The look of a node: the flags first (an unreachable target, an attack, an embark), then its
    /// turn (counted from 1; 0 is read as the first turn).
    pub fn of(step: &PathStep) -> Self {
        if step.flags & PathFlags::UNREACHABLE != 0 {
            Self::Inaccessible
        } else if step.flags & PathFlags::ATTACK != 0 {
            Self::Attack
        } else if step.flags & PathFlags::EMBARK != 0 {
            Self::Embark
        } else {
            match step.turn {
                ..=1 => Self::OneTurn,
                2 => Self::TwoTurns,
                _ => Self::ThreeTurns,
            }
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            Self::OneTurn => "OneTurn",
            Self::TwoTurns => "TwoTurns",
            Self::ThreeTurns => "ThreeTurns",
            Self::Embark => "Embark",
            Self::Attack => "Attack",
            Self::Inaccessible => "Inaccessible",
        }
    }
}

/// The id of the model of a kind and a look (`md:centeroneturn`, `md:connectorattack`,
/// `md:selectedanitwoturns`, ...).
pub fn piece_model(kind: PieceKind, look: Look) -> LogicalId {
    let base = match kind {
        PieceKind::Center => "Center",
        PieceKind::Connector => "Connector",
        PieceKind::Selected => "SelectedAni",
    };
    model_id(&format!("{base}{}.fxsxml", look.suffix()))
}

/// One piece: a model at a place.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Piece {
    pub kind: PieceKind,
    pub look: Look,
    /// Our basis (X east, Y up, Z south), in the unrolled frame of the path.
    pub position: Vec3,
    /// About the vertical axis, B1's `rotate_y`.
    pub yaw: f32,
    pub scale: f32,
}

impl Piece {
    pub fn model(&self) -> LogicalId {
        piece_model(self.kind, self.look)
    }
}

/// The scale of the unit's own node, the `SelectedAni*` mark and the connectors.
pub const SCALE: f32 = 1.0;
/// The scale of the `Center*` of the last node of a turn and of the destination.
// re: cvMovementPathVisSystem::UpdateFromHandle 0x088c08c6 (D3DXMatrixScaling 1.5)
pub const BIG_SCALE: f32 = 1.5;
/// The scale of the `Center*` of the other nodes.
// re: cvMovementPathVisSystem::UpdateFromHandle 0x088c08c6 (D3DXMatrixScaling 0.75)
pub const SMALL_SCALE: f32 = 0.75;
/// Connectors on an edge (half of them on the last edge).
// re: cvMovementPathVisSystem::UpdateFromHandle 0x088c08c6 (LoadConnectorModelsIntoRenderList(.., 6, bLast))
pub const CONNECTORS: u32 = 6;

/// The yaw that turns the model's +X axis to `d` (X east, Z south).
pub fn yaw_of(d: Vec2) -> f32 {
    (-d.y).atan2(d.x)
}

/// The yaw that turns the model's north axis (the original's +y, our -Z) to `d`: the exe's basis of a
/// connector, `x -> side = (dir.y, -dir.x)`, `y -> dir`.
// re: cvMovementPathVisSystem::LoadConnectorModelsIntoRenderList 0x088c1e8e
pub fn connector_yaw(d: Vec2) -> f32 {
    yaw_of(d) - std::f32::consts::FRAC_PI_2
}

/// The pieces of the path on a map of `shape`, `ground` giving the height of the relief at `(x, z)`.
///
/// A path of one node is drawn only if it is an unreachable target. A step that is not on the map is
/// left out; the steps are laid in one frame (a step over the seam goes on in the same direction), so
/// a path over the seam is one piece with no break.
pub fn pieces(path: &PathInput, shape: &GridShape, ground: &dyn Fn(Vec2) -> f32) -> Vec<Piece> {
    let steps: Vec<PathStep> = path
        .steps
        .iter()
        .filter_map(|s| {
            let (x, y) = shape.normalize(s.x, s.y)?;
            Some(PathStep { x, y, ..*s })
        })
        .collect();
    let at = unrolled_path(shape, &steps);
    let place = |p: Vec2| Vec3::new(p.x, ground(p), p.y);
    let mut out = Vec::new();
    if let [only] = steps.as_slice() {
        if only.flags & PathFlags::UNREACHABLE != 0 {
            let look = Look::Inaccessible;
            for kind in [PieceKind::Center, PieceKind::Selected] {
                out.push(Piece {
                    kind,
                    look,
                    position: place(at[0]),
                    yaw: 0.0,
                    scale: SCALE,
                });
            }
        }
        return out;
    }
    if steps.len() < 2 {
        return out;
    }
    let last = steps.len() - 1;
    let ends = turn_markers(&steps);
    let mut prev = place(at[0]);
    out.push(Piece {
        kind: PieceKind::Center,
        look: Look::OneTurn,
        position: prev,
        yaw: 0.0,
        scale: SCALE,
    });
    for i in 1..=last {
        let pos = place(at[i]);
        let look = Look::of(&steps[i]);
        if i == last {
            out.push(Piece {
                kind: PieceKind::Selected,
                look,
                position: pos,
                yaw: 0.0,
                scale: SCALE,
            });
        }
        out.push(Piece {
            kind: PieceKind::Center,
            look,
            position: pos,
            yaw: 0.0,
            scale: if ends.contains(&i) {
                BIG_SCALE
            } else {
                SMALL_SCALE
            },
        });
        let yaw = connector_yaw(at[i] - at[i - 1]);
        let step = (pos - prev) / (CONNECTORS as f32 + 1.0);
        let n = CONNECTORS >> u32::from(i == last);
        for k in 1..=n {
            out.push(Piece {
                kind: PieceKind::Connector,
                look,
                position: prev + step * k as f32,
                yaw,
                scale: SCALE,
            });
        }
        prev = pos;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hexgrid::HEX_WIDTH;

    fn shape(wrap: bool) -> GridShape {
        GridShape {
            width: 8,
            height: 8,
            wrap_x: wrap,
        }
    }

    fn step(x: i32, y: i32, turn: i32, flags: u16) -> PathStep {
        PathStep { x, y, turn, flags }
    }

    fn flat(_: Vec2) -> f32 {
        0.0
    }

    #[test]
    fn the_look_follows_the_flags_then_the_turn() {
        assert_eq!(Look::of(&step(0, 0, 0, 0)), Look::OneTurn);
        assert_eq!(Look::of(&step(0, 0, 1, 0)), Look::OneTurn);
        assert_eq!(Look::of(&step(0, 0, 2, 0)), Look::TwoTurns);
        assert_eq!(Look::of(&step(0, 0, 3, 0)), Look::ThreeTurns);
        assert_eq!(Look::of(&step(0, 0, 9, 0)), Look::ThreeTurns);
        assert_eq!(Look::of(&step(0, 0, 1, PathFlags::EMBARK)), Look::Embark);
        assert_eq!(Look::of(&step(0, 0, 2, PathFlags::ATTACK)), Look::Attack);
        assert_eq!(
            Look::of(&step(0, 0, 1, PathFlags::UNREACHABLE | PathFlags::ATTACK)),
            Look::Inaccessible
        );
    }

    #[test]
    fn the_models_have_the_names_of_the_files_of_the_original() {
        assert_eq!(
            piece_model(PieceKind::Center, Look::OneTurn).as_str(),
            "md:centeroneturn"
        );
        assert_eq!(
            piece_model(PieceKind::Connector, Look::Attack).as_str(),
            "md:connectorattack"
        );
        assert_eq!(
            piece_model(PieceKind::Selected, Look::ThreeTurns).as_str(),
            "md:selectedanithreeturns"
        );
        assert_eq!(
            piece_model(PieceKind::Center, Look::Inaccessible).as_str(),
            "md:centerinaccessible"
        );
    }

    fn of(p: &[Piece], kind: PieceKind) -> Vec<Piece> {
        p.iter().filter(|x| x.kind == kind).copied().collect()
    }

    /// The exe's render list: a centre per node, six connectors per edge (three on the last), the
    /// mark on the destination; the looks follow the turn of the node an edge leads to.
    #[test]
    fn a_path_has_six_connectors_per_edge_a_center_per_node_and_a_mark_at_the_end() {
        let path = PathInput {
            steps: vec![
                step(1, 1, 1, 0),
                step(2, 1, 1, 0),
                step(3, 1, 2, 0),
                step(4, 1, 3, 0),
            ],
        };
        let p = pieces(&path, &shape(false), &flat);
        let con = of(&p, PieceKind::Connector);
        assert_eq!(con.len(), 6 + 6 + 3);
        assert_eq!(of(&p, PieceKind::Center).len(), 4);
        assert_eq!(of(&p, PieceKind::Selected).len(), 1);
        let looks: Vec<Look> = con.iter().map(|x| x.look).collect();
        assert_eq!(&looks[..6], [Look::OneTurn; 6]);
        assert_eq!(&looks[6..12], [Look::TwoTurns; 6]);
        assert_eq!(&looks[12..], [Look::ThreeTurns; 3]);
        let mark = of(&p, PieceKind::Selected)[0];
        assert_eq!((mark.look, mark.scale), (Look::ThreeTurns, SCALE));
    }

    /// The unit's own node is `CenterOneTurn` unscaled; the end of a turn and the destination are big,
    /// the other nodes small.
    #[test]
    fn the_centers_are_big_at_the_end_of_a_turn_and_small_elsewhere() {
        let path = PathInput {
            steps: vec![
                step(1, 1, 1, 0),
                step(2, 1, 1, 0),
                step(3, 1, 1, 0),
                step(4, 1, 2, 0),
                step(5, 1, 2, 0),
            ],
        };
        let c = of(&pieces(&path, &shape(false), &flat), PieceKind::Center);
        let scales: Vec<f32> = c.iter().map(|x| x.scale).collect();
        assert_eq!(
            scales,
            [SCALE, SMALL_SCALE, BIG_SCALE, SMALL_SCALE, BIG_SCALE]
        );
        assert_eq!(c[0].look, Look::OneTurn);
        assert_eq!(c[4].look, Look::TwoTurns);
    }

    /// Connector `k` of an edge stands at `k / 7` of the way from the node before, its north axis
    /// along the edge.
    #[test]
    fn connectors_are_spaced_by_sevenths_and_face_along_the_edge() {
        let path = PathInput {
            steps: vec![step(1, 1, 1, 0), step(2, 1, 1, 0), step(3, 1, 1, 0)],
        };
        let p = pieces(&path, &shape(false), &flat);
        let con = of(&p, PieceKind::Connector);
        let a = crate::hexgrid::cell_center(1, 1);
        for (k, c) in con[..6].iter().enumerate() {
            let want = a.x + HEX_WIDTH * (k as f32 + 1.0) / 7.0;
            assert!((c.position.x - want).abs() < 1e-3, "{k}: {}", c.position.x);
            // The model's north (-Z) turned by the yaw points east.
            let (s, co) = c.yaw.sin_cos();
            let north = Vec2::new(-s, -co);
            assert!((north - Vec2::X).length() < 1e-5, "{north}");
        }
    }

    #[test]
    fn embark_and_attack_nodes_have_their_looks() {
        let path = PathInput {
            steps: vec![
                step(1, 1, 1, 0),
                step(2, 1, 1, PathFlags::EMBARK),
                step(3, 1, 1, PathFlags::ATTACK),
            ],
        };
        let p = pieces(&path, &shape(false), &flat);
        let c = of(&p, PieceKind::Center);
        assert_eq!(c[1].look, Look::Embark);
        assert_eq!(c[2].look, Look::Attack);
        assert_eq!(of(&p, PieceKind::Selected)[0].look, Look::Attack);
    }

    #[test]
    fn an_unreachable_target_is_one_node_in_the_inaccessible_look() {
        let path = PathInput {
            steps: vec![step(3, 3, 0, PathFlags::UNREACHABLE)],
        };
        let p = pieces(&path, &shape(false), &flat);
        assert_eq!(p.len(), 2);
        assert!(p.iter().all(|x| x.look == Look::Inaccessible));
        // A lone reachable node is not a path.
        let lone = PathInput {
            steps: vec![step(3, 3, 0, 0)],
        };
        assert!(pieces(&lone, &shape(false), &flat).is_empty());
        assert!(pieces(&PathInput::default(), &shape(false), &flat).is_empty());
    }

    #[test]
    fn the_height_is_the_reliefs() {
        let path = PathInput {
            steps: vec![step(1, 1, 0, 0), step(2, 1, 1, 0)],
        };
        let hill = |p: Vec2| 10.0 + p.x * 0.01;
        for piece in pieces(&path, &shape(false), &hill) {
            let want = 10.0 + piece.position.x * 0.01;
            assert!((piece.position.y - want).abs() < 1e-4);
        }
    }

    #[test]
    fn the_yaw_of_a_direction() {
        assert!((yaw_of(Vec2::new(0.0, -1.0)) - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        assert!(connector_yaw(Vec2::new(0.0, -1.0)).abs() < 1e-5, "north");
    }

    /// A path over the seam of a wrapping map is one run: the centres follow each other at one cell
    /// and the frame does not jump.
    #[test]
    fn a_path_over_the_seam_is_one_piece() {
        let path = PathInput {
            steps: vec![
                step(6, 2, 1, 0),
                step(7, 2, 1, 0),
                step(8, 2, 2, 0),
                step(9, 2, 2, 0),
            ],
        };
        let p = pieces(&path, &shape(true), &flat);
        let xs: Vec<f32> = of(&p, PieceKind::Center)
            .iter()
            .map(|x| x.position.x)
            .collect();
        assert_eq!(xs.len(), 4);
        for w in xs.windows(2) {
            assert!((w[1] - w[0] - HEX_WIDTH).abs() < 1e-3, "{xs:?}");
        }
    }
}
