//! The sounds the exe plays on game events: popups (`ButtonPopupSounds.xml`) and the seat's own new units and cities
//! (`EventSoundConnections.xml`). The notification, selection and ambience sounds have their tests with their features.
mod fake;

use exe_api::UiEventKind as K;
use fake::{EvExt, FakeGame, city_block, ev, rig_with_game, unit_block, with_block};

fn game() -> FakeGame {
    FakeGame {
        attach_script: "ButtonPopupTypes = { BUTTONPOPUP_NEW_ERA = 61, BUTTONPOPUP_TEXT = 1 }
             Game = Game or {}
             Game.GetActivePlayer = function() return 0 end"
            .into(),
        ..FakeGame::default()
    }
}

#[test]
fn a_popup_asks_for_its_sound_by_its_type_name() {
    let mut r = rig_with_game(Default::default(), game());
    r.game
        .borrow_mut()
        .events
        .push(ev(K::Popup, &[61, 0, 0, 0, 0, 0, 0]).with_text("era"));
    r.game
        .borrow_mut()
        .events
        .push(ev(K::Popup, &[999, 0, 0, 0, 0, 0, 0]).with_text("unknown type"));
    r.exe.dispatch(&r.rt);
    assert_eq!(*r.host.popup_sounds.borrow(), ["BUTTONPOPUP_NEW_ERA"]);
}

#[test]
fn the_seats_new_unit_and_city_make_their_event_sound_but_not_the_others() {
    let mut r = rig_with_game(Default::default(), game());
    // The seat is player 0.
    r.game
        .borrow_mut()
        .events
        .push(with_block(K::UnitCreated, unit_block(0, 1, 5, 5), &[]));
    r.game
        .borrow_mut()
        .events
        .push(with_block(K::UnitCreated, unit_block(3, 2, 6, 5), &[]));
    r.game
        .borrow_mut()
        .events
        .push(with_block(K::CityCreated, city_block(0, 7, 4, 4, 1), &[2]));
    r.game
        .borrow_mut()
        .events
        .push(with_block(K::CityCreated, city_block(3, 8, 9, 9, 1), &[2]));
    r.exe.dispatch(&r.rt);
    assert_eq!(
        *r.host.event_sounds.borrow(),
        ["UnitCreated", "CityCreated"]
    );
}

#[test]
fn the_announcement_of_a_loaded_game_is_silent() {
    let mut r = rig_with_game(Default::default(), game());
    let mut unit = with_block(K::UnitCreated, unit_block(0, 1, 5, 5), &[]);
    unit.rebuild = true;
    let mut city = with_block(K::CityCreated, city_block(0, 7, 4, 4, 1), &[2]);
    city.rebuild = true;
    r.game.borrow_mut().events.push(unit);
    r.game.borrow_mut().events.push(city);
    r.exe.dispatch(&r.rt);
    assert!(r.host.event_sounds.borrow().is_empty());
}

/// A reload of the page replays the turn's notifications and popups while the replica catches up: no sound then; the
/// news after it sound as before.
#[test]
fn what_the_join_replays_is_silent_and_the_news_after_it_are_not() {
    let mut g = game();
    g.attach_script
        .push_str(" NotificationTypes = { NOTIFICATION_GOODY = 7 }");
    let mut r = rig_with_game(Default::default(), g);
    let note = |id| {
        ev(K::NotificationAdded, &[id, 7, 1, 1, 0, 0, 0])
            .with_text("Ruins")
            .with_text2("Ruins")
    };
    r.game.borrow_mut().catching_up = true;
    for id in 1..=5 {
        r.game.borrow_mut().events.push(note(id));
    }
    r.game
        .borrow_mut()
        .events
        .push(ev(K::Popup, &[61, 0, 0, 0, 0, 0, 0]).with_text("era"));
    r.game
        .borrow_mut()
        .events
        .push(with_block(K::UnitCreated, unit_block(0, 1, 5, 5), &[]));
    r.exe.dispatch(&r.rt);
    // The queue drained in this frame: the frame after still holds.
    r.game.borrow_mut().catching_up = false;
    r.game.borrow_mut().events.push(note(6));
    r.exe.dispatch(&r.rt);
    assert!(r.host.notification_sounds.borrow().is_empty());
    assert!(r.host.popup_sounds.borrow().is_empty());
    assert!(r.host.event_sounds.borrow().is_empty());
    // A new one, live: heard.
    r.game.borrow_mut().events.push(note(7));
    r.game
        .borrow_mut()
        .events
        .push(ev(K::Popup, &[61, 0, 0, 0, 0, 0, 0]).with_text("era"));
    r.exe.dispatch(&r.rt);
    assert_eq!(*r.host.notification_sounds.borrow(), ["NOTIFICATION_GOODY"]);
    assert_eq!(*r.host.popup_sounds.borrow(), ["BUTTONPOPUP_NEW_ERA"]);
}

/// The "look here" sparkle sounds with its effect (live report 2026-11-01: the sparkle had no sound), but not while the
/// replica joins.
#[test]
fn the_look_here_effect_makes_its_sounds() {
    let mut r = rig_with_game(Default::default(), game());
    r.game.borrow_mut().events.push(ev(K::DoFx, &[6, 3, -1]));
    // `GameplayDoFX(NULL)` has no plot: no effect, no sound.
    r.game.borrow_mut().events.push(ev(K::DoFx, &[-1, -1, -1]));
    r.exe.dispatch(&r.rt);
    assert_eq!(
        *r.host.effect_sounds.borrow(),
        ["AS2D_WORLD_RAY_OF_GOD", "AS2D_BUILD_UNIT"]
    );
    r.host.effect_sounds.borrow_mut().clear();
    r.game.borrow_mut().catching_up = true;
    r.game.borrow_mut().events.push(ev(K::DoFx, &[6, 3, -1]));
    r.exe.dispatch(&r.rt);
    assert!(r.host.effect_sounds.borrow().is_empty());
}

/// A catch-up that never ends (a replica stuck out of the live state) held every notification sound for the rest of the
/// turn after a reload of the page (live report 2026-11-01): it holds them for a join's length only.
#[test]
fn a_catch_up_that_does_not_end_stops_holding_the_sounds() {
    let mut g = game();
    g.attach_script
        .push_str(" NotificationTypes = { NOTIFICATION_GOODY = 7 }");
    let mut r = rig_with_game(Default::default(), g);
    r.game.borrow_mut().catching_up = true;
    for _ in 0..(exe_api::SOUND_QUIET_MAX_FRAMES + 5) {
        r.exe.dispatch(&r.rt);
    }
    r.game.borrow_mut().events.push(
        ev(K::NotificationAdded, &[1, 7, 1, 1, 0, 0, 0])
            .with_text("Ruins")
            .with_text2("Ruins"),
    );
    r.exe.dispatch(&r.rt);
    assert_eq!(*r.host.notification_sounds.borrow(), ["NOTIFICATION_GOODY"]);
}

fn voice_game() -> FakeGame {
    let mut g = game();
    g.attach_script.push_str(
        r#"
        ButtonPopupTypes.BUTTONPOPUP_TECH_AWARD = 68
        ButtonPopupTypes.BUTTONPOPUP_WONDER_COMPLETED_ACTIVE_PLAYER = 69
        ButtonPopupTypes.BUTTONPOPUP_CITY_STATE_GREETING = 70
        Game.GetGameTurn = function() return 3 end
        GameInfo = {
            Technologies = { [5] = { AudioIntro = "AS2D_TECH_X", AudioIntroHeader = "AS2D_HEADING_TECH_X" } },
            Buildings = { [9] = { WonderSplashAudio = "AS2D_WONDER_SPEECH_Y" } },
            Eras = { [2] = { Type = "ERA_CLASSICAL" } },
            Era_NewEraVOs = function(f)
                local rows = {}
                if f.EraType == "ERA_CLASSICAL" then rows = { { VOScript = "AS2D_ERA_1" }, { VOScript = "AS2D_ERA_2" } } end
                local i = 0
                return function() i = i + 1; return rows[i] end
            end,
            MinorCivilizations = { [3] = { Type = "MINOR_CIV_ALMATY" } },
        }
        Players = { [22] = { IsMinorCiv = function() return true end, GetMinorCivType = function() return 3 end } }
        "#,
    );
    g
}

/// re-b15 ITEM 5 P5.2: the exe speaks after the popups of a new era, a tech and a wonder of the seat
/// (`OnButtonPopupDisplayShow`), and a city-state's popup speaks its greeting instead of the table sound.
#[test]
fn the_era_tech_and_wonder_popups_speak_their_line_and_a_city_state_greets() {
    let mut r = rig_with_game(Default::default(), voice_game());
    for args in [
        [68, 0, 0, 5, 0, 0, 0],
        [69, 9, 0, 0, 0, 0, 0],
        [61, 2, 0, 0, 0, 0, 0],
        [70, 22, 0, 0, 0, 0, 0],
    ] {
        r.game
            .borrow_mut()
            .events
            .push(ev(K::Popup, &args).with_text("popup"));
    }
    r.exe.dispatch(&r.rt);
    assert_eq!(
        *r.host.voices.borrow(),
        [
            // (a multiplayer game, as the port's are: the tech's `AudioIntroHeader`, the exe's `GetSoundMP`)
            ("AS2D_HEADING_TECH_X".to_owned(), 1000),
            ("AS2D_WONDER_SPEECH_Y".to_owned(), 0),
            // Of the era's two lines the turn (3) picks the second.
            ("AS2D_ERA_2".to_owned(), 1000),
        ]
    );
    assert_eq!(*r.host.minor_voices.borrow(), ["MINOR_CIV_ALMATY"]);
    assert_eq!(
        *r.host.popup_sounds.borrow(),
        [
            "BUTTONPOPUP_TECH_AWARD",
            "BUTTONPOPUP_WONDER_COMPLETED_ACTIVE_PLAYER",
            "BUTTONPOPUP_NEW_ERA"
        ],
        "the greeting replaces the city-state popup's table sound"
    );
}

/// re-b15 ITEM 4: the port does not animate a move, an attack or a death; the units the seat sees cue the clips the
/// exe would play, so their sound rows are heard.
#[test]
fn a_move_a_fight_and_a_death_the_seat_sees_cue_the_units_clips() {
    let mut r = rig_with_game(Default::default(), game());
    let mut hidden = unit_block(4, 9, 1, 1);
    hidden[13] = 0;
    for e in [
        with_block(K::UnitCreated, unit_block(0, 1, 5, 5), &[]),
        with_block(K::UnitCreated, unit_block(3, 2, 6, 5), &[]),
        with_block(K::UnitCreated, hidden.clone(), &[]),
        with_block(K::UnitMoved, unit_block(0, 1, 6, 6), &[]),
        with_block(K::UnitMoved, hidden, &[]),
        // attacker unit (0, 1), defender unit (3, 2), no interceptor, at (6, 5), shown and struck back
        ev(
            K::UnitCombat,
            &[1, 0, 1, 1, 3, 2, 0, -1, -1, 6, 5, 2 | 4, 0],
        ),
        // a fight the core does not show
        ev(K::UnitCombat, &[1, 0, 1, 1, 3, 2, 0, -1, -1, 6, 5, 0, 0]),
        with_block(K::UnitDestroyedInCombat, unit_block(3, 2, 6, 5), &[]),
    ] {
        r.game.borrow_mut().events.push(e);
    }
    r.exe.dispatch(&r.rt);
    let cues: Vec<_> = r
        .host
        .unit_cues
        .borrow()
        .iter()
        .map(|c| (c.owner, c.id, c.ec, (c.x, c.y), c.opponent))
        .collect();
    assert_eq!(
        cues,
        [
            (0, 1, exe_api::EC_RUN, (6, 6), None),
            (0, 1, exe_api::EC_ATTACK, (6, 5), Some((3, 2))),
            (3, 2, exe_api::EC_ATTACK, (6, 5), Some((0, 1))),
            (3, 2, exe_api::EC_DEATH, (6, 5), None),
        ]
    );
}

/// re-b15 ITEM 3: the music hears a leader speak (`AILeaderMessage`), the leader screen close and the game over screen.
#[test]
fn the_music_hears_a_leader_screen_and_its_end() {
    use exe_api::music::MusicEvent;
    let mut r = rig_with_game(Default::default(), game());
    r.exec("Events.AILeaderMessage(3, 1, 'hello', 0, 0) UI.RequestLeaveLeader()");
    r.game
        .borrow_mut()
        .events
        .push(ev(K::DiploAiLeaderMessage, &[5, 1, 0, 0]).with_text("x"));
    r.exe.dispatch(&r.rt);
    let got = r.host.music_events.borrow().clone();
    assert!(got.contains(&MusicEvent::LeaderView(3)), "{got:?}");
    assert!(got.contains(&MusicEvent::LeaveLeaderView), "{got:?}");
    assert!(got.contains(&MusicEvent::LeaderView(5)), "{got:?}");
}

/// re-b15 ITEM 3: what the music reads once a second, through the Lua API of the game.
#[test]
fn the_music_reads_the_active_civ_its_era_and_who_it_met_and_fights() {
    let r = rig_with_game(Default::default(), game());
    r.ctx
        .lua()
        .exec(
            "local function pl(id, team, civ, leader, minor)
               return { GetTeam = function() return team end, IsEverAlive = function() return true end,
                 IsAlive = function() return true end, IsMinorCiv = function() return minor end,
                 IsBarbarian = function() return false end, IsHuman = function() return id == 0 end,
                 GetCurrentEra = function() return 3 end, GetCivilizationType = function() return civ end,
                 GetLeaderType = function() return leader end }
             end
             Players = { [0] = pl(0, 0, 0, 0, false), [1] = pl(1, 1, 1, 1, false), [2] = pl(2, 2, 2, 2, true) }
             Teams = { [0] = { IsHasMet = function(_, t) return t == 1 end, IsAtWar = function(_, t) return t == 2 end } }
             GameDefines = { MAX_CIV_PLAYERS = 3 }
             GameInfo = GameInfo or {}
             GameInfo.Eras = { ERA_RENAISSANCE = { ID = 3 } }
             GameInfo.Civilizations = { [0] = { Type = 'CIVILIZATION_AMERICA' },
               [1] = { Type = 'CIVILIZATION_POLYNESIA', SoundtrackTag = 'Aztec' }, [2] = { Type = 'MINOR_CIV_X' } }
             GameInfo.Leaders = { [0] = { Type = 'LEADER_WASHINGTON' }, [1] = { Type = 'LEADER_KAMEHAMEHA' },
               [2] = { Type = 'LEADER_MINOR' } }",
            "=dll",
        )
        .unwrap();
    let st = r.exe.music_state().expect("a state");
    assert_eq!((st.active, st.human, st.era, st.start_era), (0, true, 3, 3));
    assert_eq!(st.leader_up, Some(false), "the leader screen is down");
    assert_eq!(st.players.len(), 3);
    let p1 = &st.players[1];
    assert_eq!(
        (
            p1.leader.as_str(),
            p1.civ_type.as_str(),
            p1.soundtrack.as_str()
        ),
        ("LEADER_KAMEHAMEHA", "CIVILIZATION_POLYNESIA", "Aztec")
    );
    assert!(p1.met && !p1.at_war && p1.major_alive);
    let p2 = &st.players[2];
    assert!(
        !p2.met && p2.at_war && !p2.major_alive,
        "a city-state at war"
    );
    assert!(st.players[0].met && !st.players[0].at_war);
}
