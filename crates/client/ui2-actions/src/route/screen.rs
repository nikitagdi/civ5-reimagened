//! Named actions of registered remote screens.
use crate::{intent::Intent, route::Ctx};
use protocol::Command;
use ui2_model::generated::{RefuseWhy, SCREEN_NAMES};

pub(crate) fn run(ctx: &mut Ctx<'_>, intent: &Intent) {
    let (Some(screen), Some(action)) = (intent.text("screen"), intent.text("action")) else {
        return;
    };
    if !SCREEN_NAMES.contains(&screen) {
        ctx.refuse(RefuseWhy::NoScreen);
        return;
    }
    let args = ["a", "b", "c", "d"]
        .into_iter()
        .filter_map(|name| intent.int(name).map(|n| (name.to_owned(), i64::from(n))))
        .collect();
    ctx.send(
        Command::ScreenAction {
            screen: screen.to_owned(),
            action: action.to_owned(),
            args,
        },
        "screen.act",
        screen.to_owned(),
        None,
    );
}
