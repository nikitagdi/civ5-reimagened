//! The `owners` ribbon of the bridge (world-overlays WOV0): `MapView` and the players -> the
//! render crate's [`OwnersInput`] for `Renderer::set_owners` (the borders of the `Overlay`
//! subsystem), as the [`OwnersFeed`] (R4.10). It reads the sections `PLOTS` and `PLAYERS`.

use client_core::ViewChange;
use protocol::{Catalog, MapView, PlayerFlags, PlayerView, PlotFlags, SectionMask, WorldView};
use render::Renderer;
use render::world::overlay::OwnersInput;

use super::{FeedCx, ViewFeed, mask, section};

/// Border colour per player id: the primary colour (RGBA in a `u32`) of each player.
pub fn owner_colors(players: &[PlayerView]) -> Vec<[u8; 3]> {
    let len = players
        .iter()
        .filter_map(|p| usize::try_from(p.id).ok())
        .max()
        .map_or(0, |m| m + 1);
    let mut colors = vec![[128u8; 3]; len];
    for p in players {
        if let Ok(i) = usize::try_from(p.id) {
            colors[i] = {
                let [r, g, b, _] = p.color_primary.to_be_bytes();
                [r, g, b]
            };
        }
    }
    colors
}

/// The second colour per player id (the fill of the border): the secondary colour of each player.
pub fn owner_secondary_colors(players: &[PlayerView]) -> Vec<[u8; 3]> {
    let len = players
        .iter()
        .filter_map(|p| usize::try_from(p.id).ok())
        .max()
        .map_or(0, |m| m + 1);
    let mut colors = vec![[255u8; 3]; len];
    for p in players {
        if let Ok(i) = usize::try_from(p.id) {
            let [r, g, b, _] = p.color_secondary.to_be_bytes();
            colors[i] = [r, g, b];
        }
    }
    colors
}

/// Which player ids are city states (their border is dashed).
pub fn owner_city_states(players: &[PlayerView]) -> Vec<bool> {
    let len = players
        .iter()
        .filter_map(|p| usize::try_from(p.id).ok())
        .max()
        .map_or(0, |m| m + 1);
    let mut minor = vec![false; len];
    for p in players {
        if let Ok(i) = usize::try_from(p.id) {
            minor[i] = p.flags.contains(PlayerFlags::MINOR);
        }
    }
    minor
}

/// The owner of every cell, `-1` for none or for a plot the player has not revealed (the protocol
/// zeroes it), and the colours and the kind of every player. `None` if the plot count does not match
/// the size.
pub fn owners_input(map: &MapView, players: &[PlayerView]) -> Option<OwnersInput> {
    OwnersInput::new(
        u32::from(map.width),
        u32::from(map.height),
        map.plots
            .iter()
            .map(|p| {
                if p.flags.contains(PlotFlags::REVEALED) {
                    p.owner
                } else {
                    -1
                }
            })
            .collect(),
        owner_colors(players),
    )
    .map(|o| {
        o.with_secondary_colors(owner_secondary_colors(players))
            .with_city_states(owner_city_states(players))
    })
}

/// The feed of the borders: the owner of every plot and the colours, to `Renderer::set_owners`.
/// The renderer compares the input with the last one and does nothing when they are equal.
#[derive(Debug, Default)]
pub struct OwnersFeed {
    pending: Option<OwnersInput>,
}

impl OwnersFeed {
    /// The input that waits for the next `flush` (tests).
    pub fn pending(&self) -> Option<&OwnersInput> {
        self.pending.as_ref()
    }
}

impl ViewFeed for OwnersFeed {
    fn sections(&self) -> SectionMask {
        mask(&[section::PLOTS, section::PLAYERS])
    }

    fn on_catalog(&mut self, _catalog: &Catalog) {}

    fn apply(&mut self, view: &WorldView, _change: &ViewChange, _cx: &mut FeedCx<'_>) {
        self.pending = owners_input(&view.map, &view.players);
    }

    fn flush(&mut self, renderer: &mut Renderer) {
        if let Some(owners) = self.pending.take() {
            renderer.set_owners(owners);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::tests::fixture_world;

    #[test]
    fn the_colours_follow_the_player_ids() {
        let (world, _) = fixture_world();
        let colors = owner_colors(&world.players);
        let max = world.players.iter().map(|p| p.id).max().unwrap();
        assert_eq!(colors.len(), max as usize + 1);
        let p = &world.players[0];
        let [r, g, b, _] = p.color_primary.to_be_bytes();
        assert_eq!(colors[p.id as usize], [r, g, b]);
    }

    #[test]
    fn unrevealed_plots_have_no_owner() {
        let (mut world, _) = fixture_world();
        world.map.plots[0].flags = protocol::PlotFlags(0);
        world.map.plots[0].owner = 3;
        let owners = owners_input(&world.map, &world.players).unwrap();
        assert_eq!(owners.owners[0], -1);
        assert_eq!(owners.owners.len(), world.map.plots.len());
    }

    #[test]
    fn the_owners_of_the_snapshot_and_of_the_function_agree() {
        use crate::bridge::{TerrainTable, terrain_snapshot};
        let (world, catalog) = fixture_world();
        let snapshot = terrain_snapshot(&world.map, &TerrainTable::from_catalog(&catalog))
            .expect("the fixture map is whole")
            .with_owner_colors(owner_colors(&world.players));
        let from_view = owners_input(&world.map, &world.players).unwrap();
        let from_snapshot = OwnersInput::from_snapshot(&snapshot);
        assert_eq!(from_view.owners, from_snapshot.owners);
        // The snapshot's list ends with the last owner that has a cell.
        assert!(from_view.colors.starts_with(&from_snapshot.colors));
    }
}
