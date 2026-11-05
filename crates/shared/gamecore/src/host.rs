//! Host callbacks: logging, VFS reads, game events.

use crate::GameEvent;

/// Log level of a gamecore message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl LogLevel {
    pub(crate) fn from_abi(level: i32) -> Self {
        match level {
            i32::MIN..=0 => LogLevel::Error,
            1 => LogLevel::Warn,
            2 => LogLevel::Info,
            3 => LogLevel::Debug,
            _ => LogLevel::Trace,
        }
    }
}

/// Host services. Callbacks must not re-enter the same game.
pub trait HostServices: Send {
    /// Log message. Default: `tracing`, target `gamecore`.
    fn log(&self, level: LogLevel, msg: &str) {
        match level {
            LogLevel::Error => tracing::error!(target: "gamecore", "{msg}"),
            LogLevel::Warn => tracing::warn!(target: "gamecore", "{msg}"),
            LogLevel::Info => tracing::info!(target: "gamecore", "{msg}"),
            LogLevel::Debug => tracing::debug!(target: "gamecore", "{msg}"),
            LogLevel::Trace => tracing::trace!(target: "gamecore", "{msg}"),
        }
    }

    /// File at a Civ5 VFS path, or `None`. Default: no files.
    fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        let _ = path;
        None
    }

    /// Game event.
    fn game_event(&self, ev: GameEvent);
}

/// Adapts `HostServices` to the `gamecore-sys` callbacks.
pub(crate) struct HostAdapter<H: HostServices>(pub H);

impl<H: HostServices> gamecore_sys::Host for HostAdapter<H> {
    fn log(&self, level: i32, msg: &str) {
        self.0.log(LogLevel::from_abi(level), msg);
    }

    fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        self.0.read_file(path)
    }

    fn game_event(&self, ev: &gamecore_sys::RawEvent<'_>) {
        self.0.game_event(GameEvent::from_raw(ev));
    }
}
