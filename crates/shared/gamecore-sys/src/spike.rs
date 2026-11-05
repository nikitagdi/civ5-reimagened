//! Safe wrappers for the Rust ↔ C++ ↔ Lua spike.

use std::ffi::{CStr, CString, c_char};

use lua_sys::Lua;

use crate::ffi;

const MSG_CAP: usize = 256;

fn msg_from(buf: &[c_char; MSG_CAP]) -> String {
    // SAFETY: C++ writes a NUL within cap.
    unsafe { CStr::from_ptr(buf.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

/// Constructor run count (expect 1).
pub fn ctor_count() -> i32 {
    lua_sys::init_cpp_runtime();
    // SAFETY: reads a static.
    unsafe { ffi::gc_spike_ctor_count() }
}

/// Global object fields were set by its constructor.
pub fn global_ok() -> bool {
    lua_sys::init_cpp_runtime();
    // SAFETY: reads the global object.
    unsafe { ffi::gc_spike_global_ok() != 0 }
}

/// Throws and catches in C++; `Ok(what)` if caught.
pub fn throw_catch(n: i32) -> Result<String, String> {
    lua_sys::init_cpp_runtime();
    let mut buf = [0 as c_char; MSG_CAP];
    // SAFETY: buf is MSG_CAP bytes; C++ catches everything.
    let ok = unsafe { ffi::gc_spike_throw_catch(n, buf.as_mut_ptr(), MSG_CAP) };
    let msg = msg_from(&buf);
    if ok != 0 { Ok(msg) } else { Err(msg) }
}

pub fn stl() -> Result<String, String> {
    lua_sys::init_cpp_runtime();
    let mut buf = [0 as c_char; MSG_CAP];
    // SAFETY: buf is MSG_CAP bytes; C++ catches everything.
    let ok = unsafe { ffi::gc_spike_stl(buf.as_mut_ptr(), MSG_CAP) };
    let msg = msg_from(&buf);
    if ok != 0 { Ok(msg) } else { Err(msg) }
}

pub fn mix(x: i32) -> i32 {
    lua_sys::init_cpp_runtime();
    // SAFETY: pure function, no pointers.
    unsafe { ffi::gc_spike_mix(x) }
}

pub fn lua_register(lua: &Lua) -> Result<(), i32> {
    // SAFETY: Lua state alive; registration runs in lua_cpcall.
    let status = unsafe { ffi::gc_spike_lua_register(lua.as_ptr()) };
    if status == 0 { Ok(()) } else { Err(status) }
}

/// Calls Lua global `func(x)` via C++ `lua_pcall`.
pub fn lua_call(lua: &Lua, func: &str, x: i32) -> Result<i32, String> {
    let func = CString::new(func).map_err(|_| "function name contains NUL".to_string())?;
    let mut out = 0i32;
    let mut buf = [0 as c_char; MSG_CAP];
    // SAFETY: state alive; pointers valid; C++ catches Lua errors.
    let ok = unsafe {
        ffi::gc_spike_lua_call(
            lua.as_ptr(),
            func.as_ptr(),
            x,
            &mut out,
            buf.as_mut_ptr(),
            MSG_CAP,
        )
    };
    if ok != 0 {
        Ok(out)
    } else {
        Err(msg_from(&buf))
    }
}

/// Opens an in-memory SQLite database, runs a query, and returns the result text.
pub fn sqlite() -> Result<String, String> {
    use std::ptr;

    use sqlite_sys as sq;

    let mut db = ptr::null_mut();
    // SAFETY: valid out pointer, NUL-terminated name.
    let rc = unsafe {
        sq::sqlite3_open_v2(
            c":memory:".as_ptr(),
            &mut db,
            sq::SQLITE_OPEN_READWRITE | sq::SQLITE_OPEN_CREATE,
            ptr::null(),
        )
    };
    if rc != sq::SQLITE_OK {
        return Err(format!("sqlite3_open_v2: {rc}"));
    }
    let mut stmt = ptr::null_mut();
    // SAFETY: db is open; the SQL is NUL-terminated (n_byte = -1).
    let rc = unsafe {
        sq::sqlite3_prepare_v2(
            db,
            c"SELECT sqlite_version(), 6 * 7".as_ptr(),
            -1,
            &mut stmt,
            ptr::null_mut(),
        )
    };
    let result = if rc != sq::SQLITE_OK {
        Err(format!("sqlite3_prepare_v2: {rc}"))
    } else {
        // SAFETY: stmt is live until finalize; column text is valid until the next step.
        unsafe {
            let result = if sq::sqlite3_step(stmt) == sq::SQLITE_ROW
                && sq::sqlite3_column_int(stmt, 1) == 42
            {
                let v = CStr::from_ptr(sq::sqlite3_column_text(stmt, 0).cast());
                Ok(format!("sqlite {} :memory:", v.to_string_lossy()))
            } else {
                Err("unexpected query result".to_string())
            };
            sq::sqlite3_finalize(stmt);
            result
        }
    };
    // SAFETY: db was opened above and its statement is finalized.
    unsafe { sq::sqlite3_close(db) };
    result
}
