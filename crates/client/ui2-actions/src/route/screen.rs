//! Named actions of registered remote screens.
use crate::{intent::Intent, route::Ctx};
use protocol::Command;
use ui2_model::generated::{RefuseWhy, SCREEN_NAMES, Session, SessionOp as SessionOpName};

pub(crate) fn run(ctx: &mut Ctx<'_>, intent: &Intent) {
    let (Some(screen), Some(action)) = (intent.text("screen"), intent.text("action")) else {
        return;
    };
    if !SCREEN_NAMES.contains(&screen) {
        ctx.refuse(RefuseWhy::NoScreen);
        return;
    }
    let window = ctx.ui.ui().windows.remote(screen);
    let disband = screen == "confirm"
        && action == "choose"
        && window.is_some_and(|w| {
            w.open
                && w.popup_id.is_none()
                && w.a.is_some_and(|a| a < 0 && Some(a) == intent.int("a"))
        });
    if disband
        && ctx
            .ui
            .ui()
            .pending
            .iter()
            .any(|order| order.target.as_ref() == screen)
    {
        return;
    }
    let action = if disband { "disband" } else { action };
    let mut args: Vec<_> = ["a", "b", "c", "d"]
        .into_iter()
        .filter_map(|name| intent.int(name).map(|n| (name.to_owned(), i64::from(n))))
        .collect();
    if let Some(id) = window.and_then(|w| w.popup_id) {
        args.push(("popup_id".into(), i64::from(id)));
    }
    // A session operation to run once the server applied the action: checked now, so a refused one sends nothing.
    let then = match intent.variant("then").and_then(SessionOpName::from_u16) {
        None => None,
        Some(name) => {
            let Some(op) = super::session::op_of(name, None) else {
                ctx.refuse(RefuseWhy::NoSuchPlayer);
                return;
            };
            if let Some(why) = super::session::precheck(ctx.model.get::<Session>(), op) {
                ctx.refuse(why);
                return;
            }
            Some(op)
        }
    };
    let id = ctx.send(
        Command::ScreenAction {
            screen: screen.to_owned(),
            action: action.to_owned(),
            args,
            text: intent.text("text").map(str::to_owned),
        },
        "screen.act",
        screen.to_owned(),
        None,
    );
    if let (Some(id), Some(op)) = (id, then) {
        ctx.ui.after.push((id, op));
    }
}
