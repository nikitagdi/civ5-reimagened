//! Reading a recording: the manifest (rebuilt from the segments when it is missing or stale), the segments (a torn tail
//! is reported, not an error) and the keyframe bodies.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::format::{Body, Entry, HEADER_LEN, Next, SegHeader, decode_next};
use crate::writer::{KF_DIR, KfInfo, MANIFEST, Manifest, PIN, SegInfo};

/// How a segment ends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tail {
    /// The stream ended with its checksum.
    Clean,
    /// The stream stops early (the process died, or the file is damaged): the records before it are good.
    Torn(String),
}

/// The decoded contents of one segment.
#[derive(Debug)]
pub struct SegmentRead {
    pub path: PathBuf,
    pub header: SegHeader,
    pub entries: Vec<Entry>,
    /// Records this build could not decode: `(tag, bytes)`.
    pub skipped: Vec<(u8, usize)>,
    pub tail: Tail,
    /// Size of the file.
    pub file_bytes: u64,
    /// Size of the decoded stream.
    pub raw_bytes: u64,
}

/// `pin.json`: a client report keeps the recording until `until_ms`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pin {
    pub until_ms: u64,
    /// The report id.
    pub report: String,
    pub pinned_ms: u64,
}

/// A recording directory.
#[derive(Debug)]
pub struct Recording {
    pub dir: PathBuf,
    pub manifest: Manifest,
    /// The manifest was missing or unreadable and was rebuilt from the segments.
    pub rebuilt: bool,
}

/// Decodes a segment file.
///
/// # Errors
/// The file cannot be opened or is not a segment.
pub fn read_segment(path: &Path) -> Result<SegmentRead, String> {
    let mut file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let file_bytes = file.metadata().map_or(0, |m| m.len());
    let mut head = [0u8; HEADER_LEN];
    file.read_exact(&mut head)
        .map_err(|e| format!("{}: header: {e}", path.display()))?;
    let header = SegHeader::decode(&head).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut dec = zstd::stream::read::Decoder::new(file).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    let mut chunk = vec![0u8; 256 * 1024];
    let mut tail = Tail::Clean;
    loop {
        match dec.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) => {
                tail = Tail::Torn(format!("the zstd stream ends early: {e}"));
                break;
            }
        }
    }
    let raw_bytes = buf.len() as u64;
    let mut entries = Vec::new();
    let mut skipped = Vec::new();
    let mut at = 0usize;
    loop {
        match decode_next(&buf[at..]) {
            Next::Entry(e, n) => {
                entries.push(*e);
                at += n;
            }
            Next::Skipped { tag, len, total } => {
                skipped.push((tag, len));
                at += total;
            }
            Next::Partial => {
                if tail == Tail::Clean {
                    tail = Tail::Torn(format!(
                        "a record is cut after {} records ({} bytes left)",
                        entries.len(),
                        buf.len() - at
                    ));
                }
                break;
            }
            Next::End => break,
        }
    }
    Ok(SegmentRead {
        path: path.to_path_buf(),
        header,
        entries,
        skipped,
        tail,
        file_bytes,
        raw_bytes,
    })
}

/// `seg-NNNN.c5rc` files of a directory, in order.
fn segment_files(dir: &Path) -> Vec<(u32, PathBuf)> {
    let mut v: Vec<(u32, PathBuf)> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let no = name
                .strip_prefix("seg-")?
                .strip_suffix(".c5rc")?
                .parse::<u32>()
                .ok()?;
            Some((no, e.path()))
        })
        .collect();
    v.sort_by_key(|(n, _)| *n);
    v
}

impl Recording {
    /// Opens a recording directory.
    ///
    /// # Errors
    /// The directory holds neither a manifest nor a readable segment.
    pub fn open(dir: &Path) -> Result<Self, String> {
        if !dir.is_dir() {
            return Err(format!("{} is not a directory", dir.display()));
        }
        let manifest = fs::read(dir.join(MANIFEST))
            .ok()
            .and_then(|b| serde_json::from_slice::<Manifest>(&b).ok());
        match manifest {
            Some(m) => Ok(Recording {
                dir: dir.to_path_buf(),
                manifest: m,
                rebuilt: false,
            }),
            None => Self::rebuild(dir),
        }
    }

    /// The manifest from the segments (a recording whose gateway died before it wrote one).
    fn rebuild(dir: &Path) -> Result<Self, String> {
        let files = segment_files(dir);
        if files.is_empty() {
            return Err(format!("{}: no manifest and no segment", dir.display()));
        }
        let mut m = Manifest {
            format: 0,
            game: 0,
            name: String::new(),
            gateway: String::new(),
            core_build: String::new(),
            seed: 0,
            created_ms: 0,
            finished_ms: None,
            segments: Vec::new(),
            keyframes: Vec::new(),
            stopped: None,
        };
        for (no, path) in files {
            let seg = read_segment(&path)?;
            m.game = seg.header.game;
            let (mut first, mut last) = (0, 0);
            for (i, e) in seg.entries.iter().enumerate() {
                if i == 0 {
                    first = e.wall_ms;
                }
                last = e.wall_ms;
                match &e.body {
                    Body::Header(h) => {
                        m.format = h.format;
                        m.name.clone_from(&h.name);
                        m.gateway.clone_from(&h.gateway);
                        m.core_build.clone_from(&h.core_build);
                        m.seed = h.seed;
                        m.created_ms = h.created_ms;
                    }
                    Body::Keyframe(k) => m.keyframes.push(KfInfo {
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
                        removed: k
                            .file
                            .as_ref()
                            .is_some_and(|f| !dir.join(KF_DIR).join(f).is_file()),
                    }),
                    _ => {}
                }
            }
            m.segments.push(SegInfo {
                no,
                turn: seg.header.turn,
                file: path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                records: seg.entries.len() as u64,
                bytes: seg.file_bytes,
                first_wall_ms: first,
                last_wall_ms: last,
                closed: seg.tail == Tail::Clean,
            });
        }
        Ok(Recording {
            dir: dir.to_path_buf(),
            manifest: m,
            rebuilt: true,
        })
    }

    /// The segment files on disk, in order (the manifest may be older than the files).
    pub fn segments(&self) -> Vec<PathBuf> {
        segment_files(&self.dir)
            .into_iter()
            .map(|(_, p)| p)
            .collect()
    }

    /// Every segment, decoded.
    ///
    /// # Errors
    /// A segment is not a segment.
    pub fn read_all(&self) -> Result<Vec<SegmentRead>, String> {
        self.segments().iter().map(|p| read_segment(p)).collect()
    }

    /// The bytes of a keyframe file; its xxh64 must be the record's `hash`.
    ///
    /// # Errors
    /// The file is gone (compacted or never kept) or does not match its hash.
    pub fn keyframe_body(&self, file: &str, hash: u64) -> Result<Vec<u8>, String> {
        if file.contains(['/', '\\']) || file.contains("..") {
            return Err(format!("a keyframe file name with a path: {file}"));
        }
        let path = self.dir.join(KF_DIR).join(file);
        let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let got = protocol::xxh64(0, &bytes);
        if got != hash {
            return Err(format!(
                "{file}: hash {got:016x}, the record says {hash:016x}"
            ));
        }
        Ok(bytes)
    }

    /// The pin, if the recording has one.
    pub fn pin(&self) -> Option<Pin> {
        read_pin(&self.dir)
    }
}

/// `pin.json` of a recording directory.
pub fn read_pin(dir: &Path) -> Option<Pin> {
    serde_json::from_slice(&fs::read(dir.join(PIN)).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use protocol::RecPhase;

    use super::*;
    use crate::format::{
        HeaderRec, JournalRec, KeyframeRec, KfKind, NetRec, VERSION, WorkerEv, WorkerRec,
    };
    use crate::writer::{Config, FREE_CHECK_EVERY, RecWriter, Stats, parse_df};

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("civ5-rec-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    fn header() -> HeaderRec {
        HeaderRec {
            format: VERSION,
            game: 7,
            gateway: "t".into(),
            core_build: "core-1".into(),
            seed: 3,
            wall_at_mono0_ms: 1000,
            created_ms: 1000,
            name: "t".into(),
            settings: None,
            seats: Vec::new(),
        }
    }

    fn e(i: u64, body: Body) -> Entry {
        Entry {
            mono_us: i * 1000,
            wall_ms: 1000 + i,
            body,
        }
    }

    fn journal(i: u64) -> Entry {
        e(
            i,
            Body::Journal(JournalRec {
                phase: RecPhase::Human,
                epoch: 1,
                jseq: i,
                turn: 1,
                bytes: vec![i as u8; 100],
            }),
        )
    }

    fn kf(turn: i32, kind: KfKind, base: bool) -> Entry {
        e(
            u64::try_from(turn).unwrap_or(0),
            Body::Keyframe(KeyframeRec {
                turn,
                epoch: 1,
                jseq: 0,
                raw_hash: 100 + u64::try_from(turn).unwrap_or(0),
                hash: protocol::xxh64(0, &[turn as u8; 50]),
                kind,
                from_raw_hash: (kind == KfKind::Patch).then_some(1),
                file: None,
                bytes: 50,
                base,
            }),
        )
    }

    #[test]
    fn records_and_keyframes_read_back_in_order() {
        let dir = scratch("roundtrip");
        let stats = Arc::new(Stats::default());
        let mut w =
            RecWriter::create(&dir, Config::default(), &header(), Arc::clone(&stats)).unwrap();
        w.write(&e(0, Body::Header(header()))).unwrap();
        for i in 1..=3 {
            w.write(&journal(i)).unwrap();
        }
        w.keyframe(kf(1, KfKind::Full, true), Some(&[1u8; 50]))
            .unwrap();
        w.write(&journal(4)).unwrap();
        w.keyframe(kf(2, KfKind::Patch, false), Some(&[2u8; 50]))
            .unwrap();
        w.write(&journal(5)).unwrap();
        w.finish(Some(9999)).unwrap();
        let r = Recording::open(&dir).unwrap();
        assert!(!r.rebuilt);
        assert_eq!(r.manifest.segments.len(), 3);
        assert_eq!(r.manifest.finished_ms, Some(9999));
        let segs = r.read_all().unwrap();
        assert_eq!(segs.len(), 3);
        // seg 0: header + 3 journal; seg 1: keyframe 1 + journal; seg 2: keyframe 2 + journal.
        assert_eq!(
            segs.iter().map(|s| s.entries.len()).collect::<Vec<_>>(),
            [4, 2, 2]
        );
        assert!(segs.iter().all(|s| s.tail == Tail::Clean));
        assert_eq!(segs[1].header.turn, 1);
        assert_eq!(segs[2].header.turn, 2);
        let Body::Keyframe(k) = &segs[1].entries[0].body else {
            panic!("a segment starts with its keyframe")
        };
        let file = k.file.clone().unwrap();
        assert_eq!(r.keyframe_body(&file, k.hash).unwrap(), vec![1u8; 50]);
        assert!(r.keyframe_body(&file, k.hash ^ 1).is_err());
        assert!(r.keyframe_body("../x", 0).is_err());
        assert!(stats.seg_bytes.load(std::sync::atomic::Ordering::Relaxed) > 0);
        let on_disk: u64 = r
            .segments()
            .iter()
            .map(|p| fs::metadata(p).unwrap().len())
            .sum();
        assert_eq!(
            on_disk,
            stats.seg_bytes.load(std::sync::atomic::Ordering::Relaxed)
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_killed_writer_leaves_a_readable_prefix() {
        let dir = scratch("torn");
        let mut w = RecWriter::create(&dir, Config::default(), &header(), Arc::default()).unwrap();
        w.write(&e(0, Body::Header(header()))).unwrap();
        for i in 1..=5 {
            w.write(&journal(i)).unwrap();
        }
        // An urgent record flushes the stream.
        w.write(&e(
            6,
            Body::Worker(WorkerRec {
                ev: WorkerEv::Fault { why: "boom".into() },
            }),
        ))
        .unwrap();
        // Records after the last flush are in memory only.
        for i in 7..=9 {
            w.write(&journal(i)).unwrap();
        }
        // The process dies: no finish, no drop of the encoder's end frame.
        std::mem::forget(w);
        let r = Recording::open(&dir).unwrap();
        let segs = r.read_all().unwrap();
        assert_eq!(segs.len(), 1);
        let seg = &segs[0];
        assert!(matches!(seg.tail, Tail::Torn(_)), "{:?}", seg.tail);
        // The header, five journal batches and the fault survive, the three after the flush do not.
        assert_eq!(seg.entries.len(), 7, "{:?}", seg.tail);
        assert!(matches!(seg.entries[6].body, Body::Worker(_)));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_cut_in_the_middle_reads_up_to_the_cut() {
        let dir = scratch("cut");
        let cfg = Config {
            flush_bytes: 2000,
            ..Config::default()
        };
        let mut w = RecWriter::create(&dir, cfg, &header(), Arc::default()).unwrap();
        for i in 1..=200u64 {
            let mut x = i.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
            let bytes: Vec<u8> = (0..100)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    x as u8
                })
                .collect();
            let mut j = journal(i);
            if let Body::Journal(r) = &mut j.body {
                r.bytes = bytes;
            }
            w.write(&j).unwrap();
        }
        w.finish(None).unwrap();
        let path = dir.join("seg-0000.c5rc");
        let bytes = fs::read(&path).unwrap();
        fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
        let seg = read_segment(&path).unwrap();
        assert!(matches!(seg.tail, Tail::Torn(_)));
        assert!(!seg.entries.is_empty() && seg.entries.len() < 200);
        // The manifest is gone too: it is rebuilt from what is there.
        fs::remove_file(dir.join("manifest.json")).unwrap();
        let r = Recording::open(&dir).unwrap();
        assert!(r.rebuilt);
        assert_eq!(r.manifest.game, 7);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn flushes_follow_the_period_and_the_size() {
        let dir = scratch("flush");
        let cfg = Config {
            flush_every: Duration::from_millis(50),
            flush_bytes: 1 << 20,
            ..Config::default()
        };
        let mut w = RecWriter::create(&dir, cfg, &header(), Arc::default()).unwrap();
        w.write(&journal(1)).unwrap();
        let file = dir.join("seg-0000.c5rc");
        let len0 = fs::metadata(&file).unwrap().len();
        // Not yet: the record waits in memory.
        w.tick(Instant::now());
        assert_eq!(fs::metadata(&file).unwrap().len(), len0);
        std::thread::sleep(Duration::from_millis(80));
        w.tick(Instant::now());
        assert!(
            fs::metadata(&file).unwrap().len() > len0,
            "the period flushed"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn compaction_keeps_the_detail_window_and_the_full_ones() {
        let dir = scratch("compact");
        let cfg = Config {
            detail_turns: 5,
            full_every: 10,
            ..Config::default()
        };
        let mut w = RecWriter::create(&dir, cfg, &header(), Arc::default()).unwrap();
        for turn in 1..=30 {
            let full = turn % 10 == 0 || turn == 1;
            let kind = if full { KfKind::Full } else { KfKind::Patch };
            w.keyframe(kf(turn, kind, turn == 1), Some(&[turn as u8; 50]))
                .unwrap();
            w.write(&journal(u64::try_from(turn).unwrap_or(0))).unwrap();
        }
        w.finish(None).unwrap();
        let r = Recording::open(&dir).unwrap();
        let alive: Vec<i32> = r
            .manifest
            .keyframes
            .iter()
            .filter(|k| !k.removed)
            .map(|k| k.turn)
            .collect();
        // The window is turns 25..=30; older: the base (1) and the multiples of 10.
        assert_eq!(alive, [1, 10, 20, 25, 26, 27, 28, 29, 30]);
        let files = fs::read_dir(dir.join("kf")).unwrap().count();
        assert_eq!(files, alive.len());
        // All 30 segments (and the one before the first keyframe does not exist) stay.
        assert_eq!(r.segments().len(), 30);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_caps_stop_keyframes_and_then_the_recording() {
        let dir = scratch("caps");
        let cfg = Config {
            max_game_bytes: 3000,
            ..Config::default()
        };
        let stats = Arc::new(Stats::default());
        let mut w = RecWriter::create(&dir, cfg, &header(), Arc::clone(&stats)).unwrap();
        w.keyframe(kf(1, KfKind::Full, true), Some(&vec![1u8; 2000]))
            .unwrap();
        assert!(w.manifest().keyframes[0].file.is_some());
        w.keyframe(kf(2, KfKind::Full, false), Some(&vec![2u8; 2000]))
            .unwrap();
        // 4000 bytes on disk: over the cap, so the next keyframe keeps no body.
        w.keyframe(kf(3, KfKind::Full, false), Some(&vec![3u8; 2000]))
            .unwrap();
        assert!(w.manifest().keyframes[2].file.is_none());
        assert!(w.manifest().stopped.is_some());
        // Over twice the cap the records stop.
        for i in 0..2000u64 {
            // Bytes that do not compress.
            let mut x = i.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
            let bytes: Vec<u8> = (0..400)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    x as u8
                })
                .collect();
            w.write(&e(
                i,
                Body::Net(NetRec {
                    player: 0,
                    bytes,
                    taken: true,
                }),
            ))
            .unwrap();
        }
        assert!(stats.dropped.load(std::sync::atomic::Ordering::Relaxed) > 0);
        w.finish(None).unwrap();
        let r = Recording::open(&dir).unwrap();
        let last = r.read_all().unwrap().pop().unwrap();
        assert!(last.entries.iter().any(|x| matches!(&x.body,
            Body::Worker(WorkerRec { ev: WorkerEv::Note(t) }) if t.contains("stops"))));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_nearly_full_disk_stops_the_recording_and_the_manifest_says_so() {
        let dir = scratch("free");
        let stats = Arc::new(Stats::default());
        let mut w =
            RecWriter::create(&dir, Config::default(), &header(), Arc::clone(&stats)).unwrap();
        // Plenty free: records go in.
        w.set_free_probe(|_| Some(10 * 1024 * 1024 * 1024));
        w.write(&journal(1)).unwrap();
        assert_eq!(stats.records.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert!(w.manifest().stopped.is_none());
        // The probe is asked every FREE_CHECK_EVERY only; ask again later (a test moves the clock by backdating).
        w.set_free_probe(|_| Some(100 * 1024 * 1024));
        w.tick(Instant::now() + FREE_CHECK_EVERY);
        let why = w.manifest().stopped.clone().expect("stopped");
        assert!(why.contains("100 MiB free"), "{why}");
        w.write(&journal(2)).unwrap();
        assert_eq!(stats.records.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert!(stats.dropped.load(std::sync::atomic::Ordering::Relaxed) > 0);
        w.finish(None).unwrap();
        let r = Recording::open(&dir).unwrap();
        assert!(r.manifest.stopped.is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn df_output_is_read_and_a_zero_minimum_turns_the_guard_off() {
        let df = "Filesystem 1024-blocks Used Available Capacity Mounted on
/dev/sda1 1000 400 600 40% /
";
        assert_eq!(parse_df(df), Some(600 * 1024));
        assert_eq!(parse_df(""), None);
        assert_eq!(
            parse_df(
                "x
y z
"
            ),
            None
        );
        let dir = scratch("free0");
        let cfg = Config {
            min_free_bytes: 0,
            ..Config::default()
        };
        let mut w = RecWriter::create(&dir, cfg, &header(), Arc::new(Stats::default())).unwrap();
        w.set_free_probe(|_| Some(0));
        w.tick(Instant::now() + FREE_CHECK_EVERY);
        assert!(w.manifest().stopped.is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
