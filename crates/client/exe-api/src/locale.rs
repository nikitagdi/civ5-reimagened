//! `Locale`: the text engine `loc` behind the Lua functions of `cvLuaLocaleLibrary`.
//!
//! `ConvertTextKey(key, ...)` and `Lookup(key, ...)` look the key up in the current language and compose
//! it with the arguments (a boolean is `0` / `1`, a number is a double, a string splits into forms at
//! `|`); `ToNumber`, `Compare`, `ToUpper`, `ToLower`, `HasTextKey`, `Length`, `Substring`, `EndsWith`,
//! `StartsWith`, `TruncateString`, `IsNilOrEmpty`, `IsNilOrWhitespace`, `GetHotKeyDescription` are the
//! functions of `loc`.
//!
//! SIMPLIFIED(SIMP-exe-16): when `ExeConfig::localizer` is `None`, `ConvertTextKey` and `Lookup` answer
//! the key itself and `HasTextKey` answers false.

use std::rc::Rc;

use lua_sys::{Lua, LuaString, LuaValue, MultiValue, Result, Table};

use crate::Inner;
use crate::registry::Reg;

fn text(v: Option<&LuaValue<'_>>) -> Option<String> {
    match v {
        Some(LuaValue::String(s)) => Some(s.to_string_lossy().into_owned()),
        Some(LuaValue::Number(n)) => Some(if n.fract() == 0.0 {
            format!("{}", *n as i64)
        } else {
            n.to_string()
        }),
        _ => None,
    }
}

fn int(v: Option<&LuaValue<'_>>, default: i32) -> i32 {
    match v {
        Some(LuaValue::Number(n)) => *n as i32,
        _ => default,
    }
}

/// The arguments after the key of `ConvertTextKey`.
fn loc_args(args: &MultiValue<'_>) -> Vec<loc::Arg> {
    args.iter()
        .skip(1)
        .map(|v| match v {
            LuaValue::Boolean(b) => loc::Arg::from_bool(*b),
            LuaValue::Number(n) => loc::Arg::Double(*n),
            LuaValue::String(s) => loc::Arg::Str(s.to_string_lossy().into_owned()),
            _ => loc::Arg::Str(String::new()),
        })
        .collect()
}

fn lua_str<'l>(s: String) -> LuaValue<'l> {
    LuaValue::String(LuaString(s.into_bytes()))
}

pub(crate) fn install<'l>(inner: &Rc<Inner>, lua: &'l Lua, g: &Table<'l>) -> Result<()> {
    let reg = Reg::new(inner);
    let t = lua.create_table();

    for name in ["ConvertTextKey", "Lookup"] {
        let i = Rc::clone(inner);
        reg.real(lua, &t, "Locale", name, move |_, args: MultiValue| {
            let key = text(args.get(0)).unwrap_or_default();
            Ok(match &i.cfg.localizer {
                Some(l) => l.convert(&key, &loc_args(&args)),
                None => key,
            })
        })?;
    }
    let i = Rc::clone(inner);
    reg.real(
        lua,
        &t,
        "Locale",
        "HasTextKey",
        move |_, args: MultiValue| {
            let key = text(args.get(0)).unwrap_or_default();
            Ok(i.cfg.localizer.as_ref().is_some_and(|l| l.has_key(&key)))
        },
    )?;
    let i = Rc::clone(inner);
    reg.real(lua, &t, "Locale", "ToNumber", move |_, args: MultiValue| {
        let v = match args.get(0) {
            Some(LuaValue::Number(n)) => *n,
            _ => 0.0,
        };
        let pattern = text(args.get(1)).unwrap_or_default();
        Ok(match &i.cfg.localizer {
            Some(l) => l.to_number(v, &pattern),
            None => v.to_string(),
        })
    })?;
    let i = Rc::clone(inner);
    reg.real(lua, &t, "Locale", "ToUpper", move |_, args: MultiValue| {
        let s = text(args.get(0)).unwrap_or_default();
        Ok(match &i.cfg.localizer {
            Some(l) => l.to_upper(&s),
            None => s.to_uppercase(),
        })
    })?;
    let i = Rc::clone(inner);
    reg.real(lua, &t, "Locale", "ToLower", move |_, args: MultiValue| {
        let s = text(args.get(0)).unwrap_or_default();
        Ok(match &i.cfg.localizer {
            Some(l) => l.to_lower(&s),
            None => s.to_lowercase(),
        })
    })?;
    let i = Rc::clone(inner);
    reg.real(lua, &t, "Locale", "Compare", move |_, args: MultiValue| {
        let a = text(args.get(0)).unwrap_or_default();
        let b = text(args.get(1)).unwrap_or_default();
        Ok(match &i.cfg.localizer {
            Some(l) => l.compare(&a, &b),
            None => loc::compare(&a, &b),
        })
    })?;
    reg.real(lua, &t, "Locale", "Length", |_, args: MultiValue| {
        Ok(loc::strings::length(&text(args.get(0)).unwrap_or_default()) as i32)
    })?;
    reg.real(lua, &t, "Locale", "Substring", |_, args: MultiValue| {
        let s = text(args.get(0)).unwrap_or_default();
        Ok(loc::strings::substring(
            &s,
            int(args.get(1), 1),
            int(args.get(2), i32::MAX),
        ))
    })?;
    reg.real(lua, &t, "Locale", "EndsWith", |_, args: MultiValue| {
        let s = text(args.get(0)).unwrap_or_default();
        let suffix = text(args.get(1)).unwrap_or_default();
        Ok(loc::strings::ends_with(&s, &suffix))
    })?;
    reg.real(lua, &t, "Locale", "StartsWith", |_, args: MultiValue| {
        let s = text(args.get(0)).unwrap_or_default();
        let prefix = text(args.get(1)).unwrap_or_default();
        Ok(loc::strings::starts_with(&s, &prefix))
    })?;
    reg.real(
        lua,
        &t,
        "Locale",
        "TruncateString",
        |_, args: MultiValue| {
            let s = text(args.get(0)).unwrap_or_default();
            let ellipsis = !matches!(
                args.get(2),
                Some(LuaValue::Boolean(false)) | Some(LuaValue::Nil) | None
            );
            Ok(
                match loc::strings::truncate_string(&s, int(args.get(1), 0), ellipsis) {
                    Ok(t) => lua_str(t),
                    Err(_) => LuaValue::Nil,
                },
            )
        },
    )?;
    reg.real(lua, &t, "Locale", "IsNilOrEmpty", |_, args: MultiValue| {
        let s = text(args.get(0));
        Ok(loc::strings::is_nil_or_empty(s.as_deref()))
    })?;
    reg.real(
        lua,
        &t,
        "Locale",
        "IsNilOrWhitespace",
        |_, args: MultiValue| {
            let s = text(args.get(0));
            Ok(loc::strings::is_nil_or_whitespace(s.as_deref()))
        },
    )?;
    reg.real(
        lua,
        &t,
        "Locale",
        "GetHotKeyDescription",
        |_, args: MultiValue| {
            let s = text(args.get(0)).unwrap_or_default();
            Ok(loc::strings::get_hot_key_description(&s)
                .map(lua_str)
                .unwrap_or(LuaValue::Nil))
        },
    )?;

    g.raw_set("Locale", t)?;
    reg.scope("Locale");
    Ok(())
}
