//! `window.__civ5ui2` behind `platform::ScriptHost`: the script workers of UI v2 skins and mods.
//!
//! The page (`ui2/host.js`) creates the workers and queues what they post until the frame takes it with
//! `drain()`, so nothing here keeps a callback alive and no JS runs inside a borrow of the app. The contract:
//!
//! - `spawn() -> number`: a new worker running `ui2/worker.js`; throws when the page cannot create one;
//! - `post(id, text) -> bool`: `false` for a worker that does not exist (any more);
//! - `drain() -> [[id, "msg" | "error", text], ...]`: what the workers sent since the last call, oldest first;
//! - `terminate(id)`: stops the worker at once and drops what it sent and was not drained.

use std::cell::Cell;

use js_sys::{Array, Function, Reflect};
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
            api,
            warned: Cell::new(false),
        })
    }
}

/// One entry of `drain()`: `[id, kind, text]`.
fn entry(v: &JsValue) -> Option<(WorkerId, ScriptHostEvent)> {
    let a = v.dyn_ref::<Array>()?;
    let id = a
        .get(0)
        .as_f64()
        .filter(|n| *n >= 0.0 && *n <= f64::from(u32::MAX))? as u32;
    let text = a.get(2).as_string().unwrap_or_default();
    let ev = match a.get(1).as_string().as_deref() {
        Some("msg") => ScriptHostEvent::Message(text),
        Some("error") => ScriptHostEvent::Error(text),
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
        id.as_f64()
            .filter(|n| *n >= 0.0 && *n <= f64::from(u32::MAX))
            .map(|n| WorkerId(n as u32))
            .ok_or_else(|| ScriptHostError::Spawn(format!("the page gave no worker id: {id:?}")))
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

    fn drain(&mut self, out: &mut Vec<(WorkerId, ScriptHostEvent)>) {
        match self.drain.call0(&self.api) {
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

    fn terminate(&mut self, worker: WorkerId) {
        if let Err(e) = self.terminate.call1(&self.api, &JsValue::from(worker.0)) {
            tracing::warn!(
                "ui2 script workers: terminate {} failed: {}",
                worker.0,
                js_error_text(&e)
            );
        }
    }
}
