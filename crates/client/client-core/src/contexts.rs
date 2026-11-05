//! Contexts, focus and the sink the controls send into (net-client 4.6, NC-D6, NC-D7, NC-D13).
//!
//! The server precomputes what the controls need for the units the player is about to command: the move
//! tree, the actions, the targets (`UnitContext`), and the build list of the open city (`CityContext`).
//! The client stores them as the server sent them and never computes a rule. A context is `Fresh` when its
//! `view_seq` equals the client's, `Stale` when older (shown, but not used to predict, NC-D6), `Missing`
//! when there is none.

use std::collections::BTreeMap;

use protocol::{
    CityContext, CmdId, Command, ContextMsg, Focus, PathNode, Query, ReachFlags, ReachNode,
    TargetKind, TargetView, UnitContext,
};

use crate::OrderRefused;

/// How many units follow the selected one in the focus (NC-D7).
pub const FOCUS_NEXT: usize = 3;

/// A context and how good it is for the current view.
#[derive(Debug, PartialEq, Eq)]
pub enum Ctx<'a, T> {
    Fresh(&'a T),
    Stale(&'a T),
    Missing,
}

// By hand: a reference is `Copy` whatever `T` is.
impl<T> Clone for Ctx<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Ctx<'_, T> {}

impl<'a, T> Ctx<'a, T> {
    /// The context whatever its age (to show, not to predict).
    pub fn get(self) -> Option<&'a T> {
        match self {
            Ctx::Fresh(c) | Ctx::Stale(c) => Some(c),
            Ctx::Missing => None,
        }
    }

    /// Only a fresh context.
    pub fn fresh(self) -> Option<&'a T> {
        match self {
            Ctx::Fresh(c) => Some(c),
            _ => None,
        }
    }

    pub fn is_fresh(self) -> bool {
        matches!(self, Ctx::Fresh(_))
    }

    pub fn is_stale(self) -> bool {
        matches!(self, Ctx::Stale(_))
    }

    pub fn is_missing(self) -> bool {
        matches!(self, Ctx::Missing)
    }

    /// `"fresh"`, `"stale"` or `"missing"` (the test port).
    pub fn name(self) -> &'static str {
        match self {
            Ctx::Fresh(_) => "fresh",
            Ctx::Stale(_) => "stale",
            Ctx::Missing => "missing",
        }
    }
}

/// The stored contexts: the units in focus and the city in focus.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Contexts {
    units: BTreeMap<i32, UnitContext>,
    city: Option<CityContext>,
}

impl Contexts {
    pub const fn new() -> Self {
        Contexts {
            units: BTreeMap::new(),
            city: None,
        }
    }

    /// A `Context` message of the stream, as it came.
    pub fn apply(&mut self, msg: ContextMsg) {
        match msg {
            ContextMsg::Unit(c) => {
                self.units.insert(c.unit, c);
            }
            ContextMsg::City(c) => self.city = Some(c),
            ContextMsg::Valid {
                units,
                city,
                view_seq,
            } => {
                for id in units {
                    if let Some(c) = self.units.get_mut(&id) {
                        c.view_seq = view_seq;
                    }
                }
                if let (Some(id), Some(c)) = (city, self.city.as_mut())
                    && c.city == id
                {
                    c.view_seq = view_seq;
                }
            }
            ContextMsg::Dropped { unit } => {
                self.units.remove(&unit);
            }
        }
    }

    /// A new keyframe or a new `Attached { Fresh }`: nothing stored is valid.
    pub fn clear(&mut self) {
        self.units.clear();
        self.city = None;
    }

    /// The focus changed: contexts of units and cities that left it are not kept (the server stops
    /// sending them and never says `Dropped` for a unit that is merely out of focus).
    pub fn retain_focus(&mut self, focus: &Focus) {
        self.units.retain(|id, _| focus.units.contains(id));
        if self.city.as_ref().map(|c| c.city) != focus.city {
            self.city = None;
        }
    }

    pub fn unit_count(&self) -> usize {
        self.units.len()
    }

    /// Raw access, whatever the age.
    pub fn unit(&self, id: i32) -> Option<&UnitContext> {
        self.units.get(&id)
    }

    pub fn city(&self) -> Option<&CityContext> {
        self.city.as_ref()
    }
}

static EMPTY: Contexts = Contexts::new();

/// The contexts as seen at one `view_seq`: what the controls read (they never see the store itself).
#[derive(Debug, Clone, Copy)]
pub struct ContextView<'a> {
    store: &'a Contexts,
    view_seq: u64,
}

impl<'a> ContextView<'a> {
    pub fn new(store: &'a Contexts, view_seq: u64) -> Self {
        ContextView { store, view_seq }
    }

    /// No contexts at all: every one is `Missing`.
    pub fn none() -> ContextView<'static> {
        ContextView {
            store: &EMPTY,
            view_seq: 0,
        }
    }

    pub fn view_seq(&self) -> u64 {
        self.view_seq
    }

    pub fn unit(&self, id: i32) -> Ctx<'a, UnitContext> {
        match self.store.units.get(&id) {
            Some(c) if c.view_seq == self.view_seq => Ctx::Fresh(c),
            Some(c) => Ctx::Stale(c),
            None => Ctx::Missing,
        }
    }

    /// The context of city `id`; `Missing` if the stored one is of another city.
    pub fn city(&self, id: i32) -> Ctx<'a, CityContext> {
        match self.store.city.as_ref() {
            Some(c) if c.city == id && c.view_seq == self.view_seq => Ctx::Fresh(c),
            Some(c) if c.city == id => Ctx::Stale(c),
            _ => Ctx::Missing,
        }
    }
}

/// What the screens read off a move tree. The tree is the server's; these are lookups, not rules.
pub trait UnitContextExt {
    /// The way from the unit's plot to `(x, y)`, root first: up the `parent` links of the node, then
    /// reversed. `None` if the tree does not hold the plot (ask `Query::Path`). A broken tree (a parent
    /// that does not come before its child) gives `None`, never a loop.
    fn path_to(&self, x: i16, y: i16) -> Option<Vec<PathNode>>;
    /// Plots the unit reaches this turn by a plain move: nodes with `turn == 0`, not the root, not an
    /// attack.
    fn move_cells(&self) -> Vec<(i16, i16)>;
    /// Plots where a move is a melee attack (red cells).
    fn attack_cells(&self) -> Vec<(i16, i16)>;
    /// Targets of the ranged attack.
    fn ranged_targets(&self) -> Vec<&TargetView>;
    /// The tree holds the plot.
    fn reaches(&self, x: i16, y: i16) -> bool;
}

fn path_node(n: &ReachNode) -> PathNode {
    PathNode {
        x: n.x,
        y: n.y,
        turn: i16::from(n.turn),
        flags: n.flags,
    }
}

impl UnitContextExt for UnitContext {
    fn path_to(&self, x: i16, y: i16) -> Option<Vec<PathNode>> {
        // The earliest turn is the node to take if the plot is listed more than once.
        let mut at = self
            .reach
            .iter()
            .enumerate()
            .filter(|(_, n)| (n.x, n.y) == (x, y))
            .min_by_key(|(_, n)| n.turn)?
            .0;
        let mut way = vec![path_node(&self.reach[at])];
        while at != 0 {
            let parent = usize::from(self.reach[at].parent);
            if parent >= at {
                return None;
            }
            at = parent;
            way.push(path_node(&self.reach[at]));
        }
        way.reverse();
        Some(way)
    }

    fn move_cells(&self) -> Vec<(i16, i16)> {
        self.reach
            .iter()
            .skip(1)
            .filter(|n| n.turn == 0 && !n.flags.contains(ReachFlags::ATTACK))
            .map(|n| (n.x, n.y))
            .collect()
    }

    fn attack_cells(&self) -> Vec<(i16, i16)> {
        self.reach
            .iter()
            .skip(1)
            .filter(|n| n.turn == 0 && n.flags.contains(ReachFlags::ATTACK))
            .map(|n| (n.x, n.y))
            .collect()
    }

    fn ranged_targets(&self) -> Vec<&TargetView> {
        self.targets
            .iter()
            .filter(|t| t.kind == TargetKind::Ranged)
            .collect()
    }

    fn reaches(&self, x: i16, y: i16) -> bool {
        self.reach.iter().any(|n| (n.x, n.y) == (x, y))
    }
}

/// The unit an order is given to; `None` for city, turn, diplomacy orders.
pub fn unit_of(cmd: &Command) -> Option<i32> {
    match *cmd {
        Command::UnitMoveTo { unit, .. }
        | Command::UnitRangeAttack { unit, .. }
        | Command::UnitFoundCity { unit }
        | Command::UnitFortify { unit }
        | Command::UnitSleep { unit }
        | Command::UnitSkip { unit }
        | Command::UnitHeal { unit }
        | Command::UnitAutomateBuild { unit }
        | Command::UnitAutomateExplore { unit }
        | Command::UnitCancel { unit }
        | Command::UnitDisband { unit }
        | Command::UnitWake { unit } => Some(unit),
        _ => None,
    }
}

/// What the player works with: the selected unit, the next ones that need orders, the open city (NC-D7).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FocusPlan {
    pub selected: Option<i32>,
    /// Up to [`FOCUS_NEXT`] units in the order of "next unit", never the selected one, no repeats.
    pub next: Vec<i32>,
    pub city: Option<i32>,
}

impl FocusPlan {
    pub fn new(
        selected: Option<i32>,
        next: impl IntoIterator<Item = i32>,
        city: Option<i32>,
    ) -> Self {
        let mut list: Vec<i32> = Vec::new();
        for id in next {
            if Some(id) != selected && !list.contains(&id) && list.len() < FOCUS_NEXT {
                list.push(id);
            }
        }
        FocusPlan {
            selected,
            next: list,
            city,
        }
    }

    /// The message: the selected unit first (the server treats the first as selected).
    pub fn to_focus(&self) -> Focus {
        Focus {
            units: self
                .selected
                .into_iter()
                .chain(self.next.iter().copied())
                .collect(),
            city: self.city,
        }
    }
}

/// Where the controls send orders, queries and the focus. `ClientCore` is one (B2); the local session of
/// the B1 path is the other, until it goes (N4.5).
pub trait OrderSink {
    /// An order. The B1 session has no numbers and gives `Ok(0)`.
    ///
    /// # Errors
    /// No connection, no game, too many orders in flight (NC-D5): nothing was sent.
    fn order(&mut self, cmd: Command) -> Result<CmdId, OrderRefused>;
    /// A question; the number the answer carries, or 0 when nothing was sent.
    fn query(&mut self, q: Query) -> u32;
    /// The focus the controls want now. The sink sends it when it changed.
    fn set_focus(&mut self, plan: &FocusPlan);
}

impl OrderSink for crate::ClientCore {
    fn order(&mut self, cmd: Command) -> Result<CmdId, OrderRefused> {
        crate::ClientCore::order(self, cmd)
    }

    fn query(&mut self, q: Query) -> u32 {
        crate::ClientCore::query(self, q)
    }

    fn set_focus(&mut self, plan: &FocusPlan) {
        crate::ClientCore::set_focus(self, plan);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Root (5,5); (6,5) and (5,6) from it; (7,5) from (6,5); (8,5) from (7,5) on turn 1; (9,5), an
    /// attack, from (8,5).
    fn tree() -> UnitContext {
        UnitContext {
            unit: 4,
            view_seq: 10,
            reach: vec![
                node(5, 5, 0, 0, 0),
                node(6, 5, 0, 0, 0),
                node(5, 6, 0, 0, 0),
                node(7, 5, 1, 0, ReachFlags::EMBARK),
                node(8, 5, 3, 1, 0),
                node(9, 5, 4, 1, ReachFlags::ATTACK),
                node(6, 6, 0, 0, ReachFlags::ATTACK),
            ],
            ..UnitContext::default()
        }
    }

    #[test]
    fn a_context_is_fresh_while_its_view_seq_is_the_clients() {
        let mut s = Contexts::new();
        s.apply(ContextMsg::Unit(tree()));
        assert!(ContextView::new(&s, 10).unit(4).is_fresh());
        assert!(ContextView::new(&s, 11).unit(4).is_stale());
        assert!(ContextView::new(&s, 10).unit(5).is_missing());
        assert!(ContextView::none().unit(4).is_missing());
        // A stale context is shown, but not fresh.
        let v = ContextView::new(&s, 11);
        assert!(v.unit(4).get().is_some() && v.unit(4).fresh().is_none());
        assert_eq!(v.unit(4).name(), "stale");
    }

    #[test]
    fn valid_moves_the_listed_contexts_to_the_new_view() {
        let mut s = Contexts::new();
        s.apply(ContextMsg::Unit(tree()));
        s.apply(ContextMsg::Unit(UnitContext {
            unit: 6,
            view_seq: 10,
            ..UnitContext::default()
        }));
        s.apply(ContextMsg::City(CityContext {
            city: 2,
            view_seq: 10,
            ..CityContext::default()
        }));
        s.apply(ContextMsg::Valid {
            units: vec![4, 99],
            city: Some(2),
            view_seq: 12,
        });
        let v = ContextView::new(&s, 12);
        assert!(v.unit(4).is_fresh(), "listed");
        assert!(v.unit(6).is_stale(), "not listed");
        assert!(v.city(2).is_fresh());
        // The city of another number is not this one.
        assert!(v.city(3).is_missing());
        s.apply(ContextMsg::Valid {
            units: vec![],
            city: Some(3),
            view_seq: 13,
        });
        assert!(
            ContextView::new(&s, 13).city(2).is_stale(),
            "another city's Valid changes nothing"
        );
    }

    #[test]
    fn dropped_and_clear_forget() {
        let mut s = Contexts::new();
        s.apply(ContextMsg::Unit(tree()));
        s.apply(ContextMsg::Dropped { unit: 4 });
        assert_eq!(s.unit_count(), 0);
        s.apply(ContextMsg::Unit(tree()));
        s.apply(ContextMsg::City(CityContext::default()));
        s.clear();
        assert_eq!((s.unit_count(), s.city().is_none()), (0, true));
    }

    #[test]
    fn leaving_the_focus_drops_the_context() {
        let mut s = Contexts::new();
        s.apply(ContextMsg::Unit(tree()));
        s.apply(ContextMsg::Unit(UnitContext {
            unit: 6,
            ..UnitContext::default()
        }));
        s.apply(ContextMsg::City(CityContext {
            city: 2,
            ..CityContext::default()
        }));
        s.retain_focus(&Focus {
            units: vec![6],
            city: Some(2),
        });
        assert_eq!((s.unit(4).is_none(), s.unit(6).is_some()), (true, true));
        assert!(s.city().is_some());
        s.retain_focus(&Focus {
            units: vec![],
            city: None,
        });
        assert_eq!((s.unit_count(), s.city().is_none()), (0, true));
    }

    #[test]
    fn path_to_climbs_the_parents_and_turns_them_round() {
        let t = tree();
        let way: Vec<(i16, i16, i16)> = t
            .path_to(8, 5)
            .unwrap()
            .iter()
            .map(|n| (n.x, n.y, n.turn))
            .collect();
        assert_eq!(way, [(5, 5, 0), (6, 5, 0), (7, 5, 0), (8, 5, 1)]);
        let nodes = t.path_to(7, 5).unwrap();
        assert!(
            nodes[2].flags.contains(ReachFlags::EMBARK),
            "flags are the node's"
        );
        assert_eq!(
            t.path_to(5, 5).unwrap().len(),
            1,
            "the root is a path of one"
        );
        assert_eq!(t.path_to(20, 20), None, "not in the tree: ask the server");
    }

    #[test]
    fn a_broken_tree_gives_no_path_and_no_loop() {
        let mut t = tree();
        // (8,5) names itself, then a later node, as its parent.
        t.reach[4].parent = 4;
        assert_eq!(t.path_to(8, 5), None);
        t.reach[4].parent = 6;
        assert_eq!(t.path_to(8, 5), None);
        let empty = UnitContext::default();
        assert_eq!(empty.path_to(0, 0), None);
        assert!(empty.move_cells().is_empty());
    }

    #[test]
    fn cells_come_from_the_nodes() {
        let t = tree();
        assert_eq!(t.move_cells(), [(6, 5), (5, 6), (7, 5)]);
        assert_eq!(
            t.attack_cells(),
            [(6, 6)],
            "turn 1 is not this turn's attack"
        );
        assert!(t.reaches(9, 5) && !t.reaches(1, 1));
        let ctx = UnitContext {
            targets: vec![
                TargetView {
                    x: 1,
                    y: 2,
                    kind: TargetKind::Ranged,
                    flags: 0,
                    preview: None,
                },
                TargetView {
                    x: 3,
                    y: 4,
                    kind: TargetKind::Melee,
                    flags: 0,
                    preview: None,
                },
            ],
            ..UnitContext::default()
        };
        assert_eq!(ctx.ranged_targets().len(), 1);
    }

    #[test]
    fn the_focus_is_the_selection_then_up_to_three_others() {
        let plan = FocusPlan::new(Some(4), [4, 1, 1, 7, 8, 9, 10], Some(2));
        assert_eq!(
            plan.next,
            [1, 7, 8],
            "no repeats, not the selected, three at most"
        );
        assert_eq!(
            plan.to_focus(),
            Focus {
                units: vec![4, 1, 7, 8],
                city: Some(2)
            }
        );
        let none = FocusPlan::default();
        assert_eq!(none.to_focus(), Focus::default());
        let only_next = FocusPlan::new(None, [3], None);
        assert_eq!(only_next.to_focus().units, [3]);
    }
}
