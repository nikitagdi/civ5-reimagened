//! `window.__civ5stats`: numbers for the playtest scripts and the console (C11, AC-1.3).
//!
//! The object is created once and then updated field by field, so the per-second frame stats
//! (`fps`, `draws`, `triangles`) and the one-off start-up timings (`timing.*`, `timeToMapMs`) live
//! side by side. Main thread only; without a `window` nothing happens.

use wasm_bindgen::JsValue;

const GLOBAL: &str = "__civ5stats";
const TIMING: &str = "timing";

fn object_at(parent: &JsValue, key: &str) -> Option<js_sys::Object> {
    let key = JsValue::from_str(key);
    if let Ok(v) = js_sys::Reflect::get(parent, &key)
        && v.is_object()
    {
        return Some(v.into());
    }
    let obj = js_sys::Object::new();
    js_sys::Reflect::set(parent, &key, &obj).ok()?;
    Some(obj)
}

fn root() -> Option<js_sys::Object> {
    let window = web_sys::window()?;
    object_at(&window, GLOBAL)
}

/// Sets `window.__civ5stats[key] = value`.
pub fn set(key: &str, value: f64) {
    if let Some(stats) = root() {
        let _ = js_sys::Reflect::set(&stats, &key.into(), &JsValue::from_f64(value));
    }
}

/// Sets `window.__civ5stats.passes` to `{ name: ms, ... }`, a new object each time: the time of each
/// pass of the frame (GPU time where the device gives it, else CPU time; render-core 4.13).
pub fn set_passes(passes: &[(&str, f64)]) {
    let Some(stats) = root() else { return };
    let obj = js_sys::Object::new();
    for (name, ms) in passes {
        let _ = js_sys::Reflect::set(&obj, &(*name).into(), &JsValue::from_f64(*ms));
    }
    let _ = js_sys::Reflect::set(&stats, &"passes".into(), &obj);
}

/// Sets `window.__civ5stats.counters` to `{ name: n, ... }` (`fog.updates`, `anim.failed_total`), a
/// new object each time.
pub fn set_counters(counters: &[(&str, f64)]) {
    let Some(stats) = root() else { return };
    let obj = js_sys::Object::new();
    for (name, n) in counters {
        // A value that is not finite (a time the device does not give) is `null`.
        let value = if n.is_finite() {
            JsValue::from_f64(*n)
        } else {
            JsValue::NULL
        };
        let _ = js_sys::Reflect::set(&obj, &(*name).into(), &value);
    }
    let _ = js_sys::Reflect::set(&stats, &"counters".into(), &obj);
}

/// One slow frame for [`set_slow_frames`]: `(seq, ms, turn, [(scope, ms)])`.
pub type SlowRow<'a> = (f64, f64, i32, Vec<(&'a str, f64)>);

/// Sets `window.__civ5stats.slow_frames` to `[{seq, ms, turn, scopes: [[name, ms], ...]}, ...]`,
/// oldest first, a new array each time.
pub fn set_slow_frames(frames: &[SlowRow<'_>]) {
    let Some(stats) = root() else { return };
    let all = js_sys::Array::new();
    for (seq, ms, turn, scopes) in frames {
        let row = js_sys::Object::new();
        let _ = js_sys::Reflect::set(&row, &"seq".into(), &JsValue::from_f64(*seq));
        let _ = js_sys::Reflect::set(&row, &"ms".into(), &JsValue::from_f64(*ms));
        let _ = js_sys::Reflect::set(&row, &"turn".into(), &JsValue::from_f64(f64::from(*turn)));
        let list = js_sys::Array::new();
        for (name, v) in scopes {
            let pair = js_sys::Array::new();
            pair.push(&JsValue::from_str(name));
            pair.push(&JsValue::from_f64(*v));
            list.push(&pair);
        }
        let _ = js_sys::Reflect::set(&row, &"scopes".into(), &list);
        all.push(&row);
    }
    let _ = js_sys::Reflect::set(&stats, &"slow_frames".into(), &all);
}

/// Sets `window.__civ5stats.timing[name] = ms`.
pub fn set_timing(name: &str, ms: f64) {
    if let Some(timing) = root().and_then(|s| object_at(&s, TIMING)) {
        let _ = js_sys::Reflect::set(&timing, &name.into(), &JsValue::from_f64(ms));
    }
}

/// Sets `window.__civ5stats[key]` to `{ name: n, ... }`, a new object each time.
pub fn set_object(key: &str, values: &[(&str, f64)]) {
    let Some(stats) = root() else { return };
    let _ = js_sys::Reflect::set(&stats, &key.into(), &number_object(values));
}

/// Sets `window.__civ5stats[key]` to `{ name: { field: n, ... }, ... }`, a new object each time.
pub fn set_nested(key: &str, rows: &[(&str, Vec<(&str, f64)>)]) {
    let Some(stats) = root() else { return };
    let obj = js_sys::Object::new();
    for (name, values) in rows {
        let _ = js_sys::Reflect::set(&obj, &(*name).into(), &number_object(values));
    }
    let _ = js_sys::Reflect::set(&stats, &key.into(), &obj);
}

thread_local! {
    static BETWEEN: std::cell::RefCell<Vec<(&'static str, f64)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

fn page_now() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map_or(0.0, |p| p.now())
}

/// Times the page's own work outside the frame (a WebSocket handler, a timer): hold it while the
/// handler runs. The time of each label since the last [`take_between`] goes to the gap rows of the
/// slow-frame record (`render::perf::slow`), which say what ran while no frame did.
#[must_use = "the guard times the block it is held in"]
pub struct Between {
    label: &'static str,
    start: f64,
}

pub fn between(label: &'static str) -> Between {
    Between {
        label,
        start: page_now(),
    }
}

impl Drop for Between {
    fn drop(&mut self) {
        let ms = page_now() - self.start;
        BETWEEN.with(|b| {
            let mut b = b.borrow_mut();
            match b.iter_mut().find(|(l, _)| *l == self.label) {
                Some(e) => e.1 += ms,
                None => b.push((self.label, ms)),
            }
        });
    }
}

/// The times of [`between`] since the last call.
pub fn take_between() -> Vec<(&'static str, f64)> {
    BETWEEN.with(|b| std::mem::take(&mut *b.borrow_mut()))
}

/// Sets `window.__civ5stats[key]` to a string.
pub fn set_str(key: &str, value: &str) {
    if let Some(stats) = root() {
        let _ = js_sys::Reflect::set(&stats, &key.into(), &JsValue::from_str(value));
    }
}

fn number_object(values: &[(&str, f64)]) -> js_sys::Object {
    let obj = js_sys::Object::new();
    for (name, n) in values {
        let _ = js_sys::Reflect::set(&obj, &(*name).into(), &JsValue::from_f64(*n));
    }
    obj
}

/// The JS heap of the tab as Chrome reports it (`performance.memory`): used, total and limit in
/// bytes. `None` in browsers without it.
pub fn js_heap() -> Option<(f64, f64, f64)> {
    let perf = js_sys::Reflect::get(&web_sys::window()?.into(), &"performance".into()).ok()?;
    let mem = js_sys::Reflect::get(&perf, &"memory".into()).ok()?;
    if !mem.is_object() {
        return None;
    }
    let n = |k: &str| js_sys::Reflect::get(&mem, &k.into()).ok()?.as_f64();
    Some((
        n("usedJSHeapSize")?,
        n("totalJSHeapSize")?,
        n("jsHeapSizeLimit")?,
    ))
}

/// Sets `window.__civ5stats.turn` to `{ level, open_turn, turns: { "<turn>": { field: n, ... }, ... } }` from the
/// flat values of the `turn` group (`level`, `open_turn`, and `"<turn>.<field>"`), a new object each time. The keys
/// of `turns` are integers, so a reader walks them oldest first.
pub fn set_turn_group(values: &[(&str, f64)]) {
    let Some(stats) = root() else { return };
    let out = js_sys::Object::new();
    let turns = js_sys::Object::new();
    for (name, n) in values {
        let value = if n.is_finite() {
            JsValue::from_f64(*n)
        } else {
            JsValue::NULL
        };
        match name.split_once('.') {
            Some((turn, field)) => {
                let key = JsValue::from_str(turn);
                let row = match js_sys::Reflect::get(&turns, &key) {
                    Ok(r) if r.is_object() => r,
                    _ => {
                        let r: JsValue = js_sys::Object::new().into();
                        let _ = js_sys::Reflect::set(&turns, &key, &r);
                        r
                    }
                };
                let _ = js_sys::Reflect::set(&row, &field.into(), &value);
            }
            None => {
                let _ = js_sys::Reflect::set(&out, &(*name).into(), &value);
            }
        }
    }
    let _ = js_sys::Reflect::set(&out, &"turns".into(), &turns);
    let _ = js_sys::Reflect::set(&stats, &"turn".into(), &out);
}

/// Sets `window.__civ5stats.<group>` to `{ name: n, ... }`, a new object each time (ui, replica, exe).
pub fn set_group(group: &str, values: &[(&str, f64)]) {
    let Some(stats) = root() else { return };
    let obj = js_sys::Object::new();
    for (name, n) in values {
        let value = if n.is_finite() {
            JsValue::from_f64(*n)
        } else {
            JsValue::NULL
        };
        let _ = js_sys::Reflect::set(&obj, &(*name).into(), &value);
    }
    let _ = js_sys::Reflect::set(&stats, &group.into(), &obj);
}
