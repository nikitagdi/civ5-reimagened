//! `window.__civ5ui2` behind `platform::ScriptHost`: the script workers of UI v2 skins and mods.
//!
//! The page (`ui2/host.js`) creates the workers and queues what they post until the frame takes it with
//! `drain()`, so nothing here keeps a callback alive and no JS runs inside a borrow of the app. The contract:
//!
//! - `spawn() -> number`: a new worker running `ui2/worker.js`; throws when the page cannot create one;
//! - `post(id, text) -> bool`: `false` for a worker that does not exist (any more);
//! - `wire` (optional): 2 when the workers' messages posted as bytes reach `drain`;
//! - `drain(ids) -> [[id, "msg" | "bin" | "error", data], ...]`: what the workers `ids` sent since the last call, oldest first;
//!   the other workers' messages stay queued. The page has one queue for every host (the skin's supervisor and the
//!   mods' each have one), so a host drains only the workers it started;
//! - `terminate(id)`: stops the worker at once and drops what it sent and was not drained;
//! - `live() -> number | null` (optional): the time (`performance.now()`, the app's clock) up to which the page has
//!   taken in what the workers posted (see `ScriptHost::delivered_until`).

use std::cell::Cell;

use js_sys::{Array, Function, Reflect, Uint8Array};
use platform::{ScriptHost, ScriptHostError, ScriptHostEvent, WorkerId};
use wasm_bindgen::{JsCast, JsValue};

use crate::transport::js_error_text;

/// Script workers over the page's `window.__civ5ui2`.
pub struct WebScriptHost {
    api: JsValue,
    spawn: Function,
    post: Function,
    drain: Function,
    terminate: Function,
    /// `live()`, when the page has it.
    live: Option<Function>,
    /// The page's `wire` is 2: the workers' byte messages reach `drain`.
    bytes: bool,
    /// The workers this host started and did not terminate: the ones whose messages it drains.
    own: Vec<u32>,
    /// One log line for the first failed `drain`, not one per frame.
    warned: Cell<bool>,
}

impl WebScriptHost {
    /// The host, when the page defines `window.__civ5ui2` with its four functions (`ui2/host.js` is loaded);
    /// `None` otherwise (skins then run on their markup alone).
    #[must_use]
    pub fn detect() -> Option<WebScriptHost> {
        let api = Reflect::get(&js_sys::global(), &"__civ5ui2".into()).ok()?;
        if !api.is_object() {
            return None;
        }
        let func = |name: &str| {
            Reflect::get(&api, &name.into())
                .ok()
                .and_then(|f| f.dyn_into::<Function>().ok())
        };
        Some(WebScriptHost {
            spawn: func("spawn")?,
            post: func("post")?,
            drain: func("drain")?,
            terminate: func("terminate")?,
            live: func("live"),
            bytes: Reflect::get(&api, &"wire".into())
                .ok()
                .and_then(|v| v.as_f64())
                .is_some_and(|w| w >= 2.0),
            api,
            own: Vec::new(),
            warned: Cell::new(false),
        })
    }
}

/// One entry of `drain()`: `[id, kind, data]`.
fn entry(v: &JsValue) -> Option<(WorkerId, ScriptHostEvent)> {
    let a = v.dyn_ref::<Array>()?;
    let id = a
        .get(0)
        .as_f64()
        .filter(|n| *n >= 0.0 && *n <= f64::from(u32::MAX))? as u32;
    let data = a.get(2);
    let text = || data.as_string().unwrap_or_default();
    let ev = match a.get(1).as_string().as_deref() {
        Some("msg") => ScriptHostEvent::Message(text()),
        Some("bin") => ScriptHostEvent::Bytes(data.dyn_ref::<Uint8Array>()?.to_vec()),
        Some("error") => ScriptHostEvent::Error(text()),
        _ => return None,
    };
    Some((WorkerId(id), ev))
}

impl ScriptHost for WebScriptHost {
    fn spawn(&mut self) -> Result<WorkerId, ScriptHostError> {
        let id = self
            .spawn
            .call0(&self.api)
            .map_err(|e| ScriptHostError::Spawn(js_error_text(&e)))?;
        let id = id
            .as_f64()
            .filter(|n| *n >= 0.0 && *n <= f64::from(u32::MAX))
            .map(|n| WorkerId(n as u32))
            .ok_or_else(|| ScriptHostError::Spawn(format!("the page gave no worker id: {id:?}")))?;
        self.own.push(id.0);
        Ok(id)
    }

    fn post(&mut self, worker: WorkerId, text: &str) -> Result<(), ScriptHostError> {
        match self.post.call2(
            &self.api,
            &JsValue::from(worker.0),
            &JsValue::from_str(text),
        ) {
            Ok(v) if v.as_bool() == Some(true) => Ok(()),
            // A worker that is gone, or a page that could not post (a text it cannot clone): either way the
            // worker will not see the message.
            Ok(_) | Err(_) => Err(ScriptHostError::NoWorker(worker)),
        }
    }

    fn carries_bytes(&self) -> bool {
        self.bytes
    }

    fn drain(&mut self, out: &mut Vec<(WorkerId, ScriptHostEvent)>) {
        if self.own.is_empty() {
            return;
        }
        let ids: Array = self.own.iter().map(|id| JsValue::from(*id)).collect();
        match self.drain.call1(&self.api, &ids) {
            Ok(list) => {
                if let Some(list) = list.dyn_ref::<Array>() {
                    out.extend(list.iter().filter_map(|v| entry(&v)));
                }
            }
            Err(e) => {
                if !self.warned.replace(true) {
                    tracing::warn!("ui2 script workers: drain failed: {}", js_error_text(&e));
                }
            }
        }
    }

    fn delivered_until(&self) -> Option<f64> {
        let live = self.live.as_ref()?;
        live.call0(&self.api)
            .ok()
            .and_then(|v| v.as_f64())
            .filter(|t| t.is_finite())
    }

    fn terminate(&mut self, worker: WorkerId) {
        self.own.retain(|id| *id != worker.0);
        if let Err(e) = self.terminate.call1(&self.api, &JsValue::from(worker.0)) {
            tracing::warn!(
                "ui2 script workers: terminate {} failed: {}",
                worker.0,
                js_error_text(&e)
            );
        }
    }
}
