//! A scanner for Lua 5.1 source: tokens without comments, and the references to exe-level names.
//!
//! The coverage report does not need a parse tree. It needs the places where a script names a global
//! of the exe (`UI.LookAt`, `Events.SerialEventUnitCreated.Add`, `ToHexFromGrid{ ... }`), so it reads the
//! tokens: comments and the text of strings are skipped, which is where a regular expression goes wrong.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tok {
    Ident(String),
    Str(String),
    Num,
    /// One punctuation character. `..` and `...` are `Concat` and `Ellipsis`, so a `.` is always a field access.
    Punct(char),
    Concat,
    Ellipsis,
}

/// A token and its line (from 1).
pub type Spanned = (Tok, u32);

/// The level of a long bracket at `i` (`[[` is 0, `[==[` is 2), if one opens there.
fn long_open(s: &[u8], i: usize) -> Option<usize> {
    if s.get(i) != Some(&b'[') {
        return None;
    }
    let mut j = i + 1;
    while s.get(j) == Some(&b'=') {
        j += 1;
    }
    (s.get(j) == Some(&b'[')).then_some(j - i - 1)
}

/// The index after the closing bracket of level `level`, starting the search at `from`.
fn long_close(s: &[u8], from: usize, level: usize) -> usize {
    let mut i = from;
    while i < s.len() {
        if s[i] == b']' {
            let mut j = i + 1;
            while s.get(j) == Some(&b'=') {
                j += 1;
            }
            if j - i - 1 == level && s.get(j) == Some(&b']') {
                return j + 1;
            }
        }
        i += 1;
    }
    s.len()
}

fn is_ident_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_'
}

fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

pub fn lex(src: &[u8]) -> Vec<Spanned> {
    let mut out = Vec::new();
    let mut i = 0;
    let mut line = 1u32;
    let count =
        |from: usize, to: usize| src[from..to].iter().filter(|&&c| c == b'\n').count() as u32;
    // A UTF-8 byte order mark.
    if src.starts_with(&[0xef, 0xbb, 0xbf]) {
        i = 3;
    }
    while i < src.len() {
        let c = src[i];
        match c {
            b'\n' => {
                line += 1;
                i += 1;
            }
            b' ' | b'\t' | b'\r' => i += 1,
            b'-' if src.get(i + 1) == Some(&b'-') => {
                if let Some(level) = long_open(src, i + 2) {
                    let end = long_close(src, i + 4 + level, level);
                    line += count(i, end);
                    i = end;
                } else {
                    while i < src.len() && src[i] != b'\n' {
                        i += 1;
                    }
                }
            }
            b'"' | b'\'' => {
                let mut j = i + 1;
                let mut text = Vec::new();
                while j < src.len() && src[j] != c && src[j] != b'\n' {
                    if src[j] == b'\\' && j + 1 < src.len() {
                        j += 1;
                    }
                    text.push(src[j]);
                    j += 1;
                }
                out.push((Tok::Str(String::from_utf8_lossy(&text).into_owned()), line));
                i = (j + 1).min(src.len());
            }
            b'[' if long_open(src, i).is_some() => {
                let level = long_open(src, i).unwrap_or(0);
                let start = i + level + 2;
                let end = long_close(src, start, level);
                let body_end = end.saturating_sub(level + 2).max(start);
                out.push((
                    Tok::Str(
                        String::from_utf8_lossy(&src[start..body_end.min(src.len())]).into_owned(),
                    ),
                    line,
                ));
                line += count(i, end);
                i = end;
            }
            b'0'..=b'9' => {
                let mut j = i + 1;
                while j < src.len()
                    && (is_ident(src[j]) || (src[j] == b'.' && src.get(j + 1) != Some(&b'.')))
                {
                    j += 1;
                }
                out.push((Tok::Num, line));
                i = j;
            }
            b'.' if src.get(i + 1) == Some(&b'.') => {
                if src.get(i + 2) == Some(&b'.') {
                    out.push((Tok::Ellipsis, line));
                    i += 3;
                } else {
                    out.push((Tok::Concat, line));
                    i += 2;
                }
            }
            b'.' if src.get(i + 1).is_some_and(u8::is_ascii_digit) => {
                // A number such as `.5`.
                let mut j = i + 1;
                while j < src.len() && is_ident(src[j]) {
                    j += 1;
                }
                out.push((Tok::Num, line));
                i = j;
            }
            _ if is_ident_start(c) => {
                let mut j = i + 1;
                while j < src.len() && is_ident(src[j]) {
                    j += 1;
                }
                out.push((
                    Tok::Ident(String::from_utf8_lossy(&src[i..j]).into_owned()),
                    line,
                ));
                i = j;
            }
            _ => {
                if c.is_ascii() {
                    out.push((Tok::Punct(c as char), line));
                }
                i += 1;
            }
        }
    }
    out
}

/// How a name is used in a script.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Use {
    /// `Global.Member`, `Global["Member"]`.
    Member(String, String),
    /// `Global:Method(...)`.
    Method(String, String),
    /// `Global` alone (a function called, or a table passed around).
    Bare(String),
}

/// The uses of the names `known` accepts (see [`crate::coverage`]), in order, with their lines.
/// A name that follows `.` or `:` is a field of something else, and one after `local`/`function` is a
/// declaration; neither counts.
pub fn uses(toks: &[Spanned], known: impl Fn(&str) -> bool) -> Vec<(Use, u32)> {
    let mut out = Vec::new();
    let tok = |i: usize| toks.get(i).map(|t| &t.0);
    for (i, (t, line)) in toks.iter().enumerate() {
        let Tok::Ident(name) = t else { continue };
        if !known(name) {
            continue;
        }
        if i > 0 {
            match &toks[i - 1].0 {
                Tok::Punct('.') | Tok::Punct(':') => continue,
                Tok::Ident(k) if k == "local" || k == "function" => continue,
                _ => {}
            }
        }
        // `local a, Name = ...` is a declaration too: look back over a comma list to `local`.
        if declared_by_local(toks, i) {
            continue;
        }
        match (tok(i + 1), tok(i + 2), tok(i + 3)) {
            (Some(Tok::Punct('.')), Some(Tok::Ident(m)), _) => {
                out.push((Use::Member(name.clone(), m.clone()), *line));
            }
            (Some(Tok::Punct('[')), Some(Tok::Str(m)), Some(Tok::Punct(']'))) => {
                out.push((Use::Member(name.clone(), m.clone()), *line));
            }
            (Some(Tok::Punct(':')), Some(Tok::Ident(m)), _) => {
                out.push((Use::Method(name.clone(), m.clone()), *line));
            }
            _ => out.push((Use::Bare(name.clone()), *line)),
        }
    }
    out
}

fn declared_by_local(toks: &[Spanned], i: usize) -> bool {
    let mut j = i;
    while j >= 2 && toks[j - 1].0 == Tok::Punct(',') && matches!(toks[j - 2].0, Tok::Ident(_)) {
        j -= 2;
    }
    j > 0 && matches!(&toks[j - 1].0, Tok::Ident(k) if k == "local")
}

/// The module names `include( "X" )` asks for (the string argument as written).
pub fn includes(toks: &[Spanned]) -> Vec<String> {
    let mut out = Vec::new();
    for (i, (t, _)) in toks.iter().enumerate() {
        if !matches!(t, Tok::Ident(n) if n == "include") {
            continue;
        }
        if i > 0 && matches!(&toks[i - 1].0, Tok::Punct('.') | Tok::Punct(':')) {
            continue;
        }
        let arg = match (toks.get(i + 1).map(|t| &t.0), toks.get(i + 2).map(|t| &t.0)) {
            (Some(Tok::Punct('(')), Some(Tok::Str(s))) => Some(s),
            (Some(Tok::Str(s)), _) => Some(s),
            _ => None,
        };
        if let Some(s) = arg {
            out.push(s.clone());
        }
    }
    out
}

/// The identifiers a script defines: `local X`, `function X`, parameters, `for X`, `X = ...`, table keys.
/// Over-approximate on purpose: it only decides which names are *not* exe-level.
pub fn defined(toks: &[Spanned]) -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    let tok = |i: usize| toks.get(i).map(|t| &t.0);
    for (i, (t, _)) in toks.iter().enumerate() {
        let Tok::Ident(name) = t else { continue };
        let prev = if i > 0 { tok(i - 1) } else { None };
        let after_field = matches!(prev, Some(Tok::Punct('.')) | Some(Tok::Punct(':')));
        // `X = ...` (assignment or table key) but not `X == ...`
        if tok(i + 1) == Some(&Tok::Punct('='))
            && tok(i + 2) != Some(&Tok::Punct('='))
            && !after_field
        {
            set.insert(name.clone());
        }
        match prev {
            Some(Tok::Ident(k)) if k == "local" || k == "function" || k == "for" => {
                set.insert(name.clone());
            }
            Some(Tok::Punct(',')) | Some(Tok::Punct('(')) if inside_binding(toks, i) => {
                set.insert(name.clone());
            }
            _ => {}
        }
    }
    set
}

/// Is token `i` in a list of names after `local`/`for`, or in the parameters of a function?
fn inside_binding(toks: &[Spanned], i: usize) -> bool {
    let mut j = i;
    while j > 0 {
        match &toks[j - 1].0 {
            Tok::Punct(',') | Tok::Punct('(') | Tok::Ident(_) | Tok::Ellipsis => {
                if let Tok::Ident(k) = &toks[j - 1].0
                    && (k == "local" || k == "for" || k == "function")
                {
                    return true;
                }
                // `function Name.Sub:method (a, b)`: walk back over `.`/`:` joined names
                j -= 1;
            }
            Tok::Punct('.') | Tok::Punct(':') => j -= 1,
            _ => return false,
        }
        if i - j > 40 {
            return false;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(src: &str) -> Vec<Use> {
        uses(&lex(src.as_bytes()), |n| {
            matches!(n, "UI" | "Events" | "Mouse" | "ToHexFromGrid" | "UIManager")
        })
        .into_iter()
        .map(|(u, _)| u)
        .collect()
    }

    #[test]
    fn comments_and_strings_do_not_count() {
        let src = r#"
            -- UI.InComment()
            --[[ UI.InBlock() ]]
            --[==[ UI.InLevel2 ]] still ]==]
            local s = "UI.InString" .. 'Events.X' .. [[UI.InLong]]
            UI.Real()
        "#;
        assert_eq!(names(src), vec![Use::Member("UI".into(), "Real".into())]);
    }

    #[test]
    fn members_methods_bare_and_fields_of_other_things() {
        let src = r#"
            local ToHexFromGrid = ToHexFromGrid
            Events.SerialEventUnitCreated.Add(f)
            Events["Other"](1)
            UIManager:QueuePopup(ContextPtr, 1)
            Controls.UI.NotMine()
            obj:Mouse()
            local a, UI = 1, 2
            x = Mouse.eLClick
        "#;
        assert_eq!(
            names(src),
            vec![
                Use::Bare("ToHexFromGrid".into()),
                Use::Member("Events".into(), "SerialEventUnitCreated".into()),
                Use::Member("Events".into(), "Other".into()),
                Use::Method("UIManager".into(), "QueuePopup".into()),
                Use::Member("Mouse".into(), "eLClick".into()),
            ]
        );
    }

    #[test]
    fn includes_with_and_without_parentheses() {
        let toks = lex(br#"include("IconSupport") include "InstanceManager" include( 'FLuaVector.lua' ) x.include("no")"#);
        assert_eq!(
            includes(&toks),
            vec!["IconSupport", "InstanceManager", "FLuaVector.lua"]
        );
    }

    #[test]
    fn defined_names_cover_locals_params_and_assignments() {
        let toks = lex(b"local a, b = 1, 2\nfunction F(x, y, ...) for i, v in pairs(t) do end end\nG = 3\nlocal t = { K = 1 }");
        let d = defined(&toks);
        for n in ["a", "b", "F", "x", "y", "i", "v", "G", "K", "t"] {
            assert!(d.contains(n), "{n} in {d:?}");
        }
    }

    #[test]
    fn lines_are_counted_through_long_strings() {
        let toks = lex(b"a\n[[x\ny]]\nb");
        let lines: Vec<u32> = toks.iter().map(|t| t.1).collect();
        assert_eq!(lines, vec![1, 2, 4]);
    }
}
