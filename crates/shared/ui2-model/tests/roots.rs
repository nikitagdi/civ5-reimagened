//! Optional roots and list roots of the v1 schema, through the store, the state mirror, samples and UMD1.

use ui2_model::generated as g;
use ui2_model::{
    Key, KeyedList, ModelState, ModelStore, Op, Owner, Sample, Schema, Value, WatchSet,
};

fn s() -> &'static Schema {
    Schema::v1()
}

fn note(id: u64, text: &str) -> g::Note {
    g::Note {
        id: Key(id),
        message: text.into(),
        ..Default::default()
    }
}

fn unit(name: &str) -> g::Unit {
    g::Unit {
        key: Key::new(1, 4),
        name: name.into(),
        ..Default::default()
    }
}

/// Replay the frame on a plain mirror, as a remote consumer would, and compare it with the store's mirror.
fn replay(store: &mut ModelStore, state: &mut ModelState) {
    if let Some(frame) = store.take_frame() {
        state.apply(&frame.ops).expect("the frame applies");
    }
    for r in s().roots() {
        assert_eq!(
            state.value(r.field, &[]),
            store.value(r.field, &[]),
            "{}",
            r.name
        );
    }
}

#[test]
fn an_optional_root_appears_changes_and_goes_away() {
    let mut store = ModelStore::new(s());
    let mut mirror = ModelState::new(s());
    let field = s().resolve("unit").unwrap().id;
    let name = s().resolve("unit.name").unwrap().id;

    // First write: a reset of the root.
    store
        .writer(Owner::Source)
        .put(Some(unit("Settler")))
        .unwrap();
    let f = store.take_frame().unwrap();
    assert!(matches!(f.ops.as_slice(), [Op::Reset { .. }]));
    mirror.apply(&f.ops).unwrap();

    // A field of the present value changes: one `Set` of that field.
    store
        .writer(Owner::Source)
        .put(Some(unit("Worker")))
        .unwrap();
    let f = store.take_frame().unwrap();
    assert_eq!(f.ops.len(), 1);
    assert!(matches!(&f.ops[0], Op::Set { field: id, .. } if *id == name));
    mirror.apply(&f.ops).unwrap();
    assert_eq!(
        store.value(name, &[]).unwrap(),
        &Value::Str("Worker".into())
    );

    // It goes away: a `Set` of the root to none; the fields below read as absent.
    store.writer(Owner::Source).put(None::<g::Unit>).unwrap();
    let f = store.take_frame().unwrap();
    assert!(matches!(
        f.ops.as_slice(),
        [Op::Set { field: id, value: Value::None, .. }] if *id == field
    ));
    mirror.apply(&f.ops).unwrap();
    assert_eq!(store.value(field, &[]).unwrap(), &Value::None);
    assert!(store.value(name, &[]).is_none());
    assert!(mirror.value(name, &[]).is_none());

    // And back.
    store
        .writer(Owner::Source)
        .put(Some(unit("Scout")))
        .unwrap();
    replay(&mut store, &mut mirror);
    let typed = store.get::<Option<g::Unit>>().unwrap();
    assert_eq!(typed.as_ref().unwrap().name.as_ref(), "Scout");
}

#[test]
fn a_list_root_inserts_moves_removes_and_updates_by_key() {
    let mut store = ModelStore::new(s());
    let mut mirror = ModelState::new(s());
    let list = s().resolve("notes").unwrap().id;
    let message = s().resolve("notes[].message").unwrap().id;
    let put = |store: &mut ModelStore, ids: &[(u64, &str)]| {
        let items = ids.iter().map(|(i, t)| note(*i, t)).collect();
        store
            .writer(Owner::Source)
            .put(KeyedList::from_vec(items).unwrap())
            .unwrap();
    };

    put(&mut store, &[(1, "a"), (2, "b")]);
    replay(&mut store, &mut mirror);

    // Insert at the end and update an element: ops on the root field and on the element field.
    put(&mut store, &[(1, "a"), (2, "B"), (3, "c")]);
    let f = store.take_frame().unwrap();
    assert!(
        f.ops
            .iter()
            .any(|o| matches!(o, Op::Insert { list: l, key, .. } if *l == list && *key == Key(3)))
    );
    assert!(f.ops.iter().any(
        |o| matches!(o, Op::Set { field, keys, .. } if *field == message && keys.as_slice() == [Key(2)])
    ));
    mirror.apply(&f.ops).unwrap();

    // Reorder and remove.
    put(&mut store, &[(3, "c"), (1, "a")]);
    replay(&mut store, &mut mirror);
    let keys: Vec<Key> = store.list_keys(list, &[]).unwrap().collect();
    assert_eq!(keys, [Key(3), Key(1)]);
    assert_eq!(
        mirror.list_keys(list, &[]).unwrap().collect::<Vec<_>>(),
        keys
    );
    assert_eq!(
        store.value(message, &[Key(1)]).unwrap(),
        &Value::Str("a".into())
    );
    assert!(store.value(message, &[Key(2)]).is_none());
}

#[test]
fn snapshot_and_umd1_carry_the_new_roots() {
    let mut store = ModelStore::new(s());
    store
        .writer(Owner::Source)
        .put(KeyedList::from_vec(vec![note(7, "hi")]).unwrap())
        .unwrap();
    store
        .writer(Owner::Source)
        .put(Some(unit("Archer")))
        .unwrap();
    store.take_frame().unwrap();

    // A snapshot applies to an empty mirror and gives the same values.
    let mut mirror = ModelState::new(s());
    mirror.apply(&store.snapshot().ops).unwrap();
    for r in s().roots() {
        assert_eq!(
            mirror.value(r.field, &[]),
            store.value(r.field, &[]),
            "{}",
            r.name
        );
    }

    // The watch set of a list root keeps the root and its elements.
    let watch = WatchSet::from_prefixes(s(), &["notes"]).unwrap();
    let text = store.snapshot_umd1(&watch);
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    let rows = v.as_array().unwrap();
    // The revision, the hello op and the reset of `notes` only.
    assert_eq!(rows.len(), 3, "{text}");
    assert_eq!(rows[2][0], 4);
    assert_eq!(rows[2][2][0][0], "0:7");
}

#[test]
fn samples_fill_list_and_optional_roots() {
    let text = r#"{
      "model": {
        "players": [ { "id": "0:1", "leader": "A", "civ": "B", "color": 255, "human": true, "me": true } ],
        "unit": { "key": "1:4", "name": "Worker", "act": { "fortify": { "shown": true, "enabled": true } },
                  "actions": [ { "kind": "fortify", "enabled": true, "supported": true } ] },
        "notes": [ { "id": "0:9", "category": "production", "message": "m", "summary": "s", "turn": 3 } ],
        "chat": []
      }
    }"#;
    let sample = Sample::parse(s(), text).unwrap_or_else(|e| panic!("{e:?}"));
    assert_eq!(sample.roots().len(), 4);
    let mut store = ModelStore::new(s());
    sample.apply(&mut store).unwrap();
    store.take_frame().unwrap();

    let players = store.get::<KeyedList<g::PlayerRow>>().unwrap();
    assert_eq!(players.len(), 1);
    assert!(players[0].human);
    let u = store.get::<Option<g::Unit>>().unwrap().as_ref().unwrap();
    assert!(u.act.fortify.shown);
    assert!(!u.act.sleep.shown);
    assert_eq!(u.actions[0].kind, g::UnitActionKind::Fortify);
    assert_eq!(
        store.get::<KeyedList<g::Note>>().unwrap()[0].category,
        g::NoteCat::Production
    );
    // Wrong shapes are named by path.
    let errs = Sample::parse(s(), r#"{ "model": { "notes": { "id": 1 } } }"#).unwrap_err();
    assert!(
        errs[0].starts_with("model.notes: expected an array"),
        "{errs:?}"
    );
    let errs = Sample::parse(s(), r#"{ "model": { "unit": [] } }"#).unwrap_err();
    assert!(
        errs[0].starts_with("model.unit: expected an object"),
        "{errs:?}"
    );
    let errs =
        Sample::parse(s(), r#"{ "model": { "notes": [ { "message": "x" } ] } }"#).unwrap_err();
    assert!(
        errs[0].contains("notes[0].id: list key is missing"),
        "{errs:?}"
    );
}
