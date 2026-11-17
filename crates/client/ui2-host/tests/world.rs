//! The normal network entry point uses shown state and revision-scoped queries.
#[path = "../../client-core/tests/common/mod.rs"]
mod network;
mod support;

use client_core::ClientEvent;
use network::world::World;
use network::{GamePlan, Net};
use protocol::{
    ClientMsg, ContextMsg, GameMsg, Query, QueryAnswer, ReachNode, SeatTurn, TurnPhase, TurnState,
    UnitContext, UnitView, ViewHash,
};
use ui2_host::{NetSink, V2Frame, V2Init, V2Ui};
use ui2_model::generated::{Map, Meta, Research, Source, Top, Unit};
use ui2_model::wire::{Origin, WireIntent};
use ui2_model::{Key, Value};

fn setup() -> (Net, V2Ui) {
    let mut net = Net::start();
    let mut world = World::new(7, 11, 6, 5);
    world.view.human = 1;
    world.view.turn = 5;
    world.view.units = vec![UnitView {
        id: 4,
        owner: 1,
        x: 1,
        y: 1,
        hp: 100,
        max_hp: 100,
        moves_left: 120,
        max_moves: 120,
        cargo_of: -1,
        building: -1,
        flags: protocol::UnitFlags(
            protocol::UnitFlags::CAN_MOVE | protocol::UnitFlags::NEEDS_ORDERS,
        ),
        name: "Fixture unit".into(),
        ..Default::default()
    }];
    world.hash = ViewHash::of(&world.view);
    net.srv.world = Some(world);
    net.srv.plan = Some(GamePlan {
        game: 7,
        player: 1,
        stream: 3,
        last_cmd_id: 0,
        last_seq: 0,
    });
    net.srv.book.auto = false;
    net.srv.answer_queries = false;
    net.connect();
    net.login();
    net.core.set_catalog(protocol::Catalog::default());
    let token = net.core.token().expect("session token");
    let world = net.srv.world.as_mut().expect("world");
    let seq = world.view_seq;
    let message = world.push(vec![
        GameMsg::TurnState(TurnState {
            turn: 5,
            phase: TurnPhase::Humans,
            mode: protocol::TurnMode::Simultaneous,
            seats: vec![SeatTurn {
                player: 1,
                finished: false,
                may_act: true,
                group: 0,
            }],
            timers: Vec::new(),
            paused: false,
        }),
        GameMsg::Context(ContextMsg::Unit(UnitContext {
            unit: 4,
            view_seq: seq,
            reach: vec![
                ReachNode {
                    x: 1,
                    y: 1,
                    moves_left: 120,
                    ..Default::default()
                },
                ReachNode {
                    x: 2,
                    y: 1,
                    moves_left: 60,
                    ..Default::default()
                },
            ],
            ..Default::default()
        })),
    ]);
    net.push(token, message);
    net.settle();
    net.take_events();
    let mut ui = V2Ui::new(support::strip(), V2Init::default()).expect("UI");
    ui.set_read_budget(u32::MAX);
    frame(&mut ui, &mut net);
    (net, ui)
}

fn frame(ui: &mut V2Ui, net: &mut Net) -> ui2_host::V2FrameReport {
    for event in net.take_events() {
        if let ClientEvent::QueryResult { id, answer } = event {
            ui.query_result(id, &answer);
        }
    }
    let mut sink = NetSink::new(&mut net.core);
    ui.frame(V2Frame {
        world: None,
        net: Some(&mut sink),
        events: &[],
        mods: None,
        now_ms: 1000.0,
    })
}

fn intent(name: &str, args: Vec<(&str, Value)>) -> WireIntent {
    WireIntent {
        name: name.into(),
        args: args.into_iter().map(|(k, v)| (k.into(), v)).collect(),
        origin: Origin::Markup { node: 1 },
    }
}

#[test]
fn a_selected_unit_and_optimistic_move_reach_the_same_frames_model() {
    let (mut net, mut ui) = setup();
    ui.push_intent(intent(
        "unit.select",
        vec![("unit", Value::Str(Key::new(1, 4).to_js().into()))],
    ));
    frame(&mut ui, &mut net);
    let selected = ui
        .store()
        .get::<Option<Unit>>()
        .expect("unit root")
        .as_ref()
        .expect("selected");
    assert_eq!(selected.key, Key::new(1, 4));
    ui.push_intent(intent(
        "unit.move",
        vec![
            ("unit", Value::Str(Key::new(1, 4).to_js().into())),
            ("plot", Value::Struct(vec![Value::I32(2), Value::I32(1)])),
        ],
    ));
    let report = frame(&mut ui, &mut net);
    assert_eq!(report.refused, 0);
    assert_eq!(net.core.base().expect("base").units[0].x, 1);
    assert_eq!(net.core.shown().expect("shown").units[0].x, 2);
    let map = ui.store().get::<Map>().expect("map");
    assert_eq!(
        map.units
            .iter()
            .find(|u| u.key == Key::new(1, 4))
            .expect("unit flag")
            .plot
            .x,
        2
    );
    assert_eq!(
        ui.store()
            .get::<Option<Unit>>()
            .expect("unit")
            .as_ref()
            .expect("selected")
            .moves,
        1.0
    );
}

#[test]
fn queries_rebuild_on_answers_and_idle_frames_do_no_work() {
    let (mut net, mut ui) = setup();
    ui.push_intent(intent(
        "unit.select",
        vec![("unit", Value::Str(Key::new(1, 4).to_js().into()))],
    ));
    frame(&mut ui, &mut net);
    net.settle();
    let id = net
        .srv
        .log
        .iter()
        .find_map(|(_, m)| match m {
            ClientMsg::Query {
                id,
                q: Query::UnitActions { unit: 4 },
            } => Some(*id),
            _ => None,
        })
        .expect("unit actions query");
    ui.query_result(
        id,
        &QueryAnswer::UnitActions(vec![protocol::UnitActionView {
            action: 4,
            enabled: true,
            reason: String::new(),
        }]),
    );
    let report = frame(&mut ui, &mut net);
    assert!(report.source.roots_built > 0);
    assert!(
        ui.store()
            .get::<Option<Unit>>()
            .expect("root")
            .as_ref()
            .expect("unit")
            .act
            .fortify
            .enabled
    );
    let idle = frame(&mut ui, &mut net);
    assert_eq!(idle.source.roots_built, 0);
    assert_eq!(idle.source.getter_calls, 0);
    assert_eq!(idle.delta_ops, 0);
    assert_eq!(
        ui.store().get::<Meta>().expect("meta").source,
        Source::Server
    );
    let top = ui.store().get::<Top>().expect("top");
    assert!(top.tourism.is_none() && top.resources.is_empty() && top.policies.is_empty());
    assert!(!ui.store().get::<Research>().expect("research").known);
}

#[test]
fn asynchronous_extras_refresh_cached_unknown_fields_without_a_world_change() {
    struct Extras(std::rc::Rc<std::cell::Cell<u64>>);
    impl ui2_source::world::HudExtras for Extras {
        fn revision(&self) -> u64 {
            self.0.get()
        }
        fn tourism(&self) -> Option<i32> {
            Some(self.0.get() as i32)
        }
    }
    let (mut net, mut ui) = setup();
    let world_revision = net.core.revision();
    let version = std::rc::Rc::new(std::cell::Cell::new(1));
    ui.set_hud_extras(Box::new(Extras(version.clone())));
    frame(&mut ui, &mut net);
    assert_eq!(ui.store().get::<Top>().expect("top").tourism, Some(1));
    version.set(2);
    frame(&mut ui, &mut net);
    assert_eq!(net.core.revision(), world_revision);
    assert_eq!(ui.store().get::<Top>().expect("top").tourism, Some(2));
    let idle = frame(&mut ui, &mut net);
    assert_eq!(idle.source.getter_calls, 0);
}

#[test]
fn default_hud_service_projects_answers_and_refreshes_after_queue_only_orders() {
    let (mut net, mut ui) = setup();
    net.settle();
    let id = net
        .srv
        .log
        .iter()
        .find_map(|(_, msg)| match msg {
            ClientMsg::Query {
                id,
                q: Query::Screen { screen, args },
            } if screen == "hud" && args.is_empty() => Some(*id),
            _ => None,
        })
        .expect("automatic service subscription");
    let revision = net.core.revision();
    let event = ui2_actions::ActionEvent::QueryResult {
        id,
        answer: QueryAnswer::Screen {
            screen: "hud".into(),
            json: r#"{"research":{"techs":[{"id":0,"status":1,"queue":1,"cost":100}]},"top":{"tourism":9}}"#.into(),
        },
    };
    ui.frame(V2Frame {
        world: None,
        net: Some(&mut NetSink::new(&mut net.core)),
        events: &[event],
        mods: None,
        now_ms: 1100.0,
    });
    frame(&mut ui, &mut net);
    assert_eq!(net.core.revision(), revision);
    assert_eq!(ui.store().get::<Top>().unwrap().tourism, Some(9));
    let research = ui.store().get::<Research>().unwrap();
    assert!(research.known);
    assert_eq!(research.queue.len(), 1);
    assert!(research.next.is_some());
    // Both a replacement skin and the original runtime get the existing service snapshot.
    ui.set_skin(support::state_skin("other", "name", None), None)
        .unwrap();
    frame(&mut ui, &mut net);
    assert_eq!(ui.store().get::<Top>().unwrap().tourism, Some(9));
    ui.set_skin(support::strip(), None).unwrap();
    frame(&mut ui, &mut net);
    assert_eq!(ui.store().get::<Top>().unwrap().tourism, Some(9));
    assert_eq!(ui.store().get::<Research>().unwrap().queue.len(), 1);
    // A successful queue-only command need not produce a world diff.
    let outcome = ui2_actions::ActionEvent::Answered {
        id: 99,
        outcome: protocol::CommandOutcome::Applied,
    };
    ui.frame(V2Frame {
        world: None,
        net: Some(&mut NetSink::new(&mut net.core)),
        events: &[outcome],
        mods: None,
        now_ms: 1250.0,
    });
    net.settle();
    assert_eq!(net.core.revision(), revision);
    assert_eq!(net.srv.log.iter().filter(|(_, msg)| matches!(msg, ClientMsg::Query { q: Query::Screen { screen, .. }, .. } if screen == "hud")).count(), 2);
    let idle = frame(&mut ui, &mut net);
    assert_eq!(idle.source.getter_calls, 0);
    assert_eq!(idle.delta_ops, 0);
    ui.reset_source();
    frame(&mut ui, &mut net);
    assert_eq!(ui.store().get::<Top>().unwrap().tourism, None);
    assert!(!ui.store().get::<Research>().unwrap().known);
    assert!(
        ui.store()
            .get::<Option<ui2_model::generated::Hud>>()
            .unwrap()
            .is_none()
    );
}

#[test]
fn popup_hud_answer_opens_a_window_and_close_sends_a_numbered_dismissal() {
    let (mut net, mut ui) = setup();
    net.settle();
    let query = net
        .srv
        .log
        .iter()
        .find_map(|(_, msg)| match msg {
            ClientMsg::Query {
                id,
                q: Query::Screen { screen, .. },
            } if screen == "hud" => Some(*id),
            _ => None,
        })
        .unwrap();
    let event = ui2_actions::ActionEvent::QueryResult {
        id: query,
        answer: QueryAnswer::Screen { screen: "hud".into(), json: r#"{"popup":{"id":7,"screen":"techs","a":2,"b":3,"decision":false},"popup_count":2}"#.into() },
    };
    ui.frame(V2Frame {
        world: None,
        net: Some(&mut NetSink::new(&mut net.core)),
        events: &[event],
        mods: None,
        now_ms: 1100.0,
    });
    assert!(ui.actions().ui().windows.techs.open);
    assert_eq!(ui.actions().ui().windows.techs.popup_id, Some(7));
    frame(&mut ui, &mut net);
    net.settle();
    assert!(net.srv.log.iter().any(|(_, msg)| matches!(msg, ClientMsg::Query { q: Query::Screen {screen, args}, .. } if screen == "techs" && args.contains(&("popup_id".into(), 7)))));
    ui.push_intent(intent(
        "window.close",
        vec![("window", Value::Str("techs".into()))],
    ));
    frame(&mut ui, &mut net);
    net.settle();
    assert!(!ui.actions().ui().windows.techs.open);
    assert!(net.srv.log.iter().any(|(_, msg)| matches!(msg, ClientMsg::Cmd(cmd) if matches!(&cmd.cmd, protocol::Command::ScreenAction { screen, action, args, .. } if screen == "hud" && action == "popup_done" && args == &vec![("a".into(), 7)]))));
    ui.reset_source();
    frame(&mut ui, &mut net);
    assert_eq!(ui.actions().ui().windows.techs.popup_id, None);
}
