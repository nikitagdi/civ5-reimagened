//! The movement preview of a game UI that keeps its own selection (UI v2): the ring on the selected unit, the
//! plots it reaches this turn, its attack or shot targets, and the path to the plot under the mouse with a turn
//! badge where each turn of the move ends, as the original's `WorldView.lua` (the move range), `Bombardment.lua`
//! (the shot targets) and `PathHelpManager.lua` (the turn badges) show them.
//!
//! The UI owns the selection and the map mode; the app gives [`MovePreview::step`] that focus, the plot under the
//! mouse, the shown world and the unit's context (its move tree, which comes with the focus the UI sends). A plot
//! the tree holds has its path there; for any other the path is asked (`Query::Path`) once the mouse has rested
//! [`PATH_DWELL_MS`] on it, one question in flight at a time, and an answer for another plot, unit or world
//! revision is not drawn.

use client_core::UnitContextExt;
use protocol::{PathNode, Query, QueryAnswer, ReachFlags, TurnStatus, UnitContext, WorldView};
use render::entities::{Highlights, PathStep, highlight::turn_markers};
use render::world::overlay::highlights::HighlightSet;

use super::PATH_DWELL_MS;

/// What a click on the map does in the UI's mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusMode {
    /// A click selects; the path follows the mouse.
    #[default]
    Select,
    /// The next click is the unit's move.
    Move,
    /// The next click is the unit's ranged attack: the targets show instead of the reach and the path.
    Ranged,
}

/// How the map draws the plots of a targeting mode (the server's `targets.look`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AimLook {
    /// A ranged attack of a unit or a city: the radius and the targets (`Bombardment.lua`).
    Fire,
    /// An air strike or a nuke: the air range and the targets.
    Air,
    /// An air sweep.
    Sweep,
    /// A move to a plot (rebase, paradrop, airlift, route to).
    Move,
    /// A move-attack.
    Attack,
    /// The generic unit border of the original's other modes (embark, the gifts to a city-state).
    Mark,
}

/// The map's targeting mode of the UI (the original's interface modes that ask for a plot): the plots a click gives
/// the order on and the radius drawn around them, as the server listed them. Empty while its answer is on its way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Aim {
    pub look: AimLook,
    pub plots: Vec<(i32, i32)>,
    pub area: Vec<(i32, i32)>,
}

/// The unit the UI has selected (one of the seat's) and its mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapFocus {
    pub owner: i8,
    pub unit: i32,
    pub mode: FocusMode,
}

/// A turn badge of the path: the plot and the turn (1: this turn) the badge shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurnBadge {
    pub x: i16,
    pub y: i16,
    pub turn: i16,
}

/// What [`MovePreview::step`] reads in one frame.
pub struct PreviewInput<'a> {
    pub world: &'a WorldView,
    /// The context of the focused unit, if the client core has one (any other unit's is ignored).
    pub ctx: Option<&'a UnitContext>,
    pub focus: Option<MapFocus>,
    /// The cell under the mouse when the mouse is over the map (not over a panel, not dragging).
    pub hover: Option<(i32, i32)>,
    pub now_ms: u64,
    /// The revision of the shown world: an answer of an earlier one is not drawn.
    pub revision: u64,
    /// The UI's targeting mode: its plots replace the reach and the path.
    pub aim: Option<&'a Aim>,
}

/// The preview's state between frames.
#[derive(Debug, Default)]
pub struct MovePreview {
    focus: Option<MapFocus>,
    hover: Option<(i16, i16)>,
    hover_since: u64,
    revision: Option<u64>,
    /// The `Query::Path` in flight and what it is for.
    query: Option<u32>,
    asked: Option<(i32, i16, i16)>,
    /// The last answer and what it was for.
    answer: Option<((i32, i16, i16), Vec<PathNode>)>,
    shown: Highlights,
    ranged: bool,
    /// The targeting mode's look and radius while it is on.
    aim: Option<(AimLook, Vec<(i32, i32)>)>,
    badges: Vec<TurnBadge>,
}

impl MovePreview {
    /// Follows the frame's focus, mouse and world. `Some`: the path query to send now (then [`query_sent`]).
    ///
    /// [`query_sent`]: Self::query_sent
    pub fn step(&mut self, input: &PreviewInput<'_>) -> Option<Query> {
        let world = input.world;
        if self.focus != input.focus {
            self.focus = input.focus;
            self.asked = None;
            self.answer = None;
        }
        if self.revision != Some(input.revision) {
            // The unit may have moved or the map changed: an earlier answer is not drawn, the mouse asks again.
            self.revision = Some(input.revision);
            self.asked = None;
            self.answer = None;
        }
        let at = input.hover.and_then(|c| plot_of(world, c));
        if at != self.hover {
            self.hover = at;
            self.hover_since = input.now_ms;
        }
        let unit = input.focus.and_then(|f| {
            world
                .units
                .iter()
                .find(|u| u.id == f.unit && u.owner == f.owner && u.owner == world.human)
                .map(|u| (f, u))
        });
        if let Some(aim) = input.aim {
            // The targeting mode: the server's plots, the ring on the unit that acts; no reach, no path.
            self.badges.clear();
            self.ranged = false;
            self.aim = Some((aim.look, aim.area.clone()));
            self.shown = Highlights {
                selected: unit.map(|(_, u)| (i32::from(u.x), i32::from(u.y))),
                reachable: Vec::new(),
                targets: aim.plots.clone(),
                path: Vec::new(),
            };
            return None;
        }
        self.aim = None;
        let Some((focus, u)) = unit else {
            self.shown = Highlights::default();
            self.ranged = false;
            self.badges.clear();
            return None;
        };
        let cell = |(x, y): (i16, i16)| (i32::from(x), i32::from(y));
        let tree = input.ctx.filter(|c| c.unit == u.id);
        let our_turn = world.status == TurnStatus::HumanTurn;
        let ranged = focus.mode == FocusMode::Ranged;
        let mut ask = None;
        if let Some((x, y)) = self.hover
            && our_turn
            && !ranged
            && (u.x, u.y) != (x, y)
            && !tree.is_some_and(|c| c.reaches(x, y))
            && input.now_ms.saturating_sub(self.hover_since) >= PATH_DWELL_MS
            && self.query.is_none()
            && self.asked != Some((u.id, x, y))
        {
            self.asked = Some((u.id, x, y));
            ask = Some(Query::Path { unit: u.id, x, y });
        }
        let path = match self.hover {
            Some((x, y)) if our_turn && !ranged && (u.x, u.y) != (x, y) => {
                let from_tree = tree.and_then(|c| c.path_to(x, y));
                let answered = self
                    .answer
                    .as_ref()
                    .filter(|(k, _)| *k == (u.id, x, y))
                    .map(|(_, nodes)| nodes.clone())
                    .filter(|nodes| !unreachable(nodes));
                from_tree.or(answered).unwrap_or_default()
            }
            _ => Vec::new(),
        };
        let steps: Vec<PathStep> = path
            .iter()
            .map(|n| PathStep {
                x: i32::from(n.x),
                y: i32::from(n.y),
                // The core counts the turns from 0 (this turn), the renderer and the badges from 1 as the exe does.
                turn: i32::from(n.turn) + 1,
                flags: n.flags.0,
            })
            .collect();
        self.badges = badges(&steps);
        self.ranged = ranged;
        self.shown = Highlights {
            selected: Some(cell((u.x, u.y))),
            reachable: match tree {
                Some(c) if our_turn && !ranged => c.move_cells().into_iter().map(cell).collect(),
                _ => Vec::new(),
            },
            targets: match tree {
                Some(c) if our_turn && ranged => c
                    .ranged_targets()
                    .iter()
                    .map(|t| cell((t.x, t.y)))
                    .collect(),
                Some(c) if our_turn => c.attack_cells().into_iter().map(cell).collect(),
                _ => Vec::new(),
            },
            path: steps,
        };
        ask
    }

    /// The query [`step`](Self::step) asked for went out with this id (0: the core did not send it, and the same
    /// plot is not asked again).
    pub fn query_sent(&mut self, id: u32) {
        if id != 0 {
            self.query = Some(id);
        }
    }

    /// An answer of the client core; one for another query is ignored.
    pub fn on_query_result(&mut self, id: u32, answer: &QueryAnswer) {
        if self.query != Some(id) {
            return;
        }
        self.query = None;
        match answer {
            QueryAnswer::Path(nodes) => {
                if let Some(k) = self.asked {
                    self.answer = Some((k, nodes.clone()));
                }
            }
            QueryAnswer::Error { reason } => tracing::debug!("path query {id} failed: {reason}"),
            _ => {}
        }
    }

    /// The game was left or the UI went: nothing is shown and nothing waits.
    pub fn clear(&mut self) {
        *self = MovePreview::default();
    }

    /// The ring, the reach, the targets and the path to draw.
    pub fn highlights(&self) -> &Highlights {
        &self.shown
    }

    /// The styles of `Highlights.xml` for the reach and the targets (the ranged mode's styles in that mode).
    pub fn highlight_set(&self) -> HighlightSet {
        if let Some((look, area)) = &self.aim {
            return super::highlight_calls::aim_set(*look, area, &self.shown.targets);
        }
        super::highlight_calls::from_highlights(&self.shown, self.ranged)
    }

    /// The targeting mode's radius while it is on (for the test probe).
    pub fn aim_area(&self) -> Option<&[(i32, i32)]> {
        self.aim.as_ref().map(|(_, area)| area.as_slice())
    }

    /// The turn badges of the path.
    pub fn badges(&self) -> &[TurnBadge] {
        &self.badges
    }
}

/// The path was asked and the target cannot be reached: nothing is drawn.
fn unreachable(nodes: &[PathNode]) -> bool {
    matches!(nodes, [n] if n.flags.contains(ReachFlags::UNREACHABLE))
}

/// The badges of a path as `PathHelpManager.lua`'s `OnPath` places them: the last step of each turn and the
/// target, never the start.
pub fn badges(steps: &[PathStep]) -> Vec<TurnBadge> {
    turn_markers(steps)
        .into_iter()
        .filter_map(|i| {
            let s = steps[i];
            Some(TurnBadge {
                x: i16::try_from(s.x).ok()?,
                y: i16::try_from(s.y).ok()?,
                turn: i16::try_from(s.turn.max(1)).ok()?,
            })
        })
        .collect()
}

fn plot_of(world: &WorldView, (x, y): (i32, i32)) -> Option<(i16, i16)> {
    let m = &world.map;
    if x < 0 || y < 0 || x >= i32::from(m.width) || y >= i32::from(m.height) {
        return None;
    }
    Some((x as i16, y as i16))
}

#[cfg(test)]
mod tests {
    use protocol::{ReachNode, TargetKind, TargetView};

    use super::*;

    /// Fixture: Archer 4 at (8, 11) of the human (player 0), Settler 1 at (7, 12).
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

    /// The archer's tree: (8,11) root; (9,11), (9,12) this turn; (10,11) behind (9,11) on turn 1; an enemy at
    /// (9,10) to attack; a ranged target at (10,11).
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
            targets: vec![TargetView {
                x: 10,
                y: 11,
                kind: TargetKind::Ranged,
                flags: 0,
                preview: None,
            }],
            ..UnitContext::default()
        }
    }

    fn archer(w: &WorldView, mode: FocusMode) -> Option<MapFocus> {
        let u = w.units.iter().find(|u| u.id == 4).expect("archer");
        Some(MapFocus {
            owner: u.owner,
            unit: 4,
            mode,
        })
    }

    fn input<'a>(
        w: &'a WorldView,
        ctx: Option<&'a UnitContext>,
        focus: Option<MapFocus>,
        hover: Option<(i32, i32)>,
        now_ms: u64,
    ) -> PreviewInput<'a> {
        PreviewInput {
            world: w,
            ctx,
            focus,
            hover,
            now_ms,
            revision: 1,
            aim: None,
        }
    }

    #[test]
    fn a_targeting_mode_shows_the_servers_plots_and_radius_and_no_reach_or_path() {
        let w = world();
        let ctx = archer_ctx();
        let aim = Aim {
            look: AimLook::Fire,
            plots: vec![(10, 11)],
            area: vec![(9, 11), (10, 11)],
        };
        let mut p = MovePreview::default();
        let mut i = input(
            &w,
            Some(&ctx),
            archer(&w, FocusMode::Select),
            Some((20, 3)),
            5_000,
        );
        i.aim = Some(&aim);
        assert_eq!(p.step(&i), None, "no path is asked while aiming");
        let h = p.highlights();
        assert_eq!(h.selected, Some((8, 11)), "the ring stays on the unit");
        assert!(h.reachable.is_empty() && h.path.is_empty() && p.badges().is_empty());
        assert_eq!(h.targets, [(10, 11)]);
        let set = p.highlight_set();
        let styles: Vec<(&str, i32, i32)> = set
            .cells
            .iter()
            .map(|c| (c.style.as_str(), c.x, c.y))
            .collect();
        assert_eq!(
            styles,
            [
                ("FireRangeBorder", 9, 11),
                ("FireRangeBorder", 10, 11),
                ("ValidFireTargetBorder", 10, 11)
            ]
        );
        assert_eq!(p.aim_area(), Some(&[(9, 11), (10, 11)][..]));
        // A city's strike or a gift has no unit: the plots without a ring.
        let mut p = MovePreview::default();
        let mut i = input(&w, None, None, None, 0);
        let gift = Aim {
            look: AimLook::Mark,
            plots: vec![(7, 12)],
            area: Vec::new(),
        };
        i.aim = Some(&gift);
        p.step(&i);
        assert_eq!(p.highlights().selected, None);
        assert_eq!(p.highlights().targets, [(7, 12)]);
        assert_eq!(p.highlight_set().cells[0].style, "GUHB");
        // The mode ends: the usual preview comes back.
        p.step(&input(
            &w,
            Some(&ctx),
            archer(&w, FocusMode::Select),
            None,
            10,
        ));
        assert_eq!(p.aim_area(), None);
        assert_eq!(p.highlights().reachable, [(9, 11), (9, 12)]);
    }

    #[test]
    fn nothing_selected_shows_nothing() {
        let w = world();
        let mut p = MovePreview::default();
        assert_eq!(p.step(&input(&w, None, None, Some((9, 11)), 0)), None);
        assert!(p.highlights().is_empty() && p.badges().is_empty());
        assert!(p.highlight_set().is_empty());
    }

    #[test]
    fn a_selected_unit_has_the_ring_the_reach_and_the_tree_path_with_badges() {
        let w = world();
        let ctx = archer_ctx();
        let mut p = MovePreview::default();
        // Before the tree comes: the ring only.
        p.step(&input(&w, None, archer(&w, FocusMode::Select), None, 0));
        assert_eq!(p.highlights().selected, Some((8, 11)));
        assert!(p.highlights().reachable.is_empty());
        // With the tree, the mouse on a plot of turn 2: the path comes from the tree, nothing is asked.
        let ask = p.step(&input(
            &w,
            Some(&ctx),
            archer(&w, FocusMode::Select),
            Some((10, 11)),
            10,
        ));
        assert_eq!(ask, None);
        let h = p.highlights();
        assert_eq!(h.reachable, [(9, 11), (9, 12)]);
        assert_eq!(h.targets, [(9, 10)], "the ATTACK node");
        assert_eq!(
            h.path
                .iter()
                .map(|s| (s.x, s.y, s.turn))
                .collect::<Vec<_>>(),
            [(8, 11, 1), (9, 11, 1), (10, 11, 2)]
        );
        // A badge where turn 1 ends and one at the target.
        assert_eq!(
            p.badges(),
            [
                TurnBadge {
                    x: 9,
                    y: 11,
                    turn: 1
                },
                TurnBadge {
                    x: 10,
                    y: 11,
                    turn: 2
                }
            ]
        );
        let set = p.highlight_set();
        let styles: Vec<&str> = set.cells.iter().map(|c| c.style.as_str()).collect();
        assert_eq!(
            styles,
            ["MovementRangeBorder", "MovementRangeBorder", "AMRBorder"]
        );
    }

    #[test]
    fn a_plot_outside_the_tree_is_asked_after_the_dwell_and_its_answer_drawn() {
        let w = world();
        let ctx = archer_ctx();
        let f = archer(&w, FocusMode::Select);
        let mut p = MovePreview::default();
        assert_eq!(
            p.step(&input(&w, Some(&ctx), f, Some((20, 3)), 1_000)),
            None
        );
        assert_eq!(
            p.step(&input(&w, Some(&ctx), f, Some((20, 3)), 1_149)),
            None
        );
        let ask = p.step(&input(&w, Some(&ctx), f, Some((20, 3)), 1_150));
        assert_eq!(
            ask,
            Some(Query::Path {
                unit: 4,
                x: 20,
                y: 3
            })
        );
        p.query_sent(7);
        assert_eq!(
            p.step(&input(&w, Some(&ctx), f, Some((20, 3)), 1_300)),
            None,
            "one in flight"
        );
        let path = |turn: i16| {
            QueryAnswer::Path(vec![
                PathNode {
                    x: 8,
                    y: 11,
                    turn: 0,
                    flags: ReachFlags::default(),
                },
                PathNode {
                    x: 20,
                    y: 3,
                    turn,
                    flags: ReachFlags::default(),
                },
            ])
        };
        p.on_query_result(99, &path(3));
        p.step(&input(&w, Some(&ctx), f, Some((20, 3)), 1_310));
        assert!(p.highlights().path.is_empty(), "not our query");
        p.on_query_result(7, &path(3));
        p.step(&input(&w, Some(&ctx), f, Some((20, 3)), 1_320));
        assert_eq!(p.highlights().path.len(), 2);
        assert_eq!(
            p.badges(),
            [TurnBadge {
                x: 20,
                y: 3,
                turn: 4
            }]
        );
        // The mouse moves on: the answer is for the plot it left.
        p.step(&input(&w, Some(&ctx), f, Some((21, 3)), 1_400));
        assert!(p.highlights().path.is_empty() && p.badges().is_empty());
        // A new world revision forgets the answer.
        p.step(&input(&w, Some(&ctx), f, Some((20, 3)), 1_500));
        let mut again = input(&w, Some(&ctx), f, Some((20, 3)), 1_700);
        again.revision = 2;
        p.step(&again);
        assert!(p.highlights().path.is_empty());
    }

    #[test]
    fn an_unreachable_answer_draws_nothing() {
        let w = world();
        let f = archer(&w, FocusMode::Select);
        let mut p = MovePreview::default();
        p.step(&input(&w, None, f, Some((12, 12)), 0));
        assert!(p.step(&input(&w, None, f, Some((12, 12)), 200)).is_some());
        p.query_sent(3);
        p.on_query_result(
            3,
            &QueryAnswer::Path(vec![PathNode {
                x: 12,
                y: 12,
                turn: 0,
                flags: ReachFlags(ReachFlags::UNREACHABLE),
            }]),
        );
        p.step(&input(&w, None, f, Some((12, 12)), 210));
        assert!(p.highlights().path.is_empty() && p.badges().is_empty());
        assert_eq!(p.highlights().selected, Some((8, 11)));
    }

    #[test]
    fn the_ranged_mode_shows_the_targets_and_no_reach_or_path() {
        let w = world();
        let ctx = archer_ctx();
        let mut p = MovePreview::default();
        let ask = p.step(&input(
            &w,
            Some(&ctx),
            archer(&w, FocusMode::Ranged),
            Some((20, 3)),
            5_000,
        ));
        assert_eq!(ask, None, "nothing is asked while aiming");
        let h = p.highlights();
        assert!(h.reachable.is_empty() && h.path.is_empty());
        assert_eq!(h.targets, [(10, 11)]);
        let set = p.highlight_set();
        let styles: Vec<&str> = set.cells.iter().map(|c| c.style.as_str()).collect();
        assert_eq!(styles, ["ValidFireTargetBorder"]);
    }

    #[test]
    fn the_tree_of_another_unit_and_a_foreign_unit_are_not_used() {
        let w = world();
        let ctx = archer_ctx();
        let mut p = MovePreview::default();
        // The settler (1) is selected: the archer's tree is not his.
        let settler = w.units.iter().find(|u| u.id == 1).expect("settler");
        let f = Some(MapFocus {
            owner: settler.owner,
            unit: 1,
            mode: FocusMode::Select,
        });
        p.step(&input(&w, Some(&ctx), f, Some((10, 11)), 0));
        assert!(p.highlights().reachable.is_empty() && p.highlights().path.is_empty());
        // Another owner's unit of that id is not the seat's.
        let f = Some(MapFocus {
            owner: settler.owner + 1,
            unit: 1,
            mode: FocusMode::Select,
        });
        p.step(&input(&w, Some(&ctx), f, None, 0));
        assert!(p.highlights().is_empty());
    }

    #[test]
    fn badges_follow_path_help_manager() {
        let s = |x: i32, turn: i32| PathStep {
            x,
            y: 0,
            turn,
            flags: 0,
        };
        // Start, two steps of turn 1, one of turn 2: turn 1 ends at x 2, the target is x 3.
        let b = badges(&[s(0, 1), s(1, 1), s(2, 1), s(3, 2)]);
        assert_eq!(
            b.iter().map(|b| (b.x, b.turn)).collect::<Vec<_>>(),
            [(2, 1), (3, 2)]
        );
        // A target this turn: one badge "1" on it.
        assert_eq!(
            badges(&[s(0, 1), s(1, 1)])
                .iter()
                .map(|b| (b.x, b.turn))
                .collect::<Vec<_>>(),
            [(1, 1)]
        );
        assert!(badges(&[s(0, 1)]).is_empty());
    }
}
