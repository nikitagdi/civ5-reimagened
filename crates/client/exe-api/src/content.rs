//! `ContentManager` and `ContentType`: which content packages are installed and active.
//!
//! The EUI asks `ContentManager.IsActive(<guid>, ContentType.GAMEPLAY)` for Gods & Kings and Brave New
//! World (`gk_mode`, `bnw_mode`), lists `GetAllPackageIDs()` and prints `GetPackageDescription(id)`.
//! The GUIDs are the `<GUID>` of `Expansion1.Civ5Pkg` and `Expansion2.Civ5Pkg` of the install (the `ui`
//! set's index names packages by file stem only, so they are constants here; a test reads them back from
//! the install when `CIV5_GAME_DIR` is set).
//!
//! SIMPLIFIED(SIMP-exe-15): only these two packages are known, and the values of `ContentType` are ours.

use std::rc::Rc;

use lua_sys::{Lua, LuaString, LuaValue, MultiValue, Result, Table};

use crate::Inner;
use crate::registry::Reg;

/// `<GUID>` of Expansion - Gods and Kings (`DLC/Expansion/Expansion1.Civ5Pkg`).
pub const GUID_GK: &str = "0E3751A1-F840-4e1b-9706-519BF484E59D";
/// `<GUID>` of Expansion 2 - Brave New World (`DLC/Expansion2/Expansion2.Civ5Pkg`).
pub const GUID_BNW: &str = "6DA07636-4123-4018-B643-6575B4EC336B";

/// The kinds of content `ContentManager.IsActive` is asked about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentType {
    Gameplay = 1,
    Map = 2,
    Scenario = 3,
}

impl ContentType {
    pub const NAMES: &[(&str, ContentType)] = &[
        ("GAMEPLAY", ContentType::Gameplay),
        ("MAP", ContentType::Map),
        ("SCENARIO", ContentType::Scenario),
    ];
}

/// One installed package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentPackage {
    pub guid: String,
    /// What `GetPackageDescription` answers: a text key or a plain text.
    pub description: String,
    /// Which kinds of content of the package are on.
    pub gameplay: bool,
}

/// Gods & Kings and Brave New World, both active.
pub fn default_packages() -> Vec<ContentPackage> {
    vec![
        ContentPackage {
            guid: GUID_GK.to_owned(),
            description: "Expansion - Gods and Kings".to_owned(),
            gameplay: true,
        },
        ContentPackage {
            guid: GUID_BNW.to_owned(),
            description: "Expansion 2 - Brave New World".to_owned(),
            gameplay: true,
        },
    ]
}

/// GUIDs compare without braces and without case.
fn norm(s: &str) -> String {
    s.trim().trim_matches(['{', '}']).to_ascii_lowercase()
}

fn arg_text(v: Option<&LuaValue<'_>>) -> String {
    match v {
        Some(LuaValue::String(s)) => s.to_string_lossy().into_owned(),
        _ => String::new(),
    }
}

pub(crate) fn install<'l>(inner: &Rc<Inner>, lua: &'l Lua, g: &Table<'l>) -> Result<()> {
    let reg = Reg::new(inner);

    let types = lua.create_table();
    for (name, ty) in ContentType::NAMES {
        reg.value(&types, "ContentType", name, *ty as i32)?;
    }
    g.raw_set("ContentType", types)?;
    reg.scope("ContentType");

    let t = lua.create_table();
    let i = Rc::clone(inner);
    reg.real(
        lua,
        &t,
        "ContentManager",
        "IsActive",
        move |_, args: MultiValue| {
            let guid = norm(&arg_text(args.get(0)));
            let ty = match args.get(1) {
                Some(LuaValue::Number(n)) => *n as i32,
                _ => ContentType::Gameplay as i32,
            };
            Ok(i.cfg.packages.iter().any(|p| {
                norm(&p.guid) == guid && (ty != ContentType::Gameplay as i32 || p.gameplay)
            }))
        },
    )?;
    let i = Rc::clone(inner);
    reg.real(
        lua,
        &t,
        "ContentManager",
        "GetAllPackageIDs",
        move |lua, _: MultiValue| {
            let list = lua.create_table();
            for p in &i.cfg.packages {
                list.push(LuaValue::String(LuaString(p.guid.clone().into_bytes())))?;
            }
            Ok(list)
        },
    )?;
    let i = Rc::clone(inner);
    reg.real(
        lua,
        &t,
        "ContentManager",
        "GetPackageDescription",
        move |_, args: MultiValue| {
            let guid = norm(&arg_text(args.get(0)));
            Ok(i.cfg
                .packages
                .iter()
                .find(|p| norm(&p.guid) == guid)
                .map(|p| p.description.clone())
                .unwrap_or_default())
        },
    )?;
    g.raw_set("ContentManager", t)?;
    reg.scope("ContentManager");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guids_compare_without_braces_and_case() {
        assert_eq!(
            norm("{6DA07636-4123-4018-B643-6575B4EC336B}"),
            norm("6da07636-4123-4018-b643-6575b4ec336b")
        );
    }

    /// The constants are the `<GUID>` of the packages of the install (skipped without `CIV5_GAME_DIR`).
    #[test]
    fn guids_match_the_packages_of_the_install() {
        let Some(dir) = std::env::var_os("CIV5_GAME_DIR") else {
            eprintln!("SKIPPED: CIV5_GAME_DIR is not set");
            return;
        };
        let dlc = std::path::Path::new(&dir).join("Assets/DLC");
        for (pkg, want) in [
            ("Expansion/Expansion1.Civ5Pkg", GUID_GK),
            ("Expansion2/Expansion2.Civ5Pkg", GUID_BNW),
        ] {
            let Ok(text) = std::fs::read_to_string(dlc.join(pkg)) else {
                eprintln!("SKIPPED: {pkg} is not in the install");
                continue;
            };
            let found = text
                .split("<GUID>")
                .nth(1)
                .and_then(|s| s.split("</GUID>").next())
                .map(norm);
            assert_eq!(found, Some(norm(want)), "{pkg}");
        }
    }
}
