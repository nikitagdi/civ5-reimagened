//! The parser: tokens to [`Expr`], and text templates to [`Template`].

use super::lex::{Tok, Token, lex};
use super::{BinOp, Expr, ExprError, Func, MAX_DEPTH, Template, TplSeg, UnOp, codes};

fn syntax(at: usize, message: impl Into<String>) -> ExprError {
    ExprError::at(codes::SYNTAX, at, message)
}

struct Parser<'a> {
    src: &'a str,
    toks: Vec<Token>,
    pos: usize,
    depth: usize,
}

/// Parses one expression.
pub fn parse(src: &str) -> Result<Expr, ExprError> {
    let toks = lex(src)?;
    if toks.is_empty() {
        return Err(syntax(0, "empty expression"));
    }
    let mut p = Parser {
        src,
        toks,
        pos: 0,
        depth: 0,
    };
    let e = p.expr()?;
    if let Some(t) = p.toks.get(p.pos) {
        return Err(syntax(t.start, "unexpected token after the expression"));
    }
    if e.depth() > MAX_DEPTH {
        return Err(syntax(0, "expression nested too deeply"));
    }
    Ok(e)
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos).map(|t| &t.tok)
    }

    fn here(&self) -> usize {
        self.toks.get(self.pos).map_or(self.src.len(), |t| t.start)
    }

    fn is_op(&self, op: &str) -> bool {
        matches!(self.peek(), Some(Tok::Op(o)) if *o == op)
    }

    fn eat_op(&mut self, op: &str) -> bool {
        if self.is_op(op) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect_op(&mut self, op: &str) -> Result<(), ExprError> {
        if self.eat_op(op) {
            Ok(())
        } else {
            Err(syntax(self.here(), format!("expected `{op}`")))
        }
    }

    fn enter(&mut self) -> Result<(), ExprError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(syntax(self.here(), "expression nested too deeply"));
        }
        Ok(())
    }

    fn expr(&mut self) -> Result<Expr, ExprError> {
        self.enter()?;
        let cond = self.or()?;
        let out = if self.eat_op("?") {
            let then = self.expr()?;
            self.expect_op(":")?;
            let otherwise = self.expr()?;
            Expr::Cond(Box::new(cond), Box::new(then), Box::new(otherwise))
        } else {
            cond
        };
        self.depth -= 1;
        Ok(out)
    }

    fn or(&mut self) -> Result<Expr, ExprError> {
        let mut left = self.and()?;
        while self.eat_op("||") {
            let right = self.and()?;
            left = Expr::Binary(BinOp::Or, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<Expr, ExprError> {
        let mut left = self.cmp()?;
        while self.eat_op("&&") {
            let right = self.cmp()?;
            left = Expr::Binary(BinOp::And, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn cmp(&mut self) -> Result<Expr, ExprError> {
        let left = self.sum()?;
        let op = match self.peek() {
            Some(Tok::Op("==")) => BinOp::Eq,
            Some(Tok::Op("!=")) => BinOp::Ne,
            Some(Tok::Op("<")) => BinOp::Lt,
            Some(Tok::Op("<=")) => BinOp::Le,
            Some(Tok::Op(">")) => BinOp::Gt,
            Some(Tok::Op(">=")) => BinOp::Ge,
            _ => return Ok(left),
        };
        self.pos += 1;
        let right = self.sum()?;
        if matches!(
            self.peek(),
            Some(Tok::Op("==" | "!=" | "<" | "<=" | ">" | ">="))
        ) {
            return Err(syntax(self.here(), "comparisons cannot be chained"));
        }
        Ok(Expr::Binary(op, Box::new(left), Box::new(right)))
    }

    fn sum(&mut self) -> Result<Expr, ExprError> {
        let mut left = self.prod()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Op("+")) => BinOp::Add,
                Some(Tok::Op("-")) => BinOp::Sub,
                _ => return Ok(left),
            };
            self.pos += 1;
            let right = self.prod()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
    }

    fn prod(&mut self) -> Result<Expr, ExprError> {
        let mut left = self.unary()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Op("*")) => BinOp::Mul,
                Some(Tok::Op("/")) => BinOp::Div,
                Some(Tok::Op("%")) => BinOp::Rem,
                _ => return Ok(left),
            };
            self.pos += 1;
            let right = self.unary()?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
    }

    fn unary(&mut self) -> Result<Expr, ExprError> {
        let op = match self.peek() {
            Some(Tok::Op("!")) => UnOp::Not,
            Some(Tok::Op("-")) => UnOp::Neg,
            _ => return self.atom(),
        };
        self.enter()?;
        self.pos += 1;
        let inner = self.unary()?;
        self.depth -= 1;
        Ok(Expr::Unary(op, Box::new(inner)))
    }

    fn atom(&mut self) -> Result<Expr, ExprError> {
        let at = self.here();
        let Some(tok) = self.peek().cloned() else {
            return Err(syntax(at, "the expression ends too early"));
        };
        self.pos += 1;
        match tok {
            Tok::Int(i) => Ok(Expr::Int(i)),
            Tok::Float(f) => Ok(Expr::Float(f)),
            Tok::Str(s) => Ok(Expr::Str(s)),
            Tok::Op("(") => {
                let inner = self.expr()?;
                self.expect_op(")")?;
                Ok(inner)
            }
            Tok::Ident(first) => match first.as_str() {
                "true" => Ok(Expr::Bool(true)),
                "false" => Ok(Expr::Bool(false)),
                "null" => Ok(Expr::Null),
                _ => self.path_or_call(first, at),
            },
            Tok::Op(op) => Err(syntax(at, format!("unexpected `{op}`"))),
        }
    }

    fn path_or_call(&mut self, first: String, at: usize) -> Result<Expr, ExprError> {
        let mut segs = vec![first];
        while self.is_op(".") {
            self.pos += 1;
            match self.peek().cloned() {
                Some(Tok::Ident(name)) => {
                    self.pos += 1;
                    segs.push(name);
                }
                _ => return Err(syntax(self.here(), "expected a name after `.`")),
            }
        }
        if !self.eat_op("(") {
            if segs
                .iter()
                .any(|s| matches!(s.as_str(), "true" | "false" | "null"))
            {
                return Err(syntax(at, "a keyword cannot be part of a path"));
            }
            return Ok(Expr::Path(segs));
        }
        let name = segs.join(".");
        let Some(func) = Func::from_name(&name) else {
            return Err(ExprError::at(
                codes::UNKNOWN_FUNC,
                at,
                format!("unknown function `{name}`"),
            ));
        };
        let mut args = Vec::new();
        if !self.eat_op(")") {
            loop {
                args.push(self.expr()?);
                if self.eat_op(",") {
                    continue;
                }
                self.expect_op(")")?;
                break;
            }
        }
        if !func.accepts(args.len()) {
            let (lo, hi) = func.arity();
            let want = match hi {
                Some(h) if h == lo => format!("{lo}"),
                Some(h) => format!("{lo} to {h}"),
                None => format!("at least {lo}"),
            };
            return Err(ExprError::at(
                codes::ARGC,
                at,
                format!("`{name}` takes {want} arguments, not {}", args.len()),
            ));
        }
        Ok(Expr::Call(func, args))
    }
}

/// Finds the `}}` that closes an insert starting at `from` (just after `{{`), skipping string
/// literals. Returns the offset of the `}}`.
pub(crate) fn find_close(src: &str, from: usize) -> Option<usize> {
    let bytes = src.as_bytes();
    let mut i = from;
    let mut in_str = false;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_str => i += 1,
            b'\'' => in_str = !in_str,
            b'}' if !in_str && bytes.get(i + 1) == Some(&b'}') => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

/// Parses a text with `{{ expr }}` inserts. A `}}` outside an insert is plain text.
pub fn parse_template(src: &str) -> Result<Template, ExprError> {
    let mut segs: Vec<TplSeg> = Vec::new();
    let mut lit = String::new();
    let mut i = 0;
    while i < src.len() {
        let rest = &src[i..];
        if let Some(open) = rest.find("{{") {
            lit.push_str(&rest[..open]);
            let body = i + open + 2;
            let Some(close) = find_close(src, body) else {
                return Err(ExprError::at(
                    codes::UNCLOSED,
                    i + open,
                    "`{{` has no closing `}}`",
                ));
            };
            if !lit.is_empty() {
                segs.push(TplSeg::Lit(std::mem::take(&mut lit)));
            }
            let expr = parse(&src[body..close]).map_err(|e| e.shifted(body))?;
            segs.push(TplSeg::Expr(expr));
            i = close + 2;
        } else {
            lit.push_str(rest);
            break;
        }
    }
    if !lit.is_empty() {
        segs.push(TplSeg::Lit(lit));
    }
    Ok(Template { segs })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precedence_and_associativity() {
        let e = parse("1 + 2 * 3 - 4").expect("parse");
        // ((1 + (2 * 3)) - 4)
        assert!(matches!(e, Expr::Binary(BinOp::Sub, _, _)));
        let e = parse("a ? b : c ? d : e").expect("parse");
        assert!(matches!(&e, Expr::Cond(_, _, last) if matches!(**last, Expr::Cond(..))));
        assert!(parse("1 < 2 < 3").is_err());
        assert!(parse("!a == b").is_ok());
    }

    #[test]
    fn deep_nesting_is_an_error_not_a_crash() {
        let src = format!("{}1{}", "(".repeat(5000), ")".repeat(5000));
        assert_eq!(parse(&src).expect_err("deep").code, codes::SYNTAX);
        let src = format!("{}1", "-".repeat(5000));
        assert_eq!(parse(&src).expect_err("deep").code, codes::SYNTAX);
        let long = vec!["1"; 400].join(" + ");
        assert_eq!(parse(&long).expect_err("deep").code, codes::SYNTAX);
    }

    #[test]
    fn templates_split_and_report_positions() {
        let t = parse_template("a {{ x }} b {{ '}}' }}").expect("tpl");
        assert_eq!(t.segs.len(), 4);
        let e = parse_template("ab {{ 1 + }}").expect_err("bad");
        assert_eq!(e.code, codes::SYNTAX);
        assert_eq!(e.at, Some(10));
        assert_eq!(
            parse_template("x {{ y").expect_err("open").code,
            codes::UNCLOSED
        );
        assert_eq!(
            parse_template("a }} b")
                .expect("lit")
                .as_static()
                .as_deref(),
            Some("a }} b")
        );
    }
}
