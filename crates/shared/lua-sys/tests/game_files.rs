//! Acceptance on the real game files: libraries of the original UI and of EUI run through
//! `include`. Skipped, with a message, when the game files are absent (set `CIV5_GAME_DIR` to the
//! game's install folder, the one with `Assets`).
#![cfg(not(target_arch = "wasm32"))]

use std::path::PathBuf;
use std::rc::Rc;

use lua_sys::{DirSource, IncludeSource, Lua, Runtime, RuntimeConfig, StateMode};

fn assets() -> Option<PathBuf> {
    let candidates = [std::env::var_os("CIV5_GAME_DIR").map(PathBuf::from)];
    for dir in candidates.into_iter().flatten() {
        let assets = dir.join("Assets");
        if assets.join("UI/InstanceManager.lua").is_file() {
            return Some(assets);
        }
    }
    eprintln!("SKIPPED: game files not found (set CIV5_GAME_DIR)");
    None
}

/// Base UI, BNW, Lekmod, then EUI on top, as the package priorities order them.
fn source(assets: &std::path::Path) -> Option<Rc<DirSource>> {
    let layers: Vec<PathBuf> = [
        "UI",
        "Gameplay/Lua",
        "DLC/Expansion2/UI",
        "DLC/LEKMOD_v34.15/Lua/UI",
        "DLC/UI_bc1",
    ]
    .iter()
    .map(|l| assets.join(l))
    .filter(|p| p.is_dir())
    .collect();
    DirSource::new(&layers).ok().map(Rc::new)
}

fn runtime(files: Rc<DirSource>, setup: &'static str) -> Runtime {
    Runtime::new(RuntimeConfig::default(), files, move |lua, g| {
        lua.load(setup)
            .set_name("=setup")
            .set_environment(g.clone())
            .exec()
    })
    .unwrap()
}

#[test]
fn every_lua_file_of_the_game_parses() {
    let Some(assets) = assets() else { return };
    let files = source(&assets).unwrap();
    let lua = Lua::new().unwrap();
    let list = files.list();
    let mut failed = Vec::new();
    for path in &list {
        let bytes = files.read(path).unwrap();
        if let Err(e) = lua
            .load(&bytes)
            .set_name(&format!("@{path}"))
            .into_function()
        {
            failed.push(format!("{path}: {}", e.message));
        }
    }
    eprintln!("parsed {} files, {} failed", list.len(), failed.len());
    assert!(list.len() > 300, "only {} files found", list.len());
    assert!(
        failed.is_empty(),
        "{} files failed to parse:\n{}",
        failed.len(),
        failed.join("\n")
    );
}

#[test]
fn instance_manager_runs_through_include() {
    let Some(assets) = assets() else { return };
    let rt = runtime(source(&assets).unwrap(), "");
    let ctx = rt
        .create_context("InstanceManagerTest", |lua, env| {
            // A ContextPtr that records what the manager builds.
            lua.load(
                r#"
                built = {}
                ContextPtr = {
                  BuildInstanceForControl = function(self, name, instance, parent)
                    built[#built + 1] = name
                    instance.Root = { name = name, parent = parent,
                      ChangeParent = function(self, p) self.parent = p end,
                      SetHide = function(self, h) self.hidden = h end }
                  end,
                }"#,
            )
            .set_environment(env.clone())
            .exec()
        })
        .unwrap();
    ctx.include("InstanceManager").unwrap();
    ctx.exec(
        r#"
        local parent = { name = "stack" }
        local mgr = InstanceManager:new("ItemInstance", "Root", parent)
        local a = mgr:GetInstance()
        local b = mgr:GetInstance()
        assert(a ~= b and a.Root.parent == parent)
        assert(#built == 2 and built[1] == "ItemInstance")
        mgr:ResetInstances()
        local c = mgr:GetInstance()
        assert(c == a or c == b, "instances are reused")
        assert(#built == 2)
        "#,
        "@test",
    )
    .unwrap();
    assert_eq!(rt.error_count(), 0);
}

#[test]
fn eui_utilities_and_core_library_include() {
    let Some(assets) = assets() else { return };
    if !assets.join("DLC/UI_bc1/Core/EUI_utilities.lua").is_file() {
        eprintln!("SKIPPED: EUI (DLC/UI_bc1) not unpacked");
        return;
    }
    // Exe-API stubs: the EUI core only needs these to exist at load time.
    let rt = runtime(
        source(&assets).unwrap(),
        r#"
        MapModData = {}
        Mouse = { eLClick = 1 }
        InStrategicView = function() end
        ContentManager = { IsActive = function() return true end }
        ContentType = { GAMEPLAY = 1 }
        DB = { Query = function() return function() end end }
        Locale = { ConvertTextKey = function(k) return k end }
        -- Any other exe or gamecore global is a stub that accepts every use.
        local Stub = {}
        local ops = function() return Stub end
        setmetatable(Stub, { __index = function() return Stub end, __call = ops, __add = ops,
          __sub = ops, __mul = ops, __div = ops, __concat = function(a, b) return "" end,
          __tostring = function() return "stub" end, __lt = function() return false end,
          __le = function() return false end, __len = function() return 0 end })
        setmetatable(_G, { __index = function(_, k) return Stub end })
        "#,
    );
    let ctx = rt
        .create_context("EUI_utilities", |lua, env| {
            lua.load(
                r#"
                local calls = {}
                ContextPtr = {
                  LookUpControl = function() return { GetID = function() return "NotInGame" end } end,
                  BuildInstanceForControl = function(self, name, inst, parent)
                    calls[#calls + 1] = name
                    inst.Item = { SetHide = function() end }
                  end,
                }
                builds = calls"#,
            )
            .set_environment(env.clone())
            .exec()
        })
        .unwrap();
    // Not the InGame context, so EUI_utilities includes EUI_core_library (found by name in
    // UI_bc1/Core) and runs it here with the stubs above.
    ctx.include("EUI_utilities").unwrap();
    ctx.exec(
        r#"
        assert(EUI ~= nil and EUI == MapModData.UI_bc1 or EUI ~= nil)
        local parent = { SetHide = function() end, CalculateSize = function() end }
        local sim = StackInstanceManager("Row", "Item", parent)
        local inst = sim.GetInstance()
        assert(inst.Item ~= nil and #builds == 1)
        sim.Commit()"#,
        "@test",
    )
    .unwrap();
    assert_eq!(rt.error_count(), 0);
}

#[test]
fn eui_utilities_with_shared_eui_table() {
    let Some(assets) = assets() else { return };
    if !assets.join("DLC/UI_bc1/Core/EUI_utilities.lua").is_file() {
        return;
    }
    // MapModData.UI_bc1 present: EUI_utilities takes it and defines the instance managers.
    let rt = runtime(
        source(&assets).unwrap(),
        "MapModData = { UI_bc1 = { marker = true } }\nMouse = { eLClick = 1 }",
    );
    let ctx = rt
        .create_context("ctx", |lua, env| {
            lua.load(
                r#"
                built = {}
                ContextPtr = { BuildInstanceForControl = function(self, name, inst, parent)
                  built[#built + 1] = name
                  inst.Item = { SetHide = function() end }
                end }"#,
            )
            .set_environment(env.clone())
            .exec()
        })
        .unwrap();
    ctx.include("EUI_utilities").unwrap();
    ctx.exec(
        r#"
        assert(EUI.marker == true)
        local parent = { SetHide = function() end }
        local sim = StackInstanceManager("Row", "Item", parent)
        sim.GetInstance(); sim.GetInstance()
        assert(#built == 2 and sim.Count() == 2)
        sim.ResetInstances(); sim.GetInstance()
        assert(#built == 2 and sim.Count() == 1)
        sim.Commit()
        "#,
        "@test",
    )
    .unwrap();
    // Included again in another context of the same state: runs again there (own globals).
    let other = rt.create_context("other", |_, _| Ok(())).unwrap();
    other.exec("ContextPtr = {}", "@o").unwrap();
    other.include("EUI_utilities").unwrap();
    other
        .exec(
            "assert(EUI.marker == true and StackInstanceManager ~= nil)",
            "@o",
        )
        .unwrap();
    let _ = StateMode::Shared;
}
