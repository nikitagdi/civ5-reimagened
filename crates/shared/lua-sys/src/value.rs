//! Lua values and handles. A handle borrows the [`Lua`] it came from and owns a registry slot.
//! [`RegistryKey`] is the handle without a lifetime, for storing in Rust structures.

use std::cell::RefCell;
use std::ffi::{c_int, c_void};
use std::fmt;
use std::rc::Rc;

use crate::error::{LuaError, Result};
use crate::ffi;
use crate::state::{Lua, StackGuard, ensure_stack, push_bytes, string_bytes};

/// A Lua string: bytes, usually UTF-8 text.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct LuaString(pub Vec<u8>);

impl LuaString {
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn to_str(&self) -> Result<&str> {
        std::str::from_utf8(&self.0).map_err(|_| {
            LuaError::new(crate::LuaErrorKind::Conversion, "string is not valid UTF-8")
        })
    }

    pub fn to_string_lossy(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.0)
    }
}

impl From<&str> for LuaString {
    fn from(s: &str) -> Self {
        LuaString(s.as_bytes().to_vec())
    }
}

impl From<String> for LuaString {
    fn from(s: String) -> Self {
        LuaString(s.into_bytes())
    }
}

impl From<&[u8]> for LuaString {
    fn from(s: &[u8]) -> Self {
        LuaString(s.to_vec())
    }
}

impl From<Vec<u8>> for LuaString {
    fn from(s: Vec<u8>) -> Self {
        LuaString(s)
    }
}

/// A `lightuserdata`: an address Lua does not own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LightUserData(pub *mut c_void);

/// Owner of one registry slot, tied to the borrowed state. Dropping releases the slot.
pub(crate) struct LuaRef<'lua> {
    pub(crate) lua: &'lua Lua,
    pub(crate) idx: c_int,
}

impl<'lua> LuaRef<'lua> {
    /// Takes the value on top of the stack.
    pub(crate) fn from_top(lua: &'lua Lua) -> Self {
        LuaRef {
            lua,
            idx: lua.pop_ref(),
        }
    }

    /// Pushes the value; needs 1 free slot.
    pub(crate) fn push(&self) {
        self.lua.push_ref(self.idx)
    }

    /// Raw equality (`rawequal`), no metamethods.
    pub(crate) fn raw_eq(&self, other: &LuaRef<'_>) -> bool {
        let l = self.lua.l();
        let _g = StackGuard::new(l);
        // SAFETY: two pushes, rawequal calls no metamethods.
        unsafe {
            ensure_stack(l, 2);
            self.push();
            other.push();
            ffi::lua_rawequal(l, -1, -2) != 0
        }
    }

    pub(crate) fn pointer(&self) -> *const c_void {
        let l = self.lua.l();
        let _g = StackGuard::new(l);
        // SAFETY: one push; topointer reads only.
        unsafe {
            ensure_stack(l, 1);
            self.push();
            ffi::lua_topointer(l, -1)
        }
    }
}

impl Clone for LuaRef<'_> {
    fn clone(&self) -> Self {
        let l = self.lua.l();
        let _g = StackGuard::new(l);
        // SAFETY: push then ref.
        unsafe { ensure_stack(l, 1) };
        self.push();
        LuaRef {
            lua: self.lua,
            idx: self.lua.pop_ref(),
        }
    }
}

impl Drop for LuaRef<'_> {
    fn drop(&mut self) {
        let l = self.lua.l();
        // SAFETY: unref does rawgeti/rawseti on the registry; no Lua code runs. The `'lua`
        // borrow keeps the state alive.
        unsafe {
            ffi::lua_checkstack(l, 3);
            ffi::luaL_unref(l, ffi::LUA_REGISTRYINDEX, self.idx);
        }
    }
}

macro_rules! handle {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Clone)]
        pub struct $name<'lua>(pub(crate) LuaRef<'lua>);

        impl PartialEq for $name<'_> {
            fn eq(&self, other: &Self) -> bool {
                self.0.raw_eq(&other.0)
            }
        }

        impl fmt::Debug for $name<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({:p})", stringify!($name), self.0.pointer())
            }
        }
    };
}

handle!(
    /// A Lua table.
    Table
);
handle!(
    /// A Lua function (Lua or C).
    Function
);
handle!(
    /// A Lua userdata, of any Rust payload type (see [`AnyUserData::borrow`]).
    AnyUserData
);
handle!(
    /// A coroutine. Only kept and passed around; scripts drive it.
    Thread
);

/// A Lua value. Strings are copied out; collectable values are handles.
#[derive(Clone, Debug, PartialEq)]
pub enum LuaValue<'lua> {
    Nil,
    Boolean(bool),
    LightUserData(LightUserData),
    /// Lua 5.1 numbers are doubles; `lua_Integer` (32 bit) is only the C API's view of them.
    Number(f64),
    String(LuaString),
    Table(Table<'lua>),
    Function(Function<'lua>),
    UserData(AnyUserData<'lua>),
    Thread(Thread<'lua>),
}

impl<'lua> LuaValue<'lua> {
    pub fn type_name(&self) -> &'static str {
        match self {
            LuaValue::Nil => "nil",
            LuaValue::Boolean(_) => "boolean",
            LuaValue::LightUserData(_) | LuaValue::UserData(_) => "userdata",
            LuaValue::Number(_) => "number",
            LuaValue::String(_) => "string",
            LuaValue::Table(_) => "table",
            LuaValue::Function(_) => "function",
            LuaValue::Thread(_) => "thread",
        }
    }

    pub fn is_nil(&self) -> bool {
        matches!(self, LuaValue::Nil)
    }

    /// Lua truthiness: everything except `nil` and `false`.
    pub fn is_truthy(&self) -> bool {
        !matches!(self, LuaValue::Nil | LuaValue::Boolean(false))
    }

    pub fn as_table(&self) -> Option<&Table<'lua>> {
        match self {
            LuaValue::Table(t) => Some(t),
            _ => None,
        }
    }

    pub fn as_function(&self) -> Option<&Function<'lua>> {
        match self {
            LuaValue::Function(f) => Some(f),
            _ => None,
        }
    }

    pub fn as_userdata(&self) -> Option<&AnyUserData<'lua>> {
        match self {
            LuaValue::UserData(u) => Some(u),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            LuaValue::String(s) => std::str::from_utf8(&s.0).ok(),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            LuaValue::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            LuaValue::Boolean(b) => Some(*b),
            _ => None,
        }
    }
}

/// Reads the value at `idx` of the current thread.
///
/// SAFETY: `idx` is a valid index (or a pseudo-index) of the current thread.
pub(crate) fn read_lua_value(lua: &Lua, idx: c_int) -> LuaValue<'_> {
    let l = lua.l();
    // SAFETY: per the contract; strings are only read when the type is string.
    unsafe {
        let ty = ffi::lua_type(l, idx);
        match ty {
            ffi::LUA_TNIL | ffi::LUA_TNONE => LuaValue::Nil,
            ffi::LUA_TBOOLEAN => LuaValue::Boolean(ffi::lua_toboolean(l, idx) != 0),
            ffi::LUA_TLIGHTUSERDATA => {
                LuaValue::LightUserData(LightUserData(ffi::lua_touserdata(l, idx)))
            }
            ffi::LUA_TNUMBER => LuaValue::Number(ffi::lua_tonumber(l, idx)),
            ffi::LUA_TSTRING => LuaValue::String(LuaString(string_bytes(l, idx))),
            _ => {
                // Copy to the top (relative indices shift by one after the push).
                ensure_stack(l, 1);
                ffi::lua_pushvalue(l, idx);
                let r = LuaRef::from_top(lua);
                match ty {
                    ffi::LUA_TTABLE => LuaValue::Table(Table(r)),
                    ffi::LUA_TFUNCTION => LuaValue::Function(Function(r)),
                    ffi::LUA_TUSERDATA => LuaValue::UserData(AnyUserData(r)),
                    _ => LuaValue::Thread(Thread(r)),
                }
            }
        }
    }
}

/// Pushes `v`; needs 1 free slot.
pub(crate) fn push_lua_value(lua: &Lua, v: &LuaValue<'_>) {
    let l = lua.l();
    // SAFETY: the caller reserved a slot; pushes of non-strings allocate nothing, strings go
    // through the GC-paused push.
    unsafe {
        match v {
            LuaValue::Nil => ffi::lua_pushnil(l),
            LuaValue::Boolean(b) => ffi::lua_pushboolean(l, c_int::from(*b)),
            LuaValue::LightUserData(p) => ffi::lua_pushlightuserdata(l, p.0),
            LuaValue::Number(n) => ffi::lua_pushnumber(l, *n),
            LuaValue::String(s) => push_bytes(l, &s.0),
            LuaValue::Table(t) => t.0.push(),
            LuaValue::Function(f) => f.0.push(),
            LuaValue::UserData(u) => u.0.push(),
            LuaValue::Thread(t) => t.0.push(),
        }
    }
}

/// Zero or more values, as arguments or results.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MultiValue<'lua>(pub(crate) Vec<LuaValue<'lua>>);

impl<'lua> MultiValue<'lua> {
    pub fn new() -> Self {
        MultiValue(Vec::new())
    }

    pub fn from_vec(v: Vec<LuaValue<'lua>>) -> Self {
        MultiValue(v)
    }

    pub fn into_vec(self) -> Vec<LuaValue<'lua>> {
        self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> std::slice::Iter<'_, LuaValue<'lua>> {
        self.0.iter()
    }

    pub fn get(&self, i: usize) -> Option<&LuaValue<'lua>> {
        self.0.get(i)
    }

    pub fn push_back(&mut self, v: LuaValue<'lua>) {
        self.0.push(v);
    }
}

impl<'lua> IntoIterator for MultiValue<'lua> {
    type Item = LuaValue<'lua>;
    type IntoIter = std::vec::IntoIter<LuaValue<'lua>>;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<'lua> FromIterator<LuaValue<'lua>> for MultiValue<'lua> {
    fn from_iter<I: IntoIterator<Item = LuaValue<'lua>>>(iter: I) -> Self {
        MultiValue(iter.into_iter().collect())
    }
}

/// All remaining arguments (or results) as typed values.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Variadic<T>(pub Vec<T>);

impl<T> std::ops::Deref for Variadic<T> {
    type Target = Vec<T>;
    fn deref(&self) -> &Vec<T> {
        &self.0
    }
}

impl<T> std::ops::DerefMut for Variadic<T> {
    fn deref_mut(&mut self) -> &mut Vec<T> {
        &mut self.0
    }
}

impl<T> From<Vec<T>> for Variadic<T> {
    fn from(v: Vec<T>) -> Self {
        Variadic(v)
    }
}

/// A value kept in the registry without borrowing the state: for callbacks and objects that
/// Rust structures hold for long. Read it back with [`Lua::registry_value`].
///
/// Dropping a key only queues its slot; it is released at the next entry into the state.
/// A key from another state is rejected.
#[derive(Debug)]
pub struct RegistryKey {
    pub(crate) id: c_int,
    pub(crate) lua_id: u64,
    pub(crate) dead: Rc<RefCell<Vec<c_int>>>,
}

impl Drop for RegistryKey {
    fn drop(&mut self) {
        if self.id != ffi::LUA_NOREF && self.id != ffi::LUA_REFNIL {
            self.dead.borrow_mut().push(self.id);
        }
    }
}

impl Lua {
    /// Stores `value` in the registry.
    pub fn create_registry_value<'lua, T: crate::ToLua<'lua>>(
        &'lua self,
        value: T,
    ) -> Result<RegistryKey> {
        self.expire_registry_values();
        let v = value.to_lua(self)?;
        let l = self.l();
        let _g = StackGuard::new(l);
        // SAFETY: one push, then luaL_ref (raw).
        unsafe { ensure_stack(l, 1) };
        self.push_lua_value(&v);
        Ok(RegistryKey {
            id: self.pop_ref(),
            lua_id: self.inner.id,
            dead: self.inner.dead.clone(),
        })
    }

    /// Reads a value stored by [`Lua::create_registry_value`].
    pub fn registry_value<'lua, T: crate::FromLua<'lua>>(
        &'lua self,
        key: &RegistryKey,
    ) -> Result<T> {
        if key.lua_id != self.inner.id {
            return Err(LuaError::api("registry key belongs to another Lua state"));
        }
        let l = self.l();
        let _g = StackGuard::new(l);
        // SAFETY: one push of a registry slot.
        unsafe { ensure_stack(l, 1) };
        self.push_ref(key.id);
        let v = read_lua_value(self, -1);
        T::from_lua(v, self)
    }

    /// Releases the slot at once (otherwise dropping the key does it later).
    pub fn remove_registry_value(&self, key: RegistryKey) -> Result<()> {
        if key.lua_id != self.inner.id {
            return Err(LuaError::api("registry key belongs to another Lua state"));
        }
        drop(key);
        self.expire_registry_values();
        Ok(())
    }

    /// Stores `value` under a name in the registry (replaces the old one).
    pub fn set_named_registry_value<'lua, T: crate::ToLua<'lua>>(
        &'lua self,
        name: &str,
        value: T,
    ) -> Result<()> {
        let v = value.to_lua(self)?;
        let l = self.l();
        let _g = StackGuard::new(l);
        // SAFETY: pushes; the raw set is protected and needs 2 free slots.
        let status = unsafe {
            ensure_stack(l, 4);
            crate::state::push_str(l, name);
            self.push_lua_value(&v);
            ffi::lua_sys_rawset(l, ffi::LUA_REGISTRYINDEX)
        };
        if status != ffi::LUA_OK {
            return Err(crate::state::pop_error(l, status));
        }
        Ok(())
    }

    /// Reads a value stored by [`Lua::set_named_registry_value`] (`nil` if absent).
    pub fn named_registry_value<'lua, T: crate::FromLua<'lua>>(
        &'lua self,
        name: &str,
    ) -> Result<T> {
        let l = self.l();
        let _g = StackGuard::new(l);
        // SAFETY: raw get on the registry.
        unsafe {
            ensure_stack(l, 2);
            crate::state::push_str(l, name);
            ffi::lua_rawget(l, ffi::LUA_REGISTRYINDEX);
        }
        let v = read_lua_value(self, -1);
        T::from_lua(v, self)
    }
}

impl Lua {
    /// `tostring(value)` with `__tostring`, under pcall.
    pub fn tostring(&self, value: &LuaValue<'_>) -> Result<String> {
        let l = self.l();
        let _g = StackGuard::new(l);
        // SAFETY: lua_sys_tostring is protected; the result is a string on top.
        unsafe {
            ensure_stack(l, 6);
            self.push_lua_value(value);
            let status = ffi::lua_sys_tostring(l, -1);
            if status != ffi::LUA_OK {
                return Err(crate::state::pop_error(l, status));
            }
            if ffi::lua_type(l, -1) != ffi::LUA_TSTRING {
                return Err(LuaError::api("tostring did not return a string"));
            }
            Ok(String::from_utf8_lossy(&string_bytes(l, -1)).into_owned())
        }
    }
}
