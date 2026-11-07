//! The ground share (mod API 1.2, `GameMsg::TerrainShare`): hexes a mod lets the player draw the ground of although
//! the player has not revealed them, such as an arena in another player's land seen through a first-person camera.
//!
//! An overlay of the terrain only. For the systems that draw the ground (the field and the cells of the terrain, the
//! fluid fog and the classic fog over them, and so the trees, the water and the overlays that read the fog's state)
//! a shared hex that is unrevealed counts as revealed and not seen: its cells are built, its heights answer and it is
//! drawn like fogged revealed land. Everything that tells the player what was revealed keeps the real fog: the minimap,
//! the snapshot (units, cities, improvements, routes and resources are not drawn there), the camera bounds, the
//! tooltips and the game's view. A hex the player really reveals is drawn from its real fog; the basis is the same.

use crate::terrain::Fog;
use crate::world::fow::FogInput;
use crate::world::terrain::PlotBasis;

/// The shared hexes and their records (the fog-free basis records the server sent), ascending by index, each once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GroundShare {
    records: Vec<(u32, PlotBasis)>,
}

impl GroundShare {
    /// From the records as they came (any order; the first record of an index wins).
    pub fn new(mut records: Vec<(u32, PlotBasis)>) -> Self {
        records.sort_by_key(|(i, _)| *i);
        records.dedup_by_key(|(i, _)| *i);
        Self { records }
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn contains(&self, index: u32) -> bool {
        self.records
            .binary_search_by_key(&index, |(i, _)| *i)
            .is_ok()
    }

    /// The shared hexes, ascending.
    pub fn indices(&self) -> impl Iterator<Item = u32> + '_ {
        self.records.iter().map(|(i, _)| *i)
    }

    pub fn records(&self) -> &[(u32, PlotBasis)] {
        &self.records
    }

    /// The fog of the ground: a shared hex that is unrevealed is revealed and not seen (`Fog::Hidden`); the rest
    /// stays. An index past `fog` is ignored.
    pub fn ground_fog(&self, fog: &mut [Fog]) {
        for i in self.indices() {
            if let Some(f) = fog.get_mut(i as usize)
                && *f == Fog::Unrevealed
            {
                *f = Fog::Hidden;
            }
        }
    }

    /// [`Self::ground_fog`] of a whole fog input.
    pub fn ground_input(&self, mut fog: FogInput) -> FogInput {
        self.ground_fog(&mut fog.cells);
        fog
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::Relief;

    fn rec(relief: Relief) -> PlotBasis {
        PlotBasis {
            relief,
            river_flow: PlotBasis::NO_FLOW,
            ..PlotBasis::default()
        }
    }

    #[test]
    fn a_shared_unrevealed_hex_is_revealed_and_not_seen_for_the_ground_only() {
        let s = GroundShare::new(vec![
            (3, rec(Relief::Hill)),
            (1, rec(Relief::Flat)),
            (3, rec(Relief::Mountain)),
            (40, rec(Relief::Flat)),
        ]);
        assert_eq!(s.indices().collect::<Vec<_>>(), [1, 3, 40]);
        assert!(s.contains(3) && !s.contains(2));
        let mut fog = vec![
            Fog::Unrevealed,
            Fog::Unrevealed,
            Fog::Visible,
            Fog::Unrevealed,
        ];
        s.ground_fog(&mut fog);
        // 1 and 3 shared: fogged; 0 not shared: still unrevealed; 2 seen: stays seen; 40 is off the map.
        assert_eq!(
            fog,
            [Fog::Unrevealed, Fog::Hidden, Fog::Visible, Fog::Hidden]
        );
        let mut seen = vec![Fog::Visible; 4];
        s.ground_fog(&mut seen);
        assert_eq!(
            seen,
            [Fog::Visible; 4],
            "the real fog wins where it is revealed"
        );
        assert!(GroundShare::default().is_empty());
    }
}
