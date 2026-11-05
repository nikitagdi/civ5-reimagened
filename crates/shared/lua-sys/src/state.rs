//! Safe wrapper over the Lua state. P5: no Lua error is raised into a Rust frame.
//!
//! Rules every function here follows:
//! - an API call that can raise (metamethods, `lua_call`, `lua_concat`, ...) runs under pcall in
//!   C++ (`lua_sys_*`), never directly from a Rust frame;
//! - an API call that allocates a collectable object runs with the GC paused, since `__gc` runs
//!   unprotected in Lua 5.1 (`lua_sys_createtable`, `lua_sys_newuserdata`, `lua_sys_pushlstring`, ...);
//! - the stack top is restored on every exit (`StackGuard`).

use std::any::TypeId;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{CString, c_int, c_void};
use std::fmt;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::{LuaError, LuaErrorKind, Result};
use crate::ffi;
use crate::value::LuaValue;

/// A Lua value, passed by value. Simple API for plain data; see [`LuaValue`] for handles.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Nil,
    Boolean(bool),
    Number(f64),
    String(String),
    /// Another type, by name.
    Other(&'static str),
}

impl Value {
    pub fn as_i32(&self) -> Option<i32> {
        match *self {
            Value::Number(n)
                if n.fract() == 0.0 && n >= i32::MIN as f64 && n <= i32::MAX as f64 =>
            {
                Some(n as i32)
            }
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Nil => "nil",
            Value::Boolean(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Other(t) => t,
        }
    }
}

impl From<i32> for Value {
    fn from(v: i32) -> Self {
        Value::Number(v as f64)
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::String(v.to_owned())
    }
}

/// Rust function callable from Lua (simple API: no state, no conversions; see `create_function`).
pub type RustFn = fn(&mut CallContext<'_>) -> std::result::Result<c_int, String>;

static NEXT_LUA_ID: AtomicU64 = AtomicU64::new(1);

/// State shared by [`Lua`] and the callbacks it owns. Closing the state is its `Drop`.
pub(crate) struct Inner {
    pub(crate) main: *mut ffi::lua_State,
    /// The thread that is running Rust code right now (a coroutine inside a callback).
    pub(crate) cur: Cell<*mut ffi::lua_State>,
    pub(crate) id: u64,
    /// Nesting of Rust-initiated calls; the instruction limit is armed at depth 0.
    pub(crate) depth: Cell<u32>,
    pub(crate) limit: Cell<Option<c_int>>,
    /// Set while `lua_close` runs finalizers: callbacks refuse to run then.
    pub(crate) closing: Cell<bool>,
    /// Registry slots of dropped [`crate::RegistryKey`]s, released by `expire_registry_values`.
    pub(crate) dead: Rc<RefCell<Vec<c_int>>>,
    /// Registry slot of each userdata type's metatable.
    pub(crate) type_mts: RefCell<HashMap<TypeId, c_int>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.closing.set(true);
        // SAFETY: we own the state; finalizers run here and may drop Rust payloads.
        unsafe { ffi::lua_close(self.main) }
    }
}

/// Owned Lua state with the standard libraries. Not `Send`.
pub struct Lua {
    pub(crate) inner: Rc<Inner>,
    _not_send: PhantomData<*mut ()>,
}

impl Lua {
    /// New state with all standard libraries (not sandboxed; see [`Lua::sandbox`]).
    pub fn new() -> Result<Self> {
        crate::init_cpp_runtime();
        // SAFETY: lua_sys_newstate never returns NULL (it aborts on failure).
        let l = unsafe { ffi::lua_sys_newstate() };
        assert!(!l.is_null(), "lua_sys_newstate returned NULL");
        let lua = Lua {
            inner: Rc::new(Inner {
                main: l,
                cur: Cell::new(l),
                id: NEXT_LUA_ID.fetch_add(1, Ordering::Relaxed),
                depth: Cell::new(0),
                limit: Cell::new(None),
                closing: Cell::new(false),
                dead: Rc::new(RefCell::new(Vec::new())),
                type_mts: RefCell::new(HashMap::new()),
            }),
            _not_send: PhantomData,
        };
        // SAFETY: openlibs runs inside lua_cpcall, so no error escapes.
        let status = unsafe { ffi::lua_sys_openlibs(l) };
        if status != ffi::LUA_OK {
            return Err(pop_error(l, status));
        }
        Ok(lua)
    }

    pub(crate) fn from_inner(inner: Rc<Inner>) -> Lua {
        Lua {
            inner,
            _not_send: PhantomData,
        }
    }

    /// The main thread's state, for FFI that registers C functions (`gc_lua_register`).
    pub fn as_ptr(&self) -> *mut ffi::lua_State {
        self.inner.main
    }

    /// The thread that runs Rust code now: the main state, or a coroutine inside a callback.
    pub(crate) fn l(&self) -> *mut ffi::lua_State {
        self.inner.cur.get()
    }

    /// Process-unique id of this state, to tell registry keys of different states apart.
    pub fn id(&self) -> u64 {
        self.inner.id
    }

    pub fn exec(&self, code: &str, chunk_name: &str) -> Result<()> {
        self.load(code.as_bytes()).set_name(chunk_name).exec()
    }

    pub fn call_global(&self, name: &str, args: &[Value]) -> Result<Vec<Value>> {
        let l = self.l();
        let _guard = StackGuard::new(l);
        // SAFETY: raw table access calls no metamethods.
        let base = unsafe {
            ensure_stack(l, args.len() + 4);
            let base = ffi::lua_gettop(l);
            push_str(l, name);
            ffi::lua_rawget(l, ffi::LUA_GLOBALSINDEX);
            if ffi::lua_type(l, -1) != ffi::LUA_TFUNCTION {
                let got = read_value(l, -1).type_name();
                return Err(LuaError::api(format!(
                    "global '{name}' is not a function (got {got})"
                )));
            }
            for a in args {
                push_value(l, a);
            }
            base
        };
        let nargs =
            c_int::try_from(args.len()).expect("argument count fits in the Lua stack limit");
        self.pcall(nargs, ffi::LUA_MULTRET)?;
        // SAFETY: results lie above base; reading them raises no errors.
        let results = unsafe {
            let top = ffi::lua_gettop(l);
            (base + 1..=top).map(|i| read_value(l, i)).collect()
        };
        Ok(results)
    }

    /// Sets a global (raw); only "table overflow" can fail.
    pub fn set_global(&self, name: &str, value: &Value) -> Result<()> {
        let l = self.l();
        let _guard = StackGuard::new(l);
        // SAFETY: pushes do not throw; lua_sys_rawset is protected. 4 slots, 2 for it.
        let status = unsafe {
            ensure_stack(l, 4);
            push_str(l, name);
            push_value(l, value);
            ffi::lua_sys_rawset(l, ffi::LUA_GLOBALSINDEX)
        };
        if status != ffi::LUA_OK {
            return Err(pop_error(l, status));
        }
        Ok(())
    }

    pub fn get_global(&self, name: &str) -> Value {
        let l = self.l();
        let _guard = StackGuard::new(l);
        // SAFETY: lua_rawget calls no metamethods.
        unsafe {
            ensure_stack(l, 1);
            push_str(l, name);
            ffi::lua_rawget(l, ffi::LUA_GLOBALSINDEX);
            read_value(l, -1)
        }
    }

    /// Registers a plain function as a global (simple API).
    pub fn register(&self, name: &str, f: RustFn) -> Result<()> {
        let l = self.l();
        let _guard = StackGuard::new(l);
        // SAFETY: `f` lives for the whole program and rust_fn_entry restores its type.
        // Pushes do not throw; lua_sys_rawset is protected. 4 slots, 2 for it.
        let status = unsafe {
            ensure_stack(l, 4);
            push_str(l, name);
            ffi::lua_sys_push_rust_function(l, rust_fn_entry, f as *mut c_void);
            ffi::lua_sys_rawset(l, ffi::LUA_GLOBALSINDEX)
        };
        if status != ffi::LUA_OK {
            return Err(pop_error(l, status));
        }
        Ok(())
    }

    /// Protected call with a traceback handler; function and args are on the stack. Arms the
    /// instruction limit for the outermost call. On error the message is popped.
    pub(crate) fn pcall(&self, nargs: c_int, nresults: c_int) -> Result<()> {
        let l = self.l();
        let inner = &*self.inner;
        let outer = inner.depth.get() == 0;
        let limit = if outer { inner.limit.get() } else { None };
        inner.depth.set(inner.depth.get() + 1);
        // SAFETY: function and arguments are on the stack; the call is protected.
        let status = unsafe {
            if let Some(n) = limit {
                ffi::lua_sys_arm_limit(l, n);
            }
            let status = ffi::lua_sys_pcall(l, nargs, nresults);
            if limit.is_some() {
                ffi::lua_sys_arm_limit(l, 0);
            }
            status
        };
        inner.depth.set(inner.depth.get() - 1);
        if status != ffi::LUA_OK {
            return Err(pop_error(l, status));
        }
        Ok(())
    }

    /// Releases the registry slots of dropped [`crate::RegistryKey`]s. Called by every entry point
    /// that creates values; hosts can call it once per frame as well.
    pub fn expire_registry_values(&self) {
        let dead: Vec<c_int> = std::mem::take(&mut *self.inner.dead.borrow_mut());
        if dead.is_empty() {
            return;
        }
        let l = self.l();
        for r in dead {
            // SAFETY: unref uses rawgeti/rawseti on the registry; no Lua code runs.
            unsafe {
                ffi::lua_checkstack(l, 3);
                ffi::luaL_unref(l, ffi::LUA_REGISTRYINDEX, r);
            }
        }
    }

    /// Limits the VM instructions of each outermost call from Rust (`None`: unlimited). The
    /// script then gets the error "script exceeded the instruction limit".
    pub fn set_instruction_limit(&self, limit: Option<u32>) {
        self.inner
            .limit
            .set(limit.map(|n| c_int::try_from(n.max(1)).unwrap_or(c_int::MAX)));
    }

    /// `debug.traceback(message, level)` of the running thread. Inside a Rust callback, level 1 is
    /// the callback itself and level 2 its Lua caller.
    pub fn traceback(&self, message: &str, level: i32) -> String {
        let l = self.l();
        let _guard = StackGuard::new(l);
        // SAFETY: the function is saved by openlibs; reads raise no errors. The call itself
        // (debug.traceback) raises nothing for a string message and a number level.
        unsafe {
            ensure_stack(l, 4);
            ffi::lua_getfield(l, ffi::LUA_REGISTRYINDEX, c"lua_sys.traceback".as_ptr());
            if ffi::lua_type(l, -1) != ffi::LUA_TFUNCTION {
                return message.to_owned();
            }
            push_str(l, message);
            ffi::lua_pushinteger(l, level);
            if ffi::lua_pcall(l, 2, 1, 0) != ffi::LUA_OK {
                return message.to_owned();
            }
            match read_value(l, -1) {
                Value::String(s) => s,
                _ => message.to_owned(),
            }
        }
    }

    /// Memory in use, in bytes (`collectgarbage("count")`).
    pub fn used_memory(&self) -> usize {
        // SAFETY: lua_gc(COUNT) allocates nothing.
        unsafe { ffi::lua_gc(self.l(), ffi::LUA_GCCOUNT, 0) as usize * 1024 }
    }

    /// Full garbage collection, under pcall: an error in a `__gc` function comes back as `Err`
    /// (the collection then continues at the next step). Finalizers drop Rust payloads, so do not
    /// call this while holding a borrow a payload's `Drop` could need.
    pub fn gc_collect(&self) -> Result<()> {
        let l = self.l();
        // SAFETY: the cycle runs inside lua_cpcall, so no finalizer error escapes.
        let status = unsafe { ffi::lua_sys_gc_collect(l) };
        if status != ffi::LUA_OK {
            return Err(pop_error(l, status));
        }
        Ok(())
    }
}

impl fmt::Debug for Lua {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Lua#{}", self.inner.id)
    }
}

pub struct CallContext<'a> {
    l: *mut ffi::lua_State,
    nargs: c_int,
    _life: PhantomData<&'a mut ffi::lua_State>,
}

impl CallContext<'_> {
    pub fn arg_count(&self) -> c_int {
        self.nargs
    }

    /// Argument `i`, 1-based; `Nil` when out of range.
    pub fn arg(&self, i: c_int) -> Value {
        if i < 1 || i > self.nargs {
            return Value::Nil;
        }
        // SAFETY: the index is within the call arguments.
        unsafe { read_value(self.l, i) }
    }

    pub fn arg_i32(&self, i: c_int) -> std::result::Result<i32, String> {
        let v = self.arg(i);
        v.as_i32().ok_or_else(|| {
            format!(
                "bad argument #{i} (integer expected, got {})",
                v.type_name()
            )
        })
    }

    /// Cannot raise a Lua error. A full stack aborts: a panic cannot unwind the trampoline.
    pub fn push(&mut self, v: &Value) {
        // SAFETY: the state is alive for the call.
        unsafe {
            ensure_stack(self.l, 1);
            push_value(self.l, v);
        }
    }

    pub fn as_ptr(&self) -> *mut ffi::lua_State {
        self.l
    }
}

/// Trampoline entry point: `data` is a `RustFn`.
unsafe extern "C" fn rust_fn_entry(l: *mut ffi::lua_State, data: *mut c_void) -> c_int {
    // SAFETY: data came from a RustFn in Lua::register; pointer size and representation match.
    let f = unsafe { std::mem::transmute::<*mut c_void, RustFn>(data) };
    // SAFETY: the trampoline calls us with a live state.
    let nargs = unsafe { ffi::lua_gettop(l) };
    let mut ctx = CallContext {
        l,
        nargs,
        _life: PhantomData,
    };
    // A panic aborts here (extern "C"), so it never crosses C++ (P5).
    match f(&mut ctx) {
        Ok(n) => {
            // luaD_poscall trusts n, so clamp it to the values actually pushed.
            // SAFETY: the state is alive for the call.
            let top = unsafe { ffi::lua_gettop(l) };
            n.clamp(0, (top - nargs).max(0))
        }
        Err(msg) => {
            // SAFETY: the message is pushed for lua_error; push does not throw.
            unsafe {
                ensure_stack(l, 1);
                push_str(l, &msg);
            }
            // Legacy functions keep their message exactly (no caller position).
            ffi::RUST_ERROR_RAW
        }
    }
}

/// Restores the stack top on scope exit.
pub(crate) struct StackGuard {
    l: *mut ffi::lua_State,
    top: c_int,
}

impl StackGuard {
    pub(crate) fn new(l: *mut ffi::lua_State) -> Self {
        // SAFETY: l is the owner's live state.
        let top = unsafe { ffi::lua_gettop(l) };
        StackGuard { l, top }
    }
}

impl Drop for StackGuard {
    fn drop(&mut self) {
        // SAFETY: functions only push (net), so `top` is not above the current top.
        unsafe { ffi::lua_settop(self.l, self.top) }
    }
}

pub(crate) fn c_string(s: &str) -> CString {
    CString::new(s.replace('\0', "?")).expect("NUL bytes were replaced")
}

/// SAFETY: `l` is a live state.
pub(crate) unsafe fn ensure_stack(l: *mut ffi::lua_State, extra: usize) {
    let extra = c_int::try_from(extra).unwrap_or(c_int::MAX);
    // SAFETY: lua_checkstack raises no errors.
    if unsafe { ffi::lua_checkstack(l, extra) } == 0 {
        panic!("Lua stack overflow: {extra} more slots needed");
    }
}

/// SAFETY: `l` is a live state with room on the stack.
pub(crate) unsafe fn push_str(l: *mut ffi::lua_State, s: &str) {
    // SAFETY: Lua copies the string; GC is paused.
    unsafe { ffi::lua_sys_pushlstring(l, s.as_ptr().cast(), s.len()) }
}

/// SAFETY: `l` is a live state with room on the stack.
pub(crate) unsafe fn push_bytes(l: *mut ffi::lua_State, s: &[u8]) {
    // SAFETY: Lua copies the bytes; GC is paused.
    unsafe { ffi::lua_sys_pushlstring(l, s.as_ptr().cast(), s.len()) }
}

/// SAFETY: `l` is a live state with room on the stack.
unsafe fn push_value(l: *mut ffi::lua_State, v: &Value) {
    // SAFETY: see the function contract.
    unsafe {
        match v {
            Value::Nil | Value::Other(_) => ffi::lua_pushnil(l),
            Value::Boolean(b) => ffi::lua_pushboolean(l, c_int::from(*b)),
            Value::Number(n) => ffi::lua_pushnumber(l, *n),
            Value::String(s) => push_str(l, s),
        }
    }
}

/// SAFETY: `l` is a live state and `idx` is valid.
unsafe fn read_value(l: *mut ffi::lua_State, idx: c_int) -> Value {
    // SAFETY: lua_tolstring runs only on strings, so no number-to-string GC step.
    unsafe {
        match ffi::lua_type(l, idx) {
            ffi::LUA_TNIL | ffi::LUA_TNONE => Value::Nil,
            ffi::LUA_TBOOLEAN => Value::Boolean(ffi::lua_toboolean(l, idx) != 0),
            ffi::LUA_TNUMBER => Value::Number(ffi::lua_tonumber(l, idx)),
            ffi::LUA_TSTRING => {
                Value::String(String::from_utf8_lossy(&string_bytes(l, idx)).into_owned())
            }
            ffi::LUA_TTABLE => Value::Other("table"),
            ffi::LUA_TFUNCTION => Value::Other("function"),
            ffi::LUA_TUSERDATA | ffi::LUA_TLIGHTUSERDATA => Value::Other("userdata"),
            ffi::LUA_TTHREAD => Value::Other("thread"),
            _ => Value::Other("unknown"),
        }
    }
}

/// Bytes of the string at `idx`.
///
/// SAFETY: `l` is a live state and the value at `idx` is a string (no conversion happens).
pub(crate) unsafe fn string_bytes(l: *mut ffi::lua_State, idx: c_int) -> Vec<u8> {
    let mut len = 0usize;
    // SAFETY: on a string, lua_tolstring converts nothing and allocates nothing.
    unsafe {
        let p = ffi::lua_tolstring(l, idx, &mut len);
        std::slice::from_raw_parts(p.cast::<u8>(), len).to_vec()
    }
}

/// Takes the error value off the top of the stack.
pub(crate) fn pop_error(l: *mut ffi::lua_State, status: c_int) -> LuaError {
    let kind = match status {
        ffi::LUA_ERRSYNTAX => LuaErrorKind::Syntax,
        ffi::LUA_ERRMEM => LuaErrorKind::Memory,
        ffi::LUA_ERRERR => LuaErrorKind::ErrorHandler,
        _ => LuaErrorKind::Runtime,
    };
    // SAFETY: after a failed pcall/load, the error value is on top of the stack.
    let text = unsafe {
        let text = match ffi::lua_type(l, -1) {
            ffi::LUA_TSTRING => String::from_utf8_lossy(&string_bytes(l, -1)).into_owned(),
            ffi::LUA_TNUMBER => format!("{}", ffi::lua_tonumber(l, -1)),
            t => format!(
                "(error object is a {} value)",
                std::ffi::CStr::from_ptr(ffi::lua_typename(l, t)).to_string_lossy()
            ),
        };
        ffi::lua_settop(l, -2);
        text
    };
    LuaError::from_lua_text(kind, text)
}

// Helpers for the other modules.

impl Lua {
    /// Pushes the value of registry slot `r`.
    pub(crate) fn push_ref(&self, r: c_int) {
        // SAFETY: rawgeti on the registry raises nothing; room is ensured by the caller.
        unsafe { ffi::lua_rawgeti(self.l(), ffi::LUA_REGISTRYINDEX, r) }
    }

    /// Pops the top value into a new registry slot.
    pub(crate) fn pop_ref(&self) -> c_int {
        // SAFETY: luaL_ref only does raw table operations (no GC step, no metamethods).
        unsafe { ffi::luaL_ref(self.l(), ffi::LUA_REGISTRYINDEX) }
    }

    /// Pushes `v`; needs 1 free slot.
    pub(crate) fn push_lua_value(&self, v: &LuaValue<'_>) {
        crate::value::push_lua_value(self, v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::result::Result;

    fn add(ctx: &mut CallContext<'_>) -> Result<c_int, String> {
        let a = ctx.arg_i32(1)?;
        let b = ctx.arg_i32(2)?;
        ctx.push(&Value::from(a + b));
        Ok(1)
    }

    fn fail(_: &mut CallContext<'_>) -> Result<c_int, String> {
        Err("rust says no".into())
    }

    #[test]
    fn lua_integer_is_32_bit() {
        assert_eq!(std::mem::size_of::<ffi::lua_Integer>(), 4);
    }

    #[test]
    fn exec_and_globals() {
        let lua = Lua::new().unwrap();
        lua.exec("x = 6 * 7; s = 'hi' .. x", "test").unwrap();
        assert_eq!(lua.get_global("x").as_i32(), Some(42));
        assert_eq!(lua.get_global("s"), Value::String("hi42".into()));
        lua.set_global("y", &Value::from(5)).unwrap();
        lua.exec("assert(y == 5)", "test").unwrap();
    }

    #[test]
    fn syntax_and_runtime_errors() {
        let lua = Lua::new().unwrap();
        assert_eq!(
            lua.exec("x = = 1", "bad").unwrap_err().kind,
            LuaErrorKind::Syntax
        );
        let e = lua.exec("error('boom')", "bad").unwrap_err();
        assert_eq!(e.kind, LuaErrorKind::Runtime);
        assert!(e.message.contains("boom"), "{e}");
    }

    #[test]
    fn rust_function_ok_and_error() {
        let lua = Lua::new().unwrap();
        lua.register("add", add).unwrap();
        lua.register("fail", fail).unwrap();
        let r = lua
            .call_global("add", &[Value::from(2), Value::from(40)])
            .unwrap();
        assert_eq!(r, vec![Value::from(42)]);
        // Argument and Rust errors both become Lua errors.
        let e = lua.call_global("add", &[Value::from(1)]).unwrap_err();
        assert!(e.message.contains("bad argument #2"), "{e}");
        let e = lua.call_global("fail", &[]).unwrap_err();
        assert_eq!(e.message, "rust says no");
        lua.exec(
            "local ok, msg = pcall(fail); assert(not ok and msg == 'rust says no')",
            "pcall",
        )
        .unwrap();
        // SAFETY: the state is alive.
        assert_eq!(unsafe { ffi::lua_gettop(lua.as_ptr()) }, 0);
    }

    /// Full GC cycle, with finalizers, at every check.
    const GC_EVERY_CHECK: &str = r#"
collectgarbage("setpause", 0)
collectgarbage("setstepmul", 0)
collectgarbage("collect")
"#;

    fn push_text(ctx: &mut CallContext<'_>) -> Result<c_int, String> {
        ctx.push(&Value::from("pushed"));
        Ok(1)
    }

    #[test]
    fn gc_metamethod_error_inside_rust_function() {
        let lua = Lua::new().unwrap();
        lua.register("push_text", push_text).unwrap();
        // Without the pause, this GC step would run __gc inside push_text's push.
        let code = format!(
            "{GC_EVERY_CHECK}
local function arm()
  local p = newproxy(true)
  getmetatable(p).__gc = function() error('gc boom') end
end
arm()
result = push_text()"
        );
        let e = lua.exec(&code, "gc").unwrap_err();
        // The finalizer ran later, under pcall.
        assert!(e.message.contains("gc boom"), "{e}");
        assert_eq!(lua.get_global("result"), Value::from("pushed"));
        lua.exec("assert(push_text() == 'pushed')", "after")
            .unwrap();
    }

    #[test]
    fn gc_metamethod_error_outside_pcall() {
        let lua = Lua::new().unwrap();
        let code = format!(
            "{GC_EVERY_CHECK}
holder = newproxy(true)
getmetatable(holder).__gc = function() error('gc boom') end"
        );
        lua.exec(&code, "gc").unwrap();
        // Without the pause, the next allocating call would run __gc outside pcall.
        lua.set_global("holder", &Value::Nil).unwrap();
        lua.set_global("s", &Value::from("text")).unwrap();
        assert_eq!(lua.get_global("s"), Value::from("text"));
        lua.register("push_text", push_text).unwrap();
        let e = lua
            .call_global("no_such_function", &[Value::from("arg")])
            .unwrap_err();
        assert_eq!(e.kind, LuaErrorKind::Api);
        // The deferred finalizer fires on the next Lua run under pcall.
        let e = lua.call_global("push_text", &[]).unwrap_err();
        assert!(e.message.contains("gc boom"), "{e}");
        assert_eq!(
            lua.call_global("push_text", &[]).unwrap(),
            vec![Value::from("pushed")]
        );
        lua.exec("x = 1", "after").unwrap();
    }

    #[test]
    fn gc_pause_keeps_stopped_gc_stopped() {
        let lua = Lua::new().unwrap();
        lua.register("push_text", push_text).unwrap();
        // The pause must restore the old threshold, not restart the collector.
        lua.exec(
            "collectgarbage('collect'); collectgarbage('stop')
             finalized = false
             do local p = newproxy(true); getmetatable(p).__gc = function() finalized = true end end
             for i = 1, 20000 do local s = push_text() .. i end",
            "stop",
        )
        .unwrap();
        for i in 0..1000 {
            lua.set_global("tmp", &Value::from(format!("v{i}").as_str()))
                .unwrap();
        }
        lua.exec(
            "assert(finalized == false)
             collectgarbage('restart'); collectgarbage('collect')
             assert(finalized == true)",
            "restart",
        )
        .unwrap();
    }

    fn claims_million(_: &mut CallContext<'_>) -> Result<c_int, String> {
        Ok(1_000_000)
    }

    fn pushes_one_claims_five(ctx: &mut CallContext<'_>) -> Result<c_int, String> {
        ctx.push(&Value::from(7));
        Ok(5)
    }

    fn claims_negative(_: &mut CallContext<'_>) -> Result<c_int, String> {
        Ok(-3)
    }

    #[test]
    fn result_count_is_clamped_to_pushed_values() {
        let lua = Lua::new().unwrap();
        lua.register("claims_million", claims_million).unwrap();
        lua.register("pushes_one_claims_five", pushes_one_claims_five)
            .unwrap();
        lua.register("claims_negative", claims_negative).unwrap();
        lua.exec(
            "assert(select('#', claims_million()) == 0)
             assert(select('#', claims_million(1, 2, 3)) == 0)
             assert(select('#', pushes_one_claims_five('a', 'b')) == 1)
             assert(pushes_one_claims_five('a', 'b') == 7)
             assert(select('#', claims_negative()) == 0)",
            "clamp",
        )
        .unwrap();
        assert_eq!(lua.call_global("claims_million", &[]).unwrap(), vec![]);
        assert_eq!(
            lua.call_global("pushes_one_claims_five", &[Value::from(1)])
                .unwrap(),
            vec![Value::from(7)]
        );
    }

    #[test]
    fn trampoline_upvalue_is_not_reachable_from_lua() {
        let lua = Lua::new().unwrap();
        lua.register("add", add).unwrap();
        lua.exec(
            "assert(debug.setupvalue(add, 1, io.stdout) == nil)
             assert(debug.getupvalue(add, 1) == nil)
             assert(add(1, 2) == 3)",
            "upvalue",
        )
        .unwrap();
    }

    #[test]
    fn missing_global_function() {
        let lua = Lua::new().unwrap();
        let e = lua.call_global("nope", &[]).unwrap_err();
        assert_eq!(e.kind, LuaErrorKind::Api);
    }
}
