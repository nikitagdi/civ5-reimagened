//! Domination progress as VictoryProgress shows it: every original capital and its holder, and the
//! leading civilization; unmet founders and leaders stay anonymous to the seat.
use game_host::Worker;
use protocol::Start;
#[path = "common/real.rs"]
mod common;
#[allow(dead_code)]
#[path = "common/city_screen.rs"]
mod screen_helper;

#[test]
fn domination_lists_original_capitals_and_the_leader() {
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
    for seat in [0, 1] {
        h.found_city(seat);
    }
    // Test-only bridge: seat 0 takes seat 1's capital (far away, unseen, its founder never met).
    let lua = lua_sys::Lua::new().unwrap();
    h.w.core_mut().gamecore_mut().attach_lua(&lua).unwrap();
    lua.exec(
        r#"
        local capital = Players[1]:GetCapitalCity()
        assert(capital and capital:IsOriginalMajorCapital(), "seat 1's original capital")
        fixture_name = capital:GetName()
        fixture_met = Teams[Players[0]:GetTeam()]:IsHasMet(Players[1]:GetTeam()) and 1 or 0
        Players[0]:AcquireCity(capital, false, true)
        local held = 0
        for city in Players[0]:Cities() do if city:IsOriginalMajorCapital() then held = held + 1 end end
        assert(held == 2, "seat 0 holds both original capitals")
        fixture_met_after = Teams[Players[0]:GetTeam()]:IsHasMet(Players[1]:GetTeam()) and 1 or 0
    "#,
        "=domination-fixture",
    )
    .unwrap();
    let name: String = lua.globals().get("fixture_name").unwrap();
    let met_after: i32 = lua.globals().get("fixture_met_after").unwrap();
    drop(lua);
    let root = screen_helper::query_args(&mut h, "victory", vec![]);
    let dom = &root["domination"];
    assert_eq!(dom["own_count"], 2, "{dom}");
    let leader = &dom["leader"];
    assert_eq!(leader["player"], 0, "{dom}");
    assert_eq!(leader["count"], 2);
    assert_eq!(leader["is_team"], false);
    assert!(!leader["name"].as_str().unwrap().is_empty());
    let row = dom["capitals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == 1)
        .expect("seat 1's capital row");
    // The seat holds it, so it knows the holder; the name follows whether it has met the founder.
    assert_eq!(row["owner"], 0, "{row}");
    if met_after == 1 {
        assert_eq!(row["capital_name"], name.as_str());
    } else {
        assert_eq!(row["capital_name"], "Unknown capital");
    }
    // Seat 1 sees the same leader, by name only once it has met seat 0.
    let theirs = query_seat(&mut h, 1);
    let lead = &theirs["domination"]["leader"];
    assert_eq!(lead["count"], 2, "{theirs}");
    if met_after == 1 {
        assert_eq!(lead["player"], 0);
    } else {
        assert_eq!(
            lead["player"], -1,
            "an unmet leader stays anonymous: {lead}"
        );
    }
    let own = theirs["domination"]["capitals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == 1)
        .unwrap();
    assert_eq!(
        own["capital_name"],
        name.as_str(),
        "seat 1 knows its own capital"
    );
    assert_eq!(own["owner_met"], met_after == 1, "{own}");
}

fn query_seat(h: &mut common::H, player: i8) -> serde_json::Value {
    use protocol::{Query, QueryAnswer, ToWorker, WorkerMsg};
    let before = h.frames.len();
    h.send(ToWorker::Query {
        player,
        qid: 77,
        q: Query::Screen {
            screen: "victory".into(),
            args: vec![],
        },
    });
    h.run();
    let json = h.frames[before..]
        .iter()
        .flat_map(|f| &f.msgs)
        .find_map(|m| match m {
            WorkerMsg::QueryResult {
                qid: 77,
                answer: QueryAnswer::Screen { json, .. },
            } => Some(json.clone()),
            _ => None,
        })
        .expect("victory answer");
    serde_json::from_str(&json).unwrap()
}
