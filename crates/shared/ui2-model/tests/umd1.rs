//! UMD1 text: the reference delta that the JS decoder test reads (`tests/data/umd1-reference.json`).

use ui2_model::{FrameDelta, Key, Keys, Op, RootId, Schema, Text, Value};

const REFERENCE_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/data/umd1-reference.json"
);

/// A delta that has every kind of op, built against the v1 schema.
fn reference_delta(s: &Schema) -> FrameDelta {
    let id = |p: &str| s.resolve(p).unwrap().id;
    let res = |k: Key, name: Text, avail: i32| {
        Value::Struct(vec![
            Value::Key(k),
            Value::Text(name),
            Value::Asset(ui2_model::AssetRef::new("icon:RES_ATLAS:3")),
            Value::I32(avail),
            Value::I32(0),
            Value::I32(avail),
        ])
    };
    let iron = Key::new(1, 3);
    let horses = Key::new(1, 4);
    let meta = s.root_by_name("meta").unwrap();
    let top = s.root_by_name("top").unwrap();
    FrameDelta {
        revision: 7,
        ops: vec![
            Op::Reset {
                root: meta,
                value: Value::Struct(vec![
                    Value::Str("1.0".into()),
                    Value::Enum(1),
                    Value::Bool(true),
                    Value::Bool(false),
                    Value::Bool(true),
                ]),
            },
            Op::Set {
                field: id("top.gold"),
                keys: Keys::new(),
                value: Value::I32(-12),
            },
            Op::Set {
                field: id("top.tourism"),
                keys: Keys::new(),
                value: Value::None,
            },
            Op::Insert {
                list: id("top.resources"),
                keys: Keys::new(),
                at: 0,
                key: iron,
                value: res(iron, Text::key("res.iron", &["2"]), 2),
            },
            Op::Insert {
                list: id("top.resources"),
                keys: Keys::new(),
                at: 1,
                key: horses,
                value: res(horses, Text::raw("Horses \"fast\""), 1),
            },
            Op::Set {
                field: id("top.resources[].available"),
                keys: Keys::from_slice(&[horses]).unwrap(),
                value: Value::I32(5),
            },
            Op::Move {
                list: id("top.resources"),
                keys: Keys::new(),
                key: horses,
                to: 0,
            },
            Op::Remove {
                list: id("top.resources"),
                keys: Keys::new(),
                key: iron,
            },
            Op::Reset {
                root: RootId(top.0),
                value: Value::Struct(vec![
                    Value::I32(100),
                    Value::I32(3),
                    Value::I32(0),
                    Value::List(vec![]),
                ]),
            },
        ],
    }
}

#[test]
fn reference_delta_text_is_stable() {
    let s = Schema::v1();
    let text = reference_delta(s).to_umd1(s);
    if std::env::var_os("UPDATE_FIXTURES").is_some() {
        std::fs::write(REFERENCE_PATH, format!("{text}\n")).unwrap();
    }
    let expected = std::fs::read_to_string(REFERENCE_PATH)
        .expect("fixture exists; run with UPDATE_FIXTURES=1 to create it");
    assert_eq!(
        expected.trim_end(),
        text,
        "the JS decoder test reads this file; rerun with UPDATE_FIXTURES=1 after changing the format"
    );
    // It is valid JSON.
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v[0], 7);
    assert_eq!(v.as_array().unwrap().len(), 1 + 9);
}

#[test]
fn escapes_and_numbers() {
    let s = Schema::v1();
    let d = FrameDelta {
        revision: 1,
        ops: vec![Op::Set {
            field: s.resolve("meta.schema").unwrap().id,
            keys: Keys::new(),
            value: Value::Str("a\"b\\c\nd\u{1}".into()),
        }],
    };
    let v: serde_json::Value = serde_json::from_str(&d.to_umd1(s)).unwrap();
    assert_eq!(v[1][3], "a\"b\\c\nd\u{1}");
}
