//! `chat.send`: one chat line, to everybody or to one player.

use ui2_model::KeyedList;
use ui2_model::generated::{ChatTo as To, PlayerRow, RefuseWhy};

use crate::intent::Intent;
use crate::route::Ctx;
use crate::validate::entity_id;

/// The longest line, in characters.
pub const MAX_CHARS: usize = 500;
/// The shortest time between two lines, in milliseconds.
pub const MIN_GAP_MS: u64 = 500;

/// Run a `chat.send` intent. The text must have something in it, at most [`MAX_CHARS`] characters (it is not
/// cut: the player is told), and a line may not follow another by less than [`MIN_GAP_MS`]. The gap counts
/// from the last line that was sent.
pub(crate) fn send(ctx: &mut Ctx<'_>, intent: &Intent) {
    let (Some(text), Some(to)) = (
        intent.text("text"),
        intent.variant("to").and_then(To::from_u16),
    ) else {
        return;
    };
    if text.trim().is_empty() {
        ctx.refuse(RefuseWhy::ChatEmpty);
        return;
    }
    if text.chars().count() > MAX_CHARS {
        ctx.refuse(RefuseWhy::ChatTooLong);
        return;
    }
    if let Some(last) = ctx.ui.last_chat_ms
        && ctx.now_ms.saturating_sub(last) < MIN_GAP_MS
    {
        ctx.refuse(RefuseWhy::ChatRate);
        return;
    }
    let to = match to {
        To::All => protocol::ChatTo::All,
        To::Player => {
            let player = intent.key("player").or(ctx.ui.ui().chat.to_player);
            let id = player.and_then(|p| {
                let known = ctx
                    .model
                    .get::<KeyedList<PlayerRow>>()
                    .is_none_or(|l| l.get(p).is_some());
                known.then(|| i8::try_from(entity_id(p)).ok()).flatten()
            });
            match id {
                Some(id) => protocol::ChatTo::Player(id),
                None => {
                    ctx.refuse(RefuseWhy::Other);
                    return;
                }
            }
        }
    };
    match ctx.sink.chat(to, text) {
        Ok(()) => ctx.ui.last_chat_ms = Some(ctx.now_ms),
        Err(refused) => ctx.refuse(refused.why()),
    }
}
