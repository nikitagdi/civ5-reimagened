//! `platform` implementations for native: tungstenite WebSocket, thread pool, monotonic clock.
//! [`block_on`] runs async renderer init.

mod audio;
mod blob;
mod executor;
mod http;
mod kv;
mod tasks;
mod transport;

pub use audio::NativeAudio;
pub use blob::StubBlobStore;
pub use executor::block_on;
pub use http::StdHttp;
pub use kv::FileStore;
pub use tasks::ThreadPool;
pub use transport::WsTransport;

use std::time::Instant;

/// Zeroed at creation.
#[derive(Debug, Clone, Copy)]
pub struct NativeClock {
    start: Instant,
}

impl NativeClock {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl Default for NativeClock {
    fn default() -> Self {
        Self::new()
    }
}

impl platform::Clock for NativeClock {
    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    fn now_ms_f64(&self) -> f64 {
        self.start.elapsed().as_secs_f64() * 1000.0
    }
}
