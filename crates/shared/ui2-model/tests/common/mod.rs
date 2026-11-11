//! Shared helpers of the integration tests: a seeded random generator and random model values.

#![allow(dead_code)]

use std::sync::Arc;

use ui2_model::{AssetRef, Key, ListType, Scalar, Schema, Text, Type, Value, default_value};

/// xorshift64*: deterministic, no dependency.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// 0..n
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() >> 11) as usize % n
        }
    }

    pub fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }
}

pub fn leak(s: Schema) -> &'static Schema {
    Box::leak(Box::new(s))
}

fn scalar(rng: &mut Rng, s: Scalar) -> Value {
    match s {
        Scalar::Bool => Value::Bool(rng.chance(50)),
        Scalar::I8 => Value::I32(rng.below(256) as i32 - 128),
        Scalar::I16 => Value::I32(rng.below(65536) as i32 - 32768),
        Scalar::I32 => Value::I32(rng.below(2000) as i32 - 1000),
        Scalar::U32 | Scalar::Color => Value::U32(rng.below(1 << 30) as u32),
        Scalar::F32 => Value::F32(rng.below(1000) as f32 / 1000.0),
        Scalar::Str => Value::Str(Arc::from(format!("s{}", rng.below(6)).as_str())),
        Scalar::Text => {
            if rng.chance(50) {
                Value::Text(Text::raw(&format!("t{}", rng.below(6))))
            } else {
                Value::Text(Text::key("K", &[&format!("{}", rng.below(4))]))
            }
        }
        Scalar::Asset => Value::Asset(AssetRef::new(&format!("icon:A:{}", rng.below(8)))),
    }
}

/// A random value of a type.
pub fn gen_value(schema: &Schema, ty: &Type, rng: &mut Rng) -> Value {
    match ty {
        Type::Scalar(s) => scalar(rng, *s),
        Type::Key(_) => Value::Key(Key(rng.below(40) as u64)),
        Type::Enum(e) => Value::Enum(rng.below(schema.enum_def(*e).variants.len()) as u16),
        Type::Struct(t) => Value::Struct(
            schema
                .type_def(*t)
                .fields
                .iter()
                .map(|f| gen_value(schema, &f.ty, rng))
                .collect(),
        ),
        Type::Option(inner) => {
            if rng.chance(40) {
                Value::None
            } else {
                gen_value(schema, inner, rng)
            }
        }
        Type::List(l) => {
            let n = rng.below(7);
            let mut items: Vec<(Key, Value)> = Vec::new();
            for _ in 0..n {
                let ev = gen_value(schema, &Type::Struct(l.elem), rng);
                let ev = with_fresh_key(schema, l, ev, &items, rng);
                if let Some(ev) = ev {
                    let k = key_of(l, &ev);
                    items.push((k, ev));
                }
            }
            Value::List(items)
        }
    }
}

fn key_of(l: &ListType, ev: &Value) -> Key {
    let Value::Struct(fs) = ev else {
        unreachable!()
    };
    fs[l.key_index].as_key().unwrap()
}

/// Give an element a key that is not used in `items` (None if the key space is exhausted).
fn with_fresh_key(
    schema: &Schema,
    l: &ListType,
    ev: Value,
    items: &[(Key, Value)],
    rng: &mut Rng,
) -> Option<Value> {
    let Value::Struct(mut fs) = ev else {
        unreachable!()
    };
    for _ in 0..30 {
        let cand = match l.key_ty.as_ref() {
            Type::Key(_) => Value::Key(Key(rng.below(40) as u64)),
            Type::Enum(e) => Value::Enum(rng.below(schema.enum_def(*e).variants.len()) as u16),
            Type::Scalar(Scalar::Str) => {
                Value::Str(Arc::from(format!("n{}", rng.below(12)).as_str()))
            }
            Type::Scalar(Scalar::I8) => Value::I32(rng.below(20) as i32),
            Type::Scalar(Scalar::U32) => Value::U32(rng.below(20) as u32),
            _ => Value::I32(rng.below(20) as i32),
        };
        let k = cand.as_key().unwrap();
        if !items.iter().any(|(ik, _)| *ik == k) {
            fs[l.key_index] = cand;
            return Some(Value::Struct(fs));
        }
    }
    None
}

/// A random change of a value: most of it stays, some fields change, lists lose, gain and reorder elements.
pub fn mutate(schema: &Schema, ty: &Type, v: &Value, rng: &mut Rng, keep: Option<usize>) -> Value {
    match (ty, v) {
        (Type::Struct(t), Value::Struct(fs)) => {
            let td = schema.type_def(*t);
            Value::Struct(
                td.fields
                    .iter()
                    .zip(fs)
                    .enumerate()
                    .map(|(i, (f, fv))| {
                        if Some(i) == keep || !rng.chance(35) {
                            // Descend anyway into nested structures sometimes.
                            if Some(i) != keep && matches!(f.ty, Type::List(_) | Type::Struct(_)) {
                                mutate(schema, &f.ty, fv, rng, None)
                            } else {
                                fv.clone()
                            }
                        } else {
                            mutate(schema, &f.ty, fv, rng, None)
                        }
                    })
                    .collect(),
            )
        }
        (Type::Option(inner), cur) => {
            if rng.chance(25) {
                gen_value(schema, ty, rng)
            } else if matches!(cur, Value::None) {
                Value::None
            } else {
                mutate(schema, inner, cur, rng, None)
            }
        }
        (Type::List(l), Value::List(items)) => {
            let mut out: Vec<(Key, Value)> = Vec::new();
            for (k, ev) in items {
                if rng.chance(20) {
                    continue;
                }
                out.push((
                    *k,
                    mutate(schema, &Type::Struct(l.elem), ev, rng, Some(l.key_index)),
                ));
            }
            // Reorder.
            if out.len() > 1 && rng.chance(50) {
                match rng.below(3) {
                    0 => {
                        let (a, b) = (rng.below(out.len()), rng.below(out.len()));
                        out.swap(a, b);
                    }
                    1 => out.reverse(),
                    _ => {
                        let i = rng.below(out.len());
                        let e = out.remove(i);
                        let j = rng.below(out.len() + 1);
                        out.insert(j, e);
                    }
                }
            }
            // New elements.
            for _ in 0..rng.below(3) {
                let ev = gen_value(schema, &Type::Struct(l.elem), rng);
                if let Some(ev) = with_fresh_key(schema, l, ev, &out, rng) {
                    let k = key_of(l, &ev);
                    let at = rng.below(out.len() + 1);
                    out.insert(at, (k, ev));
                }
            }
            Value::List(out)
        }
        (Type::Struct(_), _) | (Type::List(_), _) => default_value(schema, ty),
        _ => gen_value(schema, ty, rng),
    }
}

/// A schema with every kind of field: optionals, enums, nested lists keyed by key, int, str and enum.
pub const TEST_SCHEMA: &str = r#"
version = "1.0"
[enum]
Hue = ["red", "green", "blue"]
[type.Leaf]
id = "i32"
tag = "str?"
w = "f32"
[type.Node]
key = "key<node>"
name = "text"
hue = "Hue"
leaves = { list = "Leaf", key = "id" }
extra = "Leaf?"
[type.ByName]
name = "str"
n = "u32"
[type.ByHue]
hue = "Hue"
on = "bool"
[model.a]
n = "i32"
flag = "bool?"
nodes = { list = "Node", key = "key" }
names = { list = "ByName", key = "name" }
hues = { list = "ByHue", key = "hue" }
pos = "Leaf"
asset = "asset"
color = "color"
small = "i8"
opt_node = "Node?"
[model.b]
x = "i32"
"#;
