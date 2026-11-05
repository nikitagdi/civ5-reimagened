//! Conversions between Rust types and Lua values, single and multiple.
//!
//! Numbers follow the C API the exe's bindings use: a Lua number converts to an integer
//! like `lua_tointeger` (the fraction is dropped), a numeric string converts to a number, and a
//! number converts to a string. `bool` is Lua truthiness.

use std::collections::{BTreeMap, HashMap};

use crate::error::{LuaError, LuaErrorKind, Result};
use crate::state::Lua;
use crate::value::{LightUserData, LuaString, LuaValue, MultiValue, Variadic};
use crate::{AnyUserData, Function, Table};

/// Rust value to Lua value.
pub trait ToLua<'lua>: Sized {
    fn to_lua(self, lua: &'lua Lua) -> Result<LuaValue<'lua>>;
}

/// Lua value to Rust value.
pub trait FromLua<'lua>: Sized {
    fn from_lua(value: LuaValue<'lua>, lua: &'lua Lua) -> Result<Self>;
}

/// Rust value(s) to a list of Lua values (call arguments, function results).
pub trait ToLuaMulti<'lua>: Sized {
    fn to_lua_multi(self, lua: &'lua Lua) -> Result<MultiValue<'lua>>;
}

/// A list of Lua values (call arguments, function results) to Rust value(s). Missing values are
/// `nil`, extra values are dropped.
pub trait FromLuaMulti<'lua>: Sized {
    fn from_lua_multi(values: MultiValue<'lua>, lua: &'lua Lua) -> Result<Self>;
}

fn mismatch(v: &LuaValue<'_>, to: &str) -> LuaError {
    LuaError::conversion(v.type_name(), to)
}

impl<'lua> ToLua<'lua> for LuaValue<'lua> {
    fn to_lua(self, _: &'lua Lua) -> Result<LuaValue<'lua>> {
        Ok(self)
    }
}

impl<'lua> FromLua<'lua> for LuaValue<'lua> {
    fn from_lua(value: LuaValue<'lua>, _: &'lua Lua) -> Result<Self> {
        Ok(value)
    }
}

/// Always `nil`; reads anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Nil;

impl<'lua> ToLua<'lua> for Nil {
    fn to_lua(self, _: &'lua Lua) -> Result<LuaValue<'lua>> {
        Ok(LuaValue::Nil)
    }
}

impl<'lua> FromLua<'lua> for Nil {
    fn from_lua(_: LuaValue<'lua>, _: &'lua Lua) -> Result<Self> {
        Ok(Nil)
    }
}

macro_rules! handle_conv {
    ($t:ident, $variant:ident, $name:literal) => {
        impl<'lua> ToLua<'lua> for $t<'lua> {
            fn to_lua(self, _: &'lua Lua) -> Result<LuaValue<'lua>> {
                Ok(LuaValue::$variant(self))
            }
        }

        impl<'lua> FromLua<'lua> for $t<'lua> {
            fn from_lua(value: LuaValue<'lua>, _: &'lua Lua) -> Result<Self> {
                match value {
                    LuaValue::$variant(h) => Ok(h),
                    other => Err(mismatch(&other, $name)),
                }
            }
        }
    };
}

handle_conv!(Table, Table, "table");
handle_conv!(Function, Function, "function");
handle_conv!(AnyUserData, UserData, "userdata");

impl<'lua> ToLua<'lua> for bool {
    fn to_lua(self, _: &'lua Lua) -> Result<LuaValue<'lua>> {
        Ok(LuaValue::Boolean(self))
    }
}

impl<'lua> FromLua<'lua> for bool {
    fn from_lua(value: LuaValue<'lua>, _: &'lua Lua) -> Result<Self> {
        Ok(value.is_truthy())
    }
}

impl<'lua> ToLua<'lua> for LightUserData {
    fn to_lua(self, _: &'lua Lua) -> Result<LuaValue<'lua>> {
        Ok(LuaValue::LightUserData(self))
    }
}

impl<'lua> ToLua<'lua> for LuaString {
    fn to_lua(self, _: &'lua Lua) -> Result<LuaValue<'lua>> {
        Ok(LuaValue::String(self))
    }
}

impl<'lua> FromLua<'lua> for LuaString {
    fn from_lua(value: LuaValue<'lua>, _: &'lua Lua) -> Result<Self> {
        match value {
            LuaValue::String(s) => Ok(s),
            LuaValue::Number(n) => Ok(LuaString(number_to_string(n).into_bytes())),
            other => Err(mismatch(&other, "string")),
        }
    }
}

impl<'lua> ToLua<'lua> for String {
    fn to_lua(self, _: &'lua Lua) -> Result<LuaValue<'lua>> {
        Ok(LuaValue::String(LuaString(self.into_bytes())))
    }
}

impl<'lua> ToLua<'lua> for &str {
    fn to_lua(self, _: &'lua Lua) -> Result<LuaValue<'lua>> {
        Ok(LuaValue::String(LuaString(self.as_bytes().to_vec())))
    }
}

impl<'lua> ToLua<'lua> for &String {
    fn to_lua(self, _: &'lua Lua) -> Result<LuaValue<'lua>> {
        Ok(LuaValue::String(LuaString(self.as_bytes().to_vec())))
    }
}

impl<'lua> FromLua<'lua> for String {
    fn from_lua(value: LuaValue<'lua>, _: &'lua Lua) -> Result<Self> {
        match value {
            LuaValue::String(s) => Ok(String::from_utf8_lossy(&s.0).into_owned()),
            LuaValue::Number(n) => Ok(number_to_string(n)),
            other => Err(mismatch(&other, "string")),
        }
    }
}

/// Number to text as Lua does (`%.14g`).
pub(crate) fn number_to_string(n: f64) -> String {
    if n.is_nan() {
        return if n.is_sign_negative() { "-nan" } else { "nan" }.into();
    }
    if n.is_infinite() {
        return if n < 0.0 { "-inf" } else { "inf" }.into();
    }
    if n == n.trunc() && n.abs() < 1e15 {
        return format!("{}", n as i64);
    }
    // %.14g: 14 significant digits, trailing zeros dropped, exponent form when needed.
    let exp = n.abs().log10().floor() as i32;
    if (-5..14).contains(&exp) {
        let decimals = (13 - exp).max(0) as usize;
        let s = format!("{n:.decimals$}");
        let s = s.trim_end_matches('0').trim_end_matches('.');
        s.to_owned()
    } else {
        let s = format!("{n:.13e}");
        let (mant, e) = s.split_once('e').unwrap_or((&s, "0"));
        let mant = mant.trim_end_matches('0').trim_end_matches('.');
        let e: i32 = e.parse().unwrap_or(0);
        format!("{mant}e{}{:02}", if e < 0 { '-' } else { '+' }, e.abs())
    }
}

fn parse_number(s: &[u8]) -> Option<f64> {
    let s = std::str::from_utf8(s).ok()?.trim();
    if s.is_empty() {
        return None;
    }
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        return u64::from_str_radix(hex, 16).ok().map(|v| v as f64);
    }
    // Rust accepts "inf"/"nan"/"infinity", which Lua's strtod accepts too, but "1_0" no.
    s.parse::<f64>().ok()
}

fn to_number(value: &LuaValue<'_>, to: &str) -> Result<f64> {
    match value {
        LuaValue::Number(n) => Ok(*n),
        LuaValue::String(s) => parse_number(&s.0).ok_or_else(|| mismatch(value, to)),
        other => Err(mismatch(other, to)),
    }
}

macro_rules! int_conv {
    ($($t:ty),*) => {$(
        impl<'lua> ToLua<'lua> for $t {
            fn to_lua(self, _: &'lua Lua) -> Result<LuaValue<'lua>> {
                Ok(LuaValue::Number(self as f64))
            }
        }

        impl<'lua> FromLua<'lua> for $t {
            fn from_lua(value: LuaValue<'lua>, _: &'lua Lua) -> Result<Self> {
                // `as` truncates toward zero and saturates, like a C cast of lua_Number.
                Ok(to_number(&value, stringify!($t))? as $t)
            }
        }
    )*};
}

int_conv!(i8, u8, i16, u16, i32, u32, i64, u64, isize, usize, f32, f64);

impl<'lua, T: ToLua<'lua>> ToLua<'lua> for Option<T> {
    fn to_lua(self, lua: &'lua Lua) -> Result<LuaValue<'lua>> {
        match self {
            Some(v) => v.to_lua(lua),
            None => Ok(LuaValue::Nil),
        }
    }
}

impl<'lua, T: FromLua<'lua>> FromLua<'lua> for Option<T> {
    fn from_lua(value: LuaValue<'lua>, lua: &'lua Lua) -> Result<Self> {
        match value {
            LuaValue::Nil => Ok(None),
            v => T::from_lua(v, lua).map(Some),
        }
    }
}

impl<'lua, T: ToLua<'lua>> ToLua<'lua> for Vec<T> {
    fn to_lua(self, lua: &'lua Lua) -> Result<LuaValue<'lua>> {
        Ok(LuaValue::Table(lua.create_sequence_from(self)?))
    }
}

impl<'lua, T: FromLua<'lua>> FromLua<'lua> for Vec<T> {
    fn from_lua(value: LuaValue<'lua>, lua: &'lua Lua) -> Result<Self> {
        match value {
            LuaValue::Table(t) => t.sequence_values(),
            other => {
                let _ = lua;
                Err(mismatch(&other, "sequence table"))
            }
        }
    }
}

impl<'lua, K: ToLua<'lua>, V: ToLua<'lua>> ToLua<'lua> for HashMap<K, V> {
    fn to_lua(self, lua: &'lua Lua) -> Result<LuaValue<'lua>> {
        Ok(LuaValue::Table(lua.create_table_from(self)?))
    }
}

impl<'lua, K: ToLua<'lua>, V: ToLua<'lua>> ToLua<'lua> for BTreeMap<K, V> {
    fn to_lua(self, lua: &'lua Lua) -> Result<LuaValue<'lua>> {
        Ok(LuaValue::Table(lua.create_table_from(self)?))
    }
}

impl<'lua, K, V> FromLua<'lua> for HashMap<K, V>
where
    K: FromLua<'lua> + Eq + std::hash::Hash,
    V: FromLua<'lua>,
{
    fn from_lua(value: LuaValue<'lua>, _: &'lua Lua) -> Result<Self> {
        match value {
            LuaValue::Table(t) => t.pairs::<K, V>().collect(),
            other => Err(mismatch(&other, "table")),
        }
    }
}

// Multiple values.

impl<'lua, T: ToLua<'lua>> ToLuaMulti<'lua> for T {
    fn to_lua_multi(self, lua: &'lua Lua) -> Result<MultiValue<'lua>> {
        Ok(MultiValue(vec![self.to_lua(lua)?]))
    }
}

impl<'lua, T: FromLua<'lua>> FromLuaMulti<'lua> for T {
    fn from_lua_multi(values: MultiValue<'lua>, lua: &'lua Lua) -> Result<Self> {
        let first = values.0.into_iter().next().unwrap_or(LuaValue::Nil);
        T::from_lua(first, lua).map_err(|e| arg_error(1, e))
    }
}

impl<'lua> ToLuaMulti<'lua> for () {
    fn to_lua_multi(self, _: &'lua Lua) -> Result<MultiValue<'lua>> {
        Ok(MultiValue(Vec::new()))
    }
}

impl<'lua> FromLuaMulti<'lua> for () {
    fn from_lua_multi(_: MultiValue<'lua>, _: &'lua Lua) -> Result<Self> {
        Ok(())
    }
}

impl<'lua> ToLuaMulti<'lua> for MultiValue<'lua> {
    fn to_lua_multi(self, _: &'lua Lua) -> Result<MultiValue<'lua>> {
        Ok(self)
    }
}

impl<'lua> FromLuaMulti<'lua> for MultiValue<'lua> {
    fn from_lua_multi(values: MultiValue<'lua>, _: &'lua Lua) -> Result<Self> {
        Ok(values)
    }
}

impl<'lua, T: ToLua<'lua>> ToLuaMulti<'lua> for Variadic<T> {
    fn to_lua_multi(self, lua: &'lua Lua) -> Result<MultiValue<'lua>> {
        self.0
            .into_iter()
            .map(|v| v.to_lua(lua))
            .collect::<Result<Vec<_>>>()
            .map(MultiValue)
    }
}

impl<'lua, T: FromLua<'lua>> FromLuaMulti<'lua> for Variadic<T> {
    fn from_lua_multi(values: MultiValue<'lua>, lua: &'lua Lua) -> Result<Self> {
        values
            .0
            .into_iter()
            .map(|v| T::from_lua(v, lua))
            .collect::<Result<Vec<_>>>()
            .map(Variadic)
    }
}

macro_rules! tuple_multi {
    ($(($($name:ident),+))+) => {$(
        #[allow(non_snake_case)]
        impl<'lua, $($name: ToLua<'lua>),+> ToLuaMulti<'lua> for ($($name,)+) {
            fn to_lua_multi(self, lua: &'lua Lua) -> Result<MultiValue<'lua>> {
                let ($($name,)+) = self;
                Ok(MultiValue(vec![$($name.to_lua(lua)?),+]))
            }
        }

        #[allow(non_snake_case)]
        impl<'lua, $($name: FromLua<'lua>),+> FromLuaMulti<'lua> for ($($name,)+) {
            fn from_lua_multi(values: MultiValue<'lua>, lua: &'lua Lua) -> Result<Self> {
                let mut it = values.0.into_iter();
                let mut pos = 0usize;
                Ok(($({
                    pos += 1;
                    $name::from_lua(it.next().unwrap_or(LuaValue::Nil), lua)
                        .map_err(|e| arg_error(pos, e))?
                },)+))
            }
        }
    )+};
}

fn arg_error(pos: usize, e: LuaError) -> LuaError {
    if e.kind == LuaErrorKind::Conversion {
        LuaError::new(
            LuaErrorKind::Conversion,
            format!("bad argument #{pos} ({})", e.message),
        )
    } else {
        e
    }
}

tuple_multi! {
    (A)
    (A, B)
    (A, B, C)
    (A, B, C, D)
    (A, B, C, D, E)
    (A, B, C, D, E, F)
    (A, B, C, D, E, F, G)
    (A, B, C, D, E, F, G, H)
}

// The variadic tail after fixed values: `(A, Variadic<B>)` is common for callbacks.
impl<'lua, A: FromLua<'lua>, B: FromLua<'lua>> FromLuaMulti<'lua> for (A, Variadic<B>) {
    fn from_lua_multi(values: MultiValue<'lua>, lua: &'lua Lua) -> Result<Self> {
        let mut it = values.0.into_iter();
        let a =
            A::from_lua(it.next().unwrap_or(LuaValue::Nil), lua).map_err(|e| arg_error(1, e))?;
        let rest = it
            .enumerate()
            .map(|(i, v)| B::from_lua(v, lua).map_err(|e| arg_error(i + 2, e)))
            .collect::<Result<Vec<_>>>()?;
        Ok((a, Variadic(rest)))
    }
}
