/* Rust<->C++<->Lua spike, used by selftest only. */
#ifndef GC_SPIKE_H
#define GC_SPIKE_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

struct lua_State;

int32_t gc_spike_ctor_count(void);
int32_t gc_spike_global_ok(void);
int32_t gc_spike_throw_catch(int32_t n, char* msg, size_t cap);
int32_t gc_spike_stl(char* msg, size_t cap);
int32_t gc_spike_mix(int32_t x);
int32_t gc_spike_lua_register(struct lua_State* L);
int32_t gc_spike_lua_call(struct lua_State* L, const char* fn, int32_t x, int32_t* out, char* err, size_t cap);

#ifdef __cplusplus
}
#endif

#endif /* GC_SPIKE_H */
