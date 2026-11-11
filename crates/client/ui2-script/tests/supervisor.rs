//! The supervisor against a mock worker host: what is sent per frame, and how answers become output.

use std::sync::Arc;

use platform::{NullScriptHost, ScriptHostError, WorkerId};
use serde_json::Value as Json;
use ui2_model::generated::{Game, Top};
use ui2_model::value::Key;
use ui2_model::wire::{IslandOp, Origin, ScriptCtxId, ScriptEvent};
use ui2_model::{Fragment, ModelStore, Namespace, Owner, Schema, Value};
use ui2_script::testing::MockScriptHost;
use ui2_script::{
    FrameInput, LogLevel, MAX_CONTEXTS, ScriptBundle, ScriptEnv, ScriptError, ScriptInit,
    ScriptStatus, ScriptSupervisor, SettingsNs, StateNs, check_api,
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
fn a_mod_that_fails_is_switched_off_without_fatal() {
    let host = MockScriptHost::new();
    let mut sup = ScriptSupervisor::new(Box::new(host.clone()));
    let store = store();
    let ctx = ScriptCtxId::Mod(Arc::from("hello"));
    start(&mut sup, &store, ctx.clone(), mod_bundle("hello")).unwrap_or_else(|e| panic!("{e}"));
    let w = host.live()[0];
    host.error(w, "worker.js: 404");
    let out = sup.drain(0.0);
    assert_eq!(out.failed, vec![ctx]);
    assert!(sup.take_fatal().is_none());
    assert_eq!(sup.stats().errors, 1);
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
fn without_workers_start_is_refused() {
    let mut sup = ScriptSupervisor::new(Box::new(NullScriptHost));
    let r = start(&mut sup, &store(), ScriptCtxId::Skin, skin_bundle(&["top"]));
    assert_eq!(r, Err(ScriptError::Host(ScriptHostError::Unsupported)));
    assert_eq!(sup.stats().contexts, 0);
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
