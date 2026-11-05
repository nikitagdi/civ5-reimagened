//! The synchronous writer of a recording: segments, keyframe files, the manifest, the caps and the compaction.
//!
//! No thread, no clock of its own except `Instant` for the flush period: the gateway wraps it in a thread
//! (`server/src/game/recording.rs`) that calls [`RecWriter::write`], [`RecWriter::keyframe`] and, when its queue is
//! idle, [`RecWriter::tick`]. Nothing is created before it is needed: a segment file appears with its first record.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::format::{Body, Entry, HeaderRec, KfKind, SegHeader, WorkerEv, WorkerRec, encode_entry};

/// Name of the manifest in a recording directory.
pub const MANIFEST: &str = "manifest.json";
/// Name of the pin file (a client report keeps the recording).
pub const PIN: &str = "pin.json";
/// Keyframe bodies live here.
pub const KF_DIR: &str = "kf";

/// The knobs of the writer (`[recording]` of the server configuration).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// zstd level of the segments.
    pub level: i32,
    /// The stream is flushed at least this often while records wait.
    pub flush_every: Duration,
    /// ... and when this many bytes of records wait.
    pub flush_bytes: usize,
    /// A full snapshot body is written every this many turns (and after a worker start); the other turns keep the patch.
    pub full_every: u32,
    /// Keyframes older than this many turns are compacted: only the full ones that are a multiple of `full_every` (and
    /// the first after a worker start) stay.
    pub detail_turns: u32,
    /// Over this many bytes the recording stops keeping keyframe bodies; over twice this it stops writing.
    pub max_game_bytes: u64,
    /// The writer stops (the manifest says why) when the disk under the recording has fewer free bytes than this;
    /// 0: no guard. Asked at most every [`FREE_CHECK_EVERY`].
    pub min_free_bytes: u64,
}

/// How often the free space of the disk is asked.
pub const FREE_CHECK_EVERY: Duration = Duration::from_secs(30);

/// Free bytes on the disk that holds `dir` (`df -Pk`); `None` where that cannot be told (not unix, no `df`).
pub fn free_bytes(dir: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        let out = std::process::Command::new("df")
            .arg("-Pk")
            .arg(dir)
            .stdin(std::process::Stdio::null())
            .output()
            .ok()?;
        parse_df(&String::from_utf8_lossy(&out.stdout))
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
        None
    }
}

/// The available kilobytes of the last line of `df -Pk` (column 4), in bytes.
pub fn parse_df(text: &str) -> Option<u64> {
    let line = text.lines().rev().find(|l| !l.trim().is_empty())?;
    let kb: u64 = line.split_whitespace().nth(3)?.parse().ok()?;
    kb.checked_mul(1024)
}

impl Default for Config {
    fn default() -> Self {
        Config {
            level: 3,
            flush_every: Duration::from_secs(2),
            flush_bytes: 64 * 1024,
            full_every: 10,
            detail_turns: 20,
            max_game_bytes: 300 * 1024 * 1024,
            min_free_bytes: 512 * 1024 * 1024,
        }
    }
}

/// Counters of one writer, shared with the gateway (metrics, the measurements of the packet).
#[derive(Debug, Default)]
pub struct Stats {
    pub records: AtomicU64,
    /// Bytes of the records before compression.
    pub raw_bytes: AtomicU64,
    /// Bytes of the segment files written so far.
    pub seg_bytes: AtomicU64,
    /// Bytes of the keyframe files written.
    pub kf_bytes: AtomicU64,
    pub flushes: AtomicU64,
    pub keyframes: AtomicU64,
    /// Time inside the writer (compression and file I/O), microseconds.
    pub busy_us: AtomicU64,
    /// Records dropped because a cap stopped the writer.
    pub dropped: AtomicU64,
}

/// A segment in the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SegInfo {
    pub no: u32,
    /// The turn of its keyframe (0: before the first keyframe).
    pub turn: i32,
    pub file: String,
    pub records: u64,
    /// File size when the segment was closed (or last known).
    pub bytes: u64,
    pub first_wall_ms: u64,
    pub last_wall_ms: u64,
    pub closed: bool,
}

/// A keyframe in the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KfInfo {
    pub turn: i32,
    pub epoch: u32,
    pub jseq: u64,
    pub kind: KfKind,
    pub file: Option<String>,
    pub raw_hash: u64,
    pub hash: u64,
    pub from_raw_hash: Option<u64>,
    pub bytes: u32,
    pub base: bool,
    /// The compaction deleted the body.
    pub removed: bool,
}

/// `manifest.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u16,
    pub game: u32,
    pub name: String,
    pub gateway: String,
    pub core_build: String,
    pub seed: u64,
    pub created_ms: u64,
    pub finished_ms: Option<u64>,
    pub segments: Vec<SegInfo>,
    pub keyframes: Vec<KfInfo>,
    /// Set when a cap stopped the recording.
    pub stopped: Option<String>,
}

impl Manifest {
    pub fn new(h: &HeaderRec) -> Self {
        Manifest {
            format: h.format,
            game: h.game,
            name: h.name.clone(),
            gateway: h.gateway.clone(),
            core_build: h.core_build.clone(),
            seed: h.seed,
            created_ms: h.created_ms,
            finished_ms: None,
            segments: Vec::new(),
            keyframes: Vec::new(),
            stopped: None,
        }
    }
}

/// A file counted as it is written.
struct Counting {
    inner: File,
    total: Arc<AtomicU64>,
}

impl Write for Counting {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.total.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

struct Seg {
    /// Index in `manifest.segments`.
    idx: usize,
    enc: zstd::stream::write::Encoder<'static, Counting>,
    pending_in: usize,
    last_flush: Instant,
    /// Bytes this segment has written to its file.
    written: Arc<AtomicU64>,
    /// Bytes of `written` that the counters hold already.
    booked: u64,
}

/// The writer of one recording directory.
pub struct RecWriter {
    dir: PathBuf,
    game: u32,
    cfg: Config,
    manifest: Manifest,
    seg: Option<Seg>,
    /// The segment that comes next lazily: its number and the turn of its keyframe.
    next_no: u32,
    next_turn: i32,
    stats: Arc<Stats>,
    /// Bytes on disk now (segments and the keyframes that were not compacted).
    disk: u64,
    stopped: bool,
    kf_stopped: bool,
    /// When the free space was asked last, and the answer source (a test replaces it).
    free_checked: Option<Instant>,
    free_probe: fn(&Path) -> Option<u64>,
}

#[cfg(unix)]
fn private(opts: &mut fs::OpenOptions) -> &mut fs::OpenOptions {
    use std::os::unix::fs::OpenOptionsExt;
    opts.mode(0o600)
}

#[cfg(not(unix))]
fn private(opts: &mut fs::OpenOptions) -> &mut fs::OpenOptions {
    opts
}

/// Writes `bytes` to `path` through a temporary file in the same directory.
///
/// # Errors
/// The file cannot be written or renamed.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    {
        let mut f = private(
            fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true),
        )
        .open(&tmp)?;
        f.write_all(bytes)?;
    }
    fs::rename(&tmp, path)
}

fn micros(t: Instant) -> u64 {
    u64::try_from(t.elapsed().as_micros()).unwrap_or(u64::MAX)
}

/// `seg-0007.c5rc`.
pub fn seg_name(no: u32) -> String {
    format!("seg-{no:04}.c5rc")
}

/// `00126-<hash>.full`.
pub fn kf_name(turn: i32, hash: u64, kind: KfKind) -> String {
    let ext = match kind {
        KfKind::Full => "full",
        KfKind::Patch => "patch",
    };
    format!("{turn:05}-{hash:016x}.{ext}")
}

impl RecWriter {
    /// Makes the directory and the manifest. No segment file yet.
    ///
    /// # Errors
    /// The directory cannot be made or the manifest written.
    pub fn create(
        dir: &Path,
        cfg: Config,
        header: &HeaderRec,
        stats: Arc<Stats>,
    ) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let w = RecWriter {
            dir: dir.to_path_buf(),
            game: header.game,
            cfg,
            manifest: Manifest::new(header),
            seg: None,
            next_no: 0,
            next_turn: 0,
            stats,
            disk: 0,
            stopped: false,
            kf_stopped: false,
            free_checked: None,
            free_probe: free_bytes,
        };
        w.save_manifest()?;
        Ok(w)
    }

    /// Replaces the free-space probe (tests).
    pub fn set_free_probe(&mut self, f: fn(&Path) -> Option<u64>) {
        self.free_probe = f;
    }

    /// The free-disk guard: asked at most every [`FREE_CHECK_EVERY`]; under the minimum the recording stops and the
    /// manifest says so. `true` when it stopped now.
    fn check_free(&mut self, now: Instant) -> bool {
        if self.stopped || self.cfg.min_free_bytes == 0 {
            return false;
        }
        if self
            .free_checked
            .is_some_and(|t| now.saturating_duration_since(t) < FREE_CHECK_EVERY)
        {
            return false;
        }
        self.free_checked = Some(now);
        let Some(free) = (self.free_probe)(&self.dir) else {
            return false;
        };
        if free >= self.cfg.min_free_bytes {
            return false;
        }
        self.stopped = true;
        self.manifest.stopped = Some(format!(
            "stopped: the disk has {} MiB free, under the {} MiB minimum",
            free / (1024 * 1024),
            self.cfg.min_free_bytes / (1024 * 1024)
        ));
        let _ = self.flush_seg();
        let _ = self.save_manifest();
        true
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The core build the worker reported (known after the header was written).
    pub fn set_core_build(&mut self, build: &str) {
        if self.manifest.core_build != build {
            self.manifest.core_build = build.to_owned();
        }
    }

    fn save_manifest(&self) -> io::Result<()> {
        let text = serde_json::to_vec_pretty(&self.manifest).map_err(io::Error::other)?;
        write_atomic(&self.dir.join(MANIFEST), &text)
    }

    /// Opens the file of the pending segment: header, then the zstd stream.
    fn open_seg(&mut self, first_wall_ms: u64) -> io::Result<()> {
        if self.seg.is_some() {
            return Ok(());
        }
        let no = self.next_no;
        let file_name = seg_name(no);
        let mut file = private(fs::OpenOptions::new().write(true).create_new(true))
            .open(self.dir.join(&file_name))?;
        let header = SegHeader::new(self.game, self.next_turn).encode();
        file.write_all(&header)?;
        let written = Arc::new(AtomicU64::new(header.len() as u64));
        let counting = Counting {
            inner: file,
            total: Arc::clone(&written),
        };
        let mut enc = zstd::stream::write::Encoder::new(counting, self.cfg.level)?;
        enc.include_checksum(true)?;
        let idx = self.manifest.segments.len();
        self.manifest.segments.push(SegInfo {
            no,
            turn: self.next_turn,
            file: file_name,
            records: 0,
            bytes: header.len() as u64,
            first_wall_ms,
            last_wall_ms: first_wall_ms,
            closed: false,
        });
        self.seg = Some(Seg {
            idx,
            enc,
            pending_in: 0,
            last_flush: Instant::now(),
            booked: header.len() as u64,
            written,
        });
        self.disk += header.len() as u64;
        self.stats
            .seg_bytes
            .fetch_add(header.len() as u64, Ordering::Relaxed);
        Ok(())
    }

    fn book(&mut self, seg_idx: usize, now: u64, booked: u64) {
        let added = now.saturating_sub(booked);
        self.manifest.segments[seg_idx].bytes = now;
        self.disk += added;
        self.stats.seg_bytes.fetch_add(added, Ordering::Relaxed);
    }

    fn flush_seg(&mut self) -> io::Result<()> {
        let Some(seg) = self.seg.as_mut() else {
            return Ok(());
        };
        if seg.pending_in == 0 {
            return Ok(());
        }
        seg.enc.flush()?;
        seg.pending_in = 0;
        seg.last_flush = Instant::now();
        let now = seg.written.load(Ordering::Relaxed);
        let (idx, booked) = (seg.idx, seg.booked);
        seg.booked = now;
        self.stats.flushes.fetch_add(1, Ordering::Relaxed);
        self.book(idx, now, booked);
        Ok(())
    }

    fn close_seg(&mut self) -> io::Result<()> {
        let Some(seg) = self.seg.take() else {
            return Ok(());
        };
        let Seg {
            idx,
            enc,
            written,
            booked,
            ..
        } = seg;
        drop(enc.finish()?);
        let now = written.load(Ordering::Relaxed);
        self.manifest.segments[idx].closed = true;
        self.book(idx, now, booked);
        Ok(())
    }

    /// Writes one record.
    ///
    /// # Errors
    /// The file refused the bytes. The recording is then stopped (a later call does nothing).
    pub fn write(&mut self, e: &Entry) -> io::Result<()> {
        let t = Instant::now();
        let r = self.write_inner(e);
        self.stats.busy_us.fetch_add(micros(t), Ordering::Relaxed);
        if r.is_err() {
            self.stopped = true;
        }
        r
    }

    fn write_inner(&mut self, e: &Entry) -> io::Result<()> {
        if self.stopped {
            self.stats.dropped.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
        if self.check_free(Instant::now()) {
            self.stats.dropped.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
        let framed = encode_entry(e).map_err(io::Error::other)?;
        self.open_seg(e.wall_ms)?;
        let Some(seg) = self.seg.as_mut() else {
            return Ok(());
        };
        seg.enc.write_all(&framed)?;
        seg.pending_in += framed.len();
        let info = &mut self.manifest.segments[seg.idx];
        info.records += 1;
        info.last_wall_ms = e.wall_ms;
        self.stats.records.fetch_add(1, Ordering::Relaxed);
        self.stats
            .raw_bytes
            .fetch_add(framed.len() as u64, Ordering::Relaxed);
        if e.body.urgent() || seg.pending_in >= self.cfg.flush_bytes {
            self.flush_seg()?;
        }
        self.check_caps(e.mono_us, e.wall_ms)
    }

    /// The per-game caps.
    fn check_caps(&mut self, mono_us: u64, wall_ms: u64) -> io::Result<()> {
        if !self.kf_stopped && self.disk > self.cfg.max_game_bytes {
            self.kf_stopped = true;
            self.manifest.stopped = Some("keyframes stopped: the per-game cap was reached".into());
            self.note(
                mono_us,
                wall_ms,
                "the per-game cap was reached: no more keyframe bodies",
            )?;
        }
        if !self.stopped && self.disk > self.cfg.max_game_bytes.saturating_mul(2) {
            self.note(
                mono_us,
                wall_ms,
                "twice the per-game cap: the recording stops",
            )?;
            self.stopped = true;
            self.manifest.stopped = Some("stopped: twice the per-game cap".into());
            self.flush_seg()?;
            self.save_manifest()?;
        }
        Ok(())
    }

    fn note(&mut self, mono_us: u64, wall_ms: u64, text: &str) -> io::Result<()> {
        let e = Entry {
            mono_us,
            wall_ms,
            body: Body::Worker(WorkerRec {
                ev: WorkerEv::Note(text.to_owned()),
            }),
        };
        let framed = encode_entry(&e).map_err(io::Error::other)?;
        if let Some(seg) = self.seg.as_mut() {
            seg.enc.write_all(&framed)?;
            seg.pending_in += framed.len();
        }
        Ok(())
    }

    /// A keyframe: closes the segment, opens the next one with this record, keeps the body (unless a cap says not to)
    /// and compacts the older keyframes.
    ///
    /// # Errors
    /// A file refused the bytes.
    pub fn keyframe(&mut self, mut e: Entry, body: Option<&[u8]>) -> io::Result<()> {
        let t = Instant::now();
        let r = self.keyframe_inner(&mut e, body);
        self.stats.busy_us.fetch_add(micros(t), Ordering::Relaxed);
        if r.is_err() {
            self.stopped = true;
        }
        r
    }

    fn keyframe_inner(&mut self, e: &mut Entry, body: Option<&[u8]>) -> io::Result<()> {
        if self.stopped {
            self.stats.dropped.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
        let Body::Keyframe(k) = &mut e.body else {
            return Err(io::Error::other("keyframe() needs a Keyframe record"));
        };
        self.flush_seg()?;
        self.close_seg()?;
        self.next_no = self.manifest.segments.last().map_or(0, |s| s.no + 1);
        self.next_turn = k.turn;
        if let Some(bytes) = body
            && !self.kf_stopped
        {
            let name = kf_name(k.turn, k.hash, k.kind);
            let kf_dir = self.dir.join(KF_DIR);
            fs::create_dir_all(&kf_dir)?;
            write_atomic(&kf_dir.join(&name), bytes)?;
            self.disk += bytes.len() as u64;
            self.stats
                .kf_bytes
                .fetch_add(bytes.len() as u64, Ordering::Relaxed);
            k.file = Some(name);
        } else {
            k.file = None;
        }
        self.stats.keyframes.fetch_add(1, Ordering::Relaxed);
        let info = KfInfo {
            turn: k.turn,
            epoch: k.epoch,
            jseq: k.jseq,
            kind: k.kind,
            file: k.file.clone(),
            raw_hash: k.raw_hash,
            hash: k.hash,
            from_raw_hash: k.from_raw_hash,
            bytes: k.bytes,
            base: k.base,
            removed: false,
        };
        let turn = k.turn;
        self.manifest.keyframes.push(info);
        self.write_inner(&*e)?;
        self.compact(turn);
        self.save_manifest()
    }

    /// Deletes the keyframe bodies of turns older than `detail_turns` that are not the full ones a replay can start
    /// from. The segments stay: those turns are replayed from the previous full keyframe through the journal.
    fn compact(&mut self, now_turn: i32) {
        let horizon =
            now_turn.saturating_sub(i32::try_from(self.cfg.detail_turns).unwrap_or(i32::MAX));
        let full_every = i32::try_from(self.cfg.full_every.max(1)).unwrap_or(i32::MAX);
        let dir = self.dir.join(KF_DIR);
        for kf in &mut self.manifest.keyframes {
            if kf.removed || kf.turn >= horizon {
                continue;
            }
            let keep = kf.kind == KfKind::Full && (kf.turn % full_every == 0 || kf.base);
            if keep {
                continue;
            }
            if let Some(name) = &kf.file {
                let path = dir.join(name);
                let len = fs::metadata(&path).map_or(0, |m| m.len());
                if fs::remove_file(&path).is_ok() {
                    self.disk = self.disk.saturating_sub(len);
                }
            }
            kf.removed = true;
        }
    }

    /// Flushes the stream if records have waited long enough.
    pub fn tick(&mut self, now: Instant) {
        self.check_free(now);
        let due = self.seg.as_ref().is_some_and(|s| {
            s.pending_in > 0 && now.saturating_duration_since(s.last_flush) >= self.cfg.flush_every
        });
        if due {
            let t = Instant::now();
            if self.flush_seg().is_err() {
                self.stopped = true;
            }
            self.stats.busy_us.fetch_add(micros(t), Ordering::Relaxed);
        }
    }

    /// Ends the stream with its checksum and writes the manifest.
    ///
    /// # Errors
    /// A file refused the bytes.
    pub fn finish(&mut self, finished_ms: Option<u64>) -> io::Result<()> {
        let t = Instant::now();
        let r = (|| {
            self.flush_seg()?;
            self.close_seg()?;
            self.manifest.finished_ms = finished_ms;
            self.save_manifest()
        })();
        self.stats.busy_us.fetch_add(micros(t), Ordering::Relaxed);
        r
    }
}
