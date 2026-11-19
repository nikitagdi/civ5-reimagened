//! `window.open`, `window.close`, `window.toggle`: the windows of the skin, `ui.windows.<w>.open`. No network:
//! these change the `ui` root only and take effect in the frame they are applied.

use ui2_model::Key;
use ui2_model::generated::{Map, RefuseWhy, ToastKind, WindowId, Windows};

use crate::intent::Intent;
use crate::route::Ctx;

/// Run a `window.*` intent.
pub(crate) fn run(ctx: &mut Ctx<'_>, intent: &Intent) {
    let Some(window) = intent.variant("window").and_then(WindowId::from_u16) else {
        return;
    };
    if held_by_decision(ctx, window) {
        return;
    }
    match intent.name() {
        "window.open" => {
            open_with(
                ctx,
                window,
                intent.key("city"),
                intent.int("a"),
                intent.int("b"),
            );
        }
        "window.close" => close(&mut ctx.ui.edit().windows, window),
        "window.toggle" => {
            if is_open(&ctx.ui.ui().windows, window) {
                close(&mut ctx.ui.edit().windows, window);
            } else {
                open(ctx, window, intent.key("city"));
            }
        }
        _ => {}
    }
}

/// A server window that shows a decision the player owes (a popup waiting for an answer): no intent replaces it.
fn held_by_decision(ctx: &Ctx<'_>, window: WindowId) -> bool {
    ctx.ui
        .ui()
        .windows
        .remote(window.as_str())
        .is_some_and(|w| w.open && w.popup_id.is_some() && w.popup_decision)
}

/// Open a window with the arguments of a server window. A server window that shows a popup is left as it is.
pub(crate) fn open_with(
    ctx: &mut Ctx<'_>,
    window: WindowId,
    city: Option<Key>,
    a: Option<i32>,
    b: Option<i32>,
) {
    if held_by_decision(ctx, window)
        || ctx
            .ui
            .ui()
            .windows
            .remote(window.as_str())
            .is_some_and(|w| w.popup_id.is_some())
    {
        return;
    }
    open(ctx, window, city);
    if let Some(w) = ctx.ui.edit().windows.remote_mut(window.as_str()) {
        w.a = a;
        w.b = b;
    }
}

/// Open a window. The production window belongs to one of the player's cities: the one given, or else the
/// selected one.
pub(crate) fn open(ctx: &mut Ctx<'_>, window: WindowId, city: Option<Key>) {
    if window == WindowId::Production {
        let city = city.or(ctx.ui.ui().selection.city);
        let Some(city) = city.filter(|c| own_city(ctx, *c)) else {
            ctx.ui
                .toast(ToastKind::Refused, RefuseWhy::NotYourCity, None, ctx.now_ms);
            return;
        };
        let w = &mut ctx.ui.edit().windows.production;
        w.open = true;
        w.city = Some(city);
        return;
    }
    set_open(&mut ctx.ui.edit().windows, window, true);
}

fn own_city(ctx: &Ctx<'_>, city: Key) -> bool {
    ctx.model
        .get::<Map>()
        .and_then(|m| m.cities.get(city))
        .is_some_and(|c| c.mine)
}

fn close(w: &mut Windows, window: WindowId) {
    set_open(w, window, false);
    if window == WindowId::Production {
        w.production.city = None;
    }
}

fn set_open(w: &mut Windows, window: WindowId, open: bool) {
    match window {
        WindowId::Production => w.production.open = open,
        WindowId::Research => w.research.open = open,
        WindowId::Menu => w.menu.open = open,
        WindowId::Chat => w.chat.open = open,
        WindowId::NotesLog => w.notes_log.open = open,
        remote => {
            if let Some(w) = w.remote_mut(remote.as_str()) {
                w.open = open;
            }
        }
    }
}

fn is_open(w: &Windows, window: WindowId) -> bool {
    match window {
        WindowId::Production => w.production.open,
        WindowId::Research => w.research.open,
        WindowId::Menu => w.menu.open,
        WindowId::Chat => w.chat.open,
        WindowId::NotesLog => w.notes_log.open,
        remote => w.remote(remote.as_str()).is_some_and(|w| w.open),
    }
}
