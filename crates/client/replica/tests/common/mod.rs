//! Helpers of the state-machine tests: a fake engine, snapshots and batches, a test clock.
#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gamecore::Applied;
use gamecore::replica::{JournalRecord, decode_journal, encode_journal};
use platform::Clock;
use protocol::xxh64;
use replica::{
    Engine, GcBase, GcBlobRef, GcJournal, GcTurn, NetMessage, Replica, ReplicaConfig, ReplicaMsg,
};
use wire::replica::encode_snapshot;

/// What the fake engine saw, and what it is told to do.
#[derive(Default)]
pub struct Log {
    pub loads: Vec<Vec<u8>>,
    /// The records of every applied batch.
    pub applied: Vec<Vec<JournalRecord>>,
    pub outbox: Vec<NetMessage>,
    /// Apply number (0-based, counting calls) that diverges.
    pub diverge_on_call: Option<usize>,
    /// Apply number that fails outright.
    pub fail_on_call: Option<usize>,
    pub fail_load: bool,
    pub calls: usize,
    /// The seat the fake says it plays (`Engine::seat`).
    pub seat: Option<u8>,
}

pub struct Fake(pub Rc<RefCell<Log>>);

impl Engine for Fake {
    fn load(&mut self, snapshot: &[u8]) -> Result<(), String> {
        let mut l = self.0.borrow_mut();
        if l.fail_load {
            return Err("load refused".into());
        }
        l.loads.push(snapshot.to_vec());
        Ok(())
    }

    fn apply(&mut self, batch: &[u8]) -> Result<Applied, String> {
        let mut l = self.0.borrow_mut();
        let n = l.calls;
        l.calls += 1;
        if l.fail_on_call == Some(n) {
            return Err("record refused".into());
        }
        if l.diverge_on_call == Some(n) {
            return Ok(Applied::Diverged("digest differs".into()));
        }
        let recs = decode_journal(batch).map_err(|e| e.to_string())?;
        l.applied.push(recs);
        Ok(Applied::InStep)
    }

    fn take_ui_messages(&mut self) -> Result<Vec<NetMessage>, String> {
        Ok(std::mem::take(&mut self.0.borrow_mut().outbox))
    }

    fn seat(&self) -> Option<u8> {
        self.0.borrow().seat
    }
}

#[derive(Clone, Default)]
pub struct TestClock(pub Rc<Cell<u64>>);

impl Clock for TestClock {
    fn now_ms(&self) -> u64 {
        self.0.get()
    }
}

pub struct Rig {
    pub r: Replica,
    pub log: Rc<RefCell<Log>>,
    pub clock: TestClock,
}

pub fn rig(cfg: ReplicaConfig) -> Rig {
    let log = Rc::new(RefCell::new(Log::default()));
    let clock = TestClock::default();
    let mut r = Replica::with_engine(Box::new(Fake(log.clone())), cfg);
    r.set_clock(Box::new(clock.clone()));
    Rig { r, log, clock }
}

pub fn cfg_always() -> ReplicaConfig {
    ReplicaConfig {
        check_always: true,
        ..ReplicaConfig::default()
    }
}

/// Some GCS1-looking bytes that differ by `n`.
pub fn raw(n: u8) -> Vec<u8> {
    let mut v = b"GCS1".to_vec();
    v.extend((0..4000u32).map(|i| (i as u8).wrapping_mul(7).wrapping_add(n)));
    v
}

pub fn blob_of(body: &[u8]) -> GcBlobRef {
    GcBlobRef {
        hash: xxh64(0, body),
        size: body.len() as u32,
    }
}

/// A snapshot: its raw bytes, its body and the reference to it.
pub struct Snap {
    pub raw: Vec<u8>,
    pub body: Vec<u8>,
    pub blob: GcBlobRef,
    pub raw_hash: u64,
}

pub fn snap(n: u8) -> Snap {
    snap_of(raw(n))
}

pub fn snap_of(raw: Vec<u8>) -> Snap {
    let body = encode_snapshot(&raw).unwrap();
    Snap {
        blob: blob_of(&body),
        raw_hash: xxh64(0, &raw),
        raw,
        body,
    }
}

pub fn base(epoch: u32, turn: i32, jseq: u64, s: &Snap) -> ReplicaMsg {
    ReplicaMsg::Base(GcBase {
        epoch,
        turn,
        jseq,
        raw_hash: s.raw_hash,
        snapshot: s.blob,
    })
}

pub fn turn_msg(epoch: u32, turn: i32, jseq: u64, s: &Snap) -> ReplicaMsg {
    ReplicaMsg::Turn(GcTurn {
        epoch,
        turn,
        jseq,
        raw_hash: s.raw_hash,
        snapshot: s.blob,
        patch: None,
    })
}

/// A batch with one pump record of game turn `turn`.
pub fn pump_batch(turn: i32) -> Vec<u8> {
    encode_journal(&[JournalRecord::Pump {
        slices: 1,
        status: 0,
        turn,
    }])
}

/// A batch with a pump record and a `CHECK`.
pub fn checked_batch(turn: i32) -> Vec<u8> {
    encode_journal(&[
        JournalRecord::Pump {
            slices: 1,
            status: 0,
            turn,
        },
        JournalRecord::Check { digest: 77 },
    ])
}

pub fn journal(epoch: u32, first: u64, batches: Vec<Vec<u8>>) -> ReplicaMsg {
    ReplicaMsg::Journal(GcJournal {
        epoch,
        first_jseq: first,
        batches,
    })
}

/// Joins at `turn` with the snapshot `s` at `jseq`, fetches the body and loads it.
pub fn join(rig: &mut Rig, epoch: u32, turn: i32, jseq: u64, s: &Snap) {
    rig.r.on_msg(base(epoch, turn, jseq, s));
    let want = rig.r.wanted_blobs();
    assert_eq!(want, vec![s.blob]);
    rig.r.blob(s.blob.hash, s.body.clone());
    finish_load(rig);
}

/// Pumps until the machine stops loading; returns the last step.
pub fn finish_load(rig: &mut Rig) -> replica::ReplicaStep {
    use replica::ReplicaStep::*;
    for _ in 0..4 {
        match rig.r.pump(10) {
            Working => continue,
            other => return other,
        }
    }
    panic!("the load did not finish");
}

pub fn pump_all(rig: &mut Rig) {
    for _ in 0..50 {
        if rig.r.pump(1000) == replica::ReplicaStep::Idle {
            return;
        }
    }
}

// ---------------------------------------------------------------- zstd patches (what the server makes)

/// A `C5RB` patch body the way the server makes it (`server::worker::packer::encode_patch`): the frame of `next` with
/// `prev` as a raw prefix (checksum on, long-distance matching, level `level`), decoded against the constant
/// dictionary header of `wire::replica::PATCH_DICT_HEAD`.
pub fn patch_body(prev: &[u8], next: &[u8], level: i32) -> Vec<u8> {
    use zstd::zstd_safe::{CCtx, CParameter};
    let mut cctx = CCtx::create();
    cctx.set_parameter(CParameter::CompressionLevel(level))
        .unwrap();
    cctx.set_parameter(CParameter::EnableLongDistanceMatching(true))
        .unwrap();
    cctx.set_parameter(CParameter::ChecksumFlag(true)).unwrap();
    cctx.set_pledged_src_size(Some(next.len() as u64)).unwrap();
    cctx.ref_prefix(prev).unwrap();
    let mut out = vec![0u8; zstd::zstd_safe::compress_bound(next.len())];
    let n = cctx.compress2(&mut out, next).unwrap();
    out.truncate(n);
    wire::replica::patch_body(next.len(), xxh64(0, next), xxh64(0, prev), &out).unwrap()
}
/// A `C5RB` snapshot body whose frame is made by the C library at `level` (what the server makes; the client reads
/// it with ruzstd all the same). `encode_snapshot` of `wire` is the pure-Rust, much weaker packer.
pub fn snapshot_body_zstd(raw: &[u8], level: i32) -> Vec<u8> {
    let frame = zstd::bulk::compress(raw, level).unwrap();
    let mut out = wire::replica::REPLICA_MAGIC.to_vec();
    out.push(0);
    out.extend_from_slice(&(raw.len() as u32).to_le_bytes());
    out.extend_from_slice(&xxh64(0, raw).to_le_bytes());
    out.extend_from_slice(&frame);
    out
}
