//! A scripted run of the replica against a fixture: boots the core on the game data, joins on a snapshot, applies
//! the journal batches, changes turns by snapshot and by patch, and compares the replica digest with the
//! server's after each step. The same script runs natively (the `real_replica` test, Linux) and in the browser
//! (`cargo xtask web-selftest`, case `replica_load`), so the numbers of AC-G4 come from one piece of code.
//!
//! The fixture is a set of named files: `script.json`, the snapshot and patch bodies (`C5RB`), the journal batches
//! (`GCJ1`), the patch's entropy header, and the game data as the core reads it (`gamedata/<vfs path>`). The native
//! `real_replica` test writes it from a server it plays; `script.json` is
//!
//! ```text
//! { "seat": 0, "check_always": true, "patches": true, "every_batch": true,
//!   "steps": [
//!     { "op": "create" },
//!     { "op": "base", "epoch": 1, "turn": 5, "jseq": 0, "raw_hash": "<hex>", "body": "snap-0.body", "digest": "<hex>" },
//!     { "op": "journal", "epoch": 1, "first": 1, "batches": [ { "file": "batch-1.bin", "digest": "<hex>" } ] },
//!     { "op": "turn", "epoch": 1, "turn": 6, "jseq": 3, "raw_hash": "<hex>", "body": "snap-1.body",
//!       "patch": "patch-1.body" | null, "from_raw_hash": "<hex>", "digest": "<hex>" } ] }
//! ```

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use platform::Clock;
use protocol::xxh64;
use serde_json::Value;

use crate::{
    GameDataHost, GameFiles, GcBase, GcBlobRef, GcJournal, GcPatchRef, GcTurn, Replica,
    ReplicaConfig, ReplicaMsg, ReplicaStep,
};

/// Sets the clocks of the C/C++ runtime of the core (wasm: the host must, before the core boots; natively a no-op).
/// `realtime_ns` is wall time since the Unix epoch, `monotonic_ns` a monotonic clock.
pub fn set_clock(realtime_ns: u64, monotonic_ns: u64) {
    gamecore::set_clock(realtime_ns, monotonic_ns);
}

/// What a run measured.
#[derive(Debug, Clone, Default)]
pub struct SelftestReport {
    /// Metrics in the order they were taken: milliseconds, megabytes, kilobytes, counts (the key says).
    pub lines: Vec<(String, f64)>,
    /// Paths the core asked for that the game data did not have.
    pub missed: Vec<String>,
}

impl SelftestReport {
    fn put(&mut self, key: impl Into<String>, v: f64) {
        self.lines.push((key.into(), v));
    }

    pub fn get(&self, key: &str) -> Option<f64> {
        self.lines.iter().find(|(k, _)| k == key).map(|(_, v)| *v)
    }

    /// One JSON object of the metrics.
    pub fn to_json(&self, name: &str) -> String {
        let mut s = format!("{{\"scenario\":{name:?}");
        for (k, v) in &self.lines {
            let _ = write!(s, ",{k:?}:{v:.2}");
        }
        s.push('}');
        s
    }
}

fn hex(v: &Value, key: &str) -> Result<u64, String> {
    let s = v
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("script: no {key}"))?;
    u64::from_str_radix(s, 16).map_err(|e| format!("script: {key}: {e}"))
}

fn num(v: &Value, key: &str) -> Result<u64, String> {
    v.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("script: no {key}"))
}

fn text<'a>(v: &'a Value, key: &str) -> Result<&'a str, String> {
    v.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("script: no {key}"))
}

fn blob_of(body: &[u8]) -> GcBlobRef {
    GcBlobRef {
        hash: xxh64(0, body),
        size: body.len() as u32,
    }
}

type Files = HashMap<String, Vec<u8>>;

fn file<'a>(files: &'a Files, name: &str) -> Result<&'a Vec<u8>, String> {
    files
        .get(name)
        .ok_or_else(|| format!("fixture: no file {name}"))
}

/// Builds the game data the core reads from the `gamedata/…` files of the fixture.
fn game_files(files: &Files) -> Result<GameFiles, String> {
    let index = file(files, "gamedata/vfs-index.json")?.clone();
    let parsed: Value = serde_json::from_slice(&index).map_err(|e| e.to_string())?;
    let mut g = GameFiles::from_index(index).map_err(|e| e.to_string())?;
    let mut put = |id: &str, path: &str| {
        if let Some(b) = files.get(&format!("gamedata/{path}")) {
            g.put(id, b.clone());
        }
    };
    put(text(&parsed["db"], "gameplay")?, "db/gameplay.db");
    put(text(&parsed["db"], "localization")?, "db/loc-en_us.db");
    for f in parsed["files"].as_array().ok_or("index: no files")? {
        put(text(f, "id")?, text(f, "path")?);
    }
    Ok(g)
}

/// Runs the script. `probe` gives a number the shell measures itself (the resident set natively); it is recorded
/// as `probe_<where>` after the create, the join and each turn change. Errors name the step that failed; a digest
/// that differs is an error.
pub fn run(
    files: &Files,
    clock: Box<dyn Clock>,
    probe: Option<&dyn Fn() -> f64>,
) -> Result<SelftestReport, String> {
    let script: Value = serde_json::from_slice(file(files, "script.json")?)
        .map_err(|e| format!("script.json: {e}"))?;
    let mut rep = SelftestReport::default();
    let now = |c: &dyn Clock| c.now_ms_f64();
    let clock: Arc<dyn Clock> = Arc::from(clock);
    let probe_at = |rep: &mut SelftestReport, at: &str| {
        if let Some(p) = probe {
            rep.put(format!("probe_{at}"), p());
        }
    };

    let t = now(clock.as_ref());
    let data = Arc::new(Mutex::new(game_files(files)?));
    rep.put("game_data_files_ms", now(clock.as_ref()) - t);
    let (loaded, bytes) = data.lock().map_err(|e| e.to_string())?.loaded();
    rep.put("game_data_files", loaded as f64);
    rep.put("game_data_mb", bytes as f64 / 1048576.0);
    probe_at(&mut rep, "before_create");

    let mut replica: Option<Replica> = None;
    let seat = script.get("seat").and_then(Value::as_u64).unwrap_or(0) as u8;
    let every_batch = script
        .get("every_batch")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let mut turn_no = 0usize;

    // A clock for the replica that is the same object as ours.
    struct Shared(Arc<dyn Clock>);
    impl Clock for Shared {
        fn now_ms(&self) -> u64 {
            self.0.now_ms()
        }
        fn now_ms_f64(&self) -> f64 {
            self.0.now_ms_f64()
        }
    }

    for (i, step) in script["steps"]
        .as_array()
        .ok_or("script: no steps")?
        .iter()
        .enumerate()
    {
        let op = text(step, "op")?;
        let fail = |what: String| format!("step {i} ({op}): {what}");
        match op {
            "create" => {
                let t = now(clock.as_ref());
                let mut r = Replica::new(GameDataHost::new(data.clone()), seat)
                    .map_err(|e| fail(e.to_string()))?;
                rep.put("gc_create_ms", now(clock.as_ref()) - t);
                r.set_clock(Box::new(Shared(clock.clone())));
                let cfg: &mut ReplicaConfig = r.config_mut();
                cfg.check_always = script
                    .get("check_always")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                cfg.pack_base = false;
                if let Some(on) = script.get("patches").and_then(Value::as_bool) {
                    cfg.patches = on;
                }
                replica = Some(r);
                probe_at(&mut rep, "after_create");
                let st = replica.as_ref().map(Replica::stats).unwrap_or_default();
                rep.put("memory_mb_after_create", f64::from(st.memory_mb));
            }
            "base" => {
                let r = replica
                    .as_mut()
                    .ok_or_else(|| fail("no create before".into()))?;
                let body = file(files, text(step, "body")?)?;
                let base_body_kb = body.len() as f64 / 1024.0;
                let t = now(clock.as_ref());
                r.on_msg(ReplicaMsg::Base(GcBase {
                    epoch: num(step, "epoch")? as u32,
                    turn: num(step, "turn")? as i32,
                    jseq: num(step, "jseq")?,
                    raw_hash: hex(step, "raw_hash")?,
                    snapshot: blob_of(body),
                }));
                let want = r.wanted_blobs();
                if want != [blob_of(body)] {
                    return Err(fail(format!("wanted {want:?}")));
                }
                r.blob(blob_of(body).hash, body.clone());
                let t1 = now(clock.as_ref());
                if r.pump(0) != ReplicaStep::Working {
                    return Err(fail("the unpack stage did not run".into()));
                }
                let t2 = now(clock.as_ref());
                if !matches!(r.pump(0), ReplicaStep::Loaded { .. }) {
                    return Err(fail("the load stage did not run".into()));
                }
                let t3 = now(clock.as_ref());
                rep.put("join_unpack_ms", t2 - t1);
                rep.put("join_load_ms", t3 - t2);
                rep.put("join_total_ms", t3 - t);
                rep.put("join_body_kb", base_body_kb);
                probe_at(&mut rep, "after_join");
                let st = r.stats();
                rep.put("memory_mb_after_join", f64::from(st.memory_mb));
                check_digest(r, step, &mut rep, "join", &clock).map_err(fail)?;
            }
            "journal" => {
                let r = replica
                    .as_mut()
                    .ok_or_else(|| fail("no create before".into()))?;
                let list = step["batches"]
                    .as_array()
                    .ok_or_else(|| fail("no batches".into()))?;
                let mut batches = Vec::new();
                for b in list {
                    batches.push(file(files, text(b, "file")?)?.clone());
                }
                let t = now(clock.as_ref());
                r.on_msg(ReplicaMsg::Journal(GcJournal {
                    epoch: num(step, "epoch")? as u32,
                    first_jseq: num(step, "first")?,
                    batches,
                }));
                let mut digest_ms = 0.0;
                for (n, b) in list.iter().enumerate() {
                    let jseq = num(step, "first")? + n as u64;
                    match r.pump(0) {
                        ReplicaStep::Applied { jseq: got } if got == jseq => {}
                        other => {
                            return Err(fail(format!(
                                "batch {jseq}: {other:?} in {:?}",
                                r.state()
                            )));
                        }
                    }
                    if every_batch || n + 1 == list.len() {
                        let td = now(clock.as_ref());
                        let got = r.digest().map_err(&fail)?;
                        digest_ms += now(clock.as_ref()) - td;
                        let want = hex(b, "digest")?;
                        if got != want {
                            return Err(fail(format!(
                                "digest after batch {jseq}: replica {got:016x}, server {want:016x}"
                            )));
                        }
                    }
                }
                rep.put(
                    format!("turn{turn_no}_journal_apply_ms"),
                    now(clock.as_ref()) - t - digest_ms,
                );
                rep.put(format!("turn{turn_no}_journal_batches"), list.len() as f64);
            }
            "turn" => {
                let r = replica
                    .as_mut()
                    .ok_or_else(|| fail("no create before".into()))?;
                turn_no += 1;
                let snapshot = file(files, text(step, "body")?)?;
                let patch = match step.get("patch").and_then(Value::as_str) {
                    Some(name) => Some(file(files, name)?),
                    None => None,
                };
                let from = hex(step, "from_raw_hash")?;
                let asked = patch.map_or(blob_of(snapshot), |p| blob_of(p));
                let t = now(clock.as_ref());
                r.on_msg(ReplicaMsg::Turn(GcTurn {
                    epoch: num(step, "epoch")? as u32,
                    turn: num(step, "turn")? as i32,
                    jseq: num(step, "jseq")?,
                    raw_hash: hex(step, "raw_hash")?,
                    snapshot: blob_of(snapshot),
                    patch: patch.map(|p| GcPatchRef {
                        from_raw_hash: from,
                        blob: blob_of(p),
                    }),
                }));
                let want = r.wanted_blobs();
                if want != [asked] {
                    return Err(fail(format!("wanted {want:?}, expected {asked:?}")));
                }
                r.blob(asked.hash, patch.unwrap_or(snapshot).clone());
                let t1 = now(clock.as_ref());
                if r.pump(0) != ReplicaStep::Working {
                    return Err(fail("the unpack stage did not run".into()));
                }
                let t2 = now(clock.as_ref());
                if !matches!(r.pump(0), ReplicaStep::TurnSwitched { .. }) {
                    return Err(fail(format!(
                        "the load stage did not run ({:?})",
                        r.state()
                    )));
                }
                let t3 = now(clock.as_ref());
                let kind = if patch.is_some() { "patch" } else { "snapshot" };
                rep.put(
                    format!("turn{turn_no}_{kind}_body_kb"),
                    patch.unwrap_or(snapshot).len() as f64 / 1024.0,
                );
                rep.put(format!("turn{turn_no}_{kind}_unpack_ms"), t2 - t1);
                rep.put(format!("turn{turn_no}_{kind}_load_ms"), t3 - t2);
                rep.put(format!("turn{turn_no}_{kind}_pause_ms"), t3 - t1);
                rep.put(format!("turn{turn_no}_{kind}_total_ms"), t3 - t);
                probe_at(&mut rep, &format!("after_turn{turn_no}"));
                rep.put(
                    format!("memory_mb_after_turn{turn_no}"),
                    f64::from(r.stats().memory_mb),
                );
                check_digest(r, step, &mut rep, &format!("turn{turn_no}"), &clock).map_err(fail)?;
                let st = r.stats();
                if patch.is_some() && st.turn_patch == 0 {
                    return Err(fail("the patch was not used".into()));
                }
            }
            other => return Err(fail(format!("unknown op {other}"))),
        }
    }
    let r = replica.as_mut().ok_or("script: no create")?;
    let st = r.stats();
    rep.put("diverged", st.diverged as f64);
    rep.put("resyncs", st.resyncs as f64);
    rep.put("checks_run", st.checks_run as f64);
    rep.put("applied", st.applied as f64);
    rep.put("turn_patch", st.turn_patch as f64);
    rep.put("turn_snapshot", st.turn_snapshot as f64);
    rep.put("patch_fallbacks", st.patch_fallbacks as f64);
    rep.put("memory_mb_final", f64::from(st.memory_mb));
    if st.diverged != 0 || st.resyncs != 0 || st.patch_fallbacks != 0 {
        return Err(format!("the replica left the server's game: {st:?}"));
    }
    let g = data.lock().map_err(|e| e.to_string())?;
    rep.put("game_files_served", g.served().len() as f64);
    rep.missed = g.missed().to_vec();
    Ok(rep)
}

fn check_digest(
    r: &mut Replica,
    step: &Value,
    rep: &mut SelftestReport,
    at: &str,
    clock: &Arc<dyn Clock>,
) -> Result<(), String> {
    let want = hex(step, "digest")?;
    let t = clock.now_ms_f64();
    let got = r.digest()?;
    rep.put(format!("digest_ms_{at}"), clock.now_ms_f64() - t);
    if got != want {
        return Err(format!(
            "digest after {at}: replica {got:016x}, server {want:016x}"
        ));
    }
    Ok(())
}
