//! The action grammar of `data-on-<event>`.

use super::{ProfileError, codes, is_ident, split_top};
use crate::profile::action::{Action, Arg};
use crate::profile::expr;

fn fail(message: impl Into<String>, at: usize) -> ProfileError {
    ProfileError {
        code: codes::ACTION,
        attr: String::new(),
        message: message.into(),
        at: Some(at),
    }
}

/// A dotted name: segments `[A-Za-z_][A-Za-z0-9_-]*` (a hyphen is allowed for mod ids).
fn is_dotted(s: &str) -> bool {
    !s.is_empty()
        && s.split('.').all(|seg| {
            let mut b = seg.bytes();
            b.next()
                .is_some_and(|c| c == b'_' || c.is_ascii_alphabetic())
                && b.all(|c| c == b'_' || c == b'-' || c.is_ascii_alphanumeric())
        })
}

/// `state.a.b` or `ui.a.b`.
fn is_state_path(s: &str) -> bool {
    match s.split_once('.') {
        Some(("state" | "ui", rest)) => rest.split('.').all(is_ident),
        _ => false,
    }
}

/// Parses the value of a `data-on-<event>` attribute. Every error found is returned.
pub(super) fn parse_actions(src: &str) -> Result<Vec<Action>, Vec<ProfileError>> {
    let mut out = Vec::new();
    let mut errs = Vec::new();
    for (off, chunk) in split_top(src, ';') {
        if chunk.trim().is_empty() {
            continue;
        }
        match action(chunk, off) {
            Ok(a) => out.push(a),
            Err(e) => errs.push(e),
        }
    }
    if out.is_empty() && errs.is_empty() {
        errs.push(fail("the attribute has no action", 0));
    }
    if errs.is_empty() { Ok(out) } else { Err(errs) }
}

fn parse_expr(text: &str, at: usize) -> Result<expr::Expr, ProfileError> {
    expr::parse(text).map_err(|e| ProfileError::from_expr("", e, at))
}

fn action(chunk: &str, base: usize) -> Result<Action, ProfileError> {
    let lead = chunk.len() - chunk.trim_start().len();
    let text = chunk.trim();
    let at = base + lead;
    let (word, rest) = match text.split_once(char::is_whitespace) {
        Some((w, r)) => (w, r),
        None => (text, ""),
    };
    let rest_at = at + word.len() + (text.len() - word.len() - rest.trim_start().len());
    let rest = rest.trim();
    match word {
        "stop" if rest.is_empty() => Ok(Action::Stop),
        "stop" => Err(fail("`stop` takes nothing", at)),
        "toggle" => {
            if is_state_path(rest) {
                Ok(Action::Toggle {
                    path: rest.to_string(),
                })
            } else {
                Err(fail("`toggle` needs a path `state.<name>`", at))
            }
        }
        "set" => {
            let Some((path, value)) = rest.split_once('=') else {
                return Err(fail("the form is `set state.<name> = <expr>`", at));
            };
            let path = path.trim();
            if !is_state_path(path) {
                return Err(fail("`set` needs a path `state.<name>`", at));
            }
            let value_at = rest_at + path.len() + (rest.len() - rest.trim_start().len()) + 1;
            let value = parse_expr(
                value.trim_start(),
                value_at + (value.len() - value.trim_start().len()),
            )?;
            Ok(Action::Set {
                path: path.to_string(),
                value,
            })
        }
        "intent" | "call" => {
            let Some(open) = rest.find('(') else {
                return Err(fail("the form is `intent name(args)`", at));
            };
            let Some(inner) = rest[open + 1..].strip_suffix(')') else {
                return Err(fail("the arguments end with `)`", at));
            };
            let name = rest[..open].trim();
            if !is_dotted(name) {
                return Err(fail("the name is a dotted identifier", at));
            }
            let args = args(inner, rest_at + open + 1)?;
            Ok(if word == "intent" {
                Action::Intent {
                    name: name.to_string(),
                    args,
                }
            } else {
                Action::Call {
                    name: name.to_string(),
                    args,
                }
            })
        }
        _ => Err(fail("an action is intent, set, toggle, call or stop", at)),
    }
}

/// `IDENT :` at the start of an argument.
fn named(item: &str) -> Option<(&str, &str)> {
    let (name, rest) = item.split_once(':')?;
    let name = name.trim();
    is_ident(name).then_some((name, rest))
}

fn args(inner: &str, base: usize) -> Result<Vec<Arg>, ProfileError> {
    if inner.trim().is_empty() {
        return Ok(Vec::new());
    }
    let mut out: Vec<Arg> = Vec::new();
    let mut seen_named = false;
    let mut seen_positional = false;
    for (off, item) in split_top(inner, ',') {
        let at = base + off;
        if item.trim().is_empty() {
            return Err(fail("an argument is empty", at));
        }
        let arg = match named(item) {
            Some((name, value)) => {
                seen_named = true;
                if out.iter().any(|a| a.name.as_deref() == Some(name)) {
                    return Err(fail(format!("argument `{name}` is given twice"), at));
                }
                let value_at = at + item.find(':').unwrap_or(0) + 1;
                Arg {
                    name: Some(name.to_string()),
                    value: parse_expr(value, value_at)?,
                }
            }
            None => {
                seen_positional = true;
                Arg {
                    name: None,
                    value: parse_expr(item, at)?,
                }
            }
        };
        out.push(arg);
    }
    if seen_named && seen_positional {
        return Err(fail("arguments are all named or all positional", base));
    }
    Ok(out)
}
