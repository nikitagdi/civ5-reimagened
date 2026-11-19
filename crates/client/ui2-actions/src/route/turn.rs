//! `turn.primary`, `turn.end`, `turn.cancel`, `turn.blocker` and `screen.open`: the turn button, the list of what
//! blocks the end of turn, and the screens that live outside the skin.

use protocol::Command;
use ui2_model::Key;
use ui2_model::generated::{
    EndTurnBlock, Game, Map, Meta, Phase, Plot, RefuseWhy, ScreenId, Selection, TargetKind, Turn,
    TurnBlocker, TurnPending, TurnUi, WindowId,
};

use ui2_model::target::TURN_TARGET;

use crate::intent::Intent;
use crate::route::{Ctx, camera, unit, window};
use crate::validate::entity_id;

/// Run a `turn.*` or `screen.open` intent.
pub(crate) fn run(ctx: &mut Ctx<'_>, intent: &Intent) {
    match intent.name() {
        "turn.primary" => primary(ctx),
        "turn.end" => end(ctx, intent.flag("force").unwrap_or(false)),
        "turn.cancel" => cancel(ctx),
        "turn.blocker" => blocker(ctx, intent.int("seq")),
        "screen.open" => {
            if let Some(screen) = intent.variant("screen").and_then(ScreenId::from_u16) {
                open_screen(ctx, screen);
            }
        }
        _ => {}
    }
}

/// What the turn button does now, by the state the source computed for it: with a blocker, go to the first entry
/// of `turn.blockers` (the core's own blocker); with none, end the turn.
fn primary(ctx: &mut Ctx<'_>) {
    let Some(turn) = ctx.model.get::<Turn>() else {
        ctx.refuse(RefuseWhy::NoGame);
        return;
    };
    match turn.state {
        TurnUi::NextTurn => end(ctx, false),
        TurnUi::Ended => cancel(ctx),
        TurnUi::WaitingOthers | TurnUi::AiMoving | TurnUi::GameOver => {
            ctx.refuse(RefuseWhy::CannotEndTurn);
        }
        state => match turn.blockers.iter().next().cloned() {
            Some(first) => go(ctx, &first),
            None => by_state(ctx, state, turn.blocker.as_ref().and_then(|b| b.city)),
        },
    }
}

/// A source without the list of blockers: the button's state alone says where to go.
fn by_state(ctx: &mut Ctx<'_>, state: TurnUi, city: Option<Key>) {
    match state {
        TurnUi::UnitNeedsOrders => unit::select_next(ctx),
        TurnUi::CityNeedsProduction => window::open(ctx, WindowId::Production, city),
        TurnUi::ResearchNeeded => window::open(ctx, WindowId::Research, None),
        TurnUi::PolicyNeeded => open_screen(ctx, ScreenId::Policies),
        _ => open_screen(ctx, ScreenId::NotificationLog),
    }
}

/// `turn.blocker`: one entry of `turn.blockers` or `turn.reminders`.
fn blocker(ctx: &mut Ctx<'_>, seq: Option<i32>) {
    let Some(turn) = ctx.model.get::<Turn>() else {
        ctx.refuse(RefuseWhy::NoGame);
        return;
    };
    let found = seq.and_then(|s| {
        turn.blockers
            .iter()
            .chain(turn.reminders.iter())
            .find(|e| e.seq == s)
            .cloned()
    });
    match found {
        Some(e) => go(ctx, &e),
        // The list changed under the click (the entry was dealt with): the toast says so.
        None => ctx.refuse(RefuseWhy::TurnChanged),
    }
}

/// Go to what an entry needs, as its notification does in the original: select the unit and look at it, open the
/// city's production, or open the window of the choice with its arguments.
fn go(ctx: &mut Ctx<'_>, e: &TurnBlocker) {
    if let Some(unit) = e.unit {
        unit::select(ctx, unit);
        if let Some(p) = &e.plot {
            camera::to(ctx, p.x, p.y);
        }
        return;
    }
    let city = e.city.or_else(|| own_city_at(ctx, e.plot.as_ref()));
    match e.kind {
        EndTurnBlock::Production => {
            window::open(ctx, WindowId::Production, city);
            return;
        }
        // The original's choose-tech popup is the HUD's research panel here.
        EndTurnBlock::Research => {
            window::open(ctx, WindowId::Research, None);
            return;
        }
        // The religion screen takes the city of the Great Prophet by its id.
        EndTurnBlock::FoundReligion | EndTurnBlock::EnhanceReligion
            if ctx.ui.ui().windows.remote("religion").is_some() =>
        {
            let id = city.map(entity_id);
            window::open_with(ctx, WindowId::Religion, None, id, e.b);
            return;
        }
        // The original's blocker selects the city and starts its ranged strike (INTERFACEMODE_CITY_RANGE_ATTACK).
        EndTurnBlock::CityRangeAttack if city.is_some() => {
            if let Some(p) = &e.plot {
                camera::to(ctx, p.x, p.y);
            }
            ctx.ui.edit().selection = Selection {
                unit: None,
                city,
                plot: e.plot.clone(),
            };
            super::map::enter(ctx, TargetKind::City, None, city, None);
            return;
        }
        // Units the replica has no entry for: the next unit that waits.
        EndTurnBlock::Units
        | EndTurnBlock::StackedUnits
        | EndTurnBlock::UnitNeedsOrders
        | EndTurnBlock::UnitPromotion => {
            unit::select_next(ctx);
            return;
        }
        _ => {}
    }
    if let Some(name) = e.window.as_deref()
        && let Some(window) = WindowId::from_name(name)
        && ctx.ui.ui().windows.remote(name).is_some()
    {
        window::open_with(ctx, window, None, e.a, e.b);
        return;
    }
    // A skin without that window: the panel of the HUD that has the choice, else the plot, else the log.
    match e.kind {
        EndTurnBlock::Research | EndTurnBlock::FreeTech | EndTurnBlock::StealTech => {
            window::open(ctx, WindowId::Research, None);
        }
        EndTurnBlock::Policy | EndTurnBlock::FreePolicy | EndTurnBlock::Ideology => {
            open_screen(ctx, ScreenId::Policies);
        }
        _ => match &e.plot {
            Some(p) => camera::to(ctx, p.x, p.y),
            None => open_screen(ctx, ScreenId::NotificationLog),
        },
    }
}

/// The player's city on a plot, from the map's banners.
fn own_city_at(ctx: &Ctx<'_>, plot: Option<&Plot>) -> Option<Key> {
    let p = plot?;
    ctx.model
        .get::<Map>()?
        .cities
        .iter()
        .find(|c| c.mine && c.plot.x == p.x && c.plot.y == p.y)
        .map(|c| c.key)
}

/// End the turn. Refused unless the game is connected, the humans are moving and the player has not ended
/// the turn yet. A second order while one is waiting is dropped without a word: the button already shows it.
fn end(ctx: &mut Ctx<'_>, force: bool) {
    let (Some(meta), Some(game), Some(turn)) = (
        ctx.model.get::<Meta>(),
        ctx.model.get::<Game>(),
        ctx.model.get::<Turn>(),
    ) else {
        ctx.refuse(RefuseWhy::NoGame);
        return;
    };
    if !meta.connected {
        ctx.refuse(RefuseWhy::NoConnection);
        return;
    }
    if turn.finished || game.phase != Phase::Humans {
        ctx.refuse(RefuseWhy::CannotEndTurn);
        return;
    }
    if ctx.ui.ui().turn_pending != TurnPending::None {
        return;
    }
    ctx.send(
        Command::EndTurn { force },
        "turn.end",
        TURN_TARGET.into(),
        Some(TurnPending::Ending),
    );
}

/// Take the end of turn back, while the other humans have not all ended theirs.
fn cancel(ctx: &mut Ctx<'_>) {
    let Some(turn) = ctx.model.get::<Turn>() else {
        ctx.refuse(RefuseWhy::NoGame);
        return;
    };
    if !turn.can_cancel {
        ctx.refuse(RefuseWhy::CannotCancel);
        return;
    }
    if ctx.ui.ui().turn_pending != TurnPending::None {
        return;
    }
    ctx.send(
        Command::CancelEndTurn,
        "turn.cancel",
        TURN_TARGET.into(),
        Some(TurnPending::Cancelling),
    );
}

/// Open a screen of the host (outside the skin); the toast says when the host has no way to.
fn open_screen(ctx: &mut Ctx<'_>, screen: ScreenId) {
    if let Some(window) = WindowId::from_name(screen.as_str())
        && ctx.ui.ui().windows.remote(screen.as_str()).is_some()
    {
        window::open(ctx, window, None);
        return;
    }

    if !ctx.sink.open_screen(screen) {
        ctx.refuse(RefuseWhy::NoScreen);
    }
}
