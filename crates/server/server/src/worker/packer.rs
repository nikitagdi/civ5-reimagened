//! The pack thread of the worker shell: the snapshots of the replica stream (`game_host::ReplicaJob`).
//!
//! A job is a raw GCS1 snapshot (about 26 MB on a late Huge game). This thread packs it as a `C5RB` snapshot
//! body (ruzstd) and, for a later turn, as a patch against the previous snapshot (zstd level 9, long-distance
//! matching, the previous snapshot as a raw prefix: `zstd --patch-from --long`). The answer goes into the channel the core thread listens on as `Input::Replica`.
//! Nothing here touches the game: it is CPU on a thread of its own, so a step of the core never waits for it.

use std::io;
use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;

use game_host::{PatchFn, ReplicaJob, SnapFn};
use zstd::zstd_safe::{CCtx, CParameter};

use super::io::Input;

/// zstd level of the patch (about 225 KB and 0.4-0.6 s on a late Huge turn).
pub const PATCH_LEVEL: i32 = 9;

/// zstd level of the snapshot body (a late Huge snapshot of 26 MB: about 300 KB; the ruzstd encoder gives 20 times
/// that).
pub const SNAPSHOT_LEVEL: i32 = 9;

/// The zstd frame of a whole snapshot, for [`wire::replica::snapshot_body`].
///
/// # Errors
/// zstd refused the data (the text is the library's).
pub fn encode_snapshot_frame(raw: &[u8]) -> Result<Vec<u8>, String> {
    zstd::bulk::compress(raw, SNAPSHOT_LEVEL).map_err(|e| e.to_string())
}

/// The patch frame of `cur` against `prev` as its raw prefix. The frame holds the content checksum; its window
/// covers `prev` and `cur`. Decoded by `wire::replica::apply_patch`.
///
/// # Errors
/// zstd refused a parameter or the data (the text is the library's).
pub fn encode_patch(prev: &[u8], cur: &[u8]) -> Result<Vec<u8>, String> {
    fn err(what: &'static str) -> impl Fn(usize) -> String {
        move |code| format!("zstd {what}: {}", zstd::zstd_safe::get_error_name(code))
    }
    let mut cctx = CCtx::create();
    cctx.set_parameter(CParameter::CompressionLevel(PATCH_LEVEL))
        .map_err(err("level"))?;
    cctx.set_parameter(CParameter::EnableLongDistanceMatching(true))
        .map_err(err("long"))?;
    cctx.set_parameter(CParameter::ChecksumFlag(true))
        .map_err(err("checksum"))?;
    cctx.set_pledged_src_size(Some(cur.len() as u64))
        .map_err(err("size"))?;
    cctx.ref_prefix(prev).map_err(err("prefix"))?;
    let mut out = vec![0u8; zstd::zstd_safe::compress_bound(cur.len())];
    let n = cctx.compress2(&mut out, cur).map_err(err("compress"))?;
    out.truncate(n);
    Ok(out)
}

/// The thread and the way to it.
pub struct Packer {
    tx: Option<Sender<ReplicaJob>>,
    thread: Option<JoinHandle<()>>,
}

impl Packer {
    /// Starts the thread; `patches` off: every turn goes whole (variant B), the answers go into `done` as
    /// `Input::Replica`.
    pub fn spawn(patches: bool, done: Sender<Input>) -> io::Result<Self> {
        let (tx, rx) = mpsc::channel::<ReplicaJob>();
        let thread = std::thread::Builder::new()
            .name("replica-pack".into())
            .spawn(move || {
                let encoder: &PatchFn = &encode_patch;
                let snap: &SnapFn = &encode_snapshot_frame;
                while let Ok(job) = rx.recv() {
                    let answer = job.pack(Some(snap), patches.then_some(encoder));
                    if done.send(Input::Replica(Box::new(answer))).is_err() {
                        tracing::debug!("the core thread is gone, the packed snapshot is dropped");
                        break;
                    }
                }
            })?;
        Ok(Packer {
            tx: Some(tx),
            thread: Some(thread),
        })
    }

    /// Queues a job. `false`: the thread is gone.
    pub fn send(&self, job: ReplicaJob) -> bool {
        self.tx.as_ref().is_some_and(|tx| tx.send(job).is_ok())
    }

    /// Ends the thread; what is queued is packed first.
    pub fn finish(mut self) {
        self.tx = None;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::xxh64;
    use wire::replica::{apply_patch, parse_header, patch_body};

    /// A pseudo snapshot: structured, a little random.
    fn snapshot(n: usize, seed: u32) -> Vec<u8> {
        let mut x = seed;
        let mut v = Vec::with_capacity(n + 8);
        while v.len() < n {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let k = (x >> 20) & 0xff;
            if k < 40 {
                v.extend_from_slice(&x.to_le_bytes());
            } else {
                v.extend_from_slice(&[(k & 7) as u8, 0, 0, 0, 1, 2, 3, 4]);
            }
        }
        v.truncate(n);
        v
    }

    fn changed(prev: &[u8]) -> Vec<u8> {
        let mut cur = prev.to_vec();
        let mut x = 7u32;
        for _ in 0..prev.len() / 800 {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let p = x as usize % cur.len();
            cur[p] ^= 0x5a;
        }
        cur.splice(10_000..10_000, snapshot(5_000, 99));
        cur.drain(90_000..91_000);
        cur
    }

    fn body_of(prev: &[u8], cur: &[u8]) -> Vec<u8> {
        let frame = encode_patch(prev, cur).unwrap();
        patch_body(cur.len(), xxh64(0, cur), xxh64(0, prev), &frame).unwrap()
    }

    #[test]
    fn a_snapshot_made_by_the_server_decodes_in_the_client_codec() {
        let raw = snapshot(500_000, 2);
        let frame = encode_snapshot_frame(&raw).unwrap();
        let body = wire::replica::snapshot_body(raw.len(), xxh64(0, &raw), &frame).unwrap();
        assert_eq!(wire::replica::decode_snapshot(&body).unwrap(), raw);
    }

    #[test]
    fn a_patch_made_by_the_server_applies_in_the_client_codec() {
        let prev = snapshot(2_000_000, 1);
        let cur = changed(&prev);
        let body = body_of(&prev, &cur);
        assert!(body.len() < cur.len() / 10, "{} bytes", body.len());
        let head = parse_header(&body).unwrap();
        assert_eq!(head.from_raw_hash, Some(xxh64(0, &prev)));
        assert_eq!(apply_patch(&body, &prev, xxh64(0, &prev)).unwrap(), cur);
    }

    #[test]
    fn a_patch_to_a_smaller_snapshot_applies() {
        // The window has to cover the base too, though the new snapshot is much shorter.
        let prev = snapshot(1_000_000, 3);
        let cur = prev[400_000..450_000].to_vec();
        let body = body_of(&prev, &cur);
        assert_eq!(apply_patch(&body, &prev, xxh64(0, &prev)).unwrap(), cur);
        let empty = body_of(&prev, &[]);
        assert_eq!(
            apply_patch(&empty, &prev, xxh64(0, &prev)).unwrap(),
            Vec::<u8>::new()
        );
    }

    #[test]
    fn a_patch_does_not_apply_to_another_base_or_cut_short() {
        let prev = snapshot(300_000, 5);
        let cur = changed(&prev);
        let body = body_of(&prev, &cur);
        let h = xxh64(0, &prev);
        let mut other = prev.clone();
        other[1000] ^= 1;
        // The caller names the hash it holds; a different base under the right hash fails on the result.
        assert!(apply_patch(&body, &other, h).is_err());
        assert!(apply_patch(&body, &prev, h ^ 1).is_err());
        for n in (0..body.len()).step_by(997) {
            assert!(apply_patch(&body[..n], &prev, h).is_err(), "cut at {n}");
        }
        assert!(apply_patch(&body[..body.len() - 1], &prev, h).is_err());
    }
}
