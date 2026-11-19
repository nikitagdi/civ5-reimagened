//! No screen query or refused screen action changes the simulation: every registered screen's data,
//! for both seats and with typical arguments, at several points of a turn, leaves the simulation
//! digest as it was. Screens only read, so a lasting effect would be a state change at query time;
//! an equal digest right after the queries means the game goes on exactly as without them.
use game_host::Worker;
use protocol::{Command, Query, Start, ToWorker};
#[path = "common/real.rs"]
mod common;

fn digest(h: &mut common::H) -> u64 {
    h.w.core_mut().gamecore_mut().state_digest(-2).unwrap()
}

/// Every screen, both seats, a few argument sets (failures are fine: they still run the script).
fn query_everything(h: &mut common::H, qid: &mut u64, extra: &[(String, i64)]) {
    for (name, _) in game_host::screens::REGISTRY {
        for seat in [0i8, 1] {
            let mut sets: Vec<Vec<(String, i64)>> = vec![
                vec![],
                vec![("a".into(), 0)],
                vec![("a".into(), 1), ("b".into(), 0)],
            ];
            sets.push(extra.to_vec());
            for args in sets {
                *qid += 1;
                h.send(ToWorker::Query {
                    player: seat,
                    qid: *qid,
                    q: Query::Screen {
                        screen: (*name).into(),
                        args,
                    },
                });
                h.run();
            }
        }
    }
}

#[test]
fn screen_queries_leave_the_simulation_digest_unchanged() {
    if gamecore::IMPLEMENTATION != "lekmod" {
        eprintln!("skipped: linked gamecore is the stub");
        return;
    }
    let mut setup = common::setup();
    setup.settings.minor_civs = 2;
    let (worker, outs) = Worker::new(common::real_core(), common::init(Start::New(setup), ""), 0);
    let mut h = common::H::with(worker, outs);
    let mut qid = 1000;
    let before = digest(&mut h);
    query_everything(&mut h, &mut qid, &[]);
    assert_eq!(digest(&mut h), before, "new game, before any city");
    for seat in [0, 1] {
        h.found_city(seat);
    }
    let city =
        h.w.fresh_view(0)
            .unwrap()
            .cities
            .iter()
            .find(|c| c.owner == 0)
            .unwrap()
            .id;
    let extra = vec![("a".into(), i64::from(city)), ("d".into(), i64::from(city))];
    let before = digest(&mut h);
    query_everything(&mut h, &mut qid, &extra);
    assert_eq!(digest(&mut h), before, "cities founded, mid turn");
    // A refused action runs the script too.
    let _ = h.cmd(
        0,
        Command::ScreenAction {
            screen: "city_state".into(),
            action: "bully_gold".into(),
            args: vec![("a".into(), 99)],
            text: None,
        },
    );
    for seat in [0, 1] {
        h.rest_idle_units(seat);
    }
    h.next_turn(&[0, 1]);
    // Turn 2's human phase: the AI has moved, caches were refreshed by the simulation or are stale.
    let before = digest(&mut h);
    query_everything(&mut h, &mut qid, &extra);
    assert_eq!(digest(&mut h), before, "next turn, human phase");
    query_everything(&mut h, &mut qid, &extra);
    assert_eq!(digest(&mut h), before, "queried twice");
    // The digest does see what an unscoped getter does: the same call outside the read scope draws from the
    // synced game RNG (DS-0038) and changes it.
    let lua = lua_sys::Lua::new().unwrap();
    h.w.core_mut().gamecore_mut().attach_lua(&lua).unwrap();
    lua.exec(
        "for c in Players[0]:Cities() do c:GetNextBuyablePlot() end",
        "=unscoped-probe",
    )
    .unwrap();
    assert_ne!(
        digest(&mut h),
        before,
        "the probe changes the digest outside the scope"
    );
}
