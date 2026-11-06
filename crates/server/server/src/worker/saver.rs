//! The save thread of the worker shell (game-host-mp 4.11, 4.16) and the file side of loading.
//!
//! The core thread hands a `SaveJob` over; this thread packs it (`saves::encode`, the core's part with
//! `wire::compress`), writes `<name>.tmp`, `fsync`s it, renames it to its name, trims the ring and answers
//! with a `SaveDone` into the channel the core thread listens on. A file is whole or not there: a cut write
//! leaves the earlier saves as they were.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::thread::JoinHandle;
use std::time::{SystemTime, UNIX_EPOCH};

use game_host::saves::{self, KEEP_DIR, RING_DIR, Save};
use game_host::{SaveDone, SaveJob};

use super::io::Input;

/// The thread and the way to it.
pub struct Saver {
    tx: Option<Sender<SaveJob>>,
    thread: Option<JoinHandle<()>>,
}

impl Saver {
    /// Starts the thread; `dir` is the save directory of the game (`Init.save_dir`), the answers go into
    /// `done` as `Input::Saved`.
    pub fn spawn(dir: PathBuf, done: Sender<Input>) -> io::Result<Self> {
        let (tx, rx) = mpsc::channel::<SaveJob>();
        let thread = std::thread::Builder::new()
            .name("saver".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    let file = job.file;
                    let (saved_at_ms, result) = write_job(&dir, job);
                    if let Err(why) = &result {
                        tracing::warn!("save {} not written: {why}", file.rel_path());
                    }
                    let answer = SaveDone {
                        file,
                        saved_at_ms,
                        result,
                    };
                    if done.send(Input::Saved(Box::new(answer))).is_err() {
                        // The core thread is gone; what is queued is still written.
                        tracing::debug!("the core thread is gone, the answer is dropped");
                    }
                }
            })?;
        Ok(Saver {
            tx: Some(tx),
            thread: Some(thread),
        })
    }

    /// Queues a job. `false`: the thread is gone.
    pub fn send(&self, job: SaveJob) -> bool {
        self.tx.as_ref().is_some_and(|tx| tx.send(job).is_ok())
    }

    /// Waits until everything queued is written.
    pub fn finish(mut self) {
        self.tx = None;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// zstd level of the core part of a save (`GCS1`, 30 MB on a late Huge turn). Measured on game 936 turn 126
/// (one thread of a desktop CPU): level 1 0.83 MB in 10 ms, 3 0.71 MB in 13 ms, 7 0.62 MB in 42 ms, 9 0.61 MB in
/// 54 ms, 12 0.60 MB in 100 ms; the pure Rust encoder gave 3.94 MB in 118 ms. Level 7 is the knee: 9 and
/// above gain under 3 %. It runs on this thread, not on the core thread, so a save never holds up a step.
pub const SAVE_LEVEL: i32 = 7;

/// The `GCS1` section body: the raw length, then a zstd frame of the C library. Old saves (the pure Rust
/// encoder's frames) and these both load through `wire::decompress`. If the library refuses, the pure Rust
/// encoder does the job.
fn pack_gcs1(raw: &[u8]) -> Vec<u8> {
    match zstd::bulk::compress(raw, SAVE_LEVEL) {
        Ok(frame) => wire::compress_with(raw, |_| frame),
        Err(e) => {
            tracing::warn!("zstd refused the save ({e}), the fast encoder packs it");
            wire::compress(raw)
        }
    }
}

/// Packs and writes one save; returns the time put into its header and the size or the reason.
fn write_job(dir: &Path, mut job: SaveJob) -> (u64, Result<u64, String>) {
    let at = now_unix_ms();
    job.header.saved_at_ms = at;
    let bytes = match saves::try_encode_with(&job.header, &job.gcs1, &job.sections, pack_gcs1) {
        Ok(bytes) => bytes,
        Err(e) => {
            // Nothing is written: a save with an empty section would only be found broken at wake time.
            tracing::error!(error = %e, "a save cannot be encoded, not written");
            return (at, Err(e.to_string()));
        }
    };
    let result = write_file(dir, &job.file.rel_path(), &bytes)
        .map(|()| bytes.len() as u64)
        .map_err(|e| e.to_string());
    if result.is_ok() {
        trim_ring(dir, job.ring);
    }
    (at, result)
}

/// `<dir>/<rel>` written whole: `.tmp`, `fsync`, rename, then the directory made durable where the
/// platform allows.
fn write_file(dir: &Path, rel: &str, bytes: &[u8]) -> io::Result<()> {
    let path = dir.join(rel);
    let parent = path.parent().unwrap_or(dir);
    fs::create_dir_all(parent)?;
    let mut tmp = path.clone().into_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let written = (|| {
        let mut f = File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = fs::rename(&tmp, &path) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    #[cfg(unix)]
    if let Ok(d) = File::open(parent) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// The files of `dir` that are saves, with their number: `(seq, file name)`.
fn list(dir: &Path) -> Vec<(u64, String)> {
    let Ok(rd) = fs::read_dir(dir) else {
        return Vec::new();
    };
    rd.filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let (seq, _turn) = saves::parse_file_name(&name)?;
            Some((seq, name))
        })
        .collect()
}

/// Deletes the oldest files of `ring/` beyond `keep`, and the `.tmp` files a crash left.
fn trim_ring(dir: &Path, keep: u32) {
    let ring = dir.join(RING_DIR);
    let files = list(&ring);
    let seqs: Vec<u64> = files.iter().map(|(s, _)| *s).collect();
    for seq in saves::ring_excess(&seqs, keep as usize) {
        for (s, name) in &files {
            if *s == seq
                && let Err(e) = fs::remove_file(ring.join(name))
            {
                tracing::warn!("cannot delete the old save {name}: {e}");
            }
        }
    }
    if let Ok(rd) = fs::read_dir(&ring) {
        for e in rd.filter_map(Result::ok) {
            if e.file_name().to_string_lossy().ends_with(".c5sv.tmp") {
                let _ = fs::remove_file(e.path());
            }
        }
    }
}

/// The highest save number in `ring/` and `keep/` of `dir` (0: none), so that new saves go above it.
pub fn newest_seq(dir: &Path) -> u64 {
    [RING_DIR, KEEP_DIR]
        .iter()
        .flat_map(|d| list(&dir.join(d)))
        .map(|(s, _)| s)
        .max()
        .unwrap_or(0)
}

/// Reads and decodes the save `Start::Load` names: an absolute path as it is (a game that starts from the
/// save of another game), a relative one against `dir`. The error is the text of the `Fatal` (code 4).
pub fn load_save(dir: &Path, file: &str) -> Result<Save, String> {
    let given = Path::new(file);
    let path = if given.is_absolute() {
        given.to_path_buf()
    } else {
        dir.join(given)
    };
    let bytes =
        fs::read(&path).map_err(|e| format!("cannot read the save {}: {e}", path.display()))?;
    saves::decode(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use game_host::SaveFile;
    use game_host::saves::{SaveHeader, SaveSections};
    use protocol::{AutoChoices, GameSettings, SaveKind, TimerMode, TurnMode};

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("civ5-saver-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    /// Something shaped like a core save: records of small counters and ids with long-range repeats.
    fn fake_gcs1(n: usize) -> Vec<u8> {
        let mut v = Vec::with_capacity(n * 40);
        let mut x: u64 = 0x9e37_79b9;
        for i in 0..n {
            x = x
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            v.extend_from_slice(&(i as u32).to_le_bytes());
            v.extend_from_slice(&((x >> 60) as u32).to_le_bytes());
            v.extend_from_slice(&[0u8; 12]);
            v.extend_from_slice(&(((i % 97) as u64) * 1000).to_le_bytes());
            v.extend_from_slice(&(((x >> 50) & 0xff) as u32).to_le_bytes());
            v.extend_from_slice(b"BUILDING_BARRACKS\0\0\0\0");
        }
        v
    }

    fn job(gcs1: Vec<u8>) -> SaveJob {
        SaveJob {
            file: SaveFile {
                seq: 3,
                kind: SaveKind::Ring,
                turn: 5,
            },
            header: SaveHeader {
                seq: 3,
                game: 9,
                turn: 5,
                saved_at_ms: 0,
                kind: SaveKind::Ring,
                build_id: "core-1".into(),
            },
            gcs1,
            sections: SaveSections {
                seats: Vec::new(),
                settings: GameSettings {
                    name: "t".into(),
                    world_size: "WORLDSIZE_DUEL".into(),
                    map_script: "m.lua".into(),
                    map_options: Vec::new(),
                    speed: "GAMESPEED_STANDARD".into(),
                    start_era: "ERA_ANCIENT".into(),
                    handicap: "HANDICAP_PRINCE".into(),
                    minor_civs: 0,
                    turn_mode: TurnMode::Hybrid,
                    timer: TimerMode::Off,
                    game_options: Vec::new(),
                    seed: Some(3),
                    map_seed: None,
                    auto_choices: AutoChoices::default(),
                    diplo_auto_decline: true,
                    mods: Vec::new(),
                },
                journals: Vec::new(),
                cmds: Vec::new(),
                city_memo: Vec::new(),
                mods: None,
            },
            ring: 10,
        }
    }

    #[test]
    fn a_save_packed_by_the_c_library_loads_and_is_much_smaller() {
        let dir = scratch("c-zstd");
        let gcs1 = fake_gcs1(200_000);
        let old = saves::encode(&job(gcs1.clone()).header, &gcs1, &job(Vec::new()).sections);
        let (_, written) = write_job(&dir, job(gcs1.clone()));
        let size = written.unwrap();
        // The synthetic data is noisier than a real save (the old encoder gets 1.7x here); the real one is 6x (the
        // `save_pack_measure` test prints it).
        assert!(
            size * 3 < old.len() as u64 * 2,
            "{size} vs old {}",
            old.len()
        );
        let save = load_save(&dir, "ring/00000003-t0005.c5sv").unwrap();
        assert_eq!(save.gcs1, gcs1);
        assert_eq!(save.info.header.game, 9);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_save_of_the_old_encoder_still_loads() {
        let dir = scratch("old-save");
        fs::create_dir_all(dir.join("keep")).unwrap();
        let gcs1 = fake_gcs1(5_000);
        let j = job(gcs1.clone());
        let old = saves::encode(&j.header, &gcs1, &j.sections);
        fs::write(dir.join("keep/00000003-t0005.c5sv"), &old).unwrap();
        let save = load_save(&dir, "keep/00000003-t0005.c5sv").unwrap();
        assert_eq!(save.gcs1, gcs1);
        let _ = fs::remove_dir_all(&dir);
    }
}
