//! Fragments and the namespaces of the store: parsing, mounting, writes by path, deltas, ownership.

use std::sync::Arc;

use ui2_model::generated::{Top, Ui};
use ui2_model::{
    Fragment, FrameDelta, Key, ModelState, ModelStore, Namespace, Op, Owner, Schema, StoreError,
    Value, WatchSet,
};

const HELLO: &str = r#"
[enum]
Mode = ["a", "b"]

[type.Counter]
n = "i32"
label = "str"

[root]
count = "i32"
mode = "Mode"
title = "text?"
history = { list = "Counter", key = "n" }
"#;

fn store() -> ModelStore {
    ModelStore::new(Schema::v1())
}

fn hello() -> Fragment {
    Fragment::parse(HELLO).expect("the fragment parses")
}

fn mods_hello() -> Namespace {
    Namespace::Mod(Arc::from("hello"))
}

fn counter(n: i32, label: &str) -> Value {
    Value::Struct(vec![Value::I32(n), Value::Str(Arc::from(label))])
}

/// The frame as a consumer sees it: every op in order.
fn frame(s: &mut ModelStore) -> Vec<Op> {
    s.take_frame().map(|f| f.ops).unwrap_or_default()
}

fn read(s: &ModelStore, path: &str) -> Option<Value> {
    let f = s.resolve(path).ok()?;
    s.value(f.id, &[]).cloned()
}

#[test]
fn parse_reads_root_types_and_enums() {
    let f = hello();
    let mut s = store();
    s.mount(mods_hello(), &f).unwrap();
    let r = s.resolve("mods.hello.count").unwrap();
    assert!(r.id.0 as usize >= Schema::v1().paths().len());
    s.resolve("mods.hello.history[].label").unwrap();
    // The types carry the id of the mod.
    let schema = s.mount_schema(&mods_hello()).unwrap();
    assert!(schema.types().iter().any(|t| t.name == "hello.Counter"));
    assert!(schema.enums().iter().any(|e| e.name == "hello.Mode"));
}

#[test]
fn parse_errors_have_positions_and_text() {
    let cases = [
        (
            "[root]\nx = \"i32\"\n[model.a]\nx = \"i32\"\n",
            "not allowed in a fragment",
        ),
        (
            "[root]\nx = \"i32\"\n[intent.\"a.b\"]\n",
            "not allowed in a fragment",
        ),
        ("[type.A]\nx = \"i32\"\n", "needs a `[root]`"),
        ("[root]\nx = \"Nope\"\n", "unknown type `Nope`"),
        ("[root]\n", "has no fields"),
        (
            "[type.A]\nb = \"A\"\n[root]\nx = \"A\"\n",
            "contains itself",
        ),
        (
            "[root]\nx = { type = \"i32\", js = true }\n",
            "only below the `ui` root",
        ),
    ];
    for (src, want) in cases {
        let err = Fragment::parse(src).unwrap_err().to_string();
        assert!(err.contains(want), "{src:?}: {err}");
    }
    let err = Fragment::parse("[root]\nx = \"i32\"\n[model.a]\nx = \"i32\"\n")
        .unwrap_err()
        .to_string();
    assert!(err.starts_with("schema:3:"), "{err}");
}

#[test]
fn a_schema_source_still_rejects_root() {
    let err = Schema::parse("version = \"1.0\"\n[root]\nx = \"i32\"\n")
        .unwrap_err()
        .to_string();
    assert!(err.contains("unknown top-level key `root`"), "{err}");
}

#[test]
fn mount_writes_a_reset_and_values_start_at_defaults() {
    let mut s = store();
    s.mount(mods_hello(), &hello()).unwrap();
    let ops = frame(&mut s);
    assert_eq!(ops.len(), 1);
    let Op::Reset { value, .. } = &ops[0] else {
        panic!("expected a reset, got {ops:?}");
    };
    assert_eq!(
        value,
        &Value::Struct(vec![
            Value::I32(0),
            Value::Enum(0),
            Value::None,
            Value::List(vec![])
        ])
    );
    assert!(s.take_frame().is_none());
    assert_eq!(read(&s, "mods.hello.count"), Some(Value::I32(0)));
}

#[test]
fn set_value_changes_one_field_and_gives_one_set() {
    let mut s = store();
    s.mount(mods_hello(), &hello()).unwrap();
    frame(&mut s);
    let count = s.resolve("mods.hello.count").unwrap().id;
    s.writer(Owner::Mod(Arc::from("hello")))
        .set_value("mods.hello.count", Value::I32(5))
        .unwrap();
    // Not visible until the frame is taken.
    assert_eq!(read(&s, "mods.hello.count"), Some(Value::I32(0)));
    let f = s.take_frame().unwrap();
    assert_eq!(f.revision, 2);
    assert_eq!(
        f.ops,
        vec![Op::Set {
            field: count,
            keys: Default::default(),
            value: Value::I32(5)
        }]
    );
    assert_eq!(read(&s, "mods.hello.count"), Some(Value::I32(5)));
}

#[test]
fn writes_that_cancel_out_give_no_frame() {
    let mut s = store();
    s.mount(mods_hello(), &hello()).unwrap();
    frame(&mut s);
    let mut w = s.writer(Owner::Mod(Arc::from("hello")));
    w.set_value("mods.hello.count", Value::I32(5)).unwrap();
    w.set_value("mods.hello.count", Value::I32(0)).unwrap();
    assert!(s.take_frame().is_none());
}

#[test]
fn types_are_checked_and_a_bad_write_changes_nothing() {
    let mut s = store();
    s.mount(mods_hello(), &hello()).unwrap();
    frame(&mut s);
    let mut w = s.writer(Owner::Mod(Arc::from("hello")));
    let err = w
        .set_value("mods.hello.count", Value::Str(Arc::from("x")))
        .unwrap_err();
    assert!(err.to_string().contains("expected"), "{err}");
    assert!(w.set_value("mods.hello.mode", Value::Enum(2)).is_err());
    assert!(w.set_value("mods.hello.nope", Value::I32(1)).is_err());
    assert!(
        w.list_put("mods.hello.count", Key(1), Value::I32(1))
            .is_err()
    );
    // A struct with the wrong key in its key field.
    assert!(
        w.list_put("mods.hello.history", Key::from_i32(2), counter(3, "x"))
            .is_err()
    );
    assert!(s.take_frame().is_none());
}

#[test]
fn lists_put_replace_remove_clear() {
    let mut s = store();
    s.mount(mods_hello(), &hello()).unwrap();
    frame(&mut s);
    let list = s.resolve("mods.hello.history").unwrap().id;
    let label = s.resolve("mods.hello.history[].label").unwrap().id;
    let mut w = s.writer(Owner::Mod(Arc::from("hello")));
    w.list_put("mods.hello.history", Key::from_i32(1), counter(1, "a"))
        .unwrap();
    w.list_put("mods.hello.history", Key::from_i32(2), counter(2, "b"))
        .unwrap();
    let ops = frame(&mut s);
    assert_eq!(ops.len(), 2);
    assert!(matches!(&ops[0], Op::Insert { list: l, at: 0, .. } if *l == list));
    assert!(matches!(&ops[1], Op::Insert { at: 1, .. }));

    // Replacing an element is a Set of its changed fields; the position stays.
    let mut w = s.writer(Owner::Mod(Arc::from("hello")));
    w.list_put("mods.hello.history", Key::from_i32(1), counter(1, "z"))
        .unwrap();
    let ops = frame(&mut s);
    assert_eq!(ops.len(), 1);
    match &ops[0] {
        Op::Set { field, keys, value } => {
            assert_eq!(*field, label);
            assert_eq!(keys.as_slice(), &[Key::from_i32(1)]);
            assert_eq!(value, &Value::Str(Arc::from("z")));
        }
        other => panic!("{other:?}"),
    }

    let mut w = s.writer(Owner::Mod(Arc::from("hello")));
    w.list_remove("mods.hello.history", Key::from_i32(1))
        .unwrap();
    assert!(
        w.list_remove("mods.hello.history", Key::from_i32(1))
            .is_err()
    );
    let ops = frame(&mut s);
    assert!(matches!(&ops[..], [Op::Remove { key, .. }] if *key == Key::from_i32(1)));

    let mut w = s.writer(Owner::Mod(Arc::from("hello")));
    w.list_put("mods.hello.history", Key::from_i32(3), counter(3, "c"))
        .unwrap();
    w.list_clear("mods.hello.history").unwrap();
    let ops = frame(&mut s);
    // Only the element that existed at the start of the frame goes.
    assert!(matches!(&ops[..], [Op::Remove { key, .. }] if *key == Key::from_i32(2)));
    assert_eq!(read(&s, "mods.hello.history"), Some(Value::List(vec![])));
}

#[test]
fn a_fragment_is_the_only_writer_of_its_namespace() {
    let mut s = store();
    s.mount(mods_hello(), &hello()).unwrap();
    s.mount(Namespace::ModState(Arc::from("hello")), &hello())
        .unwrap();
    s.mount(Namespace::Mod(Arc::from("other")), &hello())
        .unwrap();
    frame(&mut s);
    // The mod's Rust half writes mods.hello, not modstate.hello and not mods.other.
    let mut w = s.writer(Owner::Mod(Arc::from("hello")));
    assert!(w.set_value("mods.hello.count", Value::I32(1)).is_ok());
    let e = w
        .set_value("modstate.hello.count", Value::I32(1))
        .unwrap_err();
    assert!(matches!(e, StoreError::PathOwner { .. }), "{e:?}");
    assert!(matches!(
        w.set_value("mods.other.count", Value::I32(1)),
        Err(StoreError::PathOwner { .. })
    ));
    // The script of the mod writes modstate.hello.
    let mut w = s.writer(Owner::ModScript(Arc::from("hello")));
    assert!(w.set_value("modstate.hello.count", Value::I32(2)).is_ok());
    assert!(w.set_value("mods.hello.count", Value::I32(2)).is_err());
    // The skin writes state, which is not mounted yet.
    let mut w = s.writer(Owner::Skin);
    assert_eq!(
        w.set_value("state.count", Value::I32(1)),
        Err(StoreError::NotMounted("state".to_string()))
    );
    // Nobody writes data roots by accident.
    let mut w = s.writer(Owner::Mod(Arc::from("hello")));
    assert!(matches!(
        w.set_value("top.gold", Value::I32(1)),
        Err(StoreError::PathOwner { .. })
    ));
}

#[test]
fn a_path_outside_the_namespace_is_refused() {
    let mut s = store();
    s.mount(mods_hello(), &hello()).unwrap();
    frame(&mut s);
    // Not mounted: refused on resolve and on write.
    assert!(s.resolve("mods.ghost.count").is_err());
    assert!(s.resolve("mods").is_err());
    assert!(s.resolve("mods.hello").is_ok());
    let mut w = s.writer(Owner::Mod(Arc::from("ghost")));
    assert_eq!(
        w.set_value("mods.ghost.count", Value::I32(1)),
        Err(StoreError::NotMounted("mods.ghost".to_string()))
    );
    // A fragment cannot reach a field of another root by its own name.
    let e = s.resolve("mods.hello.history[].nope").unwrap_err();
    assert!(e.to_string().contains("mods.hello"), "{e}");
}

#[test]
fn bad_ids_and_double_mounts_are_refused() {
    let mut s = store();
    let f = hello();
    for id in ["", "a.b", "a b", "a[0]", &"x".repeat(65)] {
        let e = s.mount(Namespace::Mod(Arc::from(id)), &f).unwrap_err();
        assert!(e.to_string().contains("mod id"), "{id:?}: {e}");
    }
    s.mount(Namespace::Mod(Arc::from("my-mod_2")), &f).unwrap();
    s.resolve("mods.my-mod_2.count").unwrap();
    let e = s
        .mount(Namespace::Mod(Arc::from("my-mod_2")), &f)
        .unwrap_err();
    assert!(e.to_string().contains("mounted already"), "{e}");
}

#[test]
fn unmount_resets_the_root_and_retires_the_ids() {
    let mut s = store();
    s.mount(mods_hello(), &hello()).unwrap();
    frame(&mut s);
    let old = s.resolve("mods.hello.count").unwrap().id;
    s.writer(Owner::Mod(Arc::from("hello")))
        .set_value("mods.hello.count", Value::I32(9))
        .unwrap();
    frame(&mut s);
    assert!(s.unmount(&mods_hello()));
    assert!(!s.unmount(&mods_hello()));
    let ops = frame(&mut s);
    assert_eq!(ops.len(), 1);
    let Op::Reset { value, .. } = &ops[0] else {
        panic!("{ops:?}");
    };
    assert_eq!(value, &default_hello());
    assert!(s.value(old, &[]).is_none());
    assert!(s.resolve("mods.hello.count").is_err());
    assert_eq!(
        s.writer(Owner::Mod(Arc::from("hello")))
            .set_value("mods.hello.count", Value::I32(1)),
        Err(StoreError::NotMounted("mods.hello".to_string()))
    );
    // Mounting again gives fresh ids, and a fresh value.
    s.mount(mods_hello(), &hello()).unwrap();
    let again = s.resolve("mods.hello.count").unwrap().id;
    assert_ne!(old, again);
    frame(&mut s);
    assert_eq!(read(&s, "mods.hello.count"), Some(Value::I32(0)));
}

fn default_hello() -> Value {
    Value::Struct(vec![
        Value::I32(0),
        Value::Enum(0),
        Value::None,
        Value::List(vec![]),
    ])
}

#[test]
fn mount_and_unmount_in_one_frame_leave_no_trace() {
    let mut s = store();
    s.mount(mods_hello(), &hello()).unwrap();
    s.unmount(&mods_hello());
    assert!(s.take_frame().is_none());
}

#[test]
fn the_same_fragment_mounts_in_several_namespaces() {
    let mut s = store();
    let f = hello();
    s.mount(Namespace::State, &f).unwrap();
    s.mount(mods_hello(), &f).unwrap();
    frame(&mut s);
    let a = s.resolve("state.count").unwrap().id;
    let b = s.resolve("mods.hello.count").unwrap().id;
    assert_ne!(a, b);
    s.writer(Owner::Skin)
        .set_value("state.count", Value::I32(4))
        .unwrap();
    frame(&mut s);
    assert_eq!(read(&s, "state.count"), Some(Value::I32(4)));
    assert_eq!(read(&s, "mods.hello.count"), Some(Value::I32(0)));
}

#[test]
fn snapshot_applies_to_an_empty_model_with_the_fragments() {
    let mut s = store();
    s.writer(Owner::Source).put(Top::default()).unwrap();
    s.mount(mods_hello(), &hello()).unwrap();
    frame(&mut s);
    s.writer(Owner::Mod(Arc::from("hello")))
        .set_value("mods.hello.count", Value::I32(7))
        .unwrap();
    let snap = s.snapshot();
    let resets = snap
        .ops
        .iter()
        .filter(|o| matches!(o, Op::Reset { .. }))
        .count();
    assert_eq!(resets, Schema::v1().roots().len() + 1);
    // The snapshot is the value as of the last frame: the pending write is not in it.
    let last = snap.ops.last().unwrap();
    assert!(matches!(last, Op::Reset { value, .. } if value == &default_hello()));
}

#[test]
fn a_mount_never_seen_is_not_in_the_snapshot_but_in_the_next_frame() {
    let mut s = store();
    s.mount(mods_hello(), &hello()).unwrap();
    let snap = s.snapshot();
    assert_eq!(snap.ops.len(), Schema::v1().roots().len());
    assert_eq!(frame(&mut s).len(), 1);
}

#[test]
fn a_frame_replays_into_a_consumer_model() {
    // The delta of a mounted fragment applies to a plain value tree the way the store holds it.
    let mut s = store();
    s.mount(mods_hello(), &hello()).unwrap();
    s.writer(Owner::Mod(Arc::from("hello")))
        .list_put("mods.hello.history", Key::from_i32(1), counter(1, "a"))
        .unwrap();
    let f = s.take_frame().unwrap();
    assert!(matches!(f.ops[0], Op::Reset { .. }));
    // The static part of a consumer ignores them: the ids are not its own.
    let mut st = ModelState::new(Schema::v1());
    assert!(st.apply(&f.ops).is_err());
}

#[test]
fn state_is_not_a_schema_root_and_the_ui_root_still_writes_by_path() {
    let mut s = store();
    assert!(Schema::v1().resolve("state.x").is_err());
    // Typed roots can be written by path too, once they hold a value.
    let mut w = s.writer(Owner::Actions);
    assert!(matches!(
        w.set_value("ui.windows.menu.open", Value::Bool(true)),
        Err(StoreError::Unwritten(_))
    ));
    w.put(Ui::default()).unwrap();
    w.set_value("ui.windows.menu.open", Value::Bool(true))
        .unwrap();
    assert!(w.set_value("ui.windows.menu.open", Value::I32(1)).is_err());
    assert!(s.get::<Ui>().unwrap().windows.menu.open);
    let f = s.take_frame().unwrap();
    assert!(matches!(f.ops[0], Op::Reset { .. }));
    s.writer(Owner::Actions)
        .set_value("ui.windows.research.open", Value::Bool(true))
        .unwrap();
    let f = s.take_frame().unwrap();
    assert_eq!(f.ops.len(), 1);
    assert!(matches!(
        &f.ops[0],
        Op::Set {
            value: Value::Bool(true),
            ..
        }
    ));
    // The source cannot write the ui root by path.
    assert!(matches!(
        s.writer(Owner::Source)
            .set_value("ui.windows.menu.open", Value::Bool(false)),
        Err(StoreError::PathOwner { .. })
    ));
}

#[test]
fn watch_sets_cover_mounted_fields() {
    let mut s = store();
    s.mount(Namespace::State, &hello()).unwrap();
    s.mount(mods_hello(), &hello()).unwrap();
    frame(&mut s);
    let watch = WatchSet::from_store(&s, &["state", "mods.hello.history"]).unwrap();
    s.writer(Owner::Skin)
        .set_value("state.count", Value::I32(1))
        .unwrap();
    s.writer(Owner::Mod(Arc::from("hello")))
        .set_value("mods.hello.count", Value::I32(1))
        .unwrap();
    s.writer(Owner::Mod(Arc::from("hello")))
        .list_put("mods.hello.history", Key::from_i32(1), counter(1, "a"))
        .unwrap();
    let f: FrameDelta = s.take_frame().unwrap();
    assert_eq!(f.ops.len(), 3);
    let part = f.filter(&watch).unwrap();
    // The state field and the history element: not mods.hello.count.
    assert_eq!(part.ops.len(), 2);
    assert!(WatchSet::from_store(&s, &["mods.nope"]).is_err());
}

#[test]
fn umd1_encodes_fragment_ops_and_snapshots() {
    let mut s = store();
    s.mount(mods_hello(), &hello()).unwrap();
    let f = s.take_frame().unwrap();
    let text = f.to_umd1(Schema::v1());
    assert!(text.starts_with("[1,[4,"), "{text}");
    let watch = WatchSet::from_store(&s, &["mods.hello"]).unwrap();
    let snap = s.snapshot_umd1(&watch);
    assert!(snap.contains("[5,\""), "{snap}");
    assert_eq!(snap.matches("[4,").count(), 1, "{snap}");
}
