//! Row-reading wrapper over the vendored SQLite (`sqlite-sys`). Every cell is read as text
//! (`sqlite3_column_text` converts integers and reals), which is all the snapshot readers need.

use std::ffi::{CStr, CString, c_char, c_int};
use std::path::Path;
use std::ptr;

use sqlite_sys as ffi;

/// One result row; `None` is SQL NULL.
pub type Row = Vec<Option<String>>;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("sqlite {op} on {path}: {msg}")]
    Sqlite {
        path: String,
        op: String,
        msg: String,
    },
    #[error("bad database path {path}")]
    BadPath { path: String },
    #[error("{what}: {msg}")]
    Data { what: String, msg: String },
}

impl DbError {
    pub(crate) fn data(what: &str, msg: impl Into<String>) -> Self {
        DbError::Data {
            what: what.to_owned(),
            msg: msg.into(),
        }
    }
}

pub struct SnapDb {
    raw: *mut ffi::sqlite3,
    what: String,
}

impl SnapDb {
    fn open(path: &str, display: String, flags: c_int) -> Result<Self, DbError> {
        let name = CString::new(path).map_err(|_| DbError::BadPath {
            path: display.clone(),
        })?;
        let mut raw = ptr::null_mut();
        // SAFETY: `name` is NUL-terminated and outlives the call; `raw` is a valid out pointer.
        let rc = unsafe { ffi::sqlite3_open_v2(name.as_ptr(), &mut raw, flags, ptr::null()) };
        let db = SnapDb { raw, what: display };
        if rc != ffi::SQLITE_OK {
            return Err(db.error("open"));
        }
        Ok(db)
    }

    /// Opens a snapshot file read-only.
    pub fn open_read_only(path: &Path) -> Result<Self, DbError> {
        let s = path.to_str().ok_or_else(|| DbError::BadPath {
            path: path.display().to_string(),
        })?;
        Self::open(s, s.to_owned(), ffi::SQLITE_OPEN_READONLY)
    }

    /// A private in-memory database (tests build mini snapshots in it with [`SnapDb::exec`]).
    pub fn open_memory() -> Result<Self, DbError> {
        Self::open(
            ":memory:",
            ":memory:".to_owned(),
            ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_CREATE,
        )
    }

    fn error(&self, op: &str) -> DbError {
        let msg = if self.raw.is_null() {
            "out of memory".to_owned()
        } else {
            // SAFETY: `raw` is an open (or failed-open) connection; errmsg returns a NUL-terminated string.
            unsafe { CStr::from_ptr(ffi::sqlite3_errmsg(self.raw)) }
                .to_string_lossy()
                .into_owned()
        };
        DbError::Sqlite {
            path: self.what.clone(),
            op: op.to_owned(),
            msg,
        }
    }

    /// Runs one or more SQL statements, ignoring rows.
    pub fn exec(&self, sql: &str) -> Result<(), DbError> {
        let c = CString::new(sql).map_err(|_| self.error("exec (NUL in SQL)"))?;
        // SAFETY: open connection, NUL-terminated SQL, no callback, no error-message out pointer.
        let rc = unsafe {
            ffi::sqlite3_exec(self.raw, c.as_ptr(), None, ptr::null_mut(), ptr::null_mut())
        };
        if rc != ffi::SQLITE_OK {
            return Err(self.error("exec"));
        }
        Ok(())
    }

    /// All rows of one SELECT; `cols` is the number of columns it returns.
    pub fn query(&self, sql: &str, cols: usize) -> Result<Vec<Row>, DbError> {
        let c = CString::new(sql).map_err(|_| self.error("query (NUL in SQL)"))?;
        let mut stmt = ptr::null_mut();
        // SAFETY: open connection, NUL-terminated SQL, valid out pointer.
        let rc = unsafe {
            ffi::sqlite3_prepare_v2(self.raw, c.as_ptr(), -1, &mut stmt, ptr::null_mut())
        };
        if rc != ffi::SQLITE_OK {
            return Err(self.error("prepare"));
        }
        let mut rows = Vec::new();
        let result = loop {
            // SAFETY: `stmt` is a live prepared statement, finalized below on every path.
            let step = unsafe { ffi::sqlite3_step(stmt) };
            if step == ffi::SQLITE_DONE {
                break Ok(());
            }
            if step != ffi::SQLITE_ROW {
                break Err(self.error("step"));
            }
            let mut row = Vec::with_capacity(cols);
            for i in 0..cols {
                // SAFETY: a row is available and `i < cols` (the caller's column count).
                let p = unsafe { ffi::sqlite3_column_text(stmt, i as c_int) };
                row.push(if p.is_null() {
                    None
                } else {
                    // SAFETY: SQLite returns a NUL-terminated UTF-8 string valid until the next step.
                    let s = unsafe { CStr::from_ptr(p as *const c_char) };
                    Some(s.to_string_lossy().into_owned())
                });
            }
            rows.push(row);
        };
        // SAFETY: `stmt` is a live prepared statement, finalized exactly once.
        unsafe { ffi::sqlite3_finalize(stmt) };
        result.map(|()| rows)
    }

    /// First column of the first row as an integer.
    pub fn scalar_i64(&self, sql: &str) -> Result<i64, DbError> {
        let rows = self.query(sql, 1)?;
        rows.first()
            .and_then(|r| r[0].as_deref())
            .and_then(|t| t.parse().ok())
            .ok_or_else(|| DbError::data(sql, "no integer result"))
    }
}

impl Drop for SnapDb {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            // SAFETY: `raw` is a connection opened by `open_v2`, closed exactly once. Statements
            // are always finalized before a method returns.
            unsafe { ffi::sqlite3_close(self.raw) };
        }
    }
}

/// SQL string literal.
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}
