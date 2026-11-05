//! `PlayController`: selection, orders and hotkeys as a state machine without GPU or egui
//! Input and the latest `WorldView` in; `Command`s, `Query`s and UI
//! effects out. It never decides what the rules allow: the brain answers (or rejects) every order.

use client_core::{ContextView, FOCUS_NEXT, FocusPlan, UnitContextExt, unit_of};
use platform::KeyEvent;
use protocol::{
    Blocker, BlockerTarget, CityContext, CityFlags, CmdId, Command, PathNode, PlayerFlags,
    PlayerId, Query, QueryAnswer, ReachFlags, Shooter, TeamId, TurnStatus, UnitContext, UnitFlags,
    WorldView,
};

use super::hotkeys::{self, HotKey};
use super::selection::{self, Pick};
use super::{Outputs, UiEffect};

/// `EndTurnBlockingTypes::ENDTURN_BLOCKING_CITY_RANGE_ATTACK` (CvEnums.h).
const BLOCKING_CITY_RANGE_ATTACK: i32 = 10;

/// Input mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Normal,
    /// "Ranged attack": the next left click on a plot is the target.
    RangeAttack(Shooter),
}

/// A move waiting for the answer of "Declare war?".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingWar {
    team: TeamId,
    /// What to do once war is declared.
    order: WarOrder,
}

/// The order that waits for the declaration of war: a move onto the plot, or a ranged attack on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WarOrder {
    Move { unit: i32, x: i16, y: i16 },
    Range { shooter: Shooter, x: i16, y: i16 },
}

/// The mouse must rest this long on a plot the move tree does not hold before `Query::Path` is asked
/// (net-client 4.6).
pub const PATH_DWELL_MS: u64 = 150;

/// The answer to the one `Query::Path` the controller asks, for the renderer's highlights. Everything
/// else the highlights show (reach, red cells, targets, a path inside the tree) is read off the contexts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hints {
    pub path: Vec<PathNode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    Free,
    /// A barbarian or a civilization at war: moving there attacks.
    Enemy,
    /// A civilization in peace: moving there needs a declaration of war.
    Peace {
        team: TeamId,
    },
}

pub struct PlayController {
    selected: Option<i32>,
    mode: Mode,
    city_window: Option<i32>,
    pending_war: Option<PendingWar>,
    /// An order went out for the selected unit and its `World` is still to come.
    commanded: bool,
    last_turn: Option<i32>,
    /// The `Query::Path` in flight (at most one), and what it was for.
    path_query: Option<u32>,
    path_for: Option<(i32, i16, i16)>,
    hints: Hints,
    hover: Option<(i16, i16)>,
    /// When the mouse came to `hover`.
    hover_since: u64,
    /// Copies of the contexts of the selected unit and of the city in focus, taken by `sync_contexts`.
    unit_ctx: Option<UnitContext>,
    city_ctx: Option<CityContext>,
    /// The view is `base` plus the optimistic layer (a server session): a blocker whose target the layer
    /// has already settled is not one (NC-D13).
    layered: bool,
    /// Orders given to units, by number, for the selection to go back to a unit whose order was rolled back.
    sent: Vec<(CmdId, i32)>,
    /// The selection moved on by itself after an order to this unit and the player has chosen nothing since.
    advanced_from: Option<i32>,
}

/// Orders remembered for [`PlayController::on_order_rolled_back`]; the oldest are forgotten.
const SENT_CAP: usize = 64;

impl Default for PlayController {
    fn default() -> Self {
        Self::new()
    }
}

impl PlayController {
    pub fn new() -> Self {
        Self {
            selected: None,
            mode: Mode::Normal,
            city_window: None,
            pending_war: None,
            commanded: false,
            last_turn: None,
            path_query: None,
            path_for: None,
            hints: Hints::default(),
            hover: None,
            hover_since: 0,
            unit_ctx: None,
            city_ctx: None,
            layered: false,
            sent: Vec::new(),
            advanced_from: None,
        }
    }

    /// The controller reads the view of a server session (`base` plus the layer), not a host's `World`.
    pub fn set_layered(&mut self, layered: bool) {
        self.layered = layered;
    }

    /// The blocker of the end of turn as the buttons use it: the layer's pictures taken into account.
    pub fn blocker(&self, world: &WorldView) -> Option<Blocker> {
        super::blocker_of(world, self.layered)
    }

    pub fn selected_unit(&self) -> Option<i32> {
        self.selected
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// City whose window is open.
    pub fn city_window(&self) -> Option<i32> {
        self.city_window
    }

    /// A "Declare war?" question is open.
    pub fn war_question(&self) -> bool {
        self.pending_war.is_some()
    }

    pub fn hints(&self) -> &Hints {
        &self.hints
    }

    // ------------------------------------------------------------------ contexts and focus

    /// Takes what the controls read from the contexts (4.6); call every frame, after the contexts of the
    /// frame are in. The copies are made when a context changed, not every frame.
    pub fn sync_contexts(&mut self, cx: &ContextView<'_>) {
        match self.selected.map(|u| cx.unit(u)).and_then(|c| c.get()) {
            Some(c) => {
                if self.unit_ctx.as_ref().map(|o| (o.unit, o.view_seq))
                    != Some((c.unit, c.view_seq))
                {
                    self.unit_ctx = Some(c.clone());
                }
            }
            None => self.unit_ctx = None,
        }
        let city = self.focus_city();
        match city.map(|id| cx.city(id)).and_then(|c| c.get()) {
            Some(c) => {
                if self.city_ctx.as_ref().map(|o| (o.city, o.view_seq))
                    != Some((c.city, c.view_seq))
                {
                    self.city_ctx = Some(c.clone());
                }
            }
            None => self.city_ctx = None,
        }
    }

    /// The city the focus names: the one that aims, else the one whose window is open.
    fn focus_city(&self) -> Option<i32> {
        match self.mode {
            Mode::RangeAttack(Shooter::City(id)) => Some(id),
            _ => self.city_window,
        }
    }

    /// What the server should compute contexts for (NC-D7): the selected unit, the next three that
    /// need orders in the order of "next unit", the city in focus.
    pub fn focus_plan(&self, world: &WorldView) -> FocusPlan {
        let selected = self
            .selected
            .filter(|id| selection::own_unit(world, *id).is_some());
        let mut next = Vec::new();
        let mut after = selected;
        while next.len() < FOCUS_NEXT {
            match selection::next_needing_orders(world, after) {
                Some(id) if Some(id) != selected && !next.contains(&id) => {
                    next.push(id);
                    after = Some(id);
                }
                _ => break,
            }
        }
        FocusPlan::new(selected, next, self.focus_city())
    }

    /// The tree of the selected unit, if its context is in (whatever its age).
    pub(super) fn tree(&self) -> Option<&UnitContext> {
        self.unit_ctx
            .as_ref()
            .filter(|c| Some(c.unit) == self.selected)
    }

    /// Plots the shooter can attack, by its context; `None` while the context is not in.
    pub(super) fn ranged_targets(&self, shooter: Shooter) -> Option<Vec<(i16, i16)>> {
        match shooter {
            Shooter::Unit(u) => self
                .tree()
                .filter(|c| c.unit == u)
                .map(|c| c.ranged_targets().iter().map(|t| (t.x, t.y)).collect()),
            Shooter::City(id) => self
                .city_ctx
                .as_ref()
                .filter(|c| c.city == id)
                .map(|c| c.targets.iter().map(|t| (t.x, t.y)).collect()),
        }
    }

    /// The plot under the mouse.
    pub(super) fn hovered(&self) -> Option<(i16, i16)> {
        self.hover
    }

    /// The path under the mouse was asked and the target cannot be reached: the screen says so.
    pub fn hover_unreachable(&self) -> bool {
        matches!(&self.hints.path[..], [n] if n.flags.contains(ReachFlags::UNREACHABLE))
    }

    // ------------------------------------------------------------------ mouse

    /// Left click on plot `cell` (already picked).
    pub fn left_click(&mut self, world: &WorldView, cell: (i32, i32)) -> Outputs {
        let mut out = Outputs::default();
        let Some((x, y)) = plot_of(world, cell) else {
            return out;
        };
        if let Mode::RangeAttack(shooter) = self.mode {
            // Once the context has said what can be shot, a click elsewhere keeps the mode on (as the
            // original's targeting mode); before the context the server judges the shot.
            // A civilization at peace is asked about first, even if the context lists no target there.
            if let Target::Peace { team } = target_at(world, x, y) {
                self.cancel_mode();
                self.ask_war(world, team, WarOrder::Range { shooter, x, y }, &mut out);
            } else if self
                .ranged_targets(shooter)
                .is_none_or(|t| t.contains(&(x, y)))
            {
                self.range_attack(shooter, x, y, &mut out);
            }
            return out;
        }
        if self.pending_war.is_some() {
            return out;
        }
        let stack = selection::own_units_at(world, x, y);
        let own_city = selection::city_at(world, x, y)
            .filter(|c| c.owner == world.human)
            .map(|c| c.id);
        match selection::cycle(&stack, self.selected, own_city, self.city_window) {
            Some(Pick::Unit(id)) => self.select(id, &mut out),
            Some(Pick::City(id)) => self.open_city(id, &mut out),
            None => {
                if let Some(id) = own_city {
                    self.open_city(id, &mut out);
                }
            }
        }
        out
    }

    /// Right click on plot `cell`: orders the selected unit there (a move; onto an enemy it is an
    /// attack). In the ranged-attack mode it cancels the mode.
    pub fn right_click(&mut self, world: &WorldView, cell: (i32, i32)) -> Outputs {
        let mut out = Outputs::default();
        if self.mode != Mode::Normal {
            self.cancel_mode();
            return out;
        }
        let Some((x, y)) = plot_of(world, cell) else {
            return out;
        };
        let Some(unit) = self
            .selected
            .filter(|u| selection::own_unit(world, *u).is_some())
        else {
            return out;
        };
        if self.pending_war.is_some() || world.status != TurnStatus::HumanTurn {
            return out;
        }
        match target_at(world, x, y) {
            Target::Free | Target::Enemy => {
                self.order(Command::UnitMoveTo { unit, x, y }, &mut out);
            }
            Target::Peace { team } => {
                self.ask_war(world, team, WarOrder::Move { unit, x, y }, &mut out);
            }
        }
        out
    }

    /// Opens the "Declare war?" question for `order`.
    fn ask_war(&mut self, world: &WorldView, team: TeamId, order: WarOrder, out: &mut Outputs) {
        self.pending_war = Some(PendingWar { team, order });
        let civ = world
            .players
            .iter()
            .find(|p| p.team == team)
            .map(|p| p.civ_name.clone())
            .unwrap_or_default();
        out.effects.push(UiEffect::ConfirmWar {
            team,
            civ,
            ranged: matches!(order, WarOrder::Range { .. }),
        });
    }

    /// The mouse is over plot `cell` (or off the map) at `now_ms`. A plot of the unit's move tree has its
    /// path in the tree; for any other the path is asked once the mouse has rested [`PATH_DWELL_MS`],
    /// one question in flight at a time.
    pub fn hover(&mut self, world: &WorldView, cell: Option<(i32, i32)>, now_ms: u64) -> Outputs {
        let mut out = Outputs::default();
        let at = cell.and_then(|c| plot_of(world, c));
        if at != self.hover {
            self.hover = at;
            self.hover_since = now_ms;
            self.hints.path.clear();
            self.path_for = None;
        }
        if let (Some((x, y)), Some(unit)) = (at, self.selected)
            && self.mode == Mode::Normal
            && selection::own_unit(world, unit).is_some_and(|u| (u.x, u.y) != (x, y))
            && !self.tree().is_some_and(|c| c.reaches(x, y))
            && now_ms.saturating_sub(self.hover_since) >= PATH_DWELL_MS
            && self.path_query.is_none()
            && self.path_for != Some((unit, x, y))
        {
            self.path_for = Some((unit, x, y));
            out.queries.push(Query::Path { unit, x, y });
        }
        out
    }

    /// The answer of the "Declare war?" question: yes declares war, then moves.
    pub fn answer_war(&mut self, yes: bool) -> Outputs {
        let mut out = Outputs::default();
        if let Some(w) = self.pending_war.take()
            && yes
        {
            out.commands.push(Command::DeclareWar { team: w.team });
            match w.order {
                WarOrder::Move { unit, x, y } => {
                    self.order(Command::UnitMoveTo { unit, x, y }, &mut out)
                }
                WarOrder::Range { shooter, x, y } => self.range_attack(shooter, x, y, &mut out),
            }
        }
        out
    }

    // ------------------------------------------------------------------ keyboard and buttons

    pub fn key(&mut self, world: &WorldView, key: &KeyEvent) -> Outputs {
        match hotkeys::map(key) {
            Some(h) => self.hotkey(world, h),
            None => Outputs::default(),
        }
    }

    pub fn hotkey(&mut self, world: &WorldView, key: HotKey) -> Outputs {
        let mut out = Outputs::default();
        match key {
            HotKey::EndTurn { force } => self.end_turn(world, force, &mut out),
            HotKey::ToggleGrid => out.effects.push(UiEffect::ToggleGrid),
            HotKey::Center => {
                let at = self
                    .selected
                    .and_then(|id| selection::own_unit(world, id))
                    .map(|u| (u.x, u.y))
                    .or_else(|| capital(world));
                if let Some((x, y)) = at {
                    out.effects.push(UiEffect::CenterOn { x, y });
                }
            }
            HotKey::NextUnit => self.next_unit(world, &mut out),
            HotKey::Escape => self.escape(&mut out),
            HotKey::Fortify => {
                self.unit_order(world, &mut out, |unit| Command::UnitFortify { unit });
            }
            HotKey::Sleep => self.unit_order(world, &mut out, |unit| Command::UnitSleep { unit }),
            HotKey::Skip => self.unit_order(world, &mut out, |unit| Command::UnitSkip { unit }),
            HotKey::FoundCity => {
                let shooter = self
                    .selected
                    .and_then(|id| selection::own_unit(world, id))
                    .is_some_and(|u| u.ranged > 0);
                if shooter {
                    out = self.toggle_range_attack(world);
                } else {
                    self.unit_order(world, &mut out, |unit| Command::UnitFoundCity { unit });
                }
            }
            HotKey::Heal => self.unit_order(world, &mut out, |unit| Command::UnitHeal { unit }),
            HotKey::Automate => {
                self.unit_order(world, &mut out, |unit| Command::UnitAutomateBuild { unit });
            }
            HotKey::Explore => {
                self.unit_order(world, &mut out, |unit| Command::UnitAutomateExplore {
                    unit,
                });
            }
            HotKey::Cancel => self.unit_order(world, &mut out, |unit| Command::UnitCancel { unit }),
        }
        out
    }

    /// An order for the selected unit from the unit panel (its action buttons): sent like a hotkey
    /// order, so the selection moves on once the unit is done. Nothing without an own selected unit
    /// or outside the human's turn.
    pub fn unit_command(&mut self, world: &WorldView, cmd: Command) -> Outputs {
        let mut out = Outputs::default();
        let unit = match cmd {
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
            | Command::UnitWake { unit } => unit,
            _ => return out,
        };
        if self.selected == Some(unit) {
            self.unit_order(world, &mut out, |_| cmd);
        }
        out
    }

    /// The "Ranged attack" button or B: enters the mode for the selected unit, or leaves it when it
    /// is on.
    pub fn toggle_range_attack(&mut self, world: &WorldView) -> Outputs {
        if matches!(self.mode, Mode::RangeAttack(Shooter::Unit(u)) if Some(u) == self.selected) {
            self.cancel_mode();
            return Outputs::default();
        }
        self.begin_range_attack(world)
    }

    /// The end-turn button (or Enter): goes to the blocker, or ends the turn.
    pub fn end_turn_button(&mut self, world: &WorldView) -> Outputs {
        let mut out = Outputs::default();
        self.end_turn(world, false, &mut out);
        out
    }

    /// The hovered plot when a right click there would be a melee attack of the selected unit (an
    /// enemy stands there and the unit has a combat strength): highlighted as a target.
    pub fn melee_target(&self, world: &WorldView) -> Option<(i16, i16)> {
        let (x, y) = self.hover?;
        let unit = selection::own_unit(world, self.selected?)?;
        let fighter = unit.strength > 0 && !unit.flags.contains(UnitFlags::CIVILIAN);
        (self.mode == Mode::Normal && fighter && target_at(world, x, y) == Target::Enemy)
            .then_some((x, y))
    }

    /// "Ranged attack" button of the unit panel: the next left click picks the target.
    pub fn begin_range_attack(&mut self, world: &WorldView) -> Outputs {
        let mut out = Outputs::default();
        if world.status != TurnStatus::HumanTurn {
            return out;
        }
        if let Some(unit) = self
            .selected
            .filter(|u| selection::own_unit(world, *u).is_some())
        {
            self.enter_range_mode(Shooter::Unit(unit), &mut out);
        }
        out
    }

    /// "Ranged attack" of the city window.
    pub fn begin_city_range_attack(&mut self, city: i32) -> Outputs {
        let mut out = Outputs::default();
        self.enter_range_mode(Shooter::City(city), &mut out);
        out
    }

    /// A unit chosen from the UI (a flag, a list).
    pub fn select_unit(&mut self, world: &WorldView, id: i32) -> Outputs {
        let mut out = Outputs::default();
        if selection::own_unit(world, id).is_some() {
            self.select(id, &mut out);
        }
        out
    }

    /// Opens the window of an own city from the UI (production still blocks after a choice).
    pub fn open_city_window(&mut self, world: &WorldView, city: i32) -> Outputs {
        let mut out = Outputs::default();
        if world
            .cities
            .iter()
            .any(|c| c.id == city && c.owner == world.human)
        {
            self.open_city(city, &mut out);
        }
        out
    }

    /// The city window was closed from the UI.
    pub fn close_city_window(&mut self) {
        self.city_window = None;
    }

    // ------------------------------------------------------------------ host messages

    /// A new `World` arrived. Drops what no longer exists and, after an order, moves the selection
    /// to the next unit that needs orders (AC-5.10).
    pub fn on_world(&mut self, world: &WorldView) -> Outputs {
        let mut out = Outputs::default();
        if self.last_turn.is_some_and(|t| t != world.turn) {
            self.commanded = false;
        }
        self.last_turn = Some(world.turn);
        if world.status != TurnStatus::HumanTurn {
            self.cancel_mode();
            self.pending_war = None;
        }
        // A path answered for an earlier view is not drawn; the mouse asks again if it is still there.
        self.hints.path.clear();
        self.path_for = None;
        if self.city_window.is_some_and(|c| {
            !world
                .cities
                .iter()
                .any(|x| x.id == c && x.owner == world.human)
        }) {
            self.city_window = None;
            out.effects.push(UiEffect::CloseCityWindow);
        }
        let Some(id) = self.selected else {
            self.commanded = false;
            return out;
        };
        let Some(unit) = selection::own_unit(world, id) else {
            self.deselect();
            return out;
        };
        let done = self.commanded
            && (unit.moves_left <= 0 || !unit.flags.contains(UnitFlags::NEEDS_ORDERS));
        self.commanded = false;
        if done {
            match selection::next_needing_orders(world, Some(id)) {
                Some(next) => {
                    self.select(next, &mut out);
                    self.center_on_unit(world, next, &mut out);
                }
                None => self.deselect(),
            }
            // Not the player's choice: an order that is rolled back brings the selection back (4.4).
            self.advanced_from = Some(id);
        }
        out
    }

    /// An order of the sink went out under number `id` (`0`: the B1 session, which has no numbers).
    pub fn on_order_sent(&mut self, id: CmdId, cmd: &Command) {
        if id == 0 {
            return;
        }
        if let Some(unit) = unit_of(cmd) {
            self.sent.push((id, unit));
            if self.sent.len() > SENT_CAP {
                self.sent.remove(0);
            }
        }
    }

    /// The order was applied: nothing to take back.
    pub fn on_order_applied(&mut self, id: CmdId) {
        self.sent.retain(|(i, _)| *i != id);
    }

    /// The picture of order `id` came off the screen (it was rejected, lost, or its result is unknown). If the
    /// selection went on from that unit by itself and the player has chosen nothing since, it comes back
    /// to the unit, when the unit is there and waits for orders (NC6 Should, net-client 4.4).
    pub fn on_order_rolled_back(&mut self, world: &WorldView, id: CmdId) -> Outputs {
        let mut out = Outputs::default();
        let Some(at) = self.sent.iter().position(|(i, _)| *i == id) else {
            return out;
        };
        let unit = self.sent.remove(at).1;
        let waits = selection::own_unit(world, unit)
            .is_some_and(|u| u.flags.contains(UnitFlags::NEEDS_ORDERS));
        if self.advanced_from == Some(unit) && self.selected != Some(unit) && waits {
            self.select(unit, &mut out);
            self.center_on_unit(world, unit, &mut out);
        }
        out
    }

    /// `id` is what the sink returned for `query` (0: nothing was sent). Only `Query::Path` is the
    /// controller's; everything else the controls need comes in the contexts.
    pub fn query_sent(&mut self, id: u32, query: &Query) {
        if id != 0 && matches!(query, Query::Path { .. }) {
            self.path_query = Some(id);
        }
    }

    pub fn on_query_result(&mut self, id: u32, answer: &QueryAnswer) {
        if self.path_query != Some(id) {
            return;
        }
        self.path_query = None;
        match answer {
            QueryAnswer::Path(nodes) => self.hints.path.clone_from(nodes),
            QueryAnswer::Error { reason } => tracing::debug!("query {id} failed: {reason}"),
            _ => {}
        }
    }

    /// The order was refused (no connection, too many in flight) or the host rejected it: no `World`
    /// follows, so nothing to advance.
    pub fn on_command_rejected(&mut self) {
        self.commanded = false;
    }

    // ------------------------------------------------------------------ internals

    fn select(&mut self, id: i32, _out: &mut Outputs) {
        self.selected = Some(id);
        self.advanced_from = None;
        self.commanded = false;
        self.cancel_mode();
        self.hints = Hints::default();
        self.path_for = None;
        self.hover = None;
        self.unit_ctx = None;
        tracing::info!("selected unit {id}");
    }

    fn deselect(&mut self) {
        self.selected = None;
        self.advanced_from = None;
        self.commanded = false;
        self.cancel_mode();
        self.hints = Hints::default();
        self.unit_ctx = None;
    }

    fn open_city(&mut self, city: i32, out: &mut Outputs) {
        self.city_window = Some(city);
        out.effects.push(UiEffect::OpenCity { city });
    }

    fn cancel_mode(&mut self) {
        self.mode = Mode::Normal;
    }

    /// The mode on: the targets are in the context of the shooter, which the focus asks for.
    fn enter_range_mode(&mut self, shooter: Shooter, _out: &mut Outputs) {
        self.mode = Mode::RangeAttack(shooter);
    }

    fn range_attack(&mut self, shooter: Shooter, x: i16, y: i16, out: &mut Outputs) {
        self.cancel_mode();
        match shooter {
            Shooter::Unit(unit) => self.order(Command::UnitRangeAttack { unit, x, y }, out),
            Shooter::City(city) => out.commands.push(Command::CityRangeAttack { city, x, y }),
        }
    }

    /// An order for the selected unit; its `World` may advance the selection.
    fn order(&mut self, cmd: Command, out: &mut Outputs) {
        self.commanded = true;
        out.commands.push(cmd);
    }

    fn unit_order(&mut self, world: &WorldView, out: &mut Outputs, f: impl Fn(i32) -> Command) {
        if world.status != TurnStatus::HumanTurn {
            return;
        }
        if let Some(unit) = self
            .selected
            .filter(|u| selection::own_unit(world, *u).is_some())
        {
            self.order(f(unit), out);
        }
    }

    fn next_unit(&mut self, world: &WorldView, out: &mut Outputs) {
        if let Some(next) = selection::next_needing_orders(world, self.selected) {
            self.select(next, out);
            self.center_on_unit(world, next, out);
        }
    }

    /// Cancels, in order: the question, the mode, the city window, the selection.
    fn escape(&mut self, out: &mut Outputs) {
        if self.pending_war.take().is_some() {
            return;
        }
        if self.mode != Mode::Normal {
            self.cancel_mode();
        } else if self.city_window.take().is_some() {
            out.effects.push(UiEffect::CloseCityWindow);
        } else {
            self.deselect();
        }
    }

    fn end_turn(&mut self, world: &WorldView, force: bool, out: &mut Outputs) {
        if world.status != TurnStatus::HumanTurn {
            return;
        }
        if force {
            out.commands.push(Command::EndTurn { force: true });
            return;
        }
        match self.blocker(world) {
            // "City can attack": the city aims (the original's blocker button selects the city's
            // range strike).
            Some(b) if b.kind == BLOCKING_CITY_RANGE_ATTACK => match b.target {
                BlockerTarget::City(id) if world.cities.iter().any(|c| c.id == id) => {
                    if let Some(c) = world.cities.iter().find(|c| c.id == id) {
                        out.effects.push(UiEffect::CenterOn { x: c.x, y: c.y });
                    }
                    self.enter_range_mode(Shooter::City(id), out);
                }
                target => self.go_to_blocker(world, target, out),
            },
            Some(b) => self.go_to_blocker(world, b.target, out),
            None => out.commands.push(Command::EndTurn { force: false }),
        }
    }

    fn go_to_blocker(&mut self, world: &WorldView, target: BlockerTarget, out: &mut Outputs) {
        match target {
            BlockerTarget::Unit(id) => {
                if selection::own_unit(world, id).is_some() {
                    self.select(id, out);
                    self.center_on_unit(world, id, out);
                }
            }
            BlockerTarget::City(id) => {
                if let Some(c) = world.cities.iter().find(|c| c.id == id) {
                    out.effects.push(UiEffect::CenterOn { x: c.x, y: c.y });
                    self.open_city(id, out);
                }
            }
            BlockerTarget::Plot { x, y } => out.effects.push(UiEffect::CenterOn { x, y }),
            BlockerTarget::None => {}
        }
    }

    fn center_on_unit(&self, world: &WorldView, id: i32, out: &mut Outputs) {
        if let Some(u) = selection::own_unit(world, id) {
            out.effects.push(UiEffect::CenterOn { x: u.x, y: u.y });
        }
    }
}

fn plot_of(world: &WorldView, (x, y): (i32, i32)) -> Option<(i16, i16)> {
    let m = &world.map;
    if x < 0 || y < 0 || x >= i32::from(m.width) || y >= i32::from(m.height) {
        return None;
    }
    Some((x as i16, y as i16))
}

fn capital(world: &WorldView) -> Option<(i16, i16)> {
    world
        .cities
        .iter()
        .filter(|c| c.owner == world.human)
        .min_by_key(|c| (!c.flags.contains(CityFlags::CAPITAL), c.id))
        .map(|c| (c.x, c.y))
}

/// Who stands on the plot, seen from the human.
fn target_at(world: &WorldView, x: i16, y: i16) -> Target {
    let owner: Option<PlayerId> = world
        .units
        .iter()
        .find(|u| u.owner != world.human && u.x == x && u.y == y)
        .map(|u| u.owner)
        .or_else(|| {
            selection::city_at(world, x, y)
                .filter(|c| c.owner != world.human)
                .map(|c| c.owner)
        });
    let Some(owner) = owner else {
        return Target::Free;
    };
    let Some(player) = world.players.iter().find(|p| p.id == owner) else {
        return Target::Enemy;
    };
    if player.flags.contains(PlayerFlags::BARBARIAN)
        || player.flags.contains(PlayerFlags::AT_WAR_WITH_VIEWER)
    {
        Target::Enemy
    } else {
        Target::Peace { team: player.team }
    }
}

#[cfg(test)]
mod tests {
    use client_core::Contexts;
    use platform::{Key, Modifiers, NamedKey};
    use protocol::{Blocker, ContextMsg, ReachNode, TargetKind, TargetView};

    use super::*;

    /// A context for `unit` listing `targets` as ranged targets, as seen at view 1.
    fn ranged_ctx(unit: i32, targets: &[(i16, i16)]) -> UnitContext {
        UnitContext {
            unit,
            view_seq: 1,
            reach: vec![ReachNode::default()],
            targets: targets
                .iter()
                .map(|&(x, y)| TargetView {
                    x,
                    y,
                    kind: TargetKind::Ranged,
                    flags: 0,
                    preview: None,
                })
                .collect(),
            ..UnitContext::default()
        }
    }

    /// The controller reads these contexts (fresh at view 1).
    fn give(c: &mut PlayController, unit: Option<UnitContext>, city: Option<CityContext>) {
        let mut store = Contexts::new();
        if let Some(u) = unit {
            store.apply(ContextMsg::Unit(u));
        }
        if let Some(ct) = city {
            store.apply(ContextMsg::City(ct));
        }
        c.sync_contexts(&ContextView::new(&store, 1));
    }

    /// A unit as the layer shows it after an order: no moves left, no orders wanted.
    fn spent(world: &WorldView, unit: i32) -> WorldView {
        let mut w = world.clone();
        let u = w.units.iter_mut().find(|u| u.id == unit).unwrap();
        u.moves_left = 0;
        u.flags = UnitFlags(u.flags.0 & !UnitFlags::NEEDS_ORDERS);
        w
    }

    /// The selection goes on after an order at once (on the layer's picture); if the order is rolled back and
    /// the player has chosen nothing since, it comes back to the unit (4.4); if he chose, it stays.
    #[test]
    fn a_rolled_back_order_brings_the_selection_back_unless_the_player_chose() {
        let mut c = PlayController::new();
        c.set_layered(true);
        let w = world();
        let _ = c.left_click(&w, cell(8, 11));
        assert_eq!(c.selected_unit(), Some(4));
        let out = c.right_click(&w, cell(10, 11));
        assert_eq!(out.commands.len(), 1);
        c.on_order_sent(7, &out.commands[0]);
        // The picture: the Archer has spent its moves, the next unit needing orders is selected.
        let shown = spent(&w, 4);
        let _ = c.on_world(&shown);
        assert_eq!(c.selected_unit(), Some(1));
        // The server rejected the order: the base shows the Archer waiting again.
        let back = c.on_order_rolled_back(&w, 7);
        assert_eq!(c.selected_unit(), Some(4));
        assert!(back.effects.contains(&UiEffect::CenterOn { x: 8, y: 11 }));
        // Said once: the order is forgotten.
        let _ = c.left_click(&w, cell(10, 10));
        assert_eq!(c.on_order_rolled_back(&w, 7), Outputs::default());

        // The player chose another unit after the selection moved on: it is left alone.
        let mut c = PlayController::new();
        c.set_layered(true);
        let _ = c.left_click(&w, cell(8, 11));
        let out = c.right_click(&w, cell(10, 11));
        c.on_order_sent(8, &out.commands[0]);
        let _ = c.on_world(&spent(&w, 4));
        let _ = c.left_click(&w, cell(10, 10));
        assert_eq!(c.selected_unit(), Some(5));
        let _ = c.on_order_rolled_back(&w, 8);
        assert_eq!(c.selected_unit(), Some(5));

        // An order that was applied has nothing to take back.
        let mut c = PlayController::new();
        c.set_layered(true);
        let _ = c.left_click(&w, cell(8, 11));
        let out = c.right_click(&w, cell(10, 11));
        c.on_order_sent(9, &out.commands[0]);
        let _ = c.on_world(&spent(&w, 4));
        c.on_order_applied(9);
        let _ = c.on_order_rolled_back(&w, 9);
        assert_eq!(c.selected_unit(), Some(1));
    }

    /// Fixture units: human 1 Settler (7,12), 3 Warrior (6,12) in Roma (city 1), 4 Archer (8,11),
    /// 5 Horseman (10,10); AI Warrior 9 at (13,9) at war; Thebes (city 3) at (30,10).
    fn world() -> WorldView {
        crate::test_world::world()
    }

    fn cell(x: i16, y: i16) -> (i32, i32) {
        (i32::from(x), i32::from(y))
    }

    fn key(k: Key, shift: bool) -> KeyEvent {
        KeyEvent {
            key: k,
            code: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers {
                shift,
                ..Modifiers::default()
            },
        }
    }

    fn ch(c: &str) -> KeyEvent {
        key(Key::Character(c.into()), false)
    }

    fn selecting(w: &WorldView, id: i32) -> PlayController {
        let mut c = PlayController::new();
        let u = selection::own_unit(w, id).unwrap();
        c.left_click(w, cell(u.x, u.y));
        assert_eq!(c.selected_unit(), Some(id));
        c
    }

    #[test]
    fn click_on_own_unit_selects_it() {
        let w = world();
        let mut c = PlayController::new();
        let out = c.left_click(&w, cell(8, 11));
        assert_eq!(c.selected_unit(), Some(4));
        assert!(out.queries.is_empty(), "the contexts come with the focus");
        assert!(out.commands.is_empty());
    }

    #[test]
    fn click_on_foreign_unit_or_empty_plot_does_not_select() {
        let w = world();
        let mut c = PlayController::new();
        c.left_click(&w, cell(13, 9));
        c.left_click(&w, cell(20, 5));
        c.left_click(&w, (-1, 3));
        assert_eq!(c.selected_unit(), None);
    }

    #[test]
    fn stacked_units_military_first_then_civilian_then_city() {
        let mut w = world();
        // Worker 8 joins Warrior 3 in Roma.
        let worker = w.units.iter_mut().find(|u| u.id == 8).unwrap();
        (worker.x, worker.y) = (6, 12);
        let mut c = PlayController::new();
        c.left_click(&w, cell(6, 12));
        assert_eq!(c.selected_unit(), Some(3), "military first");
        c.left_click(&w, cell(6, 12));
        assert_eq!(c.selected_unit(), Some(8), "then the civilian");
        let out = c.left_click(&w, cell(6, 12));
        assert_eq!(out.effects, [UiEffect::OpenCity { city: 1 }]);
        assert_eq!(c.city_window(), Some(1));
        c.left_click(&w, cell(6, 12));
        assert_eq!(c.selected_unit(), Some(3), "wraps once the window is open");
    }

    #[test]
    fn click_on_own_city_without_units_opens_the_window() {
        let mut w = world();
        w.units.retain(|u| u.id != 3);
        let mut c = PlayController::new();
        let out = c.left_click(&w, cell(6, 12));
        assert_eq!(out.effects, [UiEffect::OpenCity { city: 1 }]);
        // A foreign city is not opened.
        let out = c.left_click(&w, cell(30, 10));
        assert!(out.effects.is_empty());
    }

    #[test]
    fn right_click_sends_unit_move_to() {
        let w = world();
        let mut c = selecting(&w, 5);
        let out = c.right_click(&w, cell(11, 12));
        assert_eq!(
            out.commands,
            [Command::UnitMoveTo {
                unit: 5,
                x: 11,
                y: 12
            }]
        );
    }

    #[test]
    fn right_click_without_a_selection_or_off_the_map_does_nothing() {
        let w = world();
        let mut c = PlayController::new();
        assert!(c.right_click(&w, cell(11, 12)).commands.is_empty());
        let mut c = selecting(&w, 5);
        assert!(c.right_click(&w, (99, 3)).commands.is_empty());
        // A foreign unit is not selectable, hence not orderable.
        let mut c = PlayController::new();
        c.select_unit(&w, 9);
        assert_eq!(c.selected_unit(), None);
    }

    #[test]
    fn right_click_on_an_enemy_is_the_same_move_command() {
        let w = world();
        let mut c = selecting(&w, 5);
        let out = c.right_click(&w, cell(13, 9));
        assert_eq!(
            out.commands,
            [Command::UnitMoveTo {
                unit: 5,
                x: 13,
                y: 9
            }]
        );
        assert!(out.effects.is_empty());
    }

    #[test]
    fn right_click_on_a_civ_at_peace_asks_first() {
        let mut w = world();
        w.players[1].flags = protocol::PlayerFlags(PlayerFlags::ALIVE | PlayerFlags::MET);
        let mut c = selecting(&w, 5);
        let out = c.right_click(&w, cell(13, 9));
        assert!(out.commands.is_empty());
        assert_eq!(
            out.effects,
            [UiEffect::ConfirmWar {
                team: 1,
                civ: "Egypt".into(),
                ranged: false
            }]
        );
        assert!(c.war_question());
        // No: nothing is sent.
        assert!(c.answer_war(false).commands.is_empty());
        // Yes: war, then the move.
        c.right_click(&w, cell(13, 9));
        let out = c.answer_war(true);
        assert_eq!(
            out.commands,
            [
                Command::DeclareWar { team: 1 },
                Command::UnitMoveTo {
                    unit: 5,
                    x: 13,
                    y: 9
                }
            ]
        );
        assert!(!c.war_question());
        // Esc also drops the question.
        c.right_click(&w, cell(13, 9));
        c.hotkey(&w, HotKey::Escape);
        assert!(!c.war_question());
        assert!(c.answer_war(true).commands.is_empty());
    }

    #[test]
    fn a_ranged_attack_on_a_civ_at_peace_asks_first() {
        let mut w = world();
        w.players[1].flags = protocol::PlayerFlags(PlayerFlags::ALIVE | PlayerFlags::MET);
        let mut c = selecting(&w, 4);
        // The context does not list the peaceful civ's plot as a target: still asked.
        c.toggle_range_attack(&w);
        give(&mut c, Some(ranged_ctx(4, &[(10, 11)])), None);
        let out = c.left_click(&w, cell(13, 9));
        assert!(out.commands.is_empty());
        assert_eq!(
            out.effects,
            [UiEffect::ConfirmWar {
                team: 1,
                civ: "Egypt".into(),
                ranged: true
            }]
        );
        assert!(c.war_question());
        assert_eq!(c.mode(), Mode::Normal);
        // No: nothing is sent.
        assert!(c.answer_war(false).commands.is_empty());
        // Yes: war, then the shot.
        c.begin_range_attack(&w);
        c.left_click(&w, cell(13, 9));
        assert_eq!(
            c.answer_war(true).commands,
            [
                Command::DeclareWar { team: 1 },
                Command::UnitRangeAttack {
                    unit: 4,
                    x: 13,
                    y: 9
                }
            ]
        );
        assert!(!c.war_question());
        // A city's shot asks the same way.
        c.begin_city_range_attack(1);
        let out = c.left_click(&w, cell(13, 9));
        assert!(matches!(
            &out.effects[..],
            [UiEffect::ConfirmWar { ranged: true, .. }]
        ));
        assert_eq!(
            c.answer_war(true).commands,
            [
                Command::DeclareWar { team: 1 },
                Command::CityRangeAttack {
                    city: 1,
                    x: 13,
                    y: 9
                }
            ]
        );
        // A plot at war needs no question.
        let w = world();
        let mut c = selecting(&w, 4);
        c.begin_range_attack(&w);
        assert!(c.left_click(&w, cell(13, 9)).effects.is_empty());
    }

    #[test]
    fn range_attack_mode_targets_and_esc_leaves() {
        let w = world();
        let mut c = selecting(&w, 4);
        let out = c.begin_range_attack(&w);
        assert_eq!(c.mode(), Mode::RangeAttack(Shooter::Unit(4)));
        assert!(out.queries.is_empty(), "the targets are in the context");
        // Before the context the server judges: a left click shoots and leaves the mode.
        let out = c.left_click(&w, cell(10, 11));
        assert_eq!(
            out.commands,
            [Command::UnitRangeAttack {
                unit: 4,
                x: 10,
                y: 11
            }]
        );
        assert_eq!(c.mode(), Mode::Normal);
        // Esc leaves the mode, the selection stays.
        c.begin_range_attack(&w);
        c.key(&w, &key(Key::Named(NamedKey::Escape), false));
        assert_eq!(c.mode(), Mode::Normal);
        assert_eq!(c.selected_unit(), Some(4));
        // In the mode a right click does not move.
        c.begin_range_attack(&w);
        assert!(c.right_click(&w, cell(10, 11)).commands.is_empty());
        assert_eq!(c.mode(), Mode::Normal);
    }

    #[test]
    fn once_the_targets_are_known_only_a_target_is_shot() {
        let w = world();
        let mut c = selecting(&w, 4);
        c.toggle_range_attack(&w);
        give(&mut c, Some(ranged_ctx(4, &[(10, 11)])), None);
        // A plot that is not a target: nothing is sent, the mode stays on.
        assert!(c.left_click(&w, cell(12, 12)).commands.is_empty());
        assert_eq!(c.mode(), Mode::RangeAttack(Shooter::Unit(4)));
        assert_eq!(
            c.left_click(&w, cell(10, 11)).commands,
            [Command::UnitRangeAttack {
                unit: 4,
                x: 10,
                y: 11
            }]
        );
        assert_eq!(c.mode(), Mode::Normal);
        // The button toggles; not in the rivals' turn.
        c.toggle_range_attack(&w);
        assert_ne!(c.mode(), Mode::Normal);
        c.toggle_range_attack(&w);
        assert_eq!(c.mode(), Mode::Normal);
        let mut ai = w.clone();
        ai.status = TurnStatus::AiTurn { active: 1 };
        assert!(c.toggle_range_attack(&ai).queries.is_empty());
        assert_eq!(c.mode(), Mode::Normal);
    }

    #[test]
    fn panel_orders_go_to_the_selected_unit_and_move_the_selection_on() {
        let mut w = world();
        let mut c = selecting(&w, 4);
        assert!(
            c.unit_command(&w, Command::UnitSkip { unit: 5 })
                .commands
                .is_empty(),
            "not the selected unit"
        );
        assert!(
            c.unit_command(&w, Command::EndTurn { force: true })
                .commands
                .is_empty()
        );
        assert_eq!(
            c.unit_command(&w, Command::UnitFortify { unit: 4 })
                .commands,
            [Command::UnitFortify { unit: 4 }]
        );
        let a = w.units.iter_mut().find(|u| u.id == 4).unwrap();
        a.flags = protocol::UnitFlags(a.flags.0 & !UnitFlags::NEEDS_ORDERS);
        c.on_world(&w);
        assert_eq!(c.selected_unit(), Some(1), "the next unit needing orders");
        w.status = TurnStatus::AiTurn { active: 1 };
        assert!(
            c.unit_command(&w, Command::UnitSkip { unit: 1 })
                .commands
                .is_empty()
        );
    }

    #[test]
    fn hovering_an_enemy_with_a_fighter_marks_the_melee_target() {
        let w = world();
        let mut c = selecting(&w, 5);
        c.hover(&w, Some(cell(13, 9)), 0);
        assert_eq!(c.melee_target(&w), Some((13, 9)));
        assert_eq!(c.highlights(&w).targets, [(13, 9)]);
        c.hover(&w, Some(cell(12, 9)), 0);
        assert_eq!(c.melee_target(&w), None, "an empty plot");
        // A civilian does not attack.
        let mut s = selecting(&w, 1);
        s.hover(&w, Some(cell(13, 9)), 0);
        assert_eq!(s.melee_target(&w), None);
    }

    #[test]
    fn enter_on_city_can_attack_aims_with_the_city() {
        let mut w = world();
        w.blocker = Some(Blocker {
            kind: BLOCKING_CITY_RANGE_ATTACK,
            target: BlockerTarget::City(1),
        });
        let mut c = PlayController::new();
        let out = c.key(&w, &key(Key::Named(NamedKey::Enter), false));
        assert!(out.commands.is_empty());
        assert_eq!(out.effects, [UiEffect::CenterOn { x: 6, y: 12 }]);
        assert!(out.queries.is_empty());
        assert_eq!(c.mode(), Mode::RangeAttack(Shooter::City(1)));
        assert_eq!(
            c.focus_plan(&w).city,
            Some(1),
            "the focus names the city that aims, so its context comes"
        );
        assert_eq!(c.highlights(&w).selected, Some((6, 12)), "the city aims");
    }

    #[test]
    fn city_range_attack() {
        let w = world();
        let mut c = PlayController::new();
        c.begin_city_range_attack(1);
        let out = c.left_click(&w, cell(8, 12));
        assert_eq!(
            out.commands,
            [Command::CityRangeAttack {
                city: 1,
                x: 8,
                y: 12
            }]
        );
    }

    #[test]
    fn range_mode_ends_with_the_selection_change_and_the_ai_turn() {
        let mut w = world();
        let mut c = selecting(&w, 4);
        c.begin_range_attack(&w);
        c.select_unit(&w, 5);
        assert_eq!(c.mode(), Mode::Normal);
        c.begin_range_attack(&w);
        w.status = TurnStatus::AiTurn { active: 1 };
        c.on_world(&w);
        assert_eq!(c.mode(), Mode::Normal);
    }

    #[test]
    fn after_an_order_the_next_unit_needing_orders_is_selected() {
        let mut w = world();
        // Archer 4 (needs orders) is selected and ordered; Settler 1 also needs orders.
        let mut c = selecting(&w, 4);
        assert_eq!(
            c.right_click(&w, cell(9, 11)).commands,
            [Command::UnitMoveTo {
                unit: 4,
                x: 9,
                y: 11
            }]
        );
        let u = w.units.iter_mut().find(|u| u.id == 4).unwrap();
        (u.x, u.y, u.moves_left) = (9, 11, 0);
        u.flags = protocol::UnitFlags(u.flags.0 & !UnitFlags::NEEDS_ORDERS);
        let out = c.on_world(&w);
        assert_eq!(c.selected_unit(), Some(1), "wraps to the lowest id");
        assert_eq!(out.effects, [UiEffect::CenterOn { x: 7, y: 12 }]);
        assert!(out.queries.is_empty());
    }

    #[test]
    fn selection_stays_when_the_unit_can_still_act_and_clears_when_nothing_is_left() {
        let mut w = world();
        let mut c = selecting(&w, 4);
        c.right_click(&w, cell(9, 11));
        // The order was only partly done: the unit still needs orders and has moves.
        assert!(c.on_world(&w).queries.is_empty());
        assert_eq!(c.selected_unit(), Some(4));
        // Nobody else needs orders and this one is done: no selection.
        for u in &mut w.units {
            u.flags = protocol::UnitFlags(u.flags.0 & !UnitFlags::NEEDS_ORDERS);
        }
        c.right_click(&w, cell(9, 11));
        c.on_world(&w);
        assert_eq!(c.selected_unit(), None);
    }

    #[test]
    fn a_rejected_order_does_not_advance_the_selection() {
        let mut w = world();
        let mut c = selecting(&w, 4);
        c.right_click(&w, cell(9, 11));
        c.on_command_rejected();
        w.units.iter_mut().find(|u| u.id == 4).unwrap().moves_left = 0;
        c.on_world(&w);
        assert_eq!(c.selected_unit(), Some(4));
    }

    #[test]
    fn a_vanished_unit_is_deselected() {
        let mut w = world();
        let mut c = selecting(&w, 5);
        w.units.retain(|u| u.id != 5);
        c.on_world(&w);
        assert_eq!(c.selected_unit(), None);
    }

    #[test]
    fn w_cycles_units_needing_orders() {
        let w = world();
        let mut c = PlayController::new();
        let out = c.key(&w, &ch("w"));
        assert_eq!(c.selected_unit(), Some(1));
        assert_eq!(out.effects, [UiEffect::CenterOn { x: 7, y: 12 }]);
        c.key(&w, &ch("w"));
        assert_eq!(c.selected_unit(), Some(4));
        c.key(&w, &ch("w"));
        assert_eq!(c.selected_unit(), Some(1), "wraps");
    }

    #[test]
    fn enter_with_a_blocker_goes_to_its_target() {
        let mut w = world();
        let enter = key(Key::Named(NamedKey::Enter), false);
        w.blocker = Some(Blocker {
            kind: 1,
            target: BlockerTarget::Unit(4),
        });
        let mut c = PlayController::new();
        let out = c.key(&w, &enter);
        assert!(out.commands.is_empty());
        assert_eq!(c.selected_unit(), Some(4));
        assert_eq!(out.effects, [UiEffect::CenterOn { x: 8, y: 11 }]);

        w.blocker = Some(Blocker {
            kind: 2,
            target: BlockerTarget::City(2),
        });
        let out = c.end_turn_button(&w);
        assert!(out.commands.is_empty());
        assert_eq!(
            out.effects,
            [
                UiEffect::CenterOn { x: 9, y: 17 },
                UiEffect::OpenCity { city: 2 }
            ]
        );
        assert_eq!(c.city_window(), Some(2));

        w.blocker = Some(Blocker {
            kind: 3,
            target: BlockerTarget::Plot { x: 3, y: 4 },
        });
        assert_eq!(
            c.key(&w, &enter).effects,
            [UiEffect::CenterOn { x: 3, y: 4 }]
        );
    }

    #[test]
    fn enter_without_a_blocker_ends_the_turn_and_shift_enter_forces() {
        let mut w = world();
        w.blocker = None;
        let mut c = PlayController::new();
        let out = c.key(&w, &key(Key::Named(NamedKey::Enter), false));
        assert_eq!(out.commands, [Command::EndTurn { force: false }]);
        // Shift+Enter ends the turn even with a blocker.
        w.blocker = Some(Blocker {
            kind: 1,
            target: BlockerTarget::Unit(4),
        });
        let out = c.key(&w, &key(Key::Named(NamedKey::Enter), true));
        assert_eq!(out.commands, [Command::EndTurn { force: true }]);
        assert_eq!(c.selected_unit(), None);
        // Not during the rivals' turn.
        w.status = TurnStatus::AiTurn { active: 1 };
        assert!(
            c.key(&w, &key(Key::Named(NamedKey::Enter), true))
                .commands
                .is_empty()
        );
        assert!(c.end_turn_button(&w).commands.is_empty());
    }

    #[test]
    fn escape_cancels_mode_then_window_then_selection() {
        let w = world();
        let mut c = selecting(&w, 3);
        c.begin_range_attack(&w);
        c.open_city_for_test(1);
        let esc = key(Key::Named(NamedKey::Escape), false);
        c.key(&w, &esc);
        assert_eq!(
            (c.mode(), c.city_window(), c.selected_unit()),
            (Mode::Normal, Some(1), Some(3))
        );
        let out = c.key(&w, &esc);
        assert_eq!(out.effects, [UiEffect::CloseCityWindow]);
        assert_eq!((c.city_window(), c.selected_unit()), (None, Some(3)));
        c.key(&w, &esc);
        assert_eq!(c.selected_unit(), None);
    }

    #[test]
    fn unit_hotkeys_order_the_selected_unit() {
        let w = world();
        let mut c = selecting(&w, 4);
        assert_eq!(
            c.key(&w, &ch("f")).commands,
            [Command::UnitFortify { unit: 4 }]
        );
        assert_eq!(
            c.key(&w, &ch("s")).commands,
            [Command::UnitSleep { unit: 4 }]
        );
        assert_eq!(
            c.key(&w, &ch(" ")).commands,
            [Command::UnitSkip { unit: 4 }]
        );
        for (k, cmd) in [
            ("h", Command::UnitHeal { unit: 4 }),
            ("a", Command::UnitAutomateBuild { unit: 4 }),
            ("e", Command::UnitAutomateExplore { unit: 4 }),
        ] {
            assert_eq!(c.key(&w, &ch(k)).commands, [cmd], "{k}");
        }
        assert_eq!(
            c.key(&w, &key(Key::Named(NamedKey::Backspace), false))
                .commands,
            [Command::UnitCancel { unit: 4 }]
        );
        // B: the archer has a ranged strength, so it aims (B again stops); the settler founds.
        let out = c.key(&w, &ch("b"));
        assert!(out.commands.is_empty() && out.queries.is_empty());
        assert_eq!(c.mode(), Mode::RangeAttack(Shooter::Unit(4)));
        c.key(&w, &ch("b"));
        assert_eq!(c.mode(), Mode::Normal);
        let mut settler = selecting(&w, 1);
        assert_eq!(
            settler.key(&w, &ch("b")).commands,
            [Command::UnitFoundCity { unit: 1 }]
        );
        let mut none = PlayController::new();
        assert!(none.key(&w, &ch("f")).commands.is_empty());
    }

    #[test]
    fn grid_and_center_hotkeys() {
        let w = world();
        let mut c = PlayController::new();
        assert_eq!(c.key(&w, &ch("g")).effects, [UiEffect::ToggleGrid]);
        // Without a selection C goes to the capital.
        assert_eq!(
            c.key(&w, &ch("c")).effects,
            [UiEffect::CenterOn { x: 6, y: 12 }]
        );
        let mut c = selecting(&w, 4);
        assert_eq!(
            c.key(&w, &ch("c")).effects,
            [UiEffect::CenterOn { x: 8, y: 11 }]
        );
    }

    #[test]
    fn only_the_path_answer_becomes_a_hint_and_only_for_its_own_question() {
        let w = world();
        let mut c = selecting(&w, 5);
        // A plot the tree does not hold: asked once, after the dwell.
        assert!(c.hover(&w, Some(cell(11, 10)), 0).queries.is_empty());
        let out = c.hover(&w, Some(cell(11, 10)), PATH_DWELL_MS);
        assert_eq!(
            out.queries,
            [Query::Path {
                unit: 5,
                x: 11,
                y: 10
            }]
        );
        c.query_sent(7, &out.queries[0]);
        // An answer to some other number is dropped.
        c.on_query_result(
            8,
            &QueryAnswer::Path(vec![PathNode {
                x: 1,
                y: 1,
                turn: 0,
                flags: Default::default(),
            }]),
        );
        assert!(c.hints().path.is_empty());
        c.on_query_result(
            7,
            &QueryAnswer::Path(vec![PathNode {
                x: 11,
                y: 10,
                turn: 1,
                flags: Default::default(),
            }]),
        );
        assert_eq!(c.hints().path.len(), 1);
        // Once per plot, and the mouse leaving the plot forgets the answer.
        assert!(c.hover(&w, Some(cell(11, 10)), 5_000).queries.is_empty());
        c.hover(&w, None, 5_100);
        assert!(c.hints().path.is_empty());
    }

    impl PlayController {
        fn open_city_for_test(&mut self, city: i32) {
            self.city_window = Some(city);
        }
    }

    #[test]
    fn the_focus_is_the_selected_unit_the_next_three_and_the_open_city() {
        let w = world();
        let c = PlayController::new();
        let plan = c.focus_plan(&w);
        assert_eq!((plan.selected, &plan.next[..]), (None, &[1, 4][..]));
        // Fixture units that need orders: 1 (Settler) and 4 (Archer), by id.
        let mut c = selecting(&w, 4);
        let plan = c.focus_plan(&w);
        assert_eq!(
            (plan.selected, &plan.next[..], plan.city),
            (Some(4), &[1][..], None)
        );
        // The window of a city is in the focus too.
        c.open_city_for_test(1);
        assert_eq!(c.focus_plan(&w).city, Some(1));
        // A unit that is gone is not selected in the plan.
        let mut gone = w.clone();
        gone.units.retain(|u| u.id != 4);
        assert_eq!(c.focus_plan(&gone).selected, None);
        // Many units need orders: three follow, in the order of "next unit", not the selected one.
        let mut many = w.clone();
        for u in many.units.iter_mut().filter(|u| u.owner == many.human) {
            u.flags = protocol::UnitFlags(u.flags.0 | UnitFlags::NEEDS_ORDERS);
        }
        let plan = selecting(&many, 3).focus_plan(&many);
        assert_eq!(plan.selected, Some(3));
        assert_eq!(plan.next.len(), 3);
        assert!(!plan.next.contains(&3));
        assert_eq!(plan.next[0], 4, "the next id after the selected one");
    }

    #[test]
    fn the_controller_copies_a_context_only_for_what_it_works_with() {
        let w = world();
        let mut c = selecting(&w, 4);
        give(&mut c, Some(ranged_ctx(4, &[(1, 1)])), None);
        assert!(c.tree().is_some());
        // The context of another unit is not the selected one's.
        give(&mut c, Some(ranged_ctx(5, &[(1, 1)])), None);
        assert!(c.tree().is_none());
        // Deselecting forgets the copy.
        give(&mut c, Some(ranged_ctx(4, &[(1, 1)])), None);
        c.hotkey(&w, HotKey::Escape);
        assert!(c.tree().is_none());
        // A city that aims takes its targets from the city's context.
        let mut c = PlayController::new();
        c.begin_city_range_attack(1);
        give(
            &mut c,
            None,
            Some(CityContext {
                city: 1,
                view_seq: 1,
                targets: vec![TargetView {
                    x: 8,
                    y: 12,
                    kind: TargetKind::Ranged,
                    flags: 0,
                    preview: None,
                }],
                ..CityContext::default()
            }),
        );
        assert_eq!(c.highlights(&w).targets, [(8, 12)]);
        assert!(
            c.left_click(&w, cell(9, 12)).commands.is_empty(),
            "not a target"
        );
        assert_eq!(c.left_click(&w, cell(8, 12)).commands.len(), 1);
    }
}
