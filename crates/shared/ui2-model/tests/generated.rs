//! The generated types agree with the schema: constants, defaults, and typed diff equal to the dynamic diff.

mod common;

use common::{Rng, gen_value, mutate};
use ui2_model::generated as g;
use ui2_model::{Model, Op, Root, Schema, Type, Value, diff_root, validate};

fn check_root<R: Root + PartialEq + std::fmt::Debug>(schema: &'static Schema) {
    let rid = schema
        .root_by_name(R::NAME)
        .expect("root exists in the schema");
    let root = schema.root(rid);
    let ty = Type::Struct(root.ty);

    // The default struct is the default value of the schema.
    assert_eq!(
        R::default().to_value(),
        ui2_model::default_value(schema, &ty),
        "{}",
        R::NAME
    );

    let mut rng = Rng(0xC0FFEE ^ u64::from(rid.0));
    for _ in 0..1500 {
        let va = gen_value(schema, &ty, &mut rng);
        let vb = if rng.chance(10) {
            gen_value(schema, &ty, &mut rng)
        } else {
            mutate(schema, &ty, &va, &mut rng, None)
        };
        validate(schema, &ty, &va).unwrap();
        let a = R::from_value(&va).unwrap();
        let b = R::from_value(&vb).unwrap();
        // Round trip through Value.
        assert_eq!(a.to_value(), va);
        assert_eq!(R::from_value(&a.to_value()).unwrap(), a);

        let mut typed = Vec::new();
        let mut cur = ui2_model::Cursor::at(root.field);
        b.diff(&a, &mut cur, &mut typed);
        assert_eq!(
            cur,
            ui2_model::Cursor::at(root.field),
            "the cursor is restored"
        );
        let mut dynamic = Vec::new();
        diff_root(schema, rid, &va, &vb, &mut dynamic);
        assert_eq!(typed, dynamic, "typed and dynamic diff of {}", R::NAME);
        if va == vb {
            assert!(typed.is_empty());
        }
        // And the ops give b.
        let mut st = ui2_model::ModelState::new(schema);
        st.apply(&[Op::Reset {
            root: rid,
            value: va,
        }])
        .unwrap();
        st.apply(&typed).unwrap();
        assert_eq!(*st.root(rid), vb);
    }
}

#[test]
fn typed_diff_equals_dynamic_diff_for_every_root() {
    let s = Schema::v1();
    check_root::<g::Meta>(s);
    check_root::<g::Top>(s);
    check_root::<g::Game>(s);
    check_root::<g::Research>(s);
}

#[test]
fn generated_constants_match_the_schema() {
    let s = Schema::v1();
    assert_eq!(g::SCHEMA_HASH, s.hash());
    assert_eq!(
        (g::SCHEMA_VERSION.0, g::SCHEMA_VERSION.1),
        (s.version().major, s.version().minor)
    );
    // Every enum: names and discriminants.
    for e in s.enums() {
        if e.name == "Source" {
            assert_eq!(g::Source::NAMES.to_vec(), e.variants);
            for (i, v) in e.variants.iter().enumerate() {
                assert_eq!(g::Source::from_u16(i as u16).unwrap().name(), v);
                assert_eq!(g::Source::from_name(v).unwrap() as u16, i as u16);
            }
            assert!(g::Source::from_u16(e.variants.len() as u16).is_none());
        }
    }
}

#[test]
fn keyed_list_rejects_duplicates_and_orders_by_insertion() {
    use ui2_model::{Key, KeyedList};
    let r = |id: u64| g::Resource {
        id: Key(id),
        ..Default::default()
    };
    let mut l = KeyedList::new();
    l.try_push(r(2)).unwrap();
    l.try_push(r(1)).unwrap();
    assert!(l.try_push(r(2)).is_err());
    assert_eq!(l.keys().collect::<Vec<_>>(), [Key(2), Key(1)]);
    assert!(KeyedList::from_vec(vec![r(1), r(1)]).is_err());
    assert_eq!(l.get(Key(1)).map(|x| x.id), Some(Key(1)));
    // A list that lists a key differently from the element is not a valid value.
    let v = Value::List(vec![(Key(9), r(1).to_value())]);
    assert!(KeyedList::<g::Resource>::from_value(&v).is_err());
}
