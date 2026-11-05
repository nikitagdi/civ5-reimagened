//! WebSocket on `web_sys::WebSocket`. Browser handlers push into a queue that `poll` drains.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use platform::{CloseReason, Connection, Transport, TransportEvent};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{BinaryType, CloseEvent, Event, MessageEvent, WebSocket};

/// The text of the `CloseReason::Error` a socket closes with when its queue passed the limit.
pub const BACKLOG_REASON: &str = "backlog";

#[derive(Debug, Default, Clone, Copy)]
pub struct WebSocketTransport;

impl WebSocketTransport {
    /// A transport whose sockets close themselves when more than `backlog_bytes` of messages wait unread
    /// (a hidden tab whose timers are slowed down, net-client 4.8). The close shows as
    /// `CloseReason::Error("backlog")`, and the client reconnects with `resume` when the tab returns.
    pub fn with_backlog(backlog_bytes: usize) -> LimitedWebSocketTransport {
        LimitedWebSocketTransport { backlog_bytes }
    }
}

impl Transport for WebSocketTransport {
    fn connect(&self, url: &str) -> Box<dyn Connection> {
        Box::new(WsConnection::open(url, usize::MAX))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct LimitedWebSocketTransport {
    backlog_bytes: usize,
}

impl Transport for LimitedWebSocketTransport {
    fn connect(&self, url: &str) -> Box<dyn Connection> {
        Box::new(WsConnection::open(url, self.backlog_bytes))
    }
}

#[derive(Default)]
struct Shared {
    events: VecDeque<TransportEvent>,
    /// No error details come from the browser.
    errored: bool,
    /// Bytes of `Message` events in `events`.
    queued_bytes: usize,
    /// The queue went over the limit: later messages are dropped and `poll` closes the socket.
    over_limit: bool,
}

struct Handlers {
    _open: Closure<dyn FnMut(Event)>,
    _message: Closure<dyn FnMut(MessageEvent)>,
    _error: Closure<dyn FnMut(Event)>,
    _close: Closure<dyn FnMut(CloseEvent)>,
}

struct WsConnection {
    ws: Option<WebSocket>,
    shared: Rc<RefCell<Shared>>,
    _handlers: Option<Handlers>,
    /// Queued until `Opened`.
    pending: Vec<Vec<u8>>,
    opened: bool,
    finished: bool,
}

const CONNECTING: u16 = 0;
const OPEN: u16 = 1;

impl WsConnection {
    fn open(url: &str, backlog_bytes: usize) -> Self {
        let shared = Rc::new(RefCell::new(Shared::default()));
        let ws =
            match WebSocket::new(url) {
                Ok(ws) => ws,
                Err(e) => {
                    // A malformed URL makes the constructor throw immediately.
                    shared.borrow_mut().events.push_back(TransportEvent::Closed(
                        CloseReason::Error(format!("WebSocket {url}: {}", js_error_text(&e))),
                    ));
                    return Self {
                        ws: None,
                        shared,
                        _handlers: None,
                        pending: Vec::new(),
                        opened: false,
                        finished: false,
                    };
                }
            };
        ws.set_binary_type(BinaryType::Arraybuffer);

        let s = Rc::clone(&shared);
        let open = Closure::<dyn FnMut(Event)>::new(move |_| {
            s.borrow_mut().events.push_back(TransportEvent::Opened);
        });
        let s = Rc::clone(&shared);
        let message = Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
            let _t = crate::stats::between("js.ws_message");
            match e.data().dyn_into::<js_sys::ArrayBuffer>() {
                Ok(buf) => {
                    let mut sh = s.borrow_mut();
                    if sh.over_limit {
                        return;
                    }
                    let len = buf_len(&buf);
                    crate::delivery::note_net_in(len);
                    if sh.queued_bytes.saturating_add(len) > backlog_bytes {
                        // Nobody reads: stop growing the queue at once; `poll` closes the socket.
                        sh.over_limit = true;
                        return;
                    }
                    let bytes = js_sys::Uint8Array::new(&buf).to_vec();
                    sh.queued_bytes += bytes.len();
                    sh.events.push_back(TransportEvent::Message(bytes));
                }
                Err(_) => tracing::warn!("WebSocket: ignored non-binary message"),
            }
        });
        let s = Rc::clone(&shared);
        let error = Closure::<dyn FnMut(Event)>::new(move |_| {
            s.borrow_mut().errored = true;
        });
        let s = Rc::clone(&shared);
        let close = Closure::<dyn FnMut(CloseEvent)>::new(move |e: CloseEvent| {
            let mut sh = s.borrow_mut();
            let reason = if sh.errored && !e.was_clean() {
                CloseReason::Error(format!(
                    "connection not established or dropped (code {})",
                    e.code()
                ))
            } else {
                CloseReason::Remote {
                    code: e.code(),
                    reason: e.reason(),
                }
            };
            sh.events.push_back(TransportEvent::Closed(reason));
        });
        ws.set_onopen(Some(open.as_ref().unchecked_ref()));
        ws.set_onmessage(Some(message.as_ref().unchecked_ref()));
        ws.set_onerror(Some(error.as_ref().unchecked_ref()));
        ws.set_onclose(Some(close.as_ref().unchecked_ref()));

        Self {
            ws: Some(ws),
            shared,
            _handlers: Some(Handlers {
                _open: open,
                _message: message,
                _error: error,
                _close: close,
            }),
            pending: Vec::new(),
            opened: false,
            finished: false,
        }
    }

    fn detach(&mut self) {
        if let Some(ws) = self.ws.take() {
            ws.set_onopen(None);
            ws.set_onmessage(None);
            ws.set_onerror(None);
            ws.set_onclose(None);
            let state = ws.ready_state();
            if state == CONNECTING || state == OPEN {
                let _ = ws.close();
            }
        }
        self._handlers = None;
    }

    fn send_now(&mut self, bytes: &[u8]) {
        if let Some(ws) = &self.ws {
            // The threaded build's memory is shared: `send` refuses a view of it, so the bytes go as
            // a copy (B3 render-mt).
            #[cfg(target_feature = "atomics")]
            let sent = ws.send_with_array_buffer_view(&js_sys::Uint8Array::from(bytes));
            #[cfg(not(target_feature = "atomics"))]
            let sent = ws.send_with_u8_array(bytes);
            match sent {
                Ok(()) => crate::delivery::note_net_out(bytes.len()),
                Err(e) => tracing::warn!("WebSocket send: {}", js_error_text(&e)),
            }
        }
    }
}

impl Connection for WsConnection {
    fn send(&mut self, bytes: Vec<u8>) {
        if self.finished {
            return;
        }
        if self.opened {
            self.send_now(&bytes);
        } else {
            self.pending.push(bytes);
        }
    }

    fn poll(&mut self) -> Option<TransportEvent> {
        if self.finished {
            return None;
        }
        if self.shared.borrow().over_limit {
            // The tab was too slow to read: the socket is closed from here, the unread messages go with it.
            tracing::warn!("WebSocket closed: the queue of unread messages is over its limit");
            self.finished = true;
            self.pending.clear();
            self.detach();
            let mut sh = self.shared.borrow_mut();
            sh.events.clear();
            sh.queued_bytes = 0;
            return Some(TransportEvent::Closed(CloseReason::Error(
                BACKLOG_REASON.into(),
            )));
        }
        let ev = {
            let mut sh = self.shared.borrow_mut();
            let ev = sh.events.pop_front()?;
            if let TransportEvent::Message(m) = &ev {
                sh.queued_bytes = sh.queued_bytes.saturating_sub(m.len());
            }
            ev
        };
        match &ev {
            TransportEvent::Opened => {
                self.opened = true;
                for bytes in std::mem::take(&mut self.pending) {
                    self.send_now(&bytes);
                }
            }
            TransportEvent::Closed(_) => {
                self.finished = true;
                self.pending.clear();
                self.detach();
            }
            TransportEvent::Message(_) => {}
        }
        Some(ev)
    }

    /// Also for a socket that has not opened yet: its handlers go first, so a late `open` or `error` of
    /// the browser reaches nobody, then the browser is told to give up the attempt.
    fn close(&mut self) {
        if self.finished {
            return;
        }
        self.pending.clear();
        self.detach();
        let mut sh = self.shared.borrow_mut();
        sh.events.clear();
        sh.queued_bytes = 0;
        sh.events
            .push_back(TransportEvent::Closed(CloseReason::Local));
    }
}

impl Drop for WsConnection {
    fn drop(&mut self) {
        self.detach();
    }
}

/// Length of an `ArrayBuffer` of a message, to count it before copying it into the queue.
fn buf_len(buf: &js_sys::ArrayBuffer) -> usize {
    buf.byte_length() as usize
}

/// JS exception text, for logging.
pub(crate) fn js_error_text(v: &wasm_bindgen::JsValue) -> String {
    if let Some(e) = v.dyn_ref::<js_sys::Error>() {
        return String::from(e.message());
    }
    v.as_string().unwrap_or_else(|| format!("{v:?}"))
}
