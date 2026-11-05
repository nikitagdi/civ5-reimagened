//! Errors of the safe API (P5: Lua errors are caught in C++ and arrive here as values).

use std::fmt;

/// Kind of Lua error, from the pcall or load status, or the Rust side that raised it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LuaErrorKind {
    /// Raised by a script (`error`, a failed operation) or by `__index` and the like.
    Runtime,
    Syntax,
    Memory,
    ErrorHandler,
    /// Misuse of the API: wrong global, wrong state, bad arguments from Rust.
    Api,
    /// A Lua value did not convert to the requested Rust type, or the other way round.
    Conversion,
    /// Made by a Rust function called from Lua (`LuaError::msg`).
    Rust,
}

/// Lua error: kind, message and, when the error came through a pcall, the stack traceback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaError {
    pub kind: LuaErrorKind,
    /// Message without the traceback. Script errors start with `chunk:line:`.
    pub message: String,
    /// `stack traceback:` text from `debug.traceback`, if the error was caught with one.
    pub traceback: Option<String>,
}

pub type Result<T> = std::result::Result<T, LuaError>;

const TRACEBACK_MARK: &str = "\nstack traceback:";

impl LuaError {
    pub fn new(kind: LuaErrorKind, message: impl Into<String>) -> Self {
        LuaError {
            kind,
            message: message.into(),
            traceback: None,
        }
    }

    /// An error raised by Rust code for Lua: the caller's `file:line:` is prepended when it
    /// reaches the script.
    pub fn msg(message: impl Into<String>) -> Self {
        LuaError::new(LuaErrorKind::Rust, message)
    }

    pub(crate) fn api(message: impl Into<String>) -> Self {
        LuaError::new(LuaErrorKind::Api, message)
    }

    pub(crate) fn conversion(from: &str, to: &str) -> Self {
        LuaError::new(
            LuaErrorKind::Conversion,
            format!("cannot convert {from} to {to}"),
        )
    }

    /// Splits the traceback off a message made by `debug.traceback`.
    pub(crate) fn from_lua_text(kind: LuaErrorKind, text: String) -> Self {
        match text.find(TRACEBACK_MARK) {
            Some(at) => LuaError {
                kind,
                message: text[..at].to_owned(),
                traceback: Some(text[at + 1..].to_owned()),
            },
            None => LuaError::new(kind, text),
        }
    }

    /// The text a Lua error carries when this error is raised again inside Lua: message, then
    /// the traceback so far (the next handler appends its own).
    pub(crate) fn lua_text(&self) -> String {
        match &self.traceback {
            Some(tb) => format!("{}\n{}", self.message, tb),
            None => self.message.clone(),
        }
    }

    /// `(chunk, line)` of the first `chunk:line:` position in the message, as Lua writes it.
    /// `[string "..."]` chunk names are returned as written.
    pub fn location(&self) -> Option<(&str, u32)> {
        let msg = self.message.as_str();
        // Walk over candidate ":<digits>:" positions; chunk names may contain ':' (drive letters).
        let bytes = msg.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b':' {
                let digits = bytes[i + 1..]
                    .iter()
                    .take_while(|b| b.is_ascii_digit())
                    .count();
                if digits > 0 && bytes.get(i + 1 + digits) == Some(&b':') && i > 0 {
                    let line = msg[i + 1..i + 1 + digits].parse().ok()?;
                    return Some((&msg[..i], line));
                }
            }
            i += 1;
        }
        None
    }

    /// Message with the traceback, for logs.
    pub fn full_text(&self) -> String {
        self.lua_text()
    }
}

impl fmt::Display for LuaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Lua {:?}: {}", self.kind, self.message)?;
        if f.alternate()
            && let Some(tb) = &self.traceback
        {
            write!(f, "\n{tb}")?;
        }
        Ok(())
    }
}

impl std::error::Error for LuaError {}

impl From<String> for LuaError {
    fn from(message: String) -> Self {
        LuaError::msg(message)
    }
}

impl From<&str> for LuaError {
    fn from(message: &str) -> Self {
        LuaError::msg(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_traceback_and_finds_location() {
        let e = LuaError::from_lua_text(
            LuaErrorKind::Runtime,
            "UI/Foo.lua:12: boom\nstack traceback:\n\t[C]: in function 'error'".into(),
        );
        assert_eq!(e.message, "UI/Foo.lua:12: boom");
        assert!(
            e.traceback
                .as_deref()
                .unwrap()
                .starts_with("stack traceback:")
        );
        assert_eq!(e.location(), Some(("UI/Foo.lua", 12)));
        assert_eq!(
            e.lua_text(),
            "UI/Foo.lua:12: boom\nstack traceback:\n\t[C]: in function 'error'"
        );
    }

    #[test]
    fn location_edge_cases() {
        let loc = |m: &str| {
            LuaError::new(LuaErrorKind::Runtime, m)
                .location()
                .map(|(c, l)| (c.to_owned(), l))
        };
        assert_eq!(loc("no position"), None);
        assert_eq!(loc("C:\\x\\a.lua:7: bad"), Some(("C:\\x\\a.lua".into(), 7)));
        assert_eq!(
            loc("[string \"x = = 1\"]:1: unexpected"),
            Some(("[string \"x = = 1\"]".into(), 1))
        );
        assert_eq!(loc(":5: x"), None);
    }
}
