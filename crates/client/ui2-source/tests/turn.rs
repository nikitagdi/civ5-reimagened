//! The roots `turn` and `players` and the session's triggers: the reference HUD's readiness (three of four humans
//! ended turn 87, the timer shows 0:42), every state of the turn button, a timer that writes exactly one delta a
//! second, and session changes that rebuild without reading the game.

mod common;

use common::*;
use protocol::{Conn, TurnPhase};
use replica::State;
use ui2_model::r#gen::{Blocker, Game, Meta, Phase, PlayerRow, Seat, Timer, Turn, TurnUi};
use ui2_model::{Key, KeyedList, Op};

fn turn(rig: &Rig) -> Turn {
    rig.store.get::<Turn>().cloned().expect("turn is written")
}

fn ts(rig: &mut Rig) -> &mut protocol::TurnState {
    rig.session.turn.as_mut().expect("a turn state")
}

#[test]
fn the_reference_readiness_and_players() {
    let mut f = game();
    f.info.blocker = 3;
    f.info.blocker_unit = 2;
    f.info.blocker_x = 12;
    f.info.blocker_y = 8;
    let mut rig = Rig::new(session());
    let stats = rig.frame(&mut f);
    assert_eq!(
        f.calls,
        [
            "catalog",
            "catalog",
            "catalog",
            "catalog",
            "catalog",
            "catalog",
            "catalog",
            "top_bar",
            "player_resources",
            "policy_branches",
            "game_info",
            "techs",
            "units",
            "notifications",
            "cities",
            "players",
        ],
        "every human seat has a nick: the turn reads no players"
    );
    assert_eq!(stats.getter_calls, 16);

    let seat = |p: i8, name: &str, finished| Seat {
        player: Key::from_i32(i32::from(p)),
        name: name.into(),
        me: p == CATHERINE,
        finished,
        may_act: true,
        online: true,
    };
    assert_eq!(
        turn(&rig),
        Turn {
            // Catherine ended her turn: the button waits for Ramesses, and she may take it back.
            state: TurnUi::WaitingOthers,
            blocker: Some(Blocker {
                kind: 3,
                unit: Some(Key::new(0, 2)),
                city: None,
                plot: Some(ui2_model::r#gen::Plot { x: 12, y: 8 }),
            }),
            units_waiting: 5,
            finished: true,
            can_cancel: true,
            timer: Some(Timer {
                left_s: 42,
                total_s: 120,
                stopped: false,
            }),
            ready: 3,
            humans: 4,
            seats: KeyedList::from_vec(vec![
                seat(CATHERINE, "anna", true),
                seat(HARUN, "bob", true),
                seat(MONTEZUMA, "carl", true),
                seat(RAMESSES, "dora", false),
            ])
            .unwrap(),
        }
    );

    let players = rig
        .store
        .get::<KeyedList<PlayerRow>>()
        .expect("players are written");
    let names: Vec<&str> = players.iter().map(|p| &*p.leader).collect();
    assert_eq!(
        names,
        ["Catherine", "Harun al-Rashid", "Montezuma", "Ramesses II"],
        "no city-state, no barbarians"
    );
    let catherine = &players[0];
    assert_eq!(
        *catherine,
        PlayerRow {
            id: Key::from_i32(0),
            leader: "Catherine".into(),
            civ: "Russia".into(),
            nick: Some("anna".into()),
            color: 0x8b00_00ff,
            color2: 0xffd7_00ff,
            human: true,
            alive: true,
            met: true,
            at_war: false,
            me: true,
        }
    );
    assert_eq!(
        players
            .iter()
            .filter(|p| p.at_war)
            .map(|p| &*p.leader)
            .collect::<Vec<_>>(),
        ["Montezuma"]
    );
    assert_eq!(players.iter().filter(|p| p.me).count(), 1);
    assert!(rig.store.get::<Meta>().is_some_and(|m| m.connected));
}

#[test]
fn every_state_of_the_turn_button() {
    let mut f = game();
    let mut s = session();
    s.turn.as_mut().unwrap().seats[0].finished = false;
    let mut rig = Rig::new(s);
    rig.frame(&mut f);
    assert_eq!(turn(&rig).state, TurnUi::NextTurn);
    assert_eq!(turn(&rig).units_waiting, 5);

    for (blocker, state) in [
        (3, TurnUi::UnitNeedsOrders),
        (2, TurnUi::CityNeedsProduction),
        (1, TurnUi::ResearchNeeded),
        (0, TurnUi::PolicyNeeded),
        (4, TurnUi::OtherBlocker),
    ] {
        f.info.blocker = blocker;
        rig.batch(&mut f);
        assert_eq!(turn(&rig).state, state, "blocker {blocker}");
        assert_eq!(turn(&rig).blocker.map(|b| b.kind), Some(blocker));
    }

    // From here on only the session changes: the turn is rebuilt from what was read, without reading again.
    f.calls.clear();
    ts(&mut rig).seats[0].finished = true;
    rig.frame(&mut f);
    let t = turn(&rig);
    assert_eq!(
        (t.state, t.finished, t.can_cancel),
        (TurnUi::WaitingOthers, true, true)
    );
    assert_eq!(t.ready, 3);

    ts(&mut rig).seats[3].finished = true;
    rig.frame(&mut f);
    let t = turn(&rig);
    assert_eq!((t.state, t.can_cancel, t.ready), (TurnUi::Ended, false, 4));

    ts(&mut rig)
        .seats
        .iter_mut()
        .for_each(|s| s.finished = false);
    ts(&mut rig).phase = TurnPhase::Ai { active: 5 };
    rig.frame(&mut f);
    assert_eq!(turn(&rig).state, TurnUi::AiMoving);
    assert_eq!(rig.store.get::<Game>().map(|g| g.phase), Some(Phase::Ai));

    ts(&mut rig).phase = TurnPhase::Starting;
    rig.frame(&mut f);
    assert_eq!(turn(&rig).state, TurnUi::AiMoving);
    assert_eq!(
        rig.store.get::<Game>().map(|g| g.phase),
        Some(Phase::Starting)
    );

    // Hybrid turns: a seat at war with the one moving waits for its slot.
    ts(&mut rig).phase = TurnPhase::Humans;
    ts(&mut rig).mode = protocol::TurnMode::Hybrid;
    ts(&mut rig).seats[0].may_act = false;
    rig.frame(&mut f);
    assert_eq!(turn(&rig).state, TurnUi::WaitingOthers);
    assert_eq!(
        rig.store.get::<Game>().map(|g| g.mode),
        Some(ui2_model::r#gen::TurnMode::Hybrid)
    );
    assert!(
        f.calls.is_empty(),
        "session changes read nothing: {:?}",
        f.calls
    );

    ts(&mut rig).phase = TurnPhase::GameOver {
        winner: Some(HARUN),
    };
    rig.frame(&mut f);
    assert_eq!(turn(&rig).state, TurnUi::GameOver);
    let g = rig.store.get::<Game>().expect("game");
    assert_eq!(g.phase, Phase::GameOver);
    assert_eq!(g.winner, Some(Key::from_i32(i32::from(HARUN))));
}

#[test]
fn without_a_server_turn_state_the_core_counts_the_humans() {
    let mut f = game();
    f.info.blocker = 2;
    let mut s = session();
    s.turn = None;
    let mut rig = Rig::new(s);
    rig.frame(&mut f);
    let t = turn(&rig);
    assert_eq!(t.state, TurnUi::CityNeedsProduction);
    assert_eq!((t.ready, t.humans), (3, 4));
    assert!(t.seats.is_empty() && t.timer.is_none() && !t.finished);
}

#[test]
fn a_seat_without_a_nick_is_named_after_its_leader() {
    let mut f = game();
    let mut s = session();
    s.seats[3].control = protocol::SeatControl::Ai { kicked: None };
    s.seats[3].conn = Conn::None;
    let mut rig = Rig::new(s);
    rig.frame(&mut f);
    let t = turn(&rig);
    let ram = t
        .seats
        .get(Key::from_i32(i32::from(RAMESSES)))
        .expect("seat");
    assert_eq!((&*ram.name, ram.online), ("Ramesses II", false));
}

#[test]
fn the_timer_writes_exactly_one_delta_a_second() {
    let mut f = game();
    let mut rig = Rig::new(session());
    rig.start(&mut f);
    let field = rig.store.resolve("turn.timer.left_s").expect("path").id;

    let mut deltas = 0;
    // Five seconds of 60 Hz frames.
    for i in 1..=312_u64 {
        rig.now_ms = T0 + i * 16;
        let stats = rig.frame(&mut f);
        assert_eq!(stats.getter_calls, 0);
        if let Some(d) = rig.delta() {
            deltas += 1;
            assert_eq!(d.ops.len(), 1, "{:?}", d.ops);
            assert!(matches!(&d.ops[0], Op::Set { field: x, .. } if *x == field));
        }
    }
    assert_eq!(deltas, 5);
    assert_eq!(turn(&rig).timer.map(|t| t.left_s), Some(37));
    assert!(f.calls.is_empty(), "the timer reads nothing of the game");

    // The timer keeps going while the replica resyncs (the game roots wait, the clock does not).
    rig.now_ms += 1000;
    let stats = rig.frame_in(State::Resyncing, &mut f, &[]);
    assert_eq!(stats.getter_calls, 0);
    assert_eq!(turn(&rig).timer.map(|t| t.left_s), Some(36));
    assert!(f.calls.is_empty());
}

#[test]
fn a_stopped_timer_writes_nothing_more() {
    let mut f = game();
    let mut rig = Rig::new(session());
    rig.start(&mut f);
    ts(&mut rig).timers[0].left_ms_when_stopped = Some(30_000);
    rig.frame(&mut f);
    assert_eq!(
        turn(&rig).timer,
        Some(Timer {
            left_s: 30,
            total_s: 120,
            stopped: true,
        })
    );
    rig.delta();
    for i in 1..=200_u64 {
        rig.now_ms = T0 + i * 16;
        assert_eq!(rig.frame(&mut f).roots_built, 0);
    }
    assert!(rig.delta().is_none());
}

#[test]
fn no_trigger_no_read_no_write() {
    let mut f = game();
    let mut rig = Rig::new(session());
    rig.start(&mut f);
    // The same second, the same session, the same replica.
    for i in 0..100_u64 {
        rig.now_ms = T0 + i;
        let stats = rig.frame(&mut f);
        assert_eq!(stats, ui2_source::SourceStats::default());
    }
    assert!(f.calls.is_empty());
    assert!(rig.delta().is_none());
}

#[test]
fn the_connection_is_in_meta() {
    let mut f = game();
    let mut rig = Rig::new(session());
    rig.start(&mut f);
    rig.session.connected = false;
    let stats = rig.frame(&mut f);
    assert_eq!(stats.roots_built, 1);
    assert!(rig.store.get::<Meta>().is_some_and(|m| !m.connected));
    assert!(f.calls.is_empty());
}

#[test]
fn a_seat_that_goes_offline_rebuilds_the_turn_only() {
    let mut f = game();
    let mut rig = Rig::new(session());
    rig.start(&mut f);
    rig.session.seats[1].conn = Conn::Lost { since_ms: 5 };
    let stats = rig.frame(&mut f);
    assert_eq!(stats.roots_built, 1);
    assert!(f.calls.is_empty());
    let t = turn(&rig);
    assert!(!t.seats.get(Key::from_i32(i32::from(HARUN))).unwrap().online);
    let d = rig.delta().expect("a delta");
    assert_eq!(d.ops.len(), 1);
}
