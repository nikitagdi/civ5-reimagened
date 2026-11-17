//! HUD service data and actions through a server-role real core.
use game_host::Worker;
use protocol::{Command, CommandOutcome, Query, QueryAnswer, Start, ToWorker, WorkerMsg};
use ui2_model::{ModelStore, Sample, Schema, generated::Hud};

#[path = "common/real.rs"]
mod common;

fn query(h: &mut common::H, id: u64, player: i8, args: Vec<(String, i64)>) -> Result<Hud, String> {
    let before = h.frames.len();
    h.send(ToWorker::Query {
        player,
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
    match answer {
        QueryAnswer::Screen { screen, json } => {
            assert_eq!(screen, "hud");
            let value: serde_json::Value = serde_json::from_str(json).unwrap();
            let sample = Sample::parse(
                Schema::v1(),
                &serde_json::json!({"model":{"hud":value}}).to_string(),
            )
            .map_err(|e| e.join("; "))?;
            let mut store = ModelStore::new(Schema::v1());
            sample.apply(&mut store).unwrap();
            Ok(store.get::<Option<Hud>>().unwrap().clone().unwrap())
        }
        QueryAnswer::Error { reason } => Err(reason.clone()),
        _ => panic!("unexpected HUD answer"),
    }
}

#[test]
fn hud_service_returns_seat_scoped_queues_and_executes_alert() {
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return;
    }
    let (worker, outs) = Worker::new(
        common::real_core(),
        common::init(Start::New(common::setup()), ""),
        0,
    );
    let mut h = common::H::with(worker, outs);
    let root = query(&mut h, 1, 0, vec![]).expect("global HUD");
    assert!(root.city.is_none() && root.unit.is_none());
    assert!(root.research.techs.len() > 50);
    assert!(!root.top.resources.is_empty());
    assert!(!root.top.policies.is_empty());
    assert!(root.top.tourism.is_some());
    let available: Vec<_> = root
        .research
        .techs
        .iter()
        .filter(|t| t.status == 1)
        .take(2)
        .map(|t| t.id)
        .collect();
    assert_eq!(available.len(), 2);
    for (i, tech) in available.iter().enumerate() {
        assert_eq!(
            h.cmd(
                0,
                Command::ResearchSet {
                    tech: *tech,
                    queue: i != 0
                }
            ),
            CommandOutcome::Applied
        );
    }
    h.found_city(0);
    let world = h.w.fresh_view(0).unwrap();
    let city = world.cities.iter().find(|c| c.owner == 0).unwrap().id;
    let build = h.pick_build(0, city);
    assert_eq!(
        h.cmd(
            0,
            Command::CityQueuePush {
                city,
                order: build.order_type,
                item: build.item,
                front: false
            }
        ),
        CommandOutcome::Applied
    );
    let unit = world
        .units
        .iter()
        .find(|u| u.owner == 0 && !u.flags.contains(protocol::UnitFlags::CIVILIAN))
        .expect("combat unit")
        .id;
    let root = query(
        &mut h,
        2,
        0,
        vec![
            ("city".into(), i64::from(city)),
            ("unit".into(), i64::from(unit)),
        ],
    )
    .expect("focused HUD");
    assert_eq!(root.city.as_ref().unwrap().queue.len(), 2);
    assert!(root.city.as_ref().unwrap().overflow >= 0);
    assert!(
        root.research
            .techs
            .iter()
            .any(|t| t.id == available[1] && t.queue == Some(1))
    );
    assert!(
        root.unit
            .as_ref()
            .unwrap()
            .actions
            .iter()
            .any(|a| a.id == 101 && a.enabled)
    );
    assert_eq!(
        h.cmd(
            0,
            Command::ScreenAction {
                screen: "hud".into(),
                action: "alert".into(),
                args: vec![("a".into(), i64::from(unit))],
                text: None
            }
        ),
        CommandOutcome::Applied
    );
    let refreshed =
        query(&mut h, 3, 0, vec![("unit".into(), i64::from(unit))]).expect("after alert");
    assert!(
        !refreshed
            .unit
            .unwrap()
            .actions
            .iter()
            .any(|a| a.id == 101 && a.enabled)
    );
    assert!(query(&mut h, 4, 1, vec![("city".into(), i64::from(city))]).is_err());
    assert!(matches!(
        h.cmd(
            1,
            Command::ScreenAction {
                screen: "hud".into(),
                action: "alert".into(),
                args: vec![("a".into(), i64::from(i32::MAX))],
                text: None
            }
        ),
        CommandOutcome::Rejected { .. }
    ));
}
