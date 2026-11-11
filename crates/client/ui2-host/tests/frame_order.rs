//! The order of `V2Ui::frame` around the actions module, seen from what each step leaves for the next one:
//!
//! - the network's events are taken before the intents of the same frame;
//! - the intents are applied before the source, which reads the `ui` root they changed in the same frame;
//! - `reconcile` runs after the source, on what the source wrote in the same frame, and before `take_frame`, so
//!   its changes reach the runtime in that frame's delta;
//! - a dismissed notification stays dismissed while the core has it.

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
        let ui = V2Ui::new(
            support::strip(),
            V2Init {
                env: Env {
                    viewport_px: (1280, 720),
                    ..Env::default()
                },
                ..V2Init::default()
            },
        )
        .expect("the skin loads");
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
