//! Fog of the test world, by team (game-host-mp 4.14): a team sees every plot within `RADIUS` of its own
//! units and cities; what it has seen stays revealed. A change of fog is a change of the plot record, so
//! `Fog::update` returns the changed plot indexes for `Core::dirty_plots`.

use std::collections::BTreeSet;

use protocol::TeamId;
use serde::{Deserialize, Serialize};

use super::map::Geo;

/// Sight of a unit or a city, in plots.
pub const RADIUS: u8 = 2;

/// Dev only: `CIV5_FIXTURE_REVEAL_ALL=1` shows the whole map to every team of a fixture core (the
/// terrain-scroll scenario needs terrain along the whole width). It is read at the queries, so a game
/// restored from a save is covered too; the real core is not touched by it.
fn reveal_all() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("CIV5_FIXTURE_REVEAL_ALL").is_some_and(|v| v != "0"))
}

/// Visible and revealed plots of every team.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fog {
    geo: Geo,
    teams: Vec<TeamId>,
    visible: Vec<Vec<bool>>,
    revealed: Vec<Vec<bool>>,
    /// No fog at all: every team sees every plot (the full set, card N-R2).
    lit: bool,
}

impl Fog {
    /// Fog of the given teams; nothing is revealed until the first `update`.
    pub fn new(geo: Geo, teams: Vec<TeamId>) -> Self {
        let n = geo.plot_count();
        Fog {
            geo,
            visible: vec![vec![false; n]; teams.len()],
            revealed: vec![vec![false; n]; teams.len()],
            teams,
            lit: false,
        }
    }

    /// Like [`Fog::new`], but every team sees the whole map from the first `update` on.
    pub fn lit(geo: Geo, teams: Vec<TeamId>) -> Self {
        Fog {
            lit: true,
            ..Self::new(geo, teams)
        }
    }

    pub fn teams(&self) -> &[TeamId] {
        &self.teams
    }

    fn slot(&self, team: TeamId) -> Option<usize> {
        self.teams.iter().position(|t| *t == team)
    }

    /// The team sees the plot now. A team without fog (barbarians) sees nothing.
    pub fn is_visible(&self, team: TeamId, index: u32) -> bool {
        if reveal_all() && self.slot(team).is_some() {
            return (index as usize) < self.geo.plot_count();
        }
        self.slot(team)
            .and_then(|s| self.visible[s].get(index as usize))
            .copied()
            .unwrap_or(false)
    }

    /// The team has seen the plot at some time.
    pub fn is_revealed(&self, team: TeamId, index: u32) -> bool {
        if reveal_all() && self.slot(team).is_some() {
            return (index as usize) < self.geo.plot_count();
        }
        self.slot(team)
            .and_then(|s| self.revealed[s].get(index as usize))
            .copied()
            .unwrap_or(false)
    }

    /// Recounts sight from the `sources` (team, position of a unit or a city). Returns the indexes of the
    /// plots whose visibility or revelation changed for some team, ascending.
    pub fn update(&mut self, sources: &[(TeamId, (i16, i16))]) -> Vec<u32> {
        let n = self.geo.plot_count();
        let mut fresh = vec![vec![self.lit; n]; self.teams.len()];
        for (team, at) in sources.iter().filter(|_| !self.lit) {
            let Some(s) = self.slot(*team) else {
                continue;
            };
            for p in self.geo.within(*at, RADIUS) {
                fresh[s][self.geo.index(p) as usize] = true;
            }
        }
        let mut changed = BTreeSet::new();
        for (s, sight) in fresh.into_iter().enumerate() {
            for (i, seen) in sight.into_iter().enumerate() {
                let was_revealed = self.revealed[s][i];
                if self.visible[s][i] != seen || was_revealed != (was_revealed || seen) {
                    changed.insert(i as u32);
                }
                self.visible[s][i] = seen;
                self.revealed[s][i] = was_revealed || seen;
            }
        }
        changed.into_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sight_accumulates_per_team() {
        let geo = Geo::new(40, 24, false);
        let mut fog = Fog::new(geo, vec![0, 1]);
        let changed = fog.update(&[(0, (5, 5)), (1, (30, 15))]);
        assert_eq!(changed.len(), 2 * 19);
        let a = geo.index((5, 5));
        let b = geo.index((30, 15));
        assert!(fog.is_visible(0, a) && !fog.is_visible(1, a));
        assert!(fog.is_visible(1, b) && !fog.is_visible(0, b));
        assert!(!fog.is_revealed(1, a));

        // Team 0 walks away: the old plots stay revealed, but are not visible.
        let changed = fog.update(&[(0, (8, 5)), (1, (30, 15))]);
        assert!(!changed.is_empty());
        assert!(fog.is_revealed(0, a));
        assert!(!fog.is_visible(0, a) || geo.distance((5, 5), (8, 5)) <= 2);
        assert!(fog.is_visible(0, geo.index((8, 5))));
        // Team 1 did not move: nothing changed for it.
        assert!(
            changed
                .iter()
                .all(|i| geo.distance(geo.xy(*i), (30, 15)) > 2)
        );

        // Nothing moves: nothing changes.
        assert!(fog.update(&[(0, (8, 5)), (1, (30, 15))]).is_empty());
    }

    #[test]
    fn sight_crosses_the_seam_and_stops_at_the_edge() {
        let wrapped = Geo::new(40, 24, true);
        let mut fog = Fog::new(wrapped, vec![0]);
        fog.update(&[(0, (0, 10))]);
        assert!(fog.is_visible(0, wrapped.index((39, 10))));
        assert!(fog.is_visible(0, wrapped.index((38, 10))));

        let flat = Geo::new(40, 24, false);
        let mut fog = Fog::new(flat, vec![0]);
        fog.update(&[(0, (0, 10))]);
        assert!(!fog.is_visible(0, flat.index((39, 10))));
    }

    #[test]
    fn unknown_team_sees_nothing() {
        let geo = Geo::new(40, 24, false);
        let mut fog = Fog::new(geo, vec![0]);
        fog.update(&[(0, (5, 5)), (63, (20, 12))]);
        assert!(!fog.is_visible(63, geo.index((20, 12))));
        assert!(!fog.is_revealed(63, geo.index((20, 12))));
    }
}
