//! The roots `map`, `notes` and `chat`: the reference HUD's city banners (Moscow 14 building Machu Picchu, Novgorod
//! 8, Kazan 5) and unit flags, a foreign city in the fog showing only its name and plot, the four notifications
//! with their categories, a notification dismissed here left out without reading the game, and the chat with the
//! turn each line came in.

mod common;

use common::*;
use protocol::{ChatLine, ChatTo};
use ui2_model::r#gen::{BannerProd, ChatRow, CityBanner, Domain, Map, Note, NoteCat, Plot, TurnUi};
use ui2_model::{AssetRef, Key, KeyedList, Text};

fn map(rig: &Rig) -> Map {
    rig.store.get::<Map>().cloned().expect("map is written")
}

fn notes(rig: &Rig) -> KeyedList<Note> {
    rig.store
        .get::<KeyedList<Note>>()
        .cloned()
        .expect("notes are written")
}

fn chat(rig: &Rig) -> KeyedList<ChatRow> {
    rig.store
        .get::<KeyedList<ChatRow>>()
        .cloned()
        .expect("chat is written")
}

fn city_key(owner: i8, id: i32) -> Key {
    Key::new(u32::from(owner.cast_unsigned()), id.cast_unsigned())
}

#[test]
fn the_reference_city_banners() {
    let mut f = game();
    let mut rig = Rig::new(session());
    rig.frame(&mut f);
    let m = map(&rig);
    let keys: Vec<Key> = m.cities.keys().collect();
    assert_eq!(
        keys,
        [
            city_key(CATHERINE, MOSCOW),
            city_key(CATHERINE, NOVGOROD),
            city_key(CATHERINE, KAZAN),
            city_key(HARUN, BAGHDAD),
            city_key(MONTEZUMA, TENOCHTITLAN),
        ]
    );
    assert_eq!(
        m.cities[0],
        CityBanner {
            key: city_key(CATHERINE, MOSCOW),
            owner: Key::from_i32(0),
            color: 0x8b00_00ff,
            color2: 0xffd7_00ff,
            name: "Moscow".into(),
            plot: Plot { x: 40, y: 20 },
            size: 14,
            capital: true,
            mine: true,
            visible: true,
            puppet: false,
            razing: false,
            occupied: false,
            resistance: false,
            can_shoot: true,
            strength: 31,
            hp_fraction: 0.75,
            growth_turns: Some(4),
            production: Some(BannerProd {
                item: Key::from_u32((1 << 16) | 40),
                name: Text::raw("Machu Picchu"),
                icon: AssetRef::new("icon:BW_ATLAS_2:4"),
                turns: Some(6),
                fraction: 0.429,
            }),
        }
    );
    let novgorod = &m.cities[1];
    assert_eq!((novgorod.size, novgorod.capital), (8, false));
    assert_eq!(
        novgorod
            .production
            .as_ref()
            .map(|p| (p.name.clone(), p.turns, p.item)),
        Some((Text::raw("Longswordsman"), Some(2), Key::from_u32(11)))
    );
    let kazan = &m.cities[2];
    assert_eq!(
        kazan.production.as_ref().map(|p| (p.name.clone(), p.turns)),
        Some((Text::raw("Granary"), Some(3)))
    );

    // A foreign city in sight: its size and strength, no production or growth.
    let baghdad = &m.cities[3];
    assert!(!baghdad.mine && baghdad.visible && baghdad.capital);
    assert_eq!(baghdad.size, 11);
    assert_eq!(
        (baghdad.production.as_ref(), baghdad.growth_turns),
        (None, None)
    );

    // A foreign city in the fog: the name and the plot (and whose it is), nothing else.
    let fogged = &m.cities[4];
    assert_eq!(
        *fogged,
        CityBanner {
            key: city_key(MONTEZUMA, TENOCHTITLAN),
            owner: Key::from_i32(i32::from(MONTEZUMA)),
            color: fogged.color,
            color2: fogged.color2,
            name: "Tenochtitlan".into(),
            plot: Plot { x: 60, y: 30 },
            ..CityBanner::default()
        }
    );
}

#[test]
fn the_reference_unit_flags() {
    let mut f = game();
    let mut rig = Rig::new(session());
    rig.frame(&mut f);
    let m = map(&rig);
    assert_eq!(m.units.len(), 9);
    let knight = m.units.get(Key::new(0, 1)).expect("the knight");
    assert!(knight.mine && knight.waiting && !knight.at_war && !knight.cargo);
    assert_eq!(knight.icon, AssetRef::new("icon:UNIT_ATLAS_1:25"));
    assert_eq!((knight.domain, knight.hp_fraction), (Domain::Land, 1.0));
    assert_eq!(knight.plot, Plot { x: 11, y: 8 });

    let waiting = m.units.iter().filter(|u| u.waiting).count();
    assert_eq!(waiting, 5, "the same count as the turn button");
    let worker = m.units.get(Key::new(0, 7)).expect("the worker");
    assert!(!worker.waiting, "automated");

    let jaguar = m
        .units
        .get(Key::new(u32::from(MONTEZUMA.cast_unsigned()), 1))
        .expect("the jaguar");
    assert!(!jaguar.mine && jaguar.at_war && !jaguar.waiting);
    assert_eq!(jaguar.color, 0x8b00_02ff);
}

#[test]
fn a_batch_rebuilds_the_map_and_nothing_else_reads() {
    let mut f = game();
    let mut rig = Rig::new(session());
    rig.start(&mut f);
    for _ in 0..10 {
        rig.frame(&mut f);
    }
    assert!(f.calls.is_empty());
    assert!(rig.delta().is_none());

    f.cities[0].population = 15;
    rig.batch(&mut f);
    assert_eq!(map(&rig).cities[0].size, 15);
    let d = rig.delta().expect("a delta");
    assert_eq!(d.ops.len(), 1, "one field of one banner: {:?}", d.ops);
}

#[test]
fn the_reference_notifications() {
    let mut f = game();
    let mut rig = Rig::new(session());
    rig.frame(&mut f);
    let n = notes(&rig);
    let rows: Vec<(i32, NoteCat, &str)> = n
        .iter()
        .map(|n| (i32::try_from(n.id.0).unwrap(), n.category, &*n.message))
        .collect();
    assert_eq!(
        rows,
        [
            (2, NoteCat::Growth, "Novgorod has grown to size 8"),
            (3, NoteCat::GreatPeople, "Great Scientist born in Moscow"),
            (4, NoteCat::Diplomacy, "Montezuma denounced you"),
            (5, NoteCat::Military, "Barbarians spotted near Kazan"),
            (6, NoteCat::Other, "A mod speaks"),
        ],
        "the one the core dismissed is not listed"
    );
    assert_eq!(n[0].plot, Some(Plot { x: 20, y: 12 }));
    assert_eq!(n[2].plot, None);
    assert_eq!(n[0].turn, 86);
}

#[test]
fn a_notification_dismissed_here_is_left_out_without_reading() {
    let mut f = game();
    let mut rig = Rig::new(session());
    rig.start(&mut f);
    rig.dismissed.insert(Key::from_i32(5));
    let stats = rig.frame(&mut f);
    assert_eq!(stats.roots_built, 1, "notes only");
    assert!(f.calls.is_empty(), "the notifications were read already");
    assert!(notes(&rig).get(Key::from_i32(5)).is_none());
    assert_eq!(notes(&rig).len(), 4);
    let d = rig.delta().expect("a delta");
    assert_eq!(d.ops.len(), 1, "one removal");

    // It stays out after the next batch, which reads the notifications again.
    rig.batch(&mut f);
    assert!(f.calls.contains(&"notifications"));
    assert!(notes(&rig).get(Key::from_i32(5)).is_none());

    // Forgotten by the actions module: it comes back.
    rig.dismissed.clear();
    rig.frame(&mut f);
    assert!(notes(&rig).get(Key::from_i32(5)).is_some());
}

#[test]
fn the_core_notification_ids_include_the_ones_dismissed_here() {
    let mut f = game();
    let mut rig = Rig::new(session());
    assert_eq!(rig.src.notification_ids(), None, "nothing read yet");
    rig.start(&mut f);
    let ids = |rig: &Rig| -> Vec<i32> {
        let ids = rig.src.notification_ids().expect("the notes were read");
        ids.iter().map(|k| i32::try_from(k.0).unwrap()).collect()
    };
    assert_eq!(ids(&rig), [2, 3, 4, 5, 6], "not the one the core dismissed");

    // Dismissed here: out of `notes`, still among the core's ids.
    rig.dismissed.insert(Key::from_i32(5));
    rig.frame(&mut f);
    assert!(notes(&rig).get(Key::from_i32(5)).is_none());
    assert_eq!(ids(&rig), [2, 3, 4, 5, 6]);

    // The core drops it: the ids follow with the batch.
    f.notes.retain(|n| n.id != 5);
    rig.batch(&mut f);
    assert_eq!(ids(&rig), [2, 3, 4, 6]);

    rig.src.reset();
    assert_eq!(rig.src.notification_ids(), None);
}

fn line(from: i8, text: &str) -> ChatLine {
    ChatLine {
        from: ["anna", "bob", "carl", "dora"][usize::try_from(from).unwrap()].into(),
        from_player: Some(from),
        to: ChatTo::All,
        text: text.into(),
        server_ms: 0,
    }
}

#[test]
fn the_chat_reads_only_the_session() {
    let mut f = game();
    let mut s = session();
    s.push_chat(line(HARUN, "gg"), T0);
    s.push_chat(
        ChatLine {
            to: ChatTo::Player(CATHERINE),
            ..line(CATHERINE, "peace?")
        },
        T0,
    );
    let mut rig = Rig::new(s);
    rig.start(&mut f);
    let c = chat(&rig);
    assert_eq!(
        c.iter().cloned().collect::<Vec<_>>(),
        vec![
            ChatRow {
                seq: Key(0),
                from: "bob".into(),
                from_player: Some(Key::from_i32(1)),
                to: ui2_model::r#gen::ChatTo::All,
                text: "gg".into(),
                turn: 87,
                mine: false,
            },
            ChatRow {
                seq: Key(1),
                from: "anna".into(),
                from_player: Some(Key::from_i32(0)),
                to: ui2_model::r#gen::ChatTo::Player,
                text: "peace?".into(),
                turn: 87,
                mine: true,
            },
        ]
    );

    // A new turn, a new line: the old lines keep the turn they came in.
    rig.session.turn.as_mut().unwrap().turn = 88;
    rig.frame(&mut f);
    rig.delta();
    rig.session.push_chat(line(RAMESSES, "wait for me"), T0 + 5);
    let stats = rig.frame(&mut f);
    assert_eq!(stats.roots_built, 1, "chat only");
    assert!(f.calls.is_empty(), "the chat reads nothing of the game");
    let c = chat(&rig);
    assert_eq!(c.iter().map(|r| r.turn).collect::<Vec<_>>(), [87, 87, 88]);
    let d = rig.delta().expect("a delta");
    assert_eq!(d.ops.len(), 1, "one insert");
}

#[test]
fn the_chat_keeps_the_last_fifty_lines() {
    let mut f = game();
    let mut s = session();
    for i in 0..60 {
        s.push_chat(line(HARUN, &format!("line {i}")), T0);
    }
    // The client itself dropped the five oldest.
    s.chat.drain(..5);
    let mut rig = Rig::new(s);
    rig.frame(&mut f);
    let c = chat(&rig);
    assert_eq!(c.len(), 50);
    assert_eq!(c[0].seq, Key(10));
    assert_eq!(&*c[0].text, "line 10");
    assert_eq!(c[49].seq, Key(59));
}

#[test]
fn the_turn_button_counts_the_same_units_as_the_flags() {
    let mut f = game();
    f.info.blocker = 3;
    let mut s = session();
    s.turn.as_mut().unwrap().seats[0].finished = false;
    let mut rig = Rig::new(s);
    rig.frame(&mut f);
    let t = rig.store.get::<ui2_model::r#gen::Turn>().expect("turn");
    assert_eq!((t.state, t.units_waiting), (TurnUi::UnitNeedsOrders, 5));
}

#[test]
fn the_read_budget_defers_whole_roots_to_the_next_frames() {
    let mut f = game();
    let mut rig = Rig::new(session());
    // One cost unit: less than any read.
    rig.src.set_read_budget(1);
    // The first frame reads the catalog and builds one root (the first of the queue: the top bar).
    let stats = rig.frame(&mut f);
    assert!(rig.store.get::<ui2_model::r#gen::Top>().is_some());
    assert!(rig.store.get::<Map>().is_none());
    let mut deferred = stats.deferred;
    assert_eq!(deferred, 8);
    // Then at least one root a frame (a root whose reads are kept costs nothing), each whole, with no new
    // trigger.
    let mut frames = 0;
    while deferred > 0 {
        let stats = rig.frame(&mut f);
        assert!(stats.deferred < deferred);
        assert!(
            stats.getter_calls <= 3,
            "one root's reads: {}",
            stats.getter_calls
        );
        deferred = stats.deferred;
        frames += 1;
    }
    assert!((4..=8).contains(&frames), "{frames}");
    assert_eq!(map(&rig).cities.len(), 5, "the map is whole");
    assert_eq!(notes(&rig).len(), 5);
    let n = f.call_count();
    rig.frame(&mut f);
    assert_eq!(f.call_count(), n, "nothing left to do");
}
