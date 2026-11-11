//! The scenario of the replica-untouched test, shared by the native test (`tests/r17_replica_untouched.rs`) and
//! the browser self-test (`app-web`, `replica_r17`, `cargo xtask web-selftest --scenario ui2-r17`): the same steps
//! on the native core and on the wasm one.
//!
//! The rule under test: what the UI v2 source and the actions module do never changes the replica. Only the
//! server's journal batches, snapshots and patches move the game, so the replica's digest, its batch counters and
//! its last applied batch stay as they were until the server answers (here there is no server, so that holds by
//! construction).
//!
//! 1. A replica joins the first snapshot of a fixture of the replica test and is live.
//! 2. Its digest, counters and last applied batch are taken.
//! 3. The model is rebuilt from scratch, twice: every root, the `unit` root of every own unit (its actions read),
//!    the `production` root of every own city and its `detail`.
//! 4. Every intent of the catalogue goes through the actions module with a recording sink (nothing reaches a
//!    network): valid and invalid arguments, every enum value, a refusing sink, every `js` field of `ui.*` and the
//!    skin's and a mod's `state.*` written, windows, selection, production queue, purchase, research, chat, notes,
//!    camera, screens, end of turn and its cancel. The source runs a frame after each, as the host does.
//! 5. Step 3 again.
//! 6. The replica is checked against step 2; the sink holds orders, a chat line, camera moves and screens, every
//!    order stamped with the model's turn and naming own units and cities only; the source did read the core.
//! 7. Sensitivity: the next journal batch of the fixture, applied through the replica's own entry points, changes
//!    the digest, and the same comparison reports it.
//!
//! This file has no clock and no file access: the callers hand it the fixture as named byte buffers.

// Each of the two callers uses a part of the report.
#![allow(dead_code)]

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use protocol::{
    Command, Conn, SeatControl, SeatTurn, SeatView, TurnMode, TurnPhase, TurnState, xxh64,
};
use replica::{
    GameDataHost, GameFiles, GcBase, GcBlobRef, GcJournal, Replica, ReplicaMsg, ReplicaStats,
    ReplicaStep, State,
};
use serde_json::{Value as Json, json};
use ui2_actions::{ActionEvent, MemorySettings, RecordingSink, Refused, Sent, StateNs, UiActions};
use ui2_model::generated::{Game, Map, Production, Research, Ui, WinCity};
use ui2_model::schema::{IntentDef, Scalar, Schema, Type};
use ui2_model::wire::{ModIdx, Origin, StateOp, WireIntent};
use ui2_model::{Fragment, Key, KeyedList, ModelStore, Owner, Value};
use ui2_source::session::testing::FakeSession;
use ui2_source::{ReplicaView, SourceFrame, UiSource};

/// The fixture: file name (as in its `list.json`) to bytes.
pub type Files = HashMap<String, Vec<u8>>;

/// The state of skin and mod scripts the scenario mounts: `state.*` writes need a mounted fragment.
const SKIN_STATE: &str = "[root]\ncount = \"i32\"\nname = \"str\"\nflag = \"bool\"\n";

/// The host's clock of the first frame, ms; each frame adds 16.
const T0: u64 = 1_000_000;

// ---------------------------------------------------------------- the fixture

fn text<'a>(v: &'a Json, key: &str) -> Result<&'a str, String> {
    v.get(key)
        .and_then(Json::as_str)
        .ok_or_else(|| format!("script: no {key}"))
}

fn num(v: &Json, key: &str) -> Result<u64, String> {
    v.get(key)
        .and_then(Json::as_u64)
        .ok_or_else(|| format!("script: no {key}"))
}

fn hex(v: &Json, key: &str) -> Result<u64, String> {
    u64::from_str_radix(text(v, key)?, 16).map_err(|e| format!("script: {key}: {e}"))
}

fn file<'a>(files: &'a Files, name: &str) -> Result<&'a Vec<u8>, String> {
    files
        .get(name)
        .ok_or_else(|| format!("fixture: no file {name}"))
}

fn blob_of(body: &[u8]) -> GcBlobRef {
    GcBlobRef {
        hash: xxh64(0, body),
        size: body.len() as u32,
    }
}

/// A replica of `seat` (the script's seat when `None`) live on the first snapshot of the script.
fn joined(files: &Files, script: &Json, seat: Option<u8>) -> Result<Replica, String> {
    let index = file(files, "gamedata/vfs-index.json")?.clone();
    let parsed: Json = serde_json::from_slice(&index).map_err(|e| e.to_string())?;
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
    let seat = seat.unwrap_or_else(|| script.get("seat").and_then(Json::as_u64).unwrap_or(0) as u8);
    let mut r = Replica::new(GameDataHost::new(Arc::new(Mutex::new(g))), seat)
        .map_err(|e| e.to_string())?;
    r.config_mut().pack_base = false;
    let base = steps(script)?
        .iter()
        .find(|s| s["op"] == "base")
        .ok_or("script: no base step")?;
    let body = file(files, text(base, "body")?)?.clone();
    let blob = blob_of(&body);
    r.on_msg(ReplicaMsg::Base(GcBase {
        epoch: num(base, "epoch")? as u32,
        turn: num(base, "turn")? as i32,
        jseq: num(base, "jseq")?,
        raw_hash: hex(base, "raw_hash")?,
        snapshot: blob,
    }));
    r.blob(blob.hash, body);
    if r.pump(0) != ReplicaStep::Working {
        return Err("the unpack stage did not run".into());
    }
    if !matches!(r.pump(0), ReplicaStep::Loaded { .. }) {
        return Err("the load stage did not run".into());
    }
    if r.state() != State::Live {
        return Err(format!("the replica is {:?} after the join", r.state()));
    }
    // The join itself is right: the server's digest of the snapshot.
    let got = r.digest()?;
    let want = hex(base, "digest")?;
    if got != want {
        return Err(format!(
            "digest after the join: replica {got:016x}, server {want:016x}"
        ));
    }
    Ok(r)
}

fn steps(script: &Json) -> Result<&Vec<Json>, String> {
    script["steps"]
        .as_array()
        .ok_or_else(|| "script: no steps".to_owned())
}

/// The first batch of the first journal step: `(epoch, jseq, bytes, server digest after it)`.
fn first_batch(files: &Files, script: &Json) -> Result<(u32, u64, Vec<u8>, u64), String> {
    let step = steps(script)?
        .iter()
        .find(|s| s["op"] == "journal")
        .ok_or("the fixture has no journal batch: the sensitivity control cannot run")?;
    let b = step["batches"]
        .as_array()
        .and_then(|l| l.first())
        .ok_or("script: an empty journal step")?;
    Ok((
        num(step, "epoch")? as u32,
        num(step, "first")?,
        file(files, text(b, "file")?)?.clone(),
        hex(b, "digest")?,
    ))
}

// ---------------------------------------------------------------- what "untouched" means

/// What the rule watches in a replica.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub digest: u64,
    pub applied_jseq: u64,
    pub generation: u64,
    pub state: State,
    /// All counters; `memory_mb` (the module's linear memory, it grows with any allocation) is left out.
    pub stats: ReplicaStats,
}

impl Mark {
    fn of(r: &mut Replica) -> Result<Mark, String> {
        let mut stats = r.stats();
        stats.memory_mb = 0;
        Ok(Mark {
            digest: r.digest()?,
            applied_jseq: r.applied_jseq(),
            generation: r.generation(),
            state: r.state(),
            stats,
        })
    }
}

/// `Ok` when the replica is as it was: the same digest, the same batches applied, the same last batch, the same
/// counters. Otherwise what differs.
pub fn untouched(before: &Mark, after: &Mark) -> Result<(), String> {
    let mut diff = Vec::new();
    if before.digest != after.digest {
        diff.push(format!(
            "digest {:016x} -> {:016x}",
            before.digest, after.digest
        ));
    }
    if before.stats.applied != after.stats.applied {
        diff.push(format!(
            "applied {} -> {}",
            before.stats.applied, after.stats.applied
        ));
    }
    if before.applied_jseq != after.applied_jseq {
        diff.push(format!(
            "applied_jseq {} -> {}",
            before.applied_jseq, after.applied_jseq
        ));
    }
    if before.generation != after.generation || before.state != after.state {
        diff.push(format!(
            "state {:?}/{} -> {:?}/{}",
            before.state, before.generation, after.state, after.generation
        ));
    }
    if diff.is_empty() && before.stats != after.stats {
        diff.push(format!("counters {:?} -> {:?}", before.stats, after.stats));
    }
    if diff.is_empty() {
        Ok(())
    } else {
        Err(diff.join("; "))
    }
}

// ---------------------------------------------------------------- the host: source, actions, store

/// What the source did over the run.
#[derive(Clone, Copy, Debug, Default)]
pub struct Reads {
    pub frames: u32,
    pub getter_calls: u64,
    pub roots_built: u64,
    /// `unit` roots written for the unit asked for, `production` roots for the city asked for, with a `detail`.
    pub unit_roots: u32,
    pub production_roots: u32,
    pub detail_roots: u32,
}

struct Host {
    schema: &'static Schema,
    store: ModelStore,
    src: UiSource,
    actions: UiActions,
    sink: RecordingSink,
    session: FakeSession,
    now: u64,
    reads: Reads,
    intents: u32,
    state_ops: u32,
}

impl Host {
    fn new(seat: u8, turn: i32) -> Host {
        let schema = Schema::v1();
        let mut store = ModelStore::new(schema);
        let mut actions = UiActions::new(schema, Box::new(MemorySettings::default()));
        actions.publish(&mut store);
        let mut src = UiSource::new(schema);
        // One frame builds everything it is asked for: the test looks at what is read, not at frame times.
        src.set_read_budget(u32::MAX);
        let me = seat as i8;
        let session = FakeSession {
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
                control: SeatControl::Human { nick: "r17".into() },
                conn: Conn::Online,
                host: true,
            }],
            chat: Vec::new(),
            chat_total: 0,
        };
        Host {
            schema,
            store,
            src,
            actions,
            sink: RecordingSink::new(),
            session,
            now: T0,
            reads: Reads::default(),
            intents: 0,
            state_ops: 0,
        }
    }

    /// One frame of the host after the intents: the source (with `ui`, or the actions module's own `ui` root),
    /// the reconcile, the frame's delta taken.
    fn frame(&mut self, r: &mut Replica, ui: Option<&Ui>) {
        let own;
        let ui = match ui {
            Some(u) => u,
            None => {
                own = self.actions.ui().clone();
                &own
            }
        };
        let stats = {
            let mut w = self.store.writer(Owner::Source);
            self.src.frame(
                SourceFrame {
                    replica: Some(ReplicaView::of(r)),
                    steps: &[],
                    session: &self.session,
                    dismissed: self.actions.dismissed(),
                    now_ms: self.now,
                },
                &mut w,
                ui,
            )
        };
        self.actions
            .reconcile(&mut self.store, self.src.notification_ids(), self.now);
        self.store.take_frame();
        self.now += 16;
        self.reads.frames += 1;
        self.reads.getter_calls += u64::from(stats.getter_calls);
        self.reads.roots_built += u64::from(stats.roots_built);
    }

    /// Intents of one frame, then the frame.
    fn send(&mut self, r: &mut Replica, intents: Vec<WireIntent>) {
        self.intents += intents.len() as u32;
        let mut intents = intents;
        self.actions
            .apply(&mut intents, &mut self.store, &mut self.sink, self.now);
        self.frame(r, None);
    }

    fn state(&mut self, r: &mut Replica, ns: StateNs, ops: &[StateOp]) {
        self.state_ops += ops.len() as u32;
        // Refusals are expected for the bad ones; the rule is about the replica.
        let _ = self.actions.state_ops(ns, ops, &mut self.store);
        self.frame(r, None);
    }

    fn map(&self) -> Map {
        self.store.get::<Map>().cloned().unwrap_or_default()
    }

    fn own_units(&self) -> Vec<Key> {
        self.map()
            .units
            .iter()
            .filter(|u| u.mine)
            .map(|u| u.key)
            .collect()
    }

    fn own_cities(&self) -> Vec<Key> {
        self.map()
            .cities
            .iter()
            .filter(|c| c.mine)
            .map(|c| c.key)
            .collect()
    }

    fn production(&self) -> Option<Production> {
        self.store.get::<Option<Production>>().cloned().flatten()
    }

    fn selected_unit(&self) -> Option<Key> {
        self.store
            .get::<Option<ui2_model::generated::Unit>>()
            .and_then(|u| u.as_ref().map(|u| u.key))
    }
}

/// The first option of the production root.
fn first_option(p: &Production) -> Option<Key> {
    p.groups
        .iter()
        .flat_map(|g| g.options.iter())
        .map(|o| o.item)
        .next()
}

/// Step 3: the whole model from scratch, twice; the `unit` root of each own unit, `production` and its
/// `detail` of each own city.
fn rebuild_all(h: &mut Host, r: &mut Replica, warm: bool) -> Result<(), String> {
    for pass in 0..2 {
        h.src.reset();
        if warm && pass == 0 {
            let read = r.read().ok_or("the replica cannot be read")?;
            let s = h.src.warm_up(read);
            h.reads.getter_calls += u64::from(s.getter_calls);
        }
        h.frame(r, None);
        let base = h.actions.ui().clone();
        for unit in h.own_units() {
            let mut ui = base.clone();
            ui.selection.unit = Some(unit);
            h.frame(r, Some(&ui));
            if h.selected_unit() == Some(unit) {
                h.reads.unit_roots += 1;
            }
        }
        for city in h.own_cities() {
            let mut ui = base.clone();
            ui.windows.production = WinCity {
                open: true,
                city: Some(city),
            };
            ui.production.focus = None;
            h.frame(r, Some(&ui));
            let Some(p) = h.production().filter(|p| p.city == city) else {
                continue;
            };
            h.reads.production_roots += 1;
            if let Some(item) = first_option(&p) {
                ui.production.focus = Some(item);
                h.frame(r, Some(&ui));
                if h.production().is_some_and(|p| p.detail.is_some()) {
                    h.reads.detail_roots += 1;
                }
            }
        }
        // Back to the actions module's own state.
        h.frame(r, None);
    }
    Ok(())
}

// ---------------------------------------------------------------- the intents

fn wire(name: &str, args: Vec<(&str, Value)>, origin: Origin) -> WireIntent {
    WireIntent {
        name: Arc::from(name),
        args: args.into_iter().map(|(k, v)| (Arc::from(k), v)).collect(),
        origin,
    }
}

fn s(t: &str) -> Value {
    Value::Str(Arc::from(t))
}

fn plot(x: i32, y: i32) -> Value {
    Value::Struct(vec![Value::I32(x), Value::I32(y)])
}

/// Things of the game the arguments name: own and foreign, and keys that name nothing.
#[derive(Default)]
struct Picks {
    own_unit: Option<Key>,
    foreign_unit: Option<Key>,
    own_city: Option<Key>,
    foreign_city: Option<Key>,
    item: Option<Key>,
    tech: Option<Key>,
    note: Option<Key>,
    player: Option<Key>,
    plot: Option<(i32, i32)>,
}

/// A key no entity has.
const NOBODY: Key = Key::new(77, 987_654);

impl Picks {
    fn of(h: &Host) -> Picks {
        let map = h.map();
        let mut p = Picks {
            own_unit: map.units.iter().find(|u| u.mine).map(|u| u.key),
            foreign_unit: map.units.iter().find(|u| !u.mine).map(|u| u.key),
            own_city: map.cities.iter().find(|c| c.mine).map(|c| c.key),
            foreign_city: map.cities.iter().find(|c| !c.mine).map(|c| c.key),
            plot: map
                .units
                .iter()
                .find(|u| u.mine)
                .map(|u| (i32::from(u.plot.x), i32::from(u.plot.y))),
            ..Picks::default()
        };
        p.item = h.production().as_ref().and_then(first_option);
        p.tech = h
            .store
            .get::<Research>()
            .and_then(|r| r.options.iter().next().map(|t| t.id));
        p.note = h
            .store
            .get::<KeyedList<ui2_model::generated::Note>>()
            .and_then(|n| n.iter().next().map(|n| n.id));
        p.player = h
            .store
            .get::<KeyedList<ui2_model::generated::PlayerRow>>()
            .and_then(|l| l.iter().find(|p| !p.me).or(l.iter().next()).map(|p| p.id));
        p
    }

    /// The values tried for an argument of type `ty`: a good one first, then the others.
    fn values(&self, schema: &Schema, ty: &Type) -> Vec<Value> {
        let keys = |a: Option<Key>, b: Option<Key>| -> Vec<Value> {
            [a, b, Some(NOBODY)]
                .into_iter()
                .flatten()
                .map(Value::Key)
                .collect()
        };
        match ty {
            Type::Key(entity) => match entity.as_str() {
                "unit" => keys(self.own_unit, self.foreign_unit),
                "city" => keys(self.own_city, self.foreign_city),
                "item" => keys(self.item, None),
                "tech" => keys(self.tech, None),
                "note" => keys(self.note, None),
                "player" => keys(self.player, None),
                _ => keys(None, None),
            },
            Type::Enum(e) => schema.enum_def(*e).variants.iter().map(|v| s(v)).collect(),
            Type::Struct(_) => {
                let (x, y) = self.plot.unwrap_or((0, 0));
                vec![plot(x, y), plot(x + 1, y), plot(-1, -1)]
            }
            Type::Scalar(Scalar::Bool) => vec![Value::Bool(true), Value::Bool(false)],
            Type::Scalar(Scalar::Str) => vec![s("r17"), s(""), s(&"x".repeat(600))],
            Type::Scalar(_) => vec![Value::I32(0), Value::I32(1), Value::I32(-1), Value::I32(99)],
            Type::Option(inner) => {
                let mut v = self.values(schema, inner);
                v.push(Value::None);
                v
            }
            Type::List(_) => vec![Value::None],
        }
    }
}

/// Every form of one catalogue entry: the good arguments, each argument through all its values, and the
/// malformed ones (an argument missing, unknown, given twice, of another type).
fn forms(def: &IntentDef, picks: &Picks, schema: &Schema) -> Vec<Vec<(String, Value)>> {
    let domains: Vec<Vec<Value>> = def
        .args
        .iter()
        .map(|a| picks.values(schema, &a.ty))
        .collect();
    let base: Vec<(String, Value)> = def
        .args
        .iter()
        .zip(&domains)
        .map(|(a, d)| (a.name.clone(), d.first().cloned().unwrap_or(Value::None)))
        .collect();
    let mut out = vec![base.clone()];
    for (i, d) in domains.iter().enumerate() {
        for v in d.iter().skip(1) {
            let mut f = base.clone();
            f[i].1 = v.clone();
            out.push(f);
        }
        // Missing, of another type, and a key as text that is not a key.
        let mut missing = base.clone();
        missing.remove(i);
        out.push(missing);
        let mut wrong = base.clone();
        wrong[i].1 = Value::F32(0.5);
        out.push(wrong);
        let mut bad_text = base.clone();
        bad_text[i].1 = s("not:a:key");
        out.push(bad_text);
        let mut twice = base.clone();
        twice.push(base[i].clone());
        out.push(twice);
    }
    let mut extra = base;
    extra.push(("r17_extra".into(), Value::I32(1)));
    out.push(extra);
    out
}

fn as_wire(name: &str, args: &[(String, Value)], origin: Origin) -> WireIntent {
    WireIntent {
        name: Arc::from(name),
        args: args
            .iter()
            .map(|(k, v)| (Arc::from(k.as_str()), v.clone()))
            .collect(),
        origin,
    }
}

/// A short story first (so that the orders that need a selection or an open window find them), then every
/// form of every catalogue entry, one frame each.
fn intents(h: &mut Host, r: &mut Replica) {
    let o = Origin::Script;
    let p = Picks::of(h);
    if let Some(u) = p.own_unit {
        h.send(
            r,
            vec![wire("unit.select", vec![("unit", Value::Key(u))], o)],
        );
        let kinds = h.schema.enums().iter().find(|e| e.name == "UnitActionKind");
        for kind in kinds.map(|e| e.variants.clone()).unwrap_or_default() {
            h.send(
                r,
                vec![wire("unit.select", vec![("unit", Value::Key(u))], o)],
            );
            h.send(
                r,
                vec![wire(
                    "unit.action",
                    vec![("unit", Value::Key(u)), ("action", s(&kind))],
                    o,
                )],
            );
        }
        if let Some((x, y)) = p.plot {
            h.send(
                r,
                vec![
                    wire("unit.select", vec![("unit", Value::Key(u))], o),
                    wire(
                        "unit.move",
                        vec![("unit", Value::Key(u)), ("plot", plot(x + 1, y))],
                        o,
                    ),
                    wire(
                        "map.click",
                        vec![("plot", plot(x, y)), ("button", s("left"))],
                        Origin::Map,
                    ),
                    wire(
                        "map.click",
                        vec![("plot", plot(x + 1, y)), ("button", s("right"))],
                        Origin::Map,
                    ),
                ],
            );
        }
    }
    h.send(
        r,
        vec![
            wire("unit.select_next", vec![], Origin::Hotkey),
            wire("unit.select_next", vec![], Origin::Hotkey),
            wire("unit.deselect", vec![], o),
        ],
    );
    if let Some(c) = p.own_city {
        h.send(
            r,
            vec![wire(
                "window.open",
                vec![("window", s("production")), ("city", Value::Key(c))],
                o,
            )],
        );
        if let Some(item) = h.production().as_ref().and_then(first_option) {
            let city = ("city", Value::Key(c));
            let it = ("item", Value::Key(item));
            for batch in [
                vec![wire("production.choose", vec![city.clone(), it.clone()], o)],
                vec![wire(
                    "production.push",
                    vec![city.clone(), it.clone(), ("at", s("back"))],
                    o,
                )],
                vec![wire(
                    "production.push",
                    vec![city.clone(), it.clone(), ("at", s("front"))],
                    o,
                )],
                vec![wire(
                    "production.move",
                    vec![city.clone(), ("from", Value::I32(1)), ("to", Value::I32(0))],
                    o,
                )],
                vec![wire(
                    "production.pop",
                    vec![city.clone(), ("slot", Value::I32(1))],
                    o,
                )],
                vec![wire(
                    "production.replace",
                    vec![city.clone(), it.clone()],
                    o,
                )],
                vec![wire(
                    "production.buy",
                    vec![city.clone(), it.clone(), ("with", s("gold"))],
                    o,
                )],
                vec![wire(
                    "production.buy",
                    vec![city.clone(), it, ("with", s("faith"))],
                    o,
                )],
            ] {
                h.send(r, batch);
            }
        }
        h.send(
            r,
            vec![
                wire("camera.to_city", vec![("city", Value::Key(c))], o),
                wire("window.close", vec![("window", s("production"))], o),
            ],
        );
    }
    if let Some(t) = p.tech {
        h.send(
            r,
            vec![
                wire("window.open", vec![("window", s("research"))], o),
                wire(
                    "research.choose",
                    vec![("tech", Value::Key(t)), ("queue", Value::Bool(false))],
                    o,
                ),
                wire(
                    "research.choose",
                    vec![("tech", Value::Key(t)), ("queue", Value::Bool(true))],
                    o,
                ),
                wire("window.toggle", vec![("window", s("research"))], o),
            ],
        );
    }
    h.send(
        r,
        vec![
            wire(
                "chat.send",
                vec![("text", s("r17 hello")), ("to", s("all"))],
                o,
            ),
            wire(
                "chat.send",
                vec![("text", s("r17 again")), ("to", s("all"))],
                o,
            ),
        ],
    );
    if let Some(n) = p.note {
        h.send(
            r,
            vec![
                wire("note.activate", vec![("note", Value::Key(n))], o),
                wire("note.dismiss", vec![("note", Value::Key(n))], o),
            ],
        );
    }
    if let Some((x, y)) = p.plot {
        h.send(
            r,
            vec![wire("camera.to_plot", vec![("plot", plot(x, y))], o)],
        );
    }
    if let Some(u) = p.own_unit {
        h.send(
            r,
            vec![wire("camera.to_unit", vec![("unit", Value::Key(u))], o)],
        );
    }
    h.send(
        r,
        vec![
            wire("screen.open", vec![("screen", s("tech_tree"))], o),
            wire("turn.end", vec![("force", Value::Bool(true))], o),
        ],
    );
    h.send(r, vec![wire("turn.cancel", vec![], o)]);
    h.send(r, vec![wire("turn.primary", vec![], o)]);

    // Every form of every catalogue entry, the picks taken again (the story opened and selected things).
    let p = Picks::of(h);
    let schema = h.schema;
    for def in schema.intents() {
        for f in forms(def, &p, schema) {
            h.send(r, vec![as_wire(&def.name, &f, o)]);
        }
    }
    // The same good forms with a sink that refuses, and without a way to open screens.
    h.sink.refuse = Some(Refused::NoConnection);
    h.sink.has_screens = false;
    for def in schema.intents() {
        if let Some(f) = forms(def, &p, schema).first() {
            h.send(r, vec![as_wire(&def.name, f, o)]);
        }
    }
    h.sink.refuse = None;
    h.sink.has_screens = true;
    // Names that are not in the catalogue, a mod's intent, an intent from a mod that is not its own.
    h.send(
        r,
        vec![
            wire("r17.nothing", vec![], o),
            wire("mods.r17.ping", vec![], o),
            wire(
                "turn.end",
                vec![("force", Value::Bool(false))],
                Origin::Mod(ModIdx(0)),
            ),
            wire(
                "unit.select",
                vec![("unit", s("1:2"))],
                Origin::Markup { node: 7 },
            ),
        ],
    );
    // The network's answers to what was sent: none of them is the server's game.
    let ids: Vec<_> = h
        .sink
        .sent
        .iter()
        .filter_map(|x| match x {
            Sent::Command { id, .. } => Some(*id),
            _ => None,
        })
        .collect();
    for id in ids {
        h.actions.on_event(
            &ActionEvent::RolledBack {
                id,
                text: "r17".into(),
            },
            h.now,
        );
    }
    h.actions
        .on_event(&ActionEvent::EndTurnCancelled { by_order: true }, h.now);
    h.frame(r, None);
}

/// `StateOp`s on every `js` field of `ui.*` (each value of its type), on the skin's and a mod's `state.*`, and
/// the writes that must be refused.
fn state_ops(h: &mut Host, r: &mut Replica) {
    let p = Picks::of(h);
    let schema = h.schema;
    for entry in schema
        .paths()
        .iter()
        .filter(|e| e.js && e.path.starts_with("ui."))
    {
        let ops: Vec<StateOp> = p
            .values(schema, &entry.ty)
            .into_iter()
            .map(|value| StateOp {
                path: Arc::from(entry.path.as_str()),
                value,
            })
            .collect();
        for op in ops {
            h.state(r, StateNs::Skin, std::slice::from_ref(&op));
        }
    }
    let op = |path: &str, value: Value| StateOp {
        path: Arc::from(path),
        value,
    };
    // Refused: not `js`, game data, unknown, nothing mounted yet.
    h.state(
        r,
        StateNs::Skin,
        &[
            op("ui.selection.unit", Value::None),
            op("ui.windows.menu.open", Value::Bool(true)),
            op("top.gold", Value::I32(1)),
            op("game.turn", Value::I32(1)),
            op("ui.nope", Value::I32(1)),
            op("state.count", Value::I32(1)),
        ],
    );
    let Ok(frag) = Fragment::parse(SKIN_STATE) else {
        return;
    };
    let _ = h
        .actions
        .set_skin(Some((Arc::from("r17"), &frag)), &mut h.store);
    let _ = h.actions.set_mod_state("r17mod", Some(&frag), &mut h.store);
    let writes = [
        op("state.count", Value::I32(5)),
        op("state.name", s("r17")),
        op("state.flag", Value::Bool(true)),
        op("state.count", s("five")),
        op("modstate.other.count", Value::I32(1)),
    ];
    h.state(r, StateNs::Skin, &writes);
    h.state(r, StateNs::Mod(Arc::from("r17mod")), &writes);
    // The skin goes and comes back.
    let _ = h.actions.set_skin(None, &mut h.store);
    h.frame(r, None);
    let _ = h
        .actions
        .set_skin(Some((Arc::from("r17"), &frag)), &mut h.store);
    h.frame(r, None);
}

// ---------------------------------------------------------------- the run

/// What a run found; `to_json` for the browser report.
#[derive(Clone, Debug)]
pub struct Report {
    pub seat: u8,
    pub turn: i32,
    pub before: Mark,
    pub own_units: usize,
    pub own_cities: usize,
    pub reads: Reads,
    pub intents: u32,
    pub state_ops: u32,
    pub commands: usize,
    pub chats: usize,
    pub cameras: usize,
    pub screens: usize,
    /// What the comparison said about the batch of the sensitivity control.
    pub control: String,
    pub control_digest: u64,
}

impl Report {
    pub fn to_json(&self) -> Json {
        json!({
            "seat": self.seat,
            "turn": self.turn,
            "digest": format!("{:016x}", self.before.digest),
            "applied": self.before.stats.applied,
            "applied_jseq": self.before.applied_jseq,
            "own_units": self.own_units,
            "own_cities": self.own_cities,
            "frames": self.reads.frames,
            "getter_calls": self.reads.getter_calls,
            "roots_built": self.reads.roots_built,
            "unit_roots": self.reads.unit_roots,
            "production_roots": self.reads.production_roots,
            "detail_roots": self.reads.detail_roots,
            "intents": self.intents,
            "state_ops": self.state_ops,
            "commands": self.commands,
            "chats": self.chats,
            "cameras": self.cameras,
            "screens": self.screens,
            "control": self.control,
            "control_digest": format!("{:016x}", self.control_digest),
        })
    }
}

/// The units and cities an order names (its first id), if any.
fn order_target(cmd: &Command) -> Option<(bool, i32)> {
    let unit = |id: i32| Some((true, id));
    let city = |id: i32| Some((false, id));
    match *cmd {
        Command::UnitMoveTo { unit: u, .. }
        | Command::UnitRangeAttack { unit: u, .. }
        | Command::UnitFoundCity { unit: u }
        | Command::UnitFortify { unit: u }
        | Command::UnitSleep { unit: u }
        | Command::UnitSkip { unit: u }
        | Command::UnitHeal { unit: u }
        | Command::UnitAutomateBuild { unit: u }
        | Command::UnitAutomateExplore { unit: u }
        | Command::UnitCancel { unit: u }
        | Command::UnitDisband { unit: u }
        | Command::UnitWake { unit: u } => unit(u),
        Command::CitySetProduction { city: c, .. }
        | Command::CityPurchase { city: c, .. }
        | Command::CityRangeAttack { city: c, .. } => city(c),
        _ => None,
    }
}

/// The id part of a unit or city key (`owner:id`).
fn key_id(k: Key) -> Option<i32> {
    k.to_js().split_once(':')?.1.parse().ok()
}

/// Runs the scenario on the fixture `files` as `seat` (the script's seat when `None`).
///
/// # Errors
/// A step failed, the replica changed, or the control batch did not change it.
pub fn run(files: &Files, seat: Option<u8>) -> Result<Report, String> {
    let script: Json = serde_json::from_slice(file(files, "script.json")?)
        .map_err(|e| format!("script.json: {e}"))?;
    // 1. Live on the snapshot.
    let mut r = joined(files, &script, seat)?;
    let read = r
        .read()
        .ok_or("the replica cannot be read after the join")?;
    let seat = read.seat();
    let turn = read.game_info().map_err(|e| e.to_string())?.turn;
    // 2. What must not change.
    let before = Mark::of(&mut r)?;

    let mut h = Host::new(seat, turn);
    // 3. The whole model, twice.
    rebuild_all(&mut h, &mut r, true)?;
    let own_units = h.own_units();
    let own_cities = h.own_cities();
    untouched(&before, &Mark::of(&mut r)?).map_err(|e| format!("after the rebuild: {e}"))?;
    // 4. Every intent and every state write.
    intents(&mut h, &mut r);
    state_ops(&mut h, &mut r);
    untouched(&before, &Mark::of(&mut r)?).map_err(|e| format!("after the intents: {e}"))?;
    // 5. The whole model again.
    rebuild_all(&mut h, &mut r, false)?;

    // 6. The replica, the sink, the reads.
    let after = Mark::of(&mut r)?;
    untouched(&before, &after).map_err(|e| format!("the replica changed: {e}"))?;
    if h.reads.getter_calls == 0 || h.reads.roots_built == 0 {
        return Err(format!("the source did not read the core: {:?}", h.reads));
    }
    if !own_units.is_empty() && h.reads.unit_roots == 0 {
        return Err("no `unit` root was built for an own unit".into());
    }
    if !own_cities.is_empty() && h.reads.production_roots == 0 {
        return Err("no `production` root was built for an own city".into());
    }
    let model_turn = h.store.get::<Game>().map_or(turn, |g| g.turn);
    let own_unit_ids: BTreeSet<i32> = own_units.iter().filter_map(|k| key_id(*k)).collect();
    let own_city_ids: BTreeSet<i32> = own_cities.iter().filter_map(|k| key_id(*k)).collect();
    let (mut commands, mut chats, mut cameras, mut screens) = (0, 0, 0, 0);
    for sent in &h.sink.sent {
        match sent {
            Sent::Command { cmd, turn: t, .. } => {
                commands += 1;
                if *t != model_turn {
                    return Err(format!(
                        "{cmd:?} stamped with turn {t}, the model's is {model_turn}"
                    ));
                }
                match order_target(cmd) {
                    Some((true, id)) if !own_unit_ids.contains(&id) => {
                        return Err(format!("an order for a unit that is not own: {cmd:?}"));
                    }
                    Some((false, id)) if !own_city_ids.contains(&id) => {
                        return Err(format!("an order for a city that is not own: {cmd:?}"));
                    }
                    _ => {}
                }
            }
            Sent::Chat { .. } => chats += 1,
            Sent::Camera(_) => cameras += 1,
            Sent::Screen(_) => screens += 1,
        }
    }
    if commands == 0 || chats == 0 || cameras == 0 || screens == 0 {
        return Err(format!(
            "the intents did not reach the order layer: {commands} orders, {chats} chat lines, {cameras} camera moves, {screens} screens"
        ));
    }

    // 7. Sensitivity: a real batch, through the replica's own entry points, is seen.
    let (epoch, jseq, batch, want) = first_batch(files, &script)?;
    r.on_msg(ReplicaMsg::Journal(GcJournal {
        epoch,
        first_jseq: jseq,
        batches: vec![batch],
    }));
    match r.pump(0) {
        ReplicaStep::Applied { jseq: got } if got == jseq => {}
        other => {
            return Err(format!(
                "control batch {jseq}: {other:?} in {:?}",
                r.state()
            ));
        }
    }
    let moved = Mark::of(&mut r)?;
    if moved.digest != want {
        return Err(format!(
            "control batch {jseq}: digest {:016x}, the server's {want:016x}",
            moved.digest
        ));
    }
    let control = match untouched(&before, &moved) {
        Ok(()) => {
            return Err(format!(
                "control batch {jseq}: the comparison saw no change, so the test proves nothing"
            ));
        }
        Err(e) => e,
    };
    if moved.digest == before.digest || moved.stats.applied != before.stats.applied + 1 {
        return Err(format!("control batch {jseq}: {control}"));
    }

    Ok(Report {
        seat,
        turn,
        before,
        own_units: own_units.len(),
        own_cities: own_cities.len(),
        reads: h.reads,
        intents: h.intents,
        state_ops: h.state_ops,
        commands,
        chats,
        cameras,
        screens,
        control,
        control_digest: moved.digest,
    })
}
