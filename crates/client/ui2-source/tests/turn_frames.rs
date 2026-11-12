//! The source over the turns of a real game, frame by frame, as the host drives it: a replica joins the first
//! snapshot of a fixture of the replica test, the source warms up and settles, then every journal batch and every
//! new turn of the script reaches the replica one pump a frame, and the source runs in each of those frames at the
//! default frame budget. Each frame's profile (cost units and measured µs per root and per kind of read) is
//! printed, and the run is held to the source's rules:
//!
//! - no frame spends more cost units than the frame cap;
//! - the turn number (`game`) and the turn root are fresh in the frame the replica switches the turn;
//! - every root asked for is written within a few frames.
//!
//! In a release build the measured time of the worst frame is held to the cap too (a debug build is far slower).
//!
//! It runs on the real core (Linux/WSL, `GAMECORE_LIB_DIR_x86_64`) with the fixture of the replica test, written by
//! `REPLICA_FIXTURE_OUT=<dir> cargo test -p replica --test real_replica -- --ignored small`.
//! `UI2_TURN_FIXTURE=<dir>/small` names the scenario directory; `UI2_TURN_SEAT=<n>` joins as another seat. Without
//! `UI2_TURN_FIXTURE` the test prints `skipped` and passes. Ignored by default, like the other benches.
//!
//! ```text
//! UI2_TURN_FIXTURE=<dir>/small cargo test --release -p ui2-source --test turn_frames -- --ignored --nocapture
//! ```

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use protocol::{Conn, SeatControl, SeatTurn, SeatView, TurnMode, TurnPhase, TurnState, xxh64};
use replica::{
    GameDataHost, GameFiles, GcBase, GcBlobRef, GcJournal, GcPatchRef, GcTurn, Replica, ReplicaMsg,
    ReplicaStep,
};
use serde_json::Value as Json;
use ui2_model::r#gen::{Game, Ui};
use ui2_model::{ModelStore, Owner, Schema};
use ui2_source::profile::{READ_NAMES, ROOT_COUNTERS};
use ui2_source::session::testing::FakeSession;
use ui2_source::trigger::Roots;
use ui2_source::{DEFAULT_FRAME_BUDGET, ReplicaView, SourceFrame, SourceProfile, UiSource};

type Files = HashMap<String, Vec<u8>>;

/// Frames within which every root a change asks for is written.
const FRESH_WITHIN: u32 = 4;

fn load(dir: &Path) -> Files {
    let list = std::fs::read(dir.join("list.json"))
        .unwrap_or_else(|e| panic!("{}: no list.json: {e}", dir.display()));
    let names: Vec<String> = serde_json::from_slice(&list).expect("list.json");
    names
        .into_iter()
        .map(|n| {
            let bytes = std::fs::read(dir.join(&n)).unwrap_or_else(|e| panic!("fixture: {n}: {e}"));
            (n, bytes)
        })
        .collect()
}

fn text<'a>(v: &'a Json, key: &str) -> &'a str {
    v.get(key)
        .and_then(Json::as_str)
        .unwrap_or_else(|| panic!("script: no {key}"))
}

fn num(v: &Json, key: &str) -> u64 {
    v.get(key)
        .and_then(Json::as_u64)
        .unwrap_or_else(|| panic!("script: no {key}"))
}

fn hex(v: &Json, key: &str) -> u64 {
    u64::from_str_radix(text(v, key), 16).expect("hex")
}

fn file<'a>(files: &'a Files, name: &str) -> &'a Vec<u8> {
    files
        .get(name)
        .unwrap_or_else(|| panic!("fixture: no file {name}"))
}

fn blob_of(body: &[u8]) -> GcBlobRef {
    GcBlobRef {
        hash: xxh64(0, body),
        size: body.len() as u32,
    }
}

fn replica(files: &Files, seat: u8) -> Replica {
    let index = file(files, "gamedata/vfs-index.json").clone();
    let parsed: Json = serde_json::from_slice(&index).expect("vfs-index.json");
    let mut g = GameFiles::from_index(index).expect("vfs index");
    let mut put = |id: &str, path: &str| {
        if let Some(b) = files.get(&format!("gamedata/{path}")) {
            g.put(id, b.clone());
        }
    };
    put(text(&parsed["db"], "gameplay"), "db/gameplay.db");
    put(text(&parsed["db"], "localization"), "db/loc-en_us.db");
    for f in parsed["files"].as_array().expect("index: no files") {
        put(text(f, "id"), text(f, "path"));
    }
    let mut r = Replica::new(GameDataHost::new(Arc::new(Mutex::new(g))), seat).expect("core");
    r.config_mut().pack_base = false;
    r
}

/// One frame of the run: what the replica did, what the source spent.
struct Row {
    frame: u32,
    step: ReplicaStep,
    units: u32,
    us: f64,
    profile: SourceProfile,
}

struct Run {
    store: ModelStore,
    src: UiSource,
    session: FakeSession,
    ui: Ui,
    now: u64,
    rows: Vec<Row>,
    /// Frames each root has waited since it was last asked for.
    waited: HashMap<u16, u32>,
}

impl Run {
    fn frame(&mut self, r: &mut Replica, step: ReplicaStep) -> &Row {
        let t0 = Instant::now();
        let stats = {
            let mut w = self.store.writer(Owner::Source);
            self.src.frame(
                SourceFrame {
                    replica: Some(ReplicaView::of(r)),
                    steps: std::slice::from_ref(&step),
                    session: &self.session,
                    dismissed: &Default::default(),
                    now_ms: self.now,
                },
                &mut w,
                &self.ui,
            )
        };
        let us = t0.elapsed().as_secs_f64() * 1e6;
        self.store.take_frame();
        self.now += 16;
        let profile = self.src.profile().clone();
        for (root, _) in ROOT_COUNTERS {
            let w = self.waited.entry(root.bits()).or_default();
            *w = if profile.pending.contains(root) {
                *w + 1
            } else {
                0
            };
            assert!(
                *w <= FRESH_WITHIN,
                "frame {}: {root:?} waited {} frames",
                self.rows.len(),
                *w
            );
        }
        self.rows.push(Row {
            frame: self.rows.len() as u32,
            step,
            units: stats.spent_units,
            us,
            profile,
        });
        self.rows.last().expect("pushed")
    }

    fn settle(&mut self, r: &mut Replica) {
        for _ in 0..32 {
            let row = self.frame(r, ReplicaStep::Idle);
            if row.profile.pending.is_empty() {
                return;
            }
        }
        panic!("the source does not settle");
    }

    fn game_turn(&self) -> Option<i32> {
        self.store.get::<Game>().map(|g| g.turn)
    }
}

fn session(seat: u8, turn: i32) -> FakeSession {
    let me = seat.cast_signed();
    FakeSession {
        connected: true,
        me: Some(me),
        turn: Some(TurnState {
            turn,
            phase: TurnPhase::Humans,
            mode: TurnMode::Simultaneous,
            seats: vec![SeatTurn {
                player: me,
                finished: false,
                may_act: true,
                group: 0,
            }],
            timers: Vec::new(),
            paused: false,
        }),
        seats: vec![SeatView {
            player: me,
            control: SeatControl::Human {
                nick: "turns".into(),
            },
            conn: Conn::Online,
            host: true,
        }],
        chat: Vec::new(),
        chat_total: 0,
    }
}

fn print(rows: &[Row]) {
    println!("UI2TURN frame step                     units      us  roots (units/us)");
    for r in rows {
        if r.profile.roots.is_empty() && r.units == 0 {
            continue;
        }
        let roots: Vec<String> = r
            .profile
            .roots
            .iter()
            .map(|c| {
                format!(
                    "{:?}{} {}/{:.0}",
                    c.root,
                    if c.written { "" } else { "(wait)" },
                    c.units,
                    c.us
                )
            })
            .collect();
        println!(
            "UI2TURN {:>5} {:<24} {:>5} {:>7.0}  {}",
            r.frame,
            format!("{:?}", r.step),
            r.units,
            r.us,
            roots.join(", ")
        );
    }
    // Per kind of read: measured µs per call against the weight.
    let mut kinds: Vec<(u32, f64, u32, f64)> = vec![(0, 0.0, 0, 0.0); READ_NAMES.len()];
    for r in rows {
        for (i, c) in r.profile.reads.iter().enumerate() {
            if c.calls > 0 {
                let k = &mut kinds[i];
                k.0 += c.calls;
                k.1 += c.us;
                k.2 += c.units;
                k.3 = k.3.max(c.us / f64::from(c.calls));
            }
        }
    }
    println!("UI2TURN read              calls  mean_us   max_us  weight");
    for (i, (calls, us, units, max)) in kinds.into_iter().enumerate() {
        if calls > 0 {
            println!(
                "UI2TURN {:<18} {:>5} {:>8.1} {:>8.1} {:>7}",
                READ_NAMES[i],
                calls,
                us / f64::from(calls),
                max,
                units / calls
            );
        }
    }
}

#[test]
#[ignore = "needs the real core and a fixture of the replica test (UI2_TURN_FIXTURE)"]
fn the_source_over_real_turns() {
    let Some(dir) = std::env::var_os("UI2_TURN_FIXTURE") else {
        eprintln!(
            "skipped: UI2_TURN_FIXTURE=<fixture dir>/<scenario> is not set (it needs the real core linked)"
        );
        return;
    };
    let files = load(Path::new(&dir));
    let script: Json = serde_json::from_slice(file(&files, "script.json")).expect("script.json");
    let seat = std::env::var("UI2_TURN_SEAT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| script.get("seat").and_then(Json::as_u64).unwrap_or(0) as u8);
    let steps = script["steps"].as_array().expect("steps");
    let mut r = replica(&files, seat);
    let t0 = Instant::now();
    let mut src = UiSource::new(Schema::v1());
    src.set_profile_clock(Box::new(move || t0.elapsed().as_secs_f64() * 1000.0));
    src.set_read_budget(DEFAULT_FRAME_BUDGET);
    let mut run = Run {
        store: ModelStore::new(Schema::v1()),
        src,
        session: session(seat, 0),
        ui: Ui::default(),
        now: 1_000_000,
        rows: Vec::new(),
        waited: HashMap::new(),
    };
    let mut turns = 0;
    for step in steps {
        match step["op"].as_str().expect("op") {
            "create" => {}
            "base" => {
                let body = file(&files, text(step, "body")).clone();
                let turn = num(step, "turn") as i32;
                run.session = session(seat, turn);
                r.on_msg(ReplicaMsg::Base(GcBase {
                    epoch: num(step, "epoch") as u32,
                    turn,
                    jseq: num(step, "jseq"),
                    raw_hash: hex(step, "raw_hash"),
                    snapshot: blob_of(&body),
                }));
                r.blob(blob_of(&body).hash, body);
                let s = r.pump(0);
                run.frame(&mut r, s);
                let s = r.pump(0);
                assert!(matches!(s, ReplicaStep::Loaded { .. }), "{s:?}");
                // The host warms the source up once the game is joined, outside the frames.
                let rd = r.read().expect("live");
                let w = run.src.warm_up(rd);
                println!(
                    "UI2TURN warm-up: {} reads, {:.0} us",
                    w.getter_calls,
                    run.src.profile().reads.iter().map(|c| c.us).sum::<f64>()
                );
                run.frame(&mut r, s);
                run.settle(&mut r);
            }
            "journal" => {
                let list = step["batches"].as_array().expect("batches");
                let batches = list
                    .iter()
                    .map(|b| file(&files, text(b, "file")).clone())
                    .collect();
                r.on_msg(ReplicaMsg::Journal(GcJournal {
                    epoch: num(step, "epoch") as u32,
                    first_jseq: num(step, "first"),
                    batches,
                }));
                // One batch a frame, as a pump of budget 0 does.
                for _ in list {
                    let s = r.pump(0);
                    assert!(matches!(s, ReplicaStep::Applied { .. }), "{s:?}");
                    run.frame(&mut r, s);
                }
                run.settle(&mut r);
            }
            "turn" => {
                turns += 1;
                let turn = num(step, "turn") as i32;
                // The server's turn state comes first: the session knows the new turn before the replica has it.
                run.session = session(seat, turn);
                run.frame(&mut r, ReplicaStep::Idle);
                let snapshot = file(&files, text(step, "body")).clone();
                let patch = step
                    .get("patch")
                    .and_then(Json::as_str)
                    .map(|p| file(&files, p).clone());
                r.on_msg(ReplicaMsg::Turn(GcTurn {
                    epoch: num(step, "epoch") as u32,
                    turn,
                    jseq: num(step, "jseq"),
                    raw_hash: hex(step, "raw_hash"),
                    snapshot: blob_of(&snapshot),
                    patch: patch.as_ref().map(|p| GcPatchRef {
                        from_raw_hash: hex(step, "from_raw_hash"),
                        blob: blob_of(p),
                    }),
                }));
                let body = patch.unwrap_or(snapshot);
                r.blob(blob_of(&body).hash, body);
                let s = r.pump(0);
                run.frame(&mut r, s);
                let s = r.pump(0);
                assert!(matches!(s, ReplicaStep::TurnSwitched { .. }), "{s:?}");
                // The host takes the core's slow first reads after the load out of the frame, as part of the load.
                let rd = r.read().expect("live");
                let w = run.src.warm_turn(rd, &run.ui);
                println!(
                    "UI2TURN warm after the load of turn {turn}: {} reads, {:.0} us",
                    w.getter_calls,
                    run.src.profile().reads.iter().map(|c| c.us).sum::<f64>()
                );
                let row = run.frame(&mut r, s);
                assert!(
                    row.profile.wrote(Roots::GAME) && row.profile.wrote(Roots::TURN),
                    "turn {turn}: the turn roots are written in the frame of the switch: {:?}",
                    row.profile.roots
                );
                assert_eq!(run.game_turn(), Some(turn), "the dial shows the new turn");
                run.settle(&mut r);
            }
            other => panic!("unknown op {other}"),
        }
    }
    print(&run.rows);
    assert!(turns > 0, "the fixture has no new turn");
    let worst_units = run.rows.iter().map(|r| r.units).max().unwrap_or(0);
    let worst_us = run
        .rows
        .iter()
        .skip(1)
        .map(|r| r.us)
        .fold(0.0_f64, f64::max);
    println!(
        "UI2TURN worst frame: {worst_units} units, {worst_us:.0} us (cap {DEFAULT_FRAME_BUDGET})"
    );
    assert!(
        worst_units <= DEFAULT_FRAME_BUDGET,
        "a frame spent {worst_units} units"
    );
    if !cfg!(debug_assertions) {
        assert!(
            worst_us <= f64::from(DEFAULT_FRAME_BUDGET),
            "a frame took {worst_us:.0} us"
        );
    }
}
