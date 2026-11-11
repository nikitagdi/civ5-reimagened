//! Diffing: the minimal list reordering (longest increasing subsequence) and the dynamic diff of [`Value`]s.
//! Generated structs diff themselves without `Value`s (see `model.rs`); both must give the same operations.

use std::collections::{HashMap, HashSet};

use crate::delta::{Cursor, Keys, Op, Ops};
use crate::schema::{RootId, Schema, Type};
use crate::value::{Key, Value};

/// One step of the reordering of a keyed list. The steps are applied in order to the old list and give the
/// new one; `Remove` steps come first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListStep {
    /// The element is gone.
    Remove(Key),
    /// A new element; `new_index` is its index in the new list, `at` the position it takes in the list as it
    /// is at this step.
    Insert { at: u32, key: Key, new_index: usize },
    /// The element changes position: take it out, then put it so that it ends up at `to`.
    Move { key: Key, to: u32 },
}

/// The steps that turn the list of keys `old` into `new`. The number of `Move` steps is minimal: elements of
/// the longest increasing subsequence of retained elements stay, every other retained element moves once.
///
/// Keys must be unique inside each list.
#[must_use]
pub fn list_steps(old: &[Key], new: &[Key]) -> Vec<ListStep> {
    let new_pos: HashMap<Key, usize> = new.iter().enumerate().map(|(i, k)| (*k, i)).collect();
    let in_old: HashSet<Key> = old.iter().copied().collect();
    let mut steps = Vec::new();
    for k in old {
        if !new_pos.contains_key(k) {
            steps.push(ListStep::Remove(*k));
        }
    }
    let retained: Vec<Key> = old
        .iter()
        .copied()
        .filter(|k| new_pos.contains_key(k))
        .collect();
    let seq: Vec<usize> = retained.iter().map(|k| new_pos[k]).collect();
    let mask = lis_mask(&seq);
    let stay: HashSet<Key> = retained
        .iter()
        .zip(&mask)
        .filter(|(_, m)| **m)
        .map(|(k, _)| *k)
        .collect();

    // Walk the new order; every element that is not in the subsequence is put right behind its predecessor.
    let mut cur = retained;
    for (i, &k) in new.iter().enumerate() {
        let existing = in_old.contains(&k);
        if existing && stay.contains(&k) {
            continue;
        }
        if existing && let Some(p) = cur.iter().position(|c| *c == k) {
            cur.remove(p);
        }
        let at = if i == 0 {
            0
        } else {
            cur.iter()
                .position(|c| *c == new[i - 1])
                .map_or(0, |p| p + 1)
        };
        cur.insert(at, k);
        steps.push(if existing {
            ListStep::Move {
                key: k,
                to: at as u32,
            }
        } else {
            ListStep::Insert {
                at: at as u32,
                key: k,
                new_index: i,
            }
        });
    }
    steps
}

/// Which elements of `seq` form a longest strictly increasing subsequence.
fn lis_mask(seq: &[usize]) -> Vec<bool> {
    let n = seq.len();
    let mut tails: Vec<usize> = Vec::new();
    let mut prev = vec![usize::MAX; n];
    for i in 0..n {
        let pos = tails.partition_point(|&t| seq[t] < seq[i]);
        if pos > 0 {
            prev[i] = tails[pos - 1];
        }
        if pos == tails.len() {
            tails.push(i);
        } else {
            tails[pos] = i;
        }
    }
    let mut mask = vec![false; n];
    let mut cur = tails.last().copied();
    while let Some(c) = cur {
        mask[c] = true;
        cur = (prev[c] != usize::MAX).then_some(prev[c]);
    }
    mask
}

/// Diff one root, appending the operations to `out`.
pub fn diff_root(schema: &Schema, root: RootId, old: &Value, new: &Value, out: &mut Ops) {
    let r = schema.root(root);
    let mut at = Cursor::at(r.field);
    diff_node(schema, &Type::Struct(r.ty), &mut at, old, new, out);
}

fn set(at: &Cursor, new: &Value, out: &mut Ops) {
    out.push(Op::Set {
        field: at.field,
        keys: at.keys,
        value: new.clone(),
    });
}

fn diff_node(schema: &Schema, ty: &Type, at: &mut Cursor, old: &Value, new: &Value, out: &mut Ops) {
    match ty {
        Type::Struct(t) => {
            let td = schema.type_def(*t);
            match (old, new) {
                (Value::Struct(a), Value::Struct(b))
                    if a.len() == td.fields.len() && b.len() == td.fields.len() =>
                {
                    let base = at.field;
                    for (j, f) in td.fields.iter().enumerate() {
                        at.field = base.child(td.offsets[j]);
                        diff_node(schema, &f.ty, at, &a[j], &b[j], out);
                    }
                    at.field = base;
                }
                _ => {
                    if old != new {
                        set(at, new, out);
                    }
                }
            }
        }
        Type::Option(inner) => match (old, new) {
            (Value::None, Value::None) => {}
            (Value::None, _) | (_, Value::None) => set(at, new, out),
            (a, b) => diff_node(schema, inner, at, a, b, out),
        },
        Type::List(l) => match (old, new) {
            (Value::List(a), Value::List(b)) => diff_list(schema, l.elem, at, a, b, out),
            _ => {
                if old != new {
                    set(at, new, out);
                }
            }
        },
        _ => {
            if old != new {
                set(at, new, out);
            }
        }
    }
}

fn diff_list(
    schema: &Schema,
    elem: crate::schema::TypeId,
    at: &mut Cursor,
    old: &[(Key, Value)],
    new: &[(Key, Value)],
    out: &mut Ops,
) {
    let same_shape = old.len() == new.len() && old.iter().zip(new).all(|(a, b)| a.0 == b.0);
    if !same_shape {
        let ok: Vec<Key> = old.iter().map(|e| e.0).collect();
        let nk: Vec<Key> = new.iter().map(|e| e.0).collect();
        for step in list_steps(&ok, &nk) {
            out.push(match step {
                ListStep::Remove(key) => Op::Remove {
                    list: at.field,
                    keys: at.keys,
                    key,
                },
                ListStep::Insert {
                    at: pos,
                    key,
                    new_index,
                } => Op::Insert {
                    list: at.field,
                    keys: at.keys,
                    at: pos,
                    key,
                    value: new[new_index].1.clone(),
                },
                ListStep::Move { key, to } => Op::Move {
                    list: at.field,
                    keys: at.keys,
                    key,
                    to,
                },
            });
        }
    }
    let ty = Type::Struct(elem);
    if same_shape {
        for (a, b) in old.iter().zip(new) {
            element(schema, &ty, at, a.0, &a.1, &b.1, out);
        }
    } else {
        let by_key: HashMap<Key, &Value> = old.iter().map(|(k, v)| (*k, v)).collect();
        for (k, b) in new {
            if let Some(a) = by_key.get(k) {
                element(schema, &ty, at, *k, a, b, out);
            }
        }
    }
}

fn element(
    schema: &Schema,
    ty: &Type,
    at: &mut Cursor,
    key: Key,
    a: &Value,
    b: &Value,
    out: &mut Ops,
) {
    if a == b {
        return;
    }
    let keys: Keys = at.keys;
    at.keys.push(key);
    diff_node(schema, ty, at, a, b, out);
    at.keys = keys;
}
