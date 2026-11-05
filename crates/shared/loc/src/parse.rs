//! Template parser, a port of `Localization::StringParser` (Civ5XP 0x08c60f36..0x08c62f31).
//!
//! A template is literal text with `{...}` parameters:
//!
//! ```text
//! {1}  {1_Name}  {@1_Name}  {TXT_KEY_X}  {TXT_KEY_X << {@1_Name}}
//! {1: number "#,###"}  {1: plural 1?a; other?b;}  {@1: gender *:an?an {1}; other?a {1};}
//! {TXT_KEY_X ^'feminine' *2}  {TXT_KEY_X[2]}
//! ```
//!
//! All offsets are byte offsets into the template; the grammar is ASCII, so UTF-8 text passes through.

use crate::text::LocString;

/// A parse error. The exe logs the message and the whole string composes to the empty string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub message: String,
    pub offset: usize,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at offset {}", self.message, self.offset)
    }
}

impl std::error::Error for ParseError {}

/// A literal operand: `'text'` or a number.
#[derive(Debug, Clone, PartialEq)]
pub enum Literal {
    Int(i32),
    Float(f32),
    Text(LocString),
}

/// What follows `[`, `^` or `*`: a literal or a nested `{...}` parameter.
#[derive(Debug, Clone, PartialEq)]
pub enum Operand {
    Literal(Literal),
    Param(Box<Param>),
}

/// `Localization::StringParameter`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Param {
    /// 1-based argument number, 0 for a text-key parameter.
    pub arg_index: i32,
    /// Inclusive byte range of the text key (`{TXT_KEY_X}`).
    pub key: Option<(usize, usize)>,
    /// The `@` flag: the argument must be a string.
    pub at: bool,
    /// Offset of `{` and of the matching `}`.
    pub start: usize,
    pub end: usize,
    /// Inclusive byte range of the transform name after `:`.
    pub tag: Option<(usize, usize)>,
    /// `[form]` explicit form number.
    pub explicit_form: Option<Operand>,
    /// `^gender`.
    pub gender: Option<Operand>,
    /// `*plurality`.
    pub plural: Option<Operand>,
    /// `<< {a} << {b}` injected arguments.
    pub injected: Vec<Param>,
}

struct Parser<'a> {
    s: &'a [u8],
}

type Res<T> = Result<T, ParseError>;

fn is_ws(c: u8) -> bool {
    c == b'\t' || c == b'\n' || c == b' '
}

/// `[A-Za-z0-9_]`, the exe's identifier table (0x0a3113d0).
fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

impl<'a> Parser<'a> {
    fn err<T>(&self, message: &str, offset: usize) -> Res<T> {
        Err(ParseError {
            message: message.to_string(),
            offset,
        })
    }

    fn at(&self, i: usize) -> u8 {
        self.s.get(i).copied().unwrap_or(0)
    }

    /// `IncrementOffset`: past the end is an error, the end itself is allowed.
    fn inc(&self, off: &mut usize, by: usize) -> Res<()> {
        // re: 0x08c614e0 StringParser::IncrementOffset
        if *off + by > self.s.len() {
            return self.err("unexpected end of string", *off);
        }
        *off += by;
        Ok(())
    }

    /// `IncrementOffsetAndSkipWhitespace`: the last byte is never skipped.
    fn inc_skip(&self, off: &mut usize, by: usize) -> Res<()> {
        // re: 0x08c61576 StringParser::IncrementOffsetAndSkipWhitespace
        let len = self.s.len();
        let mut i = *off + by;
        if i >= len {
            return self.err("unexpected end of string", *off);
        }
        while i + 1 < len && is_ws(self.s[i]) {
            i += 1;
        }
        *off = i;
        Ok(())
    }

    fn skip_ws(&self, off: &mut usize) -> Res<()> {
        // re: 0x08c6179e StringParser::SkipWhitespace
        let len = self.s.len();
        let mut i = *off;
        if i >= len {
            return Ok(());
        }
        while i + 1 < len && is_ws(self.s[i]) {
            i += 1;
        }
        *off = i;
        Ok(())
    }

    /// `ParseIdentifier`: `Some((start, end))` inclusive, the offset left on the last identifier byte.
    fn identifier(&self, off: &mut usize) -> Option<(usize, usize)> {
        // re: 0x08c61742 StringParser::ParseIdentifier
        let len = self.s.len();
        let mut i = *off;
        if !is_ident(self.at(i)) {
            return None;
        }
        let start = i;
        let mut end = i;
        while i < len && is_ident(self.s[i]) {
            *off = i;
            end = i;
            i += 1;
        }
        Some((start, end))
    }

    /// `ParseNumericIdentifier`: digits, the offset left on the last digit.
    fn numeric_identifier(&self, off: &mut usize) -> Res<i32> {
        // re: 0x08c6166a StringParser::ParseNumericIdentifier
        let start = *off;
        let len = self.s.len();
        let mut i = start;
        while i < len && self.s[i].is_ascii_digit() {
            *off = i;
            i += 1;
        }
        if i == start {
            return self.err("expected a number", start);
        }
        let digits = &self.s[start..i];
        if digits.len() >= 16 {
            return self.err("number is too long", start);
        }
        // INVARIANT: ASCII digits only
        Ok(std::str::from_utf8(digits)
            .unwrap_or("0")
            .parse::<i64>()
            .unwrap_or(0) as i32)
    }

    /// `ParseLiteral`: `'text'`, the offset left on the closing quote.
    fn literal(&self, off: &mut usize) -> Res<Option<Literal>> {
        // re: 0x08c62360 StringParser::ParseLiteral
        if self.at(*off) != b'\'' {
            return Ok(None);
        }
        self.inc(off, 1)?;
        let start = *off;
        while self.at(*off) != b'\'' {
            self.inc(off, 1)?;
            if *off >= self.s.len() {
                return self.err("unterminated literal", start);
            }
        }
        let text = String::from_utf8_lossy(&self.s[start..*off]).into_owned();
        Ok(Some(Literal::Text(LocString::new(&text))))
    }

    /// `ParseNumeric`: digits with optional `-` and `.`; an integer or a float.
    fn numeric(&self, off: &mut usize) -> Res<Literal> {
        // re: 0x08c62480 StringParser::ParseNumeric
        let start = *off;
        let len = self.s.len();
        let mut i = start;
        while i < len && (self.s[i].is_ascii_digit() || self.s[i] == b'-' || self.s[i] == b'.') {
            *off = i;
            i += 1;
        }
        let text = std::str::from_utf8(&self.s[start..i]).unwrap_or("");
        if text.len() >= 16 {
            return self.err("number is too long", start);
        }
        if text.bytes().all(|b| b.is_ascii_digit()) && !text.is_empty() {
            Ok(Literal::Int(text.parse::<i64>().unwrap_or(0) as i32))
        } else if text.parse::<f32>().is_ok() {
            Ok(Literal::Float(text.parse::<f32>().unwrap_or(0.0)))
        } else {
            self.err("expected a number", start)
        }
    }

    /// The operand shared by `[`, `^` and `*`: `'literal'`, `{param}` or a number.
    fn operand(&self, off: &mut usize) -> Res<Operand> {
        match self.at(*off) {
            b'\'' => {
                let lit = self.literal(off)?;
                // INVARIANT: the opening quote was just seen
                Ok(Operand::Literal(lit.unwrap_or(Literal::Int(0))))
            }
            b'{' => {
                let mut sub = Param::default();
                self.parameter(&mut sub, off)?;
                Ok(Operand::Param(Box::new(sub)))
            }
            c if c.is_ascii_digit() => Ok(Operand::Literal(self.numeric(off)?)),
            _ => self.err("expected a literal, a number or a parameter", *off),
        }
    }

    /// `ParseFormSpecifier`: `[operand]`; `Ok(false)` when there is no `[`.
    fn form_specifier(&self, p: &mut Param, off: &mut usize) -> Res<bool> {
        // re: 0x08c6186a StringParser::ParseFormSpecifier
        if self.at(*off) != b'[' {
            return Ok(false);
        }
        self.inc_skip(off, 1)?;
        p.explicit_form = Some(self.operand(off)?);
        self.inc_skip(off, 1)?;
        if self.at(*off) != b']' {
            return self.err("expected ']'", *off);
        }
        Ok(true)
    }

    /// `ParseGenderForm`: `^operand`.
    fn gender_form(&self, p: &mut Param, off: &mut usize) -> Res<bool> {
        // re: 0x08c625fa StringParser::ParseGenderForm
        if self.at(*off) != b'^' {
            return Ok(false);
        }
        self.inc_skip(off, 1)?;
        p.gender = Some(self.operand(off)?);
        Ok(true)
    }

    /// `ParsePluralForm`: `*operand`.
    fn plural_form(&self, p: &mut Param, off: &mut usize) -> Res<bool> {
        // re: 0x08c6293a StringParser::ParsePluralForm
        if self.at(*off) != b'*' {
            return Ok(false);
        }
        self.inc_skip(off, 1)?;
        p.plural = Some(self.operand(off)?);
        Ok(true)
    }

    /// `ParsePluralOrGenderForm`: `^g`, `*p`, in either order, both optional.
    fn plural_or_gender(&self, p: &mut Param, off: &mut usize) -> Res<()> {
        // re: 0x08c61ca0 StringParser::ParsePluralOrGenderForm
        if self.gender_form(p, off)? {
            self.inc_skip(off, 1)?;
            if self.plural_form(p, off)? {
                self.inc(off, 1)?;
            }
            return Ok(());
        }
        if self.plural_form(p, off)? {
            self.inc_skip(off, 1)?;
            if self.gender_form(p, off)? {
                self.inc(off, 1)?;
            }
        }
        Ok(())
    }

    /// `ParseInjectArgs`: `<< {param}` repeated.
    fn inject_args(&self, p: &mut Param, off: &mut usize) -> Res<()> {
        // re: 0x08c61e12 StringParser::ParseInjectArgs
        if self.at(*off) != b'<' {
            return Ok(());
        }
        self.inc(off, 1)?;
        if self.at(*off) != b'<' {
            return self.err("expected '<<'", *off);
        }
        self.inc_skip(off, 1)?;
        match self.at(*off) {
            b'{' => {
                let mut sub = Param::default();
                self.parameter(&mut sub, off)?;
                p.injected.push(sub);
            }
            b'\'' => return self.err("a literal cannot be injected", *off),
            c if c.is_ascii_digit() => return self.err("a number cannot be injected", *off),
            _ => return self.err("expected a parameter after '<<'", *off),
        }
        if *off == self.s.len() {
            return Ok(());
        }
        self.inc_skip(off, 1)?;
        self.inject_args(p, off)
    }

    /// `ParseTransform`: `: name args`, the offset left on the closing `}` of the parameter.
    fn transform(&self, p: &mut Param, off: &mut usize) -> Res<()> {
        // re: 0x08c621f0 StringParser::ParseTransform
        if self.at(*off) != b':' {
            return Ok(());
        }
        self.inc_skip(off, 1)?;
        let Some(tag) = self.identifier(off) else {
            return self.err("expected a transform name", *off);
        };
        p.tag = Some(tag);
        // Brace depth only; quotes are not special here.
        let mut depth = 1i32;
        loop {
            self.inc(off, 1)?;
            match self.at(*off) {
                b'{' => depth += 1,
                b'}' => depth -= 1,
                _ => {}
            }
            if depth <= 0 {
                break;
            }
            if *off >= self.s.len() {
                return self.err("unterminated transform", *off);
            }
        }
        Ok(())
    }

    /// `ParseParameter`: the offset is on `{`; on success it ends on the matching `}`.
    fn parameter(&self, p: &mut Param, off: &mut usize) -> Res<()> {
        // re: 0x08c6106e StringParser::ParseParameter
        let start = *off;
        if self.at(start) != b'{' {
            return self.err("expected '{'", start);
        }
        self.inc_skip(off, 1)?;
        if self.at(*off) == b'@' {
            p.at = true;
            self.inc(off, 1)?;
        }
        let c = self.at(*off);
        if (b'1'..=b'9').contains(&c) {
            p.arg_index = self.numeric_identifier(off)?;
            self.inc_skip(off, 1)?;
            // `_Name` comment after the number; it can also follow directly ("1CivName")
            if self.identifier(off).is_some() {
                self.inc(off, 1)?;
            }
        } else if c == b'0' {
            return self.err("argument numbers start at 1", *off);
        } else {
            let Some(key) = self.identifier(off) else {
                return self.err("expected an argument number or a text key", *off);
            };
            p.key = Some(key);
            self.inc(off, 1)?;
        }
        self.skip_ws(off)?;
        if self.at(*off) == b'[' {
            self.form_specifier(p, off)?;
            self.inc(off, 1)?;
        } else {
            self.plural_or_gender(p, off)?;
        }
        self.skip_ws(off)?;
        self.inject_args(p, off)?;
        self.skip_ws(off)?;
        let c = self.at(*off);
        if c == b':' || c == b'}' {
            self.transform(p, off)?;
            if self.at(*off) != b'}' {
                return self.err("expected '}'", *off);
            }
            p.start = start;
            p.end = *off;
            return Ok(());
        }
        self.err("expected ':' or '}'", *off)
    }
}

/// `StringParser::Parse`: the parameters of `template` in order.
pub fn parse(template: &str) -> Result<Vec<Param>, ParseError> {
    // re: 0x08c60f60 StringParser::Parse
    let p = Parser {
        s: template.as_bytes(),
    };
    let mut out = Vec::new();
    let mut off = 0usize;
    while off < template.len() {
        if p.at(off) == b'{' {
            let mut param = Param::default();
            p.parameter(&mut param, &mut off)?;
            out.push(param);
        }
        p.inc(&mut off, 1)?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(t: &str) -> Param {
        let v = parse(t).unwrap();
        assert_eq!(v.len(), 1, "{t}");
        v.into_iter().next().unwrap()
    }

    #[test]
    fn plain_text_has_no_parameters() {
        assert!(parse("just text }").unwrap().is_empty());
        assert!(parse("").unwrap().is_empty());
    }

    #[test]
    fn positional_with_name_and_flag() {
        let p = one("{1}");
        assert_eq!((p.arg_index, p.at, p.start, p.end), (1, false, 0, 2));
        let p = one("x {2_CivName} y");
        assert_eq!((p.arg_index, p.start, p.end), (2, 2, 12));
        let p = one("{@1_Name}");
        assert!(p.at);
        let p = one("{1CivName:textkey}");
        assert_eq!(p.tag, Some((10, 16)));
    }

    #[test]
    fn text_key_and_injection() {
        let p = one("{TXT_KEY_X}");
        assert_eq!(p.key, Some((1, 9)));
        let p = one("{TXT_KEY_GRAMMAR_A_AN << {3_CivAdj}}");
        assert_eq!(p.injected.len(), 1);
        assert_eq!(p.injected[0].arg_index, 3);
        let p = one("{TXT_KEY_X << {@1_A} << {2_B}}");
        assert_eq!(p.injected.len(), 2);
        assert!(p.injected[0].at);
    }

    #[test]
    fn transforms_keep_nested_braces() {
        let t = "{1: gender *:an?an {1}; other?a {1};}";
        let p = one(t);
        assert_eq!(p.tag, Some((4, 9)));
        assert_eq!(p.end, t.len() - 1);
        let t = "{2:gender *:sinartmasc?; masculine?{2: plural 1?el ; other?los ;}; other?x;}";
        assert_eq!(one(t).end, t.len() - 1);
        let p = one("{1_Num: number \"#,##0.##\"}");
        assert!(p.tag.is_some());
    }

    #[test]
    fn several_parameters() {
        let v = parse("{1} of {2_B} and {TXT_KEY_C}").unwrap();
        assert_eq!(v.len(), 3);
    }

    #[test]
    fn specifiers() {
        let p = one("{TXT_KEY_X ^'feminine' *2}");
        assert!(matches!(p.gender, Some(Operand::Literal(Literal::Text(_)))));
        assert!(matches!(p.plural, Some(Operand::Literal(Literal::Int(2)))));
        let p = one("{TXT_KEY_X[2]}");
        assert!(matches!(
            p.explicit_form,
            Some(Operand::Literal(Literal::Int(2)))
        ));
        let p = one("{TXT_KEY_X *{1}}");
        assert!(matches!(p.plural, Some(Operand::Param(_))));
    }

    #[test]
    fn errors() {
        assert!(parse("{1").is_err());
        assert!(parse("{0}").is_err());
        assert!(parse("{").is_err());
        assert!(parse("{ }").is_err());
        assert!(parse("{1 ?}").is_err());
        assert!(parse("{K << 'x'}").is_err());
        assert!(parse("{1: }").is_err());
    }
}
