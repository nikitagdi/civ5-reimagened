//! The expression language of bindings and its templates.
//!
//! ```text
//! expr    := or ( '?' expr ':' expr )?
//! or      := and ( '||' and )*            and  := cmp ( '&&' cmp )*
//! cmp     := sum ( ('=='|'!='|'<'|'<='|'>'|'>=') sum )?
//! sum     := prod ( ('+'|'-') prod )*     prod := unary ( ('*'|'/'|'%') unary )*
//! unary   := ('!'|'-') unary | atom
//! atom    := NUMBER | STRING | 'true' | 'false' | 'null' | path | call | '(' expr ')'
//! path    := IDENT ( '.' IDENT )*         call := FNAME '(' ( expr ( ',' expr )* )? ')'
//! NUMBER  := [0-9]+ ( '.' [0-9]+ )?       STRING := '...' with \' and \\ as the only escapes
//! tpl     := ( text | '{{' expr '}}' )*   -- an insert ends at the first '}}' outside a string
//! ```
//!
//! The syntax tree [`Expr`] and the evaluator ([`eval`], [`Value`], [`EvalHost`]) are always
//! available: the package stores trees and the runtime runs them. The lexer, the parser and the
//! type checker belong to the converter and need feature `profile`. The semantics (types, `null`,
//! number formatting, the functions) are pinned by `profile/expr-vectors.json`, which the Rust
//! tests, the converter's preview script and the runtime all run.

use std::cell::Cell;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};

mod eval;

#[cfg(feature = "profile")]
mod check;
#[cfg(feature = "profile")]
mod lex;
#[cfg(feature = "profile")]
mod parse;

#[cfg(feature = "profile")]
pub use check::{Analysis, Scope, Ty, Var, analyze, check_template, typecheck};
pub use eval::{EvalHost, Value, eval, fmt_fixed, fmt_float, render, truthy};
#[cfg(feature = "profile")]
pub(crate) use parse::find_close;
#[cfg(feature = "profile")]
pub use parse::{parse, parse_template};

/// Error codes of the expression language; the texts are in `profile.toml` (`error.<code>`).
pub mod codes {
    /// A lexical or grammatical error.
    pub const SYNTAX: &str = "E4-001";
    /// A call of a function the language does not have.
    pub const UNKNOWN_FUNC: &str = "E4-002";
    /// A call with too few or too many arguments.
    pub const ARGC: &str = "E4-003";
    /// A path that is not a root, a variable, a state path or a field of the model.
    pub const UNKNOWN_PATH: &str = "E4-004";
    /// Arithmetic or a numeric function over something that is not a number.
    pub const NOT_NUMBER: &str = "E4-005";
    /// Operands of different or incomparable types.
    pub const CANNOT_COMPARE: &str = "E4-006";
    /// A string compared with an enum that has no such value.
    pub const ENUM_VALUE: &str = "E4-007";
    /// A bool, a list or a record inserted into text.
    pub const TEXT_TYPE: &str = "E4-008";
    /// A `for` over something that is not a list.
    pub const NOT_LIST: &str = "E4-009";
    /// A loop variable named like a root, a state namespace or an enclosing variable.
    pub const VAR_NAME: &str = "E4-010";
    /// A function argument of the wrong type.
    pub const ARG_TYPE: &str = "E4-011";
    /// A field read from a list instead of from its elements.
    pub const LIST_FIELD: &str = "E4-012";
    /// A `{{` without its `}}`.
    pub const UNCLOSED: &str = "E4-013";

    /// Every code of this module.
    pub const ALL: &[&str] = &[
        SYNTAX,
        UNKNOWN_FUNC,
        ARGC,
        UNKNOWN_PATH,
        NOT_NUMBER,
        CANNOT_COMPARE,
        ENUM_VALUE,
        TEXT_TYPE,
        NOT_LIST,
        VAR_NAME,
        ARG_TYPE,
        LIST_FIELD,
        UNCLOSED,
    ];
}

/// The deepest nesting of a tree. The parser refuses more, and so does the decoder of a package,
/// so no input can overflow the stack of the reader or of the evaluator.
pub const MAX_DEPTH: usize = 128;

/// An error of the language, with the byte offset in the source when there is one.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {message}")]
pub struct ExprError {
    /// The code (`E4-...`).
    pub code: &'static str,
    /// What was wrong with this expression.
    pub message: String,
    /// Where in the source text, for lexical and grammatical errors.
    pub at: Option<usize>,
}

impl ExprError {
    /// An error without a position.
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            at: None,
        }
    }

    /// An error at a byte offset.
    pub fn at(code: &'static str, at: usize, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            at: Some(at),
        }
    }

    /// The same error with the position moved by `base` (an expression found inside a longer
    /// attribute value).
    pub fn shifted(mut self, base: usize) -> Self {
        self.at = self.at.map(|a| a + base);
        self
    }
}

/// A prefix operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnOp {
    /// `!`
    Not,
    /// `-`
    Neg,
}

/// An infix operator. Postcard writes the variant index: new variants go at the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinOp {
    /// `||`
    Or,
    /// `&&`
    And,
    /// `==`
    Eq,
    /// `!=`
    Ne,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
    /// `%`
    Rem,
}

/// A function of the language. Postcard writes the variant index: new functions go at the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Func {
    /// `fmt.int(x)`
    FmtInt,
    /// `fmt.signed(x)`
    FmtSigned,
    /// `fmt.fixed(x, n)`
    FmtFixed,
    /// `fmt.pct(x)`
    FmtPct,
    /// `fmt.mmss(s)`
    FmtMmss,
    /// `loc(key, args...)`
    Loc,
    /// `len(list)`
    Len,
    /// `min(a, b, ...)`
    Min,
    /// `max(a, b, ...)`
    Max,
    /// `clamp(x, lo, hi)`
    Clamp,
    /// `round(x)`
    Round,
    /// `floor(x)`
    Floor,
    /// `ceil(x)`
    Ceil,
    /// `abs(x)`
    Abs,
}

impl Func {
    /// Every function, in wire order.
    pub const ALL: &'static [Func] = &[
        Func::FmtInt,
        Func::FmtSigned,
        Func::FmtFixed,
        Func::FmtPct,
        Func::FmtMmss,
        Func::Loc,
        Func::Len,
        Func::Min,
        Func::Max,
        Func::Clamp,
        Func::Round,
        Func::Floor,
        Func::Ceil,
        Func::Abs,
    ];

    /// The name as written in an expression.
    pub const fn name(self) -> &'static str {
        match self {
            Func::FmtInt => "fmt.int",
            Func::FmtSigned => "fmt.signed",
            Func::FmtFixed => "fmt.fixed",
            Func::FmtPct => "fmt.pct",
            Func::FmtMmss => "fmt.mmss",
            Func::Loc => "loc",
            Func::Len => "len",
            Func::Min => "min",
            Func::Max => "max",
            Func::Clamp => "clamp",
            Func::Round => "round",
            Func::Floor => "floor",
            Func::Ceil => "ceil",
            Func::Abs => "abs",
        }
    }

    /// The function of a name.
    pub fn from_name(name: &str) -> Option<Func> {
        Func::ALL.iter().copied().find(|f| f.name() == name)
    }

    /// The fewest and the most arguments (`None`: no upper bound).
    pub const fn arity(self) -> (usize, Option<usize>) {
        match self {
            Func::FmtInt
            | Func::FmtSigned
            | Func::FmtPct
            | Func::FmtMmss
            | Func::Len
            | Func::Round
            | Func::Floor
            | Func::Ceil
            | Func::Abs => (1, Some(1)),
            Func::FmtFixed => (2, Some(2)),
            Func::Clamp => (3, Some(3)),
            Func::Min | Func::Max => (2, None),
            Func::Loc => (1, None),
        }
    }

    /// Whether `n` arguments are acceptable.
    pub fn accepts(self, n: usize) -> bool {
        let (lo, hi) = self.arity();
        n >= lo && hi.is_none_or(|h| n <= h)
    }
}

thread_local! {
    static DEPTH: Cell<usize> = const { Cell::new(0) };
}

/// Decodes a child of a tree, counting the nesting so a damaged package cannot recurse without
/// bound.
fn nested<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    DEPTH.with(|depth| {
        let level = depth.get();
        if level >= MAX_DEPTH {
            return Err(D::Error::custom("expression nested too deeply"));
        }
        depth.set(level + 1);
        let result = T::deserialize(d);
        depth.set(level);
        result
    })
}

fn de_box<'de, D: Deserializer<'de>>(d: D) -> Result<Box<Expr>, D::Error> {
    nested::<_, Expr>(d).map(Box::new)
}

fn de_vec<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Expr>, D::Error> {
    nested::<_, Vec<Expr>>(d)
}

/// The syntax tree of an expression. Postcard writes the variant index: new variants go at the
/// end. Floats are finite (the parser cannot make anything else; the package reader checks).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Expr {
    /// `null`
    Null,
    /// `true` or `false`
    Bool(bool),
    /// An integer literal.
    Int(i64),
    /// A literal with a fractional part.
    Float(f64),
    /// A string literal.
    Str(String),
    /// A path: a model root, a state path or a variable, then fields. Never empty.
    Path(Vec<String>),
    /// A prefix operator.
    Unary(UnOp, #[serde(deserialize_with = "de_box")] Box<Expr>),
    /// An infix operator.
    Binary(
        BinOp,
        #[serde(deserialize_with = "de_box")] Box<Expr>,
        #[serde(deserialize_with = "de_box")] Box<Expr>,
    ),
    /// `cond ? then : otherwise`
    Cond(
        #[serde(deserialize_with = "de_box")] Box<Expr>,
        #[serde(deserialize_with = "de_box")] Box<Expr>,
        #[serde(deserialize_with = "de_box")] Box<Expr>,
    ),
    /// A function call.
    Call(Func, #[serde(deserialize_with = "de_vec")] Vec<Expr>),
}

impl Expr {
    /// The nesting depth of the tree (a leaf is 1).
    pub fn depth(&self) -> usize {
        match self {
            Expr::Unary(_, a) => 1 + a.depth(),
            Expr::Binary(_, a, b) => 1 + a.depth().max(b.depth()),
            Expr::Cond(a, b, c) => 1 + a.depth().max(b.depth()).max(c.depth()),
            Expr::Call(_, args) => 1 + args.iter().map(Expr::depth).max().unwrap_or(0),
            _ => 1,
        }
    }

    /// Whether the tree is well formed for a reader: not nested deeper than [`MAX_DEPTH`], floats
    /// finite, paths made of non-empty names, calls with an acceptable number of arguments.
    pub fn is_sound(&self) -> bool {
        if self.depth() > MAX_DEPTH {
            return false;
        }
        self.sound_below()
    }

    fn sound_below(&self) -> bool {
        match self {
            Expr::Float(f) => f.is_finite(),
            Expr::Path(p) => !p.is_empty() && p.iter().all(|s| !s.is_empty()),
            Expr::Unary(_, a) => a.sound_below(),
            Expr::Binary(_, a, b) => a.sound_below() && b.sound_below(),
            Expr::Cond(a, b, c) => a.sound_below() && b.sound_below() && c.sound_below(),
            Expr::Call(f, args) => f.accepts(args.len()) && args.iter().all(Expr::sound_below),
            _ => true,
        }
    }

    /// Calls `f` on every path of the tree, in source order.
    pub fn for_each_path(&self, f: &mut impl FnMut(&[String])) {
        match self {
            Expr::Path(p) => f(p),
            Expr::Unary(_, a) => a.for_each_path(f),
            Expr::Binary(_, a, b) => {
                a.for_each_path(f);
                b.for_each_path(f);
            }
            Expr::Cond(a, b, c) => {
                a.for_each_path(f);
                b.for_each_path(f);
                c.for_each_path(f);
            }
            Expr::Call(_, args) => args.iter().for_each(|a| a.for_each_path(f)),
            _ => {}
        }
    }
}

/// One piece of a text template.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TplSeg {
    /// Text copied as it is.
    Lit(String),
    /// An insert `{{ expr }}`.
    Expr(Expr),
}

/// A text with `{{ expr }}` inserts. Neighbouring literals are one piece.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Template {
    /// The pieces, in order.
    pub segs: Vec<TplSeg>,
}

impl Template {
    /// Whether the template has no insert.
    pub fn is_static(&self) -> bool {
        self.segs.iter().all(|s| matches!(s, TplSeg::Lit(_)))
    }

    /// The text of a template without inserts.
    pub fn as_static(&self) -> Option<String> {
        let mut out = String::new();
        for seg in &self.segs {
            match seg {
                TplSeg::Lit(s) => out.push_str(s),
                TplSeg::Expr(_) => return None,
            }
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn function_names_round_trip() {
        for &f in Func::ALL {
            assert_eq!(Func::from_name(f.name()), Some(f));
            assert!(f.accepts(f.arity().0));
        }
        assert_eq!(Func::from_name("fmt.nope"), None);
        assert!(!Func::Clamp.accepts(2));
        assert!(Func::Min.accepts(5));
    }

    #[test]
    fn a_deep_tree_is_refused_by_the_decoder() {
        let mut e = Expr::Int(1);
        for _ in 0..MAX_DEPTH + 10 {
            e = Expr::Unary(UnOp::Neg, Box::new(e));
        }
        assert!(!e.is_sound());
        let bytes = postcard::to_allocvec(&e).expect("encode");
        assert!(postcard::from_bytes::<Expr>(&bytes).is_err());

        let mut ok = Expr::Int(1);
        for _ in 0..MAX_DEPTH - 2 {
            ok = Expr::Unary(UnOp::Neg, Box::new(ok));
        }
        assert!(ok.is_sound());
        let bytes = postcard::to_allocvec(&ok).expect("encode");
        assert_eq!(postcard::from_bytes::<Expr>(&bytes), Ok(ok));
    }
}
