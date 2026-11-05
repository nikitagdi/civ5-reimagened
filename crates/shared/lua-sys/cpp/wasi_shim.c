/* WASI preview1 imports (architecture §8). Defining them all here stops wasm-ld from creating
 * `wasi_snapshot_preview1` imports. stdout/stderr are buffered for Rust, clocks are host-set,
 * random is xorshift (not cryptographic), and proc_exit traps. */
#include <stdint.h>
#include <stddef.h>
#include <string.h>

#define HIDDEN __attribute__((visibility("hidden")))
#define I32 int32_t
#define I64 int64_t
/* u64 params as int64, the rest as int32, as in wasi-libc. */
#define W(name) HIDDEN int32_t __imported_wasi_snapshot_preview1_##name

enum { E_SUCCESS = 0, E_BADF = 8, E_NOSYS = 52 };

/* Shared stdout/stderr buffer; overflow is dropped. */
#define OUT_CAP 65536
static char g_out[OUT_CAP];
static size_t g_out_len;

HIDDEN size_t lua_sys_wasi_take_output(char* dst, size_t cap) {
    size_t n = g_out_len < cap ? g_out_len : cap;
    memcpy(dst, g_out, n);
    memmove(g_out, g_out + n, g_out_len - n);
    g_out_len -= n;
    return n;
}

static uint64_t g_realtime_ns, g_monotonic_ns;
HIDDEN void lua_sys_wasi_set_clock(uint64_t realtime_ns, uint64_t monotonic_ns) {
    g_realtime_ns = realtime_ns;
    g_monotonic_ns = monotonic_ns;
}

static uint64_t g_rng = 0x9E3779B97F4A7C15ull;

W(args_get)(I32 a, I32 b) { (void)a; (void)b; return E_SUCCESS; }
W(args_sizes_get)(I32 argc, I32 bufsize) { *(uint32_t*)(uintptr_t)argc = 0; *(uint32_t*)(uintptr_t)bufsize = 0; return E_SUCCESS; }
W(environ_get)(I32 a, I32 b) { (void)a; (void)b; return E_SUCCESS; }
W(environ_sizes_get)(I32 cnt, I32 bufsize) { *(uint32_t*)(uintptr_t)cnt = 0; *(uint32_t*)(uintptr_t)bufsize = 0; return E_SUCCESS; }
W(clock_res_get)(I32 id, I32 ret) { (void)id; *(uint64_t*)(uintptr_t)ret = 1000; return E_SUCCESS; }
W(clock_time_get)(I32 id, I64 precision, I32 ret) {
    (void)precision;
    /* 0 realtime, 1 monotonic; CPU clocks map to monotonic */
    uint64_t t = id == 0 ? g_realtime_ns : g_monotonic_ns;
    g_monotonic_ns += 1000; /* strictly increasing even without host updates */
    *(uint64_t*)(uintptr_t)ret = t;
    return E_SUCCESS;
}
W(fd_write)(I32 fd, I32 iovs, I32 iovs_len, I32 nwritten) {
    if (fd != 1 && fd != 2) return E_BADF;
    const uint32_t* iov = (const uint32_t*)(uintptr_t)iovs;
    uint32_t total = 0;
    for (I32 i = 0; i < iovs_len; ++i) {
        const char* p = (const char*)(uintptr_t)iov[2 * i];
        uint32_t len = iov[2 * i + 1];
        size_t room = OUT_CAP - g_out_len;
        size_t n = len < room ? len : room;
        memcpy(g_out + g_out_len, p, n);
        g_out_len += n;
        total += len;
    }
    *(uint32_t*)(uintptr_t)nwritten = total;
    return E_SUCCESS;
}
W(fd_fdstat_get)(I32 fd, I32 ret) {
    if (fd < 0 || fd > 2) return E_BADF;
    uint8_t* st = (uint8_t*)(uintptr_t)ret;
    memset(st, 0, 24);
    st[0] = 2; /* character device: isatty() is true */
    return E_SUCCESS;
}
W(fd_close)(I32 fd) { (void)fd; return E_BADF; }
W(fd_prestat_get)(I32 fd, I32 ret) { (void)fd; (void)ret; return E_BADF; }
W(fd_prestat_dir_name)(I32 fd, I32 p, I32 l) { (void)fd; (void)p; (void)l; return E_BADF; }
W(fd_seek)(I32 fd, I64 off, I32 wh, I32 ret) { (void)fd; (void)off; (void)wh; (void)ret; return E_BADF; }
W(fd_read)(I32 fd, I32 a, I32 b, I32 c) { (void)fd; (void)a; (void)b; (void)c; return E_BADF; }
W(random_get)(I32 buf, I32 len) {
    uint8_t* p = (uint8_t*)(uintptr_t)buf;
    for (I32 i = 0; i < len; ++i) {
        g_rng ^= g_rng << 13; g_rng ^= g_rng >> 7; g_rng ^= g_rng << 17;
        p[i] = (uint8_t)g_rng;
    }
    return E_SUCCESS;
}
HIDDEN _Noreturn void __imported_wasi_snapshot_preview1_proc_exit(I32 code) { (void)code; __builtin_trap(); }
W(sched_yield)(void) { return E_SUCCESS; }
W(poll_oneoff)(I32 a, I32 b, I32 c, I32 d) { (void)a; (void)b; (void)c; (void)d; return E_NOSYS; }
/* unsupported */
W(fd_advise)(I32 a, I64 b, I64 c, I32 d) { (void)a; (void)b; (void)c; (void)d; return E_NOSYS; }
W(fd_allocate)(I32 a, I64 b, I64 c) { (void)a; (void)b; (void)c; return E_NOSYS; }
W(fd_datasync)(I32 a) { (void)a; return E_NOSYS; }
W(fd_fdstat_set_flags)(I32 a, I32 b) { (void)a; (void)b; return E_NOSYS; }
W(fd_fdstat_set_rights)(I32 a, I64 b, I64 c) { (void)a; (void)b; (void)c; return E_NOSYS; }
W(fd_filestat_get)(I32 a, I32 b) { (void)a; (void)b; return E_BADF; }
W(fd_filestat_set_size)(I32 a, I64 b) { (void)a; (void)b; return E_NOSYS; }
W(fd_filestat_set_times)(I32 a, I64 b, I64 c, I32 d) { (void)a; (void)b; (void)c; (void)d; return E_NOSYS; }
W(fd_pread)(I32 a, I32 b, I32 c, I64 d, I32 e) { (void)a; (void)b; (void)c; (void)d; (void)e; return E_NOSYS; }
W(fd_pwrite)(I32 a, I32 b, I32 c, I64 d, I32 e) { (void)a; (void)b; (void)c; (void)d; (void)e; return E_NOSYS; }
W(fd_readdir)(I32 a, I32 b, I32 c, I64 d, I32 e) { (void)a; (void)b; (void)c; (void)d; (void)e; return E_NOSYS; }
W(fd_renumber)(I32 a, I32 b) { (void)a; (void)b; return E_NOSYS; }
W(fd_sync)(I32 a) { (void)a; return E_NOSYS; }
W(fd_tell)(I32 a, I32 b) { (void)a; (void)b; return E_BADF; }
W(path_create_directory)(I32 a, I32 b, I32 c) { (void)a; (void)b; (void)c; return E_NOSYS; }
W(path_filestat_get)(I32 a, I32 b, I32 c, I32 d, I32 e) { (void)a; (void)b; (void)c; (void)d; (void)e; return E_NOSYS; }
W(path_filestat_set_times)(I32 a, I32 b, I32 c, I32 d, I64 e, I64 f, I32 g) { (void)a; (void)b; (void)c; (void)d; (void)e; (void)f; (void)g; return E_NOSYS; }
W(path_link)(I32 a, I32 b, I32 c, I32 d, I32 e, I32 f, I32 g) { (void)a; (void)b; (void)c; (void)d; (void)e; (void)f; (void)g; return E_NOSYS; }
W(path_open)(I32 a, I32 b, I32 c, I32 d, I32 e, I64 f, I64 g, I32 h, I32 i) { (void)a; (void)b; (void)c; (void)d; (void)e; (void)f; (void)g; (void)h; (void)i; return E_NOSYS; }
W(path_readlink)(I32 a, I32 b, I32 c, I32 d, I32 e, I32 f) { (void)a; (void)b; (void)c; (void)d; (void)e; (void)f; return E_NOSYS; }
W(path_remove_directory)(I32 a, I32 b, I32 c) { (void)a; (void)b; (void)c; return E_NOSYS; }
W(path_rename)(I32 a, I32 b, I32 c, I32 d, I32 e, I32 f) { (void)a; (void)b; (void)c; (void)d; (void)e; (void)f; return E_NOSYS; }
W(path_symlink)(I32 a, I32 b, I32 c, I32 d, I32 e) { (void)a; (void)b; (void)c; (void)d; (void)e; return E_NOSYS; }
W(path_unlink_file)(I32 a, I32 b, I32 c) { (void)a; (void)b; (void)c; return E_NOSYS; }
W(sock_accept)(I32 a, I32 b, I32 c) { (void)a; (void)b; (void)c; return E_NOSYS; }
W(sock_recv)(I32 a, I32 b, I32 c, I32 d, I32 e, I32 f) { (void)a; (void)b; (void)c; (void)d; (void)e; (void)f; return E_NOSYS; }
W(sock_send)(I32 a, I32 b, I32 c, I32 d, I32 e) { (void)a; (void)b; (void)c; (void)d; (void)e; return E_NOSYS; }
W(sock_shutdown)(I32 a, I32 b) { (void)a; (void)b; return E_NOSYS; }
