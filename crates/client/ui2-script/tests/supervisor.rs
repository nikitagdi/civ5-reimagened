//! The supervisor against a mock worker host: what is sent per frame, and how answers become output.

use std::sync::Arc;

use platform::{NullScriptHost, ScriptHostError, WorkerId};
use serde_json::Value as Json;
use ui2_model::generated::{Game, Top};
use ui2_model::value::{Key, Text};
use ui2_model::wire::{IslandOp, Origin, ScriptCtxId, ScriptEvent};
use ui2_model::{Fragment, ModelStore, Namespace, Owner, Schema, Value};
use ui2_script::testing::MockScriptHost;
use ui2_script::{
    BUSY_AFTER_MS, FrameInput, KILL_AFTER_MS, LogLevel, MAX_CONTEXTS, MAX_FAILURES,
    READY_TIMEOUT_MS, ScriptBundle, ScriptEnv, ScriptError, ScriptInit, ScriptStatus,
    ScriptSupervisor, SettingsNs, StateNs, check_api,
};

const STATE: &str = "[root]\nsort = \"str\"\nopen = \"bool\"\nshare = \"f32\"\n";

fn env() -> ScriptEnv {
    ScriptEnv {
        lang: "en_US".into(),
        viewport: (1920, 1080),
        scale: 1.0,
        cell: 0,
        skin: "test".into(),
    }
}

/// A store with `top` and `game` written and the skin state mounted, all sent once.
fn store() -> ModelStore {
    let mut store = ModelStore::new(Schema::v1());
    let state = Fragment::parse(STATE).unwrap_or_else(|e| panic!("{e}"));
    store
        .mount(Namespace::State, &state)
        .unwrap_or_else(|e| panic!("{e}"));
    let mut w = store.writer(Owner::Source);
    w.put(Top::default()).unwrap_or_else(|e| panic!("{e}"));
    w.put(Game::default()).unwrap_or_else(|e| panic!("{e}"));
    let _ = store.take_frame();
    store
}

fn skin_bundle(reads: &[&str]) -> ScriptBundle {
    ScriptBundle {
        files: vec![("js/a.js".into(), Arc::from("ui.on('x', () => {});"))],
        reads: reads.iter().map(|s| s.to_string()).collect(),
        state_ns: StateNs::Skin,
        settings_ns: SettingsNs::Skin(Arc::from("test")),
        intent_prefix: None,
    }
}

fn mod_bundle(id: &str) -> ScriptBundle {
    ScriptBundle {
        files: vec![("js/m.js".into(), Arc::from(""))],
        reads: vec!["top".into()],
        state_ns: StateNs::Mod(Arc::from(id)),
        settings_ns: SettingsNs::Mod(Arc::from(id)),
        intent_prefix: Some(format!("mods.{id}.")),
    }
}

fn start(
    sup: &mut ScriptSupervisor,
    store: &ModelStore,
    ctx: ScriptCtxId,
    bundle: ScriptBundle,
) -> Result<(), ScriptError> {
    let init = ScriptInit::from_store(store, &bundle, vec![("vol".into(), "3".into())], env())?;
    sup.start(ctx, bundle, init)
}

fn json(text: &str) -> Json {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("{e}: {text}"))
}

fn setup(reads: &[&str]) -> (ScriptSupervisor, MockScriptHost, ModelStore, WorkerId) {
    let host = MockScriptHost::new();
    let mut sup = ScriptSupervisor::new(Box::new(host.clone()));
    let store = store();
    start(&mut sup, &store, ScriptCtxId::Skin, skin_bundle(reads))
        .unwrap_or_else(|e| panic!("{e}"));
    let w = host.live()[0];
    (sup, host, store, w)
}

#[test]
fn init_carries_files_model_tables_and_namespaces() {
    let (sup, host, _store, w) = setup(&["top"]);
    let posted = host.take_posted();
    assert_eq!(posted.len(), 1);
    assert_eq!(posted[0].0, w);
    let m = json(&posted[0].1);
    assert_eq!(m["t"], "init");
    assert_eq!(m["api"], ui2_script::UI_SCRIPT_API);
    assert_eq!(m["ctx"], "skin");
    assert_eq!(m["files"][0]["path"], "js/a.js");
    assert_eq!(m["reads"], json(r#"["top","ui","state"]"#));
    assert_eq!(m["ns"]["state"], "state");
    assert_eq!(m["ns"]["intents"], Json::Null);
    assert_eq!(m["settings"], json(r#"[["vol","3"]]"#));
    assert_eq!(m["env"]["viewport"], json("[1920,1080]"));
    // The snapshot holds the watched roots only: the hello op, then top, ui and state.
    let model = m["model"].as_array().unwrap_or_else(|| panic!("model"));
    assert_eq!(model[1][0], 5);
    let roots: Vec<u64> = model[2..]
        .iter()
        .map(|o| o[1].as_u64().unwrap_or(999))
        .collect();
    let s = Schema::v1();
    let id = |n: &str| u64::from(s.root_by_name(n).unwrap_or_else(|| panic!("{n}")).0);
    assert!(roots.contains(&id("top")) && roots.contains(&id("ui")));
    assert!(!roots.contains(&id("game")));
    // Tables: the schema, then the state fragment after the schema's ids.
    let parts = m["schema"]["parts"]
        .as_array()
        .unwrap_or_else(|| panic!("parts"));
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0]["base"], 0);
    assert_eq!(parts[1]["base"], s.paths().len() as u64);
    assert_eq!(parts[1]["roots"][0][1], "state");
    assert_eq!(parts[1]["roots"][0][0], s.roots().len() as u64);
    assert_eq!(
        parts[1]["paths"][1],
        json(r#"["state.sort","str",null,false]"#)
    );
    assert_eq!(m["schema"]["hash"], format!("{:016x}", s.hash()));
    assert_eq!(sup.status(&ScriptCtxId::Skin), Some(ScriptStatus::Starting));
}

#[test]
fn ready_makes_the_context_run() {
    let (mut sup, host, _store, w) = setup(&["top"]);
    host.reply(
        w,
        r#"{"t":"ready","ctx":"skin","handlers":["sort","ready"],"ms":2}"#,
    );
    let out = sup.drain(0.0);
    assert!(out.failed.is_empty());
    assert_eq!(sup.status(&ScriptCtxId::Skin), Some(ScriptStatus::Running));
    assert_eq!(sup.handlers(&ScriptCtxId::Skin), ["sort", "ready"]);
    assert_eq!(sup.stats().running, 1);
}

#[test]
fn nothing_changed_nothing_is_sent() {
    let (mut sup, host, _store, _w) = setup(&["top"]);
    host.take_posted();
    sup.post(0.0, &FrameInput::default());
    assert!(host.take_posted().is_empty());
    // A delta the context does not watch is no reason either.
    let mut store = store();
    store
        .writer(Owner::Source)
        .set_value("game.turn", Value::I32(7))
        .unwrap_or_else(|e| panic!("{e}"));
    let delta = store.take_frame().unwrap_or_else(|| panic!("a delta"));
    sup.post(
        0.0,
        &FrameInput {
            delta: Some(&delta),
            ..FrameInput::default()
        },
    );
    assert!(host.take_posted().is_empty());
}

#[test]
fn the_delta_is_filtered_by_reads() {
    let (mut sup, host, mut store, w) = setup(&["top"]);
    host.take_posted();
    let mut wr = store.writer(Owner::Source);
    wr.set_value("game.turn", Value::I32(7))
        .unwrap_or_else(|e| panic!("{e}"));
    wr.set_value("top.gold", Value::I32(99))
        .unwrap_or_else(|e| panic!("{e}"));
    let delta = store.take_frame().unwrap_or_else(|| panic!("a delta"));
    sup.post(
        5.0,
        &FrameInput {
            delta: Some(&delta),
            ..FrameInput::default()
        },
    );
    let posted = host.take_posted();
    assert_eq!(posted.len(), 1);
    assert_eq!(posted[0].0, w);
    let m = json(&posted[0].1);
    assert_eq!(
        (m["t"].as_str(), m["seq"].as_u64()),
        (Some("frame"), Some(1))
    );
    let gold = Schema::v1()
        .resolve("top.gold")
        .unwrap_or_else(|e| panic!("{e}"))
        .id
        .0;
    assert_eq!(
        m["model"],
        json(&format!("[{},[0,{gold},[],99]]", delta.revision))
    );
    assert_eq!(m["events"], json("[]"));
    assert!(m.get("env").is_none());
}

#[test]
fn events_go_to_their_context_and_env_to_all() {
    let (mut sup, host, store, w) = setup(&["top"]);
    start(
        &mut sup,
        &store,
        ScriptCtxId::Mod(Arc::from("hello")),
        mod_bundle("hello"),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    host.take_posted();
    let ev = |ctx: ScriptCtxId, name: &str| ScriptEvent {
        ctx,
        name: Arc::from(name),
        arg: Value::Key(Key::new(1, 2)),
        node_path: Arc::from("hud.html:3"),
    };
    let events = [
        ev(ScriptCtxId::Skin, "sort"),
        ev(ScriptCtxId::Mod(Arc::from("hello")), "poke"),
    ];
    sup.post(
        0.0,
        &FrameInput {
            delta: None,
            events: &events[..1],
            env: None,
            store: None,
        },
    );
    let posted = host.take_posted();
    assert_eq!(posted.len(), 1, "only the skin has an event");
    assert_eq!(posted[0].0, w);
    let m = json(&posted[0].1);
    assert_eq!(m["model"], Json::Null);
    assert_eq!(
        m["events"],
        json(r#"[{"name":"sort","arg":"1:2","node":"hud.html:3"}]"#)
    );
    let e = env();
    sup.post(
        0.0,
        &FrameInput {
            delta: None,
            events: &[],
            env: Some(&e),
            store: None,
        },
    );
    assert_eq!(
        host.take_posted().len(),
        2,
        "a new environment goes to everyone"
    );
}

#[test]
fn answers_become_output() {
    let (mut sup, host, mut store, w) = setup(&["top"]);
    store
        .writer(Owner::Source)
        .set_value("top.gold", Value::I32(1))
        .unwrap_or_else(|e| panic!("{e}"));
    let delta = store.take_frame().unwrap_or_else(|| panic!("a delta"));
    sup.post(
        0.0,
        &FrameInput {
            delta: Some(&delta),
            ..FrameInput::default()
        },
    );
    host.reply(
        w,
        r#"{"t":"out","n":1,"ack":1,
            "state":[{"p":"state.sort","v":"turns"},{"p":"state.share","v":1},{"p":"ui.production.tab","v":"wonders"}],
            "intents":[{"name":"unit.action","args":{"unit":"0:7","action":"fortify"}}],
            "cmds":[{"island":"clock","key":"t","op":"show","on":false}],
            "settings":[{"key":"vol","value":{"a":1}},{"key":"old","value":null}],
            "log":[{"level":"warn","text":"hm","file":"js/a.js","line":3}],
            "busy_ms":0.5}"#,
    );
    let out = sup.drain(1.0);
    let ops: Vec<(&str, &Value)> = out
        .state_ops
        .iter()
        .map(|(_, o)| (&*o.path, &o.value))
        .collect();
    assert_eq!(ops[0], ("state.sort", &Value::Str(Arc::from("turns"))));
    // Numbers take the type of the field: an `f32` state field gets an `F32`.
    assert_eq!(ops[1], ("state.share", &Value::F32(1.0)));
    let tab = Schema::v1()
        .enums()
        .iter()
        .find(|e| e.name == "ProdTab")
        .and_then(|e| e.variants.iter().position(|v| v == "wonders"))
        .unwrap_or_else(|| panic!("ProdTab"));
    assert_eq!(ops[2], ("ui.production.tab", &Value::Enum(tab as u16)));
    let (ctx, intent) = &out.intents[0];
    assert_eq!(*ctx, ScriptCtxId::Skin);
    assert_eq!(&*intent.name, "unit.action");
    assert_eq!(intent.origin, Origin::Script);
    assert!(
        intent
            .args
            .iter()
            .any(|(k, v)| &**k == "unit" && *v == Value::Key(Key::new(0, 7)))
    );
    assert!(
        intent
            .args
            .iter()
            .any(|(k, v)| &**k == "action" && matches!(v, Value::Enum(_)))
    );
    assert_eq!(out.island_cmds[0].1.op, IslandOp::Show(false));
    assert_eq!(
        out.settings,
        vec![
            (
                ScriptCtxId::Skin,
                "vol".to_string(),
                Some(r#"{"a":1}"#.to_string())
            ),
            (ScriptCtxId::Skin, "old".to_string(), None),
        ]
    );
    let warn = out
        .log
        .iter()
        .find(|(_, l)| l.level == LogLevel::Warn)
        .unwrap_or_else(|| panic!("log"));
    assert_eq!(
        (warn.1.file.as_deref(), warn.1.line),
        (Some("js/a.js"), Some(3))
    );
    assert_eq!(sup.stats().worker_ms, 0.5);
    // A second drain has nothing.
    assert!(sup.drain(2.0).is_empty());
}

#[test]
fn frames_count_and_acks_are_checked() {
    let (mut sup, host, _store, w) = setup(&["top"]);
    let e = env();
    for _ in 0..3 {
        sup.post(
            0.0,
            &FrameInput {
                env: Some(&e),
                ..FrameInput::default()
            },
        );
    }
    let seqs: Vec<u64> = host
        .take_posted()
        .iter()
        .skip(1)
        .map(|(_, t)| json(t)["seq"].as_u64().unwrap_or(0))
        .collect();
    assert_eq!(seqs, [1, 2, 3]);
    host.reply(w, r#"{"t":"out","n":1,"ack":1}"#);
    host.reply(w, r#"{"t":"out","n":2,"ack":1}"#);
    host.reply(
        w,
        r#"{"t":"out","n":3,"ack":3,"state":[{"p":"state.open","v":true}]}"#,
    );
    let out = sup.drain(0.0);
    assert_eq!(
        out.log
            .iter()
            .filter(|(_, l)| l.level == LogLevel::Warn)
            .count(),
        1,
        "the repeated ack is reported"
    );
    assert_eq!(out.state_ops.len(), 1);
    // The next frame tells the worker which of its answers the model already holds.
    sup.post(
        0.0,
        &FrameInput {
            env: Some(&e),
            ..FrameInput::default()
        },
    );
    let m = json(&host.take_posted()[0].1);
    assert_eq!(
        (m["seq"].as_u64(), m["applied"].as_u64()),
        (Some(4), Some(3))
    );
}

#[test]
fn a_mod_raises_only_its_own_intents() {
    let host = MockScriptHost::new();
    let mut sup = ScriptSupervisor::new(Box::new(host.clone()));
    let store = store();
    let ctx = ScriptCtxId::Mod(Arc::from("hello"));
    start(&mut sup, &store, ctx.clone(), mod_bundle("hello")).unwrap_or_else(|e| panic!("{e}"));
    let w = host.live()[0];
    let init = json(&host.take_posted()[0].1);
    assert_eq!(init["ctx"], "mod:hello");
    assert_eq!(init["ns"]["intents"], "mods.hello.");
    host.reply(
        w,
        r#"{"t":"out","n":1,"ack":null,"intents":[{"name":"turn.primary"},{"name":"mods.hello.poke","args":{"n":2}}]}"#,
    );
    let out = sup.drain(0.0);
    assert_eq!(out.intents.len(), 1);
    assert_eq!(&*out.intents[0].1.name, "mods.hello.poke");
    assert_eq!(out.intents[0].1.args, vec![(Arc::from("n"), Value::I32(2))]);
    assert!(
        out.log
            .iter()
            .any(|(c, l)| *c == ctx && l.text.contains("turn.primary"))
    );
}

#[test]
fn a_skin_that_fails_to_load_is_fatal() {
    let (mut sup, host, _store, w) = setup(&["top"]);
    host.reply(
        w,
        r#"{"t":"fail","ctx":"skin","error":{"message":"x is not defined","file":"js/a.js","line":4}}"#,
    );
    let out = sup.drain(0.0);
    assert_eq!(out.failed, vec![ScriptCtxId::Skin]);
    let (ctx, why) = sup.take_fatal().unwrap_or_else(|| panic!("fatal"));
    assert_eq!(ctx, ScriptCtxId::Skin);
    assert!(why.contains("x is not defined at js/a.js:4"), "{why}");
    assert!(sup.take_fatal().is_none());
    assert!(matches!(
        sup.status(&ctx),
        Some(ScriptStatus::Disabled { .. })
    ));
    assert_eq!(host.terminated(), vec![w]);
    // A switched-off context gets no frames.
    let e = env();
    sup.post(
        0.0,
        &FrameInput {
            env: Some(&e),
            ..FrameInput::default()
        },
    );
    assert!(
        host.take_posted()
            .iter()
            .all(|(_, t)| json(t)["t"] == "init")
    );
}

#[test]
fn a_mod_that_fails_restarts_then_is_switched_off_without_fatal() {
    let host = MockScriptHost::new();
    let mut sup = ScriptSupervisor::new(Box::new(host.clone()));
    let store = store();
    let ctx = ScriptCtxId::Mod(Arc::from("hello"));
    start(&mut sup, &store, ctx.clone(), mod_bundle("hello")).unwrap_or_else(|e| panic!("{e}"));
    for round in 0..MAX_FAILURES {
        let w = host.live()[0];
        host.error(w, "worker.js: 404");
        let out = sup.drain(round as f64);
        assert!(sup.take_fatal().is_none());
        if round + 1 < MAX_FAILURES {
            assert!(out.failed.is_empty(), "round {round}");
            assert_eq!(sup.status(&ctx), Some(ScriptStatus::Restarting));
            sup.post(round as f64, &with_store(&store));
            assert_eq!(host.live().len(), 1, "a new worker");
        } else {
            assert_eq!(out.failed, vec![ctx.clone()]);
            assert!(matches!(
                sup.status(&ctx),
                Some(ScriptStatus::Disabled { .. })
            ));
        }
    }
    assert!(host.live().is_empty());
    assert_eq!(sup.stats().errors, MAX_FAILURES as u64);
    assert_eq!(sup.stats().restarts, MAX_FAILURES as u64 - 1);
}

fn with_store(store: &ModelStore) -> FrameInput<'_> {
    FrameInput {
        store: Some(store),
        ..FrameInput::default()
    }
}

fn ready(sup: &mut ScriptSupervisor, host: &MockScriptHost, w: WorkerId, now: f64) {
    host.reply(w, r#"{"t":"ready","ctx":"skin","handlers":[],"ms":1}"#);
    let _ = sup.drain(now);
    host.take_posted();
}

fn gold(store: &mut ModelStore, n: i32) -> ui2_model::FrameDelta {
    store
        .writer(Owner::Source)
        .set_value("top.gold", Value::I32(n))
        .unwrap_or_else(|e| panic!("{e}"));
    store.take_frame().unwrap_or_else(|| panic!("a delta"))
}

#[test]
fn a_busy_context_gets_its_frames_merged_after_the_answer() {
    let (mut sup, host, mut store, w) = setup(&["top"]);
    ready(&mut sup, &host, w, 0.0);
    let d1 = gold(&mut store, 1);
    sup.post(
        0.0,
        &FrameInput {
            delta: Some(&d1),
            ..FrameInput::default()
        },
    );
    assert_eq!(host.take_posted().len(), 1);
    assert!(sup.drain(BUSY_AFTER_MS - 1.0).is_empty());
    assert_eq!(sup.status(&ScriptCtxId::Skin), Some(ScriptStatus::Running));
    let _ = sup.drain(BUSY_AFTER_MS + 10.0);
    assert_eq!(
        sup.status(&ScriptCtxId::Skin),
        Some(ScriptStatus::Busy {
            ms: BUSY_AFTER_MS + 10.0
        })
    );
    // Busy: two more frames are held back.
    for n in [2, 3] {
        let d = gold(&mut store, n);
        let ev = [ScriptEvent {
            ctx: ScriptCtxId::Skin,
            name: Arc::from(format!("e{n}").as_str()),
            arg: Value::None,
            node_path: Arc::from(""),
        }];
        sup.post(
            300.0,
            &FrameInput {
                delta: Some(&d),
                events: &ev,
                ..FrameInput::default()
            },
        );
    }
    assert!(host.take_posted().is_empty());
    host.reply(w, r#"{"t":"out","n":1,"ack":1}"#);
    let _ = sup.drain(400.0);
    assert_eq!(sup.status(&ScriptCtxId::Skin), Some(ScriptStatus::Running));
    // The next frame carries what waited: one set of `top.gold` (the last value) and both events.
    sup.post(410.0, &FrameInput::default());
    let posted = host.take_posted();
    assert_eq!(posted.len(), 1);
    let m = json(&posted[0].1);
    assert_eq!(m["seq"], 2);
    let ops = m["model"].as_array().unwrap_or_else(|| panic!("model"));
    assert_eq!(ops.len(), 2, "revision and one op: {m}");
    assert_eq!(ops[1][3], 3);
    let names: Vec<&str> = m["events"]
        .as_array()
        .unwrap_or_else(|| panic!("events"))
        .iter()
        .filter_map(|e| e["name"].as_str())
        .collect();
    assert_eq!(names, ["e2", "e3"]);
}

#[test]
fn a_hung_worker_is_killed_and_restarted_from_a_snapshot() {
    let (mut sup, host, mut store, w) = setup(&["top"]);
    ready(&mut sup, &host, w, 0.0);
    host.reply(
        w,
        r#"{"t":"out","n":1,"ack":null,"settings":[{"key":"vol","value":7}]}"#,
    );
    let _ = sup.drain(1.0);
    let d = gold(&mut store, 5);
    sup.post(
        10.0,
        &FrameInput {
            delta: Some(&d),
            ..FrameInput::default()
        },
    );
    host.take_posted();
    let out = sup.drain(10.0 + KILL_AFTER_MS);
    assert!(
        out.log
            .iter()
            .any(|(_, l)| l.text == "ui2.js: skin killed after 1000 ms"),
        "{:?}",
        out.log
    );
    assert!(out.failed.is_empty());
    assert_eq!(host.terminated(), vec![w]);
    assert_eq!(
        sup.status(&ScriptCtxId::Skin),
        Some(ScriptStatus::Restarting)
    );
    // Without a store the restart waits; with one, a new worker gets the model as it is now.
    sup.post(1100.0, &FrameInput::default());
    assert!(host.live().is_empty());
    let d = gold(&mut store, 6);
    sup.post(
        1110.0,
        &FrameInput {
            delta: Some(&d),
            store: Some(&store),
            ..FrameInput::default()
        },
    );
    let w2 = host.live()[0];
    assert_ne!(w2, w);
    let posted = host.take_posted();
    assert_eq!(posted.len(), 1, "only the init: the snapshot has the delta");
    let m = json(&posted[0].1);
    assert_eq!(m["t"], "init");
    assert_eq!(m["settings"], json(r#"[["vol","7"]]"#));
    // The snapshot resets `top` with the gold written in the frame of the restart.
    let s = Schema::v1();
    let top = u64::from(s.root_by_name("top").unwrap_or_else(|| panic!("top")).0);
    let gold_at = s
        .types()
        .iter()
        .find(|t| t.name == "Top")
        .and_then(|t| t.fields.iter().position(|f| f.name == "gold"))
        .unwrap_or_else(|| panic!("Top.gold"));
    let reset = m["model"]
        .as_array()
        .and_then(|a| a.iter().find(|o| o[0] == 4 && o[1] == top))
        .unwrap_or_else(|| panic!("no reset of top: {}", m["model"]));
    assert_eq!(reset[2][gold_at], 6);
    assert_eq!(sup.stats().restarts, 1);
    // Its frames count from 1 again.
    ready(&mut sup, &host, w2, 1200.0);
    assert_eq!(sup.status(&ScriptCtxId::Skin), Some(ScriptStatus::Running));
    let d = gold(&mut store, 7);
    sup.post(
        1210.0,
        &FrameInput {
            delta: Some(&d),
            ..FrameInput::default()
        },
    );
    assert_eq!(json(&host.take_posted()[0].1)["seq"], 1);
}

/// While the page holds the workers' messages back (it has input to handle and long frames: a browser runs
/// those first), an answer the worker posted in time is not drained yet. The wait counts up to the time the page
/// took messages in (`delivered_until`), not up to now: an idle skin is not killed as hung. A worker that does
/// hang is still killed once the page has taken in messages past its deadline.
#[test]
fn a_worker_is_not_killed_for_answers_the_page_has_not_taken_in() {
    let (mut sup, host, mut store, w) = setup(&["top"]);
    ready(&mut sup, &host, w, 0.0);
    let d = gold(&mut store, 5);
    sup.post(
        10.0,
        &FrameInput {
            delta: Some(&d),
            ..FrameInput::default()
        },
    );
    host.take_posted();
    let killed = |out: &ui2_script::ScriptOutput| {
        out.log
            .iter()
            .any(|(_, l)| l.text.starts_with("ui2.js: skin killed"))
    };
    // 1.5 s later the page has taken in what was posted up to 200 ms only (typing over long frames).
    host.set_delivered_until(Some(200.0));
    let out = sup.drain(10.0 + KILL_AFTER_MS + 500.0);
    assert!(!killed(&out), "{:?}", out.log);
    assert!(host.terminated().is_empty());
    assert_eq!(sup.status(&ScriptCtxId::Skin), Some(ScriptStatus::Running));
    // The page catches up: the answer was there all along.
    host.set_delivered_until(Some(1600.0));
    host.reply(w, r#"{"t":"out","n":1,"ack":1}"#);
    let out = sup.drain(1700.0);
    assert!(!killed(&out), "{:?}", out.log);
    assert_eq!(sup.status(&ScriptCtxId::Skin), Some(ScriptStatus::Running));
    // A frame the worker never answers: not killed while the page has taken in less than the deadline's worth...
    let d = gold(&mut store, 6);
    sup.post(
        1710.0,
        &FrameInput {
            delta: Some(&d),
            ..FrameInput::default()
        },
    );
    host.take_posted();
    host.set_delivered_until(Some(2600.0));
    let out = sup.drain(3000.0);
    assert!(!killed(&out), "{:?}", out.log);
    // ...and killed once the page has taken in everything posted up to the deadline without its answer.
    host.set_delivered_until(Some(2710.0));
    let out = sup.drain(3016.0);
    assert!(killed(&out), "{:?}", out.log);
    assert_eq!(host.terminated(), vec![w]);
    assert_eq!(
        sup.status(&ScriptCtxId::Skin),
        Some(ScriptStatus::Restarting)
    );
}

#[test]
fn three_kills_in_a_minute_switch_the_context_off() {
    let (mut sup, host, mut store, w) = setup(&["top"]);
    let mut worker = w;
    let mut now = 0.0;
    for round in 0..MAX_FAILURES {
        ready(&mut sup, &host, worker, now);
        let d = gold(&mut store, round as i32 + 1);
        sup.post(
            now,
            &FrameInput {
                delta: Some(&d),
                ..FrameInput::default()
            },
        );
        now += KILL_AFTER_MS;
        let out = sup.drain(now);
        if round + 1 < MAX_FAILURES {
            assert!(out.failed.is_empty());
            sup.post(now, &with_store(&store));
            worker = host.live()[0];
        } else {
            assert_eq!(out.failed, vec![ScriptCtxId::Skin]);
            assert!(matches!(
                sup.status(&ScriptCtxId::Skin),
                Some(ScriptStatus::Disabled { .. })
            ));
            assert!(sup.take_fatal().is_none(), "the skin keeps its markup");
        }
    }
    assert!(host.live().is_empty());
    // Off: no more frames, no restart.
    sup.post(now + 1.0, &with_store(&store));
    assert!(host.live().is_empty());
}

#[test]
fn a_worker_that_never_gets_ready_is_killed() {
    let (mut sup, host, store, w) = setup(&["top"]);
    let _ = sup.drain(0.0);
    let _ = sup.drain(READY_TIMEOUT_MS - 1.0);
    assert_eq!(sup.status(&ScriptCtxId::Skin), Some(ScriptStatus::Starting));
    let out = sup.drain(READY_TIMEOUT_MS);
    assert!(out.log.iter().any(|(_, l)| l.text.contains("not ready")));
    assert_eq!(host.terminated(), vec![w]);
    sup.post(READY_TIMEOUT_MS, &with_store(&store));
    assert_eq!(host.live().len(), 1);
}

#[test]
fn an_error_is_logged_once_until_the_context_starts_anew() {
    let (mut sup, host, store, w) = setup(&["top"]);
    let line = r#"{"t":"out","n":1,"ack":null,"log":[{"level":"error","text":"ui2.js: skin on(x) disabled: boom","key":"on(x)|boom"},{"level":"info","text":"hello"}]}"#;
    host.reply(w, line);
    host.reply(w, line);
    let out = sup.drain(0.0);
    let errors = |out: &ui2_script::ScriptOutput| {
        out.log
            .iter()
            .filter(|(_, l)| l.level == LogLevel::Error)
            .count()
    };
    assert_eq!(errors(&out), 1);
    assert_eq!(out.log.iter().filter(|(_, l)| l.text == "hello").count(), 2);
    assert_eq!(sup.stats().errors, 2, "the counter counts each one");
    // A new skin (the context started again) may log it again.
    start(&mut sup, &store, ScriptCtxId::Skin, skin_bundle(&["top"]))
        .unwrap_or_else(|e| panic!("{e}"));
    let w2 = host.live()[0];
    host.reply(w2, line);
    assert_eq!(errors(&sup.drain(1.0)), 1);
}

#[test]
fn at_most_eight_contexts() {
    let host = MockScriptHost::new();
    let mut sup = ScriptSupervisor::new(Box::new(host.clone()));
    let store = store();
    start(&mut sup, &store, ScriptCtxId::Skin, skin_bundle(&["top"]))
        .unwrap_or_else(|e| panic!("{e}"));
    for i in 1..MAX_CONTEXTS {
        let id = format!("m{i}");
        start(
            &mut sup,
            &store,
            ScriptCtxId::Mod(Arc::from(id.as_str())),
            mod_bundle(&id),
        )
        .unwrap_or_else(|e| panic!("{e}"));
    }
    let r = start(
        &mut sup,
        &store,
        ScriptCtxId::Mod(Arc::from("ninth")),
        mod_bundle("ninth"),
    );
    assert_eq!(r, Err(ScriptError::TooMany));
    assert_eq!(host.live().len(), MAX_CONTEXTS);
    // Restarting an existing context replaces it.
    start(&mut sup, &store, ScriptCtxId::Skin, skin_bundle(&["top"]))
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(host.live().len(), MAX_CONTEXTS);
    assert_eq!(host.terminated(), vec![WorkerId(0)]);
}

#[test]
fn stop_ends_the_worker_and_its_answers() {
    let (mut sup, host, _store, w) = setup(&["top"]);
    host.reply(
        w,
        r#"{"t":"out","n":1,"ack":null,"state":[{"p":"state.open","v":true}]}"#,
    );
    host.take_posted();
    sup.stop(&ScriptCtxId::Skin);
    assert_eq!(json(&host.take_posted()[0].1)["t"], "stop");
    assert_eq!(host.terminated(), vec![w]);
    assert!(sup.drain(0.0).is_empty());
    assert_eq!(sup.status(&ScriptCtxId::Skin), None);
}

#[test]
fn a_dropped_supervisor_ends_its_workers() {
    let (sup, host, _store, w) = setup(&["top"]);
    assert!(!sup.is_empty());
    host.take_posted();
    drop(sup);
    assert_eq!(json(&host.take_posted()[0].1)["t"], "stop");
    assert_eq!(host.terminated(), vec![w]);
    assert!(host.live().is_empty());
}

#[test]
fn stats_become_the_js_counters() {
    let (mut sup, host, _store, w) = setup(&["top"]);
    host.reply(w, r#"{"t":"ready","ctx":"skin","handlers":[],"ms":1}"#);
    host.reply(w, r#"{"t":"out","n":1,"ack":null,"busy_ms":0.25}"#);
    sup.drain(0.0);
    let c = sup.stats().counters(1.5);
    let get = |name: &str| {
        c.iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| *v)
            .unwrap_or_else(|| panic!("{name}"))
    };
    assert_eq!(get("ui2.js.main_ms"), 1.5);
    assert_eq!(get("ui2.js.worker_ms"), 0.25);
    assert_eq!(get("ui2.js.posted"), 1.0, "the init");
    assert_eq!(get("ui2.js.received"), 2.0);
    assert_eq!(get("ui2.js.errors"), 0.0);
    assert_eq!(get("ui2.js.restarts"), 0.0);
    assert_eq!(get("ui2.js.contexts"), 1.0);
}

#[test]
fn without_workers_start_is_refused() {
    let mut sup = ScriptSupervisor::new(Box::new(NullScriptHost));
    let r = start(&mut sup, &store(), ScriptCtxId::Skin, skin_bundle(&["top"]));
    assert_eq!(r, Err(ScriptError::Host(ScriptHostError::Unsupported)));
    assert_eq!(sup.stats().contexts, 0);
    assert!(sup.is_empty());
}

#[test]
fn unknown_reads_are_refused() {
    let r = ScriptInit::from_store(&store(), &skin_bundle(&["top.nothing"]), Vec::new(), env());
    assert!(matches!(r, Err(ScriptError::Reads(_))));
}

#[test]
fn api_versions() {
    assert!(check_api("1.0.0").is_ok());
    assert!(check_api("1.4.0").is_ok());
    assert!(check_api("2.0.0").is_err());
    assert!(check_api("x").is_err());
}

#[test]
fn garbage_from_a_worker_is_logged() {
    let (mut sup, host, _store, w) = setup(&["top"]);
    host.reply(w, "{not json");
    let out = sup.drain(0.0);
    assert!(out.log.iter().any(|(_, l)| l.level == LogLevel::Error));
    assert!(out.failed.is_empty());
}

#[test]
fn scripts_write_their_state_and_the_js_fields_of_ui_only() {
    let (mut sup, host, _store, w) = setup(&["top"]);
    host.reply(
        w,
        r#"{"t":"out","n":1,"ack":null,"state":[
            {"p":"state.sort","v":"a"},
            {"p":"state","v":{"sort":"b","open":true,"share":0}},
            {"p":"ui.production.tab","v":"wonders"},
            {"p":"ui.production","v":null},
            {"p":"top.gold","v":1},
            {"p":"stateful","v":1}]}"#,
    );
    let out = sup.drain(0.0);
    let paths: Vec<&str> = out.state_ops.iter().map(|(_, o)| &*o.path).collect();
    assert_eq!(paths, ["state.sort", "state", "ui.production.tab"]);
    let dropped: Vec<&str> = out
        .log
        .iter()
        .filter(|(_, l)| l.level == LogLevel::Warn)
        .map(|(_, l)| l.text.as_str())
        .collect();
    assert_eq!(dropped.len(), 3, "{dropped:?}");
    assert!(dropped[0].contains("`ui.production` dropped: not a field of `ui`"));
}

#[test]
fn island_commands_settings_and_limits() {
    let (mut sup, host, _store, w) = setup(&["top"]);
    let init = json(&host.take_posted()[0].1);
    assert_eq!(
        init["limits"],
        json(r#"{"slow_ms":50,"log_per_s":20,"timer_min_ms":50}"#)
    );
    host.reply(
        w,
        r#"{"t":"out","n":1,"ack":null,"cmds":[
            {"island":"clock","key":"t","op":"text","value":"12:00"},
            {"island":"clock","key":"u","op":"text","value":{"loc":"TXT_KEY_TURN","args":["5"]}},
            {"island":"clock","key":"t","op":"cls","name":"lit","on":false},
            {"island":"clock","key":"t","op":"style","prop":"opacity","value":0.5},
            {"island":"clock","key":"u","op":"show","on":true},
            {"island":"","key":"u","op":"show","on":true}],
          "settings":[{"key":"layout","value":{"a":[1,2]}},{"key":"vol","value":null}]}"#,
    );
    let out = sup.drain(0.0);
    let ops: Vec<IslandOp> = out.island_cmds.iter().map(|(_, c)| c.op.clone()).collect();
    assert_eq!(ops.len(), 5, "the one without an island is dropped");
    assert_eq!(ops[0], IslandOp::Text(Value::Text(Text::raw("12:00"))));
    assert_eq!(
        ops[1],
        IslandOp::Text(Value::Text(Text::key("TXT_KEY_TURN", &["5"])))
    );
    assert_eq!(ops[2], IslandOp::Cls(Arc::from("lit"), false));
    assert_eq!(
        ops[3],
        IslandOp::Style(Arc::from("opacity"), Value::F32(0.5))
    );
    assert_eq!(ops[4], IslandOp::Show(true));
    assert!(out.island_cmds.iter().all(|(_, c)| &*c.island == "clock"));
    // Settings go to the actions module as the script wrote them.
    assert_eq!(
        out.settings,
        vec![
            (
                ScriptCtxId::Skin,
                "layout".to_string(),
                Some(r#"{"a":[1,2]}"#.to_string())
            ),
            (ScriptCtxId::Skin, "vol".to_string(), None),
        ]
    );
}

/// The out of `ui2/wire.test.mjs`: the JSON text of the old wire, then the bytes of wire 2.
fn out_fixture() -> (String, Vec<u8>) {
    let text = include_str!("data/out-wire2.txt");
    let (json, hex) = text
        .trim_end()
        .split_once('\t')
        .unwrap_or_else(|| panic!("fixture"));
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or_else(|e| panic!("{e}")))
        .collect();
    (json.to_string(), bytes)
}

/// Wire 2 when the host takes byte messages: `init` says so, the frames stay JSON texts, and an out of bytes
/// gives the output the same out as JSON gives.
#[test]
fn wire_2_outs_read_like_json_outs() {
    let run = |bytes: bool| {
        let host = MockScriptHost::new();
        host.carry_bytes(bytes);
        let mut sup = ScriptSupervisor::new(Box::new(host.clone()));
        let mut store = store();
        start(&mut sup, &store, ScriptCtxId::Skin, skin_bundle(&["top"]))
            .unwrap_or_else(|e| panic!("{e}"));
        let w = host.live()[0];
        let init = json(&host.take_posted()[0].1);
        assert_eq!(init["wire"], if bytes { json("2") } else { Json::Null });
        host.reply(w, r#"{"t":"ready","ctx":"skin","handlers":[],"ms":1}"#);
        let _ = sup.drain(0.0);
        let mut wr = store.writer(Owner::Source);
        wr.set_value("top.gold", Value::I32(41))
            .unwrap_or_else(|e| panic!("{e}"));
        let delta = store.take_frame();
        for _ in 0..7 {
            sup.post(
                1.0,
                &FrameInput {
                    delta: delta.as_ref(),
                    ..FrameInput::default()
                },
            );
        }
        let posted = host.take_posted();
        assert_eq!(posted.len(), 7);
        assert_eq!(json(&posted[0].1)["t"], "frame");
        (sup, host, w)
    };
    let (text, bytes) = out_fixture();
    let (mut sup, host, w) = run(true);
    host.reply_bytes(w, &bytes);
    let from_bytes = sup.drain(2.0);
    let (mut sup2, host2, w2) = run(false);
    host2.reply(w2, &text);
    let from_json = sup2.drain(2.0);
    assert_eq!(from_bytes.island_cmds.len(), 7);
    assert_eq!(from_bytes.island_cmds, from_json.island_cmds);
    assert_eq!(from_bytes.state_ops, from_json.state_ops);
    assert_eq!(from_bytes.intents, from_json.intents);
    assert_eq!(from_bytes.settings, from_json.settings);
    assert_eq!(from_bytes.log, from_json.log);
    assert_eq!(from_bytes.log.len(), 2, "{:?}", from_bytes.log);
    assert_eq!(sup.stats().worker_ms, sup2.stats().worker_ms);
    // Broken bytes are a message the worker should not send, once.
    host.reply_bytes(w, &bytes[..bytes.len() - 1]);
    host.reply_bytes(w, &[3, 9]);
    let out = sup.drain(3.0);
    assert_eq!(out.log.len(), 1, "{:?}", out.log);
    assert!(out.log[0].1.text.contains("should not"));
}
