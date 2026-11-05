//! Tables: creation, access (with and without metamethods), iteration, metatables.

use std::ffi::c_int;
use std::marker::PhantomData;

use crate::convert::{FromLua, FromLuaMulti, ToLua, ToLuaMulti};
use crate::error::{LuaError, Result};
use crate::ffi;
use crate::state::{Lua, StackGuard, ensure_stack, pop_error};
use crate::value::{LuaRef, LuaValue, MultiValue, read_lua_value};
use crate::{Function, Table};

impl Lua {
    /// The globals of the running thread (`_G`; the main state's unless a script swapped it).
    pub fn globals(&self) -> Table<'_> {
        let l = self.l();
        // SAFETY: one push of the globals pseudo-index.
        unsafe {
            ensure_stack(l, 1);
            ffi::lua_pushvalue(l, ffi::LUA_GLOBALSINDEX);
        }
        Table(LuaRef::from_top(self))
    }

    pub fn create_table(&self) -> Table<'_> {
        self.create_table_with_capacity(0, 0)
    }

    pub fn create_table_with_capacity(&self, narr: usize, nrec: usize) -> Table<'_> {
        self.expire_registry_values();
        let l = self.l();
        // SAFETY: GC-paused creation; one push.
        unsafe {
            ensure_stack(l, 1);
            ffi::lua_sys_createtable(
                l,
                c_int::try_from(narr).unwrap_or(c_int::MAX),
                c_int::try_from(nrec).unwrap_or(c_int::MAX),
            );
        }
        Table(LuaRef::from_top(self))
    }

    /// A table `{v1, v2, ...}`.
    pub fn create_sequence_from<'lua, T, I>(&'lua self, iter: I) -> Result<Table<'lua>>
    where
        T: ToLua<'lua>,
        I: IntoIterator<Item = T>,
    {
        let t = self.create_table();
        for (i, v) in iter.into_iter().enumerate() {
            t.raw_set(i + 1, v)?;
        }
        Ok(t)
    }

    /// A table `{[k1] = v1, ...}`.
    pub fn create_table_from<'lua, K, V, I>(&'lua self, iter: I) -> Result<Table<'lua>>
    where
        K: ToLua<'lua>,
        V: ToLua<'lua>,
        I: IntoIterator<Item = (K, V)>,
    {
        let t = self.create_table();
        for (k, v) in iter {
            t.raw_set(k, v)?;
        }
        Ok(t)
    }
}

impl<'lua> Table<'lua> {
    fn lua(&self) -> &'lua Lua {
        self.0.lua
    }

    /// `t[key]`, honoring `__index`. A metamethod error comes back as `Err` with its traceback.
    pub fn get<K: ToLua<'lua>, V: FromLua<'lua>>(&self, key: K) -> Result<V> {
        let lua = self.lua();
        let key = key.to_lua(lua)?;
        let l = lua.l();
        let _g = StackGuard::new(l);
        // SAFETY: lua_sys_gettable is protected; 6 slots cover the helper's needs.
        unsafe {
            ensure_stack(l, 8);
            self.0.push();
            lua.push_lua_value(&key);
            let status = ffi::lua_sys_gettable(l, -2);
            if status != ffi::LUA_OK {
                return Err(pop_error(l, status));
            }
        }
        V::from_lua(read_lua_value(lua, -1), lua)
    }

    /// `t[key] = value`, honoring `__newindex`.
    pub fn set<K: ToLua<'lua>, V: ToLua<'lua>>(&self, key: K, value: V) -> Result<()> {
        let lua = self.lua();
        let key = key.to_lua(lua)?;
        let value = value.to_lua(lua)?;
        let l = lua.l();
        let _g = StackGuard::new(l);
        // SAFETY: lua_sys_settable is protected.
        unsafe {
            ensure_stack(l, 10);
            self.0.push();
            let t = ffi::lua_gettop(l);
            lua.push_lua_value(&key);
            lua.push_lua_value(&value);
            let status = ffi::lua_sys_settable(l, t);
            if status != ffi::LUA_OK {
                return Err(pop_error(l, status));
            }
        }
        Ok(())
    }

    /// `rawget(t, key)`.
    pub fn raw_get<K: ToLua<'lua>, V: FromLua<'lua>>(&self, key: K) -> Result<V> {
        let lua = self.lua();
        let key = key.to_lua(lua)?;
        let l = lua.l();
        let _g = StackGuard::new(l);
        // SAFETY: rawget calls no metamethods and raises nothing.
        unsafe {
            ensure_stack(l, 3);
            self.0.push();
            lua.push_lua_value(&key);
            ffi::lua_rawget(l, -2);
        }
        V::from_lua(read_lua_value(lua, -1), lua)
    }

    /// `rawset(t, key, value)`. Fails for a `nil` or NaN key.
    pub fn raw_set<K: ToLua<'lua>, V: ToLua<'lua>>(&self, key: K, value: V) -> Result<()> {
        let lua = self.lua();
        let key = key.to_lua(lua)?;
        let value = value.to_lua(lua)?;
        let l = lua.l();
        let _g = StackGuard::new(l);
        // SAFETY: lua_sys_rawset is protected; it needs 2 free slots besides k, v.
        unsafe {
            ensure_stack(l, 6);
            self.0.push();
            let t = ffi::lua_gettop(l);
            lua.push_lua_value(&key);
            lua.push_lua_value(&value);
            let status = ffi::lua_sys_rawset(l, t);
            if status != ffi::LUA_OK {
                return Err(pop_error(l, status));
            }
        }
        Ok(())
    }

    /// True when `rawget(t, key)` is not `nil`.
    pub fn contains_key<K: ToLua<'lua>>(&self, key: K) -> Result<bool> {
        Ok(!self.raw_get::<_, LuaValue<'lua>>(key)?.is_nil())
    }

    /// Length of the sequence part (`#t` without `__len`, which 5.1 never calls for tables).
    pub fn raw_len(&self) -> usize {
        let l = self.lua().l();
        let _g = StackGuard::new(l);
        // SAFETY: objlen on a table is raw.
        unsafe {
            ensure_stack(l, 1);
            self.0.push();
            ffi::lua_objlen(l, -1)
        }
    }

    /// Appends at `#t + 1`.
    pub fn push<V: ToLua<'lua>>(&self, value: V) -> Result<()> {
        self.raw_set(self.raw_len() + 1, value)
    }

    /// The values `t[1] .. t[#t]` (raw).
    pub fn sequence_values<V: FromLua<'lua>>(&self) -> Result<Vec<V>> {
        let lua = self.lua();
        let n = self.raw_len();
        let mut out = Vec::with_capacity(n);
        for i in 1..=n {
            out.push(V::from_lua(self.raw_get(i)?, lua)?);
        }
        Ok(out)
    }

    /// All raw `(key, value)` pairs, taken as a snapshot first: the table may be changed while
    /// the iterator runs (and by scripts called from the loop body).
    pub fn pairs<K: FromLua<'lua>, V: FromLua<'lua>>(&self) -> TablePairs<'lua, K, V> {
        let lua = self.lua();
        let l = lua.l();
        let mut pairs = Vec::new();
        {
            let _g = StackGuard::new(l);
            // SAFETY: lua_next raises only on a key that is not in the table; the key is the one
            // it returned and nothing runs Lua code (reads create registry slots only) between
            // calls. Keys are read without conversion.
            unsafe {
                ensure_stack(l, 6);
                self.0.push();
                ffi::lua_pushnil(l);
                while ffi::lua_next(l, -2) != 0 {
                    ensure_stack(l, 3);
                    let k = read_lua_value(lua, -2);
                    let v = read_lua_value(lua, -1);
                    pairs.push((k, v));
                    ffi::lua_settop(l, -2);
                }
            }
        }
        TablePairs {
            lua,
            iter: pairs.into_iter(),
            _kv: PhantomData,
        }
    }

    /// The metatable, if any.
    pub fn metatable(&self) -> Option<Table<'lua>> {
        let lua = self.lua();
        let l = lua.l();
        let _g = StackGuard::new(l);
        // SAFETY: getmetatable pushes the table only when it exists.
        unsafe {
            ensure_stack(l, 2);
            self.0.push();
            if ffi::lua_getmetatable(l, -1) == 0 {
                return None;
            }
            Some(Table(LuaRef::from_top(lua)))
        }
    }

    /// Sets or clears the metatable.
    pub fn set_metatable(&self, meta: Option<Table<'lua>>) {
        let l = self.lua().l();
        let _g = StackGuard::new(l);
        // SAFETY: setmetatable pops the metatable and raises nothing.
        unsafe {
            ensure_stack(l, 3);
            self.0.push();
            match &meta {
                Some(m) => m.0.push(),
                None => ffi::lua_pushnil(l),
            }
            ffi::lua_setmetatable(l, -2);
        }
    }

    /// `t:name(args...)`: looks the method up with `__index`, calls it with `t` first.
    pub fn call_method<A, R>(&self, name: &str, args: A) -> Result<R>
    where
        A: ToLuaMulti<'lua>,
        R: FromLuaMulti<'lua>,
    {
        let lua = self.lua();
        let f: Function<'lua> = match self.get::<_, LuaValue<'lua>>(name)? {
            LuaValue::Function(f) => f,
            other => {
                return Err(LuaError::api(format!(
                    "method '{name}' is a {}, not a function",
                    other.type_name()
                )));
            }
        };
        let mut all = vec![LuaValue::Table(self.clone())];
        all.extend(args.to_lua_multi(lua)?.0);
        let res = f.call_multi(MultiValue(all))?;
        R::from_lua_multi(res, lua)
    }
}

/// Snapshot iterator over a table's pairs, see [`Table::pairs`].
pub struct TablePairs<'lua, K, V> {
    lua: &'lua Lua,
    iter: std::vec::IntoIter<(LuaValue<'lua>, LuaValue<'lua>)>,
    _kv: PhantomData<(K, V)>,
}

impl<'lua, K: FromLua<'lua>, V: FromLua<'lua>> Iterator for TablePairs<'lua, K, V> {
    type Item = Result<(K, V)>;

    fn next(&mut self) -> Option<Self::Item> {
        let (k, v) = self.iter.next()?;
        Some(K::from_lua(k, self.lua).and_then(|k| Ok((k, V::from_lua(v, self.lua)?))))
    }
}
