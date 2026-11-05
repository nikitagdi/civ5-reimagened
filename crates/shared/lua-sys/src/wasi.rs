//! Bindings to the WASI shim. Native builds are no-ops.

/// Returns and clears buffered C/C++ stdout and stderr (wasm only).
pub fn take_output() -> String {
    #[cfg(target_arch = "wasm32")]
    {
        crate::init_cpp_runtime();
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            // SAFETY: `buf` is valid for `buf.len()` bytes; the shim writes at most that many.
            let n = unsafe { crate::ffi::lua_sys_wasi_take_output(buf.as_mut_ptr(), buf.len()) };
            out.extend_from_slice(&buf[..n]);
            if n < buf.len() {
                break;
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }
    #[cfg(not(target_arch = "wasm32"))]
    String::new()
}

/// Sets the wasm clocks (ns: wall time since the Unix epoch, and monotonic). The host must call
/// this, since `shared` cannot reach JS time. Until then `os.clock()` advances 1 µs per shim call.
pub fn set_clock(realtime_ns: u64, monotonic_ns: u64) {
    #[cfg(target_arch = "wasm32")]
    {
        crate::init_cpp_runtime();
        // SAFETY: the shim only stores two u64 values.
        unsafe { crate::ffi::lua_sys_wasi_set_clock(realtime_ns, monotonic_ns) }
    }
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (realtime_ns, monotonic_ns);
}
