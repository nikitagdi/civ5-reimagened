//! The core's seat-scoped read-only Lua queries (advisor counsel, deals) and the screens built on
//! them, on a real game. One test: the real core allows a single game per process.
use game_host::Worker;
use protocol::{Query, QueryAnswer, Start, ToWorker, WorkerMsg};
#[path = "common/real.rs"]
mod common;

fn screen(h: &mut common::H, qid: u64, name: &str, player: i32) -> serde_json::Value {
    h.send(ToWorker::Query {
        player: player as protocol::PlayerId,
        qid,
        q: Query::Screen {
            screen: name.into(),
            args: vec![],
        },
    });
    h.run();
    let json = h
        .msgs()
        .find_map(|m| match m {
            WorkerMsg::QueryResult {
                qid: id,
                answer: QueryAnswer::Screen { json, .. },
            } if *id == qid => Some(json.clone()),
            WorkerMsg::QueryResult {
                qid: id,
                answer: QueryAnswer::Error { reason },
            } if *id == qid => panic!("{name}: {reason}"),
            _ => None,
        })
        .expect("screen answer");
    let root: serde_json::Value = serde_json::from_str(&json).unwrap();
    ui2_model::Sample::parse(
        ui2_model::Schema::v1(),
        &serde_json::json!({"model":{name:root.clone()}}).to_string(),
    )
    .unwrap_or_else(|e| panic!("{name} root matches the schema: {e:?}"));
    root
}

#[test]
fn seat_queries_feed_advisor_and_deal_history() {
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

    // The Lua contract itself, through the test bridge (reads only).
    let lua = lua_sys::Lua::new().unwrap();
    h.w.core_mut().gamecore_mut().attach_lua(&lua).unwrap();
    lua.exec(
        r#"
        local function count(t) local n = 0; for _ in pairs(t) do n = n + 1 end; return n end
        for _, seat in ipairs({0, 1}) do
            local counsel = GcUiGetAdvisorCounsel(seat)
            assert(type(counsel) == "table", "counsel table")
            -- No city yet: every advisor tells the seat to found one (zero-based pages, localized text).
            for _, a in ipairs({AdvisorTypes.ADVISOR_ECONOMIC, AdvisorTypes.ADVISOR_MILITARY,
                                AdvisorTypes.ADVISOR_FOREIGN, AdvisorTypes.ADVISOR_SCIENCE}) do
                assert(counsel[a] and type(counsel[a][0]) == "string" and #counsel[a][0] > 0, "advisor " .. a)
                assert(not string.find(counsel[a][0], "^TXT_KEY_"), "localized counsel")
            end
            assert(count(GcUiGetCurrentDeals(seat)) == 0 and count(GcUiGetHistoricDeals(seat)) == 0)
        end
        assert(GcUiGetProposedDeal(0, 0, 1) == nil)
        assert(not pcall(GcUiGetProposedDeal, 2, 0, 1), "a seat outside the deal is refused")
        assert(not pcall(GcUiGetAdvisorCounsel, -1) and not pcall(GcUiGetCurrentDeals, 999))
    "#,
        "=seat-queries",
    )
    .unwrap();
    drop(lua);

    // Advisor screen: the seat's own counsel, no unavailability reason.
    let advisor = screen(&mut h, 801, "advisor", 0);
    assert!(advisor["reason"].is_null(), "{advisor}");
    let roles = advisor["roles"].as_array().unwrap();
    assert_eq!(roles.len(), 4);
    for role in roles {
        let pages = role["pages"].as_array().unwrap();
        assert!(!pages.is_empty(), "{role}");
        assert_eq!(pages[0]["id"], 0);
        assert!(!pages[0]["text"].as_str().unwrap().is_empty());
    }
    // Seat 1 reads its own counsel too.
    let other = screen(&mut h, 802, "advisor", 1);
    assert!(other["reason"].is_null());

    // Diplomacy: deal history is available (empty at the start of a game).
    let diplo = screen(&mut h, 803, "diplo", 0);
    assert_eq!(diplo["deals_available"], true, "{diplo}");
    assert!(diplo["deals_reason"].is_null());
    assert!(diplo["deals"].as_array().unwrap().is_empty());
}
