//! The order of `V2Ui::frame` around the actions module, seen from what each step leaves for the next one:
//!
//! - the network's events are taken before the intents of the same frame;
//! - the intents are applied before the source, which reads the `ui` root they changed in the same frame;
//! - `reconcile` runs after the source, on what the source wrote in the same frame, and before `take_frame`, so
//!   its changes reach the runtime in that frame's delta;
//! - a dismissed notification stays dismissed while the core has it;
//! - the scripts' answers are drained before the markup's state and intents are applied, and their intents before
//!   the host's; the frame's delta is posted to the worker after it was taken, in the same frame;
//! - a skin without scripts starts no worker, and an idle frame sends a worker nothing;
//! - a skin whose scripts fail while loading reports a fatal load error and stops its worker.

mod support;

use std::sync::Arc;

use protocol::{CmdId, Command, CommandOutcome};
use replica::testing::FakeRead;
use replica::{ReplicaStep, State};
use support::{FakeNet, game};
use ui2_actions::{ActionEvent, Sent};
use ui2_host::{Env, ReplicaView, V2Frame, V2FrameReport, V2Init, V2Ui};
use ui2_model::r#gen::{Note, Ui, Unit as UnitRoot};
use ui2_model::wire::{Origin, WireIntent};
use ui2_model::{Key, KeyedList, Value};
use ui2_script::testing::MockScriptHost;

fn ukey(id: u32) -> Key {
    Key::new(0, id)
}

fn arg(name: &str, v: Value) -> (Arc<str>, Value) {
    (Arc::from(name), v)
}

fn text(s: &str) -> Value {
    Value::Str(Arc::from(s))
}

fn intent(name: &str, args: Vec<(Arc<str>, Value)>) -> WireIntent {
    WireIntent {
        name: Arc::from(name),
        args,
        origin: Origin::Markup { node: 1 },
    }
}

fn select(unit: u32) -> WireIntent {
    intent("unit.select", vec![arg("unit", text(&ukey(unit).to_js()))])
}

struct Rig {
    ui: V2Ui,
    game: FakeRead,
    net: FakeNet,
    jseq: u64,
    now: f64,
}

impl Rig {
    /// A running skin over the game, after a first frame.
    fn new() -> Rig {
        Rig::of(support::strip(), None)
    }

    /// A running skin of `pkg` whose scripts run on `scripts`, after a first frame.
    fn of(pkg: Arc<ui2_package::Package>, scripts: Option<&MockScriptHost>) -> Rig {
        let mut ui = V2Ui::new(
            pkg,
            V2Init {
                env: Env {
                    viewport_px: (1280, 720),
                    ..Env::default()
                },
                scripts: scripts.map(|h| Box::new(h.clone()) as Box<dyn platform::ScriptHost>),
                ..V2Init::default()
            },
        )
        .expect("the skin loads");
        // These tests look at one frame's model: every root is built in the first frame.
        ui.set_read_budget(u32::MAX);
        let mut net = FakeNet::default();
        net.session.connected = true;
        net.session.me = Some(0);
        let mut rig = Rig {
            ui,
            game: game(),
            net,
            jseq: 0,
            now: 0.0,
        };
        rig.run(&[ReplicaStep::Loaded { turn: 87 }], &[]);
        rig
    }

    fn run(&mut self, steps: &[ReplicaStep], events: &[ActionEvent]) -> V2FrameReport {
        self.now += 16.0;
        let view = ReplicaView {
            state: State::Live,
            catching_up: false,
            generation: 1,
            applied_jseq: self.jseq,
            read: Some(&mut self.game),
        };
        let r = self.ui.frame(V2Frame {
            replica: Some(view),
            steps,
            net: Some(&mut self.net),
            events,
            mods: None,
            now_ms: self.now,
        });
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        r
    }

    /// A frame with the intents queued before it.
    fn with(&mut self, intents: Vec<WireIntent>, events: &[ActionEvent]) -> V2FrameReport {
        for i in intents {
            self.ui.push_intent(i);
        }
        self.run(&[], events)
    }

    /// A frame after the game changed (one journal batch).
    fn batch(&mut self) -> V2FrameReport {
        self.jseq += 1;
        let steps = [ReplicaStep::Applied { jseq: self.jseq }];
        self.run(&steps, &[])
    }

    fn model_ui(&self) -> &Ui {
        self.ui.store().get::<Ui>().expect("the ui root")
    }

    fn unit_root(&self) -> Option<UnitRoot> {
        self.ui
            .store()
            .get::<Option<UnitRoot>>()
            .cloned()
            .expect("the unit root")
    }

    fn notes(&self) -> Vec<Key> {
        self.ui
            .store()
            .get::<KeyedList<Note>>()
            .expect("the notes root")
            .iter()
            .map(|n| n.id)
            .collect()
    }

    fn dismissed(&self) -> Vec<Key> {
        self.ui.actions().dismissed().iter().copied().collect()
    }

    fn last_order(&self) -> CmdId {
        match self.net.sink.sent.last() {
            Some(Sent::Command { id, .. }) => *id,
            other => panic!("no order sent: {other:?}"),
        }
    }
}

#[test]
fn the_ui_root_exists_from_the_first_frame() {
    let rig = Rig::new();
    assert_eq!(*rig.model_ui(), *rig.ui.actions().ui());
    assert_eq!(rig.notes(), [Key::from_i32(2), Key::from_i32(3)]);
}

#[test]
fn the_menu_button_opens_its_window_in_the_same_frame() {
    let mut rig = Rig::new();
    let toggle = || intent("window.toggle", vec![arg("window", text("menu"))]);
    let r = rig.with(vec![toggle()], &[]);
    assert_eq!(r.intents, 1);
    assert!(rig.model_ui().windows.menu.open);
    assert!(
        r.delta_ops > 0,
        "the change reached the runtime in this frame"
    );
    // Nothing left over: the next frame has no delta.
    assert_eq!(rig.run(&[], &[]).delta_ops, 0);
    rig.with(vec![toggle()], &[]);
    assert!(!rig.model_ui().windows.menu.open);
    assert!(rig.net.sink.sent.is_empty(), "windows are local");
}

#[test]
fn events_are_taken_before_the_intents_of_their_frame() {
    let mut rig = Rig::new();
    rig.with(vec![select(1)], &[]);
    assert_eq!(rig.model_ui().selection.unit, Some(ukey(1)));
    // A new game and a new selection in one frame: the new game clears the old selection first, the intent then
    // selects (the units are the same in the new game).
    let r = rig.with(vec![select(2)], &[ActionEvent::NewGame]);
    assert_eq!(r.events, 1);
    assert_eq!(rig.model_ui().selection.unit, Some(ukey(2)));
    assert!(rig.model_ui().errors.is_empty(), "no refusal");
    // The new game rebuilt every root from the replica in the same frame.
    assert!(r.source.roots_built >= 3, "{:?}", r.source);
}

#[test]
fn the_source_reads_the_ui_that_apply_wrote_in_the_same_frame() {
    let mut rig = Rig::new();
    assert_eq!(rig.unit_root(), None);
    let r = rig.with(vec![select(1)], &[]);
    assert_eq!(
        rig.unit_root().map(|u| u.key),
        Some(ukey(1)),
        "the selected unit's root is built in the frame of the selection"
    );
    assert!(r.source.roots_built >= 1);
}

#[test]
fn an_order_leaves_stamped_and_its_pending_mark_lights_and_settles() {
    let mut rig = Rig::new();
    rig.with(vec![select(1)], &[]);
    let go = intent(
        "unit.move",
        vec![
            arg("unit", text(&ukey(1).to_js())),
            arg("plot", Value::Struct(vec![Value::I32(12), Value::I32(5)])),
        ],
    );
    rig.with(vec![go], &[]);
    assert_eq!(
        rig.net.sink.sent,
        [Sent::Command {
            id: 1,
            cmd: Command::UnitMoveTo {
                unit: 1,
                x: 12,
                y: 5
            },
            turn: 87,
        }],
        "stamped with the turn of the model"
    );
    // The row the actions module wrote is the row the source looks up: the unit shows its order waiting.
    assert_eq!(rig.model_ui().pending.len(), 1);
    assert!(rig.unit_root().expect("selected").pending);

    // The answer and the `reconcile` that settles it run in one frame.
    let id = rig.last_order();
    rig.run(
        &[],
        &[ActionEvent::Answered {
            id,
            outcome: CommandOutcome::Applied,
        }],
    );
    assert!(rig.model_ui().pending.is_empty());
    // The source sees the settled row in the next frame.
    rig.run(&[], &[]);
    assert!(!rig.unit_root().expect("selected").pending);
}

#[test]
fn reconcile_judges_what_the_source_wrote_in_the_same_frame() {
    let mut rig = Rig::new();
    rig.with(vec![select(2)], &[]);
    assert_eq!(rig.model_ui().selection.unit, Some(ukey(2)));
    // The unit dies: the batch that removes it from the map also deselects it, in one delta.
    rig.game.units.retain(|u| u.id != 2);
    let r = rig.batch();
    assert_eq!(rig.model_ui().selection.unit, None);
    assert!(r.delta_ops > 0);
    assert_eq!(rig.run(&[], &[]).delta_ops, 0, "nothing was left for later");
}

#[test]
fn a_dismissed_note_stays_dismissed_until_the_core_drops_it() {
    let mut rig = Rig::new();
    let dismiss = intent(
        "note.dismiss",
        vec![arg("note", text(&Key::from_i32(2).to_js()))],
    );
    rig.with(vec![dismiss], &[]);
    assert_eq!(rig.dismissed(), [Key::from_i32(2)]);
    assert_eq!(rig.notes(), [Key::from_i32(3)]);
    // Frames and batches go by while the core still has the note: it does not come back.
    for _ in 0..3 {
        rig.batch();
        rig.run(&[], &[]);
        assert_eq!(rig.dismissed(), [Key::from_i32(2)]);
        assert_eq!(rig.notes(), [Key::from_i32(3)]);
    }
    // The core drops it: the dismissal is forgotten.
    rig.game.notes.retain(|n| n.id != 2);
    rig.batch();
    assert!(rig.dismissed().is_empty());
    assert_eq!(rig.notes(), [Key::from_i32(3)]);
}

#[test]
fn without_a_network_orders_are_refused_with_a_toast() {
    let mut rig = Rig::new();
    rig.with(vec![select(1)], &[]);
    rig.ui.push_intent(intent(
        "unit.move",
        vec![
            arg("unit", text(&ukey(1).to_js())),
            arg("plot", Value::Struct(vec![Value::I32(12), Value::I32(5)])),
        ],
    ));
    let view = ReplicaView {
        state: State::Live,
        catching_up: false,
        generation: 1,
        applied_jseq: rig.jseq,
        read: Some(&mut rig.game),
    };
    rig.ui.frame(V2Frame {
        replica: Some(view),
        steps: &[],
        net: None,
        events: &[],
        mods: None,
        now_ms: 1000.0,
    });
    let ui = rig.model_ui();
    assert!(ui.pending.is_empty());
    let why: Vec<_> = ui.errors.iter().map(|t| t.why.name()).collect();
    assert_eq!(why, ["no_connection"]);
}

#[test]
fn intents_refused_for_their_form_are_counted_not_kept() {
    let mut rig = Rig::new();
    let r = rig.with(vec![intent("window.toggle", vec![])], &[]);
    assert_eq!((r.intents, r.refused), (1, 1));
    // Reported once, then the list is empty again.
    let r = rig.run(&[], &[]);
    assert_eq!(r.refused, 0);
}

// ---------------------------------------------------------------- scripts

const READY: &str = r#"{"t":"ready","ctx":"skin","handlers":["sort"],"ms":1}"#;

fn json(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("{e}: {text}"))
}

/// The skin's worker after it said `ready` and a frame took that in; nothing posted is left.
fn scripted_rig() -> (Rig, MockScriptHost, platform::WorkerId) {
    let host = MockScriptHost::new();
    let pkg = support::scripted(&[("js/hud.js", "ui.on('sort', () => {});")], &["top"]);
    let mut rig = Rig::of(pkg, Some(&host));
    let w = host.live()[0];
    // The init, then the first frame's delta (the game loaded), which the worker reads after its files.
    let posted: Vec<_> = host.take_posted().iter().map(|(_, t)| json(t)).collect();
    let kinds: Vec<_> = posted.iter().map(|m| m["t"].clone()).collect();
    assert_eq!(kinds, [json(r#""init""#), json(r#""frame""#)]);
    host.reply(w, READY);
    rig.run(&[], &[]);
    assert_eq!(
        rig.ui.script_status(),
        Some(ui2_host::ScriptStatus::Running)
    );
    host.take_posted();
    (rig, host, w)
}

/// The frames posted to the worker since the last call.
fn frames(host: &MockScriptHost) -> Vec<serde_json::Value> {
    host.take_posted()
        .into_iter()
        .map(|(_, t)| json(&t))
        .filter(|m| m["t"] == "frame")
        .collect()
}

#[test]
fn a_skin_without_scripts_starts_no_worker_and_does_no_script_work() {
    let host = MockScriptHost::new();
    let mut rig = Rig::of(support::strip(), Some(&host));
    assert!(
        host.live().is_empty(),
        "no worker for a skin without scripts"
    );
    assert_eq!(rig.ui.script_stats(), None);
    for _ in 0..3 {
        let r = rig.batch();
        assert_eq!(r.scripts, None);
        assert_eq!(r.js_main_ms, 0.0);
        let c = r.counters();
        assert!(
            c.iter()
                .filter(|(n, _)| n.starts_with("ui2.js."))
                .all(|(_, v)| *v == 0.0),
            "{c:?}"
        );
    }
    assert!(host.take_posted().is_empty());
}

#[test]
fn an_idle_frame_sends_the_worker_nothing() {
    let (mut rig, host, _) = scripted_rig();
    for _ in 0..3 {
        let r = rig.run(&[], &[]);
        assert_eq!(r.delta_ops, 0);
        assert!(r.scripts.is_some());
    }
    assert!(host.take_posted().is_empty(), "no message without a change");
}

#[test]
fn a_script_state_write_reaches_the_model_and_the_worker_in_the_frame_that_drains_it() {
    let (mut rig, host, w) = scripted_rig();
    host.reply(
        w,
        r#"{"t":"out","n":1,"ack":null,"state":[{"p":"ui.production.tab","v":"wonders"}]}"#,
    );
    // Not before the frame drains it.
    assert_eq!(
        rig.model_ui().production.tab,
        ui2_model::r#gen::ProdTab::All
    );
    let r = rig.run(&[], &[]);
    assert_eq!(
        rig.model_ui().production.tab,
        ui2_model::r#gen::ProdTab::Wonders,
        "applied before the delta was taken"
    );
    assert!(r.delta_ops > 0, "the change is in this frame's delta");
    // Posted after the delta was taken: the worker's frame carries this frame's revision and says the write was
    // applied.
    let sent = frames(&host);
    assert_eq!(sent.len(), 1, "{sent:?}");
    assert_eq!(sent[0]["model"][0], rig.ui.store().revision());
    assert_eq!(sent[0]["applied"], 1);
    let stats = r.scripts.clone().expect("scripts run");
    assert_eq!(
        (stats.posted, stats.received),
        (3, 2),
        "init and two frames; ready and the out"
    );
    let c = r.counters();
    let get = |n: &str| c.iter().find(|(k, _)| *k == n).map(|(_, v)| *v);
    assert_eq!(get("ui2.js.posted"), Some(3.0));
    assert_eq!(get("ui2.js.received"), Some(2.0));
    assert!(get("ui2.js.main_ms").is_some() && get("ui2.js.worker_ms").is_some());
}

#[test]
fn script_intents_come_after_the_markups_and_before_the_hosts_and_the_source() {
    let (mut rig, host, w) = scripted_rig();
    // The script opens the menu and selects a unit; the host's intent of the same frame closes the menu. Applied in
    // that order, the menu ends closed; the selected unit's root is built by the source of the same frame.
    host.reply(
        w,
        &format!(
            r#"{{"t":"out","n":1,"ack":null,"intents":[{{"name":"window.open","args":{{"window":"menu"}}}},{{"name":"unit.select","args":{{"unit":"{}"}}}}]}}"#,
            ukey(1).to_js()
        ),
    );
    let close = intent("window.close", vec![arg("window", text("menu"))]);
    let r = rig.with(vec![close], &[]);
    assert_eq!(r.intents, 3);
    assert_eq!(r.refused, 0);
    assert!(
        !rig.model_ui().windows.menu.open,
        "the host's close came last"
    );
    assert_eq!(rig.model_ui().selection.unit, Some(ukey(1)));
    assert_eq!(rig.unit_root().map(|u| u.key), Some(ukey(1)));
}

#[test]
fn script_settings_go_to_the_actions_module() {
    let (mut rig, host, w) = scripted_rig();
    host.reply(
        w,
        r#"{"t":"out","n":1,"ack":null,"settings":[{"key":"layout","value":{"a":1}},{"key":"bad key!","value":1}]}"#,
    );
    // The supervisor passes keys on as written; the actions module keeps the good one and refuses the other.
    let r = rig.run(&[], &[]);
    assert_eq!(r.refused, 1, "the key breaks the limits");
}

#[test]
fn a_new_window_size_reaches_the_scripts_once() {
    let (mut rig, host, _) = scripted_rig();
    rig.ui.set_env(Env {
        viewport_px: (1600, 900),
        ..Env::default()
    });
    rig.run(&[], &[]);
    let sent = frames(&host);
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["env"]["viewport"], json("[1600,900]"));
    rig.run(&[], &[]);
    assert!(frames(&host).is_empty());
}

#[test]
fn island_commands_go_to_the_runtime_and_a_refused_one_is_reported_once() {
    let (mut rig, host, w) = scripted_rig();
    let out = r#"{"t":"out","n":1,"ack":null,"cmds":[{"island":"clock","key":"t","op":"text","value":"12:00"}]}"#;
    host.reply(w, out);
    rig.now += 16.0;
    let r = rig.ui.frame(V2Frame {
        replica: None,
        steps: &[],
        net: Some(&mut rig.net),
        events: &[],
        mods: None,
        now_ms: rig.now,
    });
    // The skin has no island `clock`: the runtime refuses the command, the frame reports it; nothing is dropped.
    assert_eq!(r.dropped_output, 0);
    assert_eq!(r.errors.len(), 1, "{:?}", r.errors);
    assert!(
        r.errors[0].path.contains("island clock#t"),
        "{:?}",
        r.errors
    );
    // The same command again: refused, not reported again.
    let out2 = out.replace(r#""n":1"#, r#""n":2"#);
    host.reply(w, &out2);
    assert!(rig.run(&[], &[]).errors.is_empty());
}

#[test]
fn the_warm_up_reads_the_slow_calls_once_and_its_cost_is_counted() {
    let mut rig = Rig::new();
    let mut game = game();
    let stats = rig.ui.warm_up(&mut game);
    assert!(stats.getter_calls > 0, "{stats:?}");
    assert!(stats.spent_units > 0, "{stats:?}");
    // A frame's counters carry the cost units next to the reads.
    let r = rig.batch();
    let c = r.counters();
    let get = |n: &str| c.iter().find(|(k, _)| *k == n).map(|(_, v)| *v);
    assert_eq!(
        get("ui2.source.units"),
        Some(f64::from(r.source.spent_units))
    );
    assert!(get("ui2.source.reads").is_some());
}

#[test]
fn scripts_failing_to_load_are_a_fatal_load_error() {
    let host = MockScriptHost::new();
    let pkg = support::scripted(&[("js/hud.js", "x();")], &[]);
    let mut rig = Rig::of(pkg, Some(&host));
    let w = host.live()[0];
    host.reply(
        w,
        r#"{"t":"fail","ctx":"skin","error":{"message":"x is not defined","file":"js/hud.js","line":1}}"#,
    );
    let r = rig.run(&[], &[]);
    let fatal = r.fatal.expect("fatal");
    assert_eq!(fatal.skin, "kit");
    assert!(
        matches!(&fatal.failure, ui2_host::LoadFailure::Scripts(why) if why.contains("x is not defined")),
        "{fatal}"
    );
    assert!(host.live().is_empty(), "the worker is gone");
    assert_eq!(r.scripts, None);
    // Once: the next frames run without scripts.
    let r = rig.run(&[], &[]);
    assert_eq!(r.fatal, None);
    assert_eq!(rig.ui.script_status(), None);
}

#[test]
fn scripts_reading_unknown_model_paths_refuse_the_skin() {
    let host = MockScriptHost::new();
    let pkg = support::scripted(&[("js/hud.js", "")], &["top.nothing"]);
    let e = V2Ui::new(
        pkg,
        V2Init {
            scripts: Some(Box::new(host.clone())),
            ..V2Init::default()
        },
    )
    .err()
    .expect("refused");
    assert!(
        matches!(e.failure, ui2_host::LoadFailure::Scripts(_)),
        "{e}"
    );
    assert!(host.live().is_empty());
}

#[test]
fn without_workers_the_skin_runs_on_its_markup() {
    let host = MockScriptHost::new();
    host.refuse_spawn(true);
    let pkg = support::scripted(&[("js/hud.js", "")], &[]);
    let mut rig = Rig::of(pkg, Some(&host));
    let r = rig.run(&[], &[]);
    assert_eq!((r.scripts, r.fatal), (None, None));
}
