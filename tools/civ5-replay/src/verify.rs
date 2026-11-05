//! SIMPLIFIED(SIMP-exe-816): the digest is compared at the server's checks and at the keyframes, not after every record.
//!
//! `verify`: replays the journal of every turn on the real core and compares with what the server recorded. The answer
//! is the first record where the replay leaves the server's game, or "clean".
//!
//! SIMPLIFIED(SIMP-exe-830): by default the replay is one process per worker, as the server: it loads the snapshot where
//! a worker began and goes on with its own game at every later keyframe that equals the server's by the core's save
//! hash. The core keeps state that no save holds (the Firaxis DLL serializes neither every AI cache nor every list), so a
//! turn replayed from its snapshot alone can take other AI decisions than the worker that played on (live game 936,
//! 2026-10-31: the military AI's unit counts, now saved by the fork; a homeland patrol move at turn 131 and 14 plot
//! bytes at turn 133 that are not found yet). `--reload` loads every snapshot and reports such turns.

use std::collections::VecDeque;

use anyhow::Result;
use gamecore::Applied;
use gamecore::replica::JournalRecord;
use recording::format::{Body, JournalRec, KeyframeRec, Verdict};

use crate::describe;
use crate::engine::{Engine, Flow, Hooks, RecCtx, walk};
use crate::store::Store;

/// Where and how the replay left the server's game.
#[derive(Debug)]
pub struct Divergence {
    pub turn: i32,
    pub seg: usize,
    pub entry: usize,
    /// The record inside the batch; `None`: at a keyframe boundary.
    pub n: Option<usize>,
    pub what: String,
    /// The orders and messages recorded just before, newest last.
    pub before: Vec<String>,
}

#[derive(Debug, Default)]
pub struct Report {
    pub first_turn: Option<i32>,
    pub last_turn: Option<i32>,
    pub segments: usize,
    pub records: u64,
    pub checks: u64,
    /// Keyframe boundaries where the replayed state equalled the next snapshot (digest, and the core's save hash when the
    /// core could save there).
    pub boundaries_ok: u64,
    pub save_hashes_ok: u64,
    /// Boundaries not compared (the body was compacted away, or the chain began again).
    pub boundaries_skipped: u64,
    pub warnings: Vec<String>,
    /// Turns whose AI phase and turn change, replayed from the journal, end in another state than the server's next
    /// snapshot, although every check of the human phase passed.
    pub ai_mismatches: Vec<String>,
    pub divergence: Option<Divergence>,
}

/// How `verify` runs.
#[derive(Debug, Default, Clone)]
pub struct Opts {
    /// Load the snapshot at every keyframe boundary. Without it the replayed game goes on at a boundary where it equals
    /// the snapshot by the core's save hash (the server's worker does not load it either).
    pub reload: bool,
    /// Write the replayed save and the snapshot of a boundary that differs into this directory.
    pub dump: Option<std::path::PathBuf>,
    /// Also write the core's save after this record (`segment.entry.n`, as `digests` prints it) into `dump`.
    pub save_at: Option<(usize, usize, usize)>,
}

struct V<'a> {
    store: &'a Store,
    opts: Opts,
    r: Report,
    /// The last orders, UI messages and journal records, as lines.
    recent: VecDeque<String>,
    /// The `(epoch, jseq)` of the last human batch.
    last_jseq: Option<(u32, u64)>,
}

const RECENT: usize = 10;

impl V<'_> {
    fn note(&mut self, line: String) {
        if self.recent.len() == RECENT {
            self.recent.pop_front();
        }
        self.recent.push_back(line);
    }

    fn diverge(&mut self, seg: usize, entry: usize, n: Option<usize>, what: String) -> Flow {
        let turn = self.store.segs[seg].header.turn;
        self.r.divergence = Some(Divergence {
            turn,
            seg,
            entry,
            n,
            what,
            before: self.recent.iter().cloned().collect(),
        });
        Flow::Stop
    }

    fn jseq_check(&mut self, j: &JournalRec, seg: usize, entry: usize) {
        if j.phase != protocol::RecPhase::Human {
            return;
        }
        if let Some((epoch, last)) = self.last_jseq
            && epoch == j.epoch
            && j.jseq != last + 1
        {
            self.r.warnings.push(format!(
                "segment {seg} entry {entry}: batch {} follows batch {last} (epoch {epoch}): a batch is missing",
                j.jseq
            ));
        }
        self.last_jseq = Some((j.epoch, j.jseq));
    }
}

impl Hooks for V<'_> {
    fn on_load(
        &mut self,
        _eng: &mut Engine,
        seg: usize,
        k: &KeyframeRec,
        _reset: bool,
    ) -> Result<Flow> {
        self.r.first_turn.get_or_insert(k.turn);
        self.last_jseq = None;
        let _ = seg;
        Ok(Flow::Go)
    }

    fn on_other(&mut self, _seg: usize, _entry: usize, e: &recording::Entry) -> Result<Flow> {
        match &e.body {
            Body::Order(o) => {
                let tag = match &o.verdict {
                    Verdict::Forwarded => "",
                    Verdict::Refused { .. } => " (refused at the gateway)",
                    _ => " (not forwarded)",
                };
                self.note(format!(
                    "{} order seat {} id {}: {:?}{tag}",
                    describe::at(e),
                    o.player,
                    o.cmd.id,
                    o.cmd.cmd
                ));
            }
            Body::Net(n) => self.note(format!(
                "{} net message of seat {} ({} bytes{})",
                describe::at(e),
                n.player,
                n.bytes.len(),
                if n.taken { "" } else { ", dropped" }
            )),
            Body::Seat(s) => {
                self.note(format!("{} seat {} {}", describe::at(e), s.player, s.event))
            }
            _ => {}
        }
        Ok(Flow::Go)
    }

    fn on_record(&mut self, eng: &mut Engine, c: &RecCtx<'_>, applied: &Applied) -> Result<Flow> {
        if c.n == 0 {
            self.jseq_check(c.journal, c.seg, c.entry);
        }
        self.r.records += 1;
        self.r.last_turn = Some(c.turn);
        if self.opts.save_at == Some((c.seg, c.entry, c.n))
            && let Some(dir) = &self.opts.dump
        {
            let Some(s) = eng.save() else {
                anyhow::bail!("--save-at: the core is not at a save point after that record");
            };
            std::fs::create_dir_all(dir)?;
            std::fs::write(dir.join(format!("s{}-{}-{}.gcs1", c.seg, c.entry, c.n)), s)?;
        }
        let line = format!(
            "{:?} jseq {}: {}",
            c.journal.phase,
            c.journal.jseq,
            describe::journal_record(c.rec)
        );
        if let Applied::Diverged(why) = applied {
            let what = if matches!(c.rec, JournalRecord::Check { .. }) {
                format!("the replica's state differs from the server's at a check ({why})")
            } else {
                format!("the core refused the record or left the server's game ({why})")
            };
            self.note(line);
            return Ok(self.diverge(c.seg, c.entry, Some(c.n), what));
        }
        if matches!(c.rec, JournalRecord::Check { .. }) {
            self.r.checks += 1;
        } else {
            self.note(line);
        }
        let _ = eng;
        Ok(Flow::Go)
    }

    fn on_boundary(
        &mut self,
        eng: &mut Engine,
        done: usize,
        next: &KeyframeRec,
        raw: Option<Vec<u8>>,
    ) -> Result<Flow> {
        self.r.segments += 1;
        let Some(raw) = raw else {
            self.r.boundaries_skipped += 1;
            return Ok(Flow::Go);
        };
        let replayed = eng.digest()?;
        let saved_bytes = eng.save();
        let saved = saved_bytes.as_deref().map(recording_hash);
        if !self.opts.reload && saved == Some(next.raw_hash) {
            self.r.save_hashes_ok += 1;
            self.r.boundaries_ok += 1;
            return Ok(Flow::Go);
        }
        if let (Some(dir), Some(s)) = (&self.opts.dump, &saved_bytes)
            && saved != Some(next.raw_hash)
        {
            std::fs::create_dir_all(dir)?;
            std::fs::write(dir.join(format!("t{}-replayed.gcs1", next.turn)), s)?;
            std::fs::write(dir.join(format!("t{}-server.gcs1", next.turn)), &raw)?;
        }
        eng.load(&raw)?;
        let loaded = eng.digest()?;
        let last_entry = self.store.segs[done].entries.len().saturating_sub(1);
        if replayed != loaded {
            // The checks of the human phase passed (a failed one stops the run earlier), so the turn's orders replay;
            // what differs is the AI phase and the turn change. It is reported, and the run goes on from the snapshot.
            let diff = saved_bytes.as_deref().map_or_else(
                || "the core could not save the replayed game to compare".to_owned(),
                |s| byte_diff(s, &raw),
            );
            self.r.ai_mismatches.push(format!(
                "turn {} -> {}: digest {replayed:016x}, the snapshot's {loaded:016x}; core save against the snapshot: {diff}",
                self.store.segs[done].header.turn, next.turn
            ));
            return Ok(Flow::Go);
        }
        if let Some(h) = saved {
            if h == next.raw_hash {
                self.r.save_hashes_ok += 1;
            } else {
                let what = format!(
                    "the digests agree at the start of turn {}, but the core's save of the replayed game has hash \
                     {h:016x}, the snapshot's raw hash is {:016x}",
                    next.turn, next.raw_hash
                );
                return Ok(self.diverge(done, last_entry, None, what));
            }
        }
        self.r.boundaries_ok += 1;
        Ok(Flow::Go)
    }
}

/// The sections of a GCS1 (rt_save.cpp: a 32-byte header — magic, version, header size, count, body length at 16,
/// body xxh64 at 24 — then per section a u16 name length, the name, a u32 size and the bytes): name, offset of the
/// first data byte, size.
pub fn gcs1_sections(b: &[u8]) -> Vec<(String, usize, usize)> {
    let mut out = Vec::new();
    let mut at = 32usize;
    while at + 2 <= b.len() {
        let n = usize::from(u16::from_le_bytes([b[at], b[at + 1]]));
        let Some(name) = b.get(at + 2..at + 2 + n) else {
            break;
        };
        let Some(sz) = b.get(at + 2 + n..at + 6 + n) else {
            break;
        };
        let size = u32::from_le_bytes([sz[0], sz[1], sz[2], sz[3]]) as usize;
        let data = at + 6 + n;
        out.push((String::from_utf8_lossy(name).into_owned(), data, size));
        at = data + size;
    }
    out
}

/// `offset` as "section+offset" (or the header field it is).
fn place(secs: &[(String, usize, usize)], offset: usize) -> String {
    if offset < 32 {
        return match offset {
            16..24 => format!("header body length+{}", offset - 16),
            24..32 => format!("header body xxh64+{}", offset - 24),
            _ => format!("header+{offset}"),
        };
    }
    secs.iter()
        .find(|(_, s, n)| offset >= *s && offset < s + n)
        .map_or_else(
            || format!("table@{offset}"),
            |(name, s, _)| format!("{name}+{}", offset - s),
        )
}

/// Where two GCS1 byte strings differ, in a phrase.
fn byte_diff(a: &[u8], b: &[u8]) -> String {
    let first = a.iter().zip(b).position(|(x, y)| x != y);
    let n = a.iter().zip(b).filter(|(x, y)| x != y).count();
    // The runs of differing bytes (start..end), the first few.
    let mut runs: Vec<(usize, usize)> = Vec::new();
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        if x != y {
            match runs.last_mut() {
                Some(r) if r.1 == i => r.1 = i + 1,
                _ => runs.push((i, i + 1)),
            }
        }
    }
    let secs = gcs1_sections(b);
    let shown: Vec<String> = runs
        .iter()
        .take(8)
        .map(|(s, e)| format!("{s}..{e} ({})", place(&secs, *s)))
        .collect();
    // The sections with a difference (the layout of `b`; a section that changed size shifts the rest).
    let mut by_sec: Vec<(String, usize)> = Vec::new();
    for (s, e) in &runs {
        let name = place(&secs, *s)
            .split('+')
            .next()
            .unwrap_or_default()
            .to_owned();
        match by_sec.last_mut() {
            Some((n, c)) if *n == name => *c += e - s,
            _ => by_sec.push((name, e - s)),
        }
    }
    let sections: Vec<String> = by_sec.iter().map(|(n, c)| format!("{n} {c}")).collect();
    format!(
        "{} and {} bytes, {n} bytes differ in {} runs, the first at offset {}: {}; bytes by section: {}",
        a.len(),
        b.len(),
        runs.len(),
        first.map_or("none".to_owned(), |f| f.to_string()),
        shown.join(" "),
        sections.join(", ")
    )
}

fn recording_hash(b: &[u8]) -> u64 {
    protocol::xxh64(0, b)
}

/// The first segment of a run: the newest keyframe at or before `from` whose body is readable (the first one that
/// is readable when no turn is given).
pub fn start_seg(store: &Store, from: Option<i32>) -> Result<usize> {
    let candidates: Vec<usize> = (0..store.segs.len())
        .filter(|&i| {
            store
                .keyframe_of(i)
                .is_some_and(|k| from.is_none_or(|f| k.turn <= f))
        })
        .collect();
    let readable = |i: &usize| store.raw_of(*i).is_ok();
    if from.is_some() {
        candidates.iter().rev().copied().find(readable)
    } else {
        candidates.iter().copied().find(readable)
    }
    .ok_or_else(|| {
        anyhow::anyhow!("no keyframe to start from (none readable on disk at or before the turn)")
    })
}

/// Replays turns `from..=to` (the nearest keyframe at or before `from` starts the run).
pub fn run(
    store: &Store,
    eng: &mut Engine,
    from: Option<i32>,
    to: Option<i32>,
    opts: Opts,
) -> Result<Report> {
    let n = store.segs.len();
    let first = start_seg(store, from)?;
    let last = match to {
        Some(t) => (first..n)
            .rev()
            .find(|&i| store.segs[i].header.turn <= t)
            .unwrap_or(first),
        None => n - 1,
    };
    let mut v = V {
        store,
        opts,
        r: Report::default(),
        recent: VecDeque::new(),
        last_jseq: None,
    };
    walk(store, eng, first, last, &mut v)?;
    v.r.segments += 1;
    Ok(v.r)
}

/// The text of a report, for the terminal.
pub fn print(store: &Store, r: &Report) {
    for w in &r.warnings {
        println!("warning: {w}");
    }
    for d in store.damage() {
        println!("torn: {d}");
    }
    if let Some(d) = &r.divergence {
        println!(
            "DIVERGED at turn {} (segment {}, entry {}{}):",
            d.turn,
            d.seg,
            d.entry,
            d.n.map_or(String::new(), |n| format!(", record {n} of the batch"))
        );
        println!("  {}", d.what);
        if let Some(e) = store.segs[d.seg].entries.get(d.entry)
            && let Body::Journal(j) = &e.body
        {
            println!(
                "  batch: {:?}, epoch {}, jseq {}, recorded {}",
                j.phase,
                j.epoch,
                j.jseq,
                describe::at(e)
            );
            if let Some(n) = d.n
                && let Ok(recs) = gamecore::replica::decode_journal(&j.bytes)
                && let Some(rec) = recs.get(n)
            {
                println!("  record: {}", describe::journal_record(rec));
            }
        }
        println!("  what the gateway and the core saw before it, newest last:");
        for l in &d.before {
            println!("    {l}");
        }
    } else {
        for m in &r.ai_mismatches {
            println!("AI PHASE DOES NOT REPLAY: {m}");
        }
        println!(
            "{}: turns {}..{}, {} segments, {} records replayed, {} checks passed, {} keyframe boundaries equal ({} also by the core's save hash), {} not compared",
            if r.ai_mismatches.is_empty() {
                "clean".to_owned()
            } else {
                format!(
                    "human phases clean, AI phase differs in {} turn(s)",
                    r.ai_mismatches.len()
                )
            },
            r.first_turn.unwrap_or(0),
            r.last_turn.unwrap_or(0),
            r.segments,
            r.records,
            r.checks,
            r.boundaries_ok,
            r.save_hashes_ok,
            r.boundaries_skipped
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A GCS1 as rt_save.cpp lays it out: 32 bytes of header, then name length, name, size, bytes per section.
    fn gcs1(sections: &[(&str, &[u8])]) -> Vec<u8> {
        let mut b = b"GCS1".to_vec();
        b.resize(32, 0);
        for (name, data) in sections {
            b.extend_from_slice(&u16::try_from(name.len()).unwrap().to_le_bytes());
            b.extend_from_slice(name.as_bytes());
            b.extend_from_slice(&u32::try_from(data.len()).unwrap().to_le_bytes());
            b.extend_from_slice(data);
        }
        b
    }

    #[test]
    fn a_difference_is_named_by_its_section() {
        let a = gcs1(&[("header", b"turn=1"), ("player05", &[0, 0, 0, 0])]);
        let mut b = a.clone();
        let at = a.len() - 2;
        b[at] = 9;
        b[24] = 1;
        let secs = gcs1_sections(&b);
        assert_eq!(secs.len(), 2);
        assert_eq!(place(&secs, at), "player05+2");
        assert_eq!(place(&secs, 24), "header body xxh64+0");
        let d = byte_diff(&a, &b);
        assert!(d.contains("player05 1"), "{d}");
        assert!(d.contains("header body xxh64 1"), "{d}");
    }
}
