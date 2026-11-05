//! `Path`: the two file-name functions the scripts call (`Path.GetExtension`,
//! `Path.GetFileNameWithoutExtension`), as .NET's `System.IO.Path` answers them.

use std::rc::Rc;

use lua_sys::{Lua, LuaValue, MultiValue, Result, Table};

use crate::Inner;
use crate::registry::Reg;

/// `.lua` for `Assets/UI/X.lua`; empty when the file name has no dot.
pub fn extension(path: &str) -> &str {
    let name_start = path.rfind(['/', '\\']).map_or(0, |i| i + 1);
    match path[name_start..].rfind('.') {
        Some(i) if i + 1 < path[name_start..].len() => &path[name_start + i..],
        _ => "",
    }
}

/// `X` for `Assets/UI/X.lua`.
pub fn file_name_without_extension(path: &str) -> &str {
    let name_start = path.rfind(['/', '\\']).map_or(0, |i| i + 1);
    let name = &path[name_start..];
    match name.rfind('.') {
        Some(i) => &name[..i],
        None => name,
    }
}

fn arg(args: &MultiValue<'_>) -> String {
    match args.get(0) {
        Some(LuaValue::String(s)) => s.to_string_lossy().into_owned(),
        _ => String::new(),
    }
}

pub(crate) fn install<'l>(inner: &Rc<Inner>, lua: &'l Lua, g: &Table<'l>) -> Result<()> {
    let reg = Reg::new(inner);
    let t = lua.create_table();
    reg.real(lua, &t, "Path", "GetExtension", |_, args: MultiValue| {
        Ok(extension(&arg(&args)).to_owned())
    })?;
    reg.real(
        lua,
        &t,
        "Path",
        "GetFileNameWithoutExtension",
        |_, args: MultiValue| Ok(file_name_without_extension(&arg(&args)).to_owned()),
    )?;
    g.raw_set("Path", t)?;
    reg.scope("Path");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_follow_system_io_path() {
        assert_eq!(extension("a/b.c/file.lua"), ".lua");
        assert_eq!(extension("a/b.c/file"), "");
        assert_eq!(extension("file."), "");
        assert_eq!(file_name_without_extension("x\\y\\Map.Civ5Map"), "Map");
        assert_eq!(file_name_without_extension("Map"), "Map");
    }
}
