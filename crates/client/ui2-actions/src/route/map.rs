//! `map.click`: a click on a plot of the map, as the host's picker reports it. What it does depends on the map
//! mode and on what stands on the plot.

use ui2_model::Key;
use ui2_model::generated::{Map, MapMode, MouseButton, Selection, WindowId};

use crate::intent::Intent;
use crate::route::{Ctx, unit, window};

/// Run a `map.click` intent.
pub(crate) fn click(ctx: &mut Ctx<'_>, intent: &Intent) {
    let (Some(plot), Some(button)) = (
        intent.plot("plot"),
        intent.variant("button").and_then(MouseButton::from_u16),
    ) else {
        return;
    };
    match button {
        MouseButton::Left => left(ctx, plot),
        MouseButton::Right => {
            if let Some(selected) = ctx.ui.ui().selection.unit {
                unit::order_at(ctx, selected, plot, false);
            }
        }
    }
}

fn left(ctx: &mut Ctx<'_>, plot: (i16, i16)) {
    let ui = ctx.ui.ui();
    let (mode, selected) = (ui.map_mode, ui.selection.unit);
    if mode != MapMode::Select {
        if let Some(selected) = selected {
            unit::order_at(ctx, selected, plot, mode == MapMode::RangeAttack);
            return;
        }
        // A mode without a unit is a stale one.
        ctx.ui.edit().map_mode = MapMode::Select;
    }
    let Some(map) = ctx.model.get::<Map>() else {
        return;
    };
    let here: Vec<Key> = map
        .units
        .iter()
        .filter(|u| u.mine && (u.plot.x, u.plot.y) == plot)
        .map(|u| u.key)
        .collect();
    if !here.is_empty() {
        // A second click on the same plot goes on to the next unit standing there.
        let at = selected
            .and_then(|cur| here.iter().position(|k| *k == cur))
            .map_or(0, |i| (i + 1) % here.len());
        unit::select(ctx, here[at]);
        return;
    }
    let city = map
        .cities
        .iter()
        .find(|c| c.mine && (c.plot.x, c.plot.y) == plot)
        .map(|c| c.key);
    if let Some(city) = city {
        let ui = ctx.ui.edit();
        ui.selection = Selection {
            unit: None,
            city: Some(city),
            plot: Some(ui2_model::generated::Plot {
                x: plot.0,
                y: plot.1,
            }),
        };
        window::open(ctx, WindowId::Production, Some(city));
    }
}
