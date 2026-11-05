//! The core that replays a recording (one per process: the core is a singleton) and the walk over the records.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use gamecore::replica::{JournalRecord, REPLICA_DIGEST, decode_journal, encode_journal};
use gamecore::{Applied, GameEvent, HostServices, ReplicaGame};
use recording::format::{Body, JournalRec, KeyframeRec};

use crate::store::Store;

/// Game files from a directory (`CIV5_GAMEDATA`, the export of the game data).
struct DirHost(PathBuf);

impl HostServices for DirHost {
    fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        if path.split(['/', '\\']).any(|p| p == "..") {
            return None;
        }
        std::fs::read(self.0.join(path)).ok()
    }

    fn game_event(&self, _ev: GameEvent) {}
}

/// Where the game data is: `--gamedata`, `CIV5_GAMEDATA`, or `assets-out/gamedata-b2` above the binary or the
/// current directory.
pub fn gamedata_dir(arg: Option<&Path>) -> Result<PathBuf> {
    if let Some(d) = arg {
        return Ok(d.to_path_buf());
    }
    if let Some(d) = std::env::var_os("CIV5_GAMEDATA") {
        return Ok(PathBuf::from(d));
    }
    let start = std::env::current_dir().unwrap_or_default();
    for a in start.ancestors() {
        for n in ["gamedata-b2", "gamedata-b1"] {
            let d = a.join("assets-out").join(n);
            if d.join("vfs-index.json").is_file() {
                return Ok(d);
            }
        }
    }
    bail!("the game data is not found: pass --gamedata <dir> or set CIV5_GAMEDATA")
}

pub struct Engine {
    pub game: ReplicaGame,
    /// xxh64 of the snapshot loaded last, until a record runs.
    loaded: Option<u64>,
}

impl Engine {
    /// Boots the core. Needs the real gamecore library (Linux).
    pub fn new(gamedata: &Path) -> Result<Engine> {
        if gamecore::IMPLEMENTATION != "lekmod" {
            bail!(
                "this build of civ5-replay links the stub gamecore ({}): build it on Linux with \
                 GAMECORE_LIB_DIR_x86_64 set (the same library as the server)",
                gamecore::IMPLEMENTATION
            );
        }
        let game = ReplicaGame::new(DirHost(gamedata.to_path_buf()), None)
            .map_err(|e| anyhow!("the core does not start: {e}"))?;
        Ok(Engine { game, loaded: None })
    }

    /// Loads (or replaces in place) the game with a GCS1 snapshot.
    pub fn load(&mut self, gcs1: &[u8]) -> Result<()> {
        self.game
            .resync(gcs1)
            .map_err(|e| anyhow!("the core refuses the snapshot: {e}"))?;
        self.loaded = Some(protocol::xxh64(0, gcs1));
        Ok(())
    }

    /// Loads the snapshot unless it is the one the core loaded last and nothing ran since (the first load of a run
    /// is then the only one, as in a worker that starts from the snapshot).
    pub fn load_once(&mut self, gcs1: &[u8]) -> Result<()> {
        if self.loaded == Some(protocol::xxh64(0, gcs1)) {
            return Ok(());
        }
        self.load(gcs1)
    }

    /// The core build the loaded game reports.
    pub fn build_id(&mut self) -> Result<String> {
        self.game
            .game()
            .build_id()
            .map_err(|e| anyhow!("build id: {e}"))
    }

    /// `gc_state_digest(-3)`, what the server and its replicas compare.
    pub fn digest(&mut self) -> Result<u64> {
        self.game
            .game()
            .state_digest(REPLICA_DIGEST)
            .map_err(|e| anyhow!("digest: {e}"))
    }

    /// The core's save of the game now (a GCS1), if the core is at a save point.
    pub fn save(&mut self) -> Option<Vec<u8>> {
        self.game.game().save().ok()
    }

    /// Replays one record.
    pub fn apply(&mut self, rec: &JournalRecord) -> Result<Applied> {
        self.loaded = None;
        let bytes = encode_journal(std::slice::from_ref(rec));
        self.game
            .apply(&bytes)
            .map_err(|e| anyhow!("the core failed on a record: {e}"))
    }
}

/// Where a record is.
pub struct RecCtx<'a> {
    pub seg: usize,
    /// The turn of the segment.
    pub turn: i32,
    /// Index of the entry in the segment.
    pub entry: usize,
    /// Index of the record inside the batch.
    pub n: usize,
    pub journal: &'a JournalRec,
    pub rec: &'a JournalRecord,
    /// Journal records replayed so far in this run, from 0.
    pub seq: u64,
}

pub enum Flow {
    Go,
    Stop,
}

/// What the walk tells its user. The defaults do what a plain replay needs.
pub trait Hooks {
    /// The run began at a keyframe (`reset`: a worker restart began a new chain in the middle of the run).
    fn on_load(
        &mut self,
        _eng: &mut Engine,
        _seg: usize,
        _k: &KeyframeRec,
        _reset: bool,
    ) -> Result<Flow> {
        Ok(Flow::Go)
    }

    /// A record was replayed.
    fn on_record(&mut self, eng: &mut Engine, c: &RecCtx<'_>, applied: &Applied) -> Result<Flow>;

    /// Any other entry (an order, a result, a worker event ...).
    fn on_other(&mut self, _seg: usize, _entry: usize, _e: &recording::Entry) -> Result<Flow> {
        Ok(Flow::Go)
    }

    /// The segment `done` ended and the next one opens with `next`. `raw` is the snapshot of `next` when its body is on
    /// disk. Load it here (or not: the state goes on as it is).
    fn on_boundary(
        &mut self,
        eng: &mut Engine,
        _done: usize,
        _next: &KeyframeRec,
        raw: Option<Vec<u8>>,
    ) -> Result<Flow> {
        if let Some(raw) = raw {
            eng.load(&raw)?;
        }
        Ok(Flow::Go)
    }
}

/// Replays segments `first..=last` from the keyframe of `first`.
pub fn walk(
    store: &Store,
    eng: &mut Engine,
    first: usize,
    last: usize,
    h: &mut dyn Hooks,
) -> Result<()> {
    let mut seq = 0u64;
    let mut prev_epoch = None;
    for seg in first..=last.min(store.segs.len().saturating_sub(1)) {
        let s = &store.segs[seg];
        let kf = store.keyframe_of(seg);
        if seg == first {
            let k = kf.with_context(|| format!("segment {seg} has no keyframe to start from"))?;
            let raw = store
                .raw_of(seg)
                .with_context(|| format!("turn {}", k.turn))?;
            eng.load_once(&raw)?;
            if matches!(h.on_load(eng, seg, k, false)?, Flow::Stop) {
                return Ok(());
            }
        } else if let Some(k) = kf {
            let reset = k.base || prev_epoch.is_some_and(|e| e != k.epoch);
            let raw = store.raw_of(seg).ok();
            if reset {
                let raw = raw.with_context(|| {
                    format!(
                        "turn {}: a new worker began and its snapshot is missing",
                        k.turn
                    )
                })?;
                eng.load(&raw)?;
                if matches!(h.on_load(eng, seg, k, true)?, Flow::Stop) {
                    return Ok(());
                }
            } else if matches!(h.on_boundary(eng, seg - 1, k, raw)?, Flow::Stop) {
                return Ok(());
            }
        }
        if let Some(k) = kf {
            prev_epoch = Some(k.epoch);
        }
        for (idx, e) in s.entries.iter().enumerate() {
            let Body::Journal(j) = &e.body else {
                if matches!(h.on_other(seg, idx, e)?, Flow::Stop) {
                    return Ok(());
                }
                continue;
            };
            let recs = match decode_journal(&j.bytes) {
                Ok(r) => r,
                Err(err) => bail!(
                    "segment {seg} entry {idx} (turn {}, {:?} jseq {}): the journal batch does not decode: {err}",
                    s.header.turn,
                    j.phase,
                    j.jseq
                ),
            };
            for (n, rec) in recs.iter().enumerate() {
                let applied = eng.apply(rec)?;
                let c = RecCtx {
                    seg,
                    turn: s.header.turn,
                    entry: idx,
                    n,
                    journal: j,
                    rec,
                    seq,
                };
                seq += 1;
                if matches!(h.on_record(eng, &c, &applied)?, Flow::Stop) {
                    return Ok(());
                }
            }
        }
    }
    Ok(())
}
