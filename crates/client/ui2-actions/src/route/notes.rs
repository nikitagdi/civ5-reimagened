//! `note.dismiss` and `note.activate`: what the player does with a notification.

use ui2_model::generated::{Map, Note, NoteGo, WindowId};
use ui2_model::{Key, KeyedList};

use crate::intent::Intent;
use crate::route::{Ctx, camera, unit, window};
use crate::validate::{self, entity_id};

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
        .cloned()
    else {
        tracing::debug!("ui2-actions: note {key} is not in the model");
        return;
    };
    match intent.name() {
        "note.dismiss" => dismiss(ctx, &note),
        "note.activate" => activate(ctx, &note),
        _ => {}
    }
}

/// Hide a notification, unless it waits for a choice of the player's (the core's `MayUserDismiss`): those go
/// when the choice is made.
fn dismiss(ctx: &mut Ctx<'_>, note: &Note) {
    if !note.dismissable {
        return;
    }
    // SIMPLIFIED(SIMP-252): a dismissed notification is remembered only by this page session and is not
    // told to the core, so it comes back after a reload. Original: the core keeps the dismissal.
    ctx.ui.dismissed.insert(note.id);
}

/// Do what the core's `CvNotifications::Activate` does for the note's type: open the window of the choice it
/// asks for (with the arguments the popup of that choice would give the window), select the unit it is about,
/// or by default look at its plot.
fn activate(ctx: &mut Ctx<'_>, note: &Note) {
    let plot = note.plot.as_ref().map(|p| (p.x, p.y));
    let open = |ctx: &mut Ctx<'_>, w: WindowId, a: Option<i32>, b: Option<i32>| {
        window::open_with(ctx, w, None, a, b);
    };
    match note.go {
        NoteGo::None => {}
        NoteGo::Look => look(ctx, plot),
        NoteGo::Capital => {
            let capital = ctx.model.get::<Map>().and_then(|m| {
                m.cities
                    .iter()
                    .find(|c| c.mine && c.capital)
                    .map(|c| (c.plot.x, c.plot.y))
            });
            look(ctx, capital);
        }
        NoteGo::Unit => {
            // The core's extra game data is the id of the seat's unit.
            let unit = ctx.model.get::<Map>().and_then(|m| {
                m.units
                    .iter()
                    .find(|u| u.mine && entity_id(u.key) == note.data2)
                    .map(|u| (u.key, (u.plot.x, u.plot.y)))
            });
            if let Some((key, at)) = unit {
                look(ctx, Some(at));
                unit::select(ctx, key);
            }
        }
        NoteGo::Production => {
            let Some(at) = plot else {
                return;
            };
            look(ctx, plot);
            if let Some(city) = own_city_at(ctx, at) {
                window::open(ctx, WindowId::Production, Some(city));
            }
        }
        // BUTTONPOPUP_CHOOSETECH: data2 is the game data, the free techs to choose.
        NoteGo::ChooseTech => open(
            ctx,
            WindowId::Techs,
            Some(if note.data1 > 0 { 2 } else { 1 }),
            None,
        ),
        // BUTTONPOPUP_TECH_AWARD: only when the core gave the tech (extra game data).
        NoteGo::TechAward => {
            if note.data2 != -1 {
                open(ctx, WindowId::TechAward, Some(note.data2), None);
            }
        }
        NoteGo::StealTech => open(ctx, WindowId::Techs, Some(3), Some(note.data1)),
        NoteGo::Policies => open(ctx, WindowId::Policies, None, None),
        NoteGo::DiploVote => open(ctx, WindowId::MpVote, Some(2), None),
        NoteGo::CityState => {
            look(ctx, plot);
            open(ctx, WindowId::CityState, Some(note.data1), Some(0));
        }
        // OpenPlayerDealScreen: the source gives the other player as data1.
        NoteGo::Deal => {
            if note.data1 >= 0 {
                open(ctx, WindowId::Deal, Some(note.data1), None);
            }
        }
        NoteGo::FreeGreatPerson => open(ctx, WindowId::FreeGreatPerson, Some(0), None),
        NoteGo::FaithGreatPerson => open(ctx, WindowId::FreeGreatPerson, Some(1), None),
        NoteGo::MayaBonus => open(ctx, WindowId::FreeGreatPerson, Some(2), None),
        NoteGo::Pantheon => open(ctx, WindowId::Pantheon, Some(0), None),
        NoteGo::Reformation => open(ctx, WindowId::Pantheon, Some(1), None),
        // BUTTONPOPUP_FOUND_RELIGION: the holy city is the city on the note's plot.
        NoteGo::FoundReligion | NoteGo::EnhanceReligion => {
            let city = plot.and_then(|at| own_city_at(ctx, at)).map(entity_id);
            let b = i32::from(note.go == NoteGo::EnhanceReligion);
            open(ctx, WindowId::Religion, city, Some(b));
        }
        NoteGo::Espionage => open(ctx, WindowId::Espionage, None, None),
        NoteGo::League => {
            if note.data1 >= 0 {
                open(ctx, WindowId::Congress, Some(0), None);
            }
        }
        NoteGo::LeagueProject => {
            if note.data1 >= 0 {
                open(ctx, WindowId::Congress, Some(2), Some(note.data2));
            }
        }
        NoteGo::Culture => {
            if note.data1 >= 0 {
                open(ctx, WindowId::Culture, None, None);
            }
        }
        NoteGo::Ideology => {
            if note.data1 >= 0 {
                open(ctx, WindowId::Ideology, None, None);
            }
        }
        NoteGo::Archaeology => {
            if note.data1 >= 0 {
                open(ctx, WindowId::Archaeology, None, None);
            }
        }
        NoteGo::GreatWork => open(ctx, WindowId::GreatWork, Some(note.data1), None),
    }
}

fn look(ctx: &mut Ctx<'_>, plot: Option<(i16, i16)>) {
    if let Some((x, y)) = plot {
        camera::to(ctx, x, y);
    }
}

/// The player's city on a plot.
fn own_city_at(ctx: &Ctx<'_>, at: (i16, i16)) -> Option<Key> {
    ctx.model
        .get::<Map>()
        .and_then(|m| {
            m.cities
                .iter()
                .find(|c| c.mine && (c.plot.x, c.plot.y) == at)
        })
        .map(|c| c.key)
        .filter(|c| validate::own_city(ctx.model, *c).is_some())
}
