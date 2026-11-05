//! Raw Lua 5.1 FFI, with `luaconf.h` types from Lekmod (`LEKMOD_LUA_INTEGER=int`).
//!
//! Allocating calls can run `__gc` unprotected, and `lua_rawset` can overflow. From Rust, use the
//! `lua_sys_*` variants instead. Metamethod-triggering calls must run inside pcall.

#![allow(non_camel_case_types, non_snake_case)]

use std::ffi::{c_char, c_int, c_void};

/// Opaque Lua state.
#[repr(C)]
pub struct lua_State {
    _private: [u8; 0],
}

pub type lua_Integer = c_int;
pub type lua_Number = f64;
pub type lua_CFunction = unsafe extern "C" fn(L: *mut lua_State) -> c_int;
/// Rust function behind the trampoline: result count, or -1 with the error message on top.
pub type lua_sys_rust_fn = unsafe extern "C" fn(L: *mut lua_State, data: *mut c_void) -> c_int;
/// Releases the payload of a userdata block; `block` is the block itself (see `lua_sys_newuserdata`).
pub type lua_sys_drop_fn = unsafe extern "C" fn(block: *mut c_void);

/// Result code of a `lua_sys_rust_fn`: Rust-made error, the caller's position is prepended.
pub const RUST_ERROR: c_int = -1;
/// Result code of a `lua_sys_rust_fn`: the message came from Lua and is raised unchanged.
pub const RUST_ERROR_RAW: c_int = -2;

pub const LUA_REGISTRYINDEX: c_int = -10000;
pub const LUA_ENVIRONINDEX: c_int = -10001;
pub const LUA_GLOBALSINDEX: c_int = -10002;
pub const LUA_MULTRET: c_int = -1;
pub const LUA_GCCOUNT: c_int = 3;
pub const LUA_NOREF: c_int = -2;
pub const LUA_REFNIL: c_int = -1;

pub const LUA_OK: c_int = 0;
pub const LUA_YIELD: c_int = 1;
pub const LUA_ERRRUN: c_int = 2;
pub const LUA_ERRSYNTAX: c_int = 3;
pub const LUA_ERRMEM: c_int = 4;
pub const LUA_ERRERR: c_int = 5;

pub const LUA_TNONE: c_int = -1;
pub const LUA_TNIL: c_int = 0;
pub const LUA_TBOOLEAN: c_int = 1;
pub const LUA_TLIGHTUSERDATA: c_int = 2;
pub const LUA_TNUMBER: c_int = 3;
pub const LUA_TSTRING: c_int = 4;
pub const LUA_TTABLE: c_int = 5;
pub const LUA_TFUNCTION: c_int = 6;
pub const LUA_TUSERDATA: c_int = 7;
pub const LUA_TTHREAD: c_int = 8;

unsafe extern "C" {
    // State (cpp/lua_sys.cpp). `lua_sys_pushlstring` does not throw; `lua_sys_rawset` needs 2 free slots.
    pub fn lua_sys_newstate() -> *mut lua_State;
    pub fn lua_sys_openlibs(L: *mut lua_State) -> c_int;
    pub fn lua_sys_push_rust_function(L: *mut lua_State, f: lua_sys_rust_fn, data: *mut c_void);
    pub fn lua_sys_pushlstring(L: *mut lua_State, s: *const c_char, len: usize);
    pub fn lua_sys_rawset(L: *mut lua_State, idx: c_int) -> c_int;
    pub fn lua_sys_push_rust_closure(
        L: *mut lua_State,
        f: lua_sys_rust_fn,
        data: *mut c_void,
        drop: Option<lua_sys_drop_fn>,
    );
    /// Userdata of `size` bytes, no metatable; the first word is the (zeroed) drop function.
    pub fn lua_sys_newuserdata(L: *mut lua_State, size: usize) -> *mut c_void;
    /// `__gc` function for blocks made by `lua_sys_newuserdata` and the closures above.
    pub fn lua_sys_gc_block(L: *mut lua_State) -> c_int;
    pub fn lua_sys_createtable(L: *mut lua_State, narr: c_int, nrec: c_int);
    pub fn lua_sys_pushcclosure(L: *mut lua_State, f: lua_CFunction, n: c_int);
    pub fn lua_sys_tolstring(L: *mut lua_State, idx: c_int, len: *mut usize) -> *const c_char;
    /// `lua_pcall` with a traceback handler; function and arguments on the stack.
    pub fn lua_sys_pcall(L: *mut lua_State, nargs: c_int, nresults: c_int) -> c_int;
    /// Key on top is replaced by `t[key]` or the error message.
    pub fn lua_sys_gettable(L: *mut lua_State, idx: c_int) -> c_int;
    /// Key and value on top are removed; an error message takes their place.
    pub fn lua_sys_settable(L: *mut lua_State, idx: c_int) -> c_int;
    /// Pushes `tostring(value)` or the error message.
    pub fn lua_sys_tostring(L: *mut lua_State, idx: c_int) -> c_int;
    pub fn lua_sys_gc_collect(L: *mut lua_State) -> c_int;
    pub fn lua_sys_arm_limit(L: *mut lua_State, n: c_int);
    pub fn lua_close(L: *mut lua_State);

    pub fn lua_gc(L: *mut lua_State, what: c_int, data: c_int) -> c_int;
    pub fn lua_gettop(L: *mut lua_State) -> c_int;
    pub fn lua_settop(L: *mut lua_State, idx: c_int);
    pub fn lua_pushvalue(L: *mut lua_State, idx: c_int);
    pub fn lua_checkstack(L: *mut lua_State, sz: c_int) -> c_int;

    pub fn lua_type(L: *mut lua_State, idx: c_int) -> c_int;
    pub fn lua_typename(L: *mut lua_State, tp: c_int) -> *const c_char;
    pub fn lua_tonumber(L: *mut lua_State, idx: c_int) -> lua_Number;
    pub fn lua_tointeger(L: *mut lua_State, idx: c_int) -> lua_Integer;
    pub fn lua_toboolean(L: *mut lua_State, idx: c_int) -> c_int;
    pub fn lua_tolstring(L: *mut lua_State, idx: c_int, len: *mut usize) -> *const c_char;
    pub fn lua_touserdata(L: *mut lua_State, idx: c_int) -> *mut c_void;

    pub fn lua_pushnil(L: *mut lua_State);
    pub fn lua_pushnumber(L: *mut lua_State, n: lua_Number);
    pub fn lua_pushinteger(L: *mut lua_State, n: lua_Integer);
    /// May run a GC step (and `__gc`): from Rust outside pcall, use `lua_sys_pushlstring`.
    pub fn lua_pushlstring(L: *mut lua_State, s: *const c_char, len: usize);
    pub fn lua_pushboolean(L: *mut lua_State, b: c_int);
    pub fn lua_pushlightuserdata(L: *mut lua_State, p: *mut c_void);
    pub fn lua_pushcclosure(L: *mut lua_State, f: lua_CFunction, n: c_int);

    pub fn lua_createtable(L: *mut lua_State, narr: c_int, nrec: c_int);
    pub fn lua_insert(L: *mut lua_State, idx: c_int);
    pub fn lua_remove(L: *mut lua_State, idx: c_int);
    pub fn lua_replace(L: *mut lua_State, idx: c_int);
    pub fn lua_objlen(L: *mut lua_State, idx: c_int) -> usize;
    pub fn lua_rawequal(L: *mut lua_State, a: c_int, b: c_int) -> c_int;
    pub fn lua_topointer(L: *mut lua_State, idx: c_int) -> *const c_void;
    pub fn lua_next(L: *mut lua_State, idx: c_int) -> c_int;
    pub fn lua_getmetatable(L: *mut lua_State, idx: c_int) -> c_int;
    pub fn lua_setmetatable(L: *mut lua_State, idx: c_int) -> c_int;
    pub fn lua_getfenv(L: *mut lua_State, idx: c_int);
    pub fn lua_setfenv(L: *mut lua_State, idx: c_int) -> c_int;
    pub fn lua_rawgeti(L: *mut lua_State, idx: c_int, n: c_int);
    pub fn lua_rawseti(L: *mut lua_State, idx: c_int, n: c_int);
    pub fn luaL_ref(L: *mut lua_State, t: c_int) -> c_int;
    pub fn luaL_unref(L: *mut lua_State, t: c_int, r: c_int);
    pub fn lua_rawget(L: *mut lua_State, idx: c_int);
    pub fn lua_rawset(L: *mut lua_State, idx: c_int);
    pub fn lua_getfield(L: *mut lua_State, idx: c_int, k: *const c_char);
    pub fn lua_setfield(L: *mut lua_State, idx: c_int, k: *const c_char);

    pub fn lua_pcall(L: *mut lua_State, nargs: c_int, nresults: c_int, errfunc: c_int) -> c_int;
    pub fn lua_cpcall(L: *mut lua_State, f: lua_CFunction, ud: *mut c_void) -> c_int;
    pub fn lua_error(L: *mut lua_State) -> c_int;
    pub fn luaL_loadbuffer(
        L: *mut lua_State,
        buff: *const c_char,
        sz: usize,
        name: *const c_char,
    ) -> c_int;
}

/// Pseudo-index of upvalue `i` of a C closure.
pub const fn lua_upvalueindex(i: c_int) -> c_int {
    LUA_GLOBALSINDEX - i
}

// WASI shim (cpp/wasi_shim.c), wasm only.
#[cfg(target_arch = "wasm32")]
unsafe extern "C" {
    pub fn lua_sys_wasi_take_output(dst: *mut u8, cap: usize) -> usize;
    pub fn lua_sys_wasi_set_clock(realtime_ns: u64, monotonic_ns: u64);
    /// C++ constructors, synthesized by wasm-ld. Call exactly once; see [`crate::init_cpp_runtime`].
    pub fn __wasm_call_ctors();
}
