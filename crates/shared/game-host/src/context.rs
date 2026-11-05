//! Focus and contexts (game-host-mp GH-D6, 4.6; net-protocol 4.9, NP-D12): what the worker precomputes for the
//! units and the city the seat is about to command.
//!
//! - The focus of a seat is the `Focus` it sent this turn, or the default: the first three of its units that
//!   need orders, by id, taken once at the start of every human phase.
//! - A context is asked from the core without side effects (GH-R11) and compared with the last one sent
//!   (xxh64 of the canonical bytes): the same is `Valid`, another is `Unit` or `City`, a unit that is gone or
//!   cannot act is `Dropped`.
//! - After every new view of the seat a context that the diff did not touch is `Valid` in the very frame of the
//!   diff; a touched one goes stale. Stale contexts are computed at once for the seat that gave an order (its
//!   first focus unit, and the city of a city order) and otherwise in the background, one per step. While the
//!   AI moves nothing is computed (GH-D14).
//!
//! What a context depends on is conservative: the plots of the move tree and of the targets with their
//! neighbours (a box of one plot around each, which holds the six hex neighbours in any row layout), the unit or
//! city records standing there, the record of the unit or the city itself, the players (war and peace change
//! the targets) and, for a city, the top bar (gold, prices). Doubt means a recompute.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use protocol::{
    CityContext, Codec, ContextMsg, Focus, PlayerId, Postcard, ReachFlags, Shooter, StepKind,
    TargetKind, TargetView, UnitContext, UnitFlags, UnitKey, ViewDiff, WorldView,
};

use crate::core::{Core, CoreError, CoreErrorKind, MapInfo};
use crate::worker::{Fb, Worker, micros};

/// Units a focus holds (net-protocol 4.9: up to 4, the first is the selected one).
pub(crate) const FOCUS_UNITS: usize = 4;

/// Rank of a city among the contexts of a seat: after all its units.
const CITY_RANK: usize = 100;

/// A context of a seat: a unit or the city of the focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Key {
    Unit(i32),
    City(i32),
}

/// What the last context sent depended on.
#[derive(Debug, Default, Clone)]
struct Deps {
    cells: BTreeSet<(i16, i16)>,
    top_bar: bool,
}

impl Deps {
    /// The plot and the box of plots around it (wrapped as the map wraps).
    fn add_around(&mut self, map: &MapInfo, x: i16, y: i16) {
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                if let Some(at) = wrapped(map, i32::from(x) + dx, i32::from(y) + dy) {
                    self.cells.insert(at);
                }
            }
        }
    }
}

fn wrapped(map: &MapInfo, x: i32, y: i32) -> Option<(i16, i16)> {
    let (w, h) = (i32::from(map.width), i32::from(map.height));
    let wrap = |v: i32, n: i32, on: bool| -> Option<i32> {
        if (0..n).contains(&v) {
            Some(v)
        } else if on && n > 0 {
            Some(v.rem_euclid(n))
        } else {
            None
        }
    };
    let x = wrap(x, w, map.wrap_x)?;
    let y = wrap(y, h, map.wrap_y)?;
    Some((i16::try_from(x).ok()?, i16::try_from(y).ok()?))
}

/// The context last sent for a key.
#[derive(Debug, Clone)]
struct Sent {
    /// xxh64 of the context with `view_seq` 0; for a dropped one, 0.
    hash: u64,
    deps: Deps,
    /// The seat was told `Dropped` (or, for a city, nothing: there is no such message).
    dropped: bool,
}

/// What a new view changed, from the diff against the view before it.
#[derive(Debug, Default)]
pub(crate) struct Touch {
    /// Everything: a keyframe, a new turn.
    all: bool,
    cells: BTreeSet<(i16, i16)>,
    units: BTreeSet<UnitKey>,
    cities: BTreeSet<UnitKey>,
    players: bool,
    top_bar: bool,
}

impl Touch {
    /// Nothing is known to be the same: a keyframe, a seat that is new.
    pub fn all() -> Self {
        Touch {
            all: true,
            ..Touch::default()
        }
    }

    /// Nothing changed (the same view sent again).
    pub fn none() -> Self {
        Touch::default()
    }

    /// The touch of `d`, which turns the view `old` into the next one. Call it before the diff is applied: the
    /// old places of the records that moved or left are in `old`.
    pub fn of(d: &ViewDiff, old: &WorldView, map: &MapInfo) -> Self {
        let mut t = Touch {
            all: d.header.is_some_and(|h| h.turn != old.turn),
            players: !d.players.upsert.is_empty() || !d.players.remove.is_empty(),
            top_bar: d.top_bar.is_some(),
            ..Touch::default()
        };
        let width = u32::from(map.width.max(1));
        for (i, _) in &d.plots {
            if let (Ok(x), Ok(y)) = (i16::try_from(i % width), i16::try_from(i / width)) {
                t.cells.insert((x, y));
            }
        }
        if !d.units.upsert.is_empty() || !d.units.remove.is_empty() {
            let old_at: BTreeMap<UnitKey, (i16, i16)> = old
                .units
                .iter()
                .map(|u| ((u.owner, u.id), (u.x, u.y)))
                .collect();
            for u in &d.units.upsert {
                t.units.insert((u.owner, u.id));
                t.cells.insert((u.x, u.y));
                t.cells.extend(old_at.get(&(u.owner, u.id)));
            }
            for k in &d.units.remove {
                t.units.insert(*k);
                t.cells.extend(old_at.get(k));
            }
        }
        if !d.cities.upsert.is_empty() || !d.cities.remove.is_empty() {
            let old_at: BTreeMap<UnitKey, (i16, i16)> = old
                .cities
                .iter()
                .map(|c| ((c.owner, c.id), (c.x, c.y)))
                .collect();
            for c in &d.cities.upsert {
                t.cities.insert((c.owner, c.id));
                t.cells.insert((c.x, c.y));
                t.cells.extend(old_at.get(&(c.owner, c.id)));
            }
            for k in &d.cities.remove {
                t.cities.insert(*k);
                t.cells.extend(old_at.get(k));
            }
        }
        t
    }
}

impl Sent {
    /// The diff changed something this context depends on.
    fn touched_by(&self, player: PlayerId, key: Key, t: &Touch) -> bool {
        if t.all || t.players {
            return true;
        }
        let own = match key {
            Key::Unit(id) => t.units.contains(&(player, id)),
            Key::City(id) => t.cities.contains(&(player, id)) || (self.deps.top_bar && t.top_bar),
        };
        own || t.cells.iter().any(|c| self.deps.cells.contains(c))
    }
}

/// The focus and the contexts of one seat.
#[derive(Debug, Default)]
pub(crate) struct CtxState {
    /// The `Focus` the seat sent this turn.
    explicit: Option<Focus>,
    /// The focus came while the AI moved: it holds for the turn that follows.
    carry: bool,
    /// The default focus of the turn.
    default: Vec<i32>,
    /// The turn the default was taken in.
    turn: Option<i32>,
    sent: BTreeMap<Key, Sent>,
    /// Keys that wait for a new context.
    pub(crate) stale: BTreeSet<Key>,
}

impl CtxState {
    fn focus_units(&self) -> &[i32] {
        match &self.explicit {
            Some(f) => &f.units,
            None => &self.default,
        }
    }

    fn focus_city(&self) -> Option<i32> {
        self.explicit.as_ref().and_then(|f| f.city)
    }

    /// The contexts the seat wants, units first in the order of the focus, the city last.
    fn wanted(&self) -> Vec<Key> {
        self.focus_units()
            .iter()
            .map(|&u| Key::Unit(u))
            .chain(self.focus_city().map(Key::City))
            .collect()
    }

    fn rank(&self, key: Key) -> usize {
        match key {
            Key::Unit(u) => self
                .focus_units()
                .iter()
                .position(|&x| x == u)
                .unwrap_or(CITY_RANK),
            Key::City(_) => CITY_RANK,
        }
    }
}

/// `focus` as the worker keeps it: no repeated unit, at most four.
fn normalized(focus: Focus) -> Focus {
    let mut units = Vec::new();
    for u in focus.units {
        if !units.contains(&u) && units.len() < FOCUS_UNITS {
            units.push(u);
        }
    }
    Focus {
        units,
        city: focus.city,
    }
}

/// What the core said about a key.
enum Built {
    Unit(UnitContext, Deps),
    City(CityContext, Deps),
    /// The unit is gone or cannot act.
    DroppedUnit(i32, Deps),
    /// The city is gone: there is no message for it, the seat only stops waiting.
    DroppedCity(Deps),
}

/// A core query that failed softly is a context that cannot be built (`None`); a core that is stuck stops
/// the worker.
fn soft<T>(r: Result<T, CoreError>) -> Result<Option<T>, CoreError> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(e) if e.kind == CoreErrorKind::Failed => {
            tracing::debug!("context query failed: {}", e.message);
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

fn hash_of(msg: &ContextMsg) -> u64 {
    // The bytes of a plain data type always encode; a failure would only make two contexts look different.
    protocol::xxh64(0, &Postcard.encode(msg).unwrap_or_default())
}

impl<C: Core> Worker<C> {
    /// `ToWorker::Focus`: the new focus replaces the old one; contexts it needs and the seat does not have
    /// are computed in the background.
    pub(crate) fn set_focus(&mut self, player: PlayerId, focus: Focus) {
        let in_humans = self.phase == protocol::TurnPhase::Humans;
        let Some(seat) = self.seats.get_mut(&player) else {
            return;
        };
        seat.ctx.explicit = Some(normalized(focus));
        seat.ctx.carry = !in_humans;
        Self::refresh_wanted(seat);
    }

    /// Makes `sent` and `stale` agree with what the seat wants: what it does not want is forgotten, what it
    /// wants and has not got is stale.
    fn refresh_wanted(seat: &mut crate::seats::Seat) {
        let wanted = seat.ctx.wanted();
        seat.ctx.sent.retain(|k, _| wanted.contains(k));
        seat.ctx.stale.retain(|k| wanted.contains(k));
        for k in wanted {
            if !seat.ctx.sent.contains_key(&k) {
                seat.ctx.stale.insert(k);
            }
        }
    }

    /// Bookkeeping after a new view of `player` (no core calls): a new turn gives the default focus, contexts
    /// the diff touched go stale, the others are `Valid` in this frame.
    pub(crate) fn ctx_after_view(&mut self, player: PlayerId, touch: &Touch, fb: &mut Fb) {
        let n = usize::from(self.cfg.focus_default);
        let Some(seat) = self.seats.get_mut(&player) else {
            return;
        };
        let Some(mirror) = seat.mirror.as_ref() else {
            return;
        };
        let rev = seat.rev;
        if seat.ctx.turn != Some(mirror.view.turn) {
            seat.ctx.turn = Some(mirror.view.turn);
            // A focus sent while the AI moved holds for the turn that follows; any other was for the turn
            // that is over.
            if !std::mem::take(&mut seat.ctx.carry) {
                seat.ctx.explicit = None;
            }
            let mut ids: Vec<i32> = mirror
                .view
                .units
                .iter()
                .filter(|u| u.owner == player && u.flags.contains(UnitFlags::NEEDS_ORDERS))
                .map(|u| u.id)
                .collect();
            ids.sort_unstable();
            ids.truncate(n);
            seat.ctx.default = ids;
        }
        Self::refresh_wanted(seat);
        for key in seat.ctx.wanted() {
            let Some(sent) = seat.ctx.sent.get(&key) else {
                continue;
            };
            if sent.touched_by(player, key, touch) {
                seat.ctx.stale.insert(key);
            } else if !seat.ctx.stale.contains(&key) && !sent.dropped {
                fb.valid(player, rev, key);
            }
        }
    }

    /// True when some seat has a context to compute.
    pub(crate) fn has_stale_contexts(&self) -> bool {
        self.seats.values().any(|s| !s.ctx.stale.is_empty())
    }

    /// The next context of the background: the seat of the last order first, then the selected units of all
    /// seats, then their next units, then cities.
    fn next_stale(&self) -> Option<(PlayerId, Key)> {
        self.seats
            .iter()
            .flat_map(|(&p, seat)| {
                seat.ctx
                    .stale
                    .iter()
                    .map(move |&k| ((self.last_order_seat != Some(p), seat.ctx.rank(k), p), p, k))
            })
            .min_by_key(|(order, _, _)| *order)
            .map(|(_, p, k)| (p, k))
    }

    /// One step of the background: the next stale context.
    pub(crate) fn ctx_background(&mut self, fb: &mut Fb) -> Result<(), CoreError> {
        fb.kind = Some(StepKind::Other);
        let Some((p, key)) = self.next_stale() else {
            return Ok(());
        };
        self.ctx_compute(p, key, fb)
    }

    /// After a batch: for every seat that gave an order, its first focus unit and the city of its city order,
    /// if they are stale, in the frame of the view.
    pub(crate) fn ctx_urgent(&mut self, fb: &mut Fb) -> Result<(), CoreError> {
        let ordered = std::mem::take(&mut self.ordered);
        for (p, city) in ordered {
            let Some(seat) = self.seats.get(&p) else {
                continue;
            };
            let mut keys: Vec<Key> = seat
                .ctx
                .focus_units()
                .first()
                .map(|&u| Key::Unit(u))
                .into_iter()
                .collect();
            keys.extend(city.map(Key::City));
            for key in keys {
                if self
                    .seats
                    .get(&p)
                    .is_some_and(|s| s.ctx.stale.contains(&key))
                {
                    self.ctx_compute(p, key, fb)?;
                }
            }
        }
        Ok(())
    }

    /// Every context a seat wants, now (a seat that has just taken a slot).
    pub(crate) fn ctx_all_now(&mut self, player: PlayerId, fb: &mut Fb) -> Result<(), CoreError> {
        let keys: Vec<Key> = self
            .seats
            .get(&player)
            .map(|s| s.ctx.stale.iter().copied().collect())
            .unwrap_or_default();
        for key in keys {
            self.ctx_compute(player, key, fb)?;
        }
        Ok(())
    }

    /// Computes the context of `key`, compares it with the last one sent and puts `Unit`, `City`, `Valid` or
    /// `Dropped` into the frame.
    fn ctx_compute(&mut self, player: PlayerId, key: Key, fb: &mut Fb) -> Result<(), CoreError> {
        let t = Instant::now();
        let built = self.build_context(player, key)?;
        self.times.ctx_us += micros(t);
        let Some(seat) = self.seats.get_mut(&player) else {
            return Ok(());
        };
        let rev = seat.rev;
        seat.ctx.stale.remove(&key);
        let before = seat.ctx.sent.get(&key).cloned();
        let same = |hash: u64| {
            before
                .as_ref()
                .is_some_and(|b| !b.dropped && b.hash == hash)
        };
        match built {
            Built::Unit(ctx, deps) => {
                let msg = ContextMsg::Unit(ctx);
                let hash = hash_of(&msg);
                if same(hash) {
                    fb.valid(player, rev, key);
                } else {
                    fb.context(player, rev, msg);
                }
                seat.ctx.sent.insert(
                    key,
                    Sent {
                        hash,
                        deps,
                        dropped: false,
                    },
                );
            }
            Built::City(ctx, deps) => {
                let msg = ContextMsg::City(ctx);
                let hash = hash_of(&msg);
                if same(hash) {
                    fb.valid(player, rev, key);
                } else {
                    fb.context(player, rev, msg);
                }
                seat.ctx.sent.insert(
                    key,
                    Sent {
                        hash,
                        deps,
                        dropped: false,
                    },
                );
            }
            Built::DroppedUnit(unit, deps) => {
                // A seat that has been told once does not need to hear it again.
                if !before.as_ref().is_some_and(|b| b.dropped) {
                    fb.context(player, rev, ContextMsg::Dropped { unit });
                }
                seat.ctx.sent.insert(
                    key,
                    Sent {
                        hash: 0,
                        deps,
                        dropped: true,
                    },
                );
            }
            Built::DroppedCity(deps) => {
                seat.ctx.sent.insert(
                    key,
                    Sent {
                        hash: 0,
                        deps,
                        dropped: true,
                    },
                );
            }
        }
        Ok(())
    }

    /// Asks the core for the context of `key` (side-effect free queries only).
    fn build_context(&mut self, player: PlayerId, key: Key) -> Result<Built, CoreError> {
        let map = self.map;
        let Some(view) = self
            .seats
            .get(&player)
            .and_then(|s| s.mirror.as_ref())
            .map(|m| &m.view)
        else {
            return Ok(match key {
                Key::Unit(u) => Built::DroppedUnit(u, Deps::default()),
                Key::City(_) => Built::DroppedCity(Deps::default()),
            });
        };
        let city_cells: BTreeSet<(i16, i16)> = view.cities.iter().map(|c| (c.x, c.y)).collect();
        let mut deps = Deps::default();
        match key {
            Key::Unit(id) => {
                let Some(u) = view.units.iter().find(|u| u.owner == player && u.id == id) else {
                    return Ok(Built::DroppedUnit(id, deps));
                };
                deps.add_around(&map, u.x, u.y);
                if !u.flags.contains(UnitFlags::CAN_MOVE) {
                    return Ok(Built::DroppedUnit(id, deps));
                }
                let core = &mut self.core;
                let shot = Shooter::Unit(id);
                // Any failed query: the unit cannot be given a context.
                let (Some(actions), Some(reach), Some(ranged)) = (
                    soft(core.unit_actions(player, id))?,
                    soft(core.reach(player, id, 1))?,
                    soft(core.targets(player, shot))?,
                ) else {
                    return Ok(Built::DroppedUnit(id, deps));
                };
                let mut targets = Vec::new();
                for &(x, y, flags) in &ranged {
                    let Some(preview) = soft(core.preview(player, shot, x, y))? else {
                        return Ok(Built::DroppedUnit(id, deps));
                    };
                    deps.add_around(&map, x, y);
                    targets.push(TargetView {
                        x,
                        y,
                        kind: TargetKind::Ranged,
                        flags,
                        preview,
                    });
                }
                for node in &reach {
                    deps.add_around(&map, node.x, node.y);
                }
                for node in reach.iter().skip(1) {
                    if !node.flags.contains(ReachFlags::ATTACK) {
                        continue;
                    }
                    let Some(preview) = soft(core.preview(player, shot, node.x, node.y))? else {
                        return Ok(Built::DroppedUnit(id, deps));
                    };
                    let city = node.flags.contains(ReachFlags::CITY)
                        || city_cells.contains(&(node.x, node.y));
                    targets.push(TargetView {
                        x: node.x,
                        y: node.y,
                        kind: TargetKind::Melee,
                        flags: if city { TargetView::CITY } else { 0 },
                        preview,
                    });
                }
                Ok(Built::Unit(
                    UnitContext {
                        unit: id,
                        view_seq: 0,
                        actions,
                        reach,
                        targets,
                    },
                    deps,
                ))
            }
            Key::City(id) => {
                let Some(c) = view.cities.iter().find(|c| c.owner == player && c.id == id) else {
                    return Ok(Built::DroppedCity(deps));
                };
                deps.add_around(&map, c.x, c.y);
                deps.top_bar = true;
                let core = &mut self.core;
                let shot = Shooter::City(id);
                let (Some(builds), Some(ranged)) = (
                    soft(core.city_builds(player, id))?,
                    soft(core.targets(player, shot))?,
                ) else {
                    return Ok(Built::DroppedCity(deps));
                };
                let mut targets = Vec::new();
                for &(x, y, flags) in &ranged {
                    let Some(preview) = soft(core.preview(player, shot, x, y))? else {
                        return Ok(Built::DroppedCity(deps));
                    };
                    deps.add_around(&map, x, y);
                    targets.push(TargetView {
                        x,
                        y,
                        kind: TargetKind::Ranged,
                        flags,
                        preview,
                    });
                }
                Ok(Built::City(
                    CityContext {
                        city: id,
                        view_seq: 0,
                        builds,
                        targets,
                    },
                    deps,
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use protocol::{PlayerView, SectionDiff, UnitView, ViewHeader};

    use super::*;

    fn map() -> MapInfo {
        MapInfo {
            width: 10,
            height: 6,
            wrap_x: true,
            ..MapInfo::default()
        }
    }

    fn unit(owner: i8, id: i32, x: i16, y: i16) -> UnitView {
        UnitView {
            owner,
            id,
            x,
            y,
            ..UnitView::default()
        }
    }

    fn view_with(units: Vec<UnitView>) -> WorldView {
        WorldView {
            turn: 3,
            year: 0,
            human: 0,
            human_team: 0,
            status: protocol::TurnStatus::HumanTurn,
            blocker: None,
            map: protocol::MapView::default(),
            units,
            cities: Vec::new(),
            players: Vec::new(),
            top_bar: protocol::TopBar::default(),
            notifications: Vec::new(),
            peace_offers: Vec::new(),
            trade_routes: Vec::new(),
        }
    }

    fn sent_around(x: i16, y: i16) -> Sent {
        let mut deps = Deps::default();
        deps.add_around(&map(), x, y);
        Sent {
            hash: 1,
            deps,
            dropped: false,
        }
    }

    #[test]
    fn the_box_around_a_plot_wraps_in_x_only() {
        let mut d = Deps::default();
        d.add_around(&map(), 0, 0);
        assert!(d.cells.contains(&(9, 0)) && d.cells.contains(&(9, 1)));
        assert!(!d.cells.iter().any(|&(_, y)| y < 0));
        assert_eq!(d.cells.len(), 6);
    }

    #[test]
    fn a_move_far_away_does_not_touch_a_context_but_a_move_near_it_does() {
        let old = view_with(vec![unit(0, 1, 2, 2), unit(0, 2, 7, 4)]);
        let moved_far = ViewDiff {
            units: SectionDiff {
                upsert: vec![unit(0, 2, 7, 3)],
                remove: vec![],
            },
            ..ViewDiff::default()
        };
        let sent = sent_around(2, 2);
        let t = Touch::of(&moved_far, &old, &map());
        assert!(!sent.touched_by(0, Key::Unit(1), &t));
        let moved_near = ViewDiff {
            units: SectionDiff {
                upsert: vec![unit(0, 2, 3, 3)],
                remove: vec![],
            },
            ..ViewDiff::default()
        };
        let t = Touch::of(&moved_near, &old, &map());
        assert!(sent.touched_by(0, Key::Unit(1), &t));
        // The old place of a unit that leaves counts too.
        let left = ViewDiff {
            units: SectionDiff {
                upsert: vec![unit(0, 2, 7, 4)],
                remove: vec![],
            },
            ..ViewDiff::default()
        };
        let old_near = view_with(vec![unit(0, 2, 3, 2)]);
        let t = Touch::of(&left, &old_near, &map());
        assert!(sent.touched_by(0, Key::Unit(1), &t));
    }

    #[test]
    fn a_new_turn_the_players_and_the_unit_itself_touch_everything() {
        let old = view_with(vec![]);
        let sent = sent_around(2, 2);
        let new_turn = ViewDiff {
            header: Some(ViewHeader {
                turn: 4,
                year: 0,
                human: 0,
                human_team: 0,
                status: protocol::TurnStatus::HumanTurn,
                blocker: None,
            }),
            ..ViewDiff::default()
        };
        assert!(sent.touched_by(0, Key::Unit(1), &Touch::of(&new_turn, &old, &map())));
        let same_turn = ViewDiff {
            header: Some(ViewHeader {
                turn: 3,
                ..new_turn.header.expect("a header")
            }),
            ..ViewDiff::default()
        };
        assert!(!sent.touched_by(0, Key::Unit(1), &Touch::of(&same_turn, &old, &map())));
        let players = ViewDiff {
            players: SectionDiff {
                upsert: vec![PlayerView::default()],
                remove: vec![],
            },
            ..ViewDiff::default()
        };
        assert!(sent.touched_by(0, Key::Unit(1), &Touch::of(&players, &old, &map())));
        let own = ViewDiff {
            units: SectionDiff {
                upsert: vec![unit(0, 1, 9, 5)],
                remove: vec![],
            },
            ..ViewDiff::default()
        };
        assert!(sent.touched_by(0, Key::Unit(1), &Touch::of(&own, &old, &map())));
        assert!(!sent.touched_by(0, Key::Unit(1), &Touch::none()));
        assert!(sent.touched_by(0, Key::Unit(1), &Touch::all()));
    }

    #[test]
    fn a_city_context_depends_on_the_top_bar_a_unit_context_does_not() {
        let old = view_with(vec![]);
        let top = ViewDiff {
            top_bar: Some(protocol::TopBar::default()),
            ..ViewDiff::default()
        };
        let t = Touch::of(&top, &old, &map());
        let mut city = sent_around(2, 2);
        city.deps.top_bar = true;
        assert!(city.touched_by(0, Key::City(1), &t));
        assert!(!sent_around(2, 2).touched_by(0, Key::Unit(1), &t));
    }

    #[test]
    fn a_focus_holds_four_distinct_units() {
        let f = normalized(Focus {
            units: vec![5, 5, 6, 7, 8, 9, 10],
            city: Some(2),
        });
        assert_eq!(f.units, vec![5, 6, 7, 8]);
        assert_eq!(f.city, Some(2));
    }
}
