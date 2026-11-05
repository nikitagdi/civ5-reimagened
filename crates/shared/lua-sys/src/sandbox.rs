//! The sandbox: no file system, no bytecode, no process control. The standard library tables
//! (`string`, `table`, `math`) stay shared and mutable by scripts.

use crate::convert::Nil;
use crate::error::Result;
use crate::state::Lua;
use crate::value::{LuaString, LuaValue};
use crate::{Function, Table};

/// Globals removed by [`Lua::sandbox`].
const REMOVED_GLOBALS: &[&str] = &[
    "io", "dofile", "loadfile", "require", "module", "package", "newproxy",
];

/// Kept from `os`: time and clocks only.
const KEPT_OS: &[&str] = &["time", "date", "clock", "difftime"];

/// Kept from `debug`: reading only (`traceback`, `getinfo`); no hooks, upvalue or metatable access.
const KEPT_DEBUG: &[&str] = &["traceback", "getinfo"];

const SANDBOX_LUA: &str = include_str!("sandbox.lua");

impl Lua {
    /// Removes file, process and bytecode access from the state, and installs `loadstring` and
    /// `load` that refuse bytecode and run the loaded chunk in the caller's environment.
    ///
    /// Removed: `io`, `dofile`, `loadfile`, `require`, `module`, `package`, `newproxy`,
    /// `string.dump`, and from `os` everything except `time`, `date`, `clock`, `difftime`;
    /// `debug` is cut to `traceback` and `getinfo`.
    pub fn sandbox(&self) -> Result<()> {
        let g = self.globals();
        let keep = |src: Table<'_>, names: &[&str]| -> Result<()> {
            for (k, _) in src
                .pairs::<LuaString, LuaValue>()
                .collect::<Result<Vec<_>>>()?
            {
                if !names.iter().any(|n| k.as_bytes() == n.as_bytes()) {
                    src.raw_set(k, Nil)?;
                }
            }
            Ok(())
        };
        keep(g.raw_get("os")?, KEPT_OS)?;
        keep(g.raw_get("debug")?, KEPT_DEBUG)?;
        let string: Table = g.raw_get("string")?;
        string.raw_set("dump", Nil)?;

        let rawload = self.create_function(|lua, (src, name): (LuaString, Option<LuaString>)| {
            let mut chunk = lua.load(src.as_bytes());
            if let Some(name) = &name {
                chunk = chunk.set_name(&name.to_string_lossy());
            }
            Ok(match chunk.into_function() {
                Ok(f) => (LuaValue::Function(f), LuaValue::Nil),
                Err(e) => (LuaValue::Nil, LuaValue::String(e.message.into())),
            })
        });
        let table: Table = g.raw_get("table")?;
        let args = (
            rawload,
            g.raw_get::<_, Function>("getfenv")?,
            g.raw_get::<_, Function>("setfenv")?,
            g.raw_get::<_, Function>("type")?,
            g.raw_get::<_, Function>("error")?,
            table.raw_get::<_, Function>("concat")?,
        );
        for name in REMOVED_GLOBALS {
            g.raw_set(*name, Nil)?;
        }
        self.load(SANDBOX_LUA)
            .set_name("=sandbox")
            .into_function()?
            .call::<_, ()>(args)
    }
}
