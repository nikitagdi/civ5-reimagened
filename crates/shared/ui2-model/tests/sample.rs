//! Samples: parsing against the schema, error paths, applying to a store; and the wire types.

use ui2_model::generated as g;
use ui2_model::wire::{
    IslandCmd, IslandOp, ModIdx, ModIntent, Origin, ScreenId, ScriptCtxId, ScriptEvent, StateOp,
    WindowId, WireIntent,
};
use ui2_model::{Key, ModelStore, Op, Sample, SampleError, Schema, Text, Value};

const TOP_SAMPLE: &str = include_str!("fixtures/top-sample.json");

fn parse(text: &str) -> Result<Sample, Vec<String>> {
    Sample::parse(Schema::v1(), text)
}

/// The fixture with one text replaced (a way to make a broken copy).
fn broken(from: &str, to: &str) -> String {
    assert!(TOP_SAMPLE.contains(from), "fixture lacks `{from}`");
    TOP_SAMPLE.replacen(from, to, 1)
}

fn errors_of(text: &str) -> Vec<String> {
    parse(text).expect_err("the sample must be refused")
}

#[test]
fn top_strip_sample_parses_and_applies() {
    let sample = parse(TOP_SAMPLE).expect("fixture parses");
    assert_eq!(sample.roots().len(), 4);
    assert!(sample.anchors().is_empty());
    assert!(sample.state().is_empty());

    let mut store = ModelStore::new(Schema::v1());
    sample.apply(&mut store).expect("applies");
    let frame = store.take_frame().expect("a frame");
    assert_eq!(frame.ops.len(), 4);
    assert!(frame.ops.iter().all(|op| matches!(op, Op::Reset { .. })));

    let top = store.get::<g::Top>().expect("top");
    assert_eq!(top.science_rate, 48);
    assert_eq!(top.gold, 312);
    assert_eq!(top.gold_rate, 21);
    assert_eq!(top.happiness, 7);
    assert_eq!(top.golden_age.progress, 412);
    assert_eq!(top.golden_age.threshold, 650);
    assert_eq!(top.culture_rate, 31);
    assert_eq!(top.policy_turns, Some(6));
    assert_eq!(top.faith, 140);
    assert_eq!(top.faith_rate, 9);
    assert_eq!(top.tourism, Some(4));
    assert_eq!(top.resources.len(), 3);
    assert_eq!(top.resources.get(Key::new(0, 2)).unwrap().available, 4);

    let game = store.get::<g::Game>().expect("game");
    assert_eq!(game.turn, 87);
    assert_eq!(game.year, 1250);
    assert_eq!(game.year_text, Text::key("year.ad", &["1250"]));
    assert_eq!(game.winner, None);

    let research = store.get::<g::Research>().expect("research");
    let current = research.current.as_ref().expect("current tech");
    assert_eq!(current.turns, Some(3));
    assert_eq!(current.fraction.to_bits(), 0.62f32.to_bits());
    assert!(research.next.as_ref().expect("next tech").turns.is_none());
    assert!(research.known);

    // Applying it again changes nothing.
    sample.apply(&mut store).unwrap();
    assert!(store.take_frame().is_none());
}

#[test]
fn applied_roots_reach_the_dynamic_mirror() {
    let schema = Schema::v1();
    let mut store = ModelStore::new(schema);
    parse(TOP_SAMPLE).unwrap().apply(&mut store).unwrap();
    store.take_frame().unwrap();
    let gold = schema.resolve("top.gold").unwrap();
    assert_eq!(store.value(gold.id, &[]), Some(&Value::I32(312)));
}

#[test]
fn a_field_outside_the_schema_is_an_error_with_its_path() {
    let errs = errors_of(&broken("\"gold\": 312", "\"golds\": 312"));
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(
        errs[0].starts_with("model.top.golds: no such field"),
        "{errs:?}"
    );

    let errs = errors_of(&broken(
        "\"icon\": \"icon:RESOURCE_ATLAS:2\"",
        "\"icons\": \"x\"",
    ));
    assert!(
        errs[0].starts_with("model.top.resources[1].icons: no such field"),
        "{errs:?}"
    );
}

#[test]
fn wrong_types_and_values_name_the_value() {
    let errs = errors_of(&broken("\"gold\": 312", "\"gold\": \"312\""));
    assert!(
        errs[0].starts_with("model.top.gold: expected an integer, found a string"),
        "{errs:?}"
    );

    let errs = errors_of(&broken("\"mood\": \"content\"", "\"mood\": \"cheerful\""));
    assert!(
        errs[0].starts_with("model.top.mood: `cheerful` is not a variant of Mood"),
        "{errs:?}"
    );

    let errs = errors_of(&broken("\"id\": 3,", "\"id\": 70000,"));
    assert!(
        errs[0].starts_with("model.game.era.id: 70000 is out of range for i16"),
        "{errs:?}"
    );

    let errs = errors_of(&broken("\"id\": \"0:2\"", "\"id\": \"two\""));
    assert!(
        errs[0].starts_with("model.top.resources[1].id: key `two`"),
        "{errs:?}"
    );
}

#[test]
fn list_keys_must_be_present_and_unique() {
    let errs = errors_of(&broken("\"id\": \"0:2\", ", ""));
    assert!(
        errs[0].starts_with("model.top.resources[1].id: list key is missing"),
        "{errs:?}"
    );

    let errs = errors_of(&broken("\"id\": \"0:2\"", "\"id\": \"0:1\""));
    assert!(
        errs[0].starts_with("model.top.resources[1].id: duplicate list key 0:1"),
        "{errs:?}"
    );
}

#[test]
fn unknown_roots_and_sections() {
    let errs = errors_of(&broken(
        "\"research\": {",
        "\"nonsense\": {} , \"research\": {",
    ));
    assert!(
        errs[0].starts_with("model.nonsense: unknown root"),
        "{errs:?}"
    );

    let errs = errors_of("{ \"modle\": {} }");
    assert!(errs[0].starts_with("modle: unknown key"), "{errs:?}");

    let errs = errors_of("[1]");
    assert_eq!(errs, ["sample: expected an object"]);
    let errs = errors_of("{");
    assert!(errs[0].starts_with("sample: "), "{errs:?}");
}

#[test]
fn every_problem_is_reported() {
    let text = broken("\"gold\": 312", "\"golds\": 1")
        .replace("\"mood\": \"content\"", "\"mood\": \"x\"")
        .replace("\"turn\": 87", "\"turn\": true");
    let errs = errors_of(&text);
    assert_eq!(errs.len(), 3, "{errs:?}");
}

#[test]
fn state_ui_and_anchors() {
    let text = r#"{
        "model": {},
        "state": { "prod": { "sort": "order" } },
        "anchors": { "city:3": [1320, 640.5] }
    }"#;
    let s = parse(text).expect("parses");
    assert!(s.roots().is_empty());
    assert_eq!(s.state().len(), 1);
    assert_eq!(s.anchors().get("city:3"), Some(&(1320.0, 640.5)));

    // `ui` is the `ui` root: unknown fields are refused with their path, an empty object writes nothing.
    let errs = errors_of(r#"{ "ui": { "windowz": {} } }"#);
    assert!(errs[0].starts_with("ui.windowz: no such field"), "{errs:?}");
    assert!(s.ui().is_none());
    assert!(parse(r#"{ "ui": {} }"#).unwrap().ui().is_none());
    let errs = errors_of(r#"{ "model": { "ui": {} } }"#);
    assert!(
        errs[0].starts_with("model.ui: the `ui` root is written under"),
        "{errs:?}"
    );
    let errs = errors_of(r#"{ "anchors": { "a": [1] } }"#);
    assert!(
        errs[0].starts_with("anchors.a: expected [x, y]"),
        "{errs:?}"
    );
    let errs = errors_of(r#"{ "state": 3 }"#);
    assert_eq!(errs, ["state: expected an object"]);
}

#[test]
fn missing_fields_take_defaults_and_options_may_be_null() {
    let s = parse(r#"{ "model": { "top": { "gold": 5, "tourism": null } } }"#).unwrap();
    let mut store = ModelStore::new(Schema::v1());
    s.apply(&mut store).unwrap();
    let top = store.get::<g::Top>().unwrap();
    assert_eq!(top.gold, 5);
    assert_eq!(top.tourism, None);
    assert_eq!(top.faith, 0);
    assert!(top.resources.is_empty());
    // Roots that the sample does not mention stay unwritten.
    assert!(store.get::<g::Game>().is_none());
}

#[test]
fn a_sample_checked_against_another_schema_is_refused() {
    let other = Box::leak(Box::new(
        Schema::parse("version = \"1.0\"\n[model.top]\ngold = \"i32\"\n").unwrap(),
    ));
    let s = Sample::parse(other, r#"{ "model": { "top": { "gold": 1 } } }"#).unwrap();
    let mut store = ModelStore::new(Schema::v1());
    assert_eq!(s.apply(&mut store), Err(SampleError::SchemaMismatch));
}

#[test]
fn texts_come_in_two_forms() {
    let s = parse(
        r#"{ "model": { "game": { "year_text": "Year", "era": { "id": 1, "name": { "k": "era.x" } } } } }"#,
    )
    .unwrap();
    let mut store = ModelStore::new(Schema::v1());
    s.apply(&mut store).unwrap();
    let game = store.get::<g::Game>().unwrap();
    assert_eq!(game.year_text, Text::raw("Year"));
    assert_eq!(game.era.name, Text::key("era.x", &[]));

    let errs = errors_of(r#"{ "model": { "game": { "year_text": { "k": "a", "z": 1 } } } }"#);
    assert!(
        errs[0].starts_with("model.game.year_text.z: text has only"),
        "{errs:?}"
    );
}

#[test]
fn wire_types_carry_plain_values() {
    let intent = WireIntent {
        name: "screen.open".into(),
        args: vec![("screen".into(), Value::Enum(0))],
        origin: Origin::Markup { node: 7 },
    };
    assert_eq!(intent.clone(), intent);
    assert_ne!(Origin::Script, Origin::Hotkey);
    assert_ne!(Origin::Mod(ModIdx(1)), Origin::Mod(ModIdx(2)));
    assert_ne!(Origin::Map, Origin::Script);

    let op = StateOp {
        path: "state.prod.sort".into(),
        value: Value::Str("turns".into()),
    };
    assert_eq!(op.clone(), op);

    let mi = ModIntent {
        mod_id: "hello".into(),
        name: "say".into(),
        args: vec![],
    };
    assert_eq!(mi.name.as_ref(), "say");

    let ev = ScriptEvent {
        ctx: ScriptCtxId::Mod("hello".into()),
        name: "pick".into(),
        arg: Value::None,
        node_path: "parts/top.html:12:3".into(),
    };
    assert_ne!(ev.ctx, ScriptCtxId::Skin);

    let cmd = IslandCmd {
        island: "ticks".into(),
        key: "t3".into(),
        op: IslandOp::Cls("on".into(), true),
    };
    assert_eq!(cmd.clone(), cmd);
    let _ = IslandOp::Text(Value::None);
    let _ = IslandOp::Style("width".into(), Value::None);
    let _ = IslandOp::Show(false);
}

#[test]
fn window_and_screen_ids_round_trip_their_names() {
    assert_eq!(WindowId::ALL.len(), 5);
    assert_eq!(ScreenId::ALL.len(), 11);
    for w in WindowId::ALL {
        assert_eq!(WindowId::from_name(w.as_str()), Some(*w));
    }
    for s in ScreenId::ALL {
        assert_eq!(ScreenId::from_name(s.as_str()), Some(*s));
    }
    assert_eq!(WindowId::NotesLog.as_str(), "notes_log");
    assert_eq!(ScreenId::TechTree.as_str(), "tech_tree");
    assert_eq!(ScreenId::from_name("production"), None);
}
