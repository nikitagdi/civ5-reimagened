// Rust <-> C++ <-> Lua spike, C++03.
#include "gc_spike.h"

#include <cstdio>
#include <cstring>
#include <map>
#include <sstream>
#include <stdexcept>
#include <string>
#include <vector>

extern "C" {
#include <lua.h>
#include <lualib.h>
#include <lauxlib.h>
}

namespace {

// Zero-initialized: valid before the constructor runs.
int g_ctor_count = 0;

struct SpikeGlobal {
    int magic;
    std::string name;
    std::vector<int> primes;
    SpikeGlobal() : magic(0x5EED), name("spike-global") {
        primes.push_back(2);
        primes.push_back(3);
        primes.push_back(5);
        ++g_ctor_count;
    }
};

SpikeGlobal g_spike_global;

void copy_out(const std::string& s, char* dst, size_t cap) {
    if (!dst || cap == 0) return;
    size_t n = s.size() < cap - 1 ? s.size() : cap - 1;
    std::memcpy(dst, s.data(), n);
    dst[n] = '\0';
}

std::string int_str(int x) {
    std::ostringstream os;
    os << x;
    return os.str();
}

void deep_throw(int depth, int n) {
    std::vector<std::string> keep(1, "frame " + int_str(depth));
    if (depth == 0) throw std::runtime_error("spike error " + int_str(n));
    deep_throw(depth - 1, n);
}

int spike_cpp_raise(lua_State* L) {
    int n = static_cast<int>(luaL_checkinteger(L, 1));
    return luaL_error(L, "cpp raised %d", n);
}

int register_protected(lua_State* L) {
    lua_register(L, "spike_cpp_raise", spike_cpp_raise);
    return 0;
}

}  // namespace

extern "C" int32_t gc_spike_ctor_count(void) {
    return g_ctor_count;
}

extern "C" int32_t gc_spike_global_ok(void) {
    return g_spike_global.magic == 0x5EED && g_spike_global.name == "spike-global" &&
           g_spike_global.primes.size() == 3 && g_spike_global.primes[2] == 5;
}

extern "C" int32_t gc_spike_throw_catch(int32_t n, char* msg, size_t cap) {
    try {
        deep_throw(5, n);
    } catch (const std::runtime_error& e) {
        copy_out(e.what(), msg, cap);
        return 1;
    } catch (...) {
        copy_out("caught unknown exception", msg, cap);
        return 0;
    }
    copy_out("no exception", msg, cap);
    return 0;
}

extern "C" int32_t gc_spike_stl(char* msg, size_t cap) {
    try {
        std::map<std::string, std::vector<int> > m;
        for (int i = 0; i < 100; ++i) m["key" + int_str(i % 10)].push_back(i);
        std::string joined;
        for (std::map<std::string, std::vector<int> >::const_iterator it = m.begin(); it != m.end(); ++it)
            joined += it->first + ":" + int_str(static_cast<int>(it->second.size())) + ";";
        const std::string expected =
            "key0:10;key1:10;key2:10;key3:10;key4:10;key5:10;key6:10;key7:10;key8:10;key9:10;";
        if (joined != expected) {
            copy_out("map/vector/string mismatch: " + joined, msg, cap);
            return 0;
        }
        copy_out("map of 10 vectors x 10", msg, cap);
        return 1;
    } catch (...) {
        copy_out("exception in stl check", msg, cap);
        return 0;
    }
}

extern "C" int32_t gc_spike_mix(int32_t x) {
    return x * 3 + 1;
}

extern "C" int32_t gc_spike_lua_register(lua_State* L) {
    try {
        return lua_cpcall(L, register_protected, 0);
    } catch (...) {
        return -1;
    }
}

extern "C" int32_t gc_spike_lua_call(lua_State* L, const char* fn, int32_t x, int32_t* out, char* err, size_t cap) {
    try {
        int top = lua_gettop(L);
        lua_getglobal(L, fn);
        lua_pushinteger(L, x);
        if (lua_pcall(L, 1, 1, 0) != 0) {
            const char* m = lua_tostring(L, -1);
            copy_out(m ? m : "(non-string error)", err, cap);
            lua_settop(L, top);
            return 0;
        }
        if (!lua_isnumber(L, -1)) {
            copy_out(std::string(fn) + " returned a non-number", err, cap);
            lua_settop(L, top);
            return 0;
        }
        *out = static_cast<int32_t>(lua_tointeger(L, -1));
        lua_settop(L, top);
        return 1;
    } catch (...) {
        copy_out("exception escaped Lua", err, cap);
        return 0;
    }
}
