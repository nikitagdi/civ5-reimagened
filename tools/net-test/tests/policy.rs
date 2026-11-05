//! The autoplay policy: its choices on synthetic contexts, the personas, and a whole play against a small fake
//! gateway (focus, context, order, end of turn; a query when no context comes).

use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use net_test::bot::persona::{Act, Persona};
use net_test::bot::policy::{
    Done, Pace, Policy, command_for, pick_build, pick_order, step_node, want_focus,
};
use net_test::bot::{Bot, BotConfig};
use protocol::{
    AttachMode, Attached, Blocker, BlockerTarget, Catalog, CatalogEntry, CatalogRef,
    CityBuildFlags, CityBuildView, ClientMsg, Codec, Command, CommandOutcome, CommandResult,
    ContextMsg, Focus, GameBatch, GameMsg, Keyframe, MapBasis, MapView, PeaceOffer, PlayerFlags,
    PlayerView, PlotBasisView, PlotView, Query, QueryAnswer, ReachFlags, ReachNode, SeatTurn,
    ServerMsg, Session, Token, TopBar, TurnMode, TurnPhase, TurnState, TurnStatus, UnitActionView,
    UnitContext, UnitFlags, UnitView, ViewDelta, ViewHash, WorldView, basis_hash_of, diff,
};
use tungstenite::Message;
use wire::{Encoder, Limits};

fn action(a: i16, enabled: bool) -> UnitActionView {
    UnitActionView {
        action: a,
        enabled,
        reason: String::new(),
    }
}

#[test]
fn the_first_enabled_order_of_the_list_wins() {
    // Found a city is first, then automate, fortify, sleep, skip.
    assert_eq!(
        pick_order(&[action(5, true), action(3, true), action(6, true)]),
        Some(3)
    );
    assert_eq!(
        pick_order(&[action(3, false), action(8, true), action(4, true)]),
        Some(8)
    );
    assert_eq!(
        pick_order(&[
            action(3, false),
            action(4, false),
            action(5, true),
            action(6, true)
        ]),
        Some(5)
    );
    // Nothing of the list is enabled, or the list is empty (a failed query).
    assert_eq!(
        pick_order(&[action(1, true), action(2, true), action(6, false)]),
        None
    );
    assert_eq!(pick_order(&[]), None);
    assert_eq!(command_for(4, 9), Some(Command::UnitFortify { unit: 9 }));
    assert_eq!(command_for(3, 9), Some(Command::UnitFoundCity { unit: 9 }));
    assert_eq!(command_for(99, 9), None);
}

fn build(order: i8, item: i16, flags: u16) -> CityBuildView {
    CityBuildView {
        order_type: order,
        item,
        turns: 1,
        production_cost: 10,
        gold_cost: -1,
        flags: CityBuildFlags(flags),
        faith_cost: -1,
    }
}

fn catalog() -> Catalog {
    let e = |id: i16, t: &str| CatalogEntry {
        id,
        type_name: t.into(),
        ..CatalogEntry::default()
    };
    Catalog {
        units: vec![
            e(0, "UNIT_SETTLER"),
            e(1, "UNIT_WORKER"),
            e(2, "UNIT_WARRIOR"),
        ],
        buildings: vec![e(0, "BUILDING_MONUMENT")],
        ..Catalog::default()
    }
}

#[test]
fn a_city_gets_the_build_of_the_preference_list() {
    let on = CityBuildFlags::ENABLED;
    let cur = CityBuildFlags::ENABLED | CityBuildFlags::CURRENT;
    let builds = [
        build(0, 1, on),
        build(0, 2, on),
        build(1, 0, on),
        build(0, 0, cur),
    ];
    // Settler is current: Warrior is next.
    let b = pick_build(&builds, Some(&catalog())).unwrap();
    assert_eq!((b.order_type, b.item), (0, 2));
    // Without the catalog: the first enabled one that is not current.
    let b = pick_build(&builds, None).unwrap();
    assert_eq!((b.order_type, b.item), (0, 1));
    // Disabled builds are not taken.
    assert!(pick_build(&[build(0, 0, 0)], Some(&catalog())).is_none());
}

fn unit(id: i32, owner: i8, flags: u16) -> UnitView {
    UnitView {
        id,
        owner,
        flags: UnitFlags(flags),
        max_moves: 120,
        moves_left: 120,
        ..UnitView::default()
    }
}

fn world(turn: i32, units: Vec<UnitView>, blocker: Option<Blocker>) -> WorldView {
    WorldView {
        turn,
        year: 0,
        human: 0,
        human_team: 0,
        status: TurnStatus::HumanTurn,
        blocker,
        map: MapView {
            width: 2,
            height: 1,
            wrap_x: false,
            wrap_y: false,
            plots: vec![PlotView::default(); 2],
            terrain_seed: 1,
        },
        units,
        cities: vec![],
        players: vec![],
        top_bar: TopBar::default(),
        notifications: vec![],
        peace_offers: vec![],
        trade_routes: vec![],
    }
}

#[test]
fn the_focus_is_the_first_three_units_that_need_orders() {
    let need = UnitFlags::NEEDS_ORDERS;
    let units = vec![
        unit(7, 0, need),
        unit(2, 0, 0),
        unit(5, 0, need),
        unit(3, 1, need),
        unit(9, 0, need),
        unit(4, 0, need),
        unit(1, 0, need),
    ];
    let v = world(1, units, None);
    let f = want_focus(&v, &Default::default());
    assert_eq!(
        f,
        Focus {
            units: vec![1, 4, 5],
            city: None
        }
    );
    // A unit that was refused is left out of the focus for the turn.
    let skip = [1].into_iter().collect();
    assert_eq!(want_focus(&v, &skip).units, vec![4, 5, 7]);
    // The city of a production blocker.
    let v = world(
        1,
        vec![],
        Some(Blocker {
            kind: 2,
            target: BlockerTarget::City(12),
        }),
    );
    assert_eq!(want_focus(&v, &Default::default()).city, Some(12));
    let v = world(
        1,
        vec![],
        Some(Blocker {
            kind: 3,
            target: BlockerTarget::City(12),
        }),
    );
    assert_eq!(want_focus(&v, &Default::default()).city, None);
}

#[test]
fn a_step_is_a_plain_node_of_this_turn() {
    let node = |x, turn, flags| ReachNode {
        x,
        y: 0,
        parent: 0,
        turn,
        moves_left: 0,
        flags: ReachFlags(flags),
    };
    let tree = [
        node(0, 0, 0),
        node(1, 0, ReachFlags::ATTACK),
        node(2, 1, 0),
        node(3, 0, 0),
    ];
    assert_eq!(step_node(&tree).map(|n| n.x), Some(3));
    assert!(step_node(&tree[..1]).is_none());
}

fn player(id: i8, flags: u16) -> PlayerView {
    PlayerView {
        id,
        team: id,
        flags: protocol::PlayerFlags(flags),
        ..PlayerView::default()
    }
}

#[test]
fn personas_act_as_their_names_say() {
    let ts = TurnState {
        turn: 6,
        phase: TurnPhase::Humans,
        mode: TurnMode::Hybrid,
        seats: vec![],
        timers: vec![],
        paused: false,
    };
    let human_met = PlayerFlags::HUMAN | PlayerFlags::MET | PlayerFlags::ALIVE;
    let mut v = world(6, vec![unit(1, 0, 0)], None);
    v.players = vec![
        player(0, PlayerFlags::HUMAN | PlayerFlags::ALIVE),
        player(1, human_met),
    ];

    // The warmonger waits for its turn, then declares war on the human it has met, once.
    let mut w = Persona::parse("warmonger", &serde_json::json!({ "war_turn": 7 })).unwrap();
    assert_eq!(w.plan(&v, &ts, "a"), None);
    let later = TurnState {
        turn: 7,
        ..ts.clone()
    };
    assert_eq!(
        w.plan(&v, &later, "a"),
        Some(Act::Cmd(Command::DeclareWar { team: 1 }))
    );
    assert_eq!(w.plan(&v, &later, "a"), None);

    // The peaceful answers an offer with yes, and offers peace to a human it is at war with, once a turn.
    let mut p = Persona::parse("peaceful", &serde_json::Value::Null).unwrap();
    v.peace_offers = vec![PeaceOffer {
        from: 1,
        to: 0,
        turn: 6,
    }];
    assert_eq!(
        p.plan(&v, &ts, "a"),
        Some(Act::Cmd(Command::AnswerPeace {
            from: 1,
            accept: true
        }))
    );
    v.peace_offers.clear();
    v.players[1].flags = protocol::PlayerFlags(human_met | PlayerFlags::AT_WAR_WITH_VIEWER);
    assert_eq!(
        p.plan(&v, &ts, "a"),
        Some(Act::Cmd(Command::ProposePeace { to: 1 }))
    );
    assert_eq!(p.plan(&v, &ts, "a"), None);

    // The chatty talks every k turns, the spammer breaks the limits, the idle never plays.
    let mut c = Persona::parse("chatty", &serde_json::json!({ "every": 3 })).unwrap();
    assert!(matches!(c.plan(&v, &ts, "a"), Some(Act::Chat(_))));
    assert_eq!(c.plan(&v, &ts, "a"), None);
    let mut s = Persona::parse("spammer", &serde_json::json!({ "per_turn": 70 })).unwrap();
    match s.plan(&v, &ts, "a") {
        Some(Act::Burst(cmds)) => assert_eq!(cmds.len(), 70),
        other => panic!("{other:?}"),
    }
    assert!(
        Persona::parse("idle", &serde_json::Value::Null)
            .unwrap()
            .is_idle()
    );
    assert!(Persona::parse("nobody", &serde_json::Value::Null).is_err());
}

// ---------------------------------------------------------------- a play against a fake gateway

fn send(ws: &mut tungstenite::WebSocket<std::net::TcpStream>, m: &ServerMsg) {
    let body = protocol::Postcard.encode(m).unwrap();
    ws.send(Message::binary(Encoder::plain(&body))).unwrap();
}

fn basis() -> MapBasis {
    let mut b = MapBasis {
        game: 1,
        basis_hash: 0,
        width: 2,
        height: 1,
        wrap_x: false,
        wrap_y: false,
        terrain_seed: 1,
        plots: vec![PlotBasisView::default(); 2],
    };
    b.basis_hash = basis_hash_of(&b);
    b
}

fn turn_state(turn: i32) -> TurnState {
    TurnState {
        turn,
        phase: TurnPhase::Humans,
        mode: TurnMode::Simultaneous,
        seats: vec![SeatTurn {
            player: 0,
            finished: false,
            may_act: true,
            group: 0,
        }],
        timers: vec![],
        paused: false,
    }
}

/// A one-unit game. The unit needs orders in turn 1; fortified, it needs none. With `contexts` the fake
/// sends the context of the unit after a `Focus`; without, it answers a `Query`. Everything the bot sends
/// goes to the returned channel.
fn fake_gateway(contexts: bool) -> (String, mpsc::Receiver<ClientMsg>) {
    fake_gateway_with(contexts, None)
}

/// With `ai_phase`, an end of turn is answered the way the turn timeline does it: the turn counter moves at once
/// (`TurnState` of the new turn in the AI phase), and the view of the new turn follows in a later batch with the
/// human phase. The hash of the gateway's last view goes to the cell.
fn fake_gateway_with(
    contexts: bool,
    ai_phase: Option<std::sync::Arc<std::sync::atomic::AtomicU64>>,
) -> (String, mpsc::Receiver<ClientMsg>) {
    let (tx, rx) = mpsc::channel();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}/ws", listener.local_addr().unwrap());
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut ws = tungstenite::accept(stream).unwrap();
        let read = |ws: &mut tungstenite::WebSocket<std::net::TcpStream>| loop {
            match ws.read() {
                Ok(Message::Binary(b)) => {
                    let m = wire::decode::<ClientMsg>(&b, &Limits::GATEWAY).unwrap();
                    let _ = tx.send(m.clone());
                    return Some(m);
                }
                Ok(Message::Close(_)) | Err(_) => return None,
                Ok(_) => {}
            }
        };
        read(&mut ws); // Hello
        read(&mut ws); // Login
        send(
            &mut ws,
            &ServerMsg::LoggedIn(Session {
                token: Some(Token([1; 16])),
                nick: "p".into(),
            }),
        );
        send(
            &mut ws,
            &ServerMsg::Attached(Attached {
                game: 1,
                player: 0,
                stream: 1,
                mode: AttachMode::Fresh,
                catalog: CatalogRef { hash: 0, len: 0 },
                last_cmd_id: 0,
            }),
        );
        let blocker = |u: i32| {
            Some(Blocker {
                kind: 3,
                target: BlockerTarget::Unit(u),
            })
        };
        let mut view = world(1, vec![unit(1, 0, UnitFlags::NEEDS_ORDERS)], blocker(1));
        let (mut seq, mut view_seq) = (1u64, 1u64);
        send(
            &mut ws,
            &ServerMsg::Game(GameBatch {
                seq,
                msgs: vec![
                    GameMsg::MapBasis(basis()),
                    GameMsg::Keyframe(Keyframe {
                        hash: ViewHash::of(&view).value(),
                        basis_hash: basis().basis_hash,
                        view: view.clone(),
                    }),
                    GameMsg::TurnState(turn_state(1)),
                ],
            }),
        );
        let ctx = |view_seq| {
            ContextMsg::Unit(UnitContext {
                unit: 1,
                view_seq,
                actions: vec![action(4, true), action(5, true), action(6, true)],
                reach: vec![],
                targets: vec![],
            })
        };
        let result = |id, outcome| GameMsg::Results(vec![CommandResult { id, outcome }]);
        while let Some(m) = read(&mut ws) {
            match m {
                ClientMsg::Focus(f) if contexts && !f.units.is_empty() => {
                    seq += 1;
                    send(
                        &mut ws,
                        &ServerMsg::Game(GameBatch {
                            seq,
                            msgs: vec![GameMsg::Context(ctx(view_seq))],
                        }),
                    );
                }
                ClientMsg::Query {
                    id,
                    q: Query::UnitActions { .. },
                } => send(
                    &mut ws,
                    &ServerMsg::QueryResult {
                        id,
                        result: QueryAnswer::UnitActions(vec![action(5, true), action(6, true)]),
                    },
                ),
                ClientMsg::Cmd(c) => {
                    seq += 1;
                    let old = view.clone();
                    let mut msgs = Vec::new();
                    match c.cmd {
                        Command::UnitFortify { .. } | Command::UnitSleep { .. } => {
                            view.units[0].flags = UnitFlags(0);
                            view.blocker = None;
                        }
                        Command::EndTurn { .. } => {
                            view.turn += 1;
                        }
                        _ => {}
                    }
                    if old != view {
                        msgs.push(GameMsg::Diff(ViewDelta {
                            base: view_seq,
                            diff: diff(&old, &view, None),
                            hash: ViewHash::of(&view).value(),
                            basis_hash: None,
                        }));
                        view_seq = seq;
                    }
                    msgs.push(result(c.id, CommandOutcome::Applied));
                    let ended = matches!(c.cmd, Command::EndTurn { .. });
                    if let (Some(last), true) = (&ai_phase, ended) {
                        let mut ai = turn_state(view.turn);
                        ai.phase = TurnPhase::Ai { active: -1 };
                        msgs.push(GameMsg::TurnState(ai));
                        send(&mut ws, &ServerMsg::Game(GameBatch { seq, msgs }));
                        // The AI phase runs; the start of the new turn changes the view once more.
                        thread::sleep(Duration::from_millis(300));
                        seq += 1;
                        let old = view.clone();
                        view.units[0].flags = UnitFlags(UnitFlags::NEEDS_ORDERS);
                        let d = GameMsg::Diff(ViewDelta {
                            base: view_seq,
                            diff: diff(&old, &view, None),
                            hash: ViewHash::of(&view).value(),
                            basis_hash: None,
                        });
                        view_seq = seq;
                        last.store(
                            ViewHash::of(&view).value(),
                            std::sync::atomic::Ordering::SeqCst,
                        );
                        let msgs = vec![d, GameMsg::TurnState(turn_state(view.turn))];
                        send(&mut ws, &ServerMsg::Game(GameBatch { seq, msgs }));
                        continue;
                    }
                    if ended {
                        msgs.push(GameMsg::TurnState(turn_state(view.turn)));
                    }
                    send(&mut ws, &ServerMsg::Game(GameBatch { seq, msgs }));
                }
                _ => {}
            }
        }
    });
    (url, rx)
}

/// NP28: the start of a game with three units and a city without production (a civ with a free scout after
/// the settler founded the capital). The blocker is the city; the units need orders too. An end of turn
/// is refused while the city has no production, forced or not (the core's FORCE_END_TURN is for units).
fn fake_production_gateway() -> (String, mpsc::Receiver<ClientMsg>) {
    let (tx, rx) = mpsc::channel();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("ws://{}/ws", listener.local_addr().unwrap());
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        // No catalog here: its fetch is refused at once.
        drop(listener);
        let mut ws = tungstenite::accept(stream).unwrap();
        let read = |ws: &mut tungstenite::WebSocket<std::net::TcpStream>| loop {
            match ws.read() {
                Ok(Message::Binary(b)) => {
                    let m = wire::decode::<ClientMsg>(&b, &Limits::GATEWAY).unwrap();
                    let _ = tx.send(m.clone());
                    return Some(m);
                }
                Ok(Message::Close(_)) | Err(_) => return None,
                Ok(_) => {}
            }
        };
        read(&mut ws); // Hello
        read(&mut ws); // Login
        send(
            &mut ws,
            &ServerMsg::LoggedIn(Session {
                token: Some(Token([1; 16])),
                nick: "p".into(),
            }),
        );
        send(
            &mut ws,
            &ServerMsg::Attached(Attached {
                game: 1,
                player: 0,
                stream: 1,
                mode: AttachMode::Fresh,
                catalog: CatalogRef { hash: 0, len: 0 },
                last_cmd_id: 0,
            }),
        );
        let production = Some(Blocker {
            kind: 2,
            target: BlockerTarget::City(10),
        });
        let units = (1..=3)
            .map(|id| unit(id, 0, UnitFlags::NEEDS_ORDERS))
            .collect();
        let mut view = world(1, units, production);
        let (mut seq, mut view_seq) = (1u64, 1u64);
        send(
            &mut ws,
            &ServerMsg::Game(GameBatch {
                seq,
                msgs: vec![
                    GameMsg::MapBasis(basis()),
                    GameMsg::Keyframe(Keyframe {
                        hash: ViewHash::of(&view).value(),
                        basis_hash: basis().basis_hash,
                        view: view.clone(),
                    }),
                    GameMsg::TurnState(turn_state(1)),
                ],
            }),
        );
        let fortify = || vec![action(4, true), action(6, true)];
        let result = |id, outcome| GameMsg::Results(vec![CommandResult { id, outcome }]);
        while let Some(m) = read(&mut ws) {
            match m {
                ClientMsg::Focus(f) => {
                    seq += 1;
                    let msgs = f
                        .units
                        .iter()
                        .map(|&unit| {
                            GameMsg::Context(ContextMsg::Unit(UnitContext {
                                unit,
                                view_seq,
                                actions: fortify(),
                                reach: vec![],
                                targets: vec![],
                            }))
                        })
                        .collect();
                    send(&mut ws, &ServerMsg::Game(GameBatch { seq, msgs }));
                }
                ClientMsg::Query { id, q } => {
                    let result = match q {
                        Query::UnitActions { .. } => QueryAnswer::UnitActions(fortify()),
                        _ => QueryAnswer::CityBuilds(vec![build(0, 2, CityBuildFlags::ENABLED)]),
                    };
                    send(&mut ws, &ServerMsg::QueryResult { id, result });
                }
                ClientMsg::Cmd(c) => {
                    seq += 1;
                    let old = view.clone();
                    let mut msgs = Vec::new();
                    let mut outcome = CommandOutcome::Applied;
                    match c.cmd {
                        Command::UnitFortify { unit } => {
                            if let Some(u) = view.units.iter_mut().find(|u| u.id == unit) {
                                u.flags = UnitFlags(0);
                            }
                        }
                        Command::CitySetProduction { .. } => view.blocker = None,
                        Command::EndTurn { .. } if view.blocker.is_some() => {
                            outcome = CommandOutcome::Rejected {
                                code: protocol::RejectCode::Core,
                                reason: "blocked by ENDTURN_BLOCKING_PRODUCTION".into(),
                            };
                        }
                        Command::EndTurn { .. } => view.turn += 1,
                        _ => {}
                    }
                    if old != view {
                        msgs.push(GameMsg::Diff(ViewDelta {
                            base: view_seq,
                            diff: diff(&old, &view, None),
                            hash: ViewHash::of(&view).value(),
                            basis_hash: None,
                        }));
                        view_seq = seq;
                    }
                    let ended = old.turn != view.turn;
                    msgs.push(result(c.id, outcome));
                    if ended {
                        msgs.push(GameMsg::TurnState(turn_state(view.turn)));
                    }
                    send(&mut ws, &ServerMsg::Game(GameBatch { seq, msgs }));
                }
                _ => {}
            }
        }
    });
    (url, rx)
}

#[test]
fn orders_for_other_units_do_not_force_the_end_of_turn_over_a_city() {
    let (bot, sent) = play_on(fake_production_gateway(), 1);
    assert_eq!(bot.play_done, Some(Done::Turns), "{:?}", bot.s.stats.errors);
    // Three units get their orders, then the city its build; the end of turn is a plain one. Before NP28
    // the three unit orders counted as tries at the city and the policy forced the end of turn instead.
    assert_eq!(
        orders(&sent),
        [
            Command::UnitFortify { unit: 1 },
            Command::UnitFortify { unit: 2 },
            Command::UnitFortify { unit: 3 },
            Command::CitySetProduction {
                city: 10,
                order: 0,
                item: 2
            },
            Command::EndTurn { force: false }
        ]
    );
    assert!(bot.s.stats.errors.is_empty(), "{:?}", bot.s.stats.errors);
}

fn play(contexts: bool) -> (Bot, Vec<ClientMsg>) {
    play_on(fake_gateway(contexts), 2)
}

fn play_on((url, rx): (String, mpsc::Receiver<ClientMsg>), turns: i32) -> (Bot, Vec<ClientMsg>) {
    let mut bot = Bot::new(BotConfig::new(&url, "p", ""));
    bot.connect(false, None).unwrap();
    let ready = bot
        .wait_until(Duration::from_secs(10), |b| {
            b.s.view.is_some() && b.s.turn.is_some()
        })
        .unwrap();
    assert!(ready);
    bot.driver = Some(Box::new(Policy::new(
        Persona::Autoplay,
        Pace::Burst,
        turns,
        1,
        Duration::from_secs(20),
    )));
    let done = bot
        .wait_until(Duration::from_secs(20), |b| b.play_done.is_some())
        .unwrap();
    assert!(done, "the policy never finished: {:?}", bot.s.stats.errors);
    bot.close();
    thread::sleep(Duration::from_millis(100));
    (bot, rx.try_iter().collect())
}

fn orders(sent: &[ClientMsg]) -> Vec<Command> {
    sent.iter()
        .filter_map(|m| {
            if let ClientMsg::Cmd(c) = m {
                Some(c.cmd)
            } else {
                None
            }
        })
        .collect()
}

#[test]
fn the_policy_orders_the_unit_from_its_context_and_ends_two_turns() {
    let (bot, sent) = play(true);
    assert_eq!(bot.play_done, Some(Done::Turns));
    // Fortify the unit of the context (the first enabled action of the list that is not found/automate),
    // then end turn 1; turn 2 has nothing to order.
    assert_eq!(
        orders(&sent),
        [
            Command::UnitFortify { unit: 1 },
            Command::EndTurn { force: false },
            Command::EndTurn { force: false }
        ]
    );
    assert!(sent.contains(&ClientMsg::Focus(Focus {
        units: vec![1],
        city: None
    })));
    assert_eq!(bot.s.stats.contexts_used, 1);
    assert_eq!(bot.s.stats.queries_sent, 0, "the context came: no query");
    assert_eq!(bot.s.ledger.open(), 0);
    assert_eq!(bot.s.stats.turns_played, 2);
    assert!(bot.s.stats.errors.is_empty(), "{:?}", bot.s.stats.errors);
}

#[test]
fn the_round_ends_in_the_human_phase_of_the_last_turn_not_in_the_ai_phase_before_it() {
    // The turn counter moves in the AI phase already; the view of the new turn comes after it. A bot that stopped
    // at the counter held the view one AI phase behind the gateway's (deploy-smoke, game 946).
    let last = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (bot, _) = play_on(fake_gateway_with(true, Some(last.clone())), 2);
    assert_eq!(bot.play_done, Some(Done::Turns), "{:?}", bot.s.stats.errors);
    let theirs = last.load(std::sync::atomic::Ordering::SeqCst);
    assert_ne!(theirs, 0);
    assert_eq!(bot.s.view.as_ref().map(|(_, h)| h.value()), Some(theirs));
    assert!(bot.s.stats.errors.is_empty(), "{:?}", bot.s.stats.errors);
}

#[test]
fn without_a_context_the_policy_asks_once() {
    let (bot, sent) = play(false);
    assert_eq!(bot.play_done, Some(Done::Turns));
    // The query answer lists sleep and skip: sleep is the first of the preference list.
    assert_eq!(orders(&sent)[0], Command::UnitSleep { unit: 1 });
    assert_eq!(bot.s.stats.queries_sent, 1);
    assert_eq!(bot.s.stats.contexts_used, 0);
    let asked = sent
        .iter()
        .filter(|m| {
            matches!(
                m,
                ClientMsg::Query {
                    q: Query::UnitActions { unit: 1 },
                    ..
                }
            )
        })
        .count();
    assert_eq!(asked, 1);
}
