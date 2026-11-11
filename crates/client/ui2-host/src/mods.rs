//! Mods in UI v2: the fields their Rust halves write (`mods.<id>.*`) and the intents their panels and scripts
//! send back to them.
//!
//! - The model: a mod's `[ui] model` fragment is mounted at `mods.<id>` ([`V2Ui::mount_mod_model`]). The
//!   application hands the writes of the mods' Rust halves over as [`ModWrite`]s ([`V2Ui::queue_mod_writes`]);
//!   the frame applies them on its step 4, after the intents and before the source, with the writer of the mod
//!   (`Owner::Mod`). A value is converted to the type its field declares ([`ModModelCheck`]); a write that does
//!   not fit is refused and logged once per mod and field.
//! - The intents: `mods.<id>.<name>` from a mod's markup or script reach the actions module with the origin of
//!   that mod (its index in the mod table); the module checks that the origin is the mod the name gives and
//!   queues them; the application takes them after the frame ([`V2Ui::take_mod_intents`]) for the mods' Rust
//!   halves.

use std::collections::BTreeSet;
use std::sync::Arc;

use ui2_model::schema::{Scalar, Schema, Type};
use ui2_model::wire::{ModIdx, ModIntent, Origin, WireIntent};
use ui2_model::{Fragment, Key, ModelStore, Namespace, Owner, Text, Value};

use crate::v2::V2Ui;

/// A value a mod's Rust half writes (the host's copy of the mod API's `UiValue`).
#[derive(Debug, Clone, PartialEq)]
pub enum ModValue {
    Null,
    Bool(bool),
    Int(i64),
    Num(f64),
    Str(Arc<str>),
}

/// One write of a mod's Rust half to its fields `mods.<id>.*`. Paths are below the mod's root (`count`,
/// `stats.last`, `echoes`).
#[derive(Debug, Clone, PartialEq)]
pub enum ModWrite {
    /// A scalar field.
    Set { field: String, value: ModValue },
    /// Inserts or replaces the record `key` of a keyed list; fields left out take their zero value.
    Put {
        list: String,
        key: i64,
        rec: Vec<(String, ModValue)>,
    },
    /// Removes the record `key` (nothing happens when there is none).
    Remove { list: String, key: i64 },
    /// Empties a list.
    Clear { list: String },
}

impl ModWrite {
    /// The path below the mod's root the write goes to.
    pub fn target(&self) -> &str {
        match self {
            ModWrite::Set { field, .. } => field,
            ModWrite::Put { list, .. }
            | ModWrite::Remove { list, .. }
            | ModWrite::Clear { list } => list,
        }
    }
}

/// The type check of one mod's fields: what a write must look like to be applied. It holds the schema of the
/// mod's mounted model fragment, so the application can check a write when the mod makes it, before any frame.
#[derive(Debug, Clone)]
pub struct ModModelCheck {
    id: Arc<str>,
    schema: &'static Schema,
}

impl ModModelCheck {
    /// The mod.
    pub fn id(&self) -> &str {
        &self.id
    }

    fn path(&self, rel: &str) -> String {
        format!("mods.{}.{rel}", self.id)
    }

    /// The write as the store takes it: the full path, the list key and the value; `Err` names the field and
    /// says why it does not fit.
    pub fn convert(&self, w: &ModWrite) -> Result<Converted, String> {
        let path = self.path(w.target());
        let field = self
            .schema
            .resolve(&path)
            .map_err(|_| format!("`{}`: no such field", w.target()))?;
        let list = |ty: &Type| match ty {
            Type::List(l) => Ok(l.clone()),
            _ => Err(format!("`{}` is not a list", w.target())),
        };
        match w {
            ModWrite::Set { field: name, value } => {
                if matches!(field.ty, Type::List(_) | Type::Struct(_)) {
                    return Err(format!("`{name}` is not a scalar field"));
                }
                let v =
                    coerce(self.schema, &field.ty, value).map_err(|e| format!("`{name}`: {e}"))?;
                Ok(Converted::Set(path, v))
            }
            ModWrite::Put {
                list: name,
                key,
                rec,
            } => {
                let l = list(&field.ty)?;
                let key_v = coerce(self.schema, &l.key_ty, &ModValue::Int(*key))
                    .map_err(|e| format!("`{name}` key {key}: {e}"))?;
                let k = list_key(&key_v)
                    .ok_or_else(|| format!("`{name}`: keys of this list are not numbers"))?;
                let def = self.schema.type_def(l.elem);
                let mut values: Vec<Value> = def
                    .fields
                    .iter()
                    .map(|f| ui2_model::default_value(self.schema, &f.ty))
                    .collect();
                values[l.key_index] = key_v;
                for (fname, v) in rec {
                    let Some(i) = def.fields.iter().position(|f| f.name == *fname) else {
                        return Err(format!("`{name}.{fname}`: no such field"));
                    };
                    if i == l.key_index {
                        continue;
                    }
                    values[i] = coerce(self.schema, &def.fields[i].ty, v)
                        .map_err(|e| format!("`{name}.{fname}`: {e}"))?;
                }
                Ok(Converted::Put(path, k, Value::Struct(values)))
            }
            ModWrite::Remove { list: name, key } => {
                let l = list(&field.ty)?;
                let key_v = coerce(self.schema, &l.key_ty, &ModValue::Int(*key))
                    .map_err(|e| format!("`{name}` key {key}: {e}"))?;
                let k = list_key(&key_v)
                    .ok_or_else(|| format!("`{name}`: keys of this list are not numbers"))?;
                Ok(Converted::Remove(path, k))
            }
            ModWrite::Clear { .. } => {
                list(&field.ty)?;
                Ok(Converted::Clear(path))
            }
        }
    }

    /// Whether the write fits the mod's fields.
    ///
    /// # Errors
    /// Why it does not: the field's name and the reason.
    pub fn check(&self, w: &ModWrite) -> Result<(), String> {
        self.convert(w).map(|_| ())
    }
}

/// A write converted to the store's terms.
#[derive(Debug, Clone)]
pub enum Converted {
    Set(String, Value),
    Put(String, Key, Value),
    Remove(String, Key),
    Clear(String),
}

/// The list key of a key field's value.
fn list_key(v: &Value) -> Option<Key> {
    match v {
        Value::I32(n) => Some(Key::from_i32(*n)),
        Value::U32(n) => Some(Key::from_u32(*n)),
        Value::Enum(n) => Some(Key::from_enum(*n)),
        _ => None,
    }
}

/// A mod's value as the value of a field of type `ty`.
fn coerce(schema: &Schema, ty: &Type, v: &ModValue) -> Result<Value, String> {
    let int = |lo: i64, hi: i64, name: &str| match v {
        ModValue::Int(n) if (lo..=hi).contains(n) => Ok(*n),
        ModValue::Int(n) => Err(format!("{n} is out of the range of {name}")),
        other => Err(format!("{} where {name} is expected", kind(other))),
    };
    match ty {
        Type::Option(inner) => match v {
            ModValue::Null => Ok(Value::None),
            v => coerce(schema, inner, v),
        },
        Type::Scalar(s) => match (s, v) {
            (Scalar::Bool, ModValue::Bool(b)) => Ok(Value::Bool(*b)),
            (Scalar::I8, _) => {
                int(i64::from(i8::MIN), i64::from(i8::MAX), "i8").map(|n| Value::I32(n as i32))
            }
            (Scalar::I16, _) => {
                int(i64::from(i16::MIN), i64::from(i16::MAX), "i16").map(|n| Value::I32(n as i32))
            }
            (Scalar::I32, _) => {
                int(i64::from(i32::MIN), i64::from(i32::MAX), "i32").map(|n| Value::I32(n as i32))
            }
            (Scalar::U32, _) => int(0, i64::from(u32::MAX), "u32").map(|n| Value::U32(n as u32)),
            (Scalar::F32, ModValue::Num(x)) => Ok(Value::F32(*x as f32)),
            (Scalar::F32, ModValue::Int(n)) => Ok(Value::F32(*n as f32)),
            (Scalar::Str, ModValue::Str(s)) => Ok(Value::Str(Arc::clone(s))),
            (Scalar::Text, ModValue::Str(s)) => Ok(Value::Text(Text::Raw(Arc::clone(s)))),
            (s, other) => Err(format!("{} where {} is expected", kind(other), s.name())),
        },
        Type::Enum(e) => {
            let def = schema
                .enums()
                .get(e.0 as usize)
                .ok_or_else(|| "an unknown enum".to_string())?;
            match v {
                ModValue::Str(s) => def
                    .variants
                    .iter()
                    .position(|n| **n == **s)
                    .map(|i| Value::Enum(i as u16))
                    .ok_or_else(|| format!("`{s}` is not one of {}", def.variants.join(", "))),
                ModValue::Int(n) if (0..def.variants.len() as i64).contains(n) => {
                    Ok(Value::Enum(*n as u16))
                }
                other => Err(format!(
                    "{} where a name of {} is expected",
                    kind(other),
                    def.name
                )),
            }
        }
        Type::Key(_) | Type::Struct(_) | Type::List(_) => {
            Err("a mod cannot write this kind of field as one value".to_string())
        }
    }
}

fn kind(v: &ModValue) -> &'static str {
    match v {
        ModValue::Null => "null",
        ModValue::Bool(_) => "a boolean",
        ModValue::Int(_) => "an integer",
        ModValue::Num(_) => "a number",
        ModValue::Str(_) => "a string",
    }
}

/// The mod side of a running UI v2 (it lives as long as the model, across skins).
#[derive(Default)]
pub(crate) struct Mods {
    /// The ids of the game's mods in index order: `Origin::Mod(ModIdx(i))` is `table[i]`.
    pub(crate) table: Vec<Arc<str>>,
    /// Writes of the mods' Rust halves for the next frame's step 4, in the order they were made.
    writes: Vec<(Arc<str>, ModWrite)>,
    /// Intents for the mods' Rust halves, for the application after the frame.
    intents_out: Vec<ModIntent>,
    /// `(mod, field)` whose refused write was logged.
    warned: BTreeSet<(Arc<str>, String)>,
}

/// The most writes kept between two frames, over all mods; later ones are dropped (the application keeps its
/// own limits per mod, this only bounds a UI that runs no frame).
pub const MAX_QUEUED_WRITES: usize = 16 * 1024;

impl Mods {
    /// The index of a mod in the table.
    pub(crate) fn idx_of(&self, id: &str) -> Option<ModIdx> {
        self.table
            .iter()
            .position(|m| &**m == id)
            .and_then(|i| u16::try_from(i).ok())
            .map(ModIdx)
    }

    /// Markup intents `mods.<id>.*` get the origin of mod `<id>`. Only a mod's fragment can carry such an intent
    /// (a skin's package that names one is refused when it is loaded), so the name tells whose markup it is.
    pub(crate) fn tag_markup(&self, intents: &mut [WireIntent]) {
        for i in intents {
            if !matches!(i.origin, Origin::Markup { .. }) {
                continue;
            }
            let Some(rest) = i.name.strip_prefix("mods.") else {
                continue;
            };
            let id = rest.split('.').next().unwrap_or("");
            if let Some(idx) = self.idx_of(id) {
                i.origin = Origin::Mod(idx);
            }
        }
    }

    /// The writes of the step 4: each mod's, in order, through the mod's writer. Returns the writes refused.
    pub(crate) fn apply_writes(&mut self, store: &mut ModelStore) -> u32 {
        let mut refused = 0;
        for (id, w) in std::mem::take(&mut self.writes) {
            if let Err(why) = apply_write(store, &id, &w) {
                refused += 1;
                if self
                    .warned
                    .insert((Arc::clone(&id), w.target().to_string()))
                {
                    tracing::warn!(target: "ui2_host", "mod {id}: a write was refused: {why}");
                }
            }
        }
        refused
    }

    /// The mod intents the actions module accepted in a frame.
    pub(crate) fn keep_intents(&mut self, intents: Vec<ModIntent>) {
        self.intents_out.extend(intents);
    }
}

/// One write of mod `id` into the store.
fn apply_write(store: &mut ModelStore, id: &Arc<str>, w: &ModWrite) -> Result<(), String> {
    let check = model_check(store, id).ok_or_else(|| format!("mods.{id} is not mounted"))?;
    let c = check.convert(w)?;
    let mut writer = store.writer(Owner::Mod(Arc::clone(id)));
    let r = match c {
        Converted::Set(path, v) => writer.set_value(&path, v),
        Converted::Put(path, k, v) => writer.list_put(&path, k, v),
        Converted::Remove(path, k) => match writer.list_remove(&path, k) {
            // Removing what is not there is nothing (the mod API's promise).
            Err(ui2_model::StoreError::Apply(ui2_model::ApplyError::NoElement { .. })) => Ok(()),
            r => r,
        },
        Converted::Clear(path) => writer.list_clear(&path),
    };
    r.map_err(|e| e.to_string())
}

/// The check of a mounted mod model.
fn model_check(store: &ModelStore, id: &Arc<str>) -> Option<ModModelCheck> {
    let schema = store.mount_schema(&Namespace::Mod(Arc::clone(id)))?;
    Some(ModModelCheck {
        id: Arc::clone(id),
        schema,
    })
}

impl V2Ui {
    /// Mounts a mod's model fragment (`[ui] model`, the text of `ui/model.toml`) at `mods.<id>`: its fields
    /// exist from the next frame, at their zero values, and the mod's writes go there. Mounting a mod that is
    /// mounted keeps what is there.
    ///
    /// # Errors
    /// The fragment does not parse, or does not lay out under `mods.<id>`.
    pub fn mount_mod_model(&mut self, id: &str, model: &str) -> Result<ModModelCheck, String> {
        let id: Arc<str> = Arc::from(id);
        let ns = Namespace::Mod(Arc::clone(&id));
        if !self.store.is_mounted(&ns) {
            let frag = Fragment::parse(model).map_err(|e| format!("the model of mod {id}: {e}"))?;
            self.store
                .mount(ns, &frag)
                .map_err(|e| format!("the model of mod {id}: {e}"))?;
        }
        model_check(&self.store, &id).ok_or_else(|| format!("mods.{id} is not mounted"))
    }

    /// The type check of a mod whose model is mounted.
    pub fn mod_model_check(&self, id: &str) -> Option<ModModelCheck> {
        model_check(&self.store, &Arc::from(id))
    }

    /// Writes of mod `id`'s Rust half, applied in the next frame (step 4) in this order.
    pub fn queue_mod_writes(&mut self, id: &str, writes: impl IntoIterator<Item = ModWrite>) {
        let id: Arc<str> = Arc::from(id);
        for w in writes {
            if self.mods.writes.len() >= MAX_QUEUED_WRITES {
                tracing::warn!(target: "ui2_host", "mod {id}: too many writes without a frame; the rest are dropped");
                return;
            }
            self.mods.writes.push((Arc::clone(&id), w));
        }
    }

    /// The intents of the mods' panels and scripts (`mods.<id>.<name>`) the frames accepted since the last call,
    /// in order: for the mods' Rust halves.
    pub fn take_mod_intents(&mut self) -> Vec<ModIntent> {
        std::mem::take(&mut self.mods.intents_out)
    }

    /// The value of the root `mods.<id>` (with this frame's writes), for tests and tools.
    pub fn mod_model_value(&self, id: &str) -> Option<&Value> {
        self.store.fragment_value(&Namespace::Mod(Arc::from(id)))
    }
}

/// The root id of a mounted namespace (tests).
#[cfg(test)]
pub(crate) fn root_of(store: &ModelStore, ns: &Namespace) -> Option<ui2_model::RootId> {
    store.mount_ids(ns).map(|(r, _)| r)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODEL: &str = "[type.Echo]\nseq = \"i32\"\ntext = \"str\"\nfrom = \"i16\"\n\n[root]\ncount = \"i32\"\nlast = \"str\"\nratio = \"f32\"\nopen = \"bool\"\nsmall = \"u32\"\nechoes = { list = \"Echo\", key = \"seq\" }\n";

    fn mounted() -> (ModelStore, ModModelCheck) {
        let mut store = ModelStore::new(Schema::v1());
        let frag = Fragment::parse(MODEL).expect("model");
        store
            .mount(Namespace::Mod(Arc::from("hello")), &frag)
            .expect("mount");
        let check = model_check(&store, &Arc::from("hello")).expect("check");
        (store, check)
    }

    fn set(field: &str, value: ModValue) -> ModWrite {
        ModWrite::Set {
            field: field.into(),
            value,
        }
    }

    #[test]
    fn writes_are_converted_to_the_declared_types_or_refused_with_the_field() {
        let (_, c) = mounted();
        assert!(c.check(&set("count", ModValue::Int(3))).is_ok());
        assert!(c.check(&set("ratio", ModValue::Int(3))).is_ok());
        assert!(c.check(&set("ratio", ModValue::Num(0.5))).is_ok());
        assert!(c.check(&set("last", ModValue::Str("x".into()))).is_ok());
        let e = c
            .check(&set("count", ModValue::Str("x".into())))
            .unwrap_err();
        assert!(e.contains("`count`"), "{e}");
        let e = c.check(&set("small", ModValue::Int(-1))).unwrap_err();
        assert!(e.contains("`small`") && e.contains("range"), "{e}");
        let e = c.check(&set("nope", ModValue::Int(1))).unwrap_err();
        assert!(e.contains("`nope`"), "{e}");
        let e = c.check(&set("echoes", ModValue::Int(1))).unwrap_err();
        assert!(e.contains("`echoes`"), "{e}");
        let put = |rec: Vec<(&str, ModValue)>| ModWrite::Put {
            list: "echoes".into(),
            key: 4,
            rec: rec.into_iter().map(|(n, v)| (n.to_string(), v)).collect(),
        };
        assert!(
            c.check(&put(vec![("text", ModValue::Str("hi".into()))]))
                .is_ok()
        );
        let e = c
            .check(&put(vec![("from", ModValue::Int(70_000))]))
            .unwrap_err();
        assert!(e.contains("`echoes.from`"), "{e}");
        let e = c.check(&put(vec![("ghost", ModValue::Null)])).unwrap_err();
        assert!(e.contains("`echoes.ghost`"), "{e}");
        assert!(
            c.check(&ModWrite::Clear {
                list: "count".into()
            })
            .is_err()
        );
    }

    #[test]
    fn applied_writes_reach_the_store_through_the_mods_writer() {
        let (mut store, _) = mounted();
        let mut mods = Mods::default();
        let id: Arc<str> = Arc::from("hello");
        mods.writes = vec![
            (Arc::clone(&id), set("count", ModValue::Int(2))),
            (
                Arc::clone(&id),
                ModWrite::Put {
                    list: "echoes".into(),
                    key: 1,
                    rec: vec![("text".into(), ModValue::Str("one".into()))],
                },
            ),
            (Arc::clone(&id), set("count", ModValue::Str("bad".into()))),
            (
                Arc::clone(&id),
                ModWrite::Remove {
                    list: "echoes".into(),
                    key: 9,
                },
            ),
            (Arc::from("ghost"), set("count", ModValue::Int(1))),
        ];
        assert_eq!(
            mods.apply_writes(&mut store),
            2,
            "the bad type and the unmounted mod"
        );
        let delta = store.take_frame().expect("a frame");
        let root = root_of(&store, &Namespace::Mod(Arc::clone(&id))).expect("root");
        assert!(
            delta
                .ops
                .iter()
                .any(|op| matches!(op, ui2_model::Op::Reset { root: r, .. } if *r == root))
        );
        let v = store
            .fragment_value(&Namespace::Mod(id))
            .expect("value")
            .clone();
        let Value::Struct(fields) = v else {
            panic!("{v:?}")
        };
        assert!(matches!(fields[0], Value::I32(2)), "{fields:?}");
        let Value::List(items) = &fields[5] else {
            panic!("{fields:?}")
        };
        assert_eq!(items.len(), 1);
    }

    #[test]
    fn markup_intents_of_a_mod_get_its_origin() {
        let mods = Mods {
            table: vec![Arc::from("alpha"), Arc::from("hello")],
            ..Mods::default()
        };
        let wire = |name: &str, origin| WireIntent {
            name: name.into(),
            args: Vec::new(),
            origin,
        };
        let mut v = vec![
            wire("mods.hello.say", Origin::Markup { node: 3 }),
            wire("mods.ghost.say", Origin::Markup { node: 3 }),
            wire("turn.end", Origin::Markup { node: 1 }),
            wire("mods.alpha.go", Origin::Script),
        ];
        mods.tag_markup(&mut v);
        assert_eq!(v[0].origin, Origin::Mod(ModIdx(1)));
        assert_eq!(v[1].origin, Origin::Markup { node: 3 }, "not in the set");
        assert_eq!(v[2].origin, Origin::Markup { node: 1 });
        assert_eq!(v[3].origin, Origin::Script, "only markup is tagged here");
    }
}
