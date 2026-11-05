//! `OptionsManager`: the game options of `UserSettings.ini`, kept in the `KeyValueStore` as
//! `civ5.options.<Key>`. The 23 functions K1-K3 and the EUI core call are real; the graphics and
//! resolution calls of the options menu (K20) are stubs.
//!
//! The defaults are the values of the `UserSettings.ini` the game writes on first start. A `Set*_Cached`
//! call keeps the value aside; `CommitGameOptions` makes it the current value and stores it
//! (SIMPLIFIED(SIMP-exe-17): the getters answer the committed value, the exe's cache rules are not
//! recovered).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use lua_sys::{Lua, LuaValue, MultiValue, Result, Table};

use crate::Inner;
use crate::registry::{Reg, Stub};

/// Prefix of the keys in the store.
pub const KEY_PREFIX: &str = "civ5.options.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Stored as `1` / `0`, a Lua boolean.
    Bool,
    /// Stored as a decimal number, a Lua number.
    Int,
}

/// A getter of `OptionsManager`: the Lua name, the key of `UserSettings.ini` and the default.
#[derive(Debug, Clone, Copy)]
pub struct Opt {
    pub getter: &'static str,
    pub key: &'static str,
    pub kind: Kind,
    pub default: i32,
}

const fn b(getter: &'static str, key: &'static str, default: i32) -> Opt {
    Opt {
        getter,
        key,
        kind: Kind::Bool,
        default,
    }
}

const fn n(getter: &'static str, key: &'static str, default: i32) -> Opt {
    Opt {
        getter,
        key,
        kind: Kind::Int,
        default,
    }
}

/// The getters, with the defaults of `UserSettings.ini` (`[GameSettings]`, `[InterfaceSettings]`).
pub const OPTIONS: &[Opt] = &[
    b("IsNoBasicHelp", "NoBasicHelp", 0),
    b("IsNoTileRecommendations", "NoTileRecommendations", 0),
    b("IsCivilianYields", "CivilianYields", 0),
    b("IsNoCitizenWarning", "NoCitizenWarning", 0),
    b("IsNoRewardPopups", "NoRewardPopups", 0),
    b("GetGridOn", "GridOn", 1),
    b("GetShowTradeOn", "ShowTradeOn", 1),
    b("GetResourceOn", "ResourceOn", 0),
    b("GetYieldOn", "YieldOn", 0),
    b("GetPolicyInfo", "PolicyInfo", 0),
    b("GetScoreList", "ScoreList", 0),
    b("GetMPScoreList", "MPScoreList", 1),
    b("GetSmallUIAssets", "SmallUIAssets", 1),
    b("GetAutoUnitCycle", "AutoUnitCycle", 1),
    // Not an ini key of the game section: the graphics options. The browser is windowed.
    b("GetFullscreen", "Fullscreen", 0),
    n("GetTooltip1Seconds", "Tooltip1Seconds", 150),
    n("GetTooltip2Seconds", "Tooltip2Seconds", 500),
];

/// The `Set<Name>_Cached` functions: the Lua name and the key they set.
pub const CACHED_SETTERS: &[(&str, &str)] = &[
    ("SetGridOn_Cached", "GridOn"),
    ("SetShowTradeOn_Cached", "ShowTradeOn"),
    ("SetYieldOn_Cached", "YieldOn"),
    ("SetResourceOn_Cached", "ResourceOn"),
    ("SetNoTileRecommendations_Cached", "NoTileRecommendations"),
    ("SetPolicyInfo_Cached", "PolicyInfo"),
];

/// Stubs of the options menu (K20): the answer when the browser has nothing to say.
const STUBS: &[(&str, Stub)] = &[
    ("CommitGraphicsOptions", Stub::Nil),
    ("SyncGameOptionsCache", Stub::Nil),
    ("SyncGraphicsOptionsCache", Stub::Nil),
    ("SyncResolutionOptionsCache", Stub::Nil),
    ("HasUserChangedResolution", Stub::Bool(false)),
    ("HasUserChangedGraphicsOptions", Stub::Bool(false)),
];

/// Values set with `Set*_Cached` and not yet committed, by key.
#[derive(Default)]
pub struct OptionsState {
    pub(crate) pending: RefCell<BTreeMap<&'static str, i32>>,
    /// The options of the menu that `options::install` has no table row for (WO-G, K20, [`crate::m4_wog`]): the
    /// pending value as the store text, by key.
    pub(crate) pending_gen: RefCell<BTreeMap<String, String>>,
    /// The volume knobs (music, SFX, speech, ambience), loaded from the store on first use.
    pub(crate) volumes: RefCell<Option<[f32; 4]>>,
}

fn find(key: &str) -> Option<&'static Opt> {
    OPTIONS.iter().find(|o| o.key == key)
}

fn current(inner: &Inner, o: &Opt) -> i32 {
    inner
        .host
        .kv()
        .get(&format!("{KEY_PREFIX}{}", o.key))
        .and_then(|s| s.trim().parse::<i32>().ok())
        .unwrap_or(o.default)
}

fn as_number(v: Option<&LuaValue<'_>>) -> i32 {
    match v {
        Some(LuaValue::Boolean(b)) => i32::from(*b),
        Some(LuaValue::Number(n)) => *n as i32,
        _ => 0,
    }
}

pub(crate) fn install<'l>(inner: &Rc<Inner>, lua: &'l Lua, g: &Table<'l>) -> Result<()> {
    let reg = Reg::new(inner);
    let t = lua.create_table();
    for o in OPTIONS {
        let i = Rc::clone(inner);
        let o = *o;
        reg.real(
            lua,
            &t,
            "OptionsManager",
            o.getter,
            move |_, _: MultiValue| {
                let v = current(&i, &o);
                Ok(match o.kind {
                    Kind::Bool => LuaValue::Boolean(v != 0),
                    Kind::Int => LuaValue::Number(f64::from(v)),
                })
            },
        )?;
    }
    for (name, key) in CACHED_SETTERS {
        let i = Rc::clone(inner);
        let (key, kind) = match find(key) {
            Some(o) => (o.key, o.kind),
            None => continue,
        };
        reg.real(
            lua,
            &t,
            "OptionsManager",
            name,
            move |_, args: MultiValue| {
                let mut v = as_number(args.get(0));
                if kind == Kind::Bool {
                    v = i32::from(v != 0);
                }
                i.options.pending.borrow_mut().insert(key, v);
                Ok(())
            },
        )?;
    }
    let i = Rc::clone(inner);
    reg.real(
        lua,
        &t,
        "OptionsManager",
        "CommitGameOptions",
        move |_, _: MultiValue| {
            let pending = std::mem::take(&mut *i.options.pending.borrow_mut());
            for (key, v) in pending {
                i.host
                    .kv()
                    .set(&format!("{KEY_PREFIX}{key}"), &v.to_string());
            }
            Ok(())
        },
    )?;
    for (name, ret) in STUBS {
        // SIMPLIFIED(SIMP-exe-13)
        reg.stub(lua, &t, "OptionsManager", name, "SIMP-exe-13", *ret)?;
    }
    g.raw_set("OptionsManager", t)?;
    reg.scope("OptionsManager");
    Ok(())
}
