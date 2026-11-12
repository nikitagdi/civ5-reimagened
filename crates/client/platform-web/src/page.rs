//! Page helpers: address parameters, WebSocket URL, canvas.

use wasm_bindgen::JsCast;
use web_sys::{HtmlCanvasElement, Url};

/// Value of `?name=` in the current address.
pub fn query_param(name: &str) -> Option<String> {
    let href = web_sys::window()?.location().href().ok()?;
    Url::new(&href).ok()?.search_params().get(name)
}

/// A query parameter that the page remembers: a value in the address is kept in `localStorage[key]` for the next
/// visits, and without one the kept value is used. Storage that throws counts as empty.
pub fn remembered_param(name: &str, key: &str) -> Option<String> {
    let storage = web_sys::window().and_then(|w| w.local_storage().ok().flatten());
    if let Some(v) = query_param(name) {
        if let Some(s) = &storage {
            let _ = s.set_item(key, &v);
        }
        return Some(v);
    }
    storage?.get_item(key).ok().flatten()
}

/// Calendar time of the page clock's zero: `Date.now() - performance.now()`, ms.
pub fn wall_offset_ms() -> f64 {
    let now = web_sys::window()
        .and_then(|w| w.performance())
        .map_or(0.0, |p| p.now());
    js_sys::Date::now() - now
}

/// The address of the page (`location.href`).
pub fn href() -> Option<String> {
    web_sys::window()?.location().href().ok()
}

/// `navigator.userAgent`; empty if the page has none.
pub fn user_agent() -> String {
    web_sys::window()
        .and_then(|w| w.navigator().user_agent().ok())
        .unwrap_or_default()
}

/// `navigator.maxTouchPoints` (0 without a touch screen, or when the page cannot say).
pub fn max_touch_points() -> u32 {
    web_sys::window()
        .map(|w| w.navigator().max_touch_points())
        .map_or(0, |n| n.max(0) as u32)
}

/// WebSocket URL on the page's host: `ws://`, or `wss://` for https.
pub fn ws_url_from_location(path: &str) -> Option<String> {
    let location = web_sys::window()?.location();
    let scheme = if location.protocol().ok()? == "https:" {
        "wss"
    } else {
        "ws"
    };
    let host = location.host().ok()?;
    Some(format!("{scheme}://{host}{path}"))
}

/// Reloads the page (the error screen's Reload).
pub fn reload() {
    if let Some(w) = web_sys::window()
        && let Err(e) = w.location().reload()
    {
        tracing::warn!("page reload: {}", crate::transport::js_error_text(&e));
    }
}

pub fn canvas_by_id(id: &str) -> Option<HtmlCanvasElement> {
    web_sys::window()?
        .document()?
        .get_element_by_id(id)?
        .dyn_into::<HtmlCanvasElement>()
        .ok()
}

/// The size of `canvas` in CSS px (its layout box, not its backing store); `None` while it has none (not in the
/// page, `display: none`).
pub fn css_size(canvas: &HtmlCanvasElement) -> Option<(f32, f32)> {
    let r = canvas.get_bounding_client_rect();
    let (w, h) = (r.width() as f32, r.height() as f32);
    (w >= 1.0 && h >= 1.0).then_some((w, h))
}

/// Canvas drawn *behind* `input_canvas` (WebGPU -> WebGL2 fallback: a `webgpu` canvas cannot yield `webgl2`).
pub fn create_canvas_behind(input_canvas: &HtmlCanvasElement) -> Option<HtmlCanvasElement> {
    let document = web_sys::window()?.document()?;
    let canvas = document
        .create_element("canvas")
        .ok()?
        .dyn_into::<HtmlCanvasElement>()
        .ok()?;
    canvas.set_id("civ5-render-canvas");
    canvas.set_width(input_canvas.width().max(1));
    canvas.set_height(input_canvas.height().max(1));
    let style = canvas.style();
    for (k, v) in [
        ("position", "fixed"),
        ("left", "0"),
        ("top", "0"),
        ("width", "100%"),
        ("height", "100%"),
        ("display", "block"),
        ("pointer-events", "none"),
    ] {
        style.set_property(k, v).ok()?;
    }
    input_canvas.style().set_property("opacity", "0").ok()?;
    let parent = input_canvas.parent_node()?;
    parent.insert_before(&canvas, Some(input_canvas)).ok()?;
    tracing::info!("created fallback canvas for WebGL2");
    Some(canvas)
}
