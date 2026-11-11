//! `unit.select`, `unit.select_next`, `unit.deselect`, `unit.action`, `unit.move`, `unit.range_attack`: the
//! selection and the orders of a unit. The selection is local (`ui.selection`); the core does not know it.

use protocol::Command;
use ui2_model::Key;
use ui2_model::generated::{Map, MapMode, Plot, RefuseWhy, Selection, UnitActionKind};

use crate::intent::Intent;
use crate::route::{Ctx, camera};
use crate::validate::{self, entity_id};

/// Run a `unit.*` intent.
pub(crate) fn run(ctx: &mut Ctx<'_>, intent: &Intent) {
    match intent.name() {
        "unit.select" => {
            if let Some(unit) = intent.key("unit") {
                select(ctx, unit);
            }
        }
        "unit.select_next" => select_next(ctx),
        "unit.deselect" => deselect(ctx),
        "unit.action" => {
            let kind = intent.variant("action").and_then(UnitActionKind::from_u16);
            if let (Some(unit), Some(kind)) = (intent.key("unit"), kind) {
                action(ctx, unit, kind);
            }
        }
        "unit.move" | "unit.range_attack" => {
            if let (Some(unit), Some((x, y))) = (intent.key("unit"), intent.plot("plot")) {
                order_at(ctx, unit, (x, y), intent.name() == "unit.range_attack");
            }
        }
        _ => {}
    }
}

/// Select a unit of the player's. The city selection goes; the mode goes back to `select`.
pub(crate) fn select(ctx: &mut Ctx<'_>, unit: Key) {
    let Some(flag) = validate::own_unit(ctx.model, unit) else {
        ctx.refuse(RefuseWhy::NoUnit);
        return;
    };
    let plot = flag.plot.clone();
    let ui = ctx.ui.edit();
    ui.selection = Selection {
        unit: Some(unit),
        city: None,
        plot: Some(plot),
    };
    ui.map_mode = MapMode::Select;
}

/// Clear the unit selection and the map mode.
pub(crate) fn deselect(ctx: &mut Ctx<'_>) {
    let ui = ctx.ui.edit();
    ui.selection.unit = None;
    ui.map_mode = MapMode::Select;
}

/// The next of the player's units that wait for orders after the selected one (the first when none is
/// selected or the selected one is the last); the camera goes to it.
pub(crate) fn select_next(ctx: &mut Ctx<'_>) {
    let waiting: Vec<(Key, Plot)> = ctx
        .model
        .get::<Map>()
        .map(|m| {
            m.units
                .iter()
                .filter(|u| u.mine && u.waiting)
                .map(|u| (u.key, u.plot.clone()))
                .collect()
        })
        .unwrap_or_default();
    if waiting.is_empty() {
        ctx.refuse(RefuseWhy::NoUnit);
        return;
    }
    let at = ctx
        .ui
        .ui()
        .selection
        .unit
        .and_then(|cur| waiting.iter().position(|(k, _)| *k == cur))
        .map_or(0, |i| (i + 1) % waiting.len());
    let (unit, plot) = waiting[at].clone();
    select(ctx, unit);
    camera::to(ctx, plot.x, plot.y);
}

/// A unit action. Move and ranged attack only set the map mode (the order goes with the click on a plot); the
/// rest is one order.
fn action(ctx: &mut Ctx<'_>, unit: Key, kind: UnitActionKind) {
    // SIMPLIFIED(SIMP-250): alert, pillage and upgrade are shown with the availability the core reports but
    // are never ordered: the protocol has no order for them. Original: all three are unit commands.
    if matches!(
        kind,
        UnitActionKind::Alert | UnitActionKind::Pillage | UnitActionKind::Upgrade
    ) {
        ctx.refuse(RefuseWhy::Unsupported);
        return;
    }
    if validate::own_unit(ctx.model, unit).is_none() {
        ctx.refuse(RefuseWhy::NoUnit);
        return;
    }
    if let Err(why) = validate::unit_action(ctx.model, unit, kind) {
        ctx.refuse(why);
        return;
    }
    let id = entity_id(unit);
    let cmd = match kind {
        UnitActionKind::Move | UnitActionKind::RangeAttack => {
            if ctx.ui.ui().selection.unit != Some(unit) {
                select(ctx, unit);
            }
            ctx.ui.edit().map_mode = if kind == UnitActionKind::Move {
                MapMode::Move
            } else {
                MapMode::RangeAttack
            };
            return;
        }
        UnitActionKind::FoundCity => Command::UnitFoundCity { unit: id },
        UnitActionKind::Fortify => Command::UnitFortify { unit: id },
        UnitActionKind::Sleep => Command::UnitSleep { unit: id },
        UnitActionKind::Skip => Command::UnitSkip { unit: id },
        UnitActionKind::Heal => Command::UnitHeal { unit: id },
        UnitActionKind::AutomateBuild => Command::UnitAutomateBuild { unit: id },
        UnitActionKind::AutomateExplore => Command::UnitAutomateExplore { unit: id },
        UnitActionKind::Cancel => Command::UnitCancel { unit: id },
        UnitActionKind::Disband => Command::UnitDisband { unit: id },
        UnitActionKind::Wake => Command::UnitWake { unit: id },
        UnitActionKind::Alert | UnitActionKind::Pillage | UnitActionKind::Upgrade => return,
    };
    ctx.send(cmd, "unit.action", unit.to_js(), None);
}

/// Order a unit to a plot: a move (a melee attack is the same order) or a ranged attack. The map mode goes
/// back to `select` whether or not the order went.
pub(crate) fn order_at(ctx: &mut Ctx<'_>, unit: Key, plot: (i16, i16), ranged: bool) {
    ctx.ui.edit().map_mode = MapMode::Select;
    if validate::own_unit(ctx.model, unit).is_none() {
        ctx.refuse(RefuseWhy::NoUnit);
        return;
    }
    if ranged && let Err(why) = validate::unit_action(ctx.model, unit, UnitActionKind::RangeAttack)
    {
        ctx.refuse(why);
        return;
    }
    let (x, y) = plot;
    let id = entity_id(unit);
    let (cmd, name) = if ranged {
        (
            Command::UnitRangeAttack { unit: id, x, y },
            "unit.range_attack",
        )
    } else {
        (Command::UnitMoveTo { unit: id, x, y }, "unit.move")
    };
    ctx.send(cmd, name, unit.to_js(), None);
}
