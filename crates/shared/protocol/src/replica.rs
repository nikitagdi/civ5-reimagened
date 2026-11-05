//! The replica stream (protocol v4): snapshot references, journal batches and the client's
//! side of the exchange. The bodies themselves travel over HTTP (`GET /replica/{hash:016x}`, codec
//! `wire::replica`); the stream only carries references and journal batches. Field order is the wire format.

use serde::{Deserialize, Serialize};

/// A body served at `GET /replica/{hash:016x}`; `hash` is the xxh64 of the whole body, `size` its length.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GcBlobRef {
    pub hash: u64,
    pub size: u32,
}

/// The base of a replica chain: a snapshot to build on. `jseq` is the number of the last journal batch the
/// snapshot already contains; the next batch is `jseq + 1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GcBase {
    pub epoch: u32,
    pub turn: i32,
    pub jseq: u64,
    /// xxh64 of the raw GCS1 bytes of the snapshot.
    pub raw_hash: u64,
    pub snapshot: GcBlobRef,
}

/// Journal batches (GCJ1) of the human phase, `first_jseq`, `first_jseq + 1`, ... in order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GcJournal {
    pub epoch: u32,
    pub first_jseq: u64,
    pub batches: Vec<Vec<u8>>,
}

/// A patch body from the snapshot with `from_raw_hash` to the one of the enclosing `GcTurn`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GcPatchRef {
    pub from_raw_hash: u64,
    pub blob: GcBlobRef,
}

/// A turn change: the snapshot of the start of the new human phase and, when the server makes them, a patch
/// from the previous one. `None` patch: the client loads `snapshot`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct GcTurn {
    pub epoch: u32,
    pub turn: i32,
    pub jseq: u64,
    pub raw_hash: u64,
    pub snapshot: GcBlobRef,
    pub patch: Option<GcPatchRef>,
}

/// Why the client asks for a new base (`ClientMsg::GcResync`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GcResyncReason {
    /// A `CHECK` record did not match.
    Diverged,
    /// A journal record was refused by the replica.
    Rejected,
    /// A body did not decode or its hash is wrong.
    BadBlob,
    /// A journal batch is missing.
    Gap,
}

/// Largest `ClientMsg::GcNet` payload (it stays inside the 64 KiB client frame).
pub const GC_NET_MAX: usize = 64 * 1024;
