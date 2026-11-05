//! What `base_setup` put into the Lua state, name by name, and the helpers that put it there.

use std::collections::BTreeMap;
use std::rc::Rc;

use lua_sys::{FromLuaMulti, Lua, LuaValue, MultiValue, Result, Table, ToLua, ToLuaMulti};

use crate::Inner;

/// How a registered name is implemented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// A real implementation.
    Real,
    /// A stub that answers a fixed value; the id names its row in [`crate::simp::SIMP_ROWS`].
    Simp(&'static str),
}

/// Names (`OptionsManager.GetGridOn`, `Mouse.eLClick`, `InStrategicView`) with their status.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Registry {
    items: BTreeMap<String, Status>,
}

impl Registry {
    pub fn add(&mut self, name: impl Into<String>, status: Status) {
        self.items.insert(name.into(), status);
    }

    pub fn get(&self, name: &str) -> Option<Status> {
        self.items.get(name).copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, Status)> {
        self.items.iter().map(|(k, v)| (k.as_str(), *v))
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// The answer of a SIMP stub.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Stub {
    Nil,
    Bool(bool),
    Num(f64),
    /// A new empty table.
    Table,
}

pub(crate) struct Reg<'a> {
    pub inner: &'a Rc<Inner>,
}

impl<'a> Reg<'a> {
    pub fn new(inner: &'a Rc<Inner>) -> Reg<'a> {
        Reg { inner }
    }

    fn note(&self, scope: &str, name: &str, status: Status) {
        let path = if scope.is_empty() {
            name.to_owned()
        } else {
            format!("{scope}.{name}")
        };
        self.inner.registry.borrow_mut().add(path, status);
    }

    /// A function `scope.name` with a real implementation.
    pub fn real<'l, A, R, F>(
        &self,
        lua: &'l Lua,
        table: &Table<'l>,
        scope: &str,
        name: &str,
        f: F,
    ) -> Result<()>
    where
        A: FromLuaMulti<'l>,
        R: ToLuaMulti<'l>,
        F: Fn(&'l Lua, A) -> Result<R> + 'static,
    {
        table.raw_set(name, lua.create_function(f))?;
        self.note(scope, name, Status::Real);
        Ok(())
    }

    /// A constant or object `scope.name`.
    pub fn value<'l, V: ToLua<'l>>(
        &self,
        table: &Table<'l>,
        scope: &str,
        name: &str,
        v: V,
    ) -> Result<()> {
        table.raw_set(name, v)?;
        self.note(scope, name, Status::Real);
        Ok(())
    }

    /// A stub function `scope.name` (SIMP `id`): counts its calls and answers `ret`.
    pub fn stub<'l>(
        &self,
        lua: &'l Lua,
        table: &Table<'l>,
        scope: &str,
        name: &str,
        id: &'static str,
        ret: Stub,
    ) -> Result<()> {
        let inner = Rc::clone(self.inner);
        table.raw_set(
            name,
            lua.create_function(move |lua, _args: MultiValue<'l>| {
                inner.simp_calls.set(inner.simp_calls.get() + 1);
                push_stub(lua, ret)
            }),
        )?;
        self.note(scope, name, Status::Simp(id));
        Ok(())
    }

    /// A global function (`InStrategicView`) rather than a member of a table.
    pub fn global_real<'l, A, R, F>(
        &self,
        lua: &'l Lua,
        g: &Table<'l>,
        name: &str,
        f: F,
    ) -> Result<()>
    where
        A: FromLuaMulti<'l>,
        R: ToLuaMulti<'l>,
        F: Fn(&'l Lua, A) -> Result<R> + 'static,
    {
        self.real(lua, g, "", name, f)
    }

    /// Notes that a table was created, so the report knows the scope exists.
    pub fn scope(&self, name: &str) {
        self.note("", name, Status::Real);
    }
}

fn push_stub<'l>(lua: &'l Lua, ret: Stub) -> Result<LuaValue<'l>> {
    Ok(match ret {
        Stub::Nil => LuaValue::Nil,
        Stub::Bool(b) => LuaValue::Boolean(b),
        Stub::Num(n) => LuaValue::Number(n),
        Stub::Table => LuaValue::Table(lua.create_table()),
    })
}
