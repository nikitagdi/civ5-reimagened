//! Variant A of the turn change: a zstd patch made like the server's (the previous snapshot as a raw prefix, C library) and read by
//! wire::replica::apply_patch (ruzstd, the constant dictionary header).

mod common;

use common::*;
use replica::{ReplicaConfig, ReplicaMsg, ReplicaStep};
use wire::replica::{REPLICA_HEAD_SNAPSHOT, encode_snapshot};

/// The decoder of the wire crate, the one the replica uses.
fn apply(base: &[u8], body: &[u8]) -> Result<Vec<u8>, wire::replica::ReplicaError> {
    wire::replica::apply_patch(body, base, protocol::xxh64(0, base))
}

/// A deterministic pseudo-random byte stream.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// A snapshot-like buffer: records of fields with small integers, runs of zeros, some text.
fn game_like(len: usize, seed: u64) -> Vec<u8> {
    let mut r = Rng(seed);
    let mut v = b"GCS1".to_vec();
    while v.len() < len {
        let kind = r.next() % 4;
        match kind {
            0 => v.extend(std::iter::repeat_n(0u8, (r.next() % 200) as usize)),
            1 => v.extend((0..16).map(|_| (r.next() % 7) as u8)),
            2 => v.extend_from_slice(
                format!("unit_{}_city_{}", r.next() % 50, r.next() % 20).as_bytes(),
            ),
            _ => v.extend((0..8).map(|_| r.next() as u8)),
        }
    }
    v.truncate(len);
    v
}

/// The next turn: the same buffer with scattered changes and a changed tail.
fn next_turn(prev: &[u8], seed: u64) -> Vec<u8> {
    let mut r = Rng(seed);
    let mut v = prev.to_vec();
    for _ in 0..(v.len() / 200) {
        let at = (r.next() as usize) % v.len();
        v[at] = r.next() as u8;
    }
    let tail = v.len() - 3000;
    v.splice(tail.., game_like(4000, seed ^ 0x55));
    v
}

#[test]
fn ruzstd_applies_a_patch_made_by_the_c_library() {
    let prev = game_like(600_000, 1);
    let next = next_turn(&prev, 2);
    let body = patch_body(&prev, &next, 9);
    let full = encode_snapshot(&next).unwrap();
    assert!(
        body.len() * 3 < full.len(),
        "patch {} vs snapshot {}",
        body.len(),
        full.len()
    );
    assert_eq!(apply(&prev, &body).unwrap(), next);
    // Higher levels read the same way.
    for level in [3, 16] {
        let b = patch_body(&prev, &next, level);
        assert_eq!(apply(&prev, &b).unwrap(), next, "level {level}");
    }
}

#[test]
fn a_patch_against_another_base_or_with_damage_is_refused() {
    let prev = game_like(100_000, 1);
    let next = next_turn(&prev, 2);
    let body = patch_body(&prev, &next, 9);
    let other = game_like(100_000, 9);
    assert!(apply(&other, &body).is_err());
    // A flipped byte in the frame: an error or a hash mismatch, never a wrong snapshot.
    for at in [
        REPLICA_HEAD_SNAPSHOT + 8 + 20,
        body.len() / 2,
        body.len() - 2,
    ] {
        let mut bad = body.clone();
        bad[at] ^= 0x10;
        match apply(&prev, &bad) {
            Err(_) => {}
            Ok(v) => assert_eq!(v, next, "damage at {at} went through"),
        }
    }
    // Every truncation is an error.
    for n in (0..body.len()).step_by(997) {
        assert!(apply(&prev, &body[..n]).is_err(), "cut at {n}");
    }
}

#[test]
fn a_frame_that_says_more_than_the_header_declares_is_refused() {
    let prev = game_like(50_000, 1);
    let next = next_turn(&prev, 2);
    let mut body = patch_body(&prev, &next, 9);
    body[5..9].copy_from_slice(&((next.len() - 10) as u32).to_le_bytes());
    assert!(apply(&prev, &body).is_err());
}

/// The replica takes the patch for a turn change when it holds the base the patch is against.
#[test]
fn a_turn_change_by_patch_loads_the_new_snapshot() {
    let mut g = rig(ReplicaConfig {
        check_always: true,
        ..ReplicaConfig::default()
    });
    let raw1 = game_like(200_000, 1);
    let raw2 = next_turn(&raw1, 2);
    let s1 = snap_of(raw1.clone());
    let s2 = snap_of(raw2.clone());
    join(&mut g, 1, 5, 0, &s1);

    let pbody = patch_body(&raw1, &raw2, 9);
    let msg = ReplicaMsg::Turn(replica::GcTurn {
        epoch: 1,
        turn: 6,
        jseq: 0,
        raw_hash: s2.raw_hash,
        snapshot: s2.blob,
        patch: Some(replica::GcPatchRef {
            from_raw_hash: s1.raw_hash,
            blob: blob_of(&pbody),
        }),
    });
    g.r.on_msg(msg);
    // The patch is asked for, not the snapshot.
    assert_eq!(g.r.wanted_blobs(), vec![blob_of(&pbody)]);
    g.r.blob(blob_of(&pbody).hash, pbody);
    assert_eq!(finish_load(&mut g), ReplicaStep::TurnSwitched { turn: 6 });
    assert_eq!(g.log.borrow().loads.last().unwrap(), &raw2);
    let st = g.r.stats();
    assert_eq!(
        (st.turn_patch, st.turn_snapshot, st.patch_fallbacks),
        (1, 0, 0)
    );
    // The new base is raw until the machine is idle, then packed.
    assert!(!st.base_packed);
    g.clock.0.set(5_000);
    assert_eq!(g.r.pump(10), ReplicaStep::Working);
    assert!(g.r.stats().base_packed);
    assert!(g.r.stats().base_bytes < raw2.len() as u64);

    // And the next turn patches against the packed base.
    let raw3 = next_turn(&raw2, 3);
    let s3 = snap_of(raw3.clone());
    let pbody = patch_body(&raw2, &raw3, 9);
    g.r.on_msg(ReplicaMsg::Turn(replica::GcTurn {
        epoch: 1,
        turn: 7,
        jseq: 0,
        raw_hash: s3.raw_hash,
        snapshot: s3.blob,
        patch: Some(replica::GcPatchRef {
            from_raw_hash: s2.raw_hash,
            blob: blob_of(&pbody),
        }),
    }));
    assert_eq!(g.r.wanted_blobs(), vec![blob_of(&pbody)]);
    g.r.blob(blob_of(&pbody).hash, pbody);
    finish_load(&mut g);
    assert_eq!(g.log.borrow().loads.last().unwrap(), &raw3);
    assert_eq!(g.r.stats().turn_patch, 2);
}

/// A patch that fails falls back to the full snapshot of the same turn, without a resync.
#[test]
fn a_bad_patch_falls_back_to_the_snapshot() {
    let mut g = rig(ReplicaConfig {
        check_always: true,
        ..ReplicaConfig::default()
    });
    let raw1 = game_like(100_000, 1);
    let raw2 = next_turn(&raw1, 2);
    let (s1, s2) = (snap_of(raw1.clone()), snap_of(raw2.clone()));
    join(&mut g, 1, 5, 0, &s1);
    let mut pbody = patch_body(&raw1, &raw2, 9);
    let n = pbody.len() / 2;
    pbody[n] ^= 0xff;
    g.r.on_msg(ReplicaMsg::Turn(replica::GcTurn {
        epoch: 1,
        turn: 6,
        jseq: 0,
        raw_hash: s2.raw_hash,
        snapshot: s2.blob,
        patch: Some(replica::GcPatchRef {
            from_raw_hash: s1.raw_hash,
            blob: blob_of(&pbody),
        }),
    }));
    g.r.wanted_blobs();
    g.r.blob(blob_of(&pbody).hash, pbody);
    // The damaged patch is refused (the base is lost with it); the snapshot is asked for.
    for _ in 0..3 {
        g.r.pump(10);
        if !g.r.wanted_blobs().is_empty() {
            break;
        }
    }
    assert_eq!(g.r.stats().patch_fallbacks, 1);
    assert!(g.r.switching());
    g.r.blob(s2.blob.hash, s2.body.clone());
    assert_eq!(finish_load(&mut g), ReplicaStep::TurnSwitched { turn: 6 });
    assert_eq!(g.log.borrow().loads.last().unwrap(), &raw2);
    assert!(g.r.take_outgoing().is_empty(), "no GcResync");
    let st = g.r.stats();
    assert_eq!((st.turn_patch, st.turn_snapshot), (0, 1));
}

/// With patches off, or against a base the replica does not hold, the snapshot is the one asked for.
#[test]
fn no_patches_or_a_foreign_base_means_the_snapshot() {
    let raw1 = game_like(50_000, 1);
    let raw2 = next_turn(&raw1, 2);
    let (s1, s2) = (snap_of(raw1.clone()), snap_of(raw2.clone()));
    let junk_patch = replica::GcPatchRef {
        from_raw_hash: s1.raw_hash,
        blob: blob_of(b"patch"),
    };
    // Patches off.
    let mut g = rig(ReplicaConfig {
        patches: false,
        ..cfg_always()
    });
    join(&mut g, 1, 5, 0, &s1);
    g.r.on_msg(ReplicaMsg::Turn(replica::GcTurn {
        epoch: 1,
        turn: 6,
        jseq: 0,
        raw_hash: s2.raw_hash,
        snapshot: s2.blob,
        patch: Some(junk_patch),
    }));
    assert_eq!(g.r.wanted_blobs(), vec![s2.blob]);
    // Patches on, but the patch is against a snapshot the replica never held.
    let mut g = rig(ReplicaConfig {
        check_always: true,
        ..ReplicaConfig::default()
    });
    join(&mut g, 1, 5, 0, &s1);
    let mut p = junk_patch;
    p.from_raw_hash ^= 1;
    g.r.on_msg(ReplicaMsg::Turn(replica::GcTurn {
        epoch: 1,
        turn: 6,
        jseq: 0,
        raw_hash: s2.raw_hash,
        snapshot: s2.blob,
        patch: Some(p),
    }));
    assert_eq!(g.r.wanted_blobs(), vec![s2.blob]);
}

#[test]
fn a_server_made_patch_decodes_in_the_replica_with_the_default_config() {
    let prev = game_like(600_000, 1);
    let next = next_turn(&prev, 2);
    let body = patch_body(&prev, &next, 9);
    assert!(ReplicaConfig::default().patches);
    assert_eq!(apply(&prev, &body).unwrap(), next);
}
