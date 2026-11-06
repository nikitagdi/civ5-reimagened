//! `platform` for the browser: web-sys WebSocket, `performance.now()` clock, task queue, OPFS (R1).

mod audio;
pub mod delivery;
pub mod host;
mod http;
mod log;
pub mod page;
mod pointer_lock;
pub mod stats;
mod store;
mod transport;
mod ui_bridge;

use std::cell::RefCell;
use std::collections::VecDeque;

use platform::{Clock, Job, TaskPool};

pub use audio::WebAudio;
pub use http::{FetchHttp, fetch_get};
pub use log::{
    CRASH_ID, ConsoleMakeWriter, ConsoleWriter, RingLayer, flush_ring, init_logging, ring_line_now,
    show_crash,
};
pub use pointer_lock::WebPointerLockProbe;
pub use store::LocalStorage;
pub use transport::{BACKLOG_REASON, LimitedWebSocketTransport, WebSocketTransport};
pub use ui_bridge::WebUiBridge;

/// Falls back to `Date.now()` (not monotonic).
pub struct WebClock {
    performance: Option<web_sys::Performance>,
}

impl WebClock {
    pub fn new() -> Self {
        Self {
            performance: web_sys::window().and_then(|w| w.performance()),
        }
    }

    /// Milliseconds since the page started (`performance.now()`), with the fraction.
    pub fn now_ms_f64(&self) -> f64 {
        self.now_f64()
    }

    fn now_f64(&self) -> f64 {
        match &self.performance {
            Some(p) => p.now(),
            None => js_sys::Date::now(),
        }
    }
}

impl Default for WebClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for WebClock {
    fn now_ms(&self) -> u64 {
        self.now_f64().max(0.0) as u64
    }

    fn now_ms_f64(&self) -> f64 {
        self.now_f64()
    }
}

/// Jobs run in `run_pending` within the budget.
pub struct WebTaskPool {
    queue: RefCell<VecDeque<Job>>,
    clock: WebClock,
}

impl WebTaskPool {
    pub fn new() -> Self {
        Self {
            queue: RefCell::new(VecDeque::new()),
            clock: WebClock::new(),
        }
    }
}

impl Default for WebTaskPool {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskPool for WebTaskPool {
    fn spawn(&self, job: Job) {
        self.queue.borrow_mut().push_back(job);
    }

    fn run_pending(&self, budget_ms: u32) {
        let start = self.clock.now_f64();
        loop {
            // Release the borrow first: a job may enqueue more jobs.
            let Some(job) = self.queue.borrow_mut().pop_front() else {
                break;
            };
            job();
            if self.clock.now_f64() - start >= f64::from(budget_ms) {
                break;
            }
        }
    }
}
