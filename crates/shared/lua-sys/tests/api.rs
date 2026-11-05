//! Safe API: tables, closures, userdata, registry, errors. Native only (needs the C++ Lua).

use std::cell::Cell;
use std::rc::Rc;

use lua_sys::{
    AnyUserData, Function, Lua, LuaError, LuaErrorKind, LuaValue, MetaMethod, MultiValue, Table,
    UserData, UserDataMethods, Variadic,
};

/// Full collection at every allocation: finalizers run at the worst moments.
const GC_STRESS: &str = r#"
collectgarbage("setpause", 0)
collectgarbage("setstepmul", 0)
"#;

#[test]
fn tables_get_set_iterate_metatables() {
    let lua = Lua::new().unwrap();
    let t = lua.create_table();
    t.set("a", 1).unwrap();
    t.set(1, "one").unwrap();
    t.push("two").unwrap();
    assert_eq!(t.get::<_, i32>("a").unwrap(), 1);
    assert_eq!(t.get::<_, String>(2).unwrap(), "two");
    assert_eq!(t.raw_len(), 2);
    assert!(t.contains_key("a").unwrap());
    assert!(!t.contains_key("zzz").unwrap());
    assert_eq!(t.sequence_values::<String>().unwrap(), vec!["one", "two"]);

    let mut pairs: Vec<(String, String)> = t
        .pairs::<LuaValue, LuaValue>()
        .map(|p| {
            let (k, v) = p.unwrap();
            (format!("{k:?}"), format!("{v:?}"))
        })
        .collect();
    pairs.sort();
    assert_eq!(pairs.len(), 3);

    // Modifying the table inside the loop is safe (snapshot).
    for p in t.pairs::<LuaValue, LuaValue>() {
        let (k, _) = p.unwrap();
        t.set(k, LuaValue::Nil).unwrap();
    }
    assert_eq!(t.pairs::<LuaValue, LuaValue>().count(), 0);

    // Metatables with __index function and __newindex that errors.
    lua.globals().set("t", t.clone()).unwrap();
    lua.exec(
        r#"setmetatable(t, {
            __index = function(_, k) return "idx:" .. tostring(k) end,
            __newindex = function(_, k) error("read-only " .. k) end })"#,
        "meta",
    )
    .unwrap();
    assert_eq!(t.get::<_, String>("x").unwrap(), "idx:x");
    assert_eq!(t.raw_get::<_, LuaValue>("x").unwrap(), LuaValue::Nil);
    let e = t.set("y", 1).unwrap_err();
    assert!(e.message.contains("read-only y"), "{e}");
    assert!(e.traceback.is_some(), "{e:#}");
    t.raw_set("y", 2).unwrap();
    assert!(t.metatable().is_some());
    t.set_metatable(None);
    assert!(t.metatable().is_none());
    assert_eq!(t.get::<_, i32>("y").unwrap(), 2);
}

#[test]
fn raw_set_rejects_nil_and_nan_keys() {
    let lua = Lua::new().unwrap();
    let t = lua.create_table();
    assert!(t.raw_set(LuaValue::Nil, 1).is_err());
    assert!(t.raw_set(f64::NAN, 1).is_err());
    // The state is intact.
    t.raw_set("ok", 1).unwrap();
}

#[test]
fn conversions() {
    let lua = Lua::new().unwrap();
    let g = lua.globals();
    g.set("n", 3.7).unwrap();
    g.set("s", "12").unwrap();
    assert_eq!(g.get::<_, i32>("n").unwrap(), 3);
    assert_eq!(g.get::<_, f64>("s").unwrap(), 12.0);
    assert_eq!(g.get::<_, String>("n").unwrap(), "3.7");
    assert!(g.get::<_, bool>("n").unwrap());
    assert!(!g.get::<_, bool>("missing").unwrap());
    assert_eq!(g.get::<_, Option<i32>>("missing").unwrap(), None);
    assert!(g.get::<_, i32>("math").is_err());
    let v: Vec<i32> = lua.load("return {1, 2, 3}").eval().unwrap();
    assert_eq!(v, vec![1, 2, 3]);
    let text: String = lua.load("return 0.1 + 0.2").eval().unwrap();
    assert_eq!(text, "0.3");
    let big: String = lua.load("return 2^53").eval().unwrap();
    assert_eq!(big, "9.007199254741e+15"); // %.14g, like Lua
    let small: String = lua.load("return 1e100").eval().unwrap();
    assert_eq!(small, "1e+100");
    // Strings are bytes: invalid UTF-8 survives a round trip.
    let bytes: lua_sys::LuaString = lua.load(r"return '\255\254'").eval().unwrap();
    assert_eq!(bytes.as_bytes(), &[0xff, 0xfe]);
}

#[test]
fn multiple_returns_and_varargs() {
    let lua = Lua::new().unwrap();
    let f = lua.create_function(|_, (a, rest): (i32, Variadic<i32>)| {
        Ok((a, rest.iter().sum::<i32>(), "x"))
    });
    lua.globals().set("f", f).unwrap();
    let (a, sum, s): (i32, i32, String) = lua.load("return f(1, 2, 3, 4)").eval().unwrap();
    assert_eq!((a, sum, s.as_str()), (1, 9, "x"));
    // Missing arguments are nil; extra ones are ignored.
    let two =
        lua.create_function(|_, (a, b): (Option<i32>, Option<i32>)| Ok((a.is_none(), b.is_none())));
    lua.globals().set("two", two).unwrap();
    let r: (bool, bool) = lua.load("return two(1)").eval().unwrap();
    assert_eq!(r, (false, true));
    let r: (bool, bool) = lua.load("return two(1, 2, 3, 4)").eval().unwrap();
    assert_eq!(r, (false, false));
    // Many results.
    let all: MultiValue = lua.load("return 1, nil, 3, nil").eval().unwrap();
    assert_eq!(all.len(), 4);
    let count: i32 = lua
        .load("return select('#', f(0, 1, 2, 3))")
        .eval()
        .unwrap();
    assert_eq!(count, 3);
    // Bad argument is a Lua error with position.
    let e = lua.load("f('x')").exec().unwrap_err();
    assert!(e.message.contains("bad argument #1"), "{e}");
    assert_eq!(e.location().map(|l| l.1), Some(1), "{e}");
}

#[test]
fn closures_with_state_and_drop() {
    struct Probe(Rc<Cell<u32>>);
    impl Drop for Probe {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }
    let dropped = Rc::new(Cell::new(0));
    {
        let lua = Lua::new().unwrap();
        let mut counter = 0;
        let probe = Probe(dropped.clone());
        let f = lua.create_function_mut(move |_, ()| {
            let _ = &probe;
            counter += 1;
            Ok(counter)
        });
        lua.globals().set("next", f).unwrap();
        let r: Vec<i32> = lua.load("return {next(), next(), next()}").eval().unwrap();
        assert_eq!(r, vec![1, 2, 3]);
        // Collected when unreachable.
        lua.load("next = nil").exec().unwrap();
        lua.gc_collect().unwrap();
        assert_eq!(dropped.get(), 1);

        let probe2 = Probe(dropped.clone());
        let g = lua.create_function(move |_, ()| {
            let _ = &probe2;
            Ok(())
        });
        lua.globals().set("keep", g).unwrap();
        // Dropped when the state closes.
    }
    assert_eq!(dropped.get(), 2);
}

#[test]
fn recursive_mut_closure_is_an_error_not_ub() {
    let lua = Lua::new().unwrap();
    let f = lua.create_function_mut(|lua, ()| {
        let again: Function = lua.globals().get("again")?;
        again.call::<_, ()>(())?;
        Ok(())
    });
    lua.globals().set("again", f).unwrap();
    let e = lua.load("again()").exec().unwrap_err();
    assert!(e.message.contains("recursively"), "{e}");
}

#[test]
fn rust_panic_becomes_lua_error() {
    let lua = Lua::new().unwrap();
    let f = lua.create_function(|_, ()| -> Result<(), LuaError> { panic!("kaboom") });
    lua.globals().set("boom", f).unwrap();
    let ok: bool = lua
        .load("local ok, e = pcall(boom); return not ok and string.find(e, 'kaboom') ~= nil")
        .eval()
        .unwrap();
    assert!(ok);
    // The state is usable afterwards.
    lua.exec("x = 1", "after").unwrap();
}

#[test]
fn registry_keys_outlive_calls_and_states() {
    let key;
    {
        let lua = Lua::new().unwrap();
        let f: Function = lua
            .load("return function(x) return x * 2 end")
            .eval()
            .unwrap();
        key = lua.create_registry_value(f).unwrap();
        let again: Function = lua.registry_value(&key).unwrap();
        assert_eq!(again.call::<_, i32>(21).unwrap(), 42);
        let other = Lua::new().unwrap();
        assert!(other.registry_value::<Function>(&key).is_err());
        lua.set_named_registry_value("answer", 42).unwrap();
        assert_eq!(lua.named_registry_value::<i32>("answer").unwrap(), 42);
        lua.remove_registry_value(lua.create_registry_value(1).unwrap())
            .unwrap();
    }
    // Dropping a key after its state is closed is fine.
    drop(key);
}

#[derive(Debug)]
struct Counter {
    n: i32,
    dropped: Rc<Cell<u32>>,
}

impl Drop for Counter {
    fn drop(&mut self) {
        self.dropped.set(self.dropped.get() + 1);
    }
}

impl UserData for Counter {
    fn add_methods<'lua, M: UserDataMethods<'lua, Self>>(m: &mut M) {
        m.add_method("get", |_, this, ()| Ok(this.n));
        m.add_method_mut("add", |_, this, by: i32| {
            this.n += by;
            Ok(this.n)
        });
        m.add_method("reenter", |lua, _this, f: Function| {
            f.call::<_, ()>(lua.globals())
        });
        m.add_method_mut("reenter_mut", |_, _this, f: Function| f.call::<_, ()>(()));
        m.add_function("make", |_, n: i32| Ok(n));
        m.add_meta_method(MetaMethod::ToString, |_, this, ()| {
            Ok(format!("Counter({})", this.n))
        });
        m.add_meta_method(MetaMethod::Index, |_, this, key: String| {
            Ok(if key == "n" { Some(this.n) } else { None })
        });
        m.add_meta_method(MetaMethod::Call, |_, this, by: i32| Ok(this.n + by));
        m.add_meta_function(MetaMethod::Eq, |_, (a, b): (AnyUserData, AnyUserData)| {
            Ok(a.borrow::<Counter>()?.n == b.borrow::<Counter>()?.n)
        });
        m.add_meta_method(MetaMethod::Len, |_, this, ()| Ok(this.n));
    }
    fn type_name() -> &'static str {
        "Counter"
    }
}

struct Other;
impl UserData for Other {}

#[test]
fn userdata_methods_meta_and_gc() {
    let dropped = Rc::new(Cell::new(0));
    {
        let lua = Lua::new().unwrap();
        lua.exec(GC_STRESS, "stress").unwrap();
        let c = lua
            .create_userdata(Counter {
                n: 5,
                dropped: dropped.clone(),
            })
            .unwrap();
        lua.globals().set("c", c.clone()).unwrap();
        lua.globals()
            .set("other", lua.create_userdata(Other).unwrap())
            .unwrap();
        lua.globals()
            .set(
                "c2",
                lua.create_userdata(Counter {
                    n: 5,
                    dropped: dropped.clone(),
                })
                .unwrap(),
            )
            .unwrap();
        let r: (i32, i32, String, i32, i32, bool, i32) = lua
            .load(
                "local a = c:get(); local b = c:add(2)
                 return a, b, tostring(c), c.n, c(10), c == c2, #c",
            )
            .eval()
            .unwrap();
        assert_eq!(r, (5, 7, "Counter(7)".into(), 7, 17, false, 7));
        assert!(c.is::<Counter>());
        assert!(!c.is::<Other>());
        assert_eq!(c.borrow::<Counter>().unwrap().n, 7);
        c.borrow_mut::<Counter>().unwrap().n = 100;
        assert_eq!(lua.load("return c:get()").eval::<i32>().unwrap(), 100);

        // Wrong type of self, and a missing method on the other type.
        let e = lua.load("return c.get(other)").eval::<i32>().unwrap_err();
        assert!(e.message.contains("Counter"), "{e}");
        assert!(lua.load("c.nothing()").exec().is_err());

        // Borrow conflicts are errors.
        let guard = c.borrow::<Counter>().unwrap();
        let e = lua.load("c:add(1)").exec().unwrap_err();
        assert!(e.message.contains("already borrowed"), "{e}");
        drop(guard);
        let e = lua
            .load("c:reenter_mut(function() c:get() end)")
            .exec()
            .unwrap_err();
        assert!(e.message.contains("borrowed"), "{e}");

        // Finalizer runs on collection.
        lua.load("c2 = nil").exec().unwrap();
        lua.gc_collect().unwrap();
        assert_eq!(dropped.get(), 1);
    }
    // And on close.
    assert_eq!(dropped.get(), 2);
}

#[test]
fn errors_both_ways_with_traceback_and_location() {
    let lua = Lua::new().unwrap();
    let f =
        lua.create_function(|_, ()| -> Result<(), LuaError> { Err(LuaError::msg("from rust")) });
    lua.globals().set("rust_fail", f).unwrap();

    // Lua -> Rust.
    let src =
        "local function inner()\n  error('deep')\nend\nlocal function outer() inner() end\nouter()";
    let e = lua.load(src).set_name("@UI/Foo.lua").exec().unwrap_err();
    assert_eq!(e.kind, LuaErrorKind::Runtime);
    assert_eq!(e.message, "UI/Foo.lua:2: deep");
    assert_eq!(e.location(), Some(("UI/Foo.lua", 2)));
    let tb = e.traceback.as_deref().unwrap();
    assert!(tb.contains("UI/Foo.lua:2: in function 'inner'"), "{tb}");
    assert!(tb.contains("UI/Foo.lua:4: in function 'outer'"), "{tb}");

    // Rust -> Lua: the caller's position is added.
    let e = lua
        .load("\n\nrust_fail()")
        .set_name("@UI/Bar.lua")
        .exec()
        .unwrap_err();
    assert_eq!(e.message, "UI/Bar.lua:3: from rust");
    assert!(e.traceback.is_some());

    // Lua -> Rust -> Lua -> Rust: the inner error text survives, with both tracebacks.
    let calls_lua = lua.create_function(|lua, ()| {
        let f: Function = lua.globals().get("thrower")?;
        f.call::<_, ()>(())
    });
    lua.globals().set("calls_lua", calls_lua).unwrap();
    lua.load("function thrower() error('inner boom') end")
        .set_name("@T.lua")
        .exec()
        .unwrap();
    let e = lua
        .load("calls_lua()")
        .set_name("@Top.lua")
        .exec()
        .unwrap_err();
    assert_eq!(e.message, "T.lua:1: inner boom", "{e:#}");
    let tb = e.traceback.unwrap();
    assert!(tb.contains("T.lua:1"), "{tb}");
    assert!(tb.contains("Top.lua:1"), "{tb}");

    // pcall inside Lua sees the message only.
    let ok: bool = lua
        .load("local ok, m = pcall(rust_fail); return not ok and m == 'from rust'")
        .eval()
        .unwrap();
    assert!(ok);

    // Non-string error objects.
    let e = lua.load("error({})").exec().unwrap_err();
    assert_eq!(e.message, "(error object is a table value)");
    let e = lua
        .load("error(setmetatable({}, {__tostring = function() return 'custom' end}))")
        .exec()
        .unwrap_err();
    assert!(e.message.contains("custom"), "{e}");

    // Syntax errors carry file and line.
    let e = lua
        .load("x = 1\ny = = 2")
        .set_name("@Syn.lua")
        .exec()
        .unwrap_err();
    assert_eq!(e.kind, LuaErrorKind::Syntax);
    assert_eq!(e.location(), Some(("Syn.lua", 2)));
}

#[test]
fn stack_does_not_grow_after_errors() {
    let lua = Lua::new().unwrap();
    let top = || unsafe { lua_sys::ffi::lua_gettop(lua.as_ptr()) };
    let before = top();
    for _ in 0..50 {
        let _ = lua.load("error('x')").exec();
        let t = lua.create_table();
        let _ = t.set(LuaValue::Nil, 1);
        let _ = lua.load("return +").into_function();
    }
    assert_eq!(top(), before);
}

#[test]
fn metamethod_errors_and_gc_errors_are_contained() {
    let lua = Lua::new().unwrap();
    lua.exec(GC_STRESS, "stress").unwrap();
    // __gc that errors, armed, then Rust API calls that allocate.
    lua.exec(
        "do local p = newproxy(true); getmetatable(p).__gc = function() error('gc boom') end end",
        "arm",
    )
    .unwrap();
    let mut seen = false;
    for i in 0..200 {
        // Each of these allocates; none may unwind through Rust.
        let t = lua.create_table();
        let _ = t.set("k", format!("value {i}"));
        let _: Vec<String> = vec![format!("v{i}")];
        let r = lua.create_sequence_from(vec![1, 2, 3]);
        assert!(r.is_ok());
        if let Err(e) = lua.load("return 1").exec() {
            assert!(e.message.contains("gc boom"), "{e}");
            seen = true;
        }
    }
    let _ = seen; // the finalizer may have run inside any protected call; either way no crash
    lua.exec("x = 1", "after").unwrap();
}

#[test]
fn instruction_limit_stops_runaway_scripts() {
    let lua = Lua::new().unwrap();
    lua.set_instruction_limit(Some(100_000));
    let e = lua.load("while true do end").exec().unwrap_err();
    assert!(e.message.contains("instruction limit"), "{e}");
    // Re-armed per outer call: a short script still runs.
    lua.load("local s = 0 for i = 1, 100 do s = s + i end")
        .exec()
        .unwrap();
    lua.set_instruction_limit(None);
    lua.load("for i = 1, 300000 do end").exec().unwrap();
}

#[test]
fn chunks_environment_bom_and_bytecode() {
    let lua = Lua::new().unwrap();
    let env = lua.create_table();
    lua.load("a = 5")
        .set_environment(env.clone())
        .exec()
        .unwrap();
    assert_eq!(env.get::<_, i32>("a").unwrap(), 5);
    assert_eq!(lua.get_global("a"), lua_sys::Value::Nil);
    // BOM is skipped.
    lua.load(b"\xEF\xBB\xBFb = 6").exec().unwrap();
    assert_eq!(lua.get_global("b").as_i32(), Some(6));
    // Bytecode is refused.
    let dumped: lua_sys::LuaString = lua
        .load("return string.dump(function() end)")
        .eval()
        .unwrap();
    let e = lua.load(dumped.as_bytes()).into_function().unwrap_err();
    assert_eq!(e.kind, LuaErrorKind::Syntax);
    // Function environments.
    let f = lua.load("return z").into_function().unwrap();
    assert!(f.set_environment(&env));
    env.set("z", 9).unwrap();
    assert_eq!(f.call::<_, i32>(()).unwrap(), 9);
    assert_eq!(f.environment().unwrap(), env);
}

#[test]
fn call_method_and_equality() {
    let lua = Lua::new().unwrap();
    let t: Table = lua
        .load("return {x = 2, mul = function(self, k) return self.x * k end}")
        .eval()
        .unwrap();
    assert_eq!(t.call_method::<_, i32>("mul", 21).unwrap(), 42);
    assert!(t.call_method::<_, i32>("nope", ()).is_err());
    assert_eq!(t, t.clone());
    assert_ne!(t, lua.create_table());
}

#[test]
fn sandbox_removes_dangerous_globals() {
    let lua = Lua::new().unwrap();
    lua.sandbox().unwrap();
    for name in ["io", "dofile", "loadfile", "require", "package", "module"] {
        assert_eq!(lua.get_global(name), lua_sys::Value::Nil, "{name}");
    }
    lua.load(
        "assert(os.execute == nil and os.exit == nil and os.getenv == nil and os.remove == nil)
         assert(type(os.time()) == 'number' and type(os.clock()) == 'number' and os.date)
         assert(string.dump == nil)
         assert(debug.sethook == nil and debug.setmetatable == nil and debug.getregistry == nil)
         assert(type(debug.traceback()) == 'string')",
    )
    .exec()
    .unwrap();
    // loadstring: text works, bytecode is refused, syntax errors are values.
    lua.load(
        "local f = loadstring('return 1 + 1'); assert(f() == 2)
         local g, err = loadstring('x = = 1'); assert(g == nil and err:find('unexpected'))
         assert(not pcall(loadstring, 5))
         local h = load(coroutine.wrap(function() coroutine.yield('return '); coroutine.yield('7') end))
         assert(h() == 7)",
    )
    .exec()
    .unwrap();
    // Dumped bytecode (from before the sandbox) is refused by loadstring.
    let plain = Lua::new().unwrap();
    let dumped: lua_sys::LuaString = plain
        .load("return string.dump(function() end)")
        .eval()
        .unwrap();
    lua.globals().set("bc", dumped).unwrap();
    lua.load("local f, e = loadstring(bc); assert(f == nil and e:find('binary'))")
        .exec()
        .unwrap();
}
