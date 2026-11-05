//! Fog auditor (game-host-mp GH-D15, 4.17, `WorkerConfig.audit_fog`): every update to a seat is checked
//! against the truth of the core for the seat's team.
//!
//! - A foreign unit stands on a plot in sight of the team and is in `Core::units(team)`.
//! - A foreign city stands on a revealed plot. In sight, its record is the core's (and the auditor writes it
//!   down); out of sight, its values (population, hp, max hp, capital/razing flags, buildings) must be the
//!   last ones the auditor wrote down from a view it saw sent (its own copy, not the memory of the worker),
//!   or zero if the city was never in sight. A city the core no longer has may stay in the view while its
//!   plot is out of sight; on a plot in sight it may not.
//! - A journal line with a cell is about a plot in sight or one where the seat has a unit or a city; a
//!   notification of the core may also name a plot the team has revealed (the original's rule).
//!
//! The white list is the map basis: the `MapBasis` message, the `basis` part of a diff and
//! `MapView::terrain_seed` are not checked (NP-D19, NP-D22). There is no combat readiness flag in B2.

use std::collections::{BTreeMap, BTreeSet};

use protocol::{
    CityKey, JournalEntry, PlayerId, PlotFlags, PlotView, Shooter, TeamId, UnitView, WorldView,
};

use crate::core::{Core, MapInfo};
use crate::seats::CityMemo;
use crate::worker::{Fb, Worker};

/// State of the auditor: what it has seen sent, by team.
#[derive(Debug, Default)]
pub(crate) struct Auditor {
    seen: BTreeMap<(TeamId, CityKey), CityMemo>,
    /// The plots where the team had a unit or a city of its own in the view audited before: a line about a
    /// unit that is gone now (captured, killed) names such a plot.
    own_before: BTreeMap<TeamId, BTreeSet<(i16, i16)>>,
    /// State of the generator of the query noise (`WorkerConfig.query_noise`).
    noise: u64,
}

impl Auditor {
    /// A pseudo-random number (xorshift64*). The noise has no seed of the game: it must not matter.
    fn rand(&mut self) -> u64 {
        if self.noise == 0 {
            self.noise = 0x9e37_79b9_7f4a_7c15;
        }
        let mut x = self.noise;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.noise = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, n: usize) -> usize {
        usize::try_from(self.rand() % (n.max(1) as u64)).unwrap_or(0)
    }
}

/// What the auditor remembers of earlier rounds, for one check.
pub(crate) struct Memory<'a> {
    /// Foreign cities as sent to the team (`Auditor::seen`).
    pub seen: &'a mut BTreeMap<(TeamId, CityKey), CityMemo>,
    /// Plots where the team had a unit or a city of its own in the view audited before.
    pub own_before: &'a BTreeSet<(i16, i16)>,
}

/// What the core says for one team.
pub(crate) struct Truth {
    pub units: Vec<UnitView>,
    pub cities: Vec<protocol::CityView>,
    /// The flags of the plots the check needs, by index.
    pub plots: BTreeMap<u32, PlotFlags>,
}

fn index_of(map: &MapInfo, x: i16, y: i16) -> Option<u32> {
    let (x, y) = (u32::try_from(x).ok()?, u32::try_from(y).ok()?);
    (x < u32::from(map.width) && y < u32::from(map.height)).then(|| y * u32::from(map.width) + x)
}

/// The plots the check of `view` and `journal` asks the core about.
fn needed_plots(
    map: &MapInfo,
    view: &WorldView,
    own: &[PlayerId],
    journal: &[JournalEntry],
) -> Vec<u32> {
    let mut idx = BTreeSet::new();
    for u in view.units.iter().filter(|u| !own.contains(&u.owner)) {
        idx.extend(index_of(map, u.x, u.y));
    }
    for c in view.cities.iter().filter(|c| !own.contains(&c.owner)) {
        idx.extend(index_of(map, c.x, c.y));
    }
    for e in journal {
        if let Some((x, y)) = e.at {
            idx.extend(index_of(map, x, y));
        }
    }
    idx.into_iter().collect()
}

/// Checks one update of a seat; returns what is wrong, one text per violation. `seen` is the auditor's own
/// record of foreign cities (`team` keys it).
pub(crate) fn audit_view(
    map: &MapInfo,
    team: TeamId,
    view: &WorldView,
    own: &[PlayerId],
    truth: &Truth,
    journal: &[JournalEntry],
    memory: Memory<'_>,
) -> Vec<String> {
    let Memory { seen, own_before } = memory;
    let mut bad = Vec::new();
    let flags = |x: i16, y: i16| -> Option<PlotFlags> {
        index_of(map, x, y).and_then(|i| truth.plots.get(&i).copied())
    };
    for u in view.units.iter().filter(|u| !own.contains(&u.owner)) {
        let seen_now = flags(u.x, u.y).is_some_and(|f| f.contains(PlotFlags::VISIBLE));
        if !seen_now {
            bad.push(format!(
                "unit {}/{} at ({}, {}) is not in sight of team {team}",
                u.owner, u.id, u.x, u.y
            ));
        } else if !truth
            .units
            .iter()
            .any(|t| (t.owner, t.id) == (u.owner, u.id))
        {
            bad.push(format!(
                "unit {}/{} is in the view but not in the units of team {team}",
                u.owner, u.id
            ));
        }
    }
    for c in view.cities.iter().filter(|c| !own.contains(&c.owner)) {
        let key = (c.owner, c.id);
        let f = flags(c.x, c.y);
        if !f.is_some_and(|f| f.contains(PlotFlags::REVEALED)) {
            bad.push(format!(
                "city {}/{} stands on a plot team {team} has not revealed",
                c.owner, c.id
            ));
            continue;
        }
        if f.is_some_and(|f| f.contains(PlotFlags::VISIBLE)) {
            match truth.cities.iter().find(|t| (t.owner, t.id) == key) {
                None => bad.push(format!(
                    "city {}/{} is in the view, its plot is in sight and the core has no such city",
                    c.owner, c.id
                )),
                Some(t) if CityMemo::of(t) != CityMemo::of(c) => bad.push(format!(
                    "city {}/{} in sight differs from the core",
                    c.owner, c.id
                )),
                Some(_) => {}
            }
            seen.insert((team, key), CityMemo::of(c));
        } else {
            let expected = seen.get(&(team, key)).cloned().unwrap_or(CityMemo {
                owner: c.owner,
                id: c.id,
                ..CityMemo::default()
            });
            if CityMemo::of(c) != expected {
                bad.push(format!(
                    "city {}/{} out of sight shows values other than the last seen",
                    c.owner, c.id
                ));
            }
        }
    }
    for e in journal {
        let Some((x, y)) = e.at else {
            continue;
        };
        let f = flags(x, y);
        let in_sight = f.is_some_and(|f| f.contains(PlotFlags::VISIBLE));
        // A notification of the core may name a plot the team has revealed but does not see now: the
        // original adds "barbarian encampment found" when the team's revealed improvement of a plot becomes
        // the camp (`CvPlot::setRevealedImprovementType`), and that happens out of sight too (a ruin that
        // shows the camps nearby, a city-state quest, the Honor opener, a plot seen half-way along a move).
        // A plot the team has not revealed is never named.
        let known = e.kind == protocol::JournalKind::Notification
            && f.is_some_and(|f| f.contains(PlotFlags::REVEALED));
        let own_there = view
            .units
            .iter()
            .any(|u| own.contains(&u.owner) && (u.x, u.y) == (x, y))
            || view
                .cities
                .iter()
                .any(|c| own.contains(&c.owner) && (c.x, c.y) == (x, y));
        if !in_sight && !known && !own_there && !own_before.contains(&(x, y)) {
            bad.push(format!(
                "journal line \"{}\" names ({x}, {y}), out of sight",
                e.text
            ));
        }
    }
    bad
}

impl<C: Core> Worker<C> {
    /// Audits the updates of this round (`audit_fog`): the seats that got a view or journal lines.
    pub(crate) fn audit_round(&mut self, fb: &Fb) {
        if !self.cfg.audit_fog {
            return;
        }
        let players: BTreeSet<PlayerId> =
            fb.views.keys().chain(fb.journal.keys()).copied().collect();
        let mut found = 0u32;
        for p in players {
            let journal = fb.journal.get(&p).map_or(&[][..], Vec::as_slice);
            found += self.audit_seat(p, journal);
        }
        self.times.fog_violations += found;
        self.fog_violations += u64::from(found);
    }

    fn audit_seat(&mut self, player: PlayerId, journal: &[JournalEntry]) -> u32 {
        let Some(seat) = self.seats.get(&player) else {
            return 0;
        };
        let (team, Some(mirror)) = (seat.team, seat.mirror.as_ref()) else {
            return 0;
        };
        let own: Vec<PlayerId> = mirror
            .view
            .players
            .iter()
            .filter(|p| p.team == team)
            .map(|p| p.id)
            .chain([player])
            .collect();
        let want = needed_plots(&self.map, &mirror.view, &own, journal);
        let mut plots = Vec::new();
        let truth = (|| {
            if !want.is_empty() {
                self.core.plots(team, Some(&want), &mut plots).ok()?;
            }
            Some(Truth {
                units: self.core.units(team).ok()?,
                cities: self.core.cities(team).ok()?,
                plots: plots
                    .into_iter()
                    .map(|(i, p): (u32, PlotView)| (i, p.flags))
                    .collect(),
            })
        })();
        let Some(truth) = truth else {
            tracing::warn!("fog audit of seat {player}: the core did not answer");
            return 0;
        };
        let before = self.auditor.own_before.remove(&team).unwrap_or_default();
        let bad = audit_view(
            &self.map,
            team,
            &mirror.view,
            &own,
            &truth,
            journal,
            Memory {
                seen: &mut self.auditor.seen,
                own_before: &before,
            },
        );
        let now: BTreeSet<(i16, i16)> = mirror
            .view
            .units
            .iter()
            .filter(|u| own.contains(&u.owner))
            .map(|u| (u.x, u.y))
            .chain(
                mirror
                    .view
                    .cities
                    .iter()
                    .filter(|c| own.contains(&c.owner))
                    .map(|c| (c.x, c.y)),
            )
            .collect();
        self.auditor.own_before.insert(team, now);
        for b in &bad {
            tracing::warn!(seat = player, "fog violation: {b}");
        }
        u32::try_from(bad.len()).unwrap_or(u32::MAX)
    }
}

impl<C: Core> Worker<C> {
    /// Query noise (`WorkerConfig.query_noise`, game-host-mp 4.17, AC-N1.4): `query_noise` random read-only
    /// questions to the core for the units and cities of the seats (actions, move trees, paths, targets,
    /// forecasts, build lists), as the contexts of the clients would ask. The answers are thrown away; the
    /// point is that the state of the game, and so its digest, must not depend on them.
    pub(crate) fn query_noise(&mut self) {
        let n = self.cfg.query_noise;
        if n == 0 || self.seats.is_empty() {
            return;
        }
        let seats: Vec<(PlayerId, TeamId)> = self.seats.iter().map(|(p, s)| (*p, s.team)).collect();
        let (w, h) = (
            i32::from(self.map.width.max(1)),
            i32::from(self.map.height.max(1)),
        );
        for _ in 0..n {
            let (player, team) = seats[self.auditor.below(seats.len())];
            let units: Vec<UnitView> = self
                .core
                .units(team)
                .unwrap_or_default()
                .into_iter()
                .filter(|u| u.owner == player)
                .collect();
            let cities: Vec<protocol::CityView> = self
                .core
                .cities(team)
                .unwrap_or_default()
                .into_iter()
                .filter(|c| c.owner == player)
                .collect();
            let a = &mut self.auditor;
            let (kind, ui, ci) = (a.below(7), a.below(units.len()), a.below(cities.len()));
            let (dx, dy) = (a.below(9) as i32 - 4, a.below(9) as i32 - 4);
            let unit = units.get(ui);
            // Errors are not the point here.
            let _ = match (kind, unit, cities.get(ci)) {
                (0, Some(u), _) => self.core.unit_actions(player, u.id).map(drop),
                (1, Some(u), _) => self.core.reach(player, u.id, 1).map(drop),
                (2, Some(u), _) => {
                    let x = (i32::from(u.x) + dx).clamp(0, w - 1);
                    let y = (i32::from(u.y) + dy).clamp(0, h - 1);
                    self.core.path(player, u.id, x as i16, y as i16).map(drop)
                }
                (3, Some(u), _) => self.core.targets(player, Shooter::Unit(u.id)).map(drop),
                (4, Some(u), _) => {
                    let x = (i32::from(u.x) + dx).clamp(0, w - 1);
                    let y = (i32::from(u.y) + dy).clamp(0, h - 1);
                    self.core
                        .preview(player, Shooter::Unit(u.id), x as i16, y as i16)
                        .map(drop)
                }
                (5, _, Some(c)) => self.core.city_builds(player, c.id).map(drop),
                _ => self.core.top_bar(player).map(drop),
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use protocol::{CityFlags, CityView, MapView, PlayerView, TurnStatus};

    use super::*;

    fn map() -> MapInfo {
        MapInfo {
            width: 8,
            height: 2,
            ..MapInfo::default()
        }
    }

    fn view() -> WorldView {
        WorldView {
            turn: 1,
            year: 0,
            human: 0,
            human_team: 0,
            status: TurnStatus::HumanTurn,
            blocker: None,
            map: MapView::default(),
            units: Vec::new(),
            cities: Vec::new(),
            players: vec![PlayerView {
                id: 0,
                team: 0,
                ..PlayerView::default()
            }],
            top_bar: protocol::TopBar::default(),
            notifications: Vec::new(),
            peace_offers: Vec::new(),
            trade_routes: Vec::new(),
        }
    }

    fn truth(plots: &[(u32, u16)]) -> Truth {
        Truth {
            units: Vec::new(),
            cities: Vec::new(),
            plots: plots.iter().map(|&(i, f)| (i, PlotFlags(f))).collect(),
        }
    }

    const REV: u16 = PlotFlags::REVEALED;
    const SEEN: u16 = PlotFlags::REVEALED | PlotFlags::VISIBLE;

    fn city(x: i16, pop: i16, buildings: &[i16]) -> CityView {
        CityView {
            id: 4,
            owner: 1,
            x,
            y: 0,
            name: "c".into(),
            population: pop,
            hp: 10,
            max_hp: 20,
            flags: CityFlags(CityFlags::CAPITAL),
            buildings: buildings.to_vec(),
            ..CityView::default()
        }
    }

    fn audit(
        v: &WorldView,
        t: &Truth,
        j: &[JournalEntry],
        seen: &mut BTreeMap<(TeamId, CityKey), CityMemo>,
    ) -> Vec<String> {
        let own_before = BTreeSet::new();
        let memory = Memory {
            seen,
            own_before: &own_before,
        };
        audit_view(&map(), 0, v, &[0], t, j, memory)
    }

    fn unit(owner: i8, id: i32, x: i16) -> UnitView {
        UnitView {
            id,
            owner,
            x,
            y: 0,
            ..UnitView::default()
        }
    }

    #[test]
    fn a_foreign_unit_must_stand_in_sight_and_be_known_to_the_core() {
        let mut v = view();
        v.units = vec![unit(0, 1, 0), unit(1, 2, 3)];
        let mut t = truth(&[(3, SEEN)]);
        t.units = vec![unit(1, 2, 3)];
        let mut seen = BTreeMap::new();
        assert!(
            audit(&v, &t, &[], &mut seen).is_empty(),
            "own units need nothing"
        );
        // In sight in the core, but the core does not list it: stale.
        t.units.clear();
        assert_eq!(audit(&v, &t, &[], &mut seen).len(), 1);
        // Listed, but the plot is out of sight: a leak.
        t.units = vec![unit(1, 2, 3)];
        t.plots.insert(3, PlotFlags(REV));
        assert_eq!(audit(&v, &t, &[], &mut seen).len(), 1);
    }

    #[test]
    fn a_city_out_of_sight_must_show_the_last_seen_values() {
        let mut v = view();
        let mut seen = BTreeMap::new();
        // In sight: the core's record, written down.
        v.cities = vec![city(2, 5, &[1])];
        let mut t = truth(&[(2, SEEN)]);
        t.cities = vec![city(2, 5, &[1])];
        assert!(audit(&v, &t, &[], &mut seen).is_empty());
        // Out of sight: the same values pass, even when the core has built on.
        t.plots.insert(2, PlotFlags(REV));
        t.cities = vec![city(2, 6, &[1, 2])];
        assert!(audit(&v, &t, &[], &mut seen).is_empty());
        // The live values of the core in the view are a leak (the population, then a building).
        v.cities = vec![city(2, 6, &[1])];
        assert_eq!(audit(&v, &t, &[], &mut seen).len(), 1);
        v.cities = vec![city(2, 5, &[1, 2])];
        assert_eq!(audit(&v, &t, &[], &mut seen).len(), 1);
    }

    #[test]
    fn a_city_never_seen_shows_zeros_and_a_gone_city_may_stay_only_under_fog() {
        let mut v = view();
        let mut seen = BTreeMap::new();
        let dark = CityView {
            id: 4,
            owner: 1,
            x: 2,
            y: 0,
            name: "c".into(),
            ..CityView::default()
        };
        v.cities = vec![dark];
        let t = truth(&[(2, REV)]);
        assert!(audit(&v, &t, &[], &mut seen).is_empty());
        v.cities = vec![city(2, 3, &[])];
        assert_eq!(
            audit(&v, &t, &[], &mut seen).len(),
            1,
            "live values of a city never seen"
        );
        // The core has no city and the plot is in sight: wrong. Not revealed: wrong.
        let t = truth(&[(2, SEEN)]);
        assert_eq!(audit(&v, &t, &[], &mut seen).len(), 1);
        let t = truth(&[(2, 0)]);
        assert_eq!(audit(&v, &t, &[], &mut seen).len(), 1);
    }

    #[test]
    fn a_journal_cell_must_be_in_sight_or_the_seat_s_own() {
        let mut v = view();
        v.units = vec![unit(0, 1, 5)];
        let t = truth(&[(2, SEEN), (3, REV), (5, REV)]);
        let line = |x| JournalEntry {
            turn: 1,
            kind: protocol::JournalKind::Combat,
            text: "fight".into(),
            at: Some((x, 0)),
            simplification: None,
        };
        let mut seen = BTreeMap::new();
        assert!(audit(&v, &t, &[line(2), line(5)], &mut seen).is_empty());
        assert_eq!(audit(&v, &t, &[line(3)], &mut seen).len(), 1);
    }

    #[test]
    fn a_notification_may_name_a_revealed_plot_out_of_sight_but_not_a_dark_one() {
        let v = view();
        let t = truth(&[(3, REV), (4, 0)]);
        let line = |kind, x| JournalEntry {
            turn: 5,
            kind,
            text: "A Barbarian Encampment has been discovered!".into(),
            at: Some((x, 0)),
            simplification: None,
        };
        let mut seen = BTreeMap::new();
        let n = protocol::JournalKind::Notification;
        assert!(audit(&v, &t, &[line(n, 3)], &mut seen).is_empty());
        assert_eq!(audit(&v, &t, &[line(n, 4)], &mut seen).len(), 1);
        // Other lines keep the rule of sight.
        let info = protocol::JournalKind::Info;
        assert_eq!(audit(&v, &t, &[line(info, 3)], &mut seen).len(), 1);
    }

    #[test]
    fn a_journal_cell_where_the_seat_had_a_unit_before_is_its_own() {
        // The unit was captured: it is not in the view any more, the line names the plot it stood on.
        let v = view();
        let t = truth(&[(3, REV)]);
        let line = JournalEntry {
            turn: 1,
            kind: protocol::JournalKind::Info,
            text: "captured".into(),
            at: Some((3, 0)),
            simplification: None,
        };
        let mut seen = BTreeMap::new();
        let before: BTreeSet<(i16, i16)> = [(3, 0)].into();
        let memory = Memory {
            seen: &mut seen,
            own_before: &before,
        };
        let bad = audit_view(&map(), 0, &v, &[0], &t, std::slice::from_ref(&line), memory);
        assert!(bad.is_empty(), "{bad:?}");
        assert_eq!(audit(&v, &t, &[line], &mut seen).len(), 1);
    }
}
