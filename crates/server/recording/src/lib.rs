//! The recording of a game (packet P2): what the gateway saw, in the order it saw it, in files a
//! tool can replay.
//!
//! ```text
//! recordings/g<game>-<created_ms>/
//!   manifest.json              segments, keyframes, sizes (atomic write, rebuilt from the segments when it is stale)
//!   seg-0000.c5rc              records before the first keyframe
//!   seg-0001.c5rc ...          one file per turn: the keyframe record, then everything until the next keyframe
//!   kf/00126-<hash>.full       the C5RB body of the snapshot of the start of a human phase (or .patch against the
//!                              previous keyframe's raw snapshot)
//!   pin.json                   present while a client report pins the recording (retention keeps it)
//! ```
//!
//! A segment is a 16-byte header, then one zstd stream (level 3, content checksum) of records. The writer flushes the
//! stream every two seconds and when a record is urgent (a fault, a hang, a resync, a keyframe), so a process that dies
//! leaves a stream that decodes up to the last flush ([`reader::read_segment`] reports the torn tail).
//!
//! * [`format`]: the record types ([`format::Body`]), their framing and the segment header.
//! * [`writer`]: [`writer::RecWriter`], the synchronous writer (the gateway runs it on a thread of its own).
//! * [`reader`]: [`reader::Recording`], the manifest, the segments, the keyframe bodies.

pub mod format;
pub mod reader;
pub mod writer;

pub use format::{Body, Entry, KfKind, SegHeader};
pub use reader::{Recording, SegmentRead, Tail};
pub use writer::{Config, Manifest, RecWriter};
