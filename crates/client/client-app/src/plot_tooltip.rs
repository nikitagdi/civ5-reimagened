//! The tooltip over the hovered plot (AC-7.4, play-ui §3.3): what the client already has in the
//! `WorldView` and the catalog, in lines. Shown after the pointer rested on a plot for
//! [`HOVER_DELAY_MS`] ([`Dwell`]).

use interim_ui::screens::tooltip::TooltipView;
use protocol::{Catalog, CatalogEntry, PlayerId, PlotFlags, WorldView};

/// The pointer has to stay on a plot this long before its tooltip shows.
pub const HOVER_DELAY_MS: u64 = 500;

const YIELDS: [&str; 6] = ["Food", "Production", "Gold", "Science", "Culture", "Faith"];

/// How long the pointer has been on the same plot.
#[derive(Debug, Default)]
pub struct Dwell {
    at: Option<((i32, i32), u64)>,
}

impl Dwell {
    /// The plot under the pointer now (`None` off the map or over a screen); returns it once it has
    /// been the same for the delay.
    pub fn update(&mut self, cell: Option<(i32, i32)>, now_ms: u64) -> Option<(i32, i32)> {
        let Some(cell) = cell else {
            self.at = None;
            return None;
        };
        let since = match self.at {
            Some((c, since)) if c == cell => since,
            _ => now_ms,
        };
        self.at = Some((cell, since));
        (now_ms.saturating_sub(since) >= HOVER_DELAY_MS).then_some(cell)
    }
}

fn name(table: Option<&Vec<CatalogEntry>>, id: i16) -> Option<String> {
    if id < 0 {
        return None;
    }
    Some(
        table
            .and_then(|t| t.iter().find(|e| e.id == id))
            .map_or_else(|| format!("#{id}"), |e| e.name.clone()),
    )
}

fn civ(world: &WorldView, owner: PlayerId) -> String {
    match world.players.iter().find(|p| p.id == owner) {
        Some(p) if !p.civ_name.is_empty() => p.civ_name.clone(),
        _ => format!("player {owner}"),
    }
}

/// "Your Archer, HP 6/10" / "Warrior (Egypt), HP 7/10".
fn unit_line(world: &WorldView, u: &protocol::UnitView) -> String {
    let who = if u.owner == world.human {
        format!("Your {}", u.name)
    } else {
        format!("{} ({})", u.name, civ(world, u.owner))
    };
    format!("{who}, HP {}/{}", u.hp, u.max_hp)
}

/// The tooltip of plot (x, y); `None` off the map or on a plot the player has not seen.
pub fn plot_tooltip(
    world: &WorldView,
    catalog: Option<&Catalog>,
    (x, y): (i32, i32),
) -> Option<TooltipView> {
    let m = &world.map;
    if x < 0 || y < 0 || x >= i32::from(m.width) || y >= i32::from(m.height) {
        return None;
    }
    let plot = m
        .plots
        .get(y as usize * usize::from(m.width) + x as usize)?;
    if !plot.flags.contains(PlotFlags::REVEALED) {
        return None;
    }
    let mut lines = Vec::new();
    // Terrain and feature make the title: "Grassland, Forest".
    let mut title = name(catalog.map(|c| &c.terrains), plot.terrain)
        .unwrap_or_else(|| "Unknown terrain".to_owned());
    if let Some(f) = name(catalog.map(|c| &c.features), plot.feature) {
        title = format!("{title}, {f}");
    }
    lines.push(format!("{title} ({x}, {y})"));
    if let Some(r) = name(catalog.map(|c| &c.resources), plot.resource) {
        lines.push(format!("Resource: {r}"));
    }
    if let Some(i) = name(catalog.map(|c| &c.improvements), plot.improvement) {
        let pillaged = if plot.flags.contains(PlotFlags::PILLAGED) {
            " (pillaged)"
        } else {
            ""
        };
        lines.push(format!("Improvement: {i}{pillaged}"));
    }
    if let Some(r) = name(catalog.map(|c| &c.routes), plot.route) {
        lines.push(format!("Route: {r}"));
    }
    if plot.owner >= 0 {
        lines.push(if plot.owner == world.human {
            "Owner: you".to_owned()
        } else {
            format!("Owner: {}", civ(world, plot.owner))
        });
    }
    let yields: Vec<String> = plot
        .yields
        .iter()
        .zip(YIELDS)
        .filter(|(v, _)| **v != 0)
        .map(|(v, n)| format!("{v} {n}"))
        .collect();
    if !yields.is_empty() {
        lines.push(yields.join(", "));
    }
    let at = (x as i16, y as i16);
    if let Some(c) = world.cities.iter().find(|c| (c.x, c.y) == at) {
        let whose = if c.owner == world.human {
            String::new()
        } else {
            format!(" ({})", civ(world, c.owner))
        };
        let pop = if c.population > 0 {
            format!(", population {}", c.population)
        } else {
            String::new()
        };
        lines.push(format!("City: {}{whose}{pop}", c.name));
    }
    for u in world.units.iter().filter(|u| (u.x, u.y) == at) {
        lines.push(unit_line(world, u));
    }
    Some(TooltipView { lines })
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::{CityView, PlayerView, PlotView, UnitView};

    fn entry(id: i16, name: &str) -> CatalogEntry {
        CatalogEntry {
            id,
            name: name.into(),
            ..CatalogEntry::default()
        }
    }

    /// A 2x2 map: (1, 0) forest with iron, a pillaged mine, Egypt's, with Thebes and a Warrior;
    /// (0, 1) grassland with our Archer; (0, 0) unseen.
    fn world() -> (WorldView, Catalog) {
        let mut world = crate::test_world::world();
        let catalog = Catalog {
            terrains: vec![entry(0, "Grassland"), entry(1, "Plains")],
            features: vec![entry(2, "Forest")],
            resources: vec![entry(3, "Iron")],
            improvements: vec![entry(4, "Mine")],
            ..Catalog::default()
        };
        let seen = PlotFlags::REVEALED | PlotFlags::VISIBLE;
        let mut plots = vec![PlotView::default(); 4];
        plots[1] = PlotView {
            flags: PlotFlags(seen | PlotFlags::PILLAGED),
            terrain: 1,
            feature: 2,
            resource: 3,
            improvement: 4,
            route: -1,
            owner: 1,
            yields: [1, 2, 0, 0, 0, 0],
            ..PlotView::default()
        };
        plots[2] = PlotView {
            flags: PlotFlags(seen),
            terrain: 0,
            feature: -1,
            resource: -1,
            improvement: -1,
            route: -1,
            owner: -1,
            yields: [2, 0, 1, 0, 0, 0],
            ..PlotView::default()
        };
        let player = |id, civ: &str| PlayerView {
            id,
            team: id,
            civ_name: civ.into(),
            ..PlayerView::default()
        };
        world.human = 0;
        world.map.width = 2;
        world.map.height = 2;
        world.map.plots = plots;
        world.players = vec![player(0, "Rome"), player(1, "Egypt")];
        world.units = vec![
            UnitView {
                id: 1,
                owner: 1,
                x: 1,
                y: 0,
                hp: 7,
                max_hp: 10,
                name: "Warrior".into(),
                ..UnitView::default()
            },
            UnitView {
                id: 2,
                owner: 0,
                x: 0,
                y: 1,
                hp: 10,
                max_hp: 10,
                name: "Archer".into(),
                ..UnitView::default()
            },
        ];
        world.cities = vec![CityView {
            id: 1,
            owner: 1,
            x: 1,
            y: 0,
            name: "Thebes".into(),
            population: 3,
            ..CityView::default()
        }];
        (world, catalog)
    }

    #[test]
    fn lists_terrain_feature_resource_improvement_owner_yields_city_and_units() {
        let (w, c) = world();
        let t = plot_tooltip(&w, Some(&c), (1, 0)).unwrap();
        assert_eq!(
            t.lines,
            [
                "Plains, Forest (1, 0)",
                "Resource: Iron",
                "Improvement: Mine (pillaged)",
                "Owner: Egypt",
                "1 Food, 2 Production",
                "City: Thebes (Egypt), population 3",
                "Warrior (Egypt), HP 7/10",
            ]
        );
    }

    #[test]
    fn a_bare_plot_has_a_title_and_yields_only() {
        let (w, c) = world();
        let t = plot_tooltip(&w, Some(&c), (0, 1)).unwrap();
        assert_eq!(
            t.lines,
            [
                "Grassland (0, 1)",
                "2 Food, 1 Gold",
                "Your Archer, HP 10/10"
            ]
        );
    }

    #[test]
    fn unseen_and_off_map_plots_have_none() {
        let (w, c) = world();
        assert!(plot_tooltip(&w, Some(&c), (0, 0)).is_none());
        assert!(plot_tooltip(&w, Some(&c), (5, 0)).is_none());
        assert!(plot_tooltip(&w, Some(&c), (-1, 0)).is_none());
    }

    #[test]
    fn the_tooltip_waits_half_a_second_on_the_same_plot() {
        let mut d = Dwell::default();
        assert_eq!(d.update(Some((1, 1)), 1000), None);
        assert_eq!(d.update(Some((1, 1)), 1499), None);
        assert_eq!(d.update(Some((1, 1)), 1500), Some((1, 1)));
        // Another plot starts the wait again; leaving the map ends it.
        assert_eq!(d.update(Some((2, 1)), 1600), None);
        assert_eq!(d.update(Some((2, 1)), 2100), Some((2, 1)));
        assert_eq!(d.update(None, 2200), None);
        assert_eq!(d.update(Some((2, 1)), 2300), None);
    }
}
