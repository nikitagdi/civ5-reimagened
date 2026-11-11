//! Evaluating binding expressions against the model: the evaluator's host, model values as expression values,
//! text templates.

use ui2_model::{Key, ModelStore, Scalar, Type, Value as MValue};
use ui2_package::profile::expr::{EvalHost, Value, eval, render};
use ui2_package::{Package, Seg, TplId};

use crate::bind::deps::{Plan, Slot};
use crate::loc::SkinLoc;

/// What an expression sees while it is evaluated for one binding row.
pub(crate) struct Host<'a> {
    pub pkg: &'a Package,
    pub plan: &'a Plan,
    pub store: &'a ModelStore,
    /// The list keys of the row's instance, outermost first.
    pub keys: &'a [Key],
    pub loc: &'a SkinLoc,
    /// The path being looked up (kept to avoid an allocation per path).
    pub buf: String,
    /// Warnings of the evaluator (division by zero, overflow).
    pub warnings: Vec<String>,
}

impl<'a> Host<'a> {
    /// A host for a row.
    pub fn new(
        pkg: &'a Package,
        plan: &'a Plan,
        store: &'a ModelStore,
        keys: &'a [Key],
        loc: &'a SkinLoc,
    ) -> Self {
        Self {
            pkg,
            plan,
            store,
            keys,
            loc,
            buf: String::new(),
            warnings: Vec::new(),
        }
    }

    fn field(&self, f: ui2_model::FieldId) -> Value {
        let schema = self.plan.schema;
        let Some(entry) = schema.paths().get(f.0 as usize) else {
            return Value::Null;
        };
        let depth = entry.path.matches("[]").count().min(self.keys.len());
        match self.store.value(f, &self.keys[..depth]) {
            Some(v) => self.model_value(&entry.ty, v),
            None => Value::Null,
        }
    }

    /// A model value as the expression language sees it: integers and keys are integers, enum values their
    /// names, colors `0xRRGGBBAA`, texts localized, a list its length.
    fn model_value(&self, ty: &Type, v: &MValue) -> Value {
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
                Type::Enum(e) => self
                    .plan
                    .schema
                    .enum_def(*e)
                    .variants
                    .get(usize::from(*i))
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
        self.buf.clear();
        for (i, s) in segs.iter().enumerate() {
            if i > 0 {
                self.buf.push('.');
            }
            self.buf.push_str(s);
        }
        match self.plan.lookup(&self.buf) {
            Slot::Field(f) => self.field(f),
            Slot::State(i) => self
                .plan
                .state
                .get(i as usize)
                .cloned()
                .unwrap_or(Value::Null),
            Slot::Missing => Value::Null,
        }
    }

    fn loc(&mut self, key: &str, args: &[Value]) -> String {
        self.loc.call(self.pkg, key, args)
    }

    fn warn(&mut self, message: &str) {
        self.warnings.push(message.to_string());
    }
}
