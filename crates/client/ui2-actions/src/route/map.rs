//! `map.click`: a click on a plot of the map, as the host's picker reports it. What it does depends on the map
//! mode and on what stands on the plot.
//!
//! `map.target` and `map.cancel`: the map's targeting mode, the original's interface modes that ask for a plot
//! (`WorldView.lua`, `InGame.lua`). The mode names what acts (`ui.target`); the `targets` service root answers with
//! the plots the core accepts. A left click on one of them gives the order through the screen action `targets.go`
//! and ends the mode; a left click elsewhere does nothing (also while the answer is still on its way); a right click
//! or Escape ends the mode.

use protocol::Command;
use ui2_model::Key;
use ui2_model::generated::{
    Map, MapMode, MouseButton, RefuseWhy, Selection, TargetKind, Targets, UiTarget, WindowId,
};
use ui2_model::target::{city_target, unit_target};

use crate::intent::Intent;
use crate::route::{Ctx, unit, window};
use crate::validate::{self, entity_id};

/// Run a `map.click` intent.
pub(crate) fn click(ctx: &mut Ctx<'_>, intent: &Intent) {
    let (Some(plot), Some(button)) = (
        intent.plot("plot"),
        intent.variant("button").and_then(MouseButton::from_u16),
    ) else {
        return;
    };
    if ctx.ui.ui().map_mode == MapMode::Target {
        match button {
            MouseButton::Left => aim(ctx, plot),
            // The original's right button leaves an interface mode (`DefaultMessageHandler[RButtonUp]`).
            MouseButton::Right => cancel(ctx),
        }
        return;
    }
    match button {
        MouseButton::Left => left(ctx, plot),
        MouseButton::Right => {
            if let Some(selected) = ctx.ui.ui().selection.unit {
                unit::order_at(ctx, selected, plot, false);
            }
        }
    }
}

/// Run a `map.target` intent: the targeting mode for a unit's order, a city's strike or a city-state gift.
pub(crate) fn target(ctx: &mut Ctx<'_>, intent: &Intent) {
    let Some(kind) = intent.variant("kind").and_then(TargetKind::from_u16) else {
        return;
    };
    enter(
        ctx,
        kind,
        intent.key("unit"),
        intent.key("city"),
        intent.int("a"),
    );
}

/// Start the targeting mode. A unit's order selects the unit; a city's strike selects the city. Refused when what
/// acts is not the player's or the arguments do not name it.
pub(crate) fn enter(
    ctx: &mut Ctx<'_>,
    kind: TargetKind,
    unit: Option<Key>,
    city: Option<Key>,
    a: Option<i32>,
) {
    let target = match kind {
        TargetKind::Unit => {
            let (Some(u), Some(a)) = (unit, a) else {
                ctx.refuse(RefuseWhy::Invalid);
                return;
            };
            if validate::own_unit(ctx.model, u).is_none() {
                ctx.refuse(RefuseWhy::NoUnit);
                return;
            }
            if ctx.ui.ui().selection.unit != Some(u) {
                unit::select(ctx, u);
            }
            UiTarget {
                kind,
                unit: Some(u),
                city: None,
                a,
            }
        }
        TargetKind::City => {
            let Some(c) = city else {
                ctx.refuse(RefuseWhy::Invalid);
                return;
            };
            let Some(banner) = ctx
                .model
                .get::<Map>()
                .and_then(|m| m.cities.get(c))
                .filter(|b| b.mine)
            else {
                ctx.refuse(RefuseWhy::NotYourCity);
                return;
            };
            if !banner.can_shoot {
                ctx.refuse(RefuseWhy::ActionUnavailable);
                return;
            }
            ctx.ui.edit().selection = Selection {
                unit: None,
                city: Some(c),
                plot: Some(banner.plot.clone()),
            };
            UiTarget {
                kind,
                unit: None,
                city: Some(c),
                a: 0,
            }
        }
        TargetKind::GiftUnit | TargetKind::GiftImprovement => {
            let Some(a) = a else {
                ctx.refuse(RefuseWhy::Invalid);
                return;
            };
            UiTarget {
                kind,
                unit: None,
                city: None,
                a,
            }
        }
    };
    let ui = ctx.ui.edit();
    ui.map_mode = MapMode::Target;
    ui.target = Some(target);
}

/// Leave the targeting mode (`map.cancel`, Escape, a right click). The original's city strike also lets the city go.
pub(crate) fn cancel(ctx: &mut Ctx<'_>) {
    let ui = ctx.ui.edit();
    if ui
        .target
        .as_ref()
        .is_some_and(|t| t.kind == TargetKind::City)
    {
        ui.selection.city = None;
    }
    ui.target = None;
    if ui.map_mode == MapMode::Target {
        ui.map_mode = MapMode::Select;
    }
}

/// The screen arguments of a targeting mode: what acts (a unit's or city's id, or the city-state) and the order, as
/// `ui2_source::remote` asks the `targets` root for them.
fn target_args(t: &UiTarget) -> (i32, i32) {
    match t.kind {
        TargetKind::Unit => (t.unit.map_or(-1, entity_id), t.a),
        TargetKind::City => (t.city.map_or(-1, entity_id), 0),
        TargetKind::GiftUnit | TargetKind::GiftImprovement => (t.a, 0),
    }
}

/// `unit.select` in the targeting mode (a click on a unit's flag): the click on the unit's plot when the unit is
/// another player's, or when the mode picks one of the player's units (a gift). `false`: an ordinary selection.
pub(crate) fn aims_at_unit(ctx: &mut Ctx<'_>, unit: Key) -> bool {
    let ui = ctx.ui.ui();
    if ui.map_mode != MapMode::Target {
        return false;
    }
    let gift = ui
        .target
        .as_ref()
        .is_some_and(|t| t.kind == TargetKind::GiftUnit);
    let Some((mine, x, y)) = ctx
        .model
        .get::<Map>()
        .and_then(|m| m.units.get(unit))
        .map(|u| (u.mine, u.plot.x, u.plot.y))
    else {
        return false;
    };
    if mine && !gift {
        return false;
    }
    aim(ctx, (x, y));
    true
}

/// A left click in the targeting mode: the order on a plot the server listed, nothing elsewhere.
fn aim(ctx: &mut Ctx<'_>, (x, y): (i16, i16)) {
    let Some(t) = ctx.ui.ui().target.clone() else {
        cancel(ctx);
        return;
    };
    let (id, action) = target_args(&t);
    let cell = i32::from(x) * 65_536 + i32::from(y);
    let accepted = ctx
        .model
        .get::<Option<Targets>>()
        .and_then(Option::as_ref)
        // An answer for another mode (still on its way out) is not this one's.
        .filter(|r| r.id == id && r.action == action)
        .is_some_and(|r| r.any || r.plots.iter().any(|p| p.cell == cell));
    if !accepted {
        return;
    }
    let what = match (t.unit, t.city) {
        (Some(u), _) => unit_target(u),
        (None, Some(c)) => city_target(c),
        _ => "targets".to_owned(),
    };
    cancel(ctx);
    ctx.send(
        Command::ScreenAction {
            screen: "targets".into(),
            action: "go".into(),
            args: vec![
                ("a".into(), i64::from(t.kind as u16)),
                ("b".into(), i64::from(id)),
                ("c".into(), i64::from(action)),
                ("d".into(), i64::from(cell)),
            ],
            text: None,
        },
        "map.target",
        what,
        None,
    );
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
