//! Page events for the network layer (net-client 4.7, 4.8, NC-D9): `visibilitychange`, `online`, `offline`,
//! `pagehide` (and `pageshow` coming back from the back/forward cache), and the safety timer that keeps the
//! network turning when the tab draws no frames.

use std::cell::RefCell;
use std::rc::Rc;

use platform::HostEvent;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{Document, Event, EventTarget, PageTransitionEvent, VisibilityState, Window};

use crate::transport::js_error_text;

type Handler = Rc<RefCell<dyn FnMut(HostEvent)>>;
type Listener = (EventTarget, &'static str, Closure<dyn FnMut(Event)>);

/// The listeners on `window` and `document`; they are removed when this is dropped.
pub struct PageEvents {
    listeners: Vec<Listener>,
}

impl PageEvents {
    /// Starts reporting page events to `handler`. The handler runs from the browser's event loop, never
    /// inside a frame, so it may borrow the network client for a moment.
    ///
    /// # Errors
    /// No `window` or `document` (not a page), or the browser refuses a listener.
    pub fn install(handler: impl FnMut(HostEvent) + 'static) -> Result<Self, String> {
        let window = web_sys::window().ok_or("no window")?;
        let document = window.document().ok_or("no document")?;
        let handler: Handler = Rc::new(RefCell::new(handler));
        let mut me = PageEvents {
            listeners: Vec::new(),
        };

        let doc = document.clone();
        me.listen(&document, "visibilitychange", &handler, move |_| {
            Some(if is_hidden(&doc) {
                HostEvent::Hidden
            } else {
                HostEvent::Visible
            })
        })?;
        me.listen(&window, "online", &handler, |_| Some(HostEvent::Online))?;
        me.listen(&window, "offline", &handler, |_| Some(HostEvent::Offline))?;
        me.listen(&window, "pagehide", &handler, |_| Some(HostEvent::PageHide))?;
        // A page that comes back from the back/forward cache has no sockets: `Visible` starts them again.
        let doc = document;
        me.listen(&window, "pageshow", &handler, move |e| {
            let restored = e
                .dyn_ref::<PageTransitionEvent>()
                .is_some_and(PageTransitionEvent::persisted);
            (restored && !is_hidden(&doc)).then_some(HostEvent::Visible)
        })?;
        Ok(me)
    }

    fn listen(
        &mut self,
        target: &EventTarget,
        name: &'static str,
        handler: &Handler,
        map: impl Fn(&Event) -> Option<HostEvent> + 'static,
    ) -> Result<(), String> {
        let h = Rc::clone(handler);
        let cb = Closure::<dyn FnMut(Event)>::new(move |e: Event| {
            let Some(ev) = map(&e) else {
                return;
            };
            match h.try_borrow_mut() {
                Ok(mut f) => f(ev),
                // The handler is running and the browser delivered another event inside it.
                Err(_) => tracing::warn!("page event {name} dropped: handler is busy"),
            }
        });
        target
            .add_event_listener_with_callback(name, cb.as_ref().unchecked_ref())
            .map_err(|e| format!("listener {name}: {}", js_error_text(&e)))?;
        self.listeners.push((target.clone(), name, cb));
        Ok(())
    }
}

impl Drop for PageEvents {
    fn drop(&mut self) {
        for (target, name, cb) in &self.listeners {
            let _ = target.remove_event_listener_with_callback(name, cb.as_ref().unchecked_ref());
        }
    }
}

fn is_hidden(doc: &Document) -> bool {
    doc.visibility_state() == VisibilityState::Hidden
}

/// What is already true when the page starts: a tab opened in the background, a machine with no network.
/// Fed to the client before the first `connect` so its checks start from the right state.
pub fn initial_events() -> Vec<HostEvent> {
    let mut v = Vec::new();
    let Some(window) = web_sys::window() else {
        return v;
    };
    if window.document().is_some_and(|d| is_hidden(&d)) {
        v.push(HostEvent::Hidden);
    }
    if !window.navigator().on_line() {
        v.push(HostEvent::Offline);
    }
    v
}

/// `setInterval`, cleared when dropped. The browser slows it down in a hidden tab by itself.
pub struct IntervalTimer {
    window: Window,
    id: i32,
    _callback: Closure<dyn FnMut()>,
}

impl IntervalTimer {
    /// The period of the safety timer that calls the network pump when no frames come (NC-D9).
    pub const SAFETY_MS: i32 = 250;

    /// # Errors
    /// No `window`, or the browser refuses the timer.
    pub fn start(period_ms: i32, tick: impl FnMut() + 'static) -> Result<Self, String> {
        let window = web_sys::window().ok_or("no window")?;
        let callback = Closure::<dyn FnMut()>::new(tick);
        let id = window
            .set_interval_with_callback_and_timeout_and_arguments_0(
                callback.as_ref().unchecked_ref(),
                period_ms,
            )
            .map_err(|e| format!("setInterval: {}", js_error_text(&e)))?;
        Ok(IntervalTimer {
            window,
            id,
            _callback: callback,
        })
    }
}

impl Drop for IntervalTimer {
    fn drop(&mut self) {
        self.window.clear_interval_with_handle(self.id);
    }
}
