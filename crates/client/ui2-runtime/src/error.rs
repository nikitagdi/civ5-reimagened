//! Errors with a node path, logged once.
//!
//! A broken skin never panics the client: the runtime substitutes a placeholder or keeps the old value and
//! reports a [`UiError`]. The [`ErrorSink`] remembers every `(path, code)` it has seen, so an error that repeats
//! every frame is logged and handed to the host exactly once.

use std::collections::HashSet;
use std::fmt;

/// What went wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    /// A template id that the package does not have.
    MissingTemplate,
    /// A style set id that the package does not have.
    MissingStyleSet,
    /// A template instance larger than a panel may be.
    NodeLimit,
    /// A node handle that is no longer alive, or a node of the wrong kind for the call.
    BadNode,
    /// The environment had a value outside its range (replaced by a sane one).
    BadEnv,
    /// No font face of the package matches a text's style, or the text cannot be shaped.
    Font,
}

impl ErrorCode {
    /// A short stable name, for logs and the host's error list.
    pub const fn name(self) -> &'static str {
        match self {
            ErrorCode::MissingTemplate => "missing-template",
            ErrorCode::MissingStyleSet => "missing-style-set",
            ErrorCode::NodeLimit => "node-limit",
            ErrorCode::BadNode => "bad-node",
            ErrorCode::BadEnv => "bad-env",
            ErrorCode::Font => "font",
        }
    }
}

/// Where a node comes from, for messages and rectangle dumps.
///
/// `label` is `<template>:<element index>` with `#k` appended for the k-th text leaf of an element (the format
/// of the converter's rectangle dump); `keys` are the list keys of the instances above the node, outermost
/// first, joined by `/`; `part` is the nearest `data-part` at or above the node and `panel` the outermost one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodePath {
    /// `<template>:<index>[#k]`.
    pub label: String,
    /// Instance keys, outermost first, `/`-separated; empty outside lists.
    pub keys: String,
    /// The nearest part name.
    pub part: Option<String>,
    /// The outermost part name: the panel the node belongs to.
    pub panel: Option<String>,
    // Extension point: the source position (`file:line`) from the package's source map, once the package
    // carries one.
}

impl fmt::Display for NodePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)?;
        if !self.keys.is_empty() {
            write!(f, " [{}]", self.keys)?;
        }
        if let Some(part) = &self.part {
            write!(f, " in part {part}")?;
        }
        Ok(())
    }
}

/// One error of the skin, with the node it concerns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UiError {
    /// The node path, as text (see [`NodePath`]).
    pub path: String,
    /// The panel (outermost part) of the node, when it has one.
    pub panel: Option<String>,
    /// What went wrong.
    pub code: ErrorCode,
    /// Details for a human.
    pub msg: String,
}

impl UiError {
    /// An error about a node.
    pub fn at(path: &NodePath, code: ErrorCode, msg: impl Into<String>) -> Self {
        Self {
            path: path.to_string(),
            panel: path.panel.clone(),
            code,
            msg: msg.into(),
        }
    }

    /// An error that concerns no particular node.
    pub fn global(code: ErrorCode, msg: impl Into<String>) -> Self {
        Self {
            path: String::new(),
            panel: None,
            code,
            msg: msg.into(),
        }
    }
}

impl fmt::Display for UiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            write!(f, "{}: {}", self.code.name(), self.msg)
        } else {
            write!(f, "{} at {}: {}", self.code.name(), self.path, self.msg)
        }
    }
}

/// Collects errors; each `(path, code)` is logged and kept once.
#[derive(Debug, Default)]
pub struct ErrorSink {
    seen: HashSet<(String, ErrorCode)>,
    fresh: Vec<UiError>,
}

impl ErrorSink {
    /// An empty sink.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reports an error. The first report of a `(path, code)` is logged and queued for
    /// [`take`](Self::take); repeats are dropped. Returns whether the error was new.
    pub fn report(&mut self, e: UiError) -> bool {
        if !self.seen.insert((e.path.clone(), e.code)) {
            return false;
        }
        tracing::warn!(target: "ui2", "{e}");
        self.fresh.push(e);
        true
    }

    /// The errors reported for the first time since the last call.
    pub fn take(&mut self) -> Vec<UiError> {
        std::mem::take(&mut self.fresh)
    }

    /// How many distinct errors were reported so far.
    pub fn distinct(&self) -> usize {
        self.seen.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repeated_error_is_kept_once() {
        let mut sink = ErrorSink::new();
        let path = NodePath {
            label: "0:3".into(),
            ..NodePath::default()
        };
        for _ in 0..100 {
            sink.report(UiError::at(&path, ErrorCode::BadNode, "gone"));
        }
        assert_eq!(sink.take().len(), 1);
        assert!(sink.take().is_empty());
        // Another code on the same node is another error.
        assert!(sink.report(UiError::at(&path, ErrorCode::NodeLimit, "big")));
        assert_eq!(sink.distinct(), 2);
    }

    #[test]
    fn paths_print_label_keys_and_part() {
        let p = NodePath {
            label: "1:0#0".into(),
            keys: "0:17/0:3".into(),
            part: Some("top".into()),
            panel: Some("hud".into()),
        };
        assert_eq!(p.to_string(), "1:0#0 [0:17/0:3] in part top");
        let e = UiError::at(&p, ErrorCode::MissingTemplate, "no template 9");
        assert_eq!(e.panel.as_deref(), Some("hud"));
        assert_eq!(
            e.to_string(),
            "missing-template at 1:0#0 [0:17/0:3] in part top: no template 9"
        );
    }
}
