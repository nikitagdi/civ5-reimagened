//! Diff and apply on dynamic values: list reordering, the round-trip property, error handling.

mod common;

use std::sync::OnceLock;

use common::{Rng, TEST_SCHEMA, gen_value, leak, mutate};
use ui2_model::{
    ApplyError, FieldId, Key, ListStep, ModelState, Op, Schema, Type, Value, diff_root, list_steps,
    validate,
};

fn schema() -> &'static Schema {
    static S: OnceLock<&'static Schema> = OnceLock::new();
    S.get_or_init(|| leak(Schema::parse(TEST_SCHEMA).unwrap()))
}

fn keys(v: &[u64]) -> Vec<Key> {
    v.iter().map(|&k| Key(k)).collect()
}

/// Apply steps to a list of keys the way a consumer does.
fn run(old: &[Key], steps: &[ListStep]) -> Vec<Key> {
    let mut cur = old.to_vec();
    for s in steps {
        match *s {
            ListStep::Remove(k) => cur.retain(|c| *c != k),
            ListStep::Insert { at, key, .. } => cur.insert(at as usize, key),
            ListStep::Move { key, to } => {
                let p = cur.iter().position(|c| *c == key).unwrap();
                let k = cur.remove(p);
                cur.insert(to as usize, k);
            }
        }
    }
    cur
}

fn moves(steps: &[ListStep]) -> usize {
    steps
        .iter()
        .filter(|s| matches!(s, ListStep::Move { .. }))
        .count()
}

/// Length of the longest increasing subsequence, the slow way.
fn lis_len(seq: &[usize]) -> usize {
    let mut best = vec![1usize; seq.len()];
    for i in 0..seq.len() {
        for j in 0..i {
            if seq[j] < seq[i] {
                best[i] = best[i].max(best[j] + 1);
            }
        }
    }
    best.into_iter().max().unwrap_or(0)
}

#[test]
fn list_steps_simple_cases() {
    let a = keys(&[1, 2, 3]);
    assert!(list_steps(&a, &a).is_empty());
    // Insert in the middle and at both ends.
    let s = list_steps(&a, &keys(&[0, 1, 9, 2, 3, 4]));
    assert_eq!(run(&a, &s), keys(&[0, 1, 9, 2, 3, 4]));
    assert_eq!(moves(&s), 0);
    // Remove.
    let s = list_steps(&a, &keys(&[2]));
    assert_eq!(s[0], ListStep::Remove(Key(1)));
    assert_eq!(run(&a, &s), keys(&[2]));
    // The first element goes to the end: one move, not two.
    let s = list_steps(&a, &keys(&[2, 3, 1]));
    assert_eq!(moves(&s), 1);
    assert_eq!(run(&a, &s), keys(&[2, 3, 1]));
    // The last goes to the front: one move.
    let s = list_steps(&a, &keys(&[3, 1, 2]));
    assert_eq!(moves(&s), 1);
    assert_eq!(run(&a, &s), keys(&[3, 1, 2]));
    // Reversal of n keeps one element and moves n - 1.
    let r = keys(&[1, 2, 3, 4, 5]);
    let rr = keys(&[5, 4, 3, 2, 1]);
    let s = list_steps(&r, &rr);
    assert_eq!(moves(&s), 4);
    assert_eq!(run(&r, &s), rr);
}

#[test]
fn list_steps_are_correct_and_minimal_on_random_lists() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..3000 {
        let n = rng.below(12);
        let mut pool: Vec<u64> = (0..16).collect();
        let mut old = Vec::new();
        for _ in 0..n {
            let i = rng.below(pool.len());
            old.push(Key(pool.swap_remove(i)));
        }
        // New list: shuffle a random subset of old plus some fresh keys.
        let mut new: Vec<Key> = old.iter().copied().filter(|_| rng.chance(75)).collect();
        for i in (1..new.len()).rev() {
            if rng.chance(40) {
                let j = rng.below(i + 1);
                new.swap(i, j);
            }
        }
        for _ in 0..rng.below(3) {
            if pool.is_empty() {
                break;
            }
            let i = rng.below(pool.len());
            let at = rng.below(new.len() + 1);
            new.insert(at, Key(pool.swap_remove(i)));
        }
        let steps = list_steps(&old, &new);
        assert_eq!(run(&old, &steps), new, "old {old:?} new {new:?}");
        // Minimal: moves = retained - LIS of the retained elements' positions in the new list.
        let seq: Vec<usize> = old
            .iter()
            .filter_map(|k| new.iter().position(|n| n == k))
            .collect();
        assert_eq!(
            moves(&steps),
            seq.len() - lis_len(&seq),
            "old {old:?} new {new:?}"
        );
    }
}

fn root_a() -> (ui2_model::RootId, usize) {
    let s = schema();
    (s.root_by_name("a").unwrap(), 0)
}

fn apply_diff(a: &Value, b: &Value) -> (Value, Vec<Op>) {
    let s = schema();
    let (rid, _) = root_a();
    let mut ops = Vec::new();
    diff_root(s, rid, a, b, &mut ops);
    let mut st = ModelState::new(s);
    st.apply(&[Op::Reset {
        root: rid,
        value: a.clone(),
    }])
    .unwrap();
    st.apply(&ops).unwrap();
    (st.root(rid).clone(), ops)
}

#[test]
fn apply_of_diff_gives_the_new_value() {
    let s = schema();
    let (rid, _) = root_a();
    let ty = Type::Struct(s.root(rid).ty);
    let mut rng = Rng(42);
    let mut total_ops = 0usize;
    for _ in 0..4000 {
        let a = gen_value(s, &ty, &mut rng);
        let b = if rng.chance(15) {
            gen_value(s, &ty, &mut rng)
        } else {
            mutate(s, &ty, &a, &mut rng, None)
        };
        validate(s, &ty, &a).unwrap();
        validate(s, &ty, &b).unwrap();
        let (got, ops) = apply_diff(&a, &b);
        total_ops += ops.len();
        assert_eq!(got, b, "ops: {ops:#?}");
        if a == b {
            assert!(ops.is_empty());
        }
    }
    assert!(total_ops > 4000, "the random pairs should produce changes");
}

#[test]
fn scalars_options_and_nested_structs() {
    let s = schema();
    let (rid, _) = root_a();
    let ty = Type::Struct(s.root(rid).ty);
    let a = common::gen_value(s, &ty, &mut Rng(7));
    let Value::Struct(mut fs) = a.clone() else {
        unreachable!()
    };
    // n: scalar.
    fs[0] = Value::I32(12345);
    // flag: None <-> Some.
    fs[1] = if fs[1] == Value::None {
        Value::Bool(true)
    } else {
        Value::None
    };
    let b = Value::Struct(fs);
    let (got, ops) = apply_diff(&a, &b);
    assert_eq!(got, b);
    assert_eq!(ops.len(), 2);
    assert!(ops.iter().all(|o| matches!(o, Op::Set { .. })));
    let n = s.resolve("a.n").unwrap().id;
    assert!(matches!(&ops[0], Op::Set { field, value: Value::I32(12345), .. } if *field == n));
}

#[test]
fn optional_struct_appears_updates_and_disappears() {
    let s = schema();
    let (rid, _) = root_a();
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
        &mut Rng(3),
    );
    let with = |v: Value| {
        let Value::Struct(mut fs) = base.clone() else {
            unreachable!()
        };
        fs[idx] = v;
        Value::Struct(fs)
    };
    // None -> Some: one Set with the whole struct.
    let (got, ops) = apply_diff(&with(Value::None), &with(node.clone()));
    assert_eq!(got, with(node.clone()));
    assert_eq!(ops.len(), 1);
    assert!(matches!(
        &ops[0],
        Op::Set {
            value: Value::Struct(_),
            ..
        }
    ));
    // Some -> Some with one field changed: a Set of that field only.
    let Value::Struct(mut nf) = node.clone() else {
        unreachable!()
    };
    nf[0] = Value::Key(Key(999));
    let changed = Value::Struct(nf);
    let (got, ops) = apply_diff(&with(node.clone()), &with(changed.clone()));
    assert_eq!(got, with(changed));
    assert_eq!(ops.len(), 1);
    let key_field = s.resolve("a.opt_node.key").unwrap().id;
    assert!(
        matches!(&ops[0], Op::Set { field, value: Value::Key(Key(999)), .. } if *field == key_field)
    );
    // Some -> None.
    let (got, ops) = apply_diff(&with(node), &with(Value::None));
    assert_eq!(got, with(Value::None));
    assert!(matches!(
        &ops[..],
        [Op::Set {
            value: Value::None,
            ..
        }]
    ));
}

#[test]
fn nested_list_element_update_carries_the_chain_of_keys() {
    let s = schema();
    let (rid, _) = root_a();
    let ty = Type::Struct(s.root(rid).ty);
    let a_idx = s
        .type_def(s.root(rid).ty)
        .fields
        .iter()
        .position(|f| f.name == "nodes")
        .unwrap();
    // One node with one leaf.
    let leaf = |id: i32, w: f32| Value::Struct(vec![Value::I32(id), Value::None, Value::F32(w)]);
    let node = |k: u64, leaves: Vec<(Key, Value)>| {
        Value::Struct(vec![
            Value::Key(Key(k)),
            Value::Text(ui2_model::Text::raw("n")),
            Value::Enum(0),
            Value::List(leaves),
            Value::None,
        ])
    };
    let base = ui2_model::default_value(s, &ty);
    let with = |nodes: Vec<(Key, Value)>| {
        let Value::Struct(mut fs) = base.clone() else {
            unreachable!()
        };
        fs[a_idx] = Value::List(nodes);
        Value::Struct(fs)
    };
    let a = with(vec![(
        Key(5),
        node(5, vec![(ui2_model::Key::from_i32(1), leaf(1, 0.5))]),
    )]);
    let b = with(vec![(
        Key(5),
        node(5, vec![(ui2_model::Key::from_i32(1), leaf(1, 0.75))]),
    )]);
    let (got, ops) = apply_diff(&a, &b);
    assert_eq!(got, b);
    assert_eq!(ops.len(), 1);
    let w = s.resolve("a.nodes[].leaves[].w").unwrap().id;
    match &ops[0] {
        Op::Set { field, keys, value } => {
            assert_eq!(*field, w);
            assert_eq!(keys.as_slice(), &[Key(5), Key::from_i32(1)]);
            assert_eq!(*value, Value::F32(0.75));
        }
        other => panic!("{other:?}"),
    }
    // f32 compares bitwise: the same value gives no op.
    let (_, ops) = apply_diff(&b, &b.clone());
    assert!(ops.is_empty());
}

#[test]
fn apply_rejects_bad_operations_and_leaves_the_state_alone() {
    let s = schema();
    let (rid, _) = root_a();
    let mut st = ModelState::new(s);
    let before = st.root(rid).clone();
    let n = s.resolve("a.n").unwrap().id;
    let good = Op::Set {
        field: n,
        keys: Default::default(),
        value: Value::I32(5),
    };
    let wrong_type = Op::Set {
        field: n,
        keys: Default::default(),
        value: Value::Bool(true),
    };
    // The first op is fine, the second is not: nothing is applied.
    let err = st.apply(&[good.clone(), wrong_type]).unwrap_err();
    assert!(matches!(err, ApplyError::Value { .. }), "{err}");
    assert_eq!(*st.root(rid), before);
    // Unknown field.
    let bad_field = Op::Set {
        field: FieldId(9999),
        keys: Default::default(),
        value: Value::None,
    };
    assert!(matches!(
        st.apply(&[bad_field]),
        Err(ApplyError::UnknownField(9999))
    ));
    // Remove of a missing element.
    let list = s.resolve("a.nodes").unwrap().id;
    let rm = Op::Remove {
        list,
        keys: Default::default(),
        key: Key(77),
    };
    assert!(matches!(st.apply(&[rm]), Err(ApplyError::NoElement { .. })));
    // Move past the end.
    let mv = Op::Move {
        list,
        keys: Default::default(),
        key: Key(77),
        to: 3,
    };
    assert!(st.apply(&[mv]).is_err());
    // A good op alone works and is readable through `value`.
    st.apply(&[good]).unwrap();
    assert_eq!(st.value(n, &[]), Some(&Value::I32(5)));
    // Keys on a field that is not in a list are refused.
    assert_eq!(st.value(n, &[Key(1)]), None);
}

#[test]
fn value_and_list_keys_read_nested_data() {
    let s = schema();
    let (rid, _) = root_a();
    let ty = Type::Struct(s.root(rid).ty);
    let mut rng = Rng(11);
    let mut st = ModelState::new(s);
    let a = loop {
        let v = gen_value(s, &ty, &mut rng);
        // A value with at least two nodes.
        let nodes = s.resolve("a.nodes").unwrap().id;
        let mut probe = ModelState::new(s);
        probe
            .apply(&[Op::Reset {
                root: rid,
                value: v.clone(),
            }])
            .unwrap();
        if probe.list_keys(nodes, &[]).unwrap().count() >= 2 {
            break v;
        }
    };
    st.apply(&[Op::Reset {
        root: rid,
        value: a,
    }])
    .unwrap();
    let nodes = s.resolve("a.nodes").unwrap().id;
    let ks: Vec<Key> = st.list_keys(nodes, &[]).unwrap().collect();
    let name = s.resolve("a.nodes[].name").unwrap().id;
    for k in &ks {
        assert!(matches!(st.value(name, &[*k]), Some(Value::Text(_))));
    }
    assert_eq!(st.value(name, &[Key(12345)]), None);
    // The whole root.
    let root_field = s.root(rid).field;
    assert!(matches!(st.value(root_field, &[]), Some(Value::Struct(_))));
}

#[test]
fn validate_catches_misshapen_values() {
    let s = schema();
    let (rid, _) = root_a();
    let ty = Type::Struct(s.root(rid).ty);
    let good = ui2_model::default_value(s, &ty);
    validate(s, &ty, &good).unwrap();
    let Value::Struct(mut fs) = good.clone() else {
        unreachable!()
    };
    fs.pop();
    assert!(validate(s, &ty, &Value::Struct(fs)).is_err());
    // i8 out of range.
    let small = s.resolve("a.small").unwrap().ty;
    assert!(validate(s, &small, &Value::I32(200)).is_err());
    assert!(validate(s, &small, &Value::I32(-5)).is_ok());
    // A list whose key does not match its element.
    let l = s.resolve("a.hues").unwrap().ty;
    let el = Value::Struct(vec![Value::Enum(1), Value::Bool(true)]);
    assert!(validate(s, &l, &Value::List(vec![(Key(0), el.clone())])).is_err());
    assert!(validate(s, &l, &Value::List(vec![(Key::from_enum(1), el.clone())])).is_ok());
    assert!(
        validate(
            s,
            &l,
            &Value::List(vec![
                (Key::from_enum(1), el.clone()),
                (Key::from_enum(1), el)
            ])
        )
        .is_err()
    );
    // Enum index.
    let hue = Type::Enum(ui2_model::EnumId(0));
    assert!(validate(s, &hue, &Value::Enum(3)).is_err());
}
