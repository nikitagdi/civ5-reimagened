//! Seats of the worker and the data of their views (game-host-mp 4.3): a seat has a view revision, the last
//! view it was sent (the mirror of the gateway) and the hash of it; the data every team shares is built once
//! per team. The plots come from the dirty list of the core (GH-R7); units, cities and players are asked in
//! full and sorted by key.

use std::collections::BTreeMap;

use protocol::{
    CityKey, CmdId, JournalEntry, MapView, NotificationView, PeaceOffer, PlayerId, PlayerView,
    PlotView, TeamId, TopBar, ViewHash, ViewHeader, WorldView,
};
use serde::{Deserialize, Serialize};

use crate::context::CtxState;
use crate::core::{Core, CoreError, MapInfo};

/// The last view a seat was sent and its hash; the diff of the next one is computed against it.
pub(crate) struct Mirror {
    pub view: WorldView,
    pub hash: ViewHash,
}

/// A human seat.
pub(crate) struct Seat {
    /// Set at the first view (`ViewHeader::human_team`).
    pub team: TeamId,
    pub nick: String,
    /// `None` until the first `Full`.
    pub mirror: Option<Mirror>,
    /// Revision of the last view sent to the seat; grows by 1 with every view.
    pub rev: u32,
    /// The seat has ended its turn (last `Core::turn`).
    pub finished: bool,
    /// The last order of the seat the core took (game-host-mp 4.11, `CMDS`).
    pub applied_upto: Option<CmdId>,
    /// The newest journal lines of the seat (`journal::KEPT`), for the save.
    pub journal: Vec<JournalEntry>,
    /// Peace offers the seat takes part in (the worker keeps them, GH-D13).
    pub peace_offers: Vec<PeaceOffer>,
    /// The focus of the seat and the contexts sent for it (game-host-mp 4.6).
    pub ctx: CtxState,
}

impl Seat {
    pub fn new(nick: String) -> Self {
        Seat {
            team: -1,
            nick,
            mirror: None,
            rev: 0,
            finished: false,
            applied_upto: None,
            journal: Vec::new(),
            peace_offers: Vec::new(),
            ctx: CtxState::default(),
        }
    }
}

/// What the worker remembers of a foreign city that was seen (game-host-mp 4.3 item 6, NP 4.15): the values
/// the viewer's team last saw. Shown while the plot is revealed but not in sight.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CityMemo {
    pub owner: PlayerId,
    pub id: i32,
    pub population: i16,
    pub hp: i16,
    pub max_hp: i16,
    /// `CityFlags::CAPITAL` and `RAZING` only.
    pub flags: u16,
    pub buildings: Vec<i16>,
}

/// The flags of a city the memo keeps.
pub(crate) const MEMO_FLAGS: u16 = protocol::CityFlags::CAPITAL | protocol::CityFlags::RAZING;

impl CityMemo {
    pub(crate) fn of(c: &protocol::CityView) -> Self {
        CityMemo {
            owner: c.owner,
            id: c.id,
            population: c.population,
            hp: c.hp,
            max_hp: c.max_hp,
            flags: c.flags.0 & MEMO_FLAGS,
            buildings: c.buildings.clone(),
        }
    }

    /// `c` (name and position from the core) with the remembered values.
    fn overlay(&self, c: &mut protocol::CityView) {
        c.population = self.population;
        c.hp = self.hp;
        c.max_hp = self.max_hp;
        c.flags = protocol::CityFlags(c.flags.0 & !MEMO_FLAGS | self.flags);
        c.buildings.clone_from(&self.buildings);
    }
}

/// The parts of a view that every seat of a team shares.
#[derive(Default, Clone)]
pub(crate) struct TeamData {
    pub team: TeamId,
    /// Dense, index `y * width + x`.
    pub plots: Vec<PlotView>,
    pub units: Vec<protocol::UnitView>,
    /// As in the view: foreign cities under fog from the memo.
    pub cities: Vec<protocol::CityView>,
    pub players: Vec<PlayerView>,
    /// Sorted by owner, then the ends.
    pub trade_routes: Vec<protocol::TradeRouteView>,
    pub memo: BTreeMap<CityKey, CityMemo>,
}

/// The routes in the order of the view: by owner, then the ends (the core's order is not stable).
pub(crate) fn sorted_routes(
    mut routes: Vec<protocol::TradeRouteView>,
) -> Vec<protocol::TradeRouteView> {
    routes.sort_by_key(|r| (r.owner, r.from, r.to, r.domain));
    routes
}

fn plot_index(i: u32, n: usize) -> Result<usize, CoreError> {
    let i = i as usize;
    if i < n {
        Ok(i)
    } else {
        Err(CoreError::failed(format!(
            "core: plot {i} is outside the map ({n} plots)"
        )))
    }
}

impl TeamData {
    /// Brings the team data up to date. `dirty`: the plots that changed since the previous round (all plots
    /// when the data is new or the core does not know).
    pub fn refresh<C: Core>(
        &mut self,
        core: &mut C,
        team: TeamId,
        map: &MapInfo,
        dirty: Option<&[u32]>,
    ) -> Result<(), CoreError> {
        self.refresh_with(core, team, map, dirty, true)
    }

    /// A copy that keeps the memo and the last cities, for building one view from scratch.
    pub fn reading_copy(&self) -> TeamData {
        TeamData {
            plots: Vec::new(),
            units: Vec::new(),
            players: Vec::new(),
            trade_routes: Vec::new(),
            ..self.clone()
        }
    }

    /// Asks everything again and leaves the memo as it is (a view for a test).
    pub fn reload<C: Core>(&mut self, core: &mut C, map: &MapInfo) -> Result<(), CoreError> {
        let team = self.team;
        self.refresh_with(core, team, map, None, false)
    }

    fn refresh_with<C: Core>(
        &mut self,
        core: &mut C,
        team: TeamId,
        map: &MapInfo,
        dirty: Option<&[u32]>,
        remember: bool,
    ) -> Result<(), CoreError> {
        let n = map.plot_count();
        let mut out = Vec::new();
        match dirty {
            Some(list) if self.plots.len() == n => {
                if !list.is_empty() {
                    core.plots(team, Some(list), &mut out)?;
                }
            }
            _ => {
                self.plots.clear();
                self.plots.resize(n, PlotView::default());
                core.plots(team, None, &mut out)?;
            }
        }
        for (i, rec) in out {
            let i = plot_index(i, n)?;
            self.plots[i] = rec;
        }
        let mut units = core.units(team)?;
        units.sort_by_key(|u| (u.owner, u.id));
        let mut players = core.players(team)?;
        players.sort_by_key(|p| p.id);
        let mut live = core.cities(team)?;
        live.sort_by_key(|c| (c.owner, c.id));
        let routes = sorted_routes(core.trade_routes(team)?);
        self.units = units;
        self.players = players;
        self.trade_routes = routes;
        self.cities = self.cities_with_memo(live, map, remember);
        Ok(())
    }

    /// The cities of the view. A city on a plot in sight is the core's record (and is remembered when it is
    /// foreign); a foreign city on a revealed plot out of sight carries the remembered values, or the core's
    /// name-and-position-only record when it was never seen; a remembered city the core no longer lists stays
    /// while its plot is out of sight and is forgotten once the plot is seen without it. `remember`: update
    /// the memo (a fresh view for a test reads it only).
    pub fn cities_with_memo(
        &mut self,
        live: Vec<protocol::CityView>,
        map: &MapInfo,
        remember: bool,
    ) -> Vec<protocol::CityView> {
        let width = usize::from(map.width);
        let plot_of = |c: &protocol::CityView| -> Option<&PlotView> {
            let x = usize::try_from(c.x).ok()?;
            let y = usize::try_from(c.y).ok()?;
            self.plots.get(y * width + x)
        };
        let visible = |c: &protocol::CityView| {
            plot_of(c).is_some_and(|p| p.flags.contains(protocol::PlotFlags::VISIBLE))
        };
        let own: Vec<PlayerId> = self
            .players
            .iter()
            .filter(|p| p.team == self.team)
            .map(|p| p.id)
            .collect();
        let mut out = Vec::with_capacity(live.len());
        let mut seen_now: Vec<CityKey> = Vec::new();
        let mut remembered: Vec<CityMemo> = Vec::new();
        for mut c in live {
            let key = (c.owner, c.id);
            seen_now.push(key);
            if own.contains(&c.owner) {
                out.push(c);
            } else if visible(&c) {
                remembered.push(CityMemo::of(&c));
                out.push(c);
            } else {
                if let Some(m) = self.memo.get(&key) {
                    m.overlay(&mut c);
                }
                out.push(c);
            }
        }
        // Remembered cities the core does not list any more.
        let previous: BTreeMap<CityKey, protocol::CityView> = self
            .cities
            .iter()
            .map(|c| ((c.owner, c.id), c.clone()))
            .collect();
        let mut forgotten: Vec<CityKey> = Vec::new();
        for key in self.memo.keys() {
            if seen_now.contains(key) {
                continue;
            }
            match previous.get(key) {
                Some(prev) if !visible(prev) => out.push(prev.clone()),
                _ => forgotten.push(*key),
            }
        }
        if remember {
            for m in remembered {
                self.memo.insert((m.owner, m.id), m);
            }
            for key in forgotten {
                self.memo.remove(&key);
            }
        }
        out.sort_by_key(|c| (c.owner, c.id));
        out
    }
}

impl TeamData {
    pub fn new(team: TeamId) -> Self {
        TeamData {
            team,
            ..TeamData::default()
        }
    }
}

/// What a seat adds to the data of its team.
pub(crate) struct SeatParts<'a> {
    pub header: ViewHeader,
    pub top_bar: TopBar,
    pub notifications: Vec<NotificationView>,
    pub peace_offers: &'a [PeaceOffer],
}

/// The view of a seat. `with_plots` copies the plots of the team; without them the map has none, which is
/// what `protocol::view::diff` wants when it is told which plots to compare itself.
pub(crate) fn assemble_view(
    map: &MapInfo,
    team: &TeamData,
    parts: SeatParts<'_>,
    with_plots: bool,
) -> WorldView {
    let h = parts.header;
    WorldView {
        turn: h.turn,
        year: h.year,
        human: h.human,
        human_team: h.human_team,
        status: h.status,
        blocker: h.blocker,
        map: MapView {
            width: map.width,
            height: map.height,
            wrap_x: map.wrap_x,
            wrap_y: map.wrap_y,
            plots: if with_plots {
                team.plots.clone()
            } else {
                Vec::new()
            },
            terrain_seed: map.terrain_seed,
        },
        units: team.units.clone(),
        cities: team.cities.clone(),
        players: team.players.clone(),
        top_bar: parts.top_bar,
        notifications: parts.notifications,
        peace_offers: parts.peace_offers.to_vec(),
        trade_routes: team.trade_routes.clone(),
    }
}

#[cfg(test)]
mod tests {
    use protocol::{CityFlags, CityView, PlayerView, PlotFlags};

    use super::*;

    const W: u16 = 6;

    fn map() -> MapInfo {
        MapInfo {
            width: W,
            height: 1,
            ..MapInfo::default()
        }
    }

    fn team() -> TeamData {
        let mut t = TeamData::new(0);
        t.plots = vec![PlotView::default(); usize::from(W)];
        t.players = vec![
            PlayerView {
                id: 0,
                team: 0,
                ..PlayerView::default()
            },
            PlayerView {
                id: 1,
                team: 1,
                ..PlayerView::default()
            },
        ];
        t
    }

    fn set_plot(t: &mut TeamData, x: usize, visible: bool, revealed: bool) {
        let mut bits = 0;
        if revealed {
            bits |= PlotFlags::REVEALED;
        }
        if visible {
            bits |= PlotFlags::VISIBLE;
        }
        t.plots[x].flags = PlotFlags(bits);
    }

    fn city(owner: i8, id: i32, x: i16, pop: i16, buildings: &[i16]) -> CityView {
        CityView {
            id,
            owner,
            x,
            y: 0,
            name: format!("c{id}"),
            population: pop,
            hp: 20 + pop,
            max_hp: 30,
            flags: CityFlags(CityFlags::CAPITAL),
            buildings: buildings.to_vec(),
            ..CityView::default()
        }
    }

    /// The record of a city the core does not see: name and position only.
    fn dark(c: &CityView) -> CityView {
        CityView {
            id: c.id,
            owner: c.owner,
            x: c.x,
            y: c.y,
            name: c.name.clone(),
            ..CityView::default()
        }
    }

    fn round(t: &mut TeamData, live: Vec<CityView>) -> Vec<CityView> {
        let out = t.cities_with_memo(live, &map(), true);
        t.cities = out.clone();
        out
    }

    #[test]
    fn a_city_in_sight_is_remembered_and_shown_as_the_core_says() {
        let mut t = team();
        set_plot(&mut t, 2, true, true);
        let c = city(1, 7, 2, 5, &[1]);
        let out = round(&mut t, vec![c.clone()]);
        assert_eq!(out, vec![c]);
        let m = &t.memo[&(1, 7)];
        assert_eq!((m.population, m.hp, m.buildings.clone()), (5, 25, vec![1]));
    }

    #[test]
    fn out_of_sight_the_last_seen_values_stay_whatever_the_core_says() {
        let mut t = team();
        set_plot(&mut t, 2, true, true);
        let seen = city(1, 7, 2, 5, &[1]);
        round(&mut t, vec![seen.clone()]);
        // The city leaves the sight; the core builds on and answers name and position only.
        set_plot(&mut t, 2, false, true);
        let grown = city(1, 7, 2, 6, &[1, 2]);
        let out = round(&mut t, vec![dark(&grown)]);
        assert_eq!(out, vec![seen.clone()], "the values of the last sight");
        // Even a core that leaks the live values does not change the view.
        let out = round(&mut t, vec![grown]);
        assert_eq!(out, vec![seen]);
    }

    #[test]
    fn a_city_that_was_never_in_sight_is_name_and_position_only() {
        let mut t = team();
        set_plot(&mut t, 3, false, true);
        let c = city(1, 8, 3, 4, &[3]);
        let out = round(&mut t, vec![dark(&c)]);
        assert_eq!(out, vec![dark(&c)]);
        assert!(t.memo.is_empty());
    }

    #[test]
    fn a_city_destroyed_under_fog_stays_until_the_plot_is_seen_empty() {
        let mut t = team();
        set_plot(&mut t, 2, true, true);
        let seen = city(1, 7, 2, 5, &[1]);
        round(&mut t, vec![seen.clone()]);
        set_plot(&mut t, 2, false, true);
        // The core has no city any more; the plot is out of sight.
        let out = round(&mut t, vec![]);
        assert_eq!(out, vec![seen], "still there in the view");
        assert!(t.memo.contains_key(&(1, 7)));
        // The plot is seen again, empty: the city is gone from the view and from the memory.
        set_plot(&mut t, 2, true, true);
        let out = round(&mut t, vec![]);
        assert!(out.is_empty());
        assert!(t.memo.is_empty());
    }

    #[test]
    fn own_cities_are_the_core_s() {
        let mut t = team();
        set_plot(&mut t, 1, false, true);
        let mine = city(0, 1, 1, 3, &[]);
        let out = round(&mut t, vec![mine.clone()]);
        assert_eq!(out, vec![mine]);
        assert!(t.memo.is_empty());
    }

    #[test]
    fn the_routes_are_sorted_and_reach_the_view() {
        use protocol::TradeRouteView;
        let r = |owner, from: (i16, i16), to: (i16, i16)| TradeRouteView {
            owner,
            from,
            to,
            domain: 0,
        };
        let sorted = sorted_routes(vec![
            r(1, (0, 0), (2, 2)),
            r(0, (3, 1), (1, 1)),
            r(0, (1, 5), (9, 9)),
        ]);
        assert_eq!(
            sorted,
            vec![
                r(0, (1, 5), (9, 9)),
                r(0, (3, 1), (1, 1)),
                r(1, (0, 0), (2, 2))
            ]
        );
        let mut t = team();
        t.trade_routes = sorted.clone();
        let parts = SeatParts {
            header: ViewHeader {
                turn: 1,
                year: 0,
                human: 0,
                human_team: 0,
                status: protocol::TurnStatus::HumanTurn,
                blocker: None,
            },
            top_bar: TopBar::default(),
            notifications: Vec::new(),
            peace_offers: &[],
        };
        let view = assemble_view(&map(), &t, parts, false);
        assert_eq!(view.trade_routes, sorted);
    }
}
