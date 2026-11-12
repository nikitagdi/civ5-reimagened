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
use ui2_model::wire::{ModIdx, ModIntent, Origin, ScriptCtxId, WireIntent};
use ui2_model::{Fragment, Key, ModelStore, Namespace, Owner, Text, Value};

use platform::ScriptHost;
use ui2_package::Package;
use ui2_runtime::{MountMode, Runtime};
use ui2_script::ScriptOutput;

use crate::scripts::Scripts;
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
    /// The mods' UI as last mounted, in load order, with why a mod shows a placeholder (mounted again in a skin
    /// built anew).
    uis: Vec<(ModUi, Option<String>)>,
    /// The skin package the panels are mounted in.
    mounted_in: Option<Arc<Package>>,
    /// The workers of the mods' scripts, before the first mod with scripts starts.
    script_host: Option<Box<dyn ScriptHost>>,
    /// The mods' script contexts (a supervisor of their own: a mod's script never stops the skin's).
    pub(crate) scripts: Option<Scripts>,
    /// The mods whose state got its defaults in this UI (a later mount keeps what is there).
    seeded: BTreeSet<Arc<str>>,
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

// ---------------------------------------------------------------------------------------------------------------
// The panels

/// The reserved path in a fragment's script files under which the converter stores the mod's model fragment
/// (`[ui] model`): no script file can have this name. The host mounts it at `mods.<id>`; it is never run.
pub const MODEL_FILE: &str = "ui2:model.toml";

/// The extension point every skin has: the panels whose point a skin lacks go there.
pub const MODS_POINT: &str = "mods";

/// The index of the mods' fragments in the web build, relative to the page.
pub const MODS_INDEX_PATH: &str = "ui2/mods-ui.json";

/// One panel of a mod (`[[ui.panels]]`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModPanel {
    pub id: String,
    pub point: String,
    /// `mode = "replace"`: in place of the point's element (hidden while the panel is there).
    pub replace: bool,
    /// `fallback = "mods"`: in the `mods` point when the skin lacks `point`; `false` (`none`): no panel then.
    pub fallback: bool,
}

/// A mod's UI for UI v2: its fragment package, the UI API it asks for, its panels in order.
#[derive(Debug, Clone)]
pub struct ModUi {
    pub id: Arc<str>,
    pub pkg: Arc<Package>,
    /// `[ui] api` as written (`^1`): what the placeholder of a mod this client cannot run says it needs.
    pub api: String,
    pub panels: Vec<ModPanel>,
}

impl ModUi {
    /// The mod's model fragment (`[ui] model`), when the package carries one.
    pub fn model(&self) -> Option<&str> {
        self.pkg
            .scripts()
            .find(|(p, _)| *p == MODEL_FILE)
            .and_then(|(_, b)| std::str::from_utf8(b).ok())
    }

    /// The mod's script files, in order (the model is not one).
    pub fn script_files(&self) -> Vec<(String, Arc<str>)> {
        self.pkg
            .scripts()
            .filter(|(p, _)| *p != MODEL_FILE)
            .filter_map(|(p, b)| Some((p.to_string(), Arc::from(std::str::from_utf8(b).ok()?))))
            .collect()
    }
}

/// One panel of the index.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct IndexPanel {
    pub id: String,
    pub point: String,
    pub mode: String,
    pub fallback: String,
}

/// One entry of `ui2/mods-ui.json`: a mod's fragment package in the build.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct ModIndexEntry {
    pub id: String,
    pub version: String,
    /// Relative to `ui2/`.
    pub file: String,
    /// blake3 of the package, hex.
    pub hash: String,
    pub api: String,
    pub panels: Vec<IndexPanel>,
}

/// Reads `ui2/mods-ui.json`.
///
/// # Errors
/// The text is not the index.
pub fn parse_mods_index(text: &str) -> Result<Vec<ModIndexEntry>, String> {
    serde_json::from_str(text).map_err(|e| format!("{MODS_INDEX_PATH}: {e}"))
}

impl ModIndexEntry {
    /// The package's path relative to the page.
    pub fn path(&self) -> String {
        format!("ui2/{}", self.file)
    }

    /// The downloaded package as the mod's UI: readable, the hash of the index, a fragment of this mod.
    ///
    /// # Errors
    /// Why the package cannot be used.
    pub fn open(&self, bytes: Arc<[u8]>) -> Result<ModUi, String> {
        let pkg = Package::read(bytes).map_err(|e| format!("{}: {e}", self.path()))?;
        let hash = crate::hash_hex(&pkg.hash());
        if !hash.eq_ignore_ascii_case(&self.hash) {
            return Err(format!(
                "{}: the package's hash {hash} is not the index's {}",
                self.path(),
                self.hash
            ));
        }
        if !matches!(pkg.manifest().kind, ui2_package::Kind::Fragment { .. })
            || pkg.manifest().id != self.id
        {
            return Err(format!(
                "{}: not the fragment of mod {}",
                self.path(),
                self.id
            ));
        }
        Ok(ModUi {
            id: Arc::from(self.id.as_str()),
            pkg: Arc::new(pkg),
            api: self.api.clone(),
            panels: self
                .panels
                .iter()
                .map(|p| ModPanel {
                    id: p.id.clone(),
                    point: p.point.clone(),
                    replace: p.mode == "replace",
                    fallback: p.fallback != "none",
                })
                .collect(),
        })
    }
}

/// What mounting a mod's UI did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModMount {
    pub id: Arc<str>,
    /// `(panel, point)` of each panel mounted, in order.
    pub panels: Vec<(String, String)>,
    /// Why the mod shows one placeholder in `mods` instead of its panels.
    pub refused: Option<String>,
    /// The panels left out, and why.
    pub skipped: Vec<String>,
}

/// The order of a mod's panel among the mod panels of a point: by load order, then by `[[ui.panels]]`.
fn order(mod_pos: usize, panel: usize) -> u32 {
    (mod_pos as u32)
        .saturating_mul(32)
        .saturating_add(panel as u32)
}

/// Mounts the panels of the mods in the skin's runtime, in load order.
fn mount_panels(
    runtime: &mut Runtime,
    skin: &Package,
    uis: &[(ModUi, Option<String>)],
) -> Vec<ModMount> {
    let mut out = Vec::new();
    for (pos, (ui, refused)) in uis.iter().enumerate() {
        let mut m = ModMount {
            id: Arc::clone(&ui.id),
            panels: Vec::new(),
            refused: refused.clone(),
            skipped: Vec::new(),
        };
        if let Some(why) = refused {
            // One line in `mods`: a fragment of one panel is mounted under the reason as its name (the placeholder
            // shows the name), else its first panel.
            let tries = [
                why.as_str(),
                ui.panels.first().map_or("", |p| p.id.as_str()),
            ];
            let mounted = tries.iter().find_map(|name| {
                runtime
                    .mount_fragment(
                        Arc::clone(&ui.pkg),
                        name,
                        MODS_POINT,
                        MountMode::Append,
                        order(pos, 0),
                    )
                    .ok()
            });
            match mounted {
                Some(id) => runtime.set_panel_failed(id, why),
                None => {
                    tracing::warn!(target: "ui2_host", "mod {}: no placeholder either: {why}", ui.id)
                }
            }
            out.push(m);
            continue;
        }
        for (i, p) in ui.panels.iter().enumerate() {
            let has = skin.slots().any(|(n, _)| n == p.point);
            let (slot, mode) = if has {
                let mode = if p.replace {
                    MountMode::Replace
                } else {
                    MountMode::Append
                };
                (p.point.as_str(), mode)
            } else if p.fallback {
                (MODS_POINT, MountMode::Append)
            } else {
                m.skipped.push(format!(
                    "{}: the skin has no point {} (fallback none)",
                    p.id, p.point
                ));
                continue;
            };
            match runtime.mount_fragment(Arc::clone(&ui.pkg), &p.id, slot, mode, order(pos, i)) {
                Ok(_) => m.panels.push((p.id.clone(), slot.to_string())),
                Err(e) => {
                    let ui2_runtime::LoadError::Fatal(why) = e;
                    tracing::warn!(target: "ui2_host", "mod {}: panel {} is not mounted: {why}", ui.id, p.id);
                    m.skipped.push(format!("{}: {why}", p.id));
                }
            }
        }
        out.push(m);
    }
    out
}

/// Whether a fragment can run in this client: versions, model paths and intents, as for a skin; `Err` is the
/// line its placeholder shows.
fn check_mod(ui: &ModUi) -> Result<(), String> {
    let mani = ui.pkg.manifest();
    if !matches!(mani.kind, ui2_package::Kind::Fragment { .. }) {
        return Err(format!("{}: not a mod fragment", ui.id));
    }
    if mani.api.0 != crate::SCRIPT_API.major || mani.api.1 > crate::SCRIPT_API.minor {
        return Err(format!("{}: needs interface API {}", ui.id, ui.api));
    }
    crate::registry::check_package(&ui.pkg).map_err(|e| format!("{}: {e}", ui.id))
}

impl V2Ui {
    /// The workers the mods' scripts run in (the skin's run in workers of their own): without one, a mod's
    /// panels work on their markup alone.
    pub fn set_mod_script_host(&mut self, host: Box<dyn ScriptHost>) {
        self.mods.script_host = Some(host);
    }

    /// Brings up the UI of the game's mods, in load order: for each, its model at `mods.<id>`, its scripts in a
    /// context of its own (`mod:<id>`), and its panels in the skin's extension points. A panel whose point the
    /// skin lacks goes to `mods` (or nowhere with `fallback = none`); a mod this client cannot run (another UI
    /// API, a model or scripts that do not load) shows one placeholder line in `mods`. The panels mounted before
    /// go first; the models and the state stay.
    pub fn mount_mods(&mut self, uis: Vec<ModUi>) -> Vec<ModMount> {
        self.unmount_mod_panels();
        self.unmount_gone_mods(&uis);
        let mut prepared = Vec::with_capacity(uis.len());
        for ui in uis {
            let refused = self.prepare_mod(&ui).err();
            if let Some(why) = &refused {
                tracing::warn!(target: "ui2_host", "mod UI not shown: {why}");
            }
            prepared.push((ui, refused));
        }
        let out = mount_panels(&mut self.skin.runtime, &self.skin.pkg, &prepared);
        self.mods.uis = prepared;
        self.mods.mounted_in = Some(Arc::clone(&self.skin.pkg));
        out
    }

    /// The mods of the last mount that are not in `uis` go: their model `mods.<id>`, their script state (set
    /// aside by the actions module, it comes back if the mod does) and their script context.
    fn unmount_gone_mods(&mut self, uis: &[ModUi]) {
        let gone: Vec<Arc<str>> = self
            .mods
            .uis
            .iter()
            .map(|(m, _)| Arc::clone(&m.id))
            .filter(|id| !uis.iter().any(|u| u.id == *id))
            .collect();
        for id in gone {
            self.store.unmount(&Namespace::Mod(Arc::clone(&id)));
            self.actions.set_mod_state(&id, None, &mut self.store);
            if let Some(s) = self.mods.scripts.as_mut() {
                s.sup.stop(&ScriptCtxId::Mod(Arc::clone(&id)));
            }
            self.mods.writes.retain(|(w, _)| *w != id);
        }
    }

    /// The mod panels go (a replaced element of the skin comes back).
    fn unmount_mod_panels(&mut self) {
        let runtime = &mut self.skin.runtime;
        for p in runtime.panels() {
            if p.owner.is_some() {
                runtime.unmount(p.id);
            }
        }
    }

    /// The model and the scripts of one mod; `Err` is the line of its placeholder.
    fn prepare_mod(&mut self, ui: &ModUi) -> Result<(), String> {
        check_mod(ui)?;
        let id = ui.id.as_ref();
        if let Some(model) = ui.model() {
            self.mount_mod_model(id, model)?;
        }
        self.mount_mod_state(ui)?;
        let files = ui.script_files();
        if files.is_empty() {
            return Ok(());
        }
        if self.mods.scripts.is_none() {
            let Some(host) = self.mods.script_host.take() else {
                tracing::info!(target: "ui2_host", "mod {id}: its scripts do not run here; its panels work on markup");
                return Ok(());
            };
            let skin = self.skin.pkg.manifest().id.clone();
            self.mods.scripts = Some(Scripts::for_mods(host, &self.skin.runtime, &skin));
        }
        let mut reads: Vec<String> = ui.pkg.script_reads().map(str::to_string).collect();
        if self.store.is_mounted(&Namespace::Mod(Arc::clone(&ui.id))) {
            reads.push(format!("mods.{id}"));
        }
        let bundle = ui2_script::ScriptBundle {
            files,
            reads,
            state_ns: ui2_script::StateNs::Mod(Arc::clone(&ui.id)),
            settings_ns: ui2_script::SettingsNs::Mod(Arc::clone(&ui.id)),
            intent_prefix: Some(format!("mods.{id}.")),
        };
        let settings = self
            .actions
            .settings_snapshot(&ui2_actions::SettingsNs::Mod(Arc::clone(&ui.id)));
        let Some(s) = self.mods.scripts.as_mut() else {
            return Ok(());
        };
        let init =
            ui2_script::ScriptInit::from_store(&self.store, &bundle, settings, s.env().clone())
                .map_err(|e| format!("{id}: its scripts cannot start ({e})"))?;
        match s
            .sup
            .start(ScriptCtxId::Mod(Arc::clone(&ui.id)), bundle, init)
        {
            Ok(()) => Ok(()),
            Err(ui2_script::ScriptError::Host(e)) => {
                tracing::info!(target: "ui2_host", "mod {id}: its scripts do not run here ({e}); markup only");
                Ok(())
            }
            Err(e) => Err(format!("{id}: its scripts cannot start ({e})")),
        }
    }

    /// The skin was built anew (its runtime has none of the mods' panels): they are mounted in it again. Called
    /// at the start of every frame; nothing to do while the skin is the one they are mounted in.
    pub(crate) fn keep_mod_panels(&mut self) {
        let same = self
            .mods
            .mounted_in
            .as_ref()
            .is_none_or(|p| Arc::ptr_eq(p, &self.skin.pkg));
        if same {
            return;
        }
        mount_panels(&mut self.skin.runtime, &self.skin.pkg, &self.mods.uis);
        self.mods.mounted_in = Some(Arc::clone(&self.skin.pkg));
    }
}

impl Mods {
    /// What the mods' script contexts answered since the last frame joins the skin's (`js`): their intents with
    /// the origin of their mod; a context that failed while loading is switched off like one that hung.
    pub(crate) fn drain_scripts(&mut self, now_ms: f64, js: &mut Option<ScriptOutput>) {
        let Some(s) = self.scripts.as_mut() else {
            return;
        };
        if s.sup.is_empty() {
            return;
        }
        let mut out = s.sup.drain(now_ms);
        if let Some((ctx, why)) = s.sup.take_fatal() {
            tracing::warn!(target: "ui2_host", "{}: {why}", ui2_script::ctx_name(&ctx));
            s.sup.stop(&ctx);
            out.failed.push(ctx);
        }
        for (ctx, i) in &mut out.intents {
            if let ScriptCtxId::Mod(id) = ctx
                && let Some(idx) = self.idx_of(id)
            {
                i.origin = Origin::Mod(idx);
            }
        }
        match js {
            None => *js = Some(out),
            Some(j) => {
                j.state_ops.append(&mut out.state_ops);
                j.intents.append(&mut out.intents);
                j.island_cmds.append(&mut out.island_cmds);
                j.settings.append(&mut out.settings);
                j.failed.append(&mut out.failed);
                j.log.append(&mut out.log);
            }
        }
    }

    /// The mods' script contexts get the frame: the delta they watch, their markup events, a new environment.
    pub(crate) fn post_scripts(
        &mut self,
        now_ms: f64,
        runtime: &Runtime,
        delta: Option<&ui2_model::FrameDelta>,
        events: &[ui2_model::wire::ScriptEvent],
        store: &ModelStore,
    ) {
        let Some(s) = self.scripts.as_mut() else {
            return;
        };
        if s.sup.is_empty() {
            return;
        }
        let env = s.env_change(runtime);
        s.sup.post(
            now_ms,
            &ui2_script::FrameInput {
                delta,
                events,
                env: env.as_ref(),
                store: Some(store),
            },
        );
    }

    /// Whether any mod script context runs.
    pub(crate) fn has_scripts(&self) -> bool {
        self.scripts.as_ref().is_some_and(|s| !s.sup.is_empty())
    }
}
/// The fragment of a mod's state (`[ui] state`, as the package's state defaults): one field per path, nested
/// tables as structs. `None` when the mod has no state.
fn state_fragment(defaults: &[(String, ui2_package::Lit)]) -> Result<Option<String>, String> {
    use std::collections::BTreeMap;
    use std::fmt::Write as _;
    #[derive(Default)]
    struct Table {
        fields: BTreeMap<String, Entry>,
    }
    enum Entry {
        Leaf(&'static str),
        Table(Table),
    }
    let mut root = Table::default();
    for (path, lit) in defaults {
        let ty = match lit {
            ui2_package::Lit::Bool(_) => "bool",
            ui2_package::Lit::Int(_) => "i32",
            ui2_package::Lit::Float(_) => "f32",
            ui2_package::Lit::Str(_) => "str",
            ui2_package::Lit::List(_) => {
                return Err(format!("state.{path}: a list is not a state value"));
            }
        };
        let segs: Vec<&str> = path.split('.').collect();
        let (last, dirs) = segs
            .split_last()
            .ok_or_else(|| "an empty state path".to_string())?;
        let mut at = &mut root;
        for d in dirs {
            let e = at
                .fields
                .entry((*d).to_string())
                .or_insert_with(|| Entry::Table(Table::default()));
            at = match e {
                Entry::Table(t) => t,
                Entry::Leaf(_) => return Err(format!("state.{path}: {d} is a value and a table")),
            };
        }
        at.fields.insert((*last).to_string(), Entry::Leaf(ty));
    }
    if root.fields.is_empty() {
        return Ok(None);
    }
    // Types first (named S0, S1, ... in the order they are met), then the root.
    fn emit(t: &Table, types: &mut Vec<String>) -> String {
        let mut body = String::new();
        for (name, e) in &t.fields {
            let ty = match e {
                Entry::Leaf(ty) => (*ty).to_string(),
                Entry::Table(inner) => {
                    let inner_body = emit(inner, types);
                    let n = format!("S{}", types.len());
                    types.push(format!("[type.{n}]\n{inner_body}"));
                    n
                }
            };
            let _ = writeln!(body, "{name} = \"{ty}\"");
        }
        body
    }
    let mut types = Vec::new();
    let root_body = emit(&root, &mut types);
    let mut src = types.join("\n");
    if !src.is_empty() {
        src.push('\n');
    }
    src.push_str("[root]\n");
    src.push_str(&root_body);
    Ok(Some(src))
}

/// The model value of a state default (`None`: it does not fit its field).
fn state_value(lit: &ui2_package::Lit) -> Option<Value> {
    match lit {
        ui2_package::Lit::Bool(b) => Some(Value::Bool(*b)),
        ui2_package::Lit::Int(i) => i32::try_from(*i).ok().map(Value::I32),
        ui2_package::Lit::Float(f) => Some(Value::F32(*f as f32)),
        ui2_package::Lit::Str(s) => Some(Value::Str(Arc::from(s.as_str()))),
        ui2_package::Lit::List(_) => None,
    }
}

/// The state writes of the markup: a mod panel's (`modstate.<id>.*`, which linking made of its `state.*`) go to
/// that mod's state, the skin's to the skin's. A skin's markup cannot name `modstate` (its converter refuses any
/// path but its own `state.*`), and the actions module keeps each mod in its own namespace.
pub(crate) fn markup_state_ops(
    actions: &mut ui2_actions::UiActions,
    writes: &[ui2_model::wire::StateOp],
    store: &mut ModelStore,
) -> Vec<ui2_actions::StateError> {
    let mut errors = Vec::new();
    for w in writes {
        let ns = match Namespace::of_path(&w.path) {
            Some(Namespace::ModState(id)) => ui2_actions::StateNs::Mod(id),
            _ => ui2_actions::StateNs::Skin,
        };
        errors.extend(actions.state_ops(ns, std::slice::from_ref(w), store));
    }
    errors
}

impl V2Ui {
    /// Mounts a mod's state `modstate.<id>` from its fragment's defaults (`[ui] state`): the first time in this UI
    /// with the defaults, later with what it held (the actions module sets it aside while it is unmounted).
    fn mount_mod_state(&mut self, ui: &ModUi) -> Result<(), String> {
        let defaults: Vec<(String, ui2_package::Lit)> = ui
            .pkg
            .state_defaults()
            .map(|(p, l)| (p.to_string(), l.clone()))
            .collect();
        let Some(src) = state_fragment(&defaults).map_err(|e| format!("{}: {e}", ui.id))? else {
            return Ok(());
        };
        let frag = Fragment::parse(&src).map_err(|e| format!("{}: its state: {e}", ui.id))?;
        let ns = Namespace::ModState(Arc::clone(&ui.id));
        if self.store.is_mounted(&ns) && self.mods.seeded.contains(&ui.id) {
            return Ok(());
        }
        let errs = self
            .actions
            .set_mod_state(&ui.id, Some(&frag), &mut self.store);
        if let Some(e) = errs.first() {
            return Err(format!("{}: its state: {e}", ui.id));
        }
        if self.mods.seeded.insert(Arc::clone(&ui.id)) {
            let mut w = self.store.writer(ns.owner());
            for (path, lit) in &defaults {
                if let Some(v) = state_value(lit) {
                    let full = format!("{}.{path}", ns.root_name());
                    if let Err(e) = w.set_value(&full, v) {
                        tracing::warn!(target: "ui2_host", "mod {}: state default {path}: {e}", ui.id);
                    }
                }
            }
        }
        Ok(())
    }

    /// The value of the root `modstate.<id>` (with this frame's writes), for tests and tools.
    pub fn mod_state_value(&self, id: &str) -> Option<&Value> {
        self.store
            .fragment_value(&Namespace::ModState(Arc::from(id)))
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
