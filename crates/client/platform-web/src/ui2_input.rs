//! `platform::ui2::TextInputBridge` for the browser: a hidden `<textarea>` that takes the keyboard while a UI v2
//! text field has the focus.
//!
//! The browser does the editing (typing, paste, undo, the selection, input methods); the UI draws the field
//! from the state [`WebTextInput::poll`] reads. The textarea is invisible and lets the pointer through, and
//! lies over the field (so an input method opens its window next to it). Its key events do not reach the
//! page: Enter (outside a composition), Escape and Tab are reported in the state instead. Offsets are
//! converted between the browser's UTF-16 and the UI's UTF-8.

use std::cell::RefCell;
use std::rc::Rc;

use platform::ui2::{
    TextInputBridge, TextInputRequest, TextInputState, TextSel, utf8_to_utf16, utf16_to_utf8,
};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{CompositionEvent, Event, HtmlElement, HtmlTextAreaElement, KeyboardEvent};

/// What the event handlers saw since the last poll.
#[derive(Default)]
struct Seen {
    dirty: bool,
    submitted: bool,
    cancelled: bool,
    tab: Option<bool>,
    /// Where the running composition starts (UTF-16) and its text.
    composing: Option<(u32, String)>,
}

type Handler = Closure<dyn FnMut(Event)>;

/// The hidden textarea.
pub struct WebTextInput {
    area: HtmlTextAreaElement,
    seen: Rc<RefCell<Seen>>,
    handlers: Vec<(&'static str, Handler)>,
    /// The element that had the focus before the field took it (the canvas): it gets it back on blur.
    restore: Option<HtmlElement>,
    focused: bool,
}

const STYLE: &str = "position:fixed;left:0;top:0;width:1px;height:1px;margin:0;padding:0;border:0;\
outline:none;resize:none;overflow:hidden;white-space:pre;opacity:0;color:transparent;\
background:transparent;caret-color:transparent;pointer-events:none;font-size:16px;z-index:0";

impl WebTextInput {
    /// Adds the textarea to the page; `None` without a document body.
    #[must_use]
    pub fn new() -> Option<WebTextInput> {
        let document = web_sys::window()?.document()?;
        let body = document.body()?;
        let area: HtmlTextAreaElement =
            document.create_element("textarea").ok()?.dyn_into().ok()?;
        for (k, v) in [
            ("style", STYLE),
            ("autocomplete", "off"),
            ("autocorrect", "off"),
            ("autocapitalize", "off"),
            ("spellcheck", "false"),
            ("tabindex", "-1"),
            ("aria-label", "text field"),
            ("data-ui2-text-input", ""),
        ] {
            let _ = area.set_attribute(k, v);
        }
        body.append_child(&area).ok()?;
        let seen = Rc::new(RefCell::new(Seen::default()));
        let mut handlers: Vec<(&'static str, Handler)> = Vec::new();
        let mut on = |name: &'static str, f: Box<dyn FnMut(Event)>| {
            let c = Closure::wrap(f);
            let _ = area.add_event_listener_with_callback(name, c.as_ref().unchecked_ref());
            handlers.push((name, c));
        };
        for name in ["input", "keyup", "select", "focus"] {
            let s = Rc::clone(&seen);
            on(
                name,
                Box::new(move |e: Event| {
                    if name == "keyup" {
                        e.stop_propagation();
                    }
                    s.borrow_mut().dirty = true;
                }),
            );
        }
        let s = Rc::clone(&seen);
        on(
            "keydown",
            Box::new(move |e: Event| {
                e.stop_propagation();
                let Some(k) = e.dyn_ref::<KeyboardEvent>() else {
                    return;
                };
                let mut s = s.borrow_mut();
                s.dirty = true;
                if k.is_composing() || s.composing.is_some() {
                    return;
                }
                match k.key().as_str() {
                    "Enter" => {
                        e.prevent_default();
                        s.submitted = true;
                    }
                    "Escape" => {
                        e.prevent_default();
                        s.cancelled = true;
                    }
                    "Tab" => {
                        e.prevent_default();
                        s.tab = Some(k.shift_key());
                    }
                    _ => {}
                }
            }),
        );
        let (s, a) = (Rc::clone(&seen), area.clone());
        on(
            "compositionstart",
            Box::new(move |_e: Event| {
                let start = a.selection_start().ok().flatten().unwrap_or(0);
                let mut s = s.borrow_mut();
                s.composing = Some((start, String::new()));
                s.dirty = true;
            }),
        );
        let s = Rc::clone(&seen);
        on(
            "compositionupdate",
            Box::new(move |e: Event| {
                let data = e
                    .dyn_ref::<CompositionEvent>()
                    .and_then(CompositionEvent::data)
                    .unwrap_or_default();
                let mut s = s.borrow_mut();
                if let Some((_, text)) = s.composing.as_mut() {
                    *text = data;
                }
                s.dirty = true;
            }),
        );
        let s = Rc::clone(&seen);
        on(
            "compositionend",
            Box::new(move |_e: Event| {
                let mut s = s.borrow_mut();
                s.composing = None;
                s.dirty = true;
            }),
        );
        Some(WebTextInput {
            area,
            seen,
            handlers,
            restore: None,
            focused: false,
        })
    }

    fn place(&self, rect: [f32; 4]) {
        let dpr = web_sys::window().map_or(1.0, |w| w.device_pixel_ratio()) as f32;
        let dpr = if dpr > 0.0 { dpr } else { 1.0 };
        let style = self.area.style();
        let px = |v: f32| format!("{}px", v / dpr);
        let _ = style.set_property("left", &px(rect[0]));
        let _ = style.set_property("top", &px(rect[1]));
        let _ = style.set_property("width", &px(rect[2].max(dpr)));
        let _ = style.set_property("height", &px(rect[3].max(dpr)));
    }
}

impl TextInputBridge for WebTextInput {
    fn request(&mut self, req: &TextInputRequest) {
        match req {
            TextInputRequest::Focus { value, sel, rect } => {
                if self.area.value() != *value {
                    self.area.set_value(value);
                }
                let (a, b) = sel.range();
                let (a, b) = (utf8_to_utf16(value, a), utf8_to_utf16(value, b));
                let dir = if sel.caret < sel.anchor {
                    "backward"
                } else {
                    "forward"
                };
                let _ = self.area.set_selection_range_with_direction(a, b, dir);
                self.place(*rect);
                if !self.focused {
                    self.restore = web_sys::window()
                        .and_then(|w| w.document())
                        .and_then(|d| d.active_element())
                        .and_then(|e| e.dyn_into::<HtmlElement>().ok())
                        .filter(|e| e != self.area.unchecked_ref::<HtmlElement>());
                    let opts = web_sys::FocusOptions::new();
                    opts.set_prevent_scroll(true);
                    let _ = self.area.focus_with_options(&opts);
                    self.focused = true;
                }
                // The UI knows this state already.
                let mut s = self.seen.borrow_mut();
                s.dirty = false;
                s.composing = None;
            }
            TextInputRequest::Blur => {
                if self.focused {
                    self.focused = false;
                    let _ = self.area.blur();
                    if let Some(r) = self.restore.take() {
                        let _ = r.focus();
                    }
                }
                *self.seen.borrow_mut() = Seen::default();
            }
        }
    }

    fn poll(&mut self) -> Option<TextInputState> {
        let mut s = self.seen.borrow_mut();
        if !self.focused || !s.dirty {
            return None;
        }
        s.dirty = false;
        let value = self.area.value();
        let start = self.area.selection_start().ok().flatten().unwrap_or(0);
        let end = self.area.selection_end().ok().flatten().unwrap_or(start);
        let backward =
            self.area.selection_direction().ok().flatten().as_deref() == Some("backward");
        let (a, b) = (utf16_to_utf8(&value, start), utf16_to_utf8(&value, end));
        let sel = if backward {
            TextSel {
                anchor: b,
                caret: a,
            }
        } else {
            TextSel {
                anchor: a,
                caret: b,
            }
        };
        let composing = s.composing.as_ref().map(|(from, text)| {
            let units = text.encode_utf16().count() as u32;
            (
                utf16_to_utf8(&value, *from),
                utf16_to_utf8(&value, from + units),
            )
        });
        Some(TextInputState {
            value,
            sel,
            composing,
            submitted: std::mem::take(&mut s.submitted),
            cancelled: std::mem::take(&mut s.cancelled),
            tab: s.tab.take(),
        })
    }
}

impl Drop for WebTextInput {
    fn drop(&mut self) {
        for (name, h) in &self.handlers {
            let _ = self
                .area
                .remove_event_listener_with_callback(name, h.as_ref().unchecked_ref());
        }
        self.area.remove();
    }
}
