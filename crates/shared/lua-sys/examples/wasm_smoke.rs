//! Runs the safe API inside wasm: tables, closures, userdata, errors with traceback, sandbox,
//! contexts and include. `tests/wasm_smoke.mjs` loads the module and calls `smoke`.
#![cfg(target_arch = "wasm32")]

use std::rc::Rc;

use lua_sys::{
    IncludePolicy, Lua, LuaError, MemorySource, Runtime, RuntimeConfig, StateMode, UserData,
    UserDataMethods,
};

struct Counter(i32);

impl UserData for Counter {
    fn add_methods<'lua, M: UserDataMethods<'lua, Self>>(m: &mut M) {
        m.add_method_mut("add", |_, this, n: i32| {
            this.0 += n;
            Ok(this.0)
        });
    }
}

fn check(report: &mut String, name: &str, r: Result<(), String>) -> u32 {
    match r {
        Ok(()) => {
            report.push_str(&format!("ok   {name}\n"));
            0
        }
        Err(e) => {
            report.push_str(&format!("FAIL {name}: {e}\n"));
            1
        }
    }
}

fn run(report: &mut String) -> u32 {
    let mut failed = 0;
    failed += check(
        report,
        "tables_closures_userdata",
        (|| {
            let lua = Lua::new().map_err(|e| e.to_string())?;
            let t = lua.create_table();
            t.set("a", 1).map_err(|e| e.to_string())?;
            let add = lua.create_function(|_, (a, b): (i32, i32)| Ok(a + b));
            lua.globals().set("add", add).map_err(|e| e.to_string())?;
            lua.globals()
                .set(
                    "c",
                    lua.create_userdata(Counter(1)).map_err(|e| e.to_string())?,
                )
                .map_err(|e| e.to_string())?;
            let n: i32 = lua
                .load("return add(2, 3) + c:add(10)")
                .eval()
                .map_err(|e| e.to_string())?;
            if n != 16 {
                return Err(format!("got {n}"));
            }
            Ok(())
        })(),
    );
    failed += check(
        report,
        "errors_with_traceback",
        (|| {
            let lua = Lua::new().map_err(|e| e.to_string())?;
            let f = lua.create_function(|_, ()| -> Result<(), LuaError> {
                Err(LuaError::msg("from rust"))
            });
            lua.globals()
                .set("rust_fail", f)
                .map_err(|e| e.to_string())?;
            let e = lua
                .load("\nrust_fail()")
                .set_name("@X.lua")
                .exec()
                .unwrap_err();
            if e.message != "X.lua:2: from rust" || e.traceback.is_none() {
                return Err(format!("{e:#}"));
            }
            let e = lua
                .load("error('deep')")
                .set_name("@Y.lua")
                .exec()
                .unwrap_err();
            if e.location() != Some(("Y.lua", 1)) {
                return Err(format!("{e:#}"));
            }
            Ok(())
        })(),
    );
    failed += check(
        report,
        "gc_error_contained",
        (|| {
            let lua = Lua::new().map_err(|e| e.to_string())?;
            lua.exec(
            "collectgarbage('setpause', 0) collectgarbage('setstepmul', 0)
             do local p = newproxy(true); getmetatable(p).__gc = function() error('gc boom') end end",
            "arm",
        )
        .map_err(|e| e.to_string())?;
            for i in 0..100 {
                let t = lua.create_table();
                let _ = t.set("k", format!("v{i}"));
                let _ = lua.load("return 1").exec();
            }
            lua.exec("x = 1", "after").map_err(|e| e.to_string())
        })(),
    );
    failed += check(
        report,
        "sandbox",
        (|| {
            let lua = Lua::new().map_err(|e| e.to_string())?;
            lua.sandbox().map_err(|e| e.to_string())?;
            lua.load("assert(io == nil and os.execute == nil and loadstring('return 4')() == 4)")
                .exec()
                .map_err(|e| e.to_string())
        })(),
    );
    for mode in [StateMode::Shared, StateMode::PerContext] {
        failed += check(
            report,
            &format!("contexts_{mode:?}"),
            (|| {
                let mut files = MemorySource::new();
                files.add("UI/Lib.lua", "lib_loaded = true");
                let cfg = RuntimeConfig {
                    mode,
                    include_policy: IncludePolicy::OncePerContext,
                    ..RuntimeConfig::default()
                };
                let rt =
                    Runtime::new(cfg, Rc::new(files), |_, _| Ok(())).map_err(|e| e.to_string())?;
                let a = rt
                    .create_context("A", |_, _| Ok(()))
                    .map_err(|e| e.to_string())?;
                let b = rt
                    .create_context("B", |_, _| Ok(()))
                    .map_err(|e| e.to_string())?;
                a.exec("include('Lib'); assert(lib_loaded)", "@a")
                    .map_err(|e| e.to_string())?;
                if b.exec("error('only B fails')", "@b").is_ok() {
                    return Err("B should fail".into());
                }
                b.exec("assert(lib_loaded == nil)", "@b")
                    .map_err(|e| e.to_string())?;
                if rt.error_count() != 1 {
                    return Err(format!("error_count {}", rt.error_count()));
                }
                Ok(())
            })(),
        );
    }
    failed
}

static REPORT: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// Runs the checks; returns the number that failed. The text is read with `report_ptr`/`report_len`.
#[unsafe(no_mangle)]
pub extern "C" fn smoke() -> u32 {
    let mut report = String::new();
    let failed = run(&mut report);
    *REPORT.lock().expect("single thread") = report;
    failed
}

#[unsafe(no_mangle)]
pub extern "C" fn report_ptr() -> *const u8 {
    REPORT.lock().expect("single thread").as_ptr()
}

#[unsafe(no_mangle)]
pub extern "C" fn report_len() -> usize {
    REPORT.lock().expect("single thread").len()
}
