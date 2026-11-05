//! Context runtime: environments, include, isolation (AC-A4). Native only.

use std::rc::Rc;

use lua_sys::{
    Context, IncludePolicy, IncludeSource, LuaError, LuaValue, MemorySource, Runtime,
    RuntimeConfig, StateMode, Table,
};

fn files() -> Rc<MemorySource> {
    let mut m = MemorySource::new();
    m.add(
        "UI/InstanceManager.lua",
        "InstanceManager = { tag = 'base' }\nloads = (loads or 0) + 1",
    );
    // A higher layer replaces the base file with the same path.
    m.add("UI/Util/Helper.lua", "helper_version = 1");
    m.add(
        "DLC/UI_bc1/Core/EUI_core.lua",
        "EUI = {}\nMapModData.EUI = EUI\nloads = (loads or 0) + 1",
    );
    m.add(
        "UI/InGame/PopupsGeneric/APopup.lua",
        "popups = (popups or '') .. 'A'",
    );
    m.add(
        "UI/InGame/PopupsGeneric/BPopup.lua",
        "popups = (popups or '') .. 'B'",
    );
    m.add(
        "UI/InGame/PopupsGeneric/Readme.lua",
        "popups = (popups or '') .. '!'",
    );
    m.add(
        "UI/Cycle1.lua",
        "cycle = (cycle or '') .. '1'; include('Cycle2')",
    );
    m.add(
        "UI/Cycle2.lua",
        "cycle = (cycle or '') .. '2'; include('Cycle1')",
    );
    m.add("UI/Broken.lua", "local x = 1\nlocal y = = 2");
    m.add("UI/Throws.lua", "local a = 1\nerror('file failed')");
    m.add("UI/Bom.lua", b"\xEF\xBB\xBFbom_ok = true".to_vec());
    Rc::new(m)
}

fn rt(mode: StateMode, policy: IncludePolicy) -> Runtime {
    let cfg = RuntimeConfig {
        mode,
        include_policy: policy,
        ..RuntimeConfig::default()
    };
    Runtime::new(cfg, files(), |lua, g| {
        g.set("MapModData", lua.create_table())?;
        g.set("ExeApi", lua.create_function(|_, x: i32| Ok(x + 1)))?;
        Ok(())
    })
    .unwrap()
}

fn ctx(rt: &Runtime, name: &str) -> Context {
    rt.create_context(name, |lua, env| {
        let n = name.to_owned();
        env.set("ContextName", n)?;
        env.set("Controls", lua.create_table())?;
        Ok(())
    })
    .unwrap()
}

const BOTH: [StateMode; 2] = [StateMode::Shared, StateMode::PerContext];

#[test]
fn globals_are_per_context_and_base_is_visible() {
    for mode in BOTH {
        let rt = rt(mode, IncludePolicy::OncePerContext);
        let a = ctx(&rt, "A");
        let b = ctx(&rt, "B");
        a.exec(
            "counter = 1; function bump() counter = counter + 1; return counter end",
            "@a",
        )
        .unwrap();
        b.exec("counter = 100", "@b").unwrap();
        assert_eq!(a.call_global::<_, i32>("bump", ()).unwrap(), 2);
        assert_eq!(b.env().get::<_, i32>("counter").unwrap(), 100);
        assert_eq!(a.env().get::<_, i32>("counter").unwrap(), 2);
        // Base functions are reachable; each context sees its own name; _G is the context.
        a.exec(
            "assert(ExeApi(1) == 2 and ContextName == 'A' and _G.counter == 2)",
            "@a",
        )
        .unwrap();
        b.exec(
            "assert(ContextName == 'B' and _G.counter == 100 and a_only == nil)",
            "@b",
        )
        .unwrap();
        // A function defined in A keeps A's globals when called from the host later.
        let key = a
            .lua()
            .create_registry_value(a.env().get::<_, LuaValue>("bump").unwrap())
            .unwrap();
        assert_eq!(a.call_key::<_, i32>(&key, ()).unwrap(), 3);
    }
}

#[test]
fn shared_mode_shares_base_tables_per_context_mode_does_not() {
    let shared = rt(StateMode::Shared, IncludePolicy::OncePerContext);
    let (a, b) = (ctx(&shared, "A"), ctx(&shared, "B"));
    a.exec("MapModData.UI_bc1 = {value = 7}", "@a").unwrap();
    b.exec("assert(MapModData.UI_bc1.value == 7)", "@b")
        .unwrap();
    assert_eq!(a.lua().as_ptr(), b.lua().as_ptr());

    let per = rt(StateMode::PerContext, IncludePolicy::OncePerContext);
    let (a, b) = (ctx(&per, "A"), ctx(&per, "B"));
    a.exec("MapModData.UI_bc1 = {value = 7}", "@a").unwrap();
    b.exec("assert(MapModData.UI_bc1 == nil)", "@b").unwrap();
    assert_ne!(a.lua().as_ptr(), b.lua().as_ptr());
}

#[test]
fn errors_are_isolated_counted_and_logged() {
    for mode in BOTH {
        let rt = rt(mode, IncludePolicy::OncePerContext);
        let good = ctx(&rt, "Good");
        let bad = ctx(&rt, "Bad");
        good.exec("function Ping() return 'pong' end", "@Good.lua")
            .unwrap();
        let e = bad
            .exec("local t = nil\nreturn t.field", "@Bad.lua")
            .unwrap_err();
        assert_eq!(e.location(), Some(("Bad.lua", 2)));
        assert!(e.traceback.is_some());
        let e2 = bad.call_global::<_, ()>("missing", ()).unwrap_err();
        assert!(e2.message.contains("missing"), "{e2}");
        // The other context and the state still work.
        assert_eq!(good.call_global::<_, String>("Ping", ()).unwrap(), "pong");
        assert_eq!(rt.error_count(), 2);
        let recent = rt.recent_errors();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].context, "Bad");
        assert_eq!(recent[0].seq, 1);
        let log = rt.take_log();
        assert!(
            log.iter()
                .any(|l| l.starts_with("ERROR [Bad]") && l.contains("Bad.lua:2")),
            "{log:?}"
        );
        assert!(rt.take_log().is_empty());

        // A runaway script is stopped without hanging the others.
        let cfg = RuntimeConfig {
            mode,
            instruction_limit: Some(50_000),
            ..RuntimeConfig::default()
        };
        let rt2 = Runtime::new(cfg, files(), |_, _| Ok(())).unwrap();
        let spin = rt2.create_context("Spin", |_, _| Ok(())).unwrap();
        let other = rt2.create_context("Other", |_, _| Ok(())).unwrap();
        let e = spin.exec("while true do end", "@spin").unwrap_err();
        assert!(e.message.contains("instruction limit"), "{e}");
        other.exec("x = 1", "@other").unwrap();
    }
}

#[test]
fn error_ring_keeps_the_latest() {
    let cfg = RuntimeConfig {
        error_ring: 3,
        ..RuntimeConfig::default()
    };
    let rt = Runtime::new(cfg, files(), |_, _| Ok(())).unwrap();
    let c = rt.create_context("C", |_, _| Ok(())).unwrap();
    for i in 0..5 {
        let _ = c.exec(&format!("error('e{i}')"), "@c");
    }
    assert_eq!(rt.error_count(), 5);
    let seqs: Vec<u64> = rt.recent_errors().iter().map(|r| r.seq).collect();
    assert_eq!(seqs, vec![3, 4, 5]);
}

#[test]
fn print_goes_to_the_log() {
    let rt = rt(StateMode::Shared, IncludePolicy::OncePerContext);
    let seen = Rc::new(std::cell::RefCell::new(Vec::new()));
    let s2 = seen.clone();
    rt.set_log_sink(move |_, line| s2.borrow_mut().push(line.to_owned()));
    let c = ctx(&rt, "P");
    c.exec("print('a', 1, nil, true, {} ~= nil)", "@p").unwrap();
    assert_eq!(rt.take_log(), vec!["a\t1\tnil\ttrue\ttrue".to_string()]);
    assert_eq!(seen.borrow().len(), 1);
}

#[test]
fn include_finds_files_by_name_in_any_directory() {
    for mode in BOTH {
        let rt = rt(mode, IncludePolicy::OncePerContext);
        let c = ctx(&rt, "I");
        c.exec(
            "include('InstanceManager'); include('instancemanager.LUA'); include('Helper')
             include('EUI_core')
             assert(InstanceManager.tag == 'base' and helper_version == 1 and EUI ~= nil)
             assert(loads == 2)",
            "@top",
        )
        .unwrap();
        // Included code defines globals in the including context only.
        let other = ctx(&rt, "J");
        other
            .exec(
                "assert(InstanceManager == nil and helper_version == nil)",
                "@j",
            )
            .unwrap();
        // Directory suffix.
        other
            .exec(
                "include('Util\\\\Helper'); assert(helper_version == 1)",
                "@j",
            )
            .unwrap();
        other.include("Bom").unwrap();
        other.exec("assert(bom_ok)", "@j").unwrap();
    }
}

#[test]
fn include_policy_once_or_always() {
    let once = rt(StateMode::Shared, IncludePolicy::OncePerContext);
    let c = ctx(&once, "C");
    c.exec(
        "include('InstanceManager'); include('InstanceManager')",
        "@c",
    )
    .unwrap();
    assert_eq!(c.env().get::<_, i32>("loads").unwrap(), 1);
    // Another context runs it again.
    let d = ctx(&once, "D");
    d.include("InstanceManager").unwrap();
    assert_eq!(d.env().get::<_, i32>("loads").unwrap(), 1);

    let always = rt(StateMode::Shared, IncludePolicy::Always);
    let c = ctx(&always, "C");
    c.exec(
        "include('InstanceManager'); include('InstanceManager')",
        "@c",
    )
    .unwrap();
    assert_eq!(c.env().get::<_, i32>("loads").unwrap(), 2);
}

#[test]
fn include_cycles_end() {
    let rt = rt(StateMode::Shared, IncludePolicy::OncePerContext);
    let c = ctx(&rt, "C");
    c.include("Cycle1").unwrap();
    assert_eq!(c.env().get::<_, String>("cycle").unwrap(), "12");
}

#[test]
fn wildcard_include_runs_all_matches_and_returns_paths() {
    for mode in BOTH {
        let rt = rt(mode, IncludePolicy::OncePerContext);
        let c = ctx(&rt, "G");
        c.exec(
            r#"local files = include("InGame\\PopupsGeneric\\%w+Popup.lua", true)
               assert(#files == 2, #files)
               assert(popups == 'AB', popups)
               for i, v in ipairs(files) do assert(type(v) == 'string') end
               local none = include("NoSuchThing%d+", true)
               assert(type(none) == 'table' and #none == 0)"#,
            "@g",
        )
        .unwrap();
        let paths = c.include_pattern("PopupsGeneric.-Popup").unwrap();
        assert_eq!(paths.len(), 2);
        // A malformed pattern is a Lua error, not a crash.
        let e = c.exec("include('[a', true)", "@g").unwrap_err();
        assert!(e.message.contains("bad pattern"), "{e}");
    }
}

#[test]
fn include_errors_carry_file_and_line() {
    let rt = rt(StateMode::Shared, IncludePolicy::OncePerContext);
    let c = ctx(&rt, "E");
    let e = c.exec("include('NoSuchFile')", "@Top.lua").unwrap_err();
    assert!(e.message.contains("file 'NoSuchFile' not found"), "{e}");
    assert!(e.message.starts_with("Top.lua:1:"), "{e}");
    let e = c.exec("\ninclude('Broken')", "@Top.lua").unwrap_err();
    assert_eq!(e.location(), Some(("UI/Broken.lua", 2)), "{e}");
    let e = c.exec("include('Throws')", "@Top.lua").unwrap_err();
    assert_eq!(e.location(), Some(("UI/Throws.lua", 2)), "{e}");
    let tb = e.traceback.as_deref().unwrap();
    assert!(
        tb.contains("UI/Throws.lua:2") && tb.contains("Top.lua:1"),
        "{tb}"
    );
    let e = c.exec("include(5)", "@Top.lua").unwrap_err();
    assert!(e.message.contains("bad argument #1 to 'include'"), "{e}");
    // The context is usable after all that.
    c.exec("ok = true", "@Top.lua").unwrap();
    assert_eq!(rt.error_count(), 4);
}

#[test]
fn loadstring_runs_in_the_callers_context() {
    for mode in BOTH {
        let rt = rt(mode, IncludePolicy::OncePerContext);
        let a = ctx(&rt, "A");
        let b = ctx(&rt, "B");
        a.exec(
            "local f = loadstring('who = ContextName; return who'); assert(f() == 'A')",
            "@a",
        )
        .unwrap();
        b.exec(
            "local f = loadstring('who = ContextName; return who'); assert(f() == 'B')",
            "@b",
        )
        .unwrap();
        assert_eq!(a.env().get::<_, String>("who").unwrap(), "A");
        assert_eq!(b.env().get::<_, String>("who").unwrap(), "B");
        // Sandbox is on in contexts.
        a.exec(
            "assert(io == nil and os.execute == nil and loadfile == nil)",
            "@a",
        )
        .unwrap();
    }
}

#[test]
fn host_userdata_in_contexts_and_context_creation_from_lua() {
    // exe-API style: a closure in the base globals creates contexts through a weak runtime.
    let slot: Rc<std::cell::RefCell<Option<lua_sys::WeakRuntime>>> = Rc::default();
    let s2 = slot.clone();
    let created = Rc::new(std::cell::RefCell::new(Vec::<Context>::new()));
    let c2 = created.clone();
    let rt = Runtime::new(RuntimeConfig::default(), files(), move |lua, g| {
        let s = s2.clone();
        let c = c2.clone();
        g.set(
            "LoadNewContext",
            lua.create_function(move |_, name: String| {
                let rt = s
                    .borrow()
                    .as_ref()
                    .and_then(|w| w.upgrade())
                    .ok_or("runtime gone")?;
                let ctx = rt.create_context(&name, |_, _| Ok(()))?;
                ctx.include("InstanceManager")?;
                c.borrow_mut().push(ctx);
                Ok(())
            }),
        )
    })
    .unwrap();
    *slot.borrow_mut() = Some(rt.downgrade());
    let top = ctx(&rt, "Top");
    top.exec("LoadNewContext('Child')", "@top").unwrap();
    let child = created.borrow()[0].clone();
    assert_eq!(child.name(), "Child");
    child
        .exec("assert(InstanceManager.tag == 'base')", "@child")
        .unwrap();
    top.exec("assert(InstanceManager == nil)", "@top").unwrap();
}

#[test]
fn dropping_everything_in_any_order_is_fine() {
    let rt = rt(StateMode::Shared, IncludePolicy::OncePerContext);
    let c = ctx(&rt, "C");
    let key = c.lua().create_registry_value(c.env()).unwrap();
    let tbl: Table = c.lua().registry_value(&key).unwrap();
    drop(tbl);
    drop(rt);
    drop(c); // the state closes here
    drop(key);
    let _: Result<(), LuaError> = Ok(());
    let _: &dyn IncludeSource = &*files();
}
