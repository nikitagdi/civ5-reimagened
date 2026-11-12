//! The skin's own state (`state.*`) in a running UI v2: mounted from the package with its defaults before the
//! first frame, written by the skin's scripts through the actions module, read by the bindings; a path the skin
//! did not declare is refused.

mod support;

use ui2_host::{Env, V2Frame, V2FrameReport, V2Init, V2Ui};
use ui2_model::{Namespace, Value};
use ui2_script::testing::MockScriptHost;

const READY: &str = r#"{"t":"ready","ctx":"skin","handlers":[],"ms":1}"#;

fn ui_of(pkg: std::sync::Arc<ui2_package::Package>, host: Option<&MockScriptHost>) -> V2Ui {
    V2Ui::new(
        pkg,
        V2Init {
            env: Env {
                viewport_px: (1280, 720),
                ..Env::default()
            },
            scripts: host.map(|h| Box::new(h.clone()) as Box<dyn platform::ScriptHost>),
            ..V2Init::default()
        },
    )
    .expect("the skin loads")
}

fn frame(ui: &mut V2Ui, now: &mut f64) -> V2FrameReport {
    *now += 16.0;
    let r = ui.frame(V2Frame {
        replica: None,
        steps: &[],
        net: None,
        events: &[],
        mods: None,
        now_ms: *now,
    });
    assert!(r.errors.is_empty(), "{:?}", r.errors);
    r
}

fn sort_text(ui: &V2Ui) -> String {
    ui.texts()
        .into_iter()
        .find(|t| t.starts_with("by "))
        .expect("the bound text")
}

fn read(ui: &V2Ui, path: &str) -> Option<Value> {
    let f = ui.store().resolve(path).ok()?;
    ui.store().value(f.id, &[]).cloned()
}

#[test]
fn the_state_is_mounted_with_the_package_defaults() {
    let mut ui = ui_of(support::state_skin("kit", "name", None), None);
    assert!(ui.store().is_mounted(&Namespace::State));
    let mut now = 0.0;
    frame(&mut ui, &mut now);
    assert_eq!(
        read(&ui, "state.prod.sort"),
        Some(Value::Str("name".into()))
    );
    assert_eq!(read(&ui, "state.open"), Some(Value::Bool(false)));
    assert_eq!(sort_text(&ui), "by name");
}

#[test]
fn a_script_write_to_a_declared_key_changes_the_bound_node_and_an_undeclared_one_is_refused() {
    let host = MockScriptHost::new();
    let pkg = support::state_skin("kit", "name", Some("ui.state.on('prod.sort', () => {});"));
    let mut ui = ui_of(pkg, Some(&host));
    let mut now = 0.0;
    frame(&mut ui, &mut now);
    let w = host.live()[0];
    // The worker starts on a model that has the skin state: it watches `state` (no "unknown model root").
    let init = host
        .take_posted()
        .into_iter()
        .map(|(_, t)| serde_json::from_str::<serde_json::Value>(&t).expect("json"))
        .find(|m| m["t"] == "init")
        .expect("the init");
    let watched = init.to_string();
    assert!(watched.contains("\"state\""), "{watched}");
    host.reply(w, READY);
    frame(&mut ui, &mut now);
    assert_eq!(sort_text(&ui), "by name");

    host.reply(
        w,
        r#"{"t":"out","n":1,"ack":null,"state":[{"p":"state.prod.sort","v":"turns"},{"p":"state.prod.nope","v":"x"},{"p":"state.open","v":"yes"}]}"#,
    );
    let r = frame(&mut ui, &mut now);
    assert_eq!(r.refused, 2, "the undeclared path and the wrong type");
    assert_eq!(
        read(&ui, "state.prod.sort"),
        Some(Value::Str("turns".into()))
    );
    assert_eq!(read(&ui, "state.open"), Some(Value::Bool(false)));
    assert_eq!(
        sort_text(&ui),
        "by turns",
        "the bound node follows the write"
    );
}
