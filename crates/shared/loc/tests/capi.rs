//! The C ABI gives the same strings as the Rust API.
#![cfg(feature = "capi")]
#![allow(unsafe_code)]

use std::ffi::{CStr, CString, c_char, c_int, c_void};

use loc::capi::*;

fn c(s: &str) -> CString {
    CString::new(s).unwrap()
}

fn compose(h: *mut LocHandle, tpl: &str, args: &[LocArg]) -> String {
    let mut buf = vec![0u8; 256];
    let t = c(tpl);
    // SAFETY: valid handle, NUL-terminated template, buffer of `cap` bytes
    let n = unsafe {
        loc_compose(
            h,
            t.as_ptr(),
            args.as_ptr(),
            args.len(),
            buf.as_mut_ptr() as *mut c_char,
            buf.len(),
        )
    };
    String::from_utf8(buf[..n.min(255)].to_vec()).unwrap()
}

fn arg_d(v: f64) -> LocArg {
    LocArg {
        kind: 3,
        int_val: 0,
        num_val: v,
        str_val: std::ptr::null(),
    }
}

fn arg_s(s: &CString) -> LocArg {
    LocArg {
        kind: 4,
        int_val: 0,
        num_val: 0.0,
        str_val: s.as_ptr(),
    }
}

#[test]
fn compose_lookup_and_stats() {
    // SAFETY: all pointers below are valid for the calls
    unsafe {
        let h = loc_new(c("en_US").as_ptr(), c("English").as_ptr(), 2);
        assert!(!h.is_null());
        assert_eq!(
            loc_add_text(
                h,
                c("TXT_KEY_N").as_ptr(),
                c("{1_N} {1_N: plural 1?item; other?items;}").as_ptr(),
                std::ptr::null(),
                std::ptr::null()
            ),
            1
        );
        assert_eq!(
            loc_add_text(
                h,
                c("TXT_KEY_ARCHER").as_ptr(),
                c("Archer").as_ptr(),
                c("neuter:an").as_ptr(),
                c("1").as_ptr()
            ),
            1
        );
        assert_eq!(
            loc_add_text(
                h,
                c("TXT_KEY_GRAMMAR_A_AN").as_ptr(),
                c("{1: gender *:an?an {1}; other?a {1};}").as_ptr(),
                std::ptr::null(),
                std::ptr::null()
            ),
            1
        );

        assert_eq!(
            compose(h, "{1} + {2}", &[arg_d(1.0), arg_d(2.5)]),
            "1 + 2.5"
        );
        let archer = c("TXT_KEY_ARCHER");
        assert_eq!(
            compose(h, "{TXT_KEY_GRAMMAR_A_AN << {@1}}", &[arg_s(&archer)]),
            "an Archer"
        );

        let mut buf = [0 as c_char; 64];
        let n = loc_lookup(
            h,
            c("TXT_KEY_N").as_ptr(),
            [arg_d(3.0)].as_ptr(),
            1,
            buf.as_mut_ptr(),
            buf.len(),
        );
        assert_eq!(CStr::from_ptr(buf.as_ptr()).to_str().unwrap(), "3 items");
        assert_eq!(n, 7);
        // truncation follows snprintf
        let mut small = [0 as c_char; 4];
        let n = loc_lookup(
            h,
            c("TXT_KEY_N").as_ptr(),
            [arg_d(3.0)].as_ptr(),
            1,
            small.as_mut_ptr(),
            small.len(),
        );
        assert_eq!(n, 7);
        assert_eq!(CStr::from_ptr(small.as_ptr()).to_str().unwrap(), "3 i");

        assert_eq!(loc_has_key(h, c("TXT_KEY_N").as_ptr()), 1);
        assert_eq!(loc_has_key(h, c("TXT_KEY_ZZZ").as_ptr()), 0);

        // an invalid template gives "" and a warning
        assert_eq!(compose(h, "{1", &[]), "");
        assert_eq!(loc_stat(h, 1), 1);
        let mut w = [0 as c_char; 128];
        let wl = loc_next_warning(h, w.as_mut_ptr(), w.len());
        assert_ne!(wl, usize::MAX);
        assert_eq!(loc_next_warning(h, w.as_mut_ptr(), w.len()), usize::MAX);

        // missing key
        let _ = compose(h, "{TXT_KEY_NOT_THERE}", &[]);
        assert_eq!(loc_stat(h, 0), 1);
        loc_free(h);
        loc_free(std::ptr::null_mut());
    }
}

unsafe extern "C" fn host_lookup(user: *mut c_void, tag: *const c_char, out: *mut LocRow) -> c_int {
    // SAFETY: `user` is the CString passed in the test; `tag` and `out` are valid
    unsafe {
        let text = &*(user as *const CString);
        if CStr::from_ptr(tag).to_str().unwrap() == "TXT_KEY_HOST" {
            (*out).text = text.as_ptr();
            (*out).gender = std::ptr::null();
            (*out).plurality = std::ptr::null();
            1
        } else {
            0
        }
    }
}

#[test]
fn host_callback_dictionary() {
    let text = c("From the host: {1}");
    // SAFETY: `text` outlives the handle; all pointers are valid for the calls
    unsafe {
        let h = loc_new(c("en_US").as_ptr(), c("English").as_ptr(), 2);
        loc_set_lookup(h, Some(host_lookup), &text as *const CString as *mut c_void);
        let mut buf = [0 as c_char; 64];
        loc_lookup(
            h,
            c("TXT_KEY_HOST").as_ptr(),
            [arg_d(7.0)].as_ptr(),
            1,
            buf.as_mut_ptr(),
            buf.len(),
        );
        assert_eq!(
            CStr::from_ptr(buf.as_ptr()).to_str().unwrap(),
            "From the host: 7"
        );
        assert_eq!(loc_has_key(h, c("TXT_KEY_HOST").as_ptr()), 1);
        loc_free(h);
    }
}
