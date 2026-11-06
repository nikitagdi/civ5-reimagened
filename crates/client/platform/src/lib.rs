//! Platform service traits and shared input types (architecture §5.3, invariant P2).
//!
//! The client core depends only on these traits; implementations live in `platform-web` and
//! `platform-native`. Services are synchronous and polled once per frame, with no `async`, so
//! the same code runs on single-threaded web (D8) and natively. Long operations return a handle.

mod audio;
mod blob;
mod http;
mod input;
mod pointer_lock;
mod store;
mod task;
mod time;
mod transport;
mod ui_bridge;

pub use audio::{AudioOutError, AudioOutput, NullOutput, Pcm, SampleId, VoiceId, VoiceParams};
pub use blob::{BlobError, BlobKey, BlobOp, BlobStore};
pub use http::{HttpClient, HttpError, HttpOp, HttpResponse};
pub use input::{
    InputEvent, Key, KeyEvent, Modifiers, MouseButton, NamedKey, PhysicalPosition, PhysicalSize,
    PointerEvent, PointerId, PointerKind, PointerPhase, WheelDelta, WheelEvent,
};
pub use pointer_lock::{
    CursorGrab, GrabError, PointerLock, PointerLockMode, PointerLockProbe, PointerLockStatus,
};
pub use store::{HostEvent, KeyValueStore, MemoryStore};
pub use task::{Job, TaskPool};
pub use time::Clock;
pub use transport::{CloseReason, Connection, Transport, TransportEvent};
pub use ui_bridge::UiBridge;
