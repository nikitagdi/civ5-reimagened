//! Script workers for the skins and mods of UI v2 (web only).
//!
//! The JS of a skin runs in a Web Worker, never on the main thread. This trait is the transport only: start a
//! worker, send it a text, take the texts it sent, stop it. What the texts mean (the init, frame and out
//! messages) is the business of the `ui2-script` crate, which owns one worker per script context. Like every
//! service here it is polled by the frame: replies a worker posts are queued by the page and taken with
//! [`ScriptHost::drain`].
//!
//! The native client runs no scripts: [`NullScriptHost`] refuses to start a worker, and a skin then works
//! with its markup bindings only.

use thiserror::Error;

/// A running worker, as numbered by the host. Ids are not reused while the host lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WorkerId(pub u32);

/// What a worker sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScriptHostEvent {
    /// A message the worker posted (a JSON text).
    Message(String),
    /// A message the worker posted as bytes (a host that [carries bytes](ScriptHost::carries_bytes)).
    Bytes(Vec<u8>),
    /// The worker itself failed: its loader could not be fetched or threw outside of any skin code. The
    /// worker is unusable afterwards; the owner terminates it.
    Error(String),
}

/// Why a worker could not be started or reached.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ScriptHostError {
    #[error("script workers are not supported on this platform")]
    Unsupported,
    #[error("script worker {0:?} is not running")]
    NoWorker(WorkerId),
    #[error("cannot start a script worker: {0}")]
    Spawn(String),
}

/// Starts and talks to script workers.
pub trait ScriptHost {
    /// Start a new worker running the script loader of the page.
    ///
    /// # Errors
    /// The platform has no workers, or the page refused to create one.
    fn spawn(&mut self) -> Result<WorkerId, ScriptHostError>;

    /// Send a text to a worker. Texts arrive in the order they were posted.
    ///
    /// # Errors
    /// The worker is unknown or was terminated.
    fn post(&mut self, worker: WorkerId, text: &str) -> Result<(), ScriptHostError>;

    /// Whether the messages the workers post as bytes come back as [`ScriptHostEvent::Bytes`]. `false` (the
    /// default): the host takes texts only.
    fn carries_bytes(&self) -> bool {
        false
    }

    /// Append what the workers sent since the last call to `out`, oldest first.
    fn drain(&mut self, out: &mut Vec<(WorkerId, ScriptHostEvent)>);

    /// Stop a worker at once, even in the middle of a long call. Unknown ids are ignored. Events of the worker
    /// that were not drained yet are dropped.
    fn terminate(&mut self, worker: WorkerId);

    /// The time (the app's clock, ms) up to which the page has taken in what the workers posted: an answer a
    /// worker posted before it is in the next [`drain`](Self::drain) (or an earlier one). The page's main
    /// thread may hold the workers' messages back for seconds while it has input to handle and long frames
    /// (a browser runs input and rendering first), so a frame unanswered at the drain is not proof that the
    /// worker did not answer; a watchdog counts the wait up to this time. `None` (the default): the host
    /// takes in every message at once, so the wait counts up to now.
    fn delivered_until(&self) -> Option<f64> {
        None
    }
}

/// The host of a platform without script workers (the native client): every start is refused.
#[derive(Clone, Copy, Debug, Default)]
pub struct NullScriptHost;

impl ScriptHost for NullScriptHost {
    fn spawn(&mut self) -> Result<WorkerId, ScriptHostError> {
        Err(ScriptHostError::Unsupported)
    }

    fn post(&mut self, _worker: WorkerId, _text: &str) -> Result<(), ScriptHostError> {
        Err(ScriptHostError::Unsupported)
    }

    fn drain(&mut self, _out: &mut Vec<(WorkerId, ScriptHostEvent)>) {}

    fn terminate(&mut self, _worker: WorkerId) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_host_refuses_everything() {
        let mut host: Box<dyn ScriptHost> = Box::new(NullScriptHost);
        assert_eq!(host.spawn(), Err(ScriptHostError::Unsupported));
        assert_eq!(
            host.post(WorkerId(0), "{}"),
            Err(ScriptHostError::Unsupported)
        );
        let mut out = Vec::new();
        host.drain(&mut out);
        assert!(out.is_empty());
        host.terminate(WorkerId(0));
    }
}
