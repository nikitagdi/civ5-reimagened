//! Functions: calling Lua from Rust, loading chunks, and Rust closures callable from Lua.

use std::any::Any;
use std::cell::RefCell;
use std::ffi::{c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

use crate::convert::{FromLuaMulti, ToLuaMulti};
use crate::error::{LuaError, LuaErrorKind, Result};
use crate::ffi;
use crate::state::{Inner, Lua, StackGuard, c_string, ensure_stack, pop_error, push_str};
use crate::value::{LuaRef, MultiValue, read_lua_value};
use crate::{Function, Table};

impl<'lua> Function<'lua> {
    /// Calls the function under pcall. A script error comes back as `Err` with its traceback
    /// (`LuaError::traceback`) and `file:line` (`LuaError::location`); the stack is restored.
    pub fn call<A, R>(&self, args: A) -> Result<R>
    where
        A: ToLuaMulti<'lua>,
        R: FromLuaMulti<'lua>,
    {
        let lua = self.0.lua;
        let args = args.to_lua_multi(lua)?;
        let res = self.call_multi(args)?;
        R::from_lua_multi(res, lua)
    }

    pub(crate) fn call_multi(&self, args: MultiValue<'lua>) -> Result<MultiValue<'lua>> {
        let lua = self.0.lua;
        lua.expire_registry_values();
        let l = lua.l();
        let _g = StackGuard::new(l);
        let nargs =
            c_int::try_from(args.0.len()).map_err(|_| LuaError::api("too many arguments"))?;
        // SAFETY: slots for the function, the arguments and the helper (3).
        let base = unsafe {
            ensure_stack(l, args.0.len() + 4);
            let base = ffi::lua_gettop(l);
            self.0.push();
            for a in &args.0 {
                lua.push_lua_value(a);
            }
            base
        };
        lua.pcall(nargs, ffi::LUA_MULTRET)?;
        // SAFETY: results lie above `base`; reading them raises nothing.
        let top = unsafe { ffi::lua_gettop(l) };
        let mut out = Vec::with_capacity((top - base).max(0) as usize);
        for i in base + 1..=top {
            // SAFETY: reading values; each may push one slot for its copy.
            unsafe { ensure_stack(l, 2) };
            out.push(read_lua_value(lua, i));
        }
        Ok(MultiValue(out))
    }

    /// Sets the global environment of the function (`setfenv`). False for functions that
    /// cannot have one.
    pub fn set_environment(&self, env: &Table<'lua>) -> bool {
        let l = self.0.lua.l();
        let _g = StackGuard::new(l);
        // SAFETY: setfenv pops the table and raises nothing.
        unsafe {
            ensure_stack(l, 2);
            self.0.push();
            env.0.push();
            ffi::lua_setfenv(l, -2) != 0
        }
    }

    /// The environment table (`getfenv`).
    pub fn environment(&self) -> Option<Table<'lua>> {
        let lua = self.0.lua;
        let l = lua.l();
        let _g = StackGuard::new(l);
        // SAFETY: getfenv pushes one table.
        unsafe {
            ensure_stack(l, 2);
            self.0.push();
            ffi::lua_getfenv(l, -1);
            if ffi::lua_type(l, -1) != ffi::LUA_TTABLE {
                return None;
            }
            Some(Table(LuaRef::from_top(lua)))
        }
    }
}

/// A chunk of source text to load, see [`Lua::load`].
pub struct Chunk<'lua, 'src> {
    lua: &'lua Lua,
    source: &'src [u8],
    name: Option<String>,
    env: Option<Table<'lua>>,
}

impl Lua {
    /// Starts loading a chunk. Binary chunks are refused (bytecode is not safe to run); a
    /// leading UTF-8 BOM is skipped.
    pub fn load<'lua, 'src, S>(&'lua self, source: &'src S) -> Chunk<'lua, 'src>
    where
        S: AsRef<[u8]> + ?Sized,
    {
        Chunk {
            lua: self,
            source: source.as_ref(),
            name: None,
            env: None,
        }
    }
}

impl<'lua> Chunk<'lua, '_> {
    /// Chunk name as in `luaL_loadbuffer`: `@path` shows as `path:line:`, `=text` as `text:line:`,
    /// anything else as `[string "text"]:line:`.
    pub fn set_name(mut self, name: &str) -> Self {
        self.name = Some(name.to_owned());
        self
    }

    /// The global environment of the loaded function.
    pub fn set_environment(mut self, env: Table<'lua>) -> Self {
        self.env = Some(env);
        self
    }

    pub fn into_function(self) -> Result<Function<'lua>> {
        let lua = self.lua;
        lua.expire_registry_values();
        let src = self
            .source
            .strip_prefix(b"\xEF\xBB\xBF")
            .unwrap_or(self.source);
        if src.first() == Some(&0x1b) {
            return Err(LuaError::new(
                LuaErrorKind::Syntax,
                "attempt to load a binary chunk (bytecode is not allowed)",
            ));
        }
        let name = match &self.name {
            Some(n) => c_string(n),
            None => {
                let head = String::from_utf8_lossy(&src[..src.len().min(80)]);
                c_string(head.lines().next().unwrap_or(""))
            }
        };
        let l = lua.l();
        let _g = StackGuard::new(l);
        // SAFETY: luaL_loadbuffer is protected; the buffer and the name outlive the call.
        let f = unsafe {
            ensure_stack(l, 2);
            let status = ffi::luaL_loadbuffer(l, src.as_ptr().cast(), src.len(), name.as_ptr());
            if status != ffi::LUA_OK {
                return Err(pop_error(l, status));
            }
            Function(LuaRef::from_top(lua))
        };
        if let Some(env) = &self.env {
            f.set_environment(env);
        }
        Ok(f)
    }

    /// Loads and runs the chunk, discarding its results.
    pub fn exec(self) -> Result<()> {
        self.into_function()?.call(())
    }

    /// Loads and runs the chunk, converting its results.
    pub fn eval<R: FromLuaMulti<'lua>>(self) -> Result<R> {
        self.into_function()?.call(())
    }
}

// Rust closures.

pub(crate) type Callback<'lua> =
    Box<dyn Fn(&'lua Lua, MultiValue<'lua>) -> Result<MultiValue<'lua>> + 'lua>;

struct CallbackData {
    inner: *const Inner,
    f: Callback<'static>,
}

/// Layout of the C++ `RustFn` block (see `lua_sys.cpp`).
#[repr(C)]
struct ClosureBlock {
    drop: Option<ffi::lua_sys_drop_fn>,
    f: ffi::lua_sys_rust_fn,
    data: *mut CallbackData,
}

impl Lua {
    /// A Rust closure as a Lua function. The closure keeps its state; it is dropped when the
    /// function is collected or the state closes. Arguments convert with [`FromLuaMulti`] (a
    /// failed conversion raises "bad argument #n"), results with [`ToLuaMulti`], an `Err` is raised
    /// as a Lua error with the caller's `file:line:`. A panic is raised as a Lua error too (not
    /// on wasm, where panics abort).
    pub fn create_function<'lua, A, R, F>(&'lua self, func: F) -> Function<'lua>
    where
        A: FromLuaMulti<'lua>,
        R: ToLuaMulti<'lua>,
        F: Fn(&'lua Lua, A) -> Result<R> + 'static,
    {
        self.create_callback(Box::new(move |lua, args| {
            func(lua, A::from_lua_multi(args, lua)?)?.to_lua_multi(lua)
        }))
    }

    /// Like [`Lua::create_function`] for `FnMut`. A call from inside itself fails with an error.
    pub fn create_function_mut<'lua, A, R, F>(&'lua self, func: F) -> Function<'lua>
    where
        A: FromLuaMulti<'lua>,
        R: ToLuaMulti<'lua>,
        F: FnMut(&'lua Lua, A) -> Result<R> + 'static,
    {
        let func = RefCell::new(func);
        self.create_function(move |lua, args: A| {
            let mut f = func
                .try_borrow_mut()
                .map_err(|_| LuaError::msg("mutable function called recursively"))?;
            (*f)(lua, args)
        })
    }

    pub(crate) fn create_callback<'lua>(&'lua self, cb: Callback<'lua>) -> Function<'lua> {
        self.expire_registry_values();
        // SAFETY: the 'lua lifetime is erased for storage. Handles inside the callback's
        // arguments and results are created and consumed within one call, with the `Lua` the
        // entry point builds; the closure itself is 'static, so it cannot hold `'lua` handles.
        let f: Callback<'static> = unsafe { std::mem::transmute(cb) };
        let data = Box::into_raw(Box::new(CallbackData {
            inner: Rc::as_ptr(&self.inner),
            f,
        }));
        let l = self.l();
        // SAFETY: the closure block owns `data` and frees it in drop_callback; 2 slots.
        unsafe {
            ensure_stack(l, 3);
            ffi::lua_sys_push_rust_closure(l, closure_entry, data.cast(), Some(drop_callback));
        }
        Function(LuaRef::from_top(self))
    }
}

unsafe extern "C" fn drop_callback(block: *mut c_void) {
    // SAFETY: `block` is a ClosureBlock made by create_callback; `data` is still owned by it.
    let data = unsafe { Box::from_raw((*block.cast::<ClosureBlock>()).data) };
    // A panicking Drop must not unwind into C++.
    let _ = catch_unwind(AssertUnwindSafe(|| drop(data)));
}

fn panic_text(p: &Box<dyn Any + Send>) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        (*s).to_owned()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_owned()
    }
}

unsafe extern "C" fn closure_entry(l: *mut ffi::lua_State, data: *mut c_void) -> c_int {
    // SAFETY: `data` is the CallbackData of a live closure block.
    let data = unsafe { &*data.cast::<CallbackData>() };
    // SAFETY: Inner outlives its closures; reading the flag needs no strong reference.
    if unsafe { (*data.inner).closing.get() } {
        // SAFETY: pushing an error message.
        unsafe {
            ensure_stack(l, 1);
            push_str(l, "Lua state is closing");
        }
        return ffi::RUST_ERROR;
    }
    // SAFETY: the state is alive (we are running in it) and not closing, so the count is >= 1.
    let lua = unsafe {
        Rc::increment_strong_count(data.inner);
        Lua::from_inner(Rc::from_raw(data.inner))
    };
    let prev = lua.inner.cur.replace(l);
    // SAFETY: the callback type erased 'lua to 'static; every handle made here is dropped before
    // `lua` is (they live inside this call: arguments, results, closure locals).
    let lua_ref: &'static Lua = unsafe { std::mem::transmute::<&Lua, &'static Lua>(&lua) };
    let outcome = catch_unwind(AssertUnwindSafe(|| -> Result<c_int> {
        // SAFETY: arguments are the values on the call's stack.
        let nargs = unsafe { ffi::lua_gettop(l) };
        let mut args = Vec::with_capacity(nargs.max(0) as usize);
        for i in 1..=nargs {
            // SAFETY: valid argument index; one scratch slot for a handle's copy.
            unsafe { ensure_stack(l, 2) };
            args.push(read_lua_value(lua_ref, i));
        }
        let res = (data.f)(lua_ref, MultiValue(args))?;
        // SAFETY: room for the results, then plain pushes.
        unsafe { ensure_stack(l, res.0.len() + 1) };
        for v in &res.0 {
            lua.push_lua_value(v);
        }
        Ok(res.0.len() as c_int)
    }));
    lua.inner.cur.set(prev);
    let (code, text) = match outcome {
        Ok(Ok(n)) => return n,
        Ok(Err(e)) => {
            // Errors made by Lua keep their own position; Rust-made ones get the caller's.
            let code = match e.kind {
                LuaErrorKind::Runtime | LuaErrorKind::Syntax | LuaErrorKind::ErrorHandler => {
                    ffi::RUST_ERROR_RAW
                }
                _ => ffi::RUST_ERROR,
            };
            (code, e.lua_text())
        }
        Err(p) => (ffi::RUST_ERROR, format!("Rust panic: {}", panic_text(&p))),
    };
    // SAFETY: pushing the message for the trampoline's lua_error.
    unsafe {
        ensure_stack(l, 1);
        push_str(l, &text);
    }
    code
}
