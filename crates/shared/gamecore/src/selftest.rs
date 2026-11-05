//! Spike selftest: Rust <-> C++ <-> Lua.

use std::ffi::c_int;
use std::fmt;

use gamecore_sys::spike;
use lua_sys::{CallContext, Lua, Value};

use crate::{Game, GameEvent, HostServices};

/// Result of one case.
#[derive(Debug, Clone)]
pub struct SelftestCase {
    pub name: &'static str,
    pub ok: bool,
    pub detail: String,
}

/// Selftest report; `Display` prints PASS/FAIL per case.
#[derive(Debug, Clone, Default)]
pub struct SelftestReport {
    pub cases: Vec<SelftestCase>,
}

impl SelftestReport {
    /// All cases pass, and there is at least one.
    pub fn passed(&self) -> bool {
        !self.cases.is_empty() && self.cases.iter().all(|c| c.ok)
    }

    fn check(&mut self, name: &'static str, result: Result<String, String>) {
        let (ok, detail) = match result {
            Ok(d) => (true, d),
            Err(d) => (false, d),
        };
        self.cases.push(SelftestCase { name, ok, detail });
    }
}

impl fmt::Display for SelftestReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", if self.passed() { "PASS" } else { "FAIL" })?;
        for c in &self.cases {
            let mark = if c.ok { "ok  " } else { "FAIL" };
            writeln!(f, "  [{mark}] {} {}", c.name, c.detail)?;
        }
        Ok(())
    }
}

/// Rust function for Lua that calls C++.
fn rust_mix(ctx: &mut CallContext<'_>) -> Result<c_int, String> {
    let x = ctx.arg_i32(1)?;
    ctx.push(&Value::from(spike::mix(x)));
    Ok(1)
}

/// Rust function for Lua that fails.
fn rust_fail(ctx: &mut CallContext<'_>) -> Result<c_int, String> {
    let n = ctx.arg_i32(1).unwrap_or(0);
    Err(format!("rust failed {n}"))
}

const LUA_SETUP: &str = r#"
function spike_roundtrip(x) return rust_mix(x) + 1000 end
function spike_roundtrip_fail(x) return rust_fail(x) end
"#;

fn expect(cond: bool, ok: impl Into<String>, fail: impl Into<String>) -> Result<String, String> {
    if cond {
        Ok(ok.into())
    } else {
        Err(fail.into())
    }
}

/// Runs all spike cases; repeatable.
pub fn selftest() -> SelftestReport {
    let mut r = SelftestReport::default();

    let ctors = spike::ctor_count();
    r.check(
        "cpp_global_ctor",
        expect(
            ctors == 1 && spike::global_ok(),
            "constructor ran once",
            format!("ctor_count={ctors} global_ok={}", spike::global_ok()),
        ),
    );

    r.check(
        "cpp_throw_catch",
        match spike::throw_catch(7) {
            Ok(m) if m == "spike error 7" => Ok(format!("caught '{m}'")),
            Ok(m) => Err(format!("unexpected what(): '{m}'")),
            Err(m) => Err(m),
        },
    );

    r.check("cpp_stl", spike::stl());
    r.check("sqlite_memory", spike::sqlite());

    match Lua::new() {
        Ok(lua) => lua_cases(&mut r, &lua),
        Err(e) => r.check("lua_state", Err(e.to_string())),
    }

    r.check(
        "abi_version",
        expect(
            gamecore_sys::abi_version() == gamecore_sys::ffi::GC_ABI_VERSION,
            format!("v{}", gamecore_sys::abi_version()),
            format!("got v{}", gamecore_sys::abi_version()),
        ),
    );

    r.check(
        "implementation",
        Ok(format!(
            "{} (abi v{})",
            gamecore_sys::IMPLEMENTATION,
            gamecore_sys::abi_version()
        )),
    );
    game_cases(&mut r);

    crate::flush_cpp_output();
    r
}

fn lua_cases(r: &mut SelftestReport, lua: &Lua) {
    let setup = lua
        .register("rust_mix", rust_mix)
        .and_then(|()| lua.register("rust_fail", rust_fail))
        .and_then(|()| lua.exec(LUA_SETUP, "selftest"));
    if let Err(e) = setup {
        r.check("lua_setup", Err(e.to_string()));
        return;
    }
    if let Err(code) = spike::lua_register(lua) {
        r.check("lua_setup", Err(format!("gc_spike_lua_register: {code}")));
        return;
    }

    // Rust -> C++ -> Lua -> Rust -> C++: 5 * 3 + 1 + 1000.
    r.check(
        "rust_cpp_lua_rust_cpp",
        match spike::lua_call(lua, "spike_roundtrip", 5) {
            Ok(1016) => Ok("5 -> 1016".into()),
            Ok(v) => Err(format!("expected 1016, got {v}")),
            Err(e) => Err(e),
        },
    );

    r.check(
        "lua_error_from_cpp",
        match lua.call_global("spike_cpp_raise", &[Value::from(42)]) {
            Err(e) if e.message.contains("cpp raised 42") => Ok(format!("caught '{}'", e.message)),
            Err(e) => Err(format!("unexpected error: {e}")),
            Ok(v) => Err(format!("no error, got {v:?}")),
        },
    );

    // A Rust error is caught by Lua, Rust and C++ pcalls alike.
    let in_lua = lua
        .exec(
            "local ok, e = pcall(rust_fail, 9); spike_rust_err = (not ok) and e or 'no error'",
            "selftest",
        )
        .map_err(|e| e.to_string())
        .and_then(|()| match lua.get_global("spike_rust_err") {
            Value::String(s) if s == "rust failed 9" => Ok(()),
            other => Err(format!("Lua pcall saw {other:?}")),
        });
    let in_rust = match lua.call_global("rust_fail", &[Value::from(3)]) {
        Err(e) if e.message == "rust failed 3" => Ok(()),
        other => Err(format!("Rust lua_pcall saw {other:?}")),
    };
    let in_cpp = match spike::lua_call(lua, "spike_roundtrip_fail", 4) {
        Err(m) if m == "rust failed 4" => Ok(()),
        other => Err(format!("C++ lua_pcall saw {other:?}")),
    };
    r.check(
        "rust_error_to_lua",
        in_lua
            .and(in_rust)
            .and(in_cpp)
            .map(|()| "caught in Lua, Rust and C++".into()),
    );
}

/// Host that ignores events.
struct QuietHost;

impl HostServices for QuietHost {
    fn game_event(&self, _ev: GameEvent) {}
}

/// New game, `pump` to `WAITING_HUMAN`, state digest.
fn game_cases(r: &mut SelftestReport) {
    if gamecore_sys::IMPLEMENTATION == "lekmod" {
        // The real core boots from the game database and Lua files, which only a host with game
        // data serves (`QuietHost` has none), and it is created once per process. A new game on
        // the real core is tested by `tests/real.rs` here and by `game-host`.
        return r.check(
            "game_pump",
            Ok(
                "skipped on the real core: it needs the game data (tests/real.rs, game-host)"
                    .into(),
            ),
        );
    }
    r.check("game_pump", pump_to_human());
}

fn pump_to_human() -> Result<String, String> {
    const MAX_CALLS: u32 = 3;
    let mut game = Game::new(QuietHost).map_err(|e| e.to_string())?;
    let run = (|| {
        game.new_game_json(r#"{"seed":4242}"#)?;
        for call in 1..=MAX_CALLS {
            let p = game.pump(1)?;
            if p.status == gamecore_sys::ffi::GC_PUMP_WAITING_HUMAN {
                let digest = game.state_digest(-1)?;
                return Ok(Some(format!(
                    "WAITING_HUMAN on turn {} after {call} pump call(s), digest {digest:016x}",
                    p.turn
                )));
            }
        }
        Ok::<_, crate::GameError>(None)
    })();
    drop(game);
    crate::flush_cpp_output();
    match run {
        Ok(Some(done)) => Ok(done),
        Ok(None) => Err(format!("still running after {MAX_CALLS} pump calls")),
        Err(e) => Err(e.to_string()),
    }
}
