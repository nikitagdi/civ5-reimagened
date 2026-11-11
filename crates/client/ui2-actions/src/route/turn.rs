//! `turn.primary`, `turn.end`, `turn.cancel` and `screen.open`: the turn button and the screens that live
//! outside the skin.

use protocol::Command;
use ui2_model::generated::{
    Game, Meta, Phase, RefuseWhy, ScreenId, Turn, TurnPending, TurnUi, WindowId,
};

use ui2_model::target::TURN_TARGET;

use crate::intent::Intent;
use crate::route::{Ctx, unit, window};

/// Run a `turn.*` or `screen.open` intent.
pub(crate) fn run(ctx: &mut Ctx<'_>, intent: &Intent) {
    match intent.name() {
        "turn.primary" => primary(ctx),
        "turn.end" => end(ctx, intent.flag("force").unwrap_or(false)),
        "turn.cancel" => cancel(ctx),
        "screen.open" => {
            if let Some(screen) = intent.variant("screen").and_then(ScreenId::from_u16) {
                open_screen(ctx, screen);
            }
        }
        _ => {}
    }
}

/// What the turn button does now, by the state the source computed for it.
fn primary(ctx: &mut Ctx<'_>) {
    let Some(turn) = ctx.model.get::<Turn>() else {
        ctx.refuse(RefuseWhy::NoGame);
        return;
    };
    match turn.state {
        TurnUi::NextTurn => end(ctx, false),
        TurnUi::UnitNeedsOrders => unit::select_next(ctx),
        TurnUi::CityNeedsProduction => {
            let city = turn.blocker.as_ref().and_then(|b| b.city);
            window::open(ctx, WindowId::Production, city);
        }
        TurnUi::ResearchNeeded => window::open(ctx, WindowId::Research, None),
        TurnUi::PolicyNeeded => open_screen(ctx, ScreenId::Policies),
        // The blockers the skin has no window for (a vote, a free item, a religion step) show up as
        // notifications; the log is where the player finds out which.
        TurnUi::OtherBlocker => open_screen(ctx, ScreenId::NotificationLog),
        TurnUi::Ended => cancel(ctx),
        TurnUi::WaitingOthers | TurnUi::AiMoving | TurnUi::GameOver => {
            ctx.refuse(RefuseWhy::CannotEndTurn);
        }
    }
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
    if !ctx.sink.open_screen(screen) {
        ctx.refuse(RefuseWhy::NoScreen);
    }
}
