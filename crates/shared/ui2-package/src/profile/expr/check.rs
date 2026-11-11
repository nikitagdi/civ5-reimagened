//! Typing of expressions against the model schema.
//!
//! The type of an expression is a [`Ty`]: a set of alternatives and a `null` flag, because a
//! path through an optional record is nullable, `a ? 1 : 'x'` is an integer or a string, and
//! `null` alone has no alternatives. The checker also reports the model paths an expression
//! reads, which the converter stores in the package (`BIND.used_paths`).

use std::collections::BTreeMap;
use std::fmt;

use super::{BinOp, Expr, ExprError, Func, Template, TplSeg, UnOp, codes};
use crate::profile::{SchemaView, TypeSig};

/// A variable of a scope: a loop item or index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Var {
    /// The type of the variable.
    pub ty: TypeSig,
    /// The model path the variable stands for (`top.cities[]` for the item of a loop over
    /// `top.cities`), when it comes from the model.
    pub origin: Option<String>,
}

/// The names an expression may use besides the model roots.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scope {
    /// Loop variables, by name.
    pub vars: BTreeMap<String, Var>,
    /// The skin state: the path below `state.` (`prod.sort`) and its type.
    pub state: BTreeMap<String, TypeSig>,
    /// Further whole paths with their types (`mods.<id>.score` in a mod fragment).
    pub extra: BTreeMap<String, TypeSig>,
}

/// Names a loop variable may not take, besides the model roots.
const RESERVED: &[&str] = &["true", "false", "null", "state", "mods", "modstate"];

impl Scope {
    /// An empty scope.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a state path.
    pub fn with_state(mut self, path: &str, ty: TypeSig) -> Self {
        self.state.insert(path.to_string(), ty);
        self
    }

    /// Adds a whole path (outside the schema) with its type.
    pub fn with_extra(mut self, path: &str, ty: TypeSig) -> Self {
        self.extra.insert(path.to_string(), ty);
        self
    }

    /// Adds a variable.
    pub fn with_var(mut self, name: &str, ty: TypeSig, origin: Option<String>) -> Self {
        self.vars.insert(name.to_string(), Var { ty, origin });
        self
    }

    /// The scope of the children of a `for` node: the item (and the index) added. `list` is the
    /// analysis of the list expression.
    pub fn for_scope(
        &self,
        item: &str,
        index: Option<&str>,
        list: &Analysis,
        view: &dyn SchemaView,
    ) -> Result<Scope, ExprError> {
        let elem = match (list.ty.alts.as_slice(), list.ty.nullable) {
            ([TypeSig::List(elem)], _) => (**elem).clone(),
            _ => {
                return Err(ExprError::new(
                    codes::NOT_LIST,
                    format!("`for` needs a list, the expression is {}", list.ty),
                ));
            }
        };
        for name in std::iter::once(item).chain(index) {
            let ident = name.bytes().enumerate().all(|(i, c)| {
                c == b'_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit())
            });
            if name.is_empty() || !ident {
                return Err(ExprError::new(
                    codes::VAR_NAME,
                    format!("`{name}` is not a valid variable name"),
                ));
            }
            if RESERVED.contains(&name) || view.root(name).is_some() || self.vars.contains_key(name)
            {
                return Err(ExprError::new(
                    codes::VAR_NAME,
                    format!("`{name}` is already a root, a keyword or a variable"),
                ));
            }
        }
        if index == Some(item) {
            return Err(ExprError::new(
                codes::VAR_NAME,
                "the item and the index have the same name",
            ));
        }
        let mut scope = self.clone();
        scope.vars.insert(
            item.to_string(),
            Var {
                ty: elem,
                origin: list.origin.as_ref().map(|o| format!("{o}[]")),
            },
        );
        if let Some(index) = index {
            scope.vars.insert(
                index.to_string(),
                Var {
                    ty: TypeSig::Int,
                    origin: None,
                },
            );
        }
        Ok(scope)
    }
}

/// The type of an expression: what it can be, and whether it can be `null`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ty {
    /// Whether the value can be `null`.
    pub nullable: bool,
    /// The types a non-null value can have, ascending and without repeats. Never `Opt`. Empty
    /// for the literal `null`.
    pub alts: Vec<TypeSig>,
}

impl Ty {
    /// The type of the literal `null`.
    pub fn null() -> Ty {
        Ty {
            nullable: true,
            alts: Vec::new(),
        }
    }

    /// A schema type (an `Opt` becomes the nullable flag).
    pub fn of(sig: TypeSig) -> Ty {
        match sig {
            TypeSig::Opt(inner) => Ty {
                nullable: true,
                alts: vec![*inner],
            },
            other => Ty {
                nullable: false,
                alts: vec![other],
            },
        }
    }

    fn nullable(mut self, yes: bool) -> Ty {
        self.nullable |= yes;
        self
    }

    /// The type as one `TypeSig`, when there is exactly one alternative (`Opt` when nullable).
    pub fn to_sig(&self) -> Option<TypeSig> {
        match self.alts.as_slice() {
            [one] if self.nullable => Some(TypeSig::Opt(Box::new(one.clone()))),
            [one] => Some(one.clone()),
            _ => None,
        }
    }

    fn join(&self, other: &Ty) -> Ty {
        let mut alts = self.alts.clone();
        alts.extend(other.alts.iter().cloned());
        alts.sort();
        alts.dedup();
        Ty {
            nullable: self.nullable || other.nullable,
            alts,
        }
    }

    fn all(&self, f: impl Fn(&TypeSig) -> bool) -> bool {
        self.alts.iter().all(f)
    }

    fn numeric(&self) -> bool {
        self.all(|a| matches!(a, TypeSig::Int | TypeSig::Float))
    }

    fn integral(&self) -> bool {
        self.all(|a| matches!(a, TypeSig::Int))
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.alts.is_empty() {
            return f.write_str("null");
        }
        for (i, a) in self.alts.iter().enumerate() {
            if i > 0 {
                f.write_str("|")?;
            }
            write!(f, "{a}")?;
        }
        if self.nullable {
            f.write_str("?")?;
        }
        Ok(())
    }
}

/// What the checker found out about an expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Analysis {
    /// The type.
    pub ty: Ty,
    /// The model paths read, with the types the schema gives them (`top.units[].hp`), ascending
    /// and without repeats.
    pub paths: Vec<(String, TypeSig)>,
    /// When the expression is a bare path into the model, that path (for a loop over it).
    pub origin: Option<String>,
}

/// The type of an expression.
pub fn typecheck(e: &Expr, scope: &Scope, view: &dyn SchemaView) -> Result<Ty, ExprError> {
    analyze(e, scope, view).map(|a| a.ty)
}

/// The type of an expression and the model paths it reads.
pub fn analyze(e: &Expr, scope: &Scope, view: &dyn SchemaView) -> Result<Analysis, ExprError> {
    let mut c = Checker {
        scope,
        view,
        paths: Vec::new(),
    };
    let ty = c.check(e)?;
    let origin = match (e, c.paths.as_slice()) {
        (Expr::Path(_), [(p, _)]) => Some(p.clone()),
        _ => None,
    };
    c.paths.sort();
    c.paths.dedup();
    Ok(Analysis {
        ty,
        paths: c.paths,
        origin,
    })
}

/// Checks a template: every insert must type, and none may be a bool, a list or a record.
/// Returns the model paths the inserts read.
pub fn check_template(
    t: &Template,
    scope: &Scope,
    view: &dyn SchemaView,
) -> Result<Vec<(String, TypeSig)>, ExprError> {
    let mut paths = Vec::new();
    for seg in &t.segs {
        let TplSeg::Expr(e) = seg else { continue };
        let a = analyze(e, scope, view)?;
        if let Some(bad) =
            a.ty.alts
                .iter()
                .find(|a| matches!(a, TypeSig::Bool | TypeSig::List(_) | TypeSig::Struct(_)))
        {
            return Err(ExprError::new(
                codes::TEXT_TYPE,
                format!("a {bad} cannot be written into text"),
            ));
        }
        paths.extend(a.paths);
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

struct Checker<'a> {
    scope: &'a Scope,
    view: &'a dyn SchemaView,
    paths: Vec<(String, TypeSig)>,
}

fn unknown(msg: String) -> ExprError {
    ExprError::new(codes::UNKNOWN_PATH, msg)
}

impl Checker<'_> {
    fn check(&mut self, e: &Expr) -> Result<Ty, ExprError> {
        match e {
            Expr::Null => Ok(Ty::null()),
            Expr::Bool(_) => Ok(Ty::of(TypeSig::Bool)),
            Expr::Int(_) => Ok(Ty::of(TypeSig::Int)),
            Expr::Float(_) => Ok(Ty::of(TypeSig::Float)),
            Expr::Str(_) => Ok(Ty::of(TypeSig::Str)),
            Expr::Path(p) => self.path(p),
            Expr::Unary(UnOp::Not, a) => {
                self.check(a)?;
                Ok(Ty::of(TypeSig::Bool))
            }
            Expr::Unary(UnOp::Neg, a) => {
                let t = self.check(a)?;
                if !t.numeric() {
                    return Err(not_number("-", &t));
                }
                Ok(t)
            }
            Expr::Binary(op, a, b) => self.binary(*op, a, b),
            Expr::Cond(c, a, b) => {
                self.check(c)?;
                let (ta, tb) = (self.check(a)?, self.check(b)?);
                Ok(ta.join(&tb))
            }
            Expr::Call(f, args) => self.call(*f, args),
        }
    }

    fn path(&mut self, segs: &[String]) -> Result<Ty, ExprError> {
        let Some(first) = segs.first() else {
            return Err(unknown("an empty path".into()));
        };
        let (mut cur, mut origin, rest) = if let Some(v) = self.scope.vars.get(first) {
            (v.ty.clone(), v.origin.clone(), &segs[1..])
        } else if first == "state" {
            let key = segs[1..].join(".");
            return match self.scope.state.get(&key) {
                Some(t) => Ok(Ty::of(t.clone())),
                None => Err(unknown(format!(
                    "`{}` is not in the skin state",
                    segs.join(".")
                ))),
            };
        } else if let Some(t) = self.scope.extra.get(&segs.join(".")) {
            return Ok(Ty::of(t.clone()));
        } else if let Some(t) = self.view.root(first) {
            (t, Some(first.clone()), &segs[1..])
        } else {
            return Err(unknown(format!(
                "`{first}` is not a model root or a variable"
            )));
        };
        let mut nullable = false;
        for name in rest {
            if let TypeSig::Opt(inner) = cur {
                nullable = true;
                cur = *inner;
            }
            match &cur {
                TypeSig::List(_) => {
                    return Err(ExprError::new(
                        codes::LIST_FIELD,
                        format!(
                            "`{}` is a list and has no field `{name}`; loop over it with data-bind-for",
                            segs.join(".")
                        ),
                    ));
                }
                TypeSig::Struct(_) => {}
                other => {
                    return Err(unknown(format!(
                        "a value of type {other} has no field `{name}`"
                    )));
                }
            }
            let Some(next) = self.view.field(&cur, name) else {
                return Err(unknown(format!(
                    "`{}` has no field `{name}`",
                    segs.join(".")
                )));
            };
            origin = origin.map(|o| format!("{o}.{name}"));
            cur = next;
        }
        if let Some(o) = origin {
            self.paths.push((o, cur.clone()));
        }
        Ok(Ty::of(cur).nullable(nullable))
    }

    fn binary(&mut self, op: BinOp, a: &Expr, b: &Expr) -> Result<Ty, ExprError> {
        let (l, r) = (self.check(a)?, self.check(b)?);
        let sign = match op {
            BinOp::Or => "||",
            BinOp::And => "&&",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
        };
        match op {
            BinOp::Or | BinOp::And => Ok(Ty::of(TypeSig::Bool)),
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
                for t in [&l, &r] {
                    if !t.numeric() {
                        return Err(not_number(sign, t));
                    }
                }
                let ints = l.integral() && r.integral();
                let pure_null = l.alts.is_empty() && r.alts.is_empty();
                let nullable = l.nullable || r.nullable || matches!(op, BinOp::Div | BinOp::Rem);
                let alts = if pure_null {
                    Vec::new()
                } else if ints && op != BinOp::Div {
                    vec![TypeSig::Int]
                } else {
                    vec![TypeSig::Float]
                };
                Ok(Ty { nullable, alts })
            }
            BinOp::Eq | BinOp::Ne => {
                self.equality(&l, &r, a, b)?;
                Ok(Ty::of(TypeSig::Bool))
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                for x in &l.alts {
                    for y in &r.alts {
                        let ok = matches!(
                            (x, y),
                            (TypeSig::Int | TypeSig::Float, TypeSig::Int | TypeSig::Float)
                                | (TypeSig::Str, TypeSig::Str)
                        );
                        if !ok {
                            return Err(ExprError::new(
                                codes::CANNOT_COMPARE,
                                format!("`{sign}` cannot order {x} against {y}"),
                            ));
                        }
                    }
                }
                Ok(Ty::of(TypeSig::Bool))
            }
        }
    }

    fn equality(&self, l: &Ty, r: &Ty, a: &Expr, b: &Expr) -> Result<(), ExprError> {
        for x in &l.alts {
            for y in &r.alts {
                let ok = match (x, y) {
                    (TypeSig::Int | TypeSig::Float, TypeSig::Int | TypeSig::Float)
                    | (TypeSig::Bool, TypeSig::Bool)
                    | (TypeSig::Str, TypeSig::Str)
                    | (TypeSig::Color, TypeSig::Color)
                    | (TypeSig::Enum(_), TypeSig::Str)
                    | (TypeSig::Str, TypeSig::Enum(_)) => true,
                    (TypeSig::Key(p), TypeSig::Key(q)) => p == q,
                    (TypeSig::Enum(p), TypeSig::Enum(q)) => p == q,
                    _ => false,
                };
                if !ok {
                    return Err(ExprError::new(
                        codes::CANNOT_COMPARE,
                        format!("{x} cannot be compared with {y}"),
                    ));
                }
            }
        }
        for (lit, other_ty) in [(a, r), (b, l)] {
            let Expr::Str(s) = lit else { continue };
            let enums: Vec<&Vec<String>> = other_ty
                .alts
                .iter()
                .filter_map(|t| match t {
                    TypeSig::Enum(v) => Some(v),
                    _ => None,
                })
                .collect();
            if enums.len() == other_ty.alts.len() {
                for values in enums {
                    if !values.contains(s) {
                        return Err(ExprError::new(
                            codes::ENUM_VALUE,
                            format!("'{s}' is not one of {}", values.join(", ")),
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    fn call(&mut self, f: Func, args: &[Expr]) -> Result<Ty, ExprError> {
        if !f.accepts(args.len()) {
            return Err(ExprError::new(
                codes::ARGC,
                format!("`{}` does not take {} arguments", f.name(), args.len()),
            ));
        }
        let mut tys = Vec::with_capacity(args.len());
        for a in args {
            tys.push(self.check(a)?);
        }
        let bad_arg = |i: usize, want: &str| {
            ExprError::new(
                codes::ARG_TYPE,
                format!(
                    "argument {} of `{}` must be {want}, it is {}",
                    i + 1,
                    f.name(),
                    tys[i]
                ),
            )
        };
        let any_null = tys.iter().any(|t| t.nullable);
        let result = |sig: TypeSig| Ty::of(sig).nullable(any_null);
        match f {
            Func::Loc => {
                if !tys[0].all(|a| matches!(a, TypeSig::Str)) {
                    return Err(bad_arg(0, "a string"));
                }
                for (i, t) in tys.iter().enumerate().skip(1) {
                    if t.alts
                        .iter()
                        .any(|a| matches!(a, TypeSig::List(_) | TypeSig::Struct(_)))
                    {
                        return Err(bad_arg(i, "a scalar"));
                    }
                }
                Ok(Ty::of(TypeSig::Text))
            }
            Func::Len => {
                if !tys[0].all(|a| matches!(a, TypeSig::List(_))) {
                    return Err(bad_arg(0, "a list"));
                }
                Ok(result(TypeSig::Int))
            }
            _ => {
                for (i, t) in tys.iter().enumerate() {
                    if f == Func::FmtFixed && i == 1 {
                        if !t.integral() {
                            return Err(bad_arg(1, "an integer"));
                        }
                    } else if !t.numeric() {
                        return Err(bad_arg(i, "a number"));
                    }
                }
                let ints = tys.iter().all(Ty::integral);
                Ok(match f {
                    Func::FmtInt
                    | Func::FmtSigned
                    | Func::FmtFixed
                    | Func::FmtPct
                    | Func::FmtMmss => result(TypeSig::Str),
                    Func::Round | Func::Floor | Func::Ceil => result(TypeSig::Int),
                    _ if ints && tys.iter().all(|t| !t.alts.is_empty()) => result(TypeSig::Int),
                    _ => result(TypeSig::Float),
                })
            }
        }
    }
}

fn not_number(op: &str, t: &Ty) -> ExprError {
    ExprError::new(
        codes::NOT_NUMBER,
        format!("`{op}` needs numbers, an operand is {t}"),
    )
}
