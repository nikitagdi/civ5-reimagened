//! The lexer: source text to tokens with byte spans.

use super::{ExprError, codes};

/// A token.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Tok {
    Int(i64),
    Float(f64),
    Str(String),
    Ident(String),
    /// An operator or punctuation, as written.
    Op(&'static str),
}

/// A token and where it is.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Token {
    pub tok: Tok,
    pub start: usize,
    pub end: usize,
}

const TWO: &[&str] = &["||", "&&", "==", "!=", "<=", ">="];
const ONE: &[&str] = &[
    "<", ">", "+", "-", "*", "/", "%", "!", "?", ":", "(", ")", ",", ".",
];

fn syntax(at: usize, message: impl Into<String>) -> ExprError {
    ExprError::at(codes::SYNTAX, at, message)
}

/// Splits `src` into tokens.
pub(super) fn lex(src: &str) -> Result<Vec<Token>, ExprError> {
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if matches!(c, b' ' | b'\t' | b'\r' | b'\n') {
            i += 1;
            continue;
        }
        let start = i;
        if c.is_ascii_digit() {
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            let mut fractional = false;
            if bytes.get(i) == Some(&b'.') && bytes.get(i + 1).is_some_and(u8::is_ascii_digit) {
                fractional = true;
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
            }
            let text = &src[start..i];
            let tok = if fractional {
                match text.parse::<f64>() {
                    Ok(f) if f.is_finite() => Tok::Float(f),
                    _ => return Err(syntax(start, "number out of range")),
                }
            } else {
                match text.parse::<i64>() {
                    Ok(n) => Tok::Int(n),
                    Err(_) => return Err(syntax(start, "number out of range")),
                }
            };
            out.push(Token { tok, start, end: i });
            continue;
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            out.push(Token {
                tok: Tok::Ident(src[start..i].to_string()),
                start,
                end: i,
            });
            continue;
        }
        if c == b'\'' {
            i += 1;
            let mut text = String::new();
            loop {
                let Some(rest) = src.get(i..) else {
                    return Err(syntax(start, "unterminated string"));
                };
                let Some(ch) = rest.chars().next() else {
                    return Err(syntax(start, "unterminated string"));
                };
                i += ch.len_utf8();
                match ch {
                    '\'' => break,
                    '\\' => match src.get(i..).and_then(|r| r.chars().next()) {
                        Some(e @ ('\'' | '\\')) => {
                            text.push(e);
                            i += 1;
                        }
                        _ => return Err(syntax(i - 1, "only \\' and \\\\ are escapes")),
                    },
                    other => text.push(other),
                }
            }
            out.push(Token {
                tok: Tok::Str(text),
                start,
                end: i,
            });
            continue;
        }
        let two = src
            .get(i..i + 2)
            .and_then(|s| TWO.iter().find(|t| **t == s));
        if let Some(op) = two {
            out.push(Token {
                tok: Tok::Op(op),
                start,
                end: i + 2,
            });
            i += 2;
            continue;
        }
        let one = src
            .get(i..i + 1)
            .and_then(|s| ONE.iter().find(|t| **t == s));
        if let Some(op) = one {
            out.push(Token {
                tok: Tok::Op(op),
                start,
                end: i + 1,
            });
            i += 1;
            continue;
        }
        let ch = src.get(i..).and_then(|r| r.chars().next()).unwrap_or('?');
        return Err(syntax(i, format!("unexpected character {ch:?}")));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Tok> {
        lex(s).expect("lex").into_iter().map(|t| t.tok).collect()
    }

    #[test]
    fn numbers_idents_and_operators() {
        assert_eq!(
            toks("a.b >= 1.5"),
            [
                Tok::Ident("a".into()),
                Tok::Op("."),
                Tok::Ident("b".into()),
                Tok::Op(">="),
                Tok::Float(1.5)
            ]
        );
        assert_eq!(toks("1.x")[..2], [Tok::Int(1), Tok::Op(".")]);
        assert_eq!(toks("'a\\'b\\\\'"), [Tok::Str("a'b\\".into())]);
    }

    #[test]
    fn bad_input_is_a_syntax_error() {
        for bad in [
            "'open",
            "'\\n'",
            "a | b",
            "a = b",
            "#",
            "99999999999999999999",
        ] {
            assert_eq!(lex(bad).expect_err(bad).code, codes::SYNTAX, "{bad}");
        }
    }
}
