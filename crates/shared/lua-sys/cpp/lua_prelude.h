// Standard headers for the Lua sources. Included before `extern "C"`, since C++ headers cannot sit inside it.
#pragma once

#include <assert.h>
#include <ctype.h>
#include <errno.h>
#include <float.h>
#include <limits.h>
#include <locale.h>
#include <math.h>
#include <setjmp.h>
#include <stdarg.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#if defined(__wasi__)
// wasi-libc lacks tmpnam/tmpfile/system. Stubs fail cleanly instead of becoming `env` imports.
#define L_tmpnam 32
static inline char* lua_sys_no_tmpnam(char*) { return NULL; }
static inline FILE* lua_sys_no_tmpfile(void) { return NULL; }
static inline int lua_sys_no_system(const char* cmd) { return cmd == NULL ? 0 : -1; }
#define tmpnam(buf) lua_sys_no_tmpnam(buf)
#define tmpfile() lua_sys_no_tmpfile()
#define system(cmd) lua_sys_no_system(cmd)
#endif
