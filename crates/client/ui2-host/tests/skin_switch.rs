//! A skin change on the screen (`V2Ui::set_skin`): the model, the actions module and the UI's own state stay (an
//! open window is open in the new skin), the old skin's `state.*` is set aside and comes back with it, the new
//! skin's scripts start and the old one's stop, a skin that cannot be used changes nothing; a hot reload of the
//! same skin keeps its state.

mod support;

use std::sync::Arc;
use std::time::Instant;

use replica::{ReplicaStep, State};
use support::{FakeNet, game};
use ui2_host::{Env, ReplicaView, V2Frame, V2Init, V2Ui};
use ui2_model::Value;
use ui2_model::r#gen::Ui;
use ui2_model::wire::{Origin, WireIntent};
use ui2_script::testing::MockScriptHost;

struct Rig {
    ui: V2Ui,
    game: replica::testing::FakeRead,
    net: FakeNet,
    now: f64,
}

impl Rig {
    fn new(pkg: Arc<ui2_package::Package>, host: Option<&MockScriptHost>) -> Rig {
        let mut ui = V2Ui::new(
            pkg,
            V2Init {
                env: Env {
                    viewport_px: (1280, 720),
                    ..Env::default()
                },
                scripts: host.map(|h| Box::new(h.clone()) as Box<dyn platform::ScriptHost>),
                ..V2Init::default()
            },
        )
        .expect("the skin loads");
        ui.set_read_budget(u32::MAX);
        let mut rig = Rig {
            ui,
            game: game(),
            net: FakeNet::default(),
            now: 0.0,
        };
        rig.run(&[ReplicaStep::Loaded { turn: 87 }]);
        rig
    }

    fn run(&mut self, steps: &[ReplicaStep]) {
        self.now += 16.0;
        let r = self.ui.frame(V2Frame {
            replica: Some(ReplicaView {
                state: State::Live,
                catching_up: false,
                generation: 1,
                applied_jseq: 0,
                turn: None,
                read: Some(&mut self.game),
            }),
            steps,
            net: Some(&mut self.net),
            events: &[],
            mods: None,
            now_ms: self.now,
        });
        assert!(r.errors.is_empty(), "{:?}", r.errors);
    }

    fn text(&self, starts: &str) -> String {
        self.ui
            .texts()
            .into_iter()
            .find(|t| t.starts_with(starts))
            .unwrap_or_else(|| panic!("no text {starts:?} in {:?}", self.ui.texts()))
    }

    fn sort(&self) -> Option<Value> {
        let f = self.ui.store().resolve("state.prod.sort").ok()?;
        self.ui.store().value(f.id, &[]).cloned()
    }

    fn menu_open(&self) -> bool {
        self.ui.store().get::<Ui>().expect("ui").windows.menu.open
    }
}

fn toggle_menu() -> WireIntent {
    WireIntent {
        name: Arc::from("window.toggle"),
        args: vec![(Arc::from("window"), Value::Str(Arc::from("menu")))],
        origin: Origin::Script,
    }
}

const SCRIPT: &str = "ui.state.on('prod.sort', () => {});";
const READY: &str = r#"{"t":"ready","ctx":"skin","handlers":[],"ms":1}"#;

/// The running skin's worker says it is ready and writes `state.prod.sort`; the next frame applies it.
fn write_sort(rig: &mut Rig, host: &MockScriptHost, v: &str) {
    let w = *host.live().last().expect("a worker");
    host.reply(w, READY);
    host.reply(
        w,
        &format!(r#"{{"t":"out","n":1,"ack":null,"state":[{{"p":"state.prod.sort","v":"{v}"}}]}}"#),
    );
    rig.run(&[]);
}

fn skin(id: &str, sort: &str) -> Arc<ui2_package::Package> {
    support::state_skin(id, sort, Some(SCRIPT))
}

#[test]
fn a_skin_change_keeps_the_windows_and_sets_the_state_aside() {
    let host = MockScriptHost::new();
    let make = || Box::new(host.clone()) as Box<dyn platform::ScriptHost>;
    let mut rig = Rig::new(skin("alpha", "name"), Some(&host));
    rig.ui.push_intent(toggle_menu());
    write_sort(&mut rig, &host, "turns");
    assert!(rig.menu_open());
    assert_eq!(rig.text("by "), "by turns");
    assert_eq!(rig.text("alpha"), "alpha turn 87");

    let t = Instant::now();
    rig.ui
        .set_skin(skin("beta", "cost"), Some(&make))
        .expect("beta loads");
    rig.run(&[]);
    let ms = t.elapsed().as_millis();
    assert!(ms < 1000, "the change took {ms} ms");
    assert_eq!(rig.ui.skin_id(), "beta");
    assert!(rig.menu_open(), "the open window stays open");
    assert_eq!(
        rig.text("beta"),
        "beta turn 87",
        "the new skin reads the model"
    );
    assert_eq!(
        rig.sort(),
        Some(Value::Str("cost".into())),
        "beta's own state"
    );
    assert_eq!(rig.text("by "), "by cost");

    // Back: alpha's state as it left it.
    rig.ui
        .set_skin(skin("alpha", "name"), Some(&make))
        .expect("alpha loads");
    rig.run(&[]);
    assert_eq!(rig.sort(), Some(Value::Str("turns".into())));
    assert_eq!(rig.text("by "), "by turns");
    assert!(rig.menu_open());
}

#[test]
fn the_scripts_follow_the_skin_and_a_skin_that_fails_changes_nothing() {
    let host = MockScriptHost::new();
    let mut rig = Rig::new(
        support::state_skin("alpha", "name", Some("ui.state.on('prod.sort', () => {});")),
        Some(&host),
    );
    let first = host.live();
    assert_eq!(first.len(), 1);
    let make = || Box::new(host.clone()) as Box<dyn platform::ScriptHost>;
    rig.ui
        .set_skin(
            support::state_skin("beta", "cost", Some("ui.on('x', () => {});")),
            Some(&make),
        )
        .expect("beta loads");
    rig.run(&[]);
    let live = host.live();
    assert_eq!(live.len(), 1, "one worker: the old one is gone");
    assert_ne!(live, first);
    host.take_posted();

    // Scripts that read what the model does not have: refused, beta and its worker run on.
    let bad = support::state_skin_with("gamma", "x", Some(("ui.on('x', () => {});", &["nope"])));
    let err = rig.ui.set_skin(bad, Some(&make)).expect_err("gamma fails");
    assert_eq!(err.skin, "gamma");
    rig.run(&[]);
    assert_eq!(rig.ui.skin_id(), "beta");
    assert_eq!(rig.sort(), Some(Value::Str("cost".into())));
    assert_eq!(host.live().len(), 1);
}

// ------------------------------------------------------------------------------------------ the `alt` fixture

/// Where the `alt` skin lies: a second skin for the `ui2-host` playtest to change to on the screen (the build has
/// one skin). `cargo test -p ui2-host --test skin_switch -- --ignored write_the_alt_fixture` writes it again.
fn alt_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/alt")
}

/// `alt`: one HUD layer with an empty box and a state of its own (`state.prod.sort` = "alt"), no text, no fonts.
fn alt() -> Arc<ui2_package::Package> {
    use ui2_package::style::{Display, Len, LenAuto, StyleRecord};
    use ui2_runtime::testkit::{Kit, TNode};
    let mut kit = Kit::new();
    kit.state_default("prod.sort", ui2_package::Lit::Str("alt".into()));
    let mut r = StyleRecord::initial();
    r.display = Display::Block;
    r.height = LenAuto::Len(Len::px(4.0));
    let r = kit.record(r);
    let set = kit.plain(r);
    let part = kit.str("alt.bar");
    kit.template(
        "doc",
        vec![TNode::layer(ui2_package::Layer::Hud, set).child(TNode::el(set).part(part))],
    );
    kit.finish_skin("alt").expect("package")
}

/// The index entry of `alt` and its file name.
fn alt_entry(pkg: &ui2_package::Package) -> (serde_json::Value, String) {
    let full = ui2_host::hash_hex(&pkg.hash());
    let file = format!("alt-{}.ui2", &full[..16]);
    let entry = serde_json::json!({
        "id": "alt",
        "file": file,
        "hash": full,
        "title_key": "TXT_KEY_SKIN_ALT",
        "themes": ["dark", "light"],
        "api": "1.0",
        "default": false,
    });
    (entry, file)
}

#[test]
#[ignore = "writes the fixture"]
fn write_the_alt_fixture() {
    let pkg = alt();
    let (entry, file) = alt_entry(&pkg);
    std::fs::create_dir_all(alt_dir()).expect("dir");
    for old in std::fs::read_dir(alt_dir()).expect("dir").flatten() {
        std::fs::remove_file(old.path()).expect("old fixture");
    }
    std::fs::write(
        alt_dir().join("entry.json"),
        serde_json::to_string_pretty(&entry).expect("json") + "\n",
    )
    .expect("entry");
    std::fs::write(alt_dir().join(file), &pkg.bytes()[..]).expect("package");
}

#[test]
fn the_alt_fixture_is_current_and_runs() {
    let pkg = alt();
    let (entry, file) = alt_entry(&pkg);
    let on_disk: serde_json::Value = serde_json::from_slice(
        &std::fs::read(alt_dir().join("entry.json")).expect("run write_the_alt_fixture"),
    )
    .expect("json");
    assert_eq!(on_disk, entry, "run write_the_alt_fixture");
    let bytes = std::fs::read(alt_dir().join(&file)).expect("run write_the_alt_fixture");
    assert_eq!(
        bytes.as_slice(),
        &pkg.bytes()[..],
        "run write_the_alt_fixture"
    );
    let mut rig = Rig::new(skin("alpha", "name"), None);
    rig.ui.set_skin(pkg, None).expect("alt runs");
    rig.run(&[]);
    assert_eq!(rig.sort(), Some(Value::Str("alt".into())));
}

#[test]
fn a_reload_of_the_same_skin_keeps_its_state_and_the_same_package_does_nothing() {
    let host = MockScriptHost::new();
    let make = || Box::new(host.clone()) as Box<dyn platform::ScriptHost>;
    let pkg = skin("alpha", "name");
    let mut rig = Rig::new(Arc::clone(&pkg), Some(&host));
    write_sort(&mut rig, &host, "turns");
    assert!(!rig.ui.reload(pkg, Some(&make)).expect("same"), "same hash");
    // A new build (another default for the same path): the player's value stays.
    let rebuilt = skin("alpha", "other");
    assert!(rig.ui.reload(rebuilt, Some(&make)).expect("reloads"));
    rig.run(&[]);
    assert_eq!(rig.sort(), Some(Value::Str("turns".into())));
    assert_eq!(rig.text("by "), "by turns");
}
