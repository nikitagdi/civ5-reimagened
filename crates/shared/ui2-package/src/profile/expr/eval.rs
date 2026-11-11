//! The reference evaluator and the text formatting rules of the language.
//!
//! Evaluation never fails: an operation that has no result (a missing path, arithmetic with
//! `null`, a division by zero, an overflow, a number that is not finite) gives `null`, and the
//! last three tell the host through [`EvalHost::warn`]. Both sides of an arithmetic or
//! comparison operator are always evaluated, left to right; `&&`, `||` and `?:` evaluate only
//! the branch they need.

use std::cmp::Ordering;

use super::{BinOp, Expr, Func, UnOp};

/// A run-time value. Keys are integers and enum values are strings.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// No value.
    Null,
    /// A bool.
    Bool(bool),
    /// An integer or a key.
    Int(i64),
    /// A finite float.
    Float(f64),
    /// A plain string, an enum value or a game asset reference.
    Str(String),
    /// A ready game text (the result of `loc`, or a model field of type `text`).
    Text(String),
    /// A color, `0xRRGGBBAA`.
    Color(u32),
    /// A list; only its length is visible to an expression.
    List(usize),
    /// A record that is present.
    Struct,
}

/// What the evaluator needs from its surroundings.
pub trait EvalHost {
    /// The value at a path: a model field, a state path or a field of a loop variable. A path
    /// that is absent, or goes through `null`, is `Value::Null`.
    fn path(&mut self, segs: &[String]) -> Value;
    /// The text of `loc(key, args...)` in the current language.
    fn loc(&mut self, key: &str, args: &[Value]) -> String;
    /// An integer written the way the current language groups digits (`1,234`).
    fn fmt_int(&mut self, n: i64) -> String {
        n.to_string()
    }
    /// One line for the log (a division by zero, an overflow).
    fn warn(&mut self, message: &str);
}

/// `false`, `null`, `0`, `''` and an empty list are false; everything else is true.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Int(i) => *i != 0,
        Value::Float(f) => *f != 0.0,
        Value::Str(s) | Value::Text(s) => !s.is_empty(),
        Value::Color(_) | Value::Struct => true,
        Value::List(n) => *n != 0,
    }
}

#[derive(Clone, Copy)]
enum Num {
    I(i64),
    F(f64),
}

impl Num {
    fn of(v: &Value) -> Option<Num> {
        match v {
            Value::Int(i) => Some(Num::I(*i)),
            Value::Float(f) => Some(Num::F(*f)),
            _ => None,
        }
    }

    fn f(self) -> f64 {
        match self {
            Num::I(i) => i as f64,
            Num::F(f) => f,
        }
    }

    fn value(self) -> Value {
        match self {
            Num::I(i) => Value::Int(i),
            Num::F(f) => Value::Float(f),
        }
    }
}

fn finite(host: &mut dyn EvalHost, f: f64) -> Value {
    if f.is_finite() {
        Value::Float(f)
    } else {
        host.warn("a result is not a finite number");
        Value::Null
    }
}

/// Evaluates an expression.
pub fn eval(e: &Expr, host: &mut dyn EvalHost) -> Value {
    match e {
        Expr::Null => Value::Null,
        Expr::Bool(b) => Value::Bool(*b),
        Expr::Int(i) => Value::Int(*i),
        Expr::Float(f) => Value::Float(*f),
        Expr::Str(s) => Value::Str(s.clone()),
        Expr::Path(p) => host.path(p),
        Expr::Unary(UnOp::Not, a) => Value::Bool(!truthy(&eval(a, host))),
        Expr::Unary(UnOp::Neg, a) => match Num::of(&eval(a, host)) {
            Some(Num::I(i)) => match i.checked_neg() {
                Some(n) => Value::Int(n),
                None => {
                    host.warn("integer overflow");
                    Value::Null
                }
            },
            Some(Num::F(f)) => Value::Float(-f),
            None => Value::Null,
        },
        Expr::Binary(BinOp::And, a, b) => {
            Value::Bool(truthy(&eval(a, host)) && truthy(&eval(b, host)))
        }
        Expr::Binary(BinOp::Or, a, b) => {
            Value::Bool(truthy(&eval(a, host)) || truthy(&eval(b, host)))
        }
        Expr::Binary(op, a, b) => {
            let l = eval(a, host);
            let r = eval(b, host);
            binary(host, *op, &l, &r)
        }
        Expr::Cond(c, a, b) => {
            if truthy(&eval(c, host)) {
                eval(a, host)
            } else {
                eval(b, host)
            }
        }
        Expr::Call(f, args) => {
            let values: Vec<Value> = args.iter().map(|a| eval(a, host)).collect();
            call(host, *f, &values)
        }
    }
}

fn binary(host: &mut dyn EvalHost, op: BinOp, l: &Value, r: &Value) -> Value {
    match op {
        BinOp::Eq => Value::Bool(equal(l, r)),
        BinOp::Ne => Value::Bool(!equal(l, r)),
        BinOp::Lt => Value::Bool(compare(l, r) == Some(Ordering::Less)),
        BinOp::Le => Value::Bool(matches!(
            compare(l, r),
            Some(Ordering::Less | Ordering::Equal)
        )),
        BinOp::Gt => Value::Bool(compare(l, r) == Some(Ordering::Greater)),
        BinOp::Ge => Value::Bool(matches!(
            compare(l, r),
            Some(Ordering::Greater | Ordering::Equal)
        )),
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
            let (Some(a), Some(b)) = (Num::of(l), Num::of(r)) else {
                return Value::Null;
            };
            arithmetic(host, op, a, b)
        }
        // Short-circuit operators are handled in `eval`.
        BinOp::And | BinOp::Or => Value::Bool(truthy(l) && truthy(r)),
    }
}

fn arithmetic(host: &mut dyn EvalHost, op: BinOp, a: Num, b: Num) -> Value {
    if let (Num::I(x), Num::I(y)) = (a, b) {
        let exact = match op {
            BinOp::Add => x.checked_add(y),
            BinOp::Sub => x.checked_sub(y),
            BinOp::Mul => x.checked_mul(y),
            BinOp::Rem => {
                if y == 0 {
                    host.warn("division by zero");
                    return Value::Null;
                }
                x.checked_rem(y)
            }
            _ => None,
        };
        match op {
            BinOp::Div => {}
            _ => {
                return match exact {
                    Some(n) => Value::Int(n),
                    None => {
                        host.warn("integer overflow");
                        Value::Null
                    }
                };
            }
        }
    }
    let (x, y) = (a.f(), b.f());
    if matches!(op, BinOp::Div | BinOp::Rem) && y == 0.0 {
        host.warn("division by zero");
        return Value::Null;
    }
    let r = match op {
        BinOp::Add => x + y,
        BinOp::Sub => x - y,
        BinOp::Mul => x * y,
        BinOp::Div => x / y,
        _ => x % y,
    };
    finite(host, r)
}

fn equal(l: &Value, r: &Value) -> bool {
    match (l, r) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Int(a), Value::Int(b)) => a == b,
        (Value::Float(a), Value::Float(b)) => a == b,
        (Value::Int(a), Value::Float(b)) | (Value::Float(b), Value::Int(a)) => *a as f64 == *b,
        (Value::Str(a) | Value::Text(a), Value::Str(b) | Value::Text(b)) => a == b,
        (Value::Color(a), Value::Color(b)) => a == b,
        _ => false,
    }
}

fn compare(l: &Value, r: &Value) -> Option<Ordering> {
    match (l, r) {
        (Value::Int(a), Value::Int(b)) => Some(a.cmp(b)),
        (Value::Str(a) | Value::Text(a), Value::Str(b) | Value::Text(b)) => Some(a.cmp(b)),
        _ => Num::of(l)?.f().partial_cmp(&Num::of(r)?.f()),
    }
}

/// Rounds half away from zero into an integer (saturating).
fn to_int(f: f64) -> i64 {
    f.round() as i64
}

fn call(host: &mut dyn EvalHost, f: Func, args: &[Value]) -> Value {
    if !f.accepts(args.len()) {
        return Value::Null;
    }
    if f == Func::Loc {
        let key = match &args[0] {
            Value::Str(s) | Value::Text(s) => s.as_str(),
            _ => "",
        };
        return Value::Text(host.loc(key, &args[1..]));
    }
    // Every other function gives `null` for a `null` or a wrong-typed argument.
    if f == Func::Len {
        return match &args[0] {
            Value::List(n) => Value::Int(*n as i64),
            _ => Value::Null,
        };
    }
    let mut nums = Vec::with_capacity(args.len());
    for a in args {
        match Num::of(a) {
            Some(n) => nums.push(n),
            None => return Value::Null,
        }
    }
    match f {
        Func::FmtInt => Value::Str(host.fmt_int(to_int(nums[0].f()))),
        Func::FmtSigned => {
            let n = to_int(nums[0].f());
            let text = host.fmt_int(n);
            Value::Str(if n > 0 { format!("+{text}") } else { text })
        }
        // The number of decimals is clamped into 0..=6.
        Func::FmtFixed => match nums[1] {
            Num::I(digits) => Value::Str(fmt_fixed(nums[0].f(), digits.clamp(0, 6) as usize)),
            Num::F(_) => Value::Null,
        },
        Func::FmtPct => Value::Str(format!("{}%", to_int(nums[0].f() * 100.0))),
        Func::FmtMmss => {
            let total = nums[0].f().trunc().abs() as u64;
            let sign = if nums[0].f() <= -1.0 { "-" } else { "" };
            Value::Str(format!("{sign}{}:{:02}", total / 60, total % 60))
        }
        Func::Min => extreme(&nums, Ordering::Less),
        Func::Max => extreme(&nums, Ordering::Greater),
        Func::Clamp => {
            let raised = extreme(&[nums[0], nums[1]], Ordering::Greater);
            match Num::of(&raised) {
                Some(n) => extreme(&[n, nums[2]], Ordering::Less),
                None => Value::Null,
            }
        }
        Func::Round => Value::Int(to_int(nums[0].f())),
        Func::Floor => Value::Int(nums[0].f().floor() as i64),
        Func::Ceil => Value::Int(nums[0].f().ceil() as i64),
        Func::Abs => match nums[0] {
            Num::I(i) => Value::Int(i.saturating_abs()),
            Num::F(f) => Value::Float(f.abs()),
        },
        Func::Loc | Func::Len => Value::Null,
    }
}

/// The smallest (`Less`) or largest (`Greater`) number; an integer when all are integers.
fn extreme(nums: &[Num], want: Ordering) -> Value {
    let all_int = nums.iter().all(|n| matches!(n, Num::I(_)));
    let mut best = nums[0];
    for &n in &nums[1..] {
        let order = n.f().partial_cmp(&best.f());
        if order == Some(want) {
            best = n;
        }
    }
    if all_int {
        best.value()
    } else {
        Value::Float(best.f())
    }
}

/// A number with exactly `digits` decimals, halves rounded away from zero (Rust's own `{:.n}`
/// rounds ties to even, which would write 0.03125 as 0.0312).
pub fn fmt_fixed(x: f64, digits: usize) -> String {
    let scale = 10f64.powi(digits as i32);
    let scaled = (x.abs() * scale).round();
    if !scaled.is_finite() || scaled >= 9.0e15 {
        // Beyond the range where the scaled value is exact: the plain decimal expansion.
        return format!("{x:.digits$}");
    }
    let units = scaled as u64;
    let unit = 10u64.pow(digits as u32);
    let sign = if x < 0.0 && units != 0 { "-" } else { "" };
    if digits == 0 {
        format!("{sign}{units}")
    } else {
        format!(
            "{sign}{}.{:0width$}",
            units / unit,
            units % unit,
            width = digits
        )
    }
}

/// A float in a template: up to four decimals, trailing zeros dropped, no exponent.
pub fn fmt_float(x: f64) -> String {
    let mut s = fmt_fixed(x, 4);
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    s
}

/// A value as the text of an insert: `null` is empty, an integer is its digits, a float follows
/// [`fmt_float`], a color is `#rrggbb` (`#rrggbbaa` when not opaque).
pub fn render(v: &Value) -> String {
    match v {
        Value::Null | Value::List(_) | Value::Struct => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => fmt_float(*f),
        Value::Str(s) | Value::Text(s) => s.clone(),
        Value::Color(c) => {
            if c & 0xff == 0xff {
                format!("#{:06x}", c >> 8)
            } else {
                format!("#{c:08x}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halves_round_away_from_zero() {
        assert_eq!(fmt_float(0.03125), "0.0313");
        assert_eq!(fmt_float(-0.03125), "-0.0313");
        assert_eq!(fmt_float(2.5), "2.5");
        assert_eq!(fmt_float(-0.00001), "0");
        assert_eq!(fmt_float(1.0), "1");
        assert_eq!(fmt_fixed(0.125, 2), "0.13");
        assert_eq!(fmt_fixed(2.5, 0), "3");
        assert_eq!(fmt_fixed(-2.5, 0), "-3");
        assert_eq!(fmt_fixed(1234.5, 1), "1234.5");
    }

    #[test]
    fn colors_render_as_hex() {
        assert_eq!(render(&Value::Color(0x11223344)), "#11223344");
        assert_eq!(render(&Value::Color(0x112233ff)), "#112233");
    }
}
