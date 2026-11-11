//! Evaluating binding expressions against the model: loop variables, the evaluator's host, model values as
//! expression values, text templates.

use ui2_model::{Key, ModelStore, Scalar, Schema, Type, Value as MValue};
use ui2_package::profile::expr::{EvalHost, Value, eval, render};
use ui2_package::{Seg, TplId};

use crate::bind::deps::{Plan, Slot};
use crate::loc::SkinLoc;

/// What a loop variable stands for.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum VarVal {
    /// The item of a `for`: the model path of the list's elements (`top.resources[]`; `None` when the list
    /// is not a model path) and the keys that name this element, outermost first.
    Item {
        origin: Option<String>,
        keys: Vec<Key>,
    },
    /// The index of a `for` item in the shown order.
    Index(i64),
}

/// A loop variable in scope.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Var {
    pub name: String,
    pub val: VarVal,
}

/// Where a path of an expression leads once loop variables are replaced.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Resolved<'v> {
    /// A model path (written into the buffer) with the keys of its lists.
    Path(&'v [Key]),
    /// The value of an index variable.
    Index(i64),
    /// The item of a list that is not a model path: nothing to read.
    Null,
}

/// Replaces a leading loop variable of a path (the innermost variable of that name wins) and writes the model
/// path into `buf`.
pub(crate) fn resolve_path<'v>(vars: &'v [Var], segs: &[String], buf: &mut String) -> Resolved<'v> {
    buf.clear();
    let var = segs
        .first()
        .and_then(|first| vars.iter().rev().find(|v| v.name == *first));
    let (keys, rest): (&[Key], &[String]) = match var {
        Some(Var {
            val: VarVal::Index(i),
            ..
        }) => return Resolved::Index(*i),
        Some(Var {
            val: VarVal::Item { origin: None, .. },
            ..
        }) => return Resolved::Null,
        Some(Var {
            val: VarVal::Item {
                origin: Some(o),
                keys,
            },
            ..
        }) => {
            buf.push_str(o);
            (keys, &segs[1..])
        }
        None => (&[], segs),
    };
    for s in rest {
        if !buf.is_empty() {
            buf.push('.');
        }
        buf.push_str(s);
    }
    Resolved::Path(keys)
}

/// What an expression sees while it is evaluated for one binding row.
pub(crate) struct Host<'a> {
    pub pkg: &'a crate::Pkg,
    pub plan: &'a Plan,
    pub store: &'a ModelStore,
    /// The loop variables in scope, outermost first.
    pub vars: &'a [Var],
    pub loc: &'a SkinLoc,
    /// The path being looked up (kept to avoid an allocation per path).
    pub buf: String,
    /// Warnings of the evaluator (division by zero, overflow).
    pub warnings: Vec<String>,
}

impl<'a> Host<'a> {
    /// A host for a row.
    pub fn new(
        pkg: &'a crate::Pkg,
        plan: &'a Plan,
        store: &'a ModelStore,
        vars: &'a [Var],
        loc: &'a SkinLoc,
    ) -> Self {
        Self {
            pkg,
            plan,
            store,
            vars,
            loc,
            buf: String::new(),
            warnings: Vec::new(),
        }
    }

    /// A field's value as the store holds it, read with the keys of the variable that led to it.
    fn stored(
        &self,
        f: ui2_model::FieldId,
        keys: &[Key],
    ) -> Option<(&'static Schema, &Type, &MValue)> {
        let (schema, entry) = self.plan.entry(f)?;
        let depth = entry.path.matches("[]").count().min(keys.len());
        let v = self.store.value(f, &keys[..depth])?;
        Some((schema, &entry.ty, v))
    }

    /// The value of a path (segments) as the model holds it (keys stay keys, enums indices, structs whole);
    /// `None` when absent.
    pub fn raw_path(&mut self, segs: &[String]) -> MValue {
        let mut buf = std::mem::take(&mut self.buf);
        let out = match resolve_path(self.vars, segs, &mut buf) {
            Resolved::Path(keys) => match self.plan.lookup(&buf) {
                Slot::Field(f) => self
                    .stored(f, keys)
                    .map_or(MValue::None, |(_, _, v)| v.clone()),
                _ => MValue::None,
            },
            Resolved::Index(i) => i32::try_from(i).map_or(MValue::None, MValue::I32),
            Resolved::Null => MValue::None,
        };
        self.buf = buf;
        out
    }

    /// A model value as the expression language sees it: integers and keys are integers, enum values their
    /// names, colors `0xRRGGBBAA`, texts localized, a list its length.
    fn model_value(&self, schema: &Schema, ty: &Type, v: &MValue) -> Value {
        let ty = match ty {
            Type::Option(inner) => inner.as_ref(),
            t => t,
        };
        match v {
            MValue::None => Value::Null,
            MValue::Bool(b) => Value::Bool(*b),
            MValue::I32(i) => Value::Int(i64::from(*i)),
            MValue::U32(u) if matches!(ty, Type::Scalar(Scalar::Color)) => Value::Color(*u),
            MValue::U32(u) => Value::Int(i64::from(*u)),
            MValue::F32(f) if f.is_finite() => Value::Float(f64::from(*f)),
            MValue::F32(_) => Value::Null,
            MValue::Str(s) => Value::Str(s.to_string()),
            MValue::Text(t) => Value::Text(self.loc.model_text(self.pkg, t)),
            MValue::Asset(a) => Value::Str(a.0.to_string()),
            MValue::Enum(i) => match ty {
                Type::Enum(e) => schema
                    .enums()
                    .get(e.0 as usize)
                    .and_then(|d| d.variants.get(usize::from(*i)))
                    .map_or(Value::Null, |n| Value::Str(n.clone())),
                _ => Value::Null,
            },
            MValue::Key(k) => Value::Int(k.0 as i64),
            MValue::Struct(_) => Value::Struct,
            MValue::List(items) => Value::List(items.len()),
        }
    }

    /// A text template with its inserts rendered.
    pub fn template(&mut self, tpl: TplId) -> String {
        let mut out = String::new();
        let Some(segs) = self.pkg.tpl(tpl) else {
            return out;
        };
        for seg in segs {
            match *seg {
                Seg::Lit(s) => out.push_str(self.pkg.str(s).unwrap_or("")),
                Seg::Expr(e) => {
                    if let Some(expr) = self.pkg.expr(e) {
                        out.push_str(&render(&eval(expr, self)));
                    }
                }
            }
        }
        out
    }

    /// The value of an expression.
    pub fn expr(&mut self, e: ui2_package::ExprId) -> Value {
        match self.pkg.expr(e) {
            Some(expr) => eval(expr, self),
            None => Value::Null,
        }
    }
}

impl EvalHost for Host<'_> {
    fn path(&mut self, segs: &[String]) -> Value {
        let mut buf = std::mem::take(&mut self.buf);
        let out = match resolve_path(self.vars, segs, &mut buf) {
            Resolved::Path(keys) => match self.plan.lookup(&buf) {
                Slot::Field(f) => match self.stored(f, keys) {
                    Some((schema, ty, v)) => self.model_value(schema, ty, v),
                    None => Value::Null,
                },
                Slot::State(i) => self
                    .plan
                    .state
                    .get(i as usize)
                    .cloned()
                    .unwrap_or(Value::Null),
                Slot::Missing => Value::Null,
            },
            Resolved::Index(i) => Value::Int(i),
            Resolved::Null => Value::Null,
        };
        self.buf = buf;
        out
    }

    fn loc(&mut self, key: &str, args: &[Value]) -> String {
        self.loc.call(self.pkg, key, args)
    }

    fn warn(&mut self, message: &str) {
        self.warnings.push(message.to_string());
    }
}
