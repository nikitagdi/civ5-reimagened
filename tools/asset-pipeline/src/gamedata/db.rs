//! Minimal safe wrapper over the vendored SQLite (`sqlite-sys`): open, exec, one scalar.

use std::ffi::{CStr, CString, c_char, c_int};
use std::path::Path;
use std::ptr;

use sqlite_sys as ffi;

use super::GameDataError;

/// `SQLITE_OPEN_URI`: not exported by sqlite-sys.
const OPEN_URI: c_int = 0x40;

pub struct Db {
    raw: *mut ffi::sqlite3,
    what: String,
}

impl Db {
    fn open(path: &Path, flags: c_int) -> Result<Self, GameDataError> {
        let what = path.display().to_string();
        let name = path_cstring(path)?;
        let mut raw = ptr::null_mut();
        // SAFETY: `name` is NUL-terminated and outlives the call; `raw` is a valid out pointer.
        let rc = unsafe { ffi::sqlite3_open_v2(name.as_ptr(), &mut raw, flags, ptr::null()) };
        let db = Db { raw, what };
        if rc != ffi::SQLITE_OK {
            return Err(db.error("open"));
        }
        Ok(db)
    }

    pub fn open_read_only(path: &Path) -> Result<Self, GameDataError> {
        Self::open(path, ffi::SQLITE_OPEN_READONLY)
    }

    /// Opens (creating if needed) a database for writing. URI file names are enabled so that
    /// `ATTACH` can open a source read-only.
    pub fn create(path: &Path) -> Result<Self, GameDataError> {
        Self::open(
            path,
            ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_CREATE | OPEN_URI,
        )
    }

    fn error(&self, op: &str) -> GameDataError {
        let msg = if self.raw.is_null() {
            "out of memory".to_owned()
        } else {
            // SAFETY: `raw` is an open (or failed-open) connection; errmsg returns a NUL-terminated string.
            unsafe { CStr::from_ptr(ffi::sqlite3_errmsg(self.raw)) }
                .to_string_lossy()
                .into_owned()
        };
        GameDataError::Sqlite {
            path: self.what.clone(),
            op: op.to_owned(),
            msg,
        }
    }

    /// Runs one or more SQL statements.
    pub fn exec(&self, sql: &str) -> Result<(), GameDataError> {
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

    /// First column of the first row as text (`None` for no row or NULL).
    pub fn scalar_text(&self, sql: &str) -> Result<Option<String>, GameDataError> {
        let c = CString::new(sql).map_err(|_| self.error("query (NUL in SQL)"))?;
        let mut stmt = ptr::null_mut();
        // SAFETY: open connection, NUL-terminated SQL, valid out pointer.
        let rc = unsafe {
            ffi::sqlite3_prepare_v2(self.raw, c.as_ptr(), -1, &mut stmt, ptr::null_mut())
        };
        if rc != ffi::SQLITE_OK {
            return Err(self.error("prepare"));
        }
        // SAFETY: `stmt` was just prepared; finalized below on every path.
        let step = unsafe { ffi::sqlite3_step(stmt) };
        let out = if step == ffi::SQLITE_ROW {
            // SAFETY: a row is available; the text pointer is valid until the next step/finalize.
            let p = unsafe { ffi::sqlite3_column_text(stmt, 0) };
            if p.is_null() {
                Ok(None)
            } else {
                // SAFETY: SQLite returns a NUL-terminated UTF-8 string.
                let s = unsafe { CStr::from_ptr(p as *const c_char) };
                Ok(Some(s.to_string_lossy().into_owned()))
            }
        } else if step == ffi::SQLITE_DONE {
            Ok(None)
        } else {
            Err(self.error("step"))
        };
        // SAFETY: `stmt` is a live prepared statement, finalized exactly once.
        unsafe { ffi::sqlite3_finalize(stmt) };
        out
    }

    /// Every row of a query as text columns (`None` for NULL).
    pub fn rows(
        &self,
        sql: &str,
        columns: usize,
    ) -> Result<Vec<Vec<Option<String>>>, GameDataError> {
        let c = CString::new(sql).map_err(|_| self.error("query (NUL in SQL)"))?;
        let mut stmt = ptr::null_mut();
        // SAFETY: open connection, NUL-terminated SQL, valid out pointer.
        let rc = unsafe {
            ffi::sqlite3_prepare_v2(self.raw, c.as_ptr(), -1, &mut stmt, ptr::null_mut())
        };
        if rc != ffi::SQLITE_OK {
            return Err(self.error("prepare"));
        }
        let mut out = Vec::new();
        let result = loop {
            // SAFETY: `stmt` is a live prepared statement, finalized below on every path.
            let step = unsafe { ffi::sqlite3_step(stmt) };
            if step == ffi::SQLITE_DONE {
                break Ok(());
            }
            if step != ffi::SQLITE_ROW {
                break Err(self.error("step"));
            }
            let mut row = Vec::with_capacity(columns);
            for i in 0..columns {
                // SAFETY: a row is available; the text pointer is valid until the next step/finalize.
                let p = unsafe { ffi::sqlite3_column_text(stmt, i as c_int) };
                row.push(if p.is_null() {
                    None
                } else {
                    // SAFETY: SQLite returns a NUL-terminated UTF-8 string.
                    Some(
                        unsafe { CStr::from_ptr(p as *const c_char) }
                            .to_string_lossy()
                            .into_owned(),
                    )
                });
            }
            out.push(row);
        };
        // SAFETY: `stmt` is a live prepared statement, finalized exactly once.
        unsafe { ffi::sqlite3_finalize(stmt) };
        result.map(|()| out)
    }

    pub fn scalar_i64(&self, sql: &str) -> Result<i64, GameDataError> {
        let text = self.scalar_text(sql)?;
        text.as_deref()
            .and_then(|t| t.parse().ok())
            .ok_or_else(|| self.error("query (no integer result)"))
    }
}

impl Drop for Db {
    fn drop(&mut self) {
        if !self.raw.is_null() {
            // SAFETY: `raw` is a connection opened by `open_v2`, closed exactly once. Statements
            // are always finalized before a method returns.
            unsafe { ffi::sqlite3_close(self.raw) };
        }
    }
}

fn bad_path(path: &Path) -> GameDataError {
    GameDataError::BadPath {
        path: path.display().to_string(),
    }
}

fn path_cstring(path: &Path) -> Result<CString, GameDataError> {
    let s = path.to_str().ok_or_else(|| bad_path(path))?;
    CString::new(s).map_err(|_| bad_path(path))
}

/// SQL string literal.
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// SQL string literal holding a read-only `file:` URI for `path` (for `ATTACH`), so the source
/// is never written: no journal, no `-wal`/`-shm` next to the snapshot.
pub fn read_only_uri_literal(path: &Path) -> Result<String, GameDataError> {
    let s = path.to_str().ok_or_else(|| bad_path(path))?;
    let mut uri = String::from("file:");
    let slashed = s.replace('\\', "/");
    if slashed.as_bytes().get(1) == Some(&b':') {
        uri.push('/'); // `C:/x` needs a leading slash in a file URI
    }
    for ch in slashed.chars() {
        match ch {
            '%' => uri.push_str("%25"),
            '?' => uri.push_str("%3f"),
            '#' => uri.push_str("%23"),
            c => uri.push(c),
        }
    }
    uri.push_str("?mode=ro");
    Ok(quote(&uri))
}
