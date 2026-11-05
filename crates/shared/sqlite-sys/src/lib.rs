//! Raw FFI to the vendored SQLite (C API, `third_party/sqlite-*`). Only what Rust code calls is declared.

use std::ffi::{c_char, c_int, c_void};

/// Opaque `sqlite3` connection.
#[repr(C)]
pub struct sqlite3 {
    _private: [u8; 0],
}

/// Opaque `sqlite3_stmt` prepared statement.
#[repr(C)]
pub struct sqlite3_stmt {
    _private: [u8; 0],
}

pub const SQLITE_OK: c_int = 0;
pub const SQLITE_ROW: c_int = 100;
pub const SQLITE_DONE: c_int = 101;
pub const SQLITE_OPEN_READONLY: c_int = 0x1;
pub const SQLITE_OPEN_READWRITE: c_int = 0x2;
pub const SQLITE_OPEN_CREATE: c_int = 0x4;

unsafe extern "C" {
    pub fn sqlite3_libversion() -> *const c_char;
    pub fn sqlite3_open_v2(
        filename: *const c_char,
        db: *mut *mut sqlite3,
        flags: c_int,
        vfs: *const c_char,
    ) -> c_int;
    pub fn sqlite3_close(db: *mut sqlite3) -> c_int;
    pub fn sqlite3_errmsg(db: *mut sqlite3) -> *const c_char;
    pub fn sqlite3_exec(
        db: *mut sqlite3,
        sql: *const c_char,
        callback: Option<
            unsafe extern "C" fn(*mut c_void, c_int, *mut *mut c_char, *mut *mut c_char) -> c_int,
        >,
        arg: *mut c_void,
        errmsg: *mut *mut c_char,
    ) -> c_int;
    pub fn sqlite3_prepare_v2(
        db: *mut sqlite3,
        sql: *const c_char,
        n_byte: c_int,
        stmt: *mut *mut sqlite3_stmt,
        tail: *mut *const c_char,
    ) -> c_int;
    pub fn sqlite3_step(stmt: *mut sqlite3_stmt) -> c_int;
    pub fn sqlite3_column_int(stmt: *mut sqlite3_stmt, col: c_int) -> c_int;
    pub fn sqlite3_column_text(stmt: *mut sqlite3_stmt, col: c_int) -> *const u8;
    pub fn sqlite3_finalize(stmt: *mut sqlite3_stmt) -> c_int;
}

#[cfg(test)]
mod tests {
    use std::ffi::{CStr, CString};
    use std::ptr;

    use super::*;

    fn prepare(db: *mut sqlite3, sql: &str) -> *mut sqlite3_stmt {
        let sql = CString::new(sql).unwrap();
        let mut stmt = ptr::null_mut();
        // SAFETY: db is open, sql NUL-terminated (n_byte = -1).
        let rc = unsafe { sqlite3_prepare_v2(db, sql.as_ptr(), -1, &mut stmt, ptr::null_mut()) };
        assert_eq!(rc, SQLITE_OK);
        stmt
    }

    #[test]
    fn memory_database_round_trip() {
        let name = CString::new(":memory:").unwrap();
        let mut db = ptr::null_mut();
        // SAFETY: valid out pointer and NUL-terminated name.
        let rc = unsafe {
            sqlite3_open_v2(
                name.as_ptr(),
                &mut db,
                SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE,
                ptr::null(),
            )
        };
        assert_eq!(rc, SQLITE_OK);

        let ddl = CString::new(
            "CREATE TABLE t(id INTEGER PRIMARY KEY, name TEXT); \
             INSERT INTO t(name) VALUES ('warrior'), ('archer');",
        )
        .unwrap();
        // SAFETY: db open; no callback.
        let rc = unsafe { sqlite3_exec(db, ddl.as_ptr(), None, ptr::null_mut(), ptr::null_mut()) };
        assert_eq!(rc, SQLITE_OK, "{}", unsafe {
            CStr::from_ptr(sqlite3_errmsg(db)).to_string_lossy()
        });

        let stmt = prepare(db, "SELECT count(*), max(name) FROM t");
        // SAFETY: stmt prepared above; column pointers are valid until the next step/finalize.
        unsafe {
            assert_eq!(sqlite3_step(stmt), SQLITE_ROW);
            assert_eq!(sqlite3_column_int(stmt, 0), 2);
            let max = CStr::from_ptr(sqlite3_column_text(stmt, 1).cast());
            assert_eq!(max.to_str().unwrap(), "warrior");
            assert_eq!(sqlite3_step(stmt), SQLITE_DONE);
            assert_eq!(sqlite3_finalize(stmt), SQLITE_OK);
            assert_eq!(sqlite3_close(db), SQLITE_OK);
        }
    }

    #[test]
    fn version_matches_vendored_amalgamation() {
        // SAFETY: returns a static NUL-terminated string.
        let v = unsafe { CStr::from_ptr(sqlite3_libversion()) };
        assert_eq!(v.to_str().unwrap(), "3.53.2");
    }
}
