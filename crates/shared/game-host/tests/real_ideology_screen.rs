use game_host::Worker;
use protocol::{Command, CommandOutcome, Query, QueryAnswer, Start, ToWorker, WorkerMsg};
use ui2_model::{Sample, Schema};

#[path = "common/real.rs"]
mod common;

fn query(h: &mut common::H, id: u64) -> serde_json::Value {
    let before = h.frames.len();
    h.send(ToWorker::Query {
        player: 0,
        qid: id,
        q: Query::Screen {
            screen: "ideology".into(),
            args: vec![],
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
        .expect("screen answer");
    let QueryAnswer::Screen { json, .. } = answer else {
        panic!("ideology data failed: {answer:?}")
    };
    let root: serde_json::Value = serde_json::from_str(json).unwrap();
    Sample::parse(
        Schema::v1(),
        &serde_json::json!({"model":{"ideology":root}}).to_string(),
    )
    .expect("valid root");
    root
}

#[test]
fn new_game_ideology_root_and_choice() {
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
    let root = query(&mut h, 1);
    assert_eq!(root["options"].as_array().unwrap().len(), 3);
    assert_eq!(root["enabled"], false);
    let id = root["options"][0]["id"].as_i64().unwrap();
    let outcome = h.cmd(
        0,
        Command::ScreenAction {
            screen: "ideology".into(),
            action: "choose".into(),
            args: vec![("a".into(), id)],
            text: None,
        },
    );
    assert!(matches!(outcome, CommandOutcome::Rejected { ref reason, .. } if !reason.is_empty()));
    assert_eq!(root, query(&mut h, 2));
}

/// With an ideology the policy screen's ideology tab reads as the original: the opinion's own text, the core's public
/// opinion tooltip, the title from its text key and the switch button's tooltip.
#[test]
fn ideology_tab_public_opinion_is_the_original_text() {
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
    h.found_city(0);
    // Test-only bridge: seat 0 has the Order ideology.
    let lua = lua_sys::Lua::new().unwrap();
    game_host::Core::attach_screen_lua(h.w.core_mut(), &lua).unwrap();
    lua.exec(
        r#"local p = Players[0]
        p:SetPolicyBranchUnlocked(GameInfoTypes.POLICY_BRANCH_ORDER, true, false)
        assert(p:GetLateGamePolicyTree() == GameInfoTypes.POLICY_BRANCH_ORDER, "the ideology is set")
        content = Locale.ConvertTextKey("TXT_KEY_CO_PUBLIC_OPINION_CONTENT")
        title = Locale.ConvertTextKey("TXT_KEY_POLICYSCREEN_IDEOLOGY_TITLE", p:GetCivilizationAdjectiveKey(), GameInfo.PolicyBranchTypes.POLICY_BRANCH_ORDER.Description)
        disabled = Locale.ConvertTextKey("TXT_KEY_POLICYSCREEN_CHANGE_IDEOLOGY_DISABLED_TT")
        unhappy = p:GetPublicOpinionUnhappiness()"#,
        "=order",
    )
    .unwrap();
    let get = |name: &str| -> String { lua.globals().get(name).unwrap() };
    let unhappy: i64 = lua.globals().get("unhappy").unwrap();
    let before = h.frames.len();
    h.send(ToWorker::Query {
        player: 0,
        qid: 5,
        q: Query::Screen {
            screen: "policies".into(),
            args: vec![("a".into(), 1)],
        },
    });
    h.run();
    let json = h.frames[before..]
        .iter()
        .flat_map(|f| &f.msgs)
        .find_map(|m| match m {
            WorkerMsg::QueryResult {
                qid: 5,
                answer: QueryAnswer::Screen { json, .. },
            } => Some(json.clone()),
            _ => None,
        })
        .expect("policies answer");
    let root: serde_json::Value = serde_json::from_str(&json).unwrap();
    Sample::parse(
        Schema::v1(),
        &serde_json::json!({"model":{"policies":root}}).to_string(),
    )
    .expect("valid root");
    let ideology = &root["ideology"];
    let opinion = &ideology["public_opinion"];
    assert_eq!(opinion["state"], "content", "{opinion}");
    assert_eq!(opinion["label"], get("content").as_str());
    assert_eq!(ideology["title"], get("title").as_str());
    let tooltip = opinion["tooltip"].as_str().unwrap();
    assert!(
        !tooltip.contains('[') && !tooltip.contains("TXT_KEY"),
        "{tooltip}"
    );
    if unhappy <= 0 {
        assert_eq!(ideology["switch"]["reason"], get("disabled").as_str());
    }
}
