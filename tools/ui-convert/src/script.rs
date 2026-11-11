//! The static rules for skin scripts: classic scripts only (no `import`/`export`), and none of
//! the browser names a worker script has no access to.
//!
//! A light tokenizer is enough: comments, strings, template literals and regular expressions are
//! skipped; an identifier that follows `.` is a property name and does not count.

use crate::errors::{ConvError, Place, codes};

/// Names a skin script may not use (it runs in a worker behind the `ui` object).
pub const FORBIDDEN: &[&str] = &[
    "document",
    "window",
    "fetch",
    "XMLHttpRequest",
    "WebSocket",
    "importScripts",
    "indexedDB",
    "localStorage",
];

fn is_id_start(c: char) -> bool {
    c == '_' || c == '$' || c.is_alphabetic()
}

fn is_id(c: char) -> bool {
    is_id_start(c) || c.is_ascii_digit()
}

/// Checks one script; `file` is its path in the skin.
pub fn check(file: &str, text: &str) -> Vec<ConvError> {
    let chars: Vec<char> = text.chars().collect();
    let mut errs = Vec::new();
    let (mut line, mut col) = (1u32, 1u32);
    let mut i = 0;
    // The last significant token: decides whether `/` starts a regular expression and whether an
    // identifier is a property name.
    let mut prev: Option<char> = None;
    let mut prev_word = String::new();
    let advance = |i: &mut usize, line: &mut u32, col: &mut u32, n: usize, chars: &[char]| {
        for _ in 0..n {
            if *i < chars.len() {
                if chars[*i] == '\n' {
                    *line += 1;
                    *col = 1;
                } else {
                    *col += 1;
                }
                *i += 1;
            }
        }
    };
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c.is_whitespace() {
            advance(&mut i, &mut line, &mut col, 1, &chars);
        } else if c == '/' && next == Some('/') {
            while i < chars.len() && chars[i] != '\n' {
                advance(&mut i, &mut line, &mut col, 1, &chars);
            }
        } else if c == '/' && next == Some('*') {
            advance(&mut i, &mut line, &mut col, 2, &chars);
            while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                advance(&mut i, &mut line, &mut col, 1, &chars);
            }
            advance(&mut i, &mut line, &mut col, 2, &chars);
        } else if c == '"' || c == '\'' || c == '`' {
            advance(&mut i, &mut line, &mut col, 1, &chars);
            while i < chars.len() && chars[i] != c {
                let n = if chars[i] == '\\' { 2 } else { 1 };
                advance(&mut i, &mut line, &mut col, n, &chars);
            }
            advance(&mut i, &mut line, &mut col, 1, &chars);
            prev = Some('"');
            prev_word.clear();
        } else if c == '/'
            && (prev.is_none_or(|p| "(,=:[!&|?{};+-*%<>~^".contains(p))
                || matches!(prev_word.as_str(), "return" | "typeof" | "case"))
        {
            // A regular expression literal.
            advance(&mut i, &mut line, &mut col, 1, &chars);
            let mut class = false;
            while i < chars.len() && chars[i] != '\n' && (class || chars[i] != '/') {
                match chars[i] {
                    '\\' => advance(&mut i, &mut line, &mut col, 1, &chars),
                    '[' => class = true,
                    ']' => class = false,
                    _ => {}
                }
                advance(&mut i, &mut line, &mut col, 1, &chars);
            }
            advance(&mut i, &mut line, &mut col, 1, &chars);
            while i < chars.len() && chars[i].is_ascii_alphabetic() {
                advance(&mut i, &mut line, &mut col, 1, &chars);
            }
            prev = Some('"');
            prev_word.clear();
        } else if is_id_start(c) {
            let (l0, c0) = (line, col);
            let start = i;
            while i < chars.len() && is_id(chars[i]) {
                advance(&mut i, &mut line, &mut col, 1, &chars);
            }
            let word: String = chars[start..i].iter().collect();
            let member = prev == Some('.');
            if !member {
                let place = Place::at(file, l0, c0);
                if word == "import" || word == "export" {
                    errs.push(
                        ConvError::new(
                            codes::JS_MODULE,
                            place,
                            format!("`{word}`: skin scripts are classic scripts"),
                        )
                        .element(word.clone()),
                    );
                } else if FORBIDDEN.contains(&word.as_str()) {
                    errs.push(
                        ConvError::new(
                            codes::JS_NAME,
                            place,
                            format!("`{word}` is not available to a skin script"),
                        )
                        .element(word.clone()),
                    );
                }
            }
            prev = Some('a');
            prev_word = word;
        } else {
            if c.is_ascii_digit() {
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '.') {
                    advance(&mut i, &mut line, &mut col, 1, &chars);
                }
                prev = Some('0');
            } else {
                prev = Some(c);
                advance(&mut i, &mut line, &mut col, 1, &chars);
            }
            prev_word.clear();
        }
    }
    errs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes_of(src: &str) -> Vec<(String, u32, u32)> {
        check("s.js", src)
            .into_iter()
            .map(|e| (e.code, e.place.line, e.place.col))
            .collect()
    }

    #[test]
    fn modules_and_browser_names_are_found() {
        assert_eq!(
            codes_of("import x from './a.js';\nconst d = document.body;"),
            [("E1-007".to_string(), 1, 1), ("E1-008".to_string(), 2, 11)]
        );
        assert_eq!(codes_of("export function f() {}")[0].0, "E1-007");
        assert_eq!(codes_of("ui.window.x = 1; const s = 'document';").len(), 0);
        assert_eq!(
            codes_of("// document\n/* fetch */ let r = /window/g;").len(),
            0
        );
        assert_eq!(codes_of("let a = 4 / 2; fetch('x');")[0].0, "E1-008");
        assert_eq!(codes_of("const t = `a ${'b'} window`;").len(), 0);
    }
}
