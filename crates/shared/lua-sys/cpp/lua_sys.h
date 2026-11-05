// C++ side of lua-sys. None of these throw (P5): errors come back as codes, or abort.
#pragma once

#include <stddef.h>

extern "C" {
#include <lua.h>
}

extern "C" {

/// Rust function called via the trampoline. Returns the result count, or an error code with the
/// message string on top: -1 = Rust-made error (the caller's `file:line:` is prepended, like
/// `luaL_error`), -2 = message from Lua, raised unchanged.
typedef int (*lua_sys_rust_fn)(lua_State* L, void* data);

/// Releases a block's payload. `block` is the userdata block itself.
typedef void (*lua_sys_drop_fn)(void* block);

/// New state. Never returns NULL; aborts on out-of-memory.
lua_State* lua_sys_newstate(void);

/// Opens the standard libraries under pcall. Returns the status (0 on success).
int lua_sys_openlibs(lua_State* L);

/// lua_pushlstring with GC paused. Needs 1 free stack slot.
void lua_sys_pushlstring(lua_State* L, const char* s, size_t len);

/// Pushes a C closure calling `f(L, data)`, with GC paused. Needs 2 free stack slots.
void lua_sys_push_rust_function(lua_State* L, lua_sys_rust_fn f, void* data);

/// Like `lua_sys_push_rust_function`, and `drop(block)` runs when the closure is collected or the
/// state is closed (once; `block` points to the closure's userdata, which starts with the drop
/// function pointer).
void lua_sys_push_rust_closure(lua_State* L, lua_sys_rust_fn f, void* data, lua_sys_drop_fn drop);

/// New userdata of `size` bytes without a metatable, GC paused. The first pointer-sized word is
/// zeroed: it holds the `lua_sys_drop_fn` the `__gc` function (`lua_sys_gc_block`) runs once.
void* lua_sys_newuserdata(lua_State* L, size_t size);

/// `__gc` function for blocks laid out as above. Set it in the metatables of such userdata.
int lua_sys_gc_block(lua_State* L);

/// lua_createtable with GC paused. Needs 1 free slot.
void lua_sys_createtable(lua_State* L, int narr, int nrec);

/// lua_pushcclosure with GC paused (upvalues already on the stack). Needs 1 free slot.
void lua_sys_pushcclosure(lua_State* L, lua_CFunction f, int n);

/// lua_tolstring with GC paused (number conversion allocates).
const char* lua_sys_tolstring(lua_State* L, int idx, size_t* len);

/// lua_pcall that adds `debug.traceback` to string errors. `f` and its args are on the stack.
/// Needs `nargs + 3` free slots.
int lua_sys_pcall(lua_State* L, int nargs, int nresults);

/// `t[k]` with metamethods, under pcall. `k` on top is replaced by the value (or the error message).
/// Needs 4 free slots.
int lua_sys_gettable(lua_State* L, int idx);

/// `t[k] = v` with metamethods, under pcall. `k, v` on top are removed (an error message replaces
/// them). Needs 6 free slots.
int lua_sys_settable(lua_State* L, int idx);

/// `tostring(value at idx)` under pcall (honors `__tostring`); pushes the string or the message.
int lua_sys_tostring(lua_State* L, int idx);

/// Full garbage collection under pcall. Returns 0, or a status with the message on the stack.
int lua_sys_gc_collect(lua_State* L);

/// Arms (n > 0) or disarms (n == 0) the instruction limit for the current thread: after `n` VM
/// instructions the running script gets an error.
void lua_sys_arm_limit(lua_State* L, int n);

/// lua_rawset(L, idx) under pcall with GC paused. Returns 0, or a status with the message on the
/// stack. Needs 2 free slots.
int lua_sys_rawset(lua_State* L, int idx);

}  // extern "C"
