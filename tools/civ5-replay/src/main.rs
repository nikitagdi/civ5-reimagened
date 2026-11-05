//! `civ5-replay`: reads and replays game recordings (packet P3).
//!
//! ```text
//! civ5-replay info   <rec>
//! civ5-replay log    <rec> [--turn T] [--player P] [--kinds order,result,...] [--json]
//! civ5-replay verify <rec> [--from T] [--to T] [--reload] [--dump DIR [--save-at S.E.N]] [--any-build]
//! civ5-replay step   <rec> --turn T [--until N] [--digest] [--dump N -o x.c5sv]
//! civ5-replay hang   <rec> [--timeout S]
//! civ5-replay diff   <rec> --other <civ5-replay of another core build>
//! civ5-replay export <rec> --turn T [--record N] -o x.c5sv
//! ```
//!
//! `<rec>` is a directory `recordings/g<game>-<ms>`. `verify`, `step`, `hang`, `diff` and `export --record` run the real
//! core (Linux; `GAMECORE_LIB_DIR_x86_64` at build time, `CIV5_GAMEDATA` or `--gamedata` at run time) and refuse a
//! recording written by another core build unless `--any-build` is given. `info`, `log` and `export --turn` read files
//! only.

mod describe;
mod engine;
mod store;
mod verify;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use engine::{Engine, Flow, Hooks, RecCtx, gamedata_dir, walk};
use recording::format::{Body, WorkerEv};
use store::Store;

#[derive(Parser)]
#[command(name = "civ5-replay", about = "Reads and replays game recordings")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(clap::Args, Clone)]
struct CoreArgs {
    /// The game data (default: CIV5_GAMEDATA, or assets-out/gamedata-b2 above the current directory).
    #[arg(long)]
    gamedata: Option<PathBuf>,
    /// Replay a recording of another core build (the result is only a report).
    #[arg(long)]
    any_build: bool,
}

#[derive(Subcommand)]
enum Cmd {
    /// The manifest, turns, sizes, keyframes, pins, faults and hangs.
    Info { rec: PathBuf },
    /// The records as lines.
    Log {
        rec: PathBuf,
        /// Only the records of this turn (the segment of its keyframe).
        #[arg(long)]
        turn: Option<i32>,
        /// Only the records of this seat.
        #[arg(long)]
        player: Option<i8>,
        /// Only these kinds: header, order, net, seat, journal, result, turn, keyframe, worker, resync, trace, report, save.
        #[arg(long, value_delimiter = ',')]
        kinds: Vec<String>,
        /// One JSON object per line.
        #[arg(long)]
        json: bool,
    },
    /// Replays the journal of every turn on the real core and compares.
    Verify {
        rec: PathBuf,
        #[arg(long)]
        from: Option<i32>,
        #[arg(long)]
        to: Option<i32>,
        /// Load the snapshot at every keyframe boundary (strict: the AI phase must follow from the snapshot and the
        /// recorded inputs alone). By default the replayed game goes on where it equals the snapshot by the core's save
        /// hash, as the server's worker does, and a snapshot is loaded only where a worker began or the game differs.
        #[arg(long)]
        reload: bool,
        /// Write both GCS1s of a boundary that differs into this directory.
        #[arg(long)]
        dump: Option<PathBuf>,
        /// With --dump: also write the core's save after the record `segment.entry.n` (as `digests` prints it).
        #[arg(long, requires = "dump", value_parser = parse_at)]
        save_at: Option<(usize, usize, usize)>,
        #[command(flatten)]
        core: CoreArgs,
    },
    /// One turn, one record at a time.
    Step {
        rec: PathBuf,
        #[arg(long)]
        turn: i32,
        /// Stop after this record of the turn (counted from 0).
        #[arg(long)]
        until: Option<u64>,
        /// Print the digest after every record (slow: one serialization of the game each).
        #[arg(long)]
        digest: bool,
        /// Write a save of the state after this record (needs a save point of the core).
        #[arg(long)]
        dump: Option<u64>,
        #[arg(short, long)]
        out: Option<PathBuf>,
        #[command(flatten)]
        core: CoreArgs,
    },
    /// Replays to the worker's hang or crash and runs the call that never returned, with a watchdog.
    Hang {
        rec: PathBuf,
        /// Seconds before the watchdog prints the stack of the core and ends the run.
        #[arg(long, default_value_t = 120)]
        timeout: u64,
        #[command(flatten)]
        core: CoreArgs,
    },
    /// Two core builds in lockstep: the first record where their digests differ.
    Diff {
        rec: PathBuf,
        /// The `civ5-replay` binary linked with the other core.
        #[arg(long)]
        other: PathBuf,
        /// Compare the digests after every N-th record.
        #[arg(long, default_value_t = 1)]
        every: u64,
        #[command(flatten)]
        core: CoreArgs,
    },
    /// The digest after every record, one per line (what `diff` compares).
    Digests {
        rec: PathBuf,
        #[arg(long, default_value_t = 1)]
        every: u64,
        /// Load the snapshot at every keyframe boundary (as erify --reload).
        #[arg(long)]
        reload: bool,
        #[command(flatten)]
        core: CoreArgs,
    },
    /// A save of the game at the start of a turn (or after a record), to play on.
    Export {
        rec: PathBuf,
        #[arg(long)]
        turn: i32,
        /// After this record of the turn (needs the core and a save point there).
        #[arg(long)]
        record: Option<u64>,
        #[arg(short, long)]
        out: PathBuf,
        #[command(flatten)]
        core: CoreArgs,
    },
    /// For tests: a copy of a recording with one command of a turn changed by a bit.
    Corrupt {
        rec: PathBuf,
        #[arg(long)]
        turn: i32,
        /// The n-th command record with arguments of the human journal of that turn.
        #[arg(long, default_value_t = 0)]
        nth: usize,
        /// Instead: cut the turn's segment after this entry and end it with a core call that never returned and a
        /// worker hang (to try `hang`).
        #[arg(long)]
        hang_after: Option<usize>,
        #[arg(short, long)]
        out: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli.cmd) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("civ5-replay: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn run(cmd: Cmd) -> Result<u8> {
    match cmd {
        Cmd::Info { rec } => info(&Store::open(&rec)?),
        Cmd::Log {
            rec,
            turn,
            player,
            kinds,
            json,
        } => log(&Store::open(&rec)?, turn, player, &kinds, json),
        Cmd::Verify {
            rec,
            from,
            to,
            reload,
            dump,
            save_at,
            core,
        } => {
            let store = Store::open(&rec)?;
            let start = verify::start_seg(&store, from)?;
            let mut eng = boot(&store, &core, Some(start))?;
            let opts = verify::Opts {
                reload,
                dump,
                save_at,
            };
            let r = verify::run(&store, &mut eng, from, to, opts)?;
            verify::print(&store, &r);
            // 1: a record or a check diverged; 3: the human phases are clean but an AI phase does not replay.
            Ok(if r.divergence.is_some() {
                1
            } else if r.ai_mismatches.is_empty() {
                0
            } else {
                3
            })
        }
        Cmd::Step {
            rec,
            turn,
            until,
            digest,
            dump,
            out,
            core,
        } => step(&rec, turn, until, digest, dump, out.as_deref(), &core),
        Cmd::Hang { rec, timeout, core } => hang(&rec, timeout, &core),
        Cmd::Diff {
            rec,
            other,
            every,
            core,
        } => diff(&rec, &other, every, &core),
        Cmd::Digests {
            rec,
            every,
            reload,
            core,
        } => digests(&rec, every, reload, &core),
        Cmd::Export {
            rec,
            turn,
            record,
            out,
            core,
        } => export(&rec, turn, record, &out, &core),
        Cmd::Corrupt {
            rec,
            turn,
            nth,
            hang_after,
            out,
        } => corrupt(&rec, turn, nth, hang_after, &out),
    }
}

/// `segment.entry.n`.
fn parse_at(s: &str) -> Result<(usize, usize, usize), String> {
    let v: Vec<usize> = s
        .split('.')
        .map(str::parse)
        .collect::<Result<_, _>>()
        .map_err(|e| format!("{s}: {e}"))?;
    match v[..] {
        [a, b, c] => Ok((a, b, c)),
        _ => Err(format!("{s}: expected segment.entry.n")),
    }
}

/// The core, with the build check against the recording. The build id is known once a game is loaded: the snapshot
/// of `start` (the segment the run begins with; default the first keyframe on disk) is loaded, so the run begins
/// with one load into a fresh core, as a server worker that starts from that snapshot (the walk does not load it
/// again). A second load into the same core is a resync, which is not always the same as a first load.
fn boot(store: &Store, core: &CoreArgs, start: Option<usize>) -> Result<Engine> {
    let dir = gamedata_dir(core.gamedata.as_deref())?;
    let mut eng = Engine::new(&dir)?;
    let first = match start {
        Some(s) => s,
        None => (0..store.segs.len())
            .find(|&i| store.keyframe_of(i).is_some() && store.raw_of(i).is_ok())
            .context("no keyframe snapshot is on disk")?,
    };
    eng.load(&store.raw_of(first)?)?;
    let have = eng.build_id()?;
    let (_, _, want) = store.game_meta();
    let want = if want.is_empty() {
        store.rec.manifest.core_build.clone()
    } else {
        want
    };
    if !want.is_empty() && want != have {
        if core.any_build {
            eprintln!(
                "civ5-replay: warning: the recording is of core build {want:?}, this core is {have:?}"
            );
        } else {
            bail!(
                "the recording is of core build {want:?}, this core is {have:?} (--any-build replays anyway and only reports)"
            );
        }
    }
    Ok(eng)
}

// ------------------------------------------------------------------ info

fn kind_name(b: &Body) -> &'static str {
    b.name()
}

fn info(store: &Store) -> Result<u8> {
    let m = &store.rec.manifest;
    println!(
        "recording of game {} {:?}: gateway {}, core {:?}, seed {}",
        m.game, m.name, m.gateway, m.core_build, m.seed
    );
    println!(
        "  created {} ms, {}",
        m.created_ms,
        m.finished_ms.map_or(
            "not closed (running, or the gateway died)".to_owned(),
            |t| format!("closed at {t} ms")
        )
    );
    if store.rec.rebuilt {
        println!("  (the manifest was missing: rebuilt from the segments)");
    }
    if let Some(s) = &m.stopped {
        println!("  STOPPED: {s}");
    }
    if let Some(p) = store.rec.pin() {
        println!("  pinned by report {} until {} ms", p.report, p.until_ms);
    }
    let seg_bytes: u64 = store.segs.iter().map(|s| s.file_bytes).sum();
    let raw_bytes: u64 = store.segs.iter().map(|s| s.raw_bytes).sum();
    let kf_bytes: u64 = m
        .keyframes
        .iter()
        .filter(|k| !k.removed)
        .map(|k| u64::from(k.bytes))
        .sum();
    println!(
        "  {} segments: {} bytes on disk, {} bytes of records; {} keyframes ({} bodies kept, {} bytes)",
        store.segs.len(),
        seg_bytes,
        raw_bytes,
        m.keyframes.len(),
        m.keyframes
            .iter()
            .filter(|k| !k.removed && k.file.is_some())
            .count(),
        kf_bytes
    );
    let mut kinds: BTreeMap<&str, u64> = BTreeMap::new();
    let mut faults = Vec::new();
    let mut open_call: Option<(usize, usize, String)> = None;
    for it in store.items() {
        *kinds.entry(kind_name(&it.entry.body)).or_default() += 1;
        if let Body::Worker(w) = &it.entry.body {
            match &w.ev {
                WorkerEv::CallBegin { what, .. } => {
                    open_call = Some((it.seg, it.idx, what.clone()))
                }
                WorkerEv::CallEnd { .. } => open_call = None,
                WorkerEv::Exit { .. } | WorkerEv::Fault { .. } | WorkerEv::Fatal { .. } => {
                    faults.push(format!(
                        "  segment {} entry {} (turn {}): {}",
                        it.seg,
                        it.idx,
                        it.turn,
                        describe::body_text(&it.entry.body)
                    ));
                    if let Some((s, i, what)) = open_call.take() {
                        faults.push(format!(
                            "    the core call begun at segment {s} entry {i} did not end: {what}"
                        ));
                    }
                }
                WorkerEv::Start { .. } | WorkerEv::Note(_) => {}
            }
        }
        if matches!(&it.entry.body, Body::Report(_)) {
            faults.push(format!(
                "  segment {} entry {} (turn {}): {}",
                it.seg,
                it.idx,
                it.turn,
                describe::body_text(&it.entry.body)
            ));
        }
    }
    println!(
        "  records: {}",
        kinds
            .iter()
            .map(|(k, n)| format!("{k} {n}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    println!("  turns (segment: records, bytes):");
    for (i, s) in store.segs.iter().enumerate() {
        let kf = store.keyframe_of(i).map_or_else(
            || "before the first keyframe".to_owned(),
            |k| {
                format!(
                    "{:?}{} {} bytes{}",
                    k.kind,
                    if k.base { " base" } else { "" },
                    k.bytes,
                    if k.file.is_none() {
                        ", body not kept"
                    } else {
                        ""
                    }
                )
            },
        );
        println!(
            "    {i:>4} turn {:>4}: {:>5} records, {:>8} bytes; keyframe {kf}",
            s.header.turn,
            s.entries.len(),
            s.file_bytes
        );
    }
    let removed = m.keyframes.iter().filter(|k| k.removed).count();
    if removed > 0 {
        println!(
            "  {removed} keyframe bodies were compacted away (older than the detail window); their turns replay from the previous full snapshot"
        );
    }
    if faults.is_empty() {
        println!("  no worker exit, hang or fault");
    } else {
        println!("  worker events and reports:");
        for f in faults {
            println!("{f}");
        }
    }
    for d in store.damage() {
        println!("  torn: {d}");
    }
    Ok(0)
}

// ------------------------------------------------------------------ log

fn log(
    store: &Store,
    turn: Option<i32>,
    player: Option<i8>,
    kinds: &[String],
    json: bool,
) -> Result<u8> {
    for it in store.items() {
        if turn.is_some_and(|t| it.turn != t) {
            continue;
        }
        let b = &it.entry.body;
        if !kinds.is_empty() && !kinds.iter().any(|k| k == b.name()) {
            continue;
        }
        if let Some(p) = player
            && describe::player_of(b).is_some_and(|q| q != p)
        {
            continue;
        }
        if player.is_some() && describe::player_of(b).is_none() {
            continue;
        }
        if json {
            let v = serde_json::json!({
                "seg": it.seg, "entry": it.idx, "turn": it.turn,
                "mono_us": it.entry.mono_us, "wall_ms": it.entry.wall_ms,
                "kind": b.name(), "text": describe::body_text(b),
            });
            println!("{v}");
        } else {
            println!(
                "{:>4}/{:<5} t{:<4} {} {}",
                it.seg,
                it.idx,
                it.turn,
                describe::at(&it.entry),
                describe::body_text(b)
            );
        }
    }
    Ok(0)
}

// ------------------------------------------------------------------ step

struct Stepper<'a> {
    until: Option<u64>,
    digest: bool,
    dump: Option<u64>,
    out: Option<&'a Path>,
    store: &'a Store,
    dumped: bool,
}

impl Hooks for Stepper<'_> {
    fn on_record(
        &mut self,
        eng: &mut Engine,
        c: &RecCtx<'_>,
        applied: &gamecore::Applied,
    ) -> Result<Flow> {
        let mut line = format!(
            "#{:<5} entry {:<4}.{:<2} {:?} jseq {:<4} {}",
            c.seq,
            c.entry,
            c.n,
            c.journal.phase,
            c.journal.jseq,
            describe::journal_record(c.rec)
        );
        if let gamecore::Applied::Diverged(why) = applied {
            line.push_str(&format!("  DIVERGED: {why}"));
            println!("{line}");
            return Ok(Flow::Stop);
        }
        if self.digest {
            line.push_str(&format!("  digest {:016x}", eng.digest()?));
        }
        println!("{line}");
        if self.dump == Some(c.seq) {
            let Some(gcs1) = eng.save() else {
                bail!(
                    "the core is not at a save point after record #{}: pick a record at the end of a pump",
                    c.seq
                );
            };
            let out = self.out.context("--dump needs -o <file>")?;
            write_save(self.store, c.turn, &gcs1, out)?;
            self.dumped = true;
            println!(
                "wrote {} ({} bytes of core state)",
                out.display(),
                gcs1.len()
            );
            return Ok(Flow::Stop);
        }
        if self.until == Some(c.seq) {
            return Ok(Flow::Stop);
        }
        Ok(Flow::Go)
    }

    fn on_other(&mut self, _seg: usize, _entry: usize, e: &recording::Entry) -> Result<Flow> {
        if matches!(
            e.body,
            Body::Order(_) | Body::Net(_) | Body::Result(_) | Body::Seat(_)
        ) {
            println!(
                "        {} {}",
                describe::at(e),
                describe::body_text(&e.body)
            );
        }
        Ok(Flow::Go)
    }
}

fn step(
    rec: &Path,
    turn: i32,
    until: Option<u64>,
    digest: bool,
    dump: Option<u64>,
    out: Option<&Path>,
    core: &CoreArgs,
) -> Result<u8> {
    let store = Store::open(rec)?;
    let seg = store
        .seg_of_turn(turn)
        .with_context(|| format!("no keyframe for turn {turn}"))?;
    let mut eng = boot(&store, core, Some(seg))?;
    let mut h = Stepper {
        until,
        digest,
        dump,
        out,
        store: &store,
        dumped: false,
    };
    walk(&store, &mut eng, seg, seg, &mut h)?;
    if dump.is_some() && !h.dumped {
        bail!("the record to dump was not reached");
    }
    Ok(0)
}

// ------------------------------------------------------------------ digests / diff

struct Digester {
    every: u64,
    reload: bool,
}

impl Hooks for Digester {
    fn on_record(
        &mut self,
        eng: &mut Engine,
        c: &RecCtx<'_>,
        applied: &gamecore::Applied,
    ) -> Result<Flow> {
        if let gamecore::Applied::Diverged(why) = applied {
            println!(
                "{} {}.{} DIVERGED {}",
                c.seg,
                c.entry,
                c.n,
                why.replace('\n', " ")
            );
            return Ok(Flow::Stop);
        }
        if c.seq.is_multiple_of(self.every.max(1)) {
            println!("{} {}.{} {:016x}", c.seg, c.entry, c.n, eng.digest()?);
        }
        Ok(Flow::Go)
    }

    fn on_boundary(
        &mut self,
        eng: &mut Engine,
        _done: usize,
        next: &recording::format::KeyframeRec,
        raw: Option<Vec<u8>>,
    ) -> Result<Flow> {
        if !self.reload
            && eng
                .save()
                .is_some_and(|s| protocol::xxh64(0, &s) == next.raw_hash)
        {
            return Ok(Flow::Go);
        }
        if let Some(raw) = raw {
            eng.load(&raw)?;
        }
        Ok(Flow::Go)
    }
}

fn digests(rec: &Path, every: u64, reload: bool, core: &CoreArgs) -> Result<u8> {
    let store = Store::open(rec)?;
    let mut eng = boot(&store, core, None)?;
    let first = (0..store.segs.len())
        .find(|&i| store.keyframe_of(i).is_some() && store.raw_of(i).is_ok())
        .context("no keyframe snapshot is on disk")?;
    let mut h = Digester { every, reload };
    walk(&store, &mut eng, first, store.segs.len() - 1, &mut h)?;
    Ok(0)
}

// SIMPLIFIED(SIMP-exe-814): one process per core build, their digest lines compared; not one process in lockstep.
fn diff(rec: &Path, other: &Path, every: u64, core: &CoreArgs) -> Result<u8> {
    let me = std::env::current_exe()?;
    let run = |exe: &Path| -> Result<Vec<String>> {
        let mut c = std::process::Command::new(exe);
        c.arg("digests")
            .arg(rec)
            .arg("--every")
            .arg(every.to_string());
        if let Some(g) = &core.gamedata {
            c.arg("--gamedata").arg(g);
        }
        if core.any_build {
            c.arg("--any-build");
        }
        let o = c
            .output()
            .with_context(|| format!("cannot run {}", exe.display()))?;
        if !o.status.success() {
            bail!(
                "{} failed: {}",
                exe.display(),
                String::from_utf8_lossy(&o.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(str::to_owned)
            .collect())
    };
    let (a, b) = (run(&me)?, run(other)?);
    let store = Store::open(rec)?;
    for (i, (x, y)) in a.iter().zip(&b).enumerate() {
        if x != y {
            println!("the two cores part at line {i}: A {x:?}, B {y:?}");
            let mut it = x.split_whitespace();
            if let (Some(seg), Some(at)) = (it.next(), it.next())
                && let (Ok(seg), Some((entry, n))) = (seg.parse::<usize>(), at.split_once('.'))
                && let (Ok(entry), Ok(n)) = (entry.parse::<usize>(), n.parse::<usize>())
                && let Some(e) = store.segs.get(seg).and_then(|s| s.entries.get(entry))
                && let Body::Journal(j) = &e.body
                && let Ok(recs) = gamecore::replica::decode_journal(&j.bytes)
                && let Some(r) = recs.get(n)
            {
                println!(
                    "  turn {} segment {seg} entry {entry} record {n} ({:?} jseq {}): {}",
                    store.segs[seg].header.turn,
                    j.phase,
                    j.jseq,
                    describe::journal_record(r)
                );
            }
            return Ok(1);
        }
    }
    if a.len() != b.len() {
        println!(
            "the streams have {} and {} lines: one core stopped early",
            a.len(),
            b.len()
        );
        return Ok(1);
    }
    println!("the two cores agree on {} digests", a.len());
    Ok(0)
}

// ------------------------------------------------------------------ hang

struct Quiet {
    stop_at: Option<(usize, usize)>,
    applied: u64,
}

impl Hooks for Quiet {
    fn on_record(
        &mut self,
        _eng: &mut Engine,
        c: &RecCtx<'_>,
        applied: &gamecore::Applied,
    ) -> Result<Flow> {
        if let gamecore::Applied::Diverged(why) = applied {
            bail!(
                "the replay diverged before the hang, at segment {} entry {}: {why}",
                c.seg,
                c.entry
            );
        }
        self.applied += 1;
        Ok(Flow::Go)
    }

    fn on_other(&mut self, seg: usize, entry: usize, e: &recording::Entry) -> Result<Flow> {
        if self.stop_at == Some((seg, entry)) {
            let _ = e;
            return Ok(Flow::Stop);
        }
        Ok(Flow::Go)
    }
}

fn hang(rec: &Path, timeout: u64, core: &CoreArgs) -> Result<u8> {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};
    let store = Store::open(rec)?;
    // The last worker exit that was a hang, else the last exit, fault or fatal.
    let mut event: Option<(usize, usize, bool)> = None;
    let mut call: Option<(i32, String)> = None;
    let mut call_at_event: Option<(i32, String)> = None;
    for it in store.items() {
        if let Body::Worker(w) = &it.entry.body {
            match &w.ev {
                WorkerEv::CallBegin { turn, what } => call = Some((*turn, what.clone())),
                WorkerEv::CallEnd { .. } => call = None,
                WorkerEv::Exit { hang, .. } if *hang || event.is_none_or(|(_, _, h)| !h) => {
                    event = Some((it.seg, it.idx, *hang));
                    call_at_event = call.take();
                }
                WorkerEv::Fault { .. } | WorkerEv::Fatal { .. }
                    if event.is_none_or(|(_, _, h)| !h) =>
                {
                    event = Some((it.seg, it.idx, false));
                    call_at_event = call.take();
                }
                _ => {}
            }
        }
    }
    let (seg, entry, is_hang) =
        event.context("the recording holds no worker exit, hang or fault")?;
    println!(
        "{} at segment {seg} entry {entry} (turn {}): {}",
        if is_hang { "hang" } else { "worker end" },
        store.segs[seg].header.turn,
        describe::body_text(&store.segs[seg].entries[entry].body)
    );
    match &call_at_event {
        Some((t, what)) => println!("the core call that never returned (turn {t}): {what}"),
        None => println!(
            "no core call was marked open (the recording was made without marks, or the worker was between calls)"
        ),
    }
    // The segment with the last keyframe at or before the event whose snapshot is readable.
    let first = (0..=seg)
        .rev()
        .find(|&i| store.keyframe_of(i).is_some() && store.raw_of(i).is_ok())
        .context("no readable keyframe before the event")?;
    let mut eng = boot(&store, core, Some(first))?;
    let mut q = Quiet {
        stop_at: Some((seg, entry)),
        applied: 0,
    };
    let t0 = Instant::now();
    walk(&store, &mut eng, first, seg, &mut q)?;
    println!(
        "replayed {} records from the keyframe of turn {} in {:?}; the core is where the worker was",
        q.applied,
        store.segs[first].header.turn,
        t0.elapsed()
    );
    // Now as the server: run the call.
    eng.game
        .game()
        .set_role(gamecore::Role::Server)
        .map_err(|e| anyhow::anyhow!("cannot make the core the server: {e}"))?;
    let started = Arc::new(AtomicU64::new(0));
    let beat = Arc::clone(&started);
    let pid = std::process::id();
    let begun = Instant::now();
    std::thread::spawn(move || {
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let at = beat.load(Ordering::Relaxed);
            if at != 0 && begun.elapsed().as_secs().saturating_sub(at) >= timeout {
                eprintln!(
                    "the core call has run for {timeout} s: the stack of the core threads follows"
                );
                let gdb = std::process::Command::new("gdb")
                    .args([
                        "-batch",
                        "-p",
                        &pid.to_string(),
                        "-ex",
                        "thread apply all bt",
                    ])
                    .output();
                match gdb {
                    Ok(o) => eprintln!("{}", String::from_utf8_lossy(&o.stdout)),
                    Err(e) => {
                        eprintln!("gdb is not available ({e}); /proc/{pid}/task/*/wchan:");
                        if let Ok(rd) = std::fs::read_dir(format!("/proc/{pid}/task")) {
                            for t in rd.flatten() {
                                let w = std::fs::read_to_string(t.path().join("wchan"))
                                    .unwrap_or_default();
                                eprintln!("  {} {w}", t.file_name().to_string_lossy());
                            }
                        }
                    }
                }
                std::process::exit(3);
            }
        }
    });
    let mut calls = 0u32;
    let mut slices = 0u64;
    let t_call = Instant::now();
    loop {
        started.store(begun.elapsed().as_secs().max(1), Ordering::Relaxed);
        let t = Instant::now();
        let r = eng.game.game().pump(4);
        started.store(0, Ordering::Relaxed);
        let r = r.map_err(|e| anyhow::anyhow!("pump failed after {calls} calls: {e}"))?;
        calls += 1;
        slices += 4;
        if calls.is_multiple_of(25) || t.elapsed() > Duration::from_secs(2) {
            println!(
                "pump #{calls}: {:?} in {:?} (status {}, turn {})",
                r,
                t.elapsed(),
                r.status,
                r.turn
            );
        }
        if r.status != gamecore_sys::ffi::GC_PUMP_AI_RUNNING {
            println!(
                "the call returned after {calls} pumps in {:?}: status {} turn {}; the hang did not reproduce on this machine",
                t_call.elapsed(),
                r.status,
                r.turn
            );
            return Ok(0);
        }
        if slices > 1_000_000 {
            bail!("a million slices and the AI phase is not over");
        }
    }
}

// ------------------------------------------------------------------ export

fn write_save(store: &Store, turn: i32, gcs1: &[u8], out: &Path) -> Result<()> {
    let (settings, seats, build) = store.game_meta();
    let settings = settings
        .context("the recording does not carry the game settings (an old or cut recording)")?;
    let header = game_host::saves::SaveHeader {
        seq: u64::try_from(turn).unwrap_or(0),
        game: store.rec.manifest.game,
        turn,
        saved_at_ms: store.rec.manifest.created_ms,
        kind: protocol::SaveKind::Ring,
        build_id: if build.is_empty() {
            store.rec.manifest.core_build.clone()
        } else {
            build
        },
    };
    let sections = game_host::saves::SaveSections {
        seats,
        settings,
        journals: Vec::new(),
        cmds: Vec::new(),
        city_memo: Vec::new(),
    };
    let bytes = game_host::saves::encode(&header, gcs1, &sections);
    std::fs::write(out, bytes).with_context(|| out.display().to_string())?;
    Ok(())
}

struct ToRecord {
    until: u64,
    out: Option<Vec<u8>>,
}

impl Hooks for ToRecord {
    fn on_record(
        &mut self,
        eng: &mut Engine,
        c: &RecCtx<'_>,
        applied: &gamecore::Applied,
    ) -> Result<Flow> {
        if let gamecore::Applied::Diverged(why) = applied {
            bail!("the replay diverged at record #{}: {why}", c.seq);
        }
        if c.seq == self.until {
            self.out = Some(
                eng.save()
                    .context("the core is not at a save point after that record")?,
            );
            return Ok(Flow::Stop);
        }
        Ok(Flow::Go)
    }
}

fn export(rec: &Path, turn: i32, record: Option<u64>, out: &Path, core: &CoreArgs) -> Result<u8> {
    let store = Store::open(rec)?;
    let seg = store
        .seg_of_turn(turn)
        .with_context(|| format!("no keyframe for turn {turn}"))?;
    let gcs1 = match record {
        None => store.raw_of(seg)?,
        Some(n) => {
            let mut eng = boot(&store, core, Some(seg))?;
            let mut h = ToRecord {
                until: n,
                out: None,
            };
            walk(&store, &mut eng, seg, seg, &mut h)?;
            h.out
                .with_context(|| format!("the turn has no record #{n}"))?
        }
    };
    write_save(&store, turn, &gcs1, out)?;
    println!(
        "wrote {} ({} bytes of core state{})",
        out.display(),
        gcs1.len(),
        record.map_or(String::new(), |n| format!(
            ", after record #{n} of turn {turn}"
        ))
    );
    Ok(0)
}

// ------------------------------------------------------------------ corrupt

fn corrupt(rec: &Path, turn: i32, nth: usize, hang_after: Option<usize>, out: &Path) -> Result<u8> {
    use gamecore::replica::{JournalRecord, decode_journal, encode_journal};
    use recording::writer::{Config, RecWriter, Stats};
    let store = Store::open(rec)?;
    let seg = store
        .seg_of_turn(turn)
        .with_context(|| format!("no keyframe for turn {turn}"))?;
    // Find the target: the nth command with arguments of the human journal of the turn.
    let mut found = 0usize;
    let mut target: Option<(usize, usize, usize)> = None;
    'find: for (idx, e) in store.segs[seg].entries.iter().enumerate() {
        if let Body::Journal(j) = &e.body
            && j.phase == protocol::RecPhase::Human
            && let Ok(recs) = decode_journal(&j.bytes)
        {
            for (n, r) in recs.iter().enumerate() {
                if matches!(r, JournalRecord::Command { args, .. } if !args.is_empty()) {
                    if found == nth {
                        target = Some((seg, idx, n));
                        break 'find;
                    }
                    found += 1;
                }
            }
        }
    }
    let (tseg, tidx, tn) = match (target, hang_after) {
        (Some(t), None) => t,
        (_, Some(_)) => (usize::MAX, 0, 0),
        (None, None) => bail!("no such command in the human journal of that turn"),
    };
    let m = &store.rec.manifest;
    let (settings, seats, build) = store.game_meta();
    let header = recording::format::HeaderRec {
        format: recording::format::VERSION,
        game: m.game,
        gateway: m.gateway.clone(),
        core_build: if build.is_empty() {
            m.core_build.clone()
        } else {
            build
        },
        seed: m.seed,
        wall_at_mono0_ms: m.created_ms,
        created_ms: m.created_ms,
        name: m.name.clone(),
        settings,
        seats,
    };
    let cfg = Config {
        detail_turns: u32::MAX / 2,
        max_game_bytes: u64::MAX / 4,
        ..Config::default()
    };
    let mut w = RecWriter::create(out, cfg, &header, std::sync::Arc::new(Stats::default()))?;
    for (si, s) in store.segs.iter().enumerate() {
        for (ei, e) in s.entries.iter().enumerate() {
            let mut e = e.clone();
            let at = describe::at(&e);
            if (si, ei) == (tseg, tidx)
                && let Body::Journal(j) = &mut e.body
            {
                let mut recs = decode_journal(&j.bytes).map_err(|x| anyhow::anyhow!("{x}"))?;
                if let JournalRecord::Command { args, .. } = &mut recs[tn]
                    && let Some(last) = args.last_mut()
                {
                    let before = *last;
                    *last ^= 1;
                    println!(
                        "changed the last argument of record {tn} of the batch at segment {tseg} entry {tidx} (turn {}, jseq {}, recorded {}): {before} -> {}",
                        s.header.turn, j.jseq, at, *last
                    );
                }
                j.bytes = encode_journal(&recs);
            }
            if let Body::Keyframe(k) = &e.body {
                let body = k
                    .file
                    .as_deref()
                    .and_then(|f| store.rec.keyframe_body(f, k.hash).ok());
                w.keyframe(e.clone(), body.as_deref())?;
            } else {
                w.write(&e)?;
            }
            if si == seg && hang_after == Some(ei) {
                let mk = |ev| recording::Entry {
                    mono_us: e.mono_us + 1,
                    wall_ms: e.wall_ms,
                    body: Body::Worker(recording::format::WorkerRec { ev }),
                };
                let what = "a step in the AI phase (injected by civ5-replay corrupt)".to_owned();
                w.write(&mk(WorkerEv::CallBegin {
                    turn: s.header.turn,
                    what,
                }))?;
                w.write(&mk(WorkerEv::Exit {
                    hang: true,
                    failure: Some("Hang".into()),
                    code: None,
                    killed: true,
                    lost: Vec::new(),
                }))?;
                println!("ended segment {si} after entry {ei} with a core call and a hang");
                break;
            }
        }
    }
    w.finish(m.finished_ms)?;
    println!("wrote {}", out.display());
    Ok(0)
}
