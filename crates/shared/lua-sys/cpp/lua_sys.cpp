// C++ side of lua-sys: state creation, the Rust trampoline, and calls safe outside pcall.
// Lua may run a GC step inside any allocating call, which calls `__gc` unprotected (P5).
// `GcPause` disables steps around such calls; the error then surfaces later, under pcall.
#include "lua_sys.h"

#include <cstdio>
#include <cstdlib>

#include "lua_prelude.h"

extern "C" {
#include <lauxlib.h>
#include <lualib.h>
// Internal header for global_State::GCthreshold; same sources, so the layout matches.
#include "lstate.h"
}

namespace {

// abort() on out-of-memory: a LUA_ERRMEM thrown from a push would cross Rust frames.
void* alloc_or_abort(void*, void* ptr, size_t, size_t nsize) {
    if (nsize == 0) {
        std::free(ptr);
        return nullptr;
    }
    void* p = std::realloc(ptr, nsize);
    if (p == nullptr) {
        std::fprintf(stderr, "lua-sys: out of memory (%zu bytes)\n", nsize);
        std::abort();
    }
    return p;
}

// Unprotected error: Lua would call exit(); abort with a message instead.
int panic_abort(lua_State* L) {
    const char* msg = lua_tostring(L, -1);
    std::fprintf(stderr, "lua-sys: unprotected Lua error: %s\n", msg ? msg : "(not a string)");
    std::abort();
}

// Raises the GC threshold; restoring the saved value is correct for nested pauses too.
class GcPause {
public:
    explicit GcPause(lua_State* L) : g_(G(L)), saved_(g_->GCthreshold) {
        g_->GCthreshold = MAX_LUMEM;
    }
    ~GcPause() { g_->GCthreshold = saved_; }
    GcPause(const GcPause&) = delete;
    GcPause& operator=(const GcPause&) = delete;

private:
    global_State* g_;
    lu_mem saved_;
};

// Scripts cannot replace this upvalue (debug.setupvalue ignores C upvalues in 5.1). `drop` comes
// first, since every userdata block that owns a payload starts with it (see lua_sys_gc_block).
struct RustFn {
    lua_sys_drop_fn drop;
    lua_sys_rust_fn f;
    void* data;
};

const char* const kGcMetatable = "lua_sys.gc_mt";
const char* const kTraceback = "lua_sys.traceback";
const char* const kHandler = "lua_sys.handler";
const char* const kGetTable = "lua_sys.gettable";
const char* const kSetTable = "lua_sys.settable";
const char* const kToString = "lua_sys.tostring";

int trampoline(lua_State* L) {
    const RustFn* rf = static_cast<const RustFn*>(lua_touserdata(L, lua_upvalueindex(1)));
    int n = rf->f(L, rf->data);
    if (n < 0) {
        if (n == -1) {
            // Prepend the caller's position, as luaL_error does.
            GcPause pause(L);
            luaL_where(L, 1);
            lua_insert(L, -2);
            lua_concat(L, 2);
        }
        // lua_error throws from this frame; only Lua and C++ frames sit above it.
        return lua_error(L);
    }
    return n;
}

// Error handler for lua_sys_pcall: appends the traceback to string errors. Other error objects
// pass through unchanged, except that a `__tostring` is applied.
int traceback_handler(lua_State* L) {
    if (!lua_isstring(L, 1)) {
        if (lua_isnoneornil(L, 1) || !luaL_callmeta(L, 1, "__tostring")) return 1;
        lua_replace(L, 1);
    }
    lua_getfield(L, LUA_REGISTRYINDEX, kTraceback);
    if (!lua_isfunction(L, -1)) {
        lua_pop(L, 1);
        return 1;
    }
    lua_pushvalue(L, 1);
    lua_pushinteger(L, 2);  // skip the handler itself
    lua_call(L, 2, 1);
    return 1;
}

int gettable_protected(lua_State* L) {  // args: t, k
    lua_gettable(L, 1);
    return 1;
}

int settable_protected(lua_State* L) {  // args: t, k, v
    lua_settable(L, 1);
    return 0;
}

int tostring_protected(lua_State* L) {  // arg: value; mirrors luaB_tostring
    if (luaL_callmeta(L, 1, "__tostring")) {
        if (!lua_isstring(L, -1)) luaL_error(L, "'__tostring' must return a string");
        return 1;
    }
    switch (lua_type(L, 1)) {
        case LUA_TNUMBER:
        case LUA_TSTRING:
            lua_pushvalue(L, 1);
            lua_tostring(L, -1);  // converts a number in place
            break;
        case LUA_TBOOLEAN:
            lua_pushstring(L, lua_toboolean(L, 1) ? "true" : "false");
            break;
        case LUA_TNIL:
            lua_pushliteral(L, "nil");
            break;
        default:
            lua_pushfstring(L, "%s: %p", luaL_typename(L, 1), lua_topointer(L, 1));
            break;
    }
    return 1;
}

void limit_hook(lua_State* L, lua_Debug*) {
    luaL_error(L, "script exceeded the instruction limit");
}

int openlibs_protected(lua_State* L) {
    luaL_openlibs(L);
    // Keep the original: scripts (and the sandbox) may replace `debug`.
    lua_getglobal(L, "debug");
    lua_getfield(L, -1, "traceback");
    lua_setfield(L, LUA_REGISTRYINDEX, kTraceback);
    lua_pop(L, 1);
    return 0;
}

// Registers a helper function in the registry; needs a fresh state with free stack.
void set_helper(lua_State* L, const char* key, lua_CFunction f) {
    lua_pushcclosure(L, f, 0);
    lua_setfield(L, LUA_REGISTRYINDEX, key);
}

// Args: table, key, value. Runs under lua_pcall.
int rawset_protected(lua_State* L) {
    lua_rawset(L, 1);
    return 0;
}

}  // namespace

extern "C" lua_State* lua_sys_newstate(void) {
    lua_State* L = lua_newstate(alloc_or_abort, nullptr);
    if (L == nullptr) {
        std::fprintf(stderr, "lua-sys: lua_newstate failed\n");
        std::abort();
    }
    lua_atpanic(L, panic_abort);
    // Fresh state, nothing can run __gc yet; the stack has LUA_MINSTACK free slots.
    lua_createtable(L, 0, 1);
    lua_pushcfunction(L, lua_sys_gc_block);
    lua_setfield(L, -2, "__gc");
    lua_setfield(L, LUA_REGISTRYINDEX, kGcMetatable);
    set_helper(L, kHandler, traceback_handler);
    set_helper(L, kGetTable, gettable_protected);
    set_helper(L, kSetTable, settable_protected);
    set_helper(L, kToString, tostring_protected);
    return L;
}

extern "C" int lua_sys_openlibs(lua_State* L) {
    return lua_cpcall(L, openlibs_protected, nullptr);
}

extern "C" void lua_sys_pushlstring(lua_State* L, const char* s, size_t len) {
    GcPause pause(L);
    lua_pushlstring(L, s, len);
}

extern "C" void lua_sys_push_rust_closure(lua_State* L, lua_sys_rust_fn f, void* data,
                                          lua_sys_drop_fn drop) {
    GcPause pause(L);
    RustFn* rf = static_cast<RustFn*>(lua_newuserdata(L, sizeof(RustFn)));
    rf->drop = nullptr;  // not droppable until fully set up
    rf->f = f;
    rf->data = data;
    rf->drop = drop;
    lua_getfield(L, LUA_REGISTRYINDEX, kGcMetatable);
    lua_setmetatable(L, -2);
    lua_pushcclosure(L, trampoline, 1);
}

extern "C" void lua_sys_push_rust_function(lua_State* L, lua_sys_rust_fn f, void* data) {
    lua_sys_push_rust_closure(L, f, data, nullptr);
}

extern "C" void* lua_sys_newuserdata(lua_State* L, size_t size) {
    GcPause pause(L);
    void* p = lua_newuserdata(L, size < sizeof(void*) ? sizeof(void*) : size);
    *static_cast<lua_sys_drop_fn*>(p) = nullptr;
    return p;
}

extern "C" int lua_sys_gc_block(lua_State* L) {
    lua_sys_drop_fn* block = static_cast<lua_sys_drop_fn*>(lua_touserdata(L, 1));
    if (block != nullptr && *block != nullptr) {
        lua_sys_drop_fn drop = *block;
        *block = nullptr;  // never twice
        drop(block);
    }
    return 0;
}

extern "C" void lua_sys_createtable(lua_State* L, int narr, int nrec) {
    GcPause pause(L);
    lua_createtable(L, narr, nrec);
}

extern "C" void lua_sys_pushcclosure(lua_State* L, lua_CFunction f, int n) {
    GcPause pause(L);
    lua_pushcclosure(L, f, n);
}

extern "C" const char* lua_sys_tolstring(lua_State* L, int idx, size_t* len) {
    GcPause pause(L);
    return lua_tolstring(L, idx, len);
}

extern "C" int lua_sys_pcall(lua_State* L, int nargs, int nresults) {
    int base = lua_gettop(L) - nargs;  // the function
    lua_getfield(L, LUA_REGISTRYINDEX, kHandler);
    lua_insert(L, base);
    int status = lua_pcall(L, nargs, nresults, base);
    lua_remove(L, base);
    return status;
}

extern "C" int lua_sys_gettable(lua_State* L, int idx) {
    if (idx < 0 && idx > LUA_REGISTRYINDEX) idx = lua_gettop(L) + idx + 1;
    // stack: k
    lua_getfield(L, LUA_REGISTRYINDEX, kGetTable);  // k, f
    lua_pushvalue(L, idx);                          // k, f, t
    lua_pushvalue(L, -3);                           // k, f, t, k
    int status = lua_sys_pcall(L, 2, 1);            // k, result
    lua_replace(L, -2);                             // result
    return status;
}

extern "C" int lua_sys_settable(lua_State* L, int idx) {
    if (idx < 0 && idx > LUA_REGISTRYINDEX) idx = lua_gettop(L) + idx + 1;
    // stack: k, v
    lua_getfield(L, LUA_REGISTRYINDEX, kSetTable);  // k, v, f
    lua_pushvalue(L, idx);                          // k, v, f, t
    lua_pushvalue(L, -4);                           // k, v, f, t, k
    lua_pushvalue(L, -4);                           // k, v, f, t, k, v
    int status = lua_sys_pcall(L, 3, 0);            // k, v [, message]
    if (status == 0) {
        lua_pop(L, 2);
    } else {
        lua_replace(L, -3);  // message over k
        lua_pop(L, 1);
    }
    return status;
}

extern "C" int lua_sys_tostring(lua_State* L, int idx) {
    if (idx < 0 && idx > LUA_REGISTRYINDEX) idx = lua_gettop(L) + idx + 1;
    lua_getfield(L, LUA_REGISTRYINDEX, kToString);
    lua_pushvalue(L, idx);
    return lua_sys_pcall(L, 1, 1);
}

extern "C" int lua_sys_gc_collect(lua_State* L) {
    return lua_cpcall(L, [](lua_State* S) -> int {
        lua_gc(S, LUA_GCCOLLECT, 0);
        return 0;
    }, nullptr);
}

extern "C" void lua_sys_arm_limit(lua_State* L, int n) {
    if (n > 0) {
        lua_sethook(L, limit_hook, LUA_MASKCOUNT, n);
    } else {
        lua_sethook(L, nullptr, 0, 0);
    }
}

extern "C" int lua_sys_rawset(lua_State* L, int idx) {
    // Make a relative index absolute before the stack shifts.
    if (idx < 0 && idx > LUA_REGISTRYINDEX) idx = lua_gettop(L) + idx + 1;
    // Paused for the whole call; "table overflow" is caught by lua_pcall.
    GcPause pause(L);
    lua_pushvalue(L, idx);                       // ..., k, v, t
    lua_pushcclosure(L, rawset_protected, 0);    // ..., k, v, t, f
    lua_insert(L, -4);                           // ..., f, k, v, t
    lua_insert(L, -3);                           // ..., f, t, k, v
    return lua_pcall(L, 3, 0, 0);
}
