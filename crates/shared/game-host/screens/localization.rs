//! Read the exported localization database in memory, then close it before Lua starts.
use loc::{LanguageInfo, Localizer, MapTable};
use lua_sys::{Lua, LuaError, LuaValue, Variadic};
use sqlite_sys as sql;
use std::ffi::{CStr, c_char};
use std::ptr;

pub(crate) fn attach(lua: &Lua, bytes: &[u8]) -> Result<(), String> {
    let mut bytes = bytes.to_vec();
    let mut db = ptr::null_mut();
    // SAFETY: SQLite gets a valid out pointer and a static NUL-terminated filename.
    let opened = unsafe {
        sql::sqlite3_open_v2(
            c":memory:".as_ptr(),
            &mut db,
            sql::SQLITE_OPEN_READWRITE | sql::SQLITE_OPEN_CREATE,
            ptr::null(),
        )
    };
    if opened != sql::SQLITE_OK {
        if !db.is_null() {
            unsafe {
                sql::sqlite3_close(db);
            }
        }
        return Err("cannot open localization database".into());
    }
    let mut table = MapTable::new();
    let mut stmt = ptr::null_mut();
    // SAFETY: bytes stays allocated until this connection closes. READONLY (4) prevents
    // SQLite from resizing or freeing the borrowed buffer. All SQL is built into the server.
    let result = unsafe {
        let rc = sql::sqlite3_deserialize(
            db,
            c"main".as_ptr(),
            bytes.as_mut_ptr(),
            bytes.len() as i64,
            bytes.len() as i64,
            4,
        );
        if rc != sql::SQLITE_OK {
            Err("cannot read localization database".to_owned())
        } else if sql::sqlite3_prepare_v2(
            db,
            c"SELECT Tag, Text FROM Language_en_US".as_ptr(),
            -1,
            &mut stmt,
            ptr::null_mut(),
        ) != sql::SQLITE_OK
        {
            Err("localization database has no English text table".to_owned())
        } else {
            let mut rc = sql::sqlite3_step(stmt);
            while rc == sql::SQLITE_ROW {
                let key = sql::sqlite3_column_text(stmt, 0);
                let value = sql::sqlite3_column_text(stmt, 1);
                if !key.is_null() && !value.is_null() {
                    let key = CStr::from_ptr(key.cast::<c_char>()).to_string_lossy();
                    let value = CStr::from_ptr(value.cast::<c_char>()).to_string_lossy();
                    table.insert_text(&key, &value);
                }
                rc = sql::sqlite3_step(stmt);
            }
            if rc == sql::SQLITE_DONE {
                Ok(())
            } else {
                Err("localization query failed".to_owned())
            }
        }
    };
    // SAFETY: stmt, when present, belongs to db. Closing ends SQLite's borrow of bytes.
    unsafe {
        if !stmt.is_null() {
            sql::sqlite3_finalize(stmt);
        }
        sql::sqlite3_close(db);
    }
    result?;
    // SIMPLIFIED(SIMP-321): server-built screen text uses the exported en_US dictionary.
    let localizer = Localizer::new(LanguageInfo::en_us(), Box::new(table));
    let convert = lua.create_function(move |_, (key, args): (String, Variadic<LuaValue>)| {
        let args = args
            .0
            .into_iter()
            .map(|v| match v {
                LuaValue::Number(n) => Ok(loc::Arg::Double(n)),
                LuaValue::String(s) => Ok(loc::Arg::Str(s.to_string_lossy().into_owned())),
                _ => Err(LuaError::msg(
                    "localization arguments must be strings or numbers",
                )),
            })
            .collect::<lua_sys::Result<Vec<_>>>()?;
        Ok(localizer.convert(&key, &args))
    });
    let locale = lua.create_table();
    locale
        .raw_set("ConvertTextKey", convert.clone())
        .map_err(|e| e.to_string())?;
    locale
        .raw_set("Lookup", convert)
        .map_err(|e| e.to_string())?;
    lua.globals()
        .raw_set("Locale", locale)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn english_dictionary_and_template_arguments_reach_lua() {
        let path =
            std::env::temp_dir().join(format!("screen-locale-{}.sqlite", std::process::id()));
        let filename = CString::new(path.to_string_lossy().as_bytes()).unwrap();
        let mut db = ptr::null_mut();
        // SAFETY: valid SQLite output pointer, filename and static SQL. The connection is closed
        // before its file is read, and the file belongs exclusively to this test process.
        unsafe {
            assert_eq!(
                sql::sqlite3_open_v2(
                    filename.as_ptr(),
                    &mut db,
                    sql::SQLITE_OPEN_READWRITE | sql::SQLITE_OPEN_CREATE,
                    ptr::null()
                ),
                sql::SQLITE_OK
            );
            assert_eq!(sql::sqlite3_exec(db, c"CREATE TABLE Language_en_US(Tag TEXT, Text TEXT); INSERT INTO Language_en_US VALUES('TXT_KEY_SCREEN_TEST','Choice {1_Name}');".as_ptr(), None, ptr::null_mut(), ptr::null_mut()), sql::SQLITE_OK);
            assert_eq!(sql::sqlite3_close(db), sql::SQLITE_OK);
        }
        let bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let lua = Lua::new().unwrap();
        attach(&lua, &bytes).unwrap();
        let value: String = lua
            .load("return Locale.ConvertTextKey('TXT_KEY_SCREEN_TEST', 'Pottery')")
            .eval()
            .unwrap();
        assert_eq!(value, "Choice Pottery");
        let value: String = lua
            .load("return Locale.Lookup('TXT_KEY_SCREEN_TEST', 'Writing')")
            .eval()
            .unwrap();
        assert_eq!(value, "Choice Writing");
    }
}
