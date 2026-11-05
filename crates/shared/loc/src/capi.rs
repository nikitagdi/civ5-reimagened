//! C ABI of the engine for the Lekmod fork (`include/loc.h`): `Localization::String::Compose` of the DLL can
//! call the same grammar as the client's `Locale`.
//!
//! The functions never unwind; strings are UTF-8, NUL-terminated. Output buffers follow the `snprintf`
//! convention: the result is truncated to fit and always terminated, the return value is the length the
//! full result needs (without the NUL).

#![allow(unsafe_code)]

use std::collections::{HashMap, VecDeque};
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::sync::{Arc, Mutex};

use crate::{Arg, LanguageInfo, Localizer, OwnedEntry, TextEntry, TextTable};

/// One argument. `kind`: 0 int (`int_val`), 1 int64 (`int_val`), 2 float (`num_val`), 3 double (`num_val`),
/// 4 string (`str_val`).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct LocArg {
    pub kind: c_int,
    pub int_val: i64,
    pub num_val: f64,
    pub str_val: *const c_char,
}

/// A row returned by the lookup callback; pointers must stay valid until the callback is called again.
#[repr(C)]
pub struct LocRow {
    pub text: *const c_char,
    /// May be null.
    pub gender: *const c_char,
    /// May be null.
    pub plurality: *const c_char,
}

/// `int fn(void* user, const char* tag, LocRow* out)`: returns non-zero when the row exists.
pub type LocLookupFn =
    unsafe extern "C" fn(user: *mut c_void, tag: *const c_char, out: *mut LocRow) -> c_int;

/// Rows added with `loc_add_text` plus the optional host callback. Rows are boxed and never freed before the
/// table, so references handed out by `get` stay valid for any borrow of the table.
struct CTable {
    rows: Mutex<HashMap<String, Box<OwnedEntry>>>,
    // The boxes keep the row addresses stable.
    #[allow(clippy::vec_box)]
    retired: Mutex<Vec<Box<OwnedEntry>>>,
    callback: Mutex<Option<(LocLookupFn, usize)>>,
}

impl CTable {
    fn insert(&self, tag: &str, entry: OwnedEntry) {
        let Ok(mut rows) = self.rows.lock() else {
            return;
        };
        if let Some(old) = rows.insert(tag.to_string(), Box::new(entry))
            && let Ok(mut r) = self.retired.lock()
        {
            r.push(old);
        }
    }

    fn borrow_row(&self, tag: &str) -> Option<TextEntry<'_>> {
        let rows = self.rows.lock().ok()?;
        let e: *const OwnedEntry = &**rows.get(tag)?;
        // SAFETY: the box is owned by this table and is only dropped with it (replaced rows move to
        // `retired`), so the reference is valid for the lifetime of `&self` even after the guard is gone.
        let e: &OwnedEntry = unsafe { &*e };
        Some(TextEntry {
            text: &e.text,
            gender: e.gender.as_deref(),
            plurality: e.plurality.as_deref(),
        })
    }
}

fn cstr(p: *const c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    // SAFETY: the caller passes a NUL-terminated string (documented in loc.h)
    let s = unsafe { CStr::from_ptr(p) };
    Some(s.to_string_lossy().into_owned())
}

impl TextTable for CTable {
    fn get(&self, tag: &str) -> Option<TextEntry<'_>> {
        if let Some(e) = self.borrow_row(tag) {
            return Some(e);
        }
        let (f, user) = (*self.callback.lock().ok()?)?;
        let ctag = CString::new(tag).ok()?;
        let mut row = LocRow {
            text: std::ptr::null(),
            gender: std::ptr::null(),
            plurality: std::ptr::null(),
        };
        // SAFETY: the host promised a valid callback and user pointer in `loc_set_lookup`
        let found = unsafe { f(user as *mut c_void, ctag.as_ptr(), &mut row) };
        if found == 0 {
            return None;
        }
        let entry = OwnedEntry {
            text: cstr(row.text)?,
            gender: cstr(row.gender),
            plurality: cstr(row.plurality),
        };
        self.insert(tag, entry);
        self.borrow_row(tag)
    }
}

/// An engine instance with one language.
pub struct LocHandle {
    loc: Localizer,
    table: Arc<CTable>,
    warnings: Mutex<VecDeque<String>>,
}

struct SharedTable(Arc<CTable>);

impl TextTable for SharedTable {
    fn get(&self, tag: &str) -> Option<TextEntry<'_>> {
        self.0.get(tag)
    }
}

fn args_from(args: *const LocArg, n: usize) -> Vec<Arg> {
    if args.is_null() || n == 0 {
        return Vec::new();
    }
    // SAFETY: the caller passes `n` readable `LocArg`s (documented in loc.h)
    let slice = unsafe { std::slice::from_raw_parts(args, n) };
    slice
        .iter()
        .map(|a| match a.kind {
            0 => Arg::Int(a.int_val as i32),
            1 => Arg::Int64(a.int_val),
            2 => Arg::Float(a.num_val as f32),
            3 => Arg::Double(a.num_val),
            _ => Arg::Str(cstr(a.str_val).unwrap_or_default()),
        })
        .collect()
}

fn write_out(s: &str, out: *mut c_char, cap: usize) -> usize {
    if !out.is_null() && cap > 0 {
        let n = s.len().min(cap - 1);
        // SAFETY: `out` has room for `cap` bytes; `n < cap`
        unsafe {
            std::ptr::copy_nonoverlapping(s.as_ptr(), out as *mut u8, n);
            *out.add(n) = 0;
        }
    }
    s.len()
}

/// Creates an engine for one language. `plural_rule` is 1..=16. Free with `loc_free`.
///
/// # Safety
/// `language_type` and `display_name` must be NUL-terminated strings (or null).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn loc_new(
    language_type: *const c_char,
    display_name: *const c_char,
    plural_rule: u32,
) -> *mut LocHandle {
    let info = LanguageInfo::new(
        &cstr(language_type).unwrap_or_else(|| "en_US".into()),
        &cstr(display_name).unwrap_or_default(),
        plural_rule,
    );
    let table = Arc::new(CTable {
        rows: Mutex::new(HashMap::new()),
        retired: Mutex::new(Vec::new()),
        callback: Mutex::new(None),
    });
    let loc = Localizer::new(info, Box::new(SharedTable(table.clone())));
    Box::into_raw(Box::new(LocHandle {
        loc,
        table,
        warnings: Mutex::new(VecDeque::new()),
    }))
}

/// Frees an engine. Null is ignored.
///
/// # Safety
/// `h` must come from `loc_new` and not be used afterwards.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn loc_free(h: *mut LocHandle) {
    if !h.is_null() {
        // SAFETY: `h` came from `Box::into_raw` in `loc_new`
        drop(unsafe { Box::from_raw(h) });
    }
}

/// Sets the host dictionary: rows not added with `loc_add_text` are fetched through `f`.
///
/// # Safety
/// `h` is a live handle; `f` and `user` stay valid for the life of the handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn loc_set_lookup(
    h: *mut LocHandle,
    f: Option<LocLookupFn>,
    user: *mut c_void,
) {
    // SAFETY: `h` is a live handle
    let Some(h) = (unsafe { h.as_ref() }) else {
        return;
    };
    if let Ok(mut cb) = h.table.callback.lock() {
        *cb = f.map(|f| (f, user as usize));
    }
}

/// Adds or replaces a text row. `gender` and `plurality` may be null. Returns 1 on success.
///
/// # Safety
/// `h` is a live handle; the strings are NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn loc_add_text(
    h: *mut LocHandle,
    tag: *const c_char,
    text: *const c_char,
    gender: *const c_char,
    plurality: *const c_char,
) -> c_int {
    // SAFETY: `h` is a live handle
    let Some(h) = (unsafe { h.as_ref() }) else {
        return 0;
    };
    let (Some(tag), Some(text)) = (cstr(tag), cstr(text)) else {
        return 0;
    };
    h.table.insert(
        &tag,
        OwnedEntry {
            text,
            gender: cstr(gender),
            plurality: cstr(plurality),
        },
    );
    1
}

fn collect_warnings(h: &LocHandle) {
    let new = h.loc.take_warnings();
    if new.is_empty() {
        return;
    }
    if let Ok(mut q) = h.warnings.lock() {
        for w in new {
            if q.len() < 256 {
                q.push_back(w);
            }
        }
    }
}

/// Composes a template string (`String(template)` + arguments + `toUTF8`): `Localization::String::Compose`.
/// Returns the length of the full result; an invalid template gives the empty string.
///
/// # Safety
/// `h` is a live handle; `template` is NUL-terminated; `args` points to `nargs` arguments; `out` has `cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn loc_compose(
    h: *mut LocHandle,
    template: *const c_char,
    args: *const LocArg,
    nargs: usize,
    out: *mut c_char,
    cap: usize,
) -> usize {
    // SAFETY: `h` is a live handle
    let Some(h) = (unsafe { h.as_ref() }) else {
        return 0;
    };
    let Some(template) = cstr(template) else {
        return 0;
    };
    let s = h.loc.compose(&template, &args_from(args, nargs));
    collect_warnings(h);
    write_out(&s, out, cap)
}

/// `Locale.ConvertTextKey`: looks `key` up (a missing key is its own text) and composes it.
///
/// # Safety
/// As `loc_compose`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn loc_lookup(
    h: *mut LocHandle,
    key: *const c_char,
    args: *const LocArg,
    nargs: usize,
    out: *mut c_char,
    cap: usize,
) -> usize {
    // SAFETY: `h` is a live handle
    let Some(h) = (unsafe { h.as_ref() }) else {
        return 0;
    };
    let Some(key) = cstr(key) else { return 0 };
    let s = h.loc.convert(&key, &args_from(args, nargs));
    collect_warnings(h);
    write_out(&s, out, cap)
}

/// `Locale.HasTextKey`: 1 when the key exists.
///
/// # Safety
/// `h` is a live handle; `key` is NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn loc_has_key(h: *mut LocHandle, key: *const c_char) -> c_int {
    // SAFETY: `h` is a live handle
    let Some(h) = (unsafe { h.as_ref() }) else {
        return 0;
    };
    let Some(key) = cstr(key) else { return 0 };
    h.loc.has_key(&key) as c_int
}

/// Counters: `which` 0 = missing keys, 1 = bad templates.
///
/// # Safety
/// `h` is a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn loc_stat(h: *mut LocHandle, which: c_int) -> u64 {
    // SAFETY: `h` is a live handle
    let Some(h) = (unsafe { h.as_ref() }) else {
        return 0;
    };
    let s = h.loc.stats();
    if which == 0 {
        s.missing
    } else {
        s.bad_template
    }
}

/// Pops the oldest pending warning into `out` (snprintf convention); returns its length, or `usize::MAX`
/// when there is none.
///
/// # Safety
/// `h` is a live handle; `out` has `cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn loc_next_warning(
    h: *mut LocHandle,
    out: *mut c_char,
    cap: usize,
) -> usize {
    // SAFETY: `h` is a live handle
    let Some(h) = (unsafe { h.as_ref() }) else {
        return usize::MAX;
    };
    collect_warnings(h);
    let w = h.warnings.lock().ok().and_then(|mut q| q.pop_front());
    match w {
        Some(w) => write_out(&w, out, cap),
        None => usize::MAX,
    }
}
