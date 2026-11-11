//! `note.dismiss` and `note.activate`: what the player does with a notification.

use ui2_model::generated::{Note, NoteCat, WindowId};
use ui2_model::{Key, KeyedList};

use crate::intent::Intent;
use crate::route::{Ctx, camera, window};
use crate::validate;

/// Run a `note.*` intent. A note the model does not have (it went while the click was on its way) is passed
/// over: there is nothing to tell the player.
pub(crate) fn run(ctx: &mut Ctx<'_>, intent: &Intent) {
    let Some(key) = intent.key("note") else {
        return;
    };
    let Some(note) = ctx
        .model
        .get::<KeyedList<Note>>()
        .and_then(|notes| notes.get(key))
    else {
        tracing::debug!("ui2-actions: note {key} is not in the model");
        return;
    };
    match intent.name() {
        "note.dismiss" => dismiss(ctx, key),
        "note.activate" => activate(ctx, note),
        _ => {}
    }
}

/// Hide a notification.
fn dismiss(ctx: &mut Ctx<'_>, note: Key) {
    // SIMPLIFIED(SIMP-252): a dismissed notification is remembered only by this page session and is not
    // told to the core, so it comes back after a reload. Original: the core keeps the dismissal.
    ctx.ui.dismissed.insert(note);
}

/// Look at what the note is about: the camera goes to its plot; a production note of a city of the player's
/// opens the production window of that city.
fn activate(ctx: &mut Ctx<'_>, note: &Note) {
    let Some(plot) = &note.plot else {
        return;
    };
    camera::to(ctx, plot.x, plot.y);
    if note.category != NoteCat::Production {
        return;
    }
    let city = ctx
        .model
        .get::<ui2_model::generated::Map>()
        .and_then(|m| {
            m.cities
                .iter()
                .find(|c| c.mine && (c.plot.x, c.plot.y) == (plot.x, plot.y))
        })
        .map(|c| c.key)
        .filter(|c| validate::own_city(ctx.model, *c).is_some());
    if let Some(city) = city {
        window::open(ctx, WindowId::Production, Some(city));
    }
}
