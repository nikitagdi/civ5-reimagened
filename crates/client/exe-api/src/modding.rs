//! `Modding`: `OpenUserData` with persistence, `GetActivatedModEntryPoints` (no mods: an empty `for`),
//! the rest as stubs.
//!
//! `Modding.OpenUserData(id, version)` answers an object with `GetValue(key)` and `SetValue(key, value)`
//! (called with a dot, as the EUI does: `EUI_options.GetValue("UnitRibbon")`). The values live in the
//! `KeyValueStore` under `civ5.userdata.<id>.<version>` as one JSON object. A change is written at most
//! once per `userdata_flush_secs` (`ExeApi::tick`), on `ExeApi::flush` (the page is hiding) and when the
//! last handle of the state goes away. The keys the EUI uses: "Enhanced User Interface Options",
//! "LekmodData", "IngameHotkeyManager".
//!
//! SIMPLIFIED(SIMP-exe-19): a value is a number, a boolean or a string; a table is dropped.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use lua_sys::{Lua, LuaString, LuaValue, MultiValue, Result, Table};
use serde_json::{Map, Number, Value};

use crate::registry::{Reg, Stub};
use crate::{ExeHost, Inner};

pub const KEY_PREFIX: &str = "civ5.userdata.";

struct Doc {
    values: Map<String, Value>,
    dirty: bool,
}

/// The open user data of this state, by store key.
pub struct UserDataStore {
    docs: RefCell<HashMap<String, Doc>>,
    last_flush: Cell<f64>,
    interval: f64,
}

impl UserDataStore {
    pub fn new(interval_secs: f64) -> UserDataStore {
        UserDataStore {
            docs: RefCell::default(),
            last_flush: Cell::new(0.0),
            interval: interval_secs,
        }
    }

    fn open(&self, host: &dyn ExeHost, key: &str) {
        let mut docs = self.docs.borrow_mut();
        docs.entry(key.to_owned()).or_insert_with(|| {
            let values = host
                .kv()
                .get(key)
                .and_then(|s| serde_json::from_str::<Value>(&s).ok())
                .and_then(|v| match v {
                    Value::Object(m) => Some(m),
                    _ => None,
                })
                .unwrap_or_default();
            let mut doc = Doc {
                values,
                dirty: false,
            };
            apply_defaults(key, &mut doc);
            doc
        });
    }

    fn get(&self, key: &str, name: &str) -> Option<Value> {
        self.docs.borrow().get(key)?.values.get(name).cloned()
    }

    fn set(&self, key: &str, name: &str, value: Option<Value>) {
        let mut docs = self.docs.borrow_mut();
        let Some(doc) = docs.get_mut(key) else {
            return;
        };
        match value {
            Some(v) => {
                if doc.values.get(name) != Some(&v) {
                    doc.values.insert(name.to_owned(), v);
                    doc.dirty = true;
                }
            }
            None => {
                if doc.values.remove(name).is_some() {
                    doc.dirty = true;
                }
            }
        }
    }

    /// Writes every document that changed.
    pub fn flush(&self, host: &dyn ExeHost) {
        let mut docs = self.docs.borrow_mut();
        for (key, doc) in docs.iter_mut() {
            if doc.dirty {
                host.kv()
                    .set(key, &Value::Object(doc.values.clone()).to_string());
                doc.dirty = false;
            }
        }
    }

    /// Writes the changes when `interval` seconds passed since the last write.
    pub fn tick(&self, host: &dyn ExeHost, now_secs: f64) {
        if now_secs - self.last_flush.get() >= self.interval {
            self.last_flush.set(now_secs);
            self.flush(host);
        }
    }
}

/// The store key of the EUI options.
pub const EUI_OPTIONS_KEY: &str = "civ5.userdata.Enhanced User Interface Options.1";
/// The mark of the defaults below in the EUI options: the version of the defaults this store has had.
pub const DEFAULTS_MARK: &str = "ReimDefaults";
const DEFAULTS_VERSION: i64 = 2;

/// SIMPLIFIED(SIMP-exe-770): our defaults of the EUI options, the players' wish (live reports 2026-10-31): the civilization
/// list (`CivRibbon`) on, the unit and city ribbons of the UnitPanel (`UnitRibbon`, `CityRibbon`) off. The EUI reads an
/// unset option as on. They are set once per store (and again only when [`DEFAULTS_VERSION`] grows), over what was saved
/// before; a change the player makes after that stays.
///
/// SIMPLIFIED(SIMP-exe-1150): the Lekmod NotificationPanel keeps two civilization lists, one on the right edge and one
/// bottom left (`CivPanelL`), and shows the one `DB_bAnchorLeft` names; an unset option is the right one. Our players
/// want the trade partners list on the left (live report 2026-11-02, it was on both sides before the option read back
/// as a number), so the option is 1 unless the player has set it: the options menu writes it, and that stays.
fn apply_defaults(key: &str, doc: &mut Doc) {
    if key != EUI_OPTIONS_KEY {
        return;
    }
    let mark = doc.values.get(DEFAULTS_MARK).and_then(Value::as_i64);
    if mark >= Some(DEFAULTS_VERSION) {
        return;
    }
    if mark < Some(1) {
        for (name, v) in [("CivRibbon", 1), ("UnitRibbon", 0), ("CityRibbon", 0)] {
            doc.values
                .insert(name.to_owned(), Value::Number(Number::from(v)));
        }
    }
    doc.values
        .entry("DB_bAnchorLeft")
        .or_insert_with(|| Value::Number(Number::from(1)));
    doc.values.insert(
        DEFAULTS_MARK.to_owned(),
        Value::Number(Number::from(DEFAULTS_VERSION)),
    );
    doc.dirty = true;
}

fn to_json(v: &LuaValue<'_>) -> Option<Value> {
    match v {
        // The exe stores a boolean as the number 1 or 0: the Lua reads it back with `== 1` / `== 0` (the Lekmod
        // NotificationPanel `bAnchorLeft == 0 / == 1`, `Clock == 1`); a `true` there matched neither branch and
        // both civ lists (CivPanel and CivPanelL) stayed visible after the options menu had been saved.
        LuaValue::Boolean(b) => Some(Value::Number(Number::from(i64::from(*b)))),
        LuaValue::Number(n) => {
            if n.fract() == 0.0 && n.abs() < 9.0e15 {
                Some(Value::Number(Number::from(*n as i64)))
            } else {
                Number::from_f64(*n).map(Value::Number)
            }
        }
        LuaValue::String(s) => Some(Value::String(s.to_string_lossy().into_owned())),
        _ => None,
    }
}

fn from_json<'l>(v: Value) -> LuaValue<'l> {
    match v {
        // Values saved by an older build as booleans read as the numbers the exe would have stored.
        Value::Bool(b) => LuaValue::Number(f64::from(u8::from(b))),
        Value::Number(n) => LuaValue::Number(n.as_f64().unwrap_or(0.0)),
        Value::String(s) => LuaValue::String(LuaString(s.into_bytes())),
        _ => LuaValue::Nil,
    }
}

fn text(v: Option<&LuaValue<'_>>) -> String {
    match v {
        Some(LuaValue::String(s)) => s.to_string_lossy().into_owned(),
        Some(LuaValue::Number(n)) => {
            if n.fract() == 0.0 {
                format!("{}", *n as i64)
            } else {
                n.to_string()
            }
        }
        _ => String::new(),
    }
}

/// The store key of the user data `id` of version `version`.
pub fn store_key(id: &str, version: &str) -> String {
    format!("{KEY_PREFIX}{id}.{version}")
}

pub(crate) fn install<'l>(inner: &Rc<Inner>, lua: &'l Lua, g: &Table<'l>) -> Result<()> {
    let reg = Reg::new(inner);
    let t = lua.create_table();

    let i = Rc::clone(inner);
    reg.real(
        lua,
        &t,
        "Modding",
        "OpenUserData",
        move |lua, args: MultiValue| {
            let key = store_key(&text(args.get(0)), &text(args.get(1)));
            i.userdata.open(&*i.host, &key);
            let obj = lua.create_table();
            let (i1, k1) = (Rc::clone(&i), key.clone());
            obj.raw_set(
                "GetValue",
                lua.create_function(move |_, args: MultiValue| {
                    let name = text(args.get(0));
                    Ok(i1
                        .userdata
                        .get(&k1, &name)
                        .map(from_json)
                        .unwrap_or(LuaValue::Nil))
                }),
            )?;
            let (i2, k2) = (Rc::clone(&i), key);
            obj.raw_set(
                "SetValue",
                lua.create_function(move |_, args: MultiValue| {
                    let name = text(args.get(0));
                    let value = args.get(1).and_then(to_json);
                    i2.userdata.set(&k2, &name, value);
                    Ok(())
                }),
            )?;
            // SIMPLIFIED(SIMP-exe-396): the exe's user data is a SQLite database; the Lekmod hotkey manager
            // (OptionsMenu) queries it (`PRAGMA table_info`, `SELECT ... FROM SimpleValues`). `Query` answers no row
            // and the writes are dropped: the hotkeys it keeps there do not stick.
            obj.raw_set(
                "Query",
                lua.create_function(|lua, _: MultiValue| {
                    Ok(lua.create_function(|_, _: MultiValue| Ok(LuaValue::Nil)))
                }),
            )?;
            Ok(obj)
        },
    )?;

    // `for addin in Modding.GetActivatedModEntryPoints("InGameUIAddin") do`: no mod is active.
    reg.real(
        lua,
        &t,
        "Modding",
        "GetActivatedModEntryPoints",
        |lua, _: MultiValue| Ok(lua.create_function(|_, _: MultiValue| Ok(()))),
    )?;

    // SIMPLIFIED(SIMP-exe-14): no mods are active.
    let stubs: &[(&str, Stub)] = &[
        ("GetActivatedMods", Stub::Table),
        ("GetModProperty", Stub::Nil),
        ("GetEvaluatedFilePath", Stub::Nil),
        ("AnyActivatedModsContainPropertyValue", Stub::Bool(false)),
        ("GetMapFiles", Stub::Table),
        ("DeactivateMods", Stub::Nil),
    ];
    for (name, ret) in stubs {
        reg.stub(lua, &t, "Modding", name, "SIMP-exe-14", *ret)?;
    }
    g.raw_set("Modding", t)?;
    reg.scope("Modding");
    Ok(())
}
