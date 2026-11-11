//! The store: frames, merging, ownership, snapshots, watch sets.

mod common;

use std::sync::OnceLock;

use common::{Rng, TEST_SCHEMA, gen_value, leak, mutate};
use ui2_model::generated as g;
use ui2_model::{
    FrameDelta, Key, ModelState, ModelStore, Op, Owner, Schema, StoreError, Type, Value, WatchSet,
    diff_root,
};

fn v1() -> &'static Schema {
    Schema::v1()
}

fn res(id: u64, avail: i32) -> g::Resource {
    g::Resource {
        id: Key(id),
        available: avail,
        total: avail,
        ..Default::default()
    }
}

fn top(gold: i32, list: Vec<g::Resource>) -> g::Top {
    g::Top {
        gold,
        resources: ui2_model::KeyedList::from_vec(list).unwrap(),
        ..Default::default()
    }
}

#[test]
fn first_frame_is_a_reset_then_deltas() {
    let mut st = ModelStore::new(v1());
    assert!(st.take_frame().is_none());
    assert_eq!(st.revision(), 0);
    assert!(st.get::<g::Top>().is_none());

    st.writer(Owner::Source)
        .put(top(5, vec![res(1, 2)]))
        .unwrap();
    let f = st.take_frame().expect("a frame");
    assert_eq!(f.revision, 1);
    assert!(matches!(f.ops.as_slice(), [Op::Reset { .. }]));
    assert_eq!(st.get::<g::Top>().unwrap().gold, 5);

    // The same value again: nothing.
    st.writer(Owner::Source)
        .put(top(5, vec![res(1, 2)]))
        .unwrap();
    assert!(st.take_frame().is_none());
    assert_eq!(st.revision(), 1);

    // A change: Set of one field plus the list update.
    st.writer(Owner::Source)
        .put(top(6, vec![res(1, 3), res(2, 1)]))
        .unwrap();
    let f = st.take_frame().unwrap();
    assert_eq!(f.revision, 2);
    assert_eq!(f.ops.len(), 4); // gold, available and total of 1, the new element
    let gold = v1().resolve("top.gold").unwrap().id;
    assert!(matches!(&f.ops[0], Op::Set { field, value: Value::I32(6), .. } if *field == gold));
    assert!(f.ops.iter().any(|o| matches!(o, Op::Insert { .. })));
}

#[test]
fn several_puts_in_a_frame_merge() {
    let mut st = ModelStore::new(v1());
    st.writer(Owner::Source).put(top(1, vec![])).unwrap();
    st.take_frame().unwrap();

    let mut w = st.writer(Owner::Source);
    w.put(top(2, vec![res(1, 1)])).unwrap();
    w.put(top(3, vec![res(1, 1), res(2, 2)])).unwrap();
    w.put(top(4, vec![res(2, 2)])).unwrap();
    let f = st.take_frame().unwrap();
    // One Set of gold (4), one Insert of key 2: no trace of the intermediate states.
    assert_eq!(f.ops.len(), 2, "{:#?}", f.ops);

    // Going back to the start value within one frame leaves no change at all.
    let mut w = st.writer(Owner::Source);
    w.put(top(9, vec![])).unwrap();
    w.put(top(4, vec![res(2, 2)])).unwrap();
    assert!(st.take_frame().is_none());
    assert_eq!(st.revision(), 2);
}

#[test]
fn only_the_owner_writes_a_root() {
    let mut st = ModelStore::new(v1());
    let e = st
        .writer(Owner::Actions)
        .try_put(top(1, vec![]))
        .unwrap_err();
    assert!(
        matches!(e, StoreError::WrongOwner { root: "top", .. }),
        "{e}"
    );
    let e = st
        .writer(Owner::Skin)
        .try_put(g::Meta::default())
        .unwrap_err();
    assert!(matches!(e, StoreError::WrongOwner { .. }));
    let e = st
        .writer(Owner::Mod("hello".into()))
        .try_put(g::Meta::default())
        .unwrap_err();
    assert!(e.to_string().contains("belongs to Source"));
    // Nothing was written.
    assert!(st.take_frame().is_none());
    assert!(st.get::<g::Top>().is_none());
}

#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "belongs to Source")]
fn put_of_a_foreign_root_panics_in_debug() {
    let mut st = ModelStore::new(v1());
    let _ = st.writer(Owner::Actions).put(g::Meta::default());
}

#[test]
fn reads_see_the_state_of_the_last_frame() {
    let mut st = ModelStore::new(v1());
    let gold = v1().resolve("top.gold").unwrap().id;
    let list = v1().resolve("top.resources").unwrap().id;
    let avail = v1().resolve("top.resources[].available").unwrap().id;
    st.writer(Owner::Source)
        .put(top(7, vec![res(3, 30), res(4, 40)]))
        .unwrap();
    // Not taken yet: the mirror still has the defaults, the typed root is current.
    assert_eq!(st.value(gold, &[]), Some(&Value::I32(0)));
    assert_eq!(st.get::<g::Top>().unwrap().gold, 7);
    st.take_frame().unwrap();
    assert_eq!(st.value(gold, &[]), Some(&Value::I32(7)));
    assert_eq!(
        st.list_keys(list, &[]).unwrap().collect::<Vec<_>>(),
        [Key(3), Key(4)]
    );
    assert_eq!(st.value(avail, &[Key(4)]), Some(&Value::I32(40)));
    assert_eq!(st.value(avail, &[Key(5)]), None);
}

#[test]
fn snapshot_applies_to_an_empty_state_and_matches() {
    let s = v1();
    let mut st = ModelStore::new(s);
    // Before any write: all defaults.
    let snap = st.snapshot();
    assert_eq!(snap.ops.len(), s.roots().len());
    st.writer(Owner::Source)
        .put(top(7, vec![res(3, 30)]))
        .unwrap();
    st.writer(Owner::Source)
        .put(g::Meta {
            live: true,
            ..Default::default()
        })
        .unwrap();
    st.take_frame().unwrap();
    let snap = st.snapshot();
    assert_eq!(snap.revision, st.revision());
    let mut copy = ModelState::new(s);
    copy.apply(&snap.ops).unwrap();
    for (i, _) in s.roots().iter().enumerate() {
        let id = ui2_model::RootId(i as u16);
        let field = s.root(id).field;
        assert_eq!(copy.value(field, &[]), st.value(field, &[]));
    }
}

#[test]
fn deltas_keep_a_remote_copy_in_step() {
    // A consumer that only sees take_frame results and applies them stays equal to the store.
    let s = v1();
    let mut st = ModelStore::new(s);
    let mut copy = ModelState::new(s);
    let rid = s.root_by_name("top").unwrap();
    let ty = Type::Struct(s.root(rid).ty);
    let mut rng = Rng(5);
    let mut cur = gen_value(s, &ty, &mut rng);
    for _ in 0..300 {
        let t = <g::Top as ui2_model::Model>::from_value(&cur).unwrap();
        st.writer(Owner::Source).put(t).unwrap();
        if let Some(f) = st.take_frame() {
            copy.apply(&f.ops).unwrap();
        }
        assert_eq!(copy.root(rid), &cur);
        cur = mutate(s, &ty, &cur, &mut rng, None);
    }
}

fn test_schema() -> &'static Schema {
    static S: OnceLock<&'static Schema> = OnceLock::new();
    S.get_or_init(|| leak(Schema::parse(TEST_SCHEMA).unwrap()))
}

#[test]
fn watch_sets_filter_deltas() {
    let s = v1();
    let mut st = ModelStore::new(s);
    st.writer(Owner::Source)
        .put(top(1, vec![res(1, 1)]))
        .unwrap();
    st.writer(Owner::Source).put(g::Meta::default()).unwrap();
    st.take_frame().unwrap();
    st.writer(Owner::Source)
        .put(top(2, vec![res(1, 5), res(2, 2)]))
        .unwrap();
    st.writer(Owner::Source)
        .put(g::Meta {
            live: true,
            ..Default::default()
        })
        .unwrap();
    let f = st.take_frame().unwrap();
    assert_eq!(f.ops.len(), 5);

    let gold_only = WatchSet::from_prefixes(s, &["top.gold"]).unwrap();
    let g = f.filter(&gold_only).unwrap();
    assert_eq!(g.revision, f.revision);
    assert_eq!(g.ops.len(), 1);

    let list = WatchSet::from_prefixes(s, &["top.resources"]).unwrap();
    assert_eq!(f.filter(&list).unwrap().ops.len(), 3);

    let meta = WatchSet::from_prefixes(s, &["meta"]).unwrap();
    assert_eq!(f.filter(&meta).unwrap().ops.len(), 1);

    let both = WatchSet::from_prefixes(s, &["meta.live", "top.resources[].available"]).unwrap();
    // live, available, and the Insert (a new element carries the watched field).
    assert_eq!(f.filter(&both).unwrap().ops.len(), 3);

    // Nothing of interest: None.
    let other = WatchSet::from_prefixes(s, &["meta.connected"]).unwrap();
    assert!(f.filter(&other).is_none());
    // A Reset of a root is kept when any field of it is watched.
    let snap = st.snapshot();
    assert_eq!(snap.filter(&other).unwrap().ops.len(), 1);
    // Unknown prefix.
    assert!(WatchSet::from_prefixes(s, &["top.nope"]).is_err());
}

#[test]
fn a_changed_struct_touches_a_watched_field_inside_it() {
    let s = test_schema();
    let rid = s.root_by_name("a").unwrap();
    let ty = Type::Struct(s.root(rid).ty);
    let base = ui2_model::default_value(s, &ty);
    let idx = s
        .type_def(s.root(rid).ty)
        .fields
        .iter()
        .position(|f| f.name == "opt_node")
        .unwrap();
    let node = gen_value(
        s,
        &Type::Struct(match &s.type_def(s.root(rid).ty).fields[idx].ty {
            Type::Option(i) => i.struct_id().unwrap(),
            _ => unreachable!(),
        }),
        &mut Rng(9),
    );
    let Value::Struct(mut fs) = base.clone() else {
        unreachable!()
    };
    fs[idx] = node;
    let mut ops = Vec::new();
    diff_root(s, rid, &base, &Value::Struct(fs), &mut ops);
    assert_eq!(ops.len(), 1);
    let d = FrameDelta { revision: 1, ops };
    let w = WatchSet::from_prefixes(s, &["a.opt_node.name"]).unwrap();
    assert!(
        d.filter(&w).is_some(),
        "Set of the whole struct concerns a field inside"
    );
    let w = WatchSet::from_prefixes(s, &["a.n"]).unwrap();
    assert!(d.filter(&w).is_none());
}

#[test]
fn snapshot_umd1_has_the_hash_and_only_watched_roots() {
    let s = v1();
    let mut st = ModelStore::new(s);
    st.writer(Owner::Source).put(top(7, vec![])).unwrap();
    st.take_frame().unwrap();
    let w = WatchSet::from_prefixes(s, &["top"]).unwrap();
    let text = st.snapshot_umd1(&w);
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v[0], 1);
    assert_eq!(v[1][0], 5);
    assert_eq!(v[1][1], format!("{:016x}", s.hash()));
    assert_eq!(v.as_array().unwrap().len(), 3);
    assert_eq!(v[2][0], 4);
}
