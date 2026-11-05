//! What the controller asks the renderer to highlight: the selected
//! unit or city, the cells it reaches, the cells it can attack, and the path under the mouse. All of
//! it is read off the contexts (net-client 4.6): the nodes of the move tree, its `ATTACK` nodes, the
//! ranged targets, the path up the tree to the plot under the mouse.

use client_core::UnitContextExt;
use protocol::{Shooter, WorldView};
use render::entities::{Highlights, PathStep};

use super::{Mode, PlayController};

impl PlayController {
    /// The highlights for `world`: the ring on the selected unit (an open city window has none: the
    /// original draws the ring only at a unit, `UnitSelectionHighlight`, lobby-ui 4.10),
    /// the reachable cells while a unit is selected, the red cells (melee targets of the tree and the
    /// hovered enemy), the shot targets in the ranged-attack mode (the reach is hidden then), and the
    /// path to the plot under the mouse.
    pub fn highlights(&self, world: &WorldView) -> Highlights {
        let cell = |(x, y): (i16, i16)| (i32::from(x), i32::from(y));
        let unit = self
            .selected_unit()
            .and_then(|id| world.units.iter().find(|u| u.id == id));
        let city_cell = |id: i32| {
            world
                .cities
                .iter()
                .find(|c| c.id == id)
                .map(|c| cell((c.x, c.y)))
        };
        let selected = match (self.mode(), unit) {
            // A shooting city is what is selected while it aims.
            (Mode::RangeAttack(Shooter::City(id)), _) => city_cell(id),
            (_, Some(u)) => Some(cell((u.x, u.y))),
            // The window on the right marks an open city; no ring (lobby-ui 4.10).
            (_, None) => None,
        };
        let ranged = matches!(self.mode(), Mode::RangeAttack(_));
        let tree = self.tree().filter(|_| unit.is_some());
        let melee: Vec<(i32, i32)> = tree
            .map(|c| c.attack_cells().into_iter().map(cell).collect())
            .unwrap_or_default();
        let hovered_enemy = self.melee_target(world).map(cell);
        Highlights {
            selected,
            reachable: match tree {
                Some(c) if !ranged => c.move_cells().into_iter().map(cell).collect(),
                _ => Vec::new(),
            },
            targets: match self.mode() {
                Mode::RangeAttack(shooter) => self
                    .ranged_targets(shooter)
                    .unwrap_or_default()
                    .into_iter()
                    .map(cell)
                    .collect(),
                Mode::Normal => {
                    let mut t = melee;
                    t.extend(hovered_enemy.filter(|c| !t.contains(c)));
                    t
                }
            },
            path: if unit.is_some() && !ranged {
                self.path_steps()
            } else {
                Vec::new()
            },
        }
    }

    /// The arrow of the ranged attack (card R5.9): in the ranged-attack mode, from the shooter (the
    /// selected unit or the aiming city) to the hovered plot when it is one of the shot targets.
    /// The original shows it on `OnSpawnArrow` (event 165) with the two plots and hides it on 166.
    pub fn attack_arrow(&self, world: &WorldView) -> Option<((i32, i32), (i32, i32))> {
        let Mode::RangeAttack(shooter) = self.mode() else {
            return None;
        };
        let cell = |(x, y): (i16, i16)| (i32::from(x), i32::from(y));
        let from = match shooter {
            Shooter::City(id) => world.cities.iter().find(|c| c.id == id).map(|c| (c.x, c.y)),
            Shooter::Unit(id) => world.units.iter().find(|u| u.id == id).map(|u| (u.x, u.y)),
        }?;
        let to = self.hovered()?;
        self.ranged_targets(shooter)?
            .contains(&to)
            .then(|| (cell(from), cell(to)))
    }

    /// The path to the hovered plot: from the tree if it holds the plot, else the answer to `Query::Path`
    /// (nothing for a target that cannot be reached: the screen says so, `hover_unreachable`).
    fn path_steps(&self) -> Vec<PathStep> {
        // The core counts the turns from 0 (this turn), the renderer from 1 as the exe does.
        let step = |n: &protocol::PathNode| PathStep {
            x: i32::from(n.x),
            y: i32::from(n.y),
            turn: i32::from(n.turn) + 1,
            flags: n.flags.0,
        };
        if let Some((x, y)) = self.hovered()
            && let Some(way) = self.tree().and_then(|c| c.path_to(x, y))
        {
            return way.iter().map(step).collect();
        }
        if self.hover_unreachable() {
            return Vec::new();
        }
        self.hints().path.iter().map(step).collect()
    }
}

#[cfg(test)]
mod tests {
    use client_core::{ContextView, Contexts};
    use protocol::{
        ContextMsg, PathNode, Query, QueryAnswer, ReachFlags, ReachNode, Shooter, TargetKind,
        TargetView, UnitContext,
    };

    use super::*;

    /// Fixture: Archer 4 at (8, 11), Settler 1 at (7, 12), Roma (city 1) at (6, 12).
    fn world() -> WorldView {
        crate::test_world::world()
    }

    fn node(x: i16, y: i16, parent: u16, turn: u8, flags: u16) -> ReachNode {
        ReachNode {
            x,
            y,
            parent,
            turn,
            moves_left: 0,
            flags: ReachFlags(flags),
        }
    }

    /// The archer's tree: (8,11) root; (9,11), (9,12) next to it; (10,11) behind (9,11) on turn 1; an
    /// enemy at (9,10) to attack.
    fn archer_ctx() -> UnitContext {
        UnitContext {
            unit: 4,
            view_seq: 1,
            reach: vec![
                node(8, 11, 0, 0, 0),
                node(9, 11, 0, 0, 0),
                node(9, 12, 0, 0, 0),
                node(10, 11, 1, 1, 0),
                node(9, 10, 0, 0, ReachFlags::ATTACK),
            ],
            targets: vec![
                TargetView {
                    x: 10,
                    y: 11,
                    kind: TargetKind::Ranged,
                    flags: 0,
                    preview: None,
                },
                TargetView {
                    x: 9,
                    y: 9,
                    kind: TargetKind::Ranged,
                    flags: 0,
                    preview: None,
                },
            ],
            ..UnitContext::default()
        }
    }

    /// The controller has the archer's context, fresh.
    fn with_context(c: &mut PlayController) {
        let mut store = Contexts::new();
        store.apply(ContextMsg::Unit(archer_ctx()));
        c.sync_contexts(&ContextView::new(&store, 1));
    }

    #[test]
    fn nothing_selected_nothing_highlighted() {
        let w = world();
        assert!(PlayController::new().highlights(&w).is_empty());
    }

    #[test]
    fn a_selected_unit_gets_the_ring_the_reach_the_red_cells_and_the_path() {
        let w = world();
        let mut c = PlayController::new();
        let out = c.left_click(&w, (8, 11));
        assert_eq!(c.selected_unit(), Some(4));
        assert!(out.queries.is_empty(), "the contexts come with the focus");
        // Before the context: the ring only.
        let h = c.highlights(&w);
        assert_eq!(h.selected, Some((8, 11)));
        assert!(h.reachable.is_empty() && h.targets.is_empty() && h.path.is_empty());
        with_context(&mut c);
        // The mouse over a plot of the tree: the path comes from the tree, nothing is asked.
        let out = c.hover(&w, Some((10, 11)), 0);
        assert!(out.queries.is_empty());
        let h = c.highlights(&w);
        assert_eq!(h.selected, Some((8, 11)));
        assert_eq!(
            h.reachable,
            [(9, 11), (9, 12)],
            "turn 1 and attacks are not this turn's moves"
        );
        assert_eq!(h.targets, [(9, 10)], "the ATTACK node is red");
        assert_eq!(
            h.path,
            [
                PathStep {
                    x: 8,
                    y: 11,
                    turn: 1,
                    flags: 0
                },
                PathStep {
                    x: 9,
                    y: 11,
                    turn: 1,
                    flags: 0
                },
                PathStep {
                    x: 10,
                    y: 11,
                    turn: 2,
                    flags: 0
                },
            ]
        );
    }

    #[test]
    fn a_plot_outside_the_tree_asks_for_its_path_after_the_dwell() {
        let w = world();
        let mut c = PlayController::new();
        c.left_click(&w, (8, 11));
        with_context(&mut c);
        // (20, 3) is not in the tree: not before 150 ms, then once.
        assert!(c.hover(&w, Some((20, 3)), 1_000).queries.is_empty());
        assert!(c.hover(&w, Some((20, 3)), 1_149).queries.is_empty());
        let out = c.hover(&w, Some((20, 3)), 1_150);
        assert_eq!(
            out.queries,
            [Query::Path {
                unit: 4,
                x: 20,
                y: 3
            }]
        );
        c.query_sent(7, &out.queries[0]);
        assert!(
            c.hover(&w, Some((20, 3)), 1_400).queries.is_empty(),
            "one in flight, and this plot was asked"
        );
        // Another plot while one is in flight waits for the answer.
        assert!(c.hover(&w, Some((21, 3)), 1_500).queries.is_empty());
        assert!(c.hover(&w, Some((21, 3)), 1_700).queries.is_empty());
        c.on_query_result(
            7,
            &QueryAnswer::Path(vec![
                PathNode {
                    x: 8,
                    y: 11,
                    turn: 0,
                    flags: Default::default(),
                },
                PathNode {
                    x: 20,
                    y: 3,
                    turn: 4,
                    flags: Default::default(),
                },
            ]),
        );
        // The answer is for the plot the mouse left: the new plot asks.
        let out = c.hover(&w, Some((21, 3)), 1_800);
        assert_eq!(
            out.queries,
            [Query::Path {
                unit: 4,
                x: 21,
                y: 3
            }]
        );
        c.query_sent(8, &out.queries[0]);
        c.on_query_result(
            8,
            &QueryAnswer::Path(vec![
                PathNode {
                    x: 8,
                    y: 11,
                    turn: 0,
                    flags: Default::default(),
                },
                PathNode {
                    x: 21,
                    y: 3,
                    turn: 5,
                    flags: Default::default(),
                },
            ]),
        );
        let h = c.highlights(&w);
        assert_eq!(
            h.path.last(),
            Some(&PathStep {
                x: 21,
                y: 3,
                turn: 6,
                flags: 0
            })
        );
        assert!(!c.hover_unreachable());
    }

    #[test]
    fn an_unreachable_target_draws_no_path_and_says_so() {
        let w = world();
        let mut c = PlayController::new();
        c.left_click(&w, (8, 11));
        let out = c.hover(&w, Some((12, 12)), 500);
        assert!(out.queries.is_empty(), "the dwell has not passed");
        let out = c.hover(&w, Some((12, 12)), 700);
        c.query_sent(3, &out.queries[0]);
        c.on_query_result(
            3,
            &QueryAnswer::Path(vec![PathNode {
                x: 12,
                y: 12,
                turn: 0,
                flags: ReachFlags(ReachFlags::UNREACHABLE),
            }]),
        );
        assert!(c.hover_unreachable());
        assert!(c.highlights(&w).path.is_empty());
        // A new view forgets the answer.
        c.on_world(&w);
        assert!(!c.hover_unreachable());
    }

    #[test]
    fn a_query_that_went_nowhere_is_not_asked_again_for_the_same_plot() {
        let w = world();
        let mut c = PlayController::new();
        c.left_click(&w, (8, 11));
        c.hover(&w, Some((12, 12)), 500);
        let out = c.hover(&w, Some((12, 12)), 700);
        // The sink refused it (no connection): id 0.
        c.query_sent(0, &out.queries[0]);
        assert!(c.hover(&w, Some((12, 12)), 900).queries.is_empty());
    }

    #[test]
    fn the_ranged_mode_shows_targets_instead_of_reach_and_path() {
        let w = world();
        let mut c = PlayController::new();
        c.left_click(&w, (8, 11));
        with_context(&mut c);
        let out = c.begin_range_attack(&w);
        assert!(out.queries.is_empty(), "the targets are in the context");
        let h = c.highlights(&w);
        assert_eq!(h.selected, Some((8, 11)));
        assert!(h.reachable.is_empty() && h.path.is_empty());
        assert_eq!(h.targets, [(10, 11), (9, 9)]);
        assert_eq!(c.mode(), Mode::RangeAttack(Shooter::Unit(4)));
    }

    /// R5.9: the arrow runs from the archer to a shot target under the mouse, and only there.
    #[test]
    fn the_ranged_mode_draws_the_arrow_to_a_hovered_target() {
        let w = world();
        let mut c = PlayController::new();
        c.left_click(&w, (8, 11));
        with_context(&mut c);
        c.hover(&w, Some((10, 11)), 0);
        assert_eq!(c.attack_arrow(&w), None, "no arrow outside the ranged mode");
        c.begin_range_attack(&w);
        assert_eq!(c.attack_arrow(&w), Some(((8, 11), (10, 11))));
        c.hover(&w, Some((9, 12)), 10);
        assert_eq!(c.attack_arrow(&w), None, "not a target");
        c.hover(&w, Some((9, 9)), 20);
        assert_eq!(c.attack_arrow(&w), Some(((8, 11), (9, 9))));
    }

    #[test]
    fn an_open_city_window_has_no_ring() {
        let w = world();
        let mut c = PlayController::new();
        // Ostia (city 2) at (9, 17) has no unit on its plot: the click opens its window.
        c.left_click(&w, (9, 17));
        assert_eq!(c.city_window(), Some(2));
        let h = c.highlights(&w);
        assert_eq!(h.selected, None, "the window marks the city, not a ring");
        assert!(h.reachable.is_empty() && h.targets.is_empty() && h.path.is_empty());
    }

    #[test]
    fn a_vanished_unit_is_not_highlighted() {
        let w = world();
        let mut c = PlayController::new();
        c.left_click(&w, (8, 11));
        with_context(&mut c);
        let mut gone = w.clone();
        gone.units.clear();
        assert!(c.highlights(&gone).is_empty());
    }

    #[test]
    fn the_context_of_another_unit_is_not_used() {
        let w = world();
        let mut c = PlayController::new();
        c.left_click(&w, (8, 11));
        with_context(&mut c);
        // Selected the horseman: the archer's tree is not his.
        c.select_unit(&w, 5);
        let h = c.highlights(&w);
        assert!(h.reachable.is_empty() && h.targets.is_empty());
    }
}
