//! HUD panels through the real server core; stub builds explicitly skip these proofs.
use game_host::Worker;
use protocol::{Query, QueryAnswer, Start, ToWorker, WorkerMsg};
use ui2_model::{ModelStore, Sample, Schema, generated::Hud};

#[path = "common/real.rs"]
mod common;

fn start() -> Option<common::H> {
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return None;
    }
    let (worker, outs) = Worker::new(
        common::real_core(),
        common::init(Start::New(common::setup()), ""),
        0,
    );
    let mut h = common::H::with(worker, outs);
    h.found_city(0);
    Some(h)
}

fn hud(h: &mut common::H, id: u64, args: Vec<(String, i64)>) -> Hud {
    let before = h.frames.len();
    h.send(ToWorker::Query {
        player: 0,
        qid: id,
        q: Query::Screen {
            screen: "hud".into(),
            args,
        },
    });
    h.run();
    let answer = h.frames[before..]
        .iter()
        .flat_map(|f| &f.msgs)
        .find_map(|m| match m {
            WorkerMsg::QueryResult { qid, answer } if *qid == id => Some(answer),
            _ => None,
        })
        .expect("HUD answer");
    let QueryAnswer::Screen { json, .. } = answer else {
        panic!("HUD query did not return screen data")
    };
    let value: serde_json::Value = serde_json::from_str(json).unwrap();
    let sample = Sample::parse(
        Schema::v1(),
        &serde_json::json!({"model":{"hud":value}}).to_string(),
    )
    .unwrap();
    let mut store = ModelStore::new(Schema::v1());
    sample.apply(&mut store).unwrap();
    store.get::<Option<Hud>>().unwrap().clone().unwrap()
}

#[test]
fn top_panel_sources_are_well_formed_after_founding() {
    let Some(mut h) = start() else { return };
    let top = hud(&mut h, 1, vec![]).top;
    assert_eq!(top.tips.len(), 7);
    assert!(
        top.tips
            .iter()
            .all(|t| !t.lines.is_empty() && t.total.is_finite())
    );
    assert!(top.resources.iter().all(|r| !r.name.is_empty()));
    assert!(top.trade_available >= top.trade_used);
    // Top-panel actions open local windows and emit no game mutation.
}

#[test]
fn end_turn_reports_queue_and_uses_the_session_handshake() {
    let Some(mut h) = start() else { return };
    let turn = hud(&mut h, 1, vec![]).turn;
    assert!(!turn.queue.is_empty());
    assert!(turn.queue.iter().all(|p| p.known || p.team == -1));
    let outcome = h.cmd(0, protocol::Command::EndTurn { force: false });
    match outcome {
        protocol::CommandOutcome::Applied => {}
        protocol::CommandOutcome::Rejected { reason, .. } => assert!(!reason.is_empty()),
        _ => panic!("End-turn handshake did not return a final outcome"),
    }
}

#[test]
fn unit_panel_catalog_and_text_rename_change_the_owned_unit() {
    let Some(mut h) = start() else { return };
    let world = h.w.fresh_view(0).unwrap();
    let id = world
        .units
        .iter()
        .find(|u| u.owner == 0)
        .expect("owned unit")
        .id;
    let root = hud(&mut h, 1, vec![("unit".into(), i64::from(id))]);
    assert!(!root.unit.unwrap().catalog.is_empty());
    assert_eq!(
        h.cmd(
            0,
            protocol::Command::ScreenAction {
                screen: "hud".into(),
                action: "rename".into(),
                args: vec![("a".into(), i64::from(id))],
                text: Some("Test Guard".into())
            }
        ),
        protocol::CommandOutcome::Applied
    );
    assert_eq!(
        hud(&mut h, 2, vec![("unit".into(), i64::from(id))])
            .unit
            .unwrap()
            .name,
        ui2_model::Text::raw("Test Guard")
    );
    let root = hud(&mut h, 3, vec![("unit".into(), i64::from(id))]);
    for (index, a) in root.unit.unwrap().catalog.iter().enumerate() {
        if !a.client.is_empty() {
            continue;
        }
        let before = h.w.fresh_view(0).unwrap();
        let qid = 10 + index as u64 * 2;
        let before_hud = hud(&mut h, qid, vec![]);
        let outcome = h.cmd(
            0,
            protocol::Command::ScreenAction {
                screen: "hud".into(),
                action: "do_action".into(),
                args: vec![("a".into(), i64::from(id)), ("b".into(), i64::from(a.id))],
                text: None,
            },
        );
        match outcome {
            protocol::CommandOutcome::Applied => {
                let after = h.w.fresh_view(0).unwrap();
                let after_hud = hud(&mut h, qid + 1, vec![]);
                assert!(
                    before != after || before_hud != after_hud,
                    "Accepted unit action must change game or HUD facts"
                );
            }
            protocol::CommandOutcome::Rejected { reason, .. } => assert!(!reason.is_empty()),
            _ => panic!("Unit action did not return a final outcome"),
        }
    }
}

#[test]
fn unit_lists_agree_with_the_owned_world_after_founding() {
    let Some(mut h) = start() else { return };
    let lists = hud(&mut h, 1, vec![]).lists;
    let world = h.w.fresh_view(0).unwrap();
    let owned = world.units.iter().filter(|u| u.owner == 0).count();
    assert_eq!(lists.units.len(), owned);
    assert!(lists.units.iter().all(|u| u.hp >= 0 && u.moves >= 0.0));
    for g in lists.great_people.iter() {
        assert!(
            g.rows
                .iter()
                .all(|r| r.per_turn > 0.0 && r.threshold > 0 && r.turns.is_some())
        );
    }
    // Lists use client selection/camera/window intents; they do not mutate the core.
}

#[test]
fn minimap_summary_is_well_formed_for_a_new_seat() {
    let Some(mut h) = start() else { return };
    let root = hud(&mut h, 1, vec![]).minimap;
    assert!(!root.observer);
    assert!(root.religion.is_none() && root.beliefs.is_empty());
    // Minimap clicks move the client camera; renderer and session options never mutate the game core.
}

#[test]
fn on_map_tooltips_include_owned_entities_after_founding() {
    let Some(mut h) = start() else { return };
    let map = hud(&mut h, 1, vec![]).map;
    assert!(!map.cities.is_empty() && !map.units.is_empty());
    assert!(map.units.iter().all(|u| u.hp <= u.hp_max && u.moves >= 0.0));
    assert!(
        map.cities
            .iter()
            .all(|c| c.hp <= c.hp_max && c.strength >= 0)
    );
    // Hover data is read-only. City opening and unit selection are client intents.
}
