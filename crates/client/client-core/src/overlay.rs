//! The optimistic layer (net-client 4.4, NC-D6, NC-D13): what the player sees of an order before the server
//! answers. `shown` is `base` plus the records of the layer; a record is `{ cmd_id, ops }` and is made only
//! for the orders in the table of net-protocol 4.8 (one to one) and only from a context that is fresh for
//! the view. The layer is a picture, not state: `base` is never touched.
//!
//! This module is pure: it predicts, applies and keeps the records. `ClientCore` decides when (an order is
//! given, an answer comes, a batch is applied) and tells the screens.

use protocol::{
    Blocker, BlockerTarget, CityBuildFlags, CmdId, Command, PlayerId, ReachFlags, TurnPhase,
    TurnState, UnitActionView, UnitFlags, UnitKey, WorldView,
};

use crate::contexts::{ContextView, unit_of};

/// gc-abi-v1 unit command numbers (4.4) the table of 4.8 asks for in a context's `actions`.
mod action {
    pub const FORTIFY: i16 = 4;
    pub const SLEEP: i16 = 5;
    pub const SKIP: i16 = 6;
    pub const HEAL: i16 = 7;
    pub const AUTOMATE_BUILD: i16 = 8;
    pub const AUTOMATE_EXPLORE: i16 = 9;
    pub const CANCEL: i16 = 10;
    pub const WAKE: i16 = 12;
}

/// `ActivityTypes` (gamecore_abi.h): the values the table of 4.8 sets.
const ACTIVITY_AWAKE: i8 = 0;
const ACTIVITY_HEAL: i8 = 3;

/// `EndTurnBlockingTypes` of the blockers the layer can lift (4.4, NC-D13): a unit waits for orders (3 and
/// 8), a city has to choose production (2).
const BLOCKING_PRODUCTION: i32 = 2;
const BLOCKING_UNITS: [i32; 2] = [3, 8];

/// One change of the picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Op {
    Unit {
        unit: i32,
        to: Option<(i16, i16)>,
        moves_left: Option<i16>,
        set: u16,
        clear: u16,
        activity: Option<i8>,
    },
    City {
        city: i32,
        order: i8,
        item: i16,
        turns: i16,
        needed: i32,
    },
    /// The `finished` of our own seat in `TurnState`.
    Finished(bool),
}

/// An order's picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Record {
    pub id: CmdId,
    pub ops: Vec<Op>,
    /// The final answer came while a keyframe was awaited: the record stays until the keyframe.
    answered: bool,
    /// The time the guard counts from: when the record was made, and again for every moment the line was
    /// down (`order_stuck_ms` does not run during an outage, 4.5).
    armed_ms: u64,
}

/// The records that were touched, as keys of the view (for `ViewChange`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Touched {
    pub units: Vec<UnitKey>,
    pub cities: Vec<UnitKey>,
    /// A record was added or removed (even one with nothing but the end of turn in it).
    pub changed: bool,
}

#[derive(Debug, Default)]
pub(crate) struct Overlay {
    records: Vec<Record>,
    /// Grows with every change of the records; part of `ClientCore::revision`.
    rev: u64,
    shown: Option<WorldView>,
    turn: Option<TurnState>,
}

impl Overlay {
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn rev(&self) -> u64 {
        self.rev
    }

    pub fn shown(&self) -> Option<&WorldView> {
        self.shown.as_ref()
    }

    pub fn turn_state(&self) -> Option<&TurnState> {
        self.turn.as_ref()
    }

    /// An order in flight already has a picture on this unit: a second one gets none (4.4: the context it
    /// would use is the one the first picture made out of date).
    pub fn has_unit(&self, unit: i32) -> bool {
        self.records
            .iter()
            .flat_map(|r| &r.ops)
            .any(|op| matches!(op, Op::Unit { unit: u, .. } if *u == unit))
    }

    pub fn has_city(&self, city: i32) -> bool {
        self.records
            .iter()
            .flat_map(|r| &r.ops)
            .any(|op| matches!(op, Op::City { city: c, .. } if *c == city))
    }

    pub fn add(&mut self, id: CmdId, ops: Vec<Op>, now_ms: u64, me: PlayerId) -> Touched {
        let touched = touched_by(&ops, me);
        self.records.push(Record {
            id,
            ops,
            answered: false,
            armed_ms: now_ms,
        });
        self.rev += 1;
        touched
    }

    /// The final answer: the record goes (for good).
    pub fn remove(&mut self, id: CmdId, me: PlayerId) -> Touched {
        match self.records.iter().position(|r| r.id == id) {
            Some(at) => {
                let r = self.records.remove(at);
                self.rev += 1;
                touched_by(&r.ops, me)
            }
            None => Touched::default(),
        }
    }

    /// The answer came but a keyframe is awaited: the record stays until it comes.
    pub fn mark_answered(&mut self, id: CmdId) {
        if let Some(r) = self.records.iter_mut().find(|r| r.id == id) {
            r.answered = true;
        }
    }

    /// The keyframe is in: the records whose answer was held go.
    pub fn remove_answered(&mut self, me: PlayerId) -> Touched {
        let mut touched = Touched::default();
        let mut kept = Vec::with_capacity(self.records.len());
        for r in self.records.drain(..) {
            if r.answered {
                let t = touched_by(&r.ops, me);
                touched.units.extend(t.units);
                touched.cities.extend(t.cities);
                touched.changed = true;
            } else {
                kept.push(r);
            }
        }
        if touched.changed {
            self.rev += 1;
        }
        self.records = kept;
        touched
    }

    /// The game ended or changed: no picture means anything.
    pub fn clear(&mut self) {
        if !self.records.is_empty() {
            self.rev += 1;
        }
        self.records.clear();
        self.shown = None;
        self.turn = None;
    }

    /// Records older than `stuck_ms` of live line, not held for a keyframe (4.4 guard). While the line is
    /// down (`line_up == false`) the time does not count. Returns their numbers.
    pub fn stuck(&mut self, now_ms: u64, line_up: bool, stuck_ms: u64) -> Vec<CmdId> {
        let mut ids = Vec::new();
        for r in &mut self.records {
            if !line_up {
                r.armed_ms = now_ms;
            } else if !r.answered && now_ms.saturating_sub(r.armed_ms) >= stuck_ms {
                ids.push(r.id);
            }
        }
        ids
    }

    /// `shown` and the turn state with the layer on them. Without records there is no copy: the caller
    /// shows `base`.
    pub fn rebuild(&mut self, base: Option<&WorldView>, turn: Option<&TurnState>, me: PlayerId) {
        self.shown = None;
        self.turn = None;
        if self.records.is_empty() {
            return;
        }
        if let Some(base) = base {
            let mut view = base.clone();
            for op in self.records.iter().flat_map(|r| &r.ops) {
                apply_view(&mut view, op, me);
            }
            lift_blocker(&mut view);
            self.shown = Some(view);
        }
        if let Some(turn) = turn {
            let mut ts = turn.clone();
            for op in self.records.iter().flat_map(|r| &r.ops) {
                if let Op::Finished(f) = op
                    && let Some(seat) = ts.seats.iter_mut().find(|s| s.player == me)
                {
                    seat.finished = *f;
                }
            }
            self.turn = Some(ts);
        }
    }
}

fn touched_by(ops: &[Op], me: PlayerId) -> Touched {
    let mut t = Touched {
        changed: true,
        ..Touched::default()
    };
    for op in ops {
        match op {
            Op::Unit { unit, .. } => t.units.push((me, *unit)),
            Op::City { city, .. } => t.cities.push((me, *city)),
            Op::Finished(_) => {}
        }
    }
    t
}

fn apply_view(view: &mut WorldView, op: &Op, me: PlayerId) {
    match op {
        Op::Unit {
            unit,
            to,
            moves_left,
            set,
            clear,
            activity,
        } => {
            let Some(u) = view
                .units
                .iter_mut()
                .find(|u| u.id == *unit && u.owner == me)
            else {
                return;
            };
            if let Some((x, y)) = *to {
                u.x = x;
                u.y = y;
            }
            if let Some(m) = *moves_left {
                u.moves_left = m;
            }
            if let Some(a) = *activity {
                u.activity = a;
            }
            u.flags = UnitFlags((u.flags.0 | *set) & !*clear);
        }
        Op::City {
            city,
            order,
            item,
            turns,
            needed,
        } => {
            if let Some(c) = view
                .cities
                .iter_mut()
                .find(|c| c.id == *city && c.owner == me)
            {
                c.production_order = *order;
                c.production_item = *item;
                c.production_turns = *turns;
                c.production_needed = *needed;
            }
        }
        Op::Finished(_) => {}
    }
}

/// NC-D13: a blocker whose condition the layer already lifted is not shown. The end-turn button then says
/// "Next turn" and sends `EndTurn { force: false }`; the server decides, and a refusal brings the next blocker.
fn lift_blocker(view: &mut WorldView) {
    let Some(Blocker { kind, target }) = view.blocker else {
        return;
    };
    let lifted = match target {
        BlockerTarget::Unit(id) if BLOCKING_UNITS.contains(&kind) => view
            .units
            .iter()
            .find(|u| u.id == id && u.owner == view.human)
            .is_some_and(|u| !u.flags.contains(UnitFlags::NEEDS_ORDERS)),
        BlockerTarget::City(id) if kind == BLOCKING_PRODUCTION => view
            .cities
            .iter()
            .find(|c| c.id == id && c.owner == view.human)
            .is_some_and(|c| c.production_item >= 0),
        _ => false,
    };
    if lifted {
        view.blocker = None;
    }
}

// ---------------------------------------------------------------- prediction

/// What the client may draw for `cmd` (the table of net-protocol 4.8), or `None`. Needs: the human phase,
/// our seat may act, a context that is fresh for the view, and no earlier picture on the same unit or city.
/// `view` is what the player sees now (`shown`); `finished` is our own seat's flag now.
pub(crate) fn predict(
    cmd: &Command,
    view: &WorldView,
    turn: Option<&TurnState>,
    cx: &ContextView<'_>,
    me: PlayerId,
    overlay: &Overlay,
) -> Option<Vec<Op>> {
    let finished = turn
        .and_then(|t| t.seats.iter().find(|s| s.player == me))
        .map(|s| s.finished);
    // The end of the turn is drawn whenever the order is accepted for sending (NC-D5 refused the rest).
    match cmd {
        Command::EndTurn { .. } => return Some(vec![Op::Finished(true)]),
        Command::CancelEndTurn => return Some(vec![Op::Finished(false)]),
        _ => {}
    }
    let turn = turn?;
    let seat = turn.seats.iter().find(|s| s.player == me)?;
    if turn.phase != TurnPhase::Humans || !seat.may_act {
        return None;
    }
    let mut ops = match *cmd {
        Command::CitySetProduction { city, order, item } => {
            predict_production(view, cx, me, overlay, city, order, item)?
        }
        _ => predict_unit(cmd, view, cx, me, overlay)?,
    };
    // NP-D23: an order after our own end of turn takes the end of turn back.
    if finished == Some(true) {
        ops.push(Op::Finished(false));
    }
    Some(ops)
}

fn predict_production(
    view: &WorldView,
    cx: &ContextView<'_>,
    me: PlayerId,
    overlay: &Overlay,
    city: i32,
    order: i8,
    item: i16,
) -> Option<Vec<Op>> {
    view.cities.iter().find(|c| c.id == city && c.owner == me)?;
    if overlay.has_city(city) {
        return None;
    }
    let ctx = cx.city(city).fresh()?;
    let row = ctx
        .builds
        .iter()
        .find(|b| b.order_type == order && b.item == item)?;
    if !row.flags.contains(CityBuildFlags::ENABLED) {
        return None;
    }
    Some(vec![Op::City {
        city,
        order,
        item,
        turns: row.turns,
        needed: row.production_cost,
    }])
}

fn has_action(actions: &[UnitActionView], number: i16) -> bool {
    actions.iter().any(|a| a.action == number && a.enabled)
}

fn unit_op(unit: i32, set: u16, clear: u16, activity: Option<i8>) -> Vec<Op> {
    vec![Op::Unit {
        unit,
        to: None,
        moves_left: None,
        set,
        clear,
        activity,
    }]
}

fn predict_unit(
    cmd: &Command,
    view: &WorldView,
    cx: &ContextView<'_>,
    me: PlayerId,
    overlay: &Overlay,
) -> Option<Vec<Op>> {
    let unit = unit_of(cmd)?;
    view.units.iter().find(|u| u.id == unit && u.owner == me)?;
    if overlay.has_unit(unit) {
        return None;
    }
    let ctx = cx.unit(unit).fresh()?;
    let needs = UnitFlags::NEEDS_ORDERS;
    let asleep = UnitFlags::SLEEPING | UnitFlags::FORTIFIED | UnitFlags::AUTOMATED;
    match *cmd {
        Command::UnitMoveTo { x, y, .. } => {
            // A node of the tree reached this turn, by a plain move: not the unit's own plot, not an
            // attack, not into the unknown, not a capture.
            let barred = ReachFlags::ATTACK | ReachFlags::UNREVEALED | ReachFlags::CAPTURE;
            let node = ctx
                .reach
                .iter()
                .skip(1)
                .find(|n| (n.x, n.y) == (x, y) && n.turn == 0 && n.flags.0 & barred == 0)?;
            let mut set = 0;
            let mut clear = 0;
            if node.flags.contains(ReachFlags::EMBARK) {
                set |= UnitFlags::EMBARKED;
            }
            if node.flags.contains(ReachFlags::DISEMBARK) {
                clear |= UnitFlags::EMBARKED;
            }
            if node.moves_left > 0 {
                set |= UnitFlags::CAN_MOVE;
            } else {
                clear |= UnitFlags::CAN_MOVE | needs;
            }
            Some(vec![Op::Unit {
                unit,
                to: Some((node.x, node.y)),
                moves_left: Some(node.moves_left),
                set,
                clear,
                activity: None,
            }])
        }
        Command::UnitSleep { .. } if has_action(&ctx.actions, action::SLEEP) => {
            Some(unit_op(unit, UnitFlags::SLEEPING, needs, None))
        }
        Command::UnitFortify { .. } if has_action(&ctx.actions, action::FORTIFY) => {
            Some(unit_op(unit, UnitFlags::FORTIFIED, needs, None))
        }
        Command::UnitAutomateBuild { .. } if has_action(&ctx.actions, action::AUTOMATE_BUILD) => {
            Some(unit_op(unit, UnitFlags::AUTOMATED, needs, None))
        }
        Command::UnitAutomateExplore { .. }
            if has_action(&ctx.actions, action::AUTOMATE_EXPLORE) =>
        {
            Some(unit_op(unit, UnitFlags::AUTOMATED, needs, None))
        }
        Command::UnitSkip { .. } if has_action(&ctx.actions, action::SKIP) => {
            Some(unit_op(unit, 0, needs, None))
        }
        Command::UnitHeal { .. } if has_action(&ctx.actions, action::HEAL) => {
            Some(unit_op(unit, 0, needs, Some(ACTIVITY_HEAL)))
        }
        Command::UnitWake { .. } if has_action(&ctx.actions, action::WAKE) => {
            Some(unit_op(unit, needs, asleep, Some(ACTIVITY_AWAKE)))
        }
        Command::UnitCancel { .. } if has_action(&ctx.actions, action::CANCEL) => {
            Some(unit_op(unit, needs, asleep, Some(ACTIVITY_AWAKE)))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use protocol::{
        CityBuildView, CityContext, CityView, ContextMsg, PlayerView, ReachNode, SeatTurn,
        TimerState, TurnMode, UnitContext, UnitView,
    };

    use super::*;
    use crate::contexts::Contexts;

    const ME: PlayerId = 1;

    fn world() -> WorldView {
        WorldView {
            turn: 5,
            year: 0,
            human: ME,
            human_team: 0,
            status: protocol::TurnStatus::HumanTurn,
            blocker: None,
            map: protocol::MapView::default(),
            top_bar: protocol::TopBar::default(),
            notifications: vec![],
            peace_offers: vec![],
            trade_routes: vec![],
            units: vec![
                UnitView {
                    id: 4,
                    owner: ME,
                    x: 5,
                    y: 5,
                    moves_left: 120,
                    max_moves: 120,
                    flags: UnitFlags(UnitFlags::CAN_MOVE | UnitFlags::NEEDS_ORDERS),
                    ..UnitView::default()
                },
                UnitView {
                    id: 6,
                    owner: ME,
                    x: 7,
                    y: 7,
                    moves_left: 120,
                    flags: UnitFlags(UnitFlags::CAN_MOVE | UnitFlags::NEEDS_ORDERS),
                    ..UnitView::default()
                },
                // Another civilization's unit with the same id as ours.
                UnitView {
                    id: 4,
                    owner: 2,
                    x: 1,
                    y: 1,
                    ..UnitView::default()
                },
            ],
            cities: vec![CityView {
                id: 2,
                owner: ME,
                production_item: -1,
                production_order: -1,
                ..CityView::default()
            }],
            players: vec![PlayerView {
                id: ME,
                ..PlayerView::default()
            }],
        }
    }

    fn turn(finished: bool, may_act: bool, phase: TurnPhase) -> TurnState {
        TurnState {
            turn: 5,
            phase,
            mode: TurnMode::Simultaneous,
            seats: vec![
                SeatTurn {
                    player: 2,
                    finished: true,
                    may_act: true,
                    group: 0,
                },
                SeatTurn {
                    player: ME,
                    finished,
                    may_act,
                    group: 0,
                },
            ],
            timers: Vec::<TimerState>::new(),
            paused: false,
        }
    }

    fn node(x: i16, y: i16, parent: u16, turn: u8, moves_left: i16, flags: u16) -> ReachNode {
        ReachNode {
            x,
            y,
            parent,
            turn,
            moves_left,
            flags: ReachFlags(flags),
        }
    }

    fn action(action: i16, enabled: bool) -> UnitActionView {
        UnitActionView {
            action,
            enabled,
            reason: String::new(),
        }
    }

    fn unit_ctx() -> UnitContext {
        UnitContext {
            unit: 4,
            view_seq: 9,
            actions: (4..=12).map(|a| action(a, a != 7)).collect(),
            reach: vec![
                node(5, 5, 0, 0, 120, 0),
                node(6, 5, 0, 0, 60, 0),
                node(7, 5, 1, 0, 0, 0),
                node(8, 5, 2, 1, 0, 0),
                node(5, 6, 0, 0, 60, ReachFlags::ATTACK),
                node(4, 5, 0, 0, 60, ReachFlags::UNREVEALED),
                node(4, 4, 0, 0, 60, ReachFlags::CAPTURE),
                node(6, 6, 0, 0, 60, ReachFlags::EMBARK),
                node(6, 4, 0, 0, 60, ReachFlags::DISEMBARK),
            ],
            ..UnitContext::default()
        }
    }

    fn city_ctx() -> CityContext {
        let row = |item: i16, flags: u16| CityBuildView {
            order_type: 0,
            item,
            turns: 7,
            production_cost: 80,
            gold_cost: -1,
            flags: CityBuildFlags(flags),
            faith_cost: -1,
        };
        CityContext {
            city: 2,
            view_seq: 9,
            builds: vec![row(1, CityBuildFlags::ENABLED), row(2, 0)],
            targets: vec![],
        }
    }

    fn store() -> Contexts {
        let mut s = Contexts::new();
        s.apply(ContextMsg::Unit(unit_ctx()));
        s.apply(ContextMsg::City(city_ctx()));
        s
    }

    fn go(cmd: Command) -> Option<Vec<Op>> {
        go_in(
            cmd,
            &world(),
            &turn(false, true, TurnPhase::Humans),
            9,
            &Overlay::default(),
        )
    }

    fn go_in(
        cmd: Command,
        view: &WorldView,
        ts: &TurnState,
        view_seq: u64,
        overlay: &Overlay,
    ) -> Option<Vec<Op>> {
        let s = store();
        predict(
            &cmd,
            view,
            Some(ts),
            &ContextView::new(&s, view_seq),
            ME,
            overlay,
        )
    }

    /// Applies the ops to the world.
    fn shown_after(ops: Vec<Op>) -> WorldView {
        let mut o = Overlay::default();
        o.add(1, ops, 0, ME);
        o.rebuild(
            Some(&world()),
            Some(&turn(false, true, TurnPhase::Humans)),
            ME,
        );
        o.shown().unwrap().clone()
    }

    fn unit4(v: &WorldView) -> &UnitView {
        v.units.iter().find(|u| u.id == 4 && u.owner == ME).unwrap()
    }

    #[test]
    fn a_move_to_a_node_of_this_turn_puts_the_unit_there() {
        let v = shown_after(
            go(Command::UnitMoveTo {
                unit: 4,
                x: 6,
                y: 5,
            })
            .unwrap(),
        );
        let u = unit4(&v);
        assert_eq!((u.x, u.y, u.moves_left), (6, 5, 60));
        assert!(u.flags.contains(UnitFlags::CAN_MOVE) && u.flags.contains(UnitFlags::NEEDS_ORDERS));
        // Out of moves: cannot move and does not need orders.
        let v = shown_after(
            go(Command::UnitMoveTo {
                unit: 4,
                x: 7,
                y: 5,
            })
            .unwrap(),
        );
        let u = unit4(&v);
        assert_eq!((u.x, u.y, u.moves_left), (7, 5, 0));
        assert!(
            !u.flags.contains(UnitFlags::CAN_MOVE) && !u.flags.contains(UnitFlags::NEEDS_ORDERS)
        );
        // The other civilization's unit with the same number is untouched.
        let other = v.units.iter().find(|u| u.owner == 2).unwrap();
        assert_eq!((other.x, other.y), (1, 1));
    }

    #[test]
    fn embarking_and_disembarking_set_and_clear_the_flag() {
        let v = shown_after(
            go(Command::UnitMoveTo {
                unit: 4,
                x: 6,
                y: 6,
            })
            .unwrap(),
        );
        assert!(unit4(&v).flags.contains(UnitFlags::EMBARKED));
        let mut start = world();
        start.units[0].flags = UnitFlags(start.units[0].flags.0 | UnitFlags::EMBARKED);
        let ops = go_in(
            Command::UnitMoveTo {
                unit: 4,
                x: 6,
                y: 4,
            },
            &start,
            &turn(false, true, TurnPhase::Humans),
            9,
            &Overlay::default(),
        )
        .unwrap();
        let mut o = Overlay::default();
        o.add(1, ops, 0, ME);
        o.rebuild(Some(&start), None, ME);
        assert!(
            !unit4(o.shown().unwrap())
                .flags
                .contains(UnitFlags::EMBARKED)
        );
    }

    #[test]
    fn what_the_table_does_not_predict_gets_no_picture() {
        // The unit's own plot, a turn-1 node, an attack, the unknown, a capture, a plot off the tree.
        for (x, y) in [(5, 5), (8, 5), (5, 6), (4, 5), (4, 4), (30, 30)] {
            assert_eq!(go(Command::UnitMoveTo { unit: 4, x, y }), None, "{x},{y}");
        }
        for cmd in [
            Command::UnitRangeAttack {
                unit: 4,
                x: 6,
                y: 5,
            },
            Command::UnitFoundCity { unit: 4 },
            Command::UnitDisband { unit: 4 },
            Command::DeclareWar { team: 1 },
            Command::CityPurchase {
                city: 2,
                order: 0,
                item: 1,
                yield_: 2,
            },
            Command::CityRangeAttack {
                city: 2,
                x: 1,
                y: 1,
            },
            Command::ProposePeace { to: 2 },
        ] {
            assert_eq!(go(cmd), None, "{cmd:?}");
        }
    }

    #[test]
    fn plain_unit_commands_set_their_flags() {
        let flags = |cmd| unit4(&shown_after(go(cmd).unwrap())).flags;
        let f = flags(Command::UnitSleep { unit: 4 });
        assert!(f.contains(UnitFlags::SLEEPING) && !f.contains(UnitFlags::NEEDS_ORDERS));
        let f = flags(Command::UnitFortify { unit: 4 });
        assert!(f.contains(UnitFlags::FORTIFIED) && !f.contains(UnitFlags::NEEDS_ORDERS));
        for cmd in [
            Command::UnitAutomateBuild { unit: 4 },
            Command::UnitAutomateExplore { unit: 4 },
        ] {
            let f = flags(cmd);
            assert!(f.contains(UnitFlags::AUTOMATED) && !f.contains(UnitFlags::NEEDS_ORDERS));
        }
        // Skip: only the need for orders goes.
        let f = flags(Command::UnitSkip { unit: 4 });
        assert!(!f.contains(UnitFlags::NEEDS_ORDERS));
        assert_eq!(f.0 & (UnitFlags::SLEEPING | UnitFlags::FORTIFIED), 0);
    }

    #[test]
    fn heal_sets_the_activity_but_not_the_sleep_or_fortify_flags() {
        // The context lists Heal as disabled: no picture.
        assert_eq!(go(Command::UnitHeal { unit: 4 }), None);
        let mut s = Contexts::new();
        let mut c = unit_ctx();
        c.actions.iter_mut().for_each(|a| a.enabled = true);
        s.apply(ContextMsg::Unit(c));
        let ops = predict(
            &Command::UnitHeal { unit: 4 },
            &world(),
            Some(&turn(false, true, TurnPhase::Humans)),
            &ContextView::new(&s, 9),
            ME,
            &Overlay::default(),
        )
        .unwrap();
        let v = shown_after(ops);
        let u = unit4(&v);
        assert_eq!(u.activity, ACTIVITY_HEAL);
        assert_eq!(u.flags.0 & (UnitFlags::SLEEPING | UnitFlags::FORTIFIED), 0);
        assert!(!u.flags.contains(UnitFlags::NEEDS_ORDERS));
    }

    #[test]
    fn wake_and_cancel_clear_the_orders_and_ask_for_new_ones() {
        let mut start = world();
        start.units[0].flags = UnitFlags(
            UnitFlags::SLEEPING | UnitFlags::FORTIFIED | UnitFlags::AUTOMATED | UnitFlags::CAN_MOVE,
        );
        start.units[0].activity = 2;
        for cmd in [
            Command::UnitWake { unit: 4 },
            Command::UnitCancel { unit: 4 },
        ] {
            let ops = go_in(
                cmd,
                &start,
                &turn(false, true, TurnPhase::Humans),
                9,
                &Overlay::default(),
            )
            .unwrap();
            let mut o = Overlay::default();
            o.add(1, ops, 0, ME);
            o.rebuild(Some(&start), None, ME);
            let u = unit4(o.shown().unwrap());
            assert_eq!(u.activity, ACTIVITY_AWAKE);
            assert_eq!(
                u.flags.0 & (UnitFlags::SLEEPING | UnitFlags::FORTIFIED | UnitFlags::AUTOMATED),
                0
            );
            assert!(u.flags.contains(UnitFlags::NEEDS_ORDERS));
        }
    }

    #[test]
    fn a_disabled_or_missing_action_gets_no_picture() {
        let mut s = Contexts::new();
        let mut c = unit_ctx();
        c.actions = vec![action(action::SLEEP, false)];
        s.apply(ContextMsg::Unit(c));
        let cx = ContextView::new(&s, 9);
        let t = turn(false, true, TurnPhase::Humans);
        let w = world();
        let none = Overlay::default();
        assert_eq!(
            predict(
                &Command::UnitSleep { unit: 4 },
                &w,
                Some(&t),
                &cx,
                ME,
                &none
            ),
            None
        );
        assert_eq!(
            predict(
                &Command::UnitFortify { unit: 4 },
                &w,
                Some(&t),
                &cx,
                ME,
                &none
            ),
            None
        );
    }

    #[test]
    fn a_city_gets_its_production_from_an_enabled_row() {
        let v = shown_after(
            go(Command::CitySetProduction {
                city: 2,
                order: 0,
                item: 1,
            })
            .unwrap(),
        );
        let c = &v.cities[0];
        assert_eq!(
            (
                c.production_order,
                c.production_item,
                c.production_turns,
                c.production_needed
            ),
            (0, 1, 7, 80)
        );
        // A row that is not enabled, a row that is not there, a city that is not ours.
        assert_eq!(
            go(Command::CitySetProduction {
                city: 2,
                order: 0,
                item: 2
            }),
            None
        );
        assert_eq!(
            go(Command::CitySetProduction {
                city: 2,
                order: 0,
                item: 9
            }),
            None
        );
        assert_eq!(
            go(Command::CitySetProduction {
                city: 3,
                order: 0,
                item: 1
            }),
            None
        );
    }

    #[test]
    fn no_picture_without_a_fresh_context_or_outside_our_turn() {
        let mv = Command::UnitMoveTo {
            unit: 4,
            x: 6,
            y: 5,
        };
        let w = world();
        // Stale (the client's view moved on), and another unit's context.
        assert_eq!(
            go_in(
                mv,
                &w,
                &turn(false, true, TurnPhase::Humans),
                10,
                &Overlay::default()
            ),
            None
        );
        assert_eq!(
            go(Command::UnitMoveTo {
                unit: 6,
                x: 8,
                y: 7
            }),
            None,
            "no context for unit 6"
        );
        // The rivals move, the game is over, the hybrid seat may not act yet.
        for t in [
            turn(false, true, TurnPhase::Ai { active: 2 }),
            turn(false, true, TurnPhase::GameOver { winner: None }),
            turn(false, false, TurnPhase::Humans),
        ] {
            assert_eq!(go_in(mv, &w, &t, 9, &Overlay::default()), None);
        }
        // Without a turn state nothing is known about the phase.
        let s = store();
        assert_eq!(
            predict(
                &mv,
                &w,
                None,
                &ContextView::new(&s, 9),
                ME,
                &Overlay::default()
            ),
            None
        );
        // The end of the turn is drawn regardless.
        assert_eq!(
            predict(
                &Command::EndTurn { force: false },
                &w,
                None,
                &ContextView::none(),
                ME,
                &Overlay::default()
            ),
            Some(vec![Op::Finished(true)])
        );
    }

    #[test]
    fn a_second_order_to_the_same_unit_or_city_gets_no_picture() {
        let mut o = Overlay::default();
        o.add(1, go(Command::UnitSleep { unit: 4 }).unwrap(), 0, ME);
        let t = turn(false, true, TurnPhase::Humans);
        let w = world();
        assert_eq!(
            go_in(
                Command::UnitMoveTo {
                    unit: 4,
                    x: 6,
                    y: 5
                },
                &w,
                &t,
                9,
                &o
            ),
            None
        );
        o.add(
            2,
            go(Command::CitySetProduction {
                city: 2,
                order: 0,
                item: 1,
            })
            .unwrap(),
            0,
            ME,
        );
        assert_eq!(
            go_in(
                Command::CitySetProduction {
                    city: 2,
                    order: 0,
                    item: 1
                },
                &w,
                &t,
                9,
                &o
            ),
            None
        );
        // Another unit is a different matter.
        assert!(o.has_unit(4) && !o.has_unit(6));
        // The record gone, the unit is free again.
        o.remove(1, ME);
        assert!(!o.has_unit(4));
    }

    #[test]
    fn end_turn_and_cancel_flip_our_own_flag_only() {
        let mut o = Overlay::default();
        o.add(1, vec![Op::Finished(true)], 0, ME);
        o.rebuild(
            Some(&world()),
            Some(&turn(false, true, TurnPhase::Humans)),
            ME,
        );
        let ts = o.turn_state().unwrap();
        let by = |p| ts.seats.iter().find(|s| s.player == p).unwrap().finished;
        assert!(by(ME) && by(2));
        o.add(2, vec![Op::Finished(false)], 0, ME);
        o.rebuild(
            Some(&world()),
            Some(&turn(true, true, TurnPhase::Humans)),
            ME,
        );
        let ts = o.turn_state().unwrap();
        assert!(
            !ts.seats.iter().find(|s| s.player == ME).unwrap().finished,
            "the last record wins"
        );
    }

    #[test]
    fn an_order_after_our_end_of_turn_takes_it_back() {
        let t = turn(true, true, TurnPhase::Humans);
        let w = world();
        let ops = go_in(
            Command::UnitSleep { unit: 4 },
            &w,
            &t,
            9,
            &Overlay::default(),
        )
        .unwrap();
        assert_eq!(ops.last(), Some(&Op::Finished(false)));
        let mut o = Overlay::default();
        o.add(1, ops, 0, ME);
        o.rebuild(Some(&w), Some(&t), ME);
        let seat = |ts: &TurnState| ts.seats.iter().find(|s| s.player == ME).unwrap().finished;
        assert!(!seat(o.turn_state().unwrap()));
        // An order that is not predicted leaves the flag alone.
        assert_eq!(
            go_in(
                Command::UnitFoundCity { unit: 4 },
                &w,
                &t,
                9,
                &Overlay::default()
            ),
            None
        );
    }

    #[test]
    fn a_blocker_the_layer_lifted_is_not_shown() {
        let mut w = world();
        w.blocker = Some(Blocker {
            kind: 3,
            target: BlockerTarget::Unit(4),
        });
        let t = turn(false, true, TurnPhase::Humans);
        let mut o = Overlay::default();
        // A record on another unit: the blocker's condition (unit 4 needs orders) still holds.
        o.add(1, unit_op(6, 0, UnitFlags::NEEDS_ORDERS, None), 0, ME);
        o.rebuild(Some(&w), Some(&t), ME);
        assert_eq!(o.shown().unwrap().blocker, w.blocker);
        // The record on unit 4 lifts it.
        o.add(
            2,
            unit_op(4, UnitFlags::SLEEPING, UnitFlags::NEEDS_ORDERS, None),
            0,
            ME,
        );
        o.rebuild(Some(&w), Some(&t), ME);
        assert_eq!(o.shown().unwrap().blocker, None);
        assert_eq!(w.blocker.unwrap().kind, 3, "base is never touched");
        // The kinds: 8 is a unit too; production lifts when the city has an item.
        w.blocker = Some(Blocker {
            kind: 8,
            target: BlockerTarget::Unit(4),
        });
        o.rebuild(Some(&w), Some(&t), ME);
        assert_eq!(o.shown().unwrap().blocker, None);
        w.blocker = Some(Blocker {
            kind: 2,
            target: BlockerTarget::City(2),
        });
        o.rebuild(Some(&w), Some(&t), ME);
        assert!(
            o.shown().unwrap().blocker.is_some(),
            "the city still has nothing to build"
        );
        o.add(
            3,
            vec![Op::City {
                city: 2,
                order: 0,
                item: 1,
                turns: 7,
                needed: 80,
            }],
            0,
            ME,
        );
        o.rebuild(Some(&w), Some(&t), ME);
        assert_eq!(o.shown().unwrap().blocker, None);
        // A blocker of another kind stays, whatever the layer did.
        w.blocker = Some(Blocker {
            kind: 0,
            target: BlockerTarget::None,
        });
        o.rebuild(Some(&w), Some(&t), ME);
        assert!(o.shown().unwrap().blocker.is_some());
    }

    #[test]
    fn without_records_there_is_no_copy() {
        let mut o = Overlay::default();
        o.rebuild(Some(&world()), None, ME);
        assert!(o.shown().is_none() && o.turn_state().is_none());
        assert!(o.is_empty());
    }

    #[test]
    fn answered_records_wait_for_the_keyframe() {
        let mut o = Overlay::default();
        o.add(1, unit_op(4, UnitFlags::SLEEPING, 0, None), 0, ME);
        o.add(2, unit_op(6, UnitFlags::FORTIFIED, 0, None), 0, ME);
        o.mark_answered(1);
        assert!(o.remove_answered(ME).units.contains(&(ME, 4)));
        assert_eq!(o.len(), 1);
        assert!(!o.remove_answered(ME).changed, "the other was not answered");
        let t = o.remove(2, ME);
        assert_eq!(t.units, [(ME, 6)]);
        assert!(o.is_empty());
    }

    #[test]
    fn the_guard_counts_live_line_time_only() {
        let mut o = Overlay::default();
        o.add(1, unit_op(4, UnitFlags::SLEEPING, 0, None), 1_000, ME);
        assert!(o.stuck(15_999, true, 15_000).is_empty());
        assert_eq!(o.stuck(16_000, true, 15_000), [1]);
        // The line was down for a long while: the clock starts again when it is back.
        let mut o = Overlay::default();
        o.add(1, unit_op(4, UnitFlags::SLEEPING, 0, None), 1_000, ME);
        assert!(o.stuck(40_000, false, 15_000).is_empty());
        assert!(o.stuck(41_000, true, 15_000).is_empty());
        assert_eq!(o.stuck(55_000, true, 15_000), [1]);
        // An answered record (held for a keyframe) is not stuck.
        o.mark_answered(1);
        assert!(o.stuck(99_000, true, 15_000).is_empty());
    }

    #[test]
    fn the_revision_grows_with_every_change() {
        let mut o = Overlay::default();
        let r0 = o.rev();
        o.add(1, vec![Op::Finished(true)], 0, ME);
        let r1 = o.rev();
        o.remove(1, ME);
        let r2 = o.rev();
        o.remove(1, ME);
        assert!(r1 > r0 && r2 > r1);
        assert_eq!(o.rev(), r2, "removing nothing changes nothing");
        o.add(2, vec![], 0, ME);
        o.clear();
        assert!(o.rev() > r2 && o.is_empty());
    }
}
