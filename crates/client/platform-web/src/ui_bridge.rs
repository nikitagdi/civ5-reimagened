//! `window.__civ5ui` behind `UiBridge`: the page (`ui.js`, `auth.js`) draws the login, lobby and room screens.
//!
//! The page defines `render(screen, json)` and `drain()` before the module loads; actions it posts wait in
//! a queue on its side until `drain()` takes them (once per frame), so nothing here keeps a callback alive.

use std::cell::Cell;

use js_sys::{Array, Function, Reflect};
use platform::UiBridge;
use wasm_bindgen::{JsCast, JsValue};

use crate::transport::js_error_text;

pub struct WebUiBridge {
    api: JsValue,
    render: Function,
    drain: Function,
    warned: Cell<bool>,
}

impl WebUiBridge {
    /// The bridge, when the page defines `window.__civ5ui.render` and `.drain`; `None` for a page without
    /// them (the app then draws the screens itself).
    pub fn detect() -> Option<Self> {
        let api = Reflect::get(&js_sys::global(), &"__civ5ui".into()).ok()?;
        if !api.is_object() {
            return None;
        }
        let func = |name: &str| {
            Reflect::get(&api, &name.into())
                .ok()
                .and_then(|f| f.dyn_into::<Function>().ok())
        };
        let (render, drain) = (func("render")?, func("drain")?);
        Some(Self {
            api,
            render,
            drain,
            warned: Cell::new(false),
        })
    }

    /// One line in the log for the first failure, not one per frame.
    fn failed(&self, what: &str, e: &JsValue) {
        if !self.warned.replace(true) {
            tracing::warn!("the page's screens: {what} failed: {}", js_error_text(e));
        }
    }
}

impl UiBridge for WebUiBridge {
    fn render(&self, screen: &str, json: &str) {
        if let Err(e) = self.render.call2(
            &self.api,
            &JsValue::from_str(screen),
            &JsValue::from_str(json),
        ) {
            self.failed("render", &e);
        }
    }

    fn take_actions(&self) -> Vec<String> {
        match self.drain.call0(&self.api) {
            Ok(list) => Array::from(&list)
                .iter()
                .filter_map(|v| v.as_string())
                .collect(),
            Err(e) => {
                self.failed("drain", &e);
                Vec::new()
            }
        }
    }
}
