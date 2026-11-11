//! `Ui2ScriptSelftest`: the script supervisor of UI v2 over the page's real workers, without a game or a skin
//! package. The page `ui2-selftest.html` drives it (playtest scenario `ui2-script`): it starts a context with
//! script files of its own, writes to a small model, and reads back what the workers answered. The frame order is
//! the host's (drain, apply the scripts' state, the model's delta, post), so a state write of a script reaches the
//! model one frame after its answer and the worker's mirror the frame after that.

use std::sync::Arc;

use serde_json::{Value as Json, json};
use ui2_model::generated::{Game, Top};
use ui2_model::wire::{ScriptCtxId, ScriptEvent};
use ui2_model::{Fragment, ModelStore, Namespace, Owner, Schema};
use ui2_script::proto::{json_to_value, value_to_json};
use ui2_script::{
    FrameInput, LogLevel, ScriptBundle, ScriptEnv, ScriptInit, ScriptOutput, ScriptStatus,
    ScriptSupervisor, SettingsNs, StateNs, ctx_name,
};
use wasm_bindgen::prelude::*;

/// The skin state of the test: what scripts write with `ui.state.set`.
const STATE: &str = "[root]\nsort = \"str\"\nopen = \"bool\"\nn = \"i32\"\n";

fn env() -> ScriptEnv {
    ScriptEnv {
        lang: "en_US".into(),
        viewport: (1280, 720),
        scale: 1.0,
        cell: 0,
        skin: "selftest".into(),
    }
}

fn err(e: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&e.to_string())
}

/// A supervisor over `window.__civ5ui2` and a model of its own.
#[wasm_bindgen]
pub struct Ui2ScriptSelftest {
    sup: ScriptSupervisor,
    store: ModelStore,
    events: Vec<ScriptEvent>,
}

#[wasm_bindgen]
impl Ui2ScriptSelftest {
    /// Fails when the page has no `window.__civ5ui2` (`ui2/host.js` not loaded).
    #[wasm_bindgen(constructor)]
    pub fn new() -> Result<Ui2ScriptSelftest, JsValue> {
        let host = platform_web::WebScriptHost::detect()
            .ok_or_else(|| err("the page has no window.__civ5ui2 (ui2/host.js)"))?;
        let mut store = ModelStore::new(Schema::v1());
        let state = Fragment::parse(STATE).map_err(err)?;
        store.mount(Namespace::State, &state).map_err(err)?;
        let mut w = store.writer(Owner::Source);
        w.put(Top::default()).map_err(err)?;
        w.put(Game::default()).map_err(err)?;
        let _ = store.take_frame();
        Ok(Ui2ScriptSelftest {
            sup: ScriptSupervisor::new(Box::new(host)),
            store,
            events: Vec::new(),
        })
    }

    /// Start the skin context with `files` (`[{path, code}]`) reading the model prefixes `reads` (`["top"]`).
    pub fn start(&mut self, files: &str, reads: &str) -> Result<(), JsValue> {
        let files: Vec<Json> = serde_json::from_str(files).map_err(err)?;
        let reads: Vec<String> = serde_json::from_str(reads).map_err(err)?;
        let bundle = ScriptBundle {
            files: files
                .iter()
                .map(|f| {
                    (
                        f["path"].as_str().unwrap_or("js/main.js").to_string(),
                        Arc::from(f["code"].as_str().unwrap_or_default()),
                    )
                })
                .collect(),
            reads,
            state_ns: StateNs::Skin,
            settings_ns: SettingsNs::Skin(Arc::from("selftest")),
            intent_prefix: None,
        };
        let init = ScriptInit::from_store(&self.store, &bundle, Vec::new(), env()).map_err(err)?;
        self.sup.start(ScriptCtxId::Skin, bundle, init).map_err(err)
    }

    /// Queue a markup event for the skin (`arg` a JSON text) for the next frame.
    pub fn event(&mut self, name: &str, arg: &str) {
        let arg = serde_json::from_str::<Json>(arg).unwrap_or(Json::Null);
        self.events.push(ScriptEvent {
            ctx: ScriptCtxId::Skin,
            name: Arc::from(name),
            arg: json_to_value(&arg, None),
            node_path: Arc::from("selftest"),
        });
    }

    /// One frame: take the workers' answers, apply their state writes, write `writes` (`[[path, value]]` of the
    /// model's game roots), send the delta and the queued events. Returns what was taken, as JSON.
    pub fn frame(&mut self, now_ms: f64, writes: &str) -> Result<String, JsValue> {
        let out = self.sup.drain(now_ms);
        {
            let mut w = self.store.writer(Owner::Skin);
            for (_, op) in &out.state_ops {
                // The actions module checks these in the game; here a bad write is only reported back.
                let _ = w.set_value(&op.path, op.value.clone());
            }
        }
        let writes: Vec<(String, Json)> = serde_json::from_str(writes).map_err(err)?;
        {
            let schema = Schema::v1();
            let mut w = self.store.writer(Owner::Source);
            for (path, v) in &writes {
                let ty = schema.resolve(path).map_err(err)?.ty;
                w.set_value(path, json_to_value(v, Some((schema, &ty))))
                    .map_err(err)?;
            }
        }
        let delta = self.store.take_frame();
        let events = std::mem::take(&mut self.events);
        self.sup.post(
            now_ms,
            &FrameInput {
                delta: delta.as_ref(),
                events: &events,
                env: None,
                // A killed worker starts again from the model as it is now.
                store: Some(&self.store),
            },
        );
        Ok(report(&self.sup, &out).to_string())
    }

    /// A value of the model (`state.sort`, `top.gold`) as JSON, `null` when it is not there.
    pub fn get(&self, path: &str) -> String {
        self.store
            .resolve(path)
            .ok()
            .and_then(|f| self.store.value(f.id, &[]))
            .map_or(Json::Null, value_to_json)
            .to_string()
    }

    /// The status of the skin context: `starting`, `running`, `busy`, `restarting`, `disabled` or `none`.
    pub fn status(&self) -> String {
        status_name(self.sup.status(&ScriptCtxId::Skin).as_ref()).to_string()
    }

    /// Stop the skin context.
    pub fn stop(&mut self) {
        self.sup.stop(&ScriptCtxId::Skin);
    }
}

fn status_name(s: Option<&ScriptStatus>) -> &'static str {
    match s {
        None => "none",
        Some(ScriptStatus::Starting) => "starting",
        Some(ScriptStatus::Running) => "running",
        Some(ScriptStatus::Busy { .. }) => "busy",
        Some(ScriptStatus::Restarting) => "restarting",
        Some(ScriptStatus::Disabled { .. }) => "disabled",
    }
}

fn report(sup: &ScriptSupervisor, out: &ScriptOutput) -> Json {
    let level = |l: LogLevel| match l {
        LogLevel::Info => "info",
        LogLevel::Warn => "warn",
        LogLevel::Error => "error",
    };
    let stats = sup.stats();
    json!({
        "state": out.state_ops.iter().map(|(_, o)| json!([&*o.path, value_to_json(&o.value)])).collect::<Vec<_>>(),
        "intents": out.intents.iter().map(|(_, i)| Json::from(&*i.name)).collect::<Vec<_>>(),
        "cmds": out.island_cmds.len(),
        "settings": out.settings.iter().map(|(_, k, v)| json!([k, v])).collect::<Vec<_>>(),
        "log": out.log.iter().map(|(_, l)| json!([level(l.level), l.text])).collect::<Vec<_>>(),
        "failed": out.failed.iter().map(ctx_name).collect::<Vec<_>>(),
        "status": status_name(sup.status(&ScriptCtxId::Skin).as_ref()),
        "handlers": sup.handlers(&ScriptCtxId::Skin),
        "stats": {
            "posted": stats.posted,
            "received": stats.received,
            "errors": stats.errors,
            "restarts": stats.restarts,
            "worker_ms": stats.worker_ms,
        },
    })
}
