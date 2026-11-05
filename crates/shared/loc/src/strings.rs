//! `Locale.*` string functions that do not need the dictionary (`cvLuaLocaleLibrary`, 0x0895210a..0x08953d32).
//! Lengths and positions count Unicode scalar values, as the exe's UTF-8 lead-byte table does.

/// `Locale.TruncateString` failed: `bAddEllipsis` with fewer than 4 characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TruncateError;

impl std::fmt::Display for TruncateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Max characters must be larger than 3 if bAddEllipsis is true.")
    }
}

impl std::error::Error for TruncateError {}

/// `Locale.Length`.
pub fn length(s: &str) -> usize {
    // re: 0x089524e6 cvLuaLocaleLibrary::lLength
    s.chars().count()
}

/// `Locale.Substring(s, start, count)`: `start` is 1-based; out-of-range parts are clamped.
pub fn substring(s: &str, start: i32, count: i32) -> String {
    // re: 0x08952c56 cvLuaLocaleLibrary::lSubstring
    let skip = (start - 1).max(0) as usize;
    s.chars().skip(skip).take(count.max(0) as usize).collect()
}

/// `Locale.TruncateString(s, maxChars, addEllipsis)`.
///
/// The result has `maxChars - 1` characters including the three dots: the exe stops one character early.
pub fn truncate_string(
    s: &str,
    max_chars: i32,
    add_ellipsis: bool,
) -> Result<String, TruncateError> {
    // re: 0x08952b0e cvLuaLocaleLibrary::lTruncateString
    if add_ellipsis && max_chars < 4 {
        return Err(TruncateError);
    }
    let chars: Vec<char> = s.chars().collect();
    if chars.is_empty() || max_chars <= 0 {
        // maxChars <= 0 wraps to a huge unsigned count in the exe, which returns the string unchanged
        return Ok(s.to_string());
    }
    let max = max_chars as usize;
    // The scan consumes `max` characters; reaching the end first returns the original.
    if chars.len() <= max {
        return Ok(s.to_string());
    }
    let keep = if add_ellipsis { max - 4 } else { max - 1 };
    let mut out: String = chars[..keep].iter().collect();
    if add_ellipsis {
        out.push_str("...");
    }
    Ok(out)
}

/// `Locale.StartsWith`.
pub fn starts_with(s: &str, prefix: &str) -> bool {
    // re: 0x0895210a cvLuaLocaleLibrary::lStartsWith
    s.as_bytes().starts_with(prefix.as_bytes())
}

/// `Locale.EndsWith`: the last occurrence of `suffix` may be followed only by CR/LF characters.
pub fn ends_with(s: &str, suffix: &str) -> bool {
    // re: 0x089521a8 cvLuaLocaleLibrary::lEndsWith
    match s.rfind(suffix) {
        None => false,
        Some(pos) => s.as_bytes()[pos + suffix.len()..]
            .iter()
            .all(|&b| b == b'\n' || b == b'\r'),
    }
}

/// `Locale.IsNilOrEmpty`.
pub fn is_nil_or_empty(s: Option<&str>) -> bool {
    // re: 0x0895235a cvLuaLocaleLibrary::lIsNilOrEmpty
    s.is_none_or(str::is_empty)
}

/// Whitespace set of `lIsNilOrWhitespace`: ASCII controls 9-13, space, NEL, NBSP, Ogham, Mongolian vowel
/// separator, U+2000..=U+200A, line/paragraph separators, narrow NBSP, medium space, ideographic space.
fn is_whitespace(c: char) -> bool {
    matches!(c as u32,
        0x09..=0x0d | 0x20 | 0x85 | 0xa0 | 0x1680 | 0x180e | 0x2000..=0x200a | 0x2028 | 0x2029 | 0x202f | 0x205f | 0x3000)
}

/// `Locale.IsNilOrWhitespace`.
pub fn is_nil_or_whitespace(s: Option<&str>) -> bool {
    // re: 0x089523be cvLuaLocaleLibrary::lIsNilOrWhitespace
    s.is_none_or(|s| s.chars().all(is_whitespace))
}

/// `Locale.IsASCII`.
pub fn is_ascii(s: &str) -> bool {
    // re: 0x08953cc0 cvLuaLocaleLibrary::lIsASCII
    s.is_ascii()
}

/// `Locale.IsUTF8` on raw bytes.
pub fn is_utf8(bytes: &[u8]) -> bool {
    // re: 0x08953d32 cvLuaLocaleLibrary::lIsUTF8
    std::str::from_utf8(bytes).is_ok()
}

/// `Locale.GetHotKeyDescription(key)`: only `KB_F10` has a description in the exe.
pub fn get_hot_key_description(key: &str) -> Option<String> {
    // re: 0x08951c12 cvLuaLocaleLibrary::lGetHotKeyDescription
    (key == "KB_F10").then(|| "F10".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn length_and_substring_count_characters() {
        assert_eq!(length("h\u{e9}llo \u{43f}\u{440}"), 8);
        assert_eq!(substring("h\u{e9}llo", 2, 3), "\u{e9}ll");
        assert_eq!(substring("abc", 1, 10), "abc");
        assert_eq!(substring("abc", 5, 2), "");
        assert_eq!(substring("abc", 0, 2), "ab");
    }

    #[test]
    fn truncate_matches_the_exe_off_by_one() {
        assert_eq!(truncate_string("abc", 3, false).unwrap(), "abc");
        assert_eq!(
            truncate_string("abcdefghij", 10, false).unwrap(),
            "abcdefghij"
        );
        assert_eq!(
            truncate_string("abcdefghijk", 10, false).unwrap(),
            "abcdefghi"
        );
        assert_eq!(
            truncate_string("abcdefghijk", 10, true).unwrap(),
            "abcdef..."
        );
        assert_eq!(truncate_string("", 4, true).unwrap(), "");
        assert!(truncate_string("abcdefgh", 3, true).is_err());
        assert_eq!(
            truncate_string("\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}", 3, false).unwrap(),
            "\u{e9}\u{e9}"
        );
    }

    #[test]
    fn affix_tests() {
        assert!(starts_with("hello", "he"));
        assert!(!starts_with("he", "hello"));
        assert!(ends_with("hello", "llo"));
        assert!(ends_with("hello\r\n", "llo"));
        assert!(!ends_with("hello x", "llo"));
        assert!(!ends_with("hello", "x"));
    }

    #[test]
    fn emptiness() {
        assert!(is_nil_or_empty(None));
        assert!(is_nil_or_empty(Some("")));
        assert!(!is_nil_or_empty(Some(" ")));
        assert!(is_nil_or_whitespace(Some(" \t\u{a0}\u{3000}")));
        assert!(!is_nil_or_whitespace(Some(" x")));
        assert!(is_nil_or_whitespace(None));
        assert!(is_ascii("abc") && !is_ascii("\u{e9}"));
        assert!(is_utf8("\u{e9}".as_bytes()) && !is_utf8(&[0xff, 0xfe]));
        assert_eq!(get_hot_key_description("KB_F10").as_deref(), Some("F10"));
        assert_eq!(get_hot_key_description("KB_A"), None);
    }
}
