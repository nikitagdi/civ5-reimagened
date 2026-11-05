//! Template evaluation: a port of `Localization::ParameterEvaluator` and the conditional transforms
//! (Civ5XP 0x08c64828..0x08c68f20, `ParameterTransforms` 0x08c5b434..0x08c5ca94).
//!
//! Every error in the exe aborts the composition of the whole string, and `String::toUTF8` then returns the
//! empty string. Here errors are values; [`Localizer::convert`] maps them to the empty string and counts them.

use std::sync::Arc;

use crate::View;
use crate::number::{self, DecimalFormat};
use crate::parse::{self, Literal, Operand, Param};
use crate::plural;
use crate::text::{Form, GenderSpec, LocString};

/// Template and clause nesting beyond this is a cycle (`{TXT_KEY_A}` inside `TXT_KEY_A`).
const MAX_DEPTH: u32 = 24;

/// An argument of `ConvertTextKey` (`ParameterArgument`): the kind decides the transforms it accepts.
#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    Int(i32),
    Int64(i64),
    Float(f32),
    Double(f64),
    /// A string argument; split into forms at `|` like `String::PushArgument(const char*)`.
    Str(String),
    /// A string with grammar data already attached.
    Loc(LocString),
}

impl Arg {
    /// Lua boolean -> `Int(0 | 1)`, as `lConvertTextKey` does.
    pub fn from_bool(b: bool) -> Arg {
        Arg::Int(b as i32)
    }
}

/// Why a composition failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvalError {
    /// The template does not parse.
    Parse(parse::ParseError),
    /// Evaluation failed: the message names the rule.
    Eval(String),
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EvalError::Parse(e) => write!(f, "template error: {e}"),
            EvalError::Eval(m) => write!(f, "evaluation error: {m}"),
        }
    }
}

impl std::error::Error for EvalError {}

type Res<T> = Result<T, EvalError>;

fn fail<T>(msg: impl Into<String>) -> Res<T> {
    Err(EvalError::Eval(msg.into()))
}

/// Internal argument: a number or a string with a selected form (`ParameterArgument::m_type` for strings).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Value {
    Int(i32),
    Int64(i64),
    Float(f32),
    Double(f64),
    Str { s: LocString, form: u32 },
}

impl Value {
    pub(crate) fn from_arg(a: &Arg) -> Value {
        match a {
            Arg::Int(v) => Value::Int(*v),
            Arg::Int64(v) => Value::Int64(*v),
            Arg::Float(v) => Value::Float(*v),
            Arg::Double(v) => Value::Double(*v),
            Arg::Str(s) => Value::Str {
                s: LocString::new(s),
                form: 0,
            },
            Arg::Loc(s) => Value::Str {
                s: s.clone(),
                form: 0,
            },
        }
    }

    /// The count used by plural rules: floats truncate, int64 keeps its low 32 bits (`cvttsd2si`, low dword).
    fn as_count(&self) -> Option<i32> {
        match self {
            Value::Int(v) => Some(*v),
            Value::Int64(v) => Some(*v as i32),
            Value::Float(v) => Some(*v as i32),
            Value::Double(v) => Some(*v as i32),
            Value::Str { .. } => None,
        }
    }
}

/// `Localization::SingleFormString` after evaluation.
type Evaluated = Form;

impl View<'_> {
    /// Parsed parameters of `template`, cached per template text.
    fn parsed(&self, template: &str) -> Result<Arc<Vec<Param>>, EvalError> {
        if let Some(hit) = self.cache_get(template) {
            return Ok(hit);
        }
        match parse::parse(template) {
            Ok(v) => {
                let v = Arc::new(v);
                self.cache_put(template, v.clone());
                Ok(v)
            }
            Err(e) => Err(EvalError::Parse(e)),
        }
    }

    /// `SingleFormString::Compose`: literal text and evaluated parameters.
    pub(crate) fn compose_form(&self, template: &str, args: &[Value], depth: u32) -> Res<String> {
        // re: 0x08c6607a SingleFormString::Compose
        if depth > MAX_DEPTH {
            return fail("nesting is too deep (a text key refers to itself?)");
        }
        let params = self.parsed(template)?;
        if params.is_empty() {
            return Ok(template.to_string());
        }
        let mut out = String::with_capacity(template.len() + 16);
        let mut pos = 0usize;
        for p in params.iter() {
            out.push_str(&template[pos..p.start]);
            let ev = self.evaluate(template, p, args, depth)?;
            out.push_str(&ev.text);
            pos = p.end + 1;
        }
        out.push_str(&template[pos..]);
        Ok(out)
    }

    /// `String::toUTF8(form)` with `args`: the composed text of a form, empty if the form does not exist or fails.
    pub(crate) fn string_text(
        &self,
        s: &LocString,
        form: u32,
        args: &[Value],
        depth: u32,
    ) -> String {
        // re: 0x08c5d728 String::toUTF8(unsigned int&, int)
        let Some(f) = s.form(form.max(1) as usize) else {
            return String::new();
        };
        match self.compose_form(&f.text, args, depth) {
            Ok(t) => t,
            Err(e) => {
                self.note_bad_template(&f.text, &e);
                String::new()
            }
        }
    }

    /// `ParameterEvaluator::GetInjectedArguments`: `<<` arguments first, then the remaining original ones.
    fn injected_args(&self, tpl: &str, p: &Param, args: &[Value], depth: u32) -> Res<Vec<Value>> {
        // re: 0x08c67d2a ParameterEvaluator::GetInjectedArguments
        let mut out = Vec::with_capacity(args.len().max(p.injected.len()));
        for inj in &p.injected {
            let simple = inj.arg_index > 0
                && !inj.at
                && inj.tag.is_none()
                && inj.injected.is_empty()
                && inj.explicit_form.is_none()
                && inj.gender.is_none()
                && inj.plural.is_none();
            if simple {
                let Some(v) = args.get(inj.arg_index as usize - 1) else {
                    return fail(format!(
                        "injected argument {} is out of range",
                        inj.arg_index
                    ));
                };
                out.push(v.clone());
            } else {
                let ev = self.evaluate(tpl, inj, args, depth + 1)?;
                out.push(Value::Str {
                    s: LocString::from_form(ev),
                    form: 0,
                });
            }
        }
        out.extend(args.iter().skip(p.injected.len()).cloned());
        Ok(out)
    }

    /// `ParameterEvaluator::Evaluate`.
    fn evaluate(&self, tpl: &str, p: &Param, args: &[Value], depth: u32) -> Res<Evaluated> {
        // re: 0x08c678b2 ParameterEvaluator::Evaluate
        if depth > MAX_DEPTH {
            return fail("nesting is too deep");
        }
        let injected = self.injected_args(tpl, p, args, depth)?;
        if p.arg_index < 1 {
            let Some((a, b)) = p.key else {
                return fail("parameter has neither an argument number nor a text key");
            };
            let text = self.lookup_string(&tpl[a..=b]);
            let arg = Value::Str { s: text, form: 0 };
            return self.evaluate_string(tpl, p, &arg, &injected, args, depth);
        }
        let idx = p.arg_index as usize - 1;
        let Some(arg) = args.get(idx) else {
            return fail(format!(
                "argument {} is out of range ({} given) in \"{}\"",
                p.arg_index,
                args.len(),
                tpl
            ));
        };
        if let Value::Str { s, .. } = arg {
            // A string argument is a text key that is translated when possible.
            let arg = match self.try_lookup_string(s.base()) {
                Some(t) => Value::Str { s: t, form: 0 },
                None => arg.clone(),
            };
            return self.evaluate_string(tpl, p, &arg, &injected, args, depth);
        }
        if p.at {
            return fail(format!("'@' needs a string argument in \"{tpl}\""));
        }
        self.transform(tpl, p, arg, &injected, depth)
    }

    /// `ParameterEvaluator::EvaluateString`: picks the form named by `[n]`, `^gender` and `*plurality`.
    fn evaluate_string(
        &self,
        tpl: &str,
        p: &Param,
        arg: &Value,
        injected: &[Value],
        args: &[Value],
        depth: u32,
    ) -> Res<Evaluated> {
        // re: 0x08c67ffc ParameterEvaluator::EvaluateString
        let Value::Str { s, .. } = arg else {
            return self.transform(tpl, p, arg, injected, depth);
        };
        let mut form = self.explicit_form(p, args)?;
        if form == 0 {
            let plural = self.plural_spec(tpl, p, args, depth)?;
            let gender = self.gender_spec(tpl, p, args, depth)?;
            form = match (plural, gender) {
                (0, None) => 0,
                (pl, Some(g)) if pl > 0 => {
                    let f = s.form_by_plurality_and_gender(pl, &g);
                    if f == 0 {
                        return fail("no form matches the plurality and gender of the parameter");
                    }
                    f as u32
                }
                (pl, None) => {
                    let f = s.form_by_plurality(pl, 0);
                    if f == 0 {
                        return fail("no form matches the plurality of the parameter");
                    }
                    f as u32
                }
                (_, Some(g)) => {
                    let mut f = s.form_by_gender(&g);
                    if f == 0 {
                        // the exe retries without the traits
                        f = s.form_by_gender(&GenderSpec {
                            gender: g.gender,
                            traits: Vec::new(),
                        });
                    }
                    if f == 0 {
                        return fail("no form matches the gender of the parameter");
                    }
                    f as u32
                }
            };
        }
        if form != 0 && s.num_forms() != 1 {
            let picked = Value::Str { s: s.clone(), form };
            self.transform(tpl, p, &picked, injected, depth)
        } else {
            self.transform(tpl, p, arg, injected, depth)
        }
    }

    /// `GetExplicitFormSpecifier`: `[n]`; 0 when absent.
    fn explicit_form(&self, p: &Param, args: &[Value]) -> Res<u32> {
        // re: 0x08c682e8 ParameterEvaluator::GetExplicitFormSpecifier
        let v = match &p.explicit_form {
            None => return Ok(0),
            Some(Operand::Literal(Literal::Int(v))) => *v,
            Some(Operand::Literal(_)) => return fail("an explicit form must be an integer"),
            Some(Operand::Param(sp)) => {
                if sp.arg_index < 1 {
                    return fail("an explicit form parameter needs an argument number");
                }
                match args.get(sp.arg_index as usize - 1) {
                    Some(Value::Int(v)) => *v,
                    Some(_) => return fail("an explicit form argument must be an integer"),
                    None => return fail("the explicit form argument is out of range"),
                }
            }
        };
        if v < 0 {
            return fail("an explicit form cannot be negative");
        }
        Ok(v as u32)
    }

    /// `GetPluralFormSpecifier`: the plural form named by `*`; 0 when absent.
    fn plural_spec(&self, tpl: &str, p: &Param, args: &[Value], depth: u32) -> Res<u32> {
        // re: 0x08c68382 ParameterEvaluator::GetPluralFormSpecifier
        match &p.plural {
            None => Ok(0),
            Some(Operand::Literal(Literal::Int(n))) => Ok(self.plural_form(*n)),
            Some(Operand::Literal(Literal::Float(f))) => Ok(self.plural_form(*f as i32)),
            Some(Operand::Literal(Literal::Text(_))) => {
                fail("a plurality literal must be a number")
            }
            Some(Operand::Param(sp)) => {
                if sp.arg_index >= 1 {
                    match args.get(sp.arg_index as usize - 1) {
                        None => fail("the plurality argument is out of range"),
                        Some(v) => match v.as_count() {
                            Some(n) => Ok(self.plural_form(n)),
                            None => match v {
                                Value::Str { s, .. } => Ok(s.form(1).map_or(0, |f| f.plurality)),
                                _ => Ok(0),
                            },
                        },
                    }
                } else {
                    Ok(self.evaluate(tpl, sp, args, depth + 1)?.plurality)
                }
            }
        }
    }

    /// `GetGenderFormSpecifier`: the gender named by `^`; `None` when absent.
    fn gender_spec(
        &self,
        tpl: &str,
        p: &Param,
        args: &[Value],
        depth: u32,
    ) -> Res<Option<GenderSpec>> {
        // re: 0x08c68526 ParameterEvaluator::GetGenderFormSpecifier
        match &p.gender {
            None => Ok(None),
            Some(Operand::Literal(Literal::Text(ls))) => {
                let text = self.string_text(ls, 1, &[], depth + 1);
                match GenderSpec::parse(&text) {
                    Some(g) => Ok(Some(g)),
                    None => fail(format!("bad gender specifier \"{text}\"")),
                }
            }
            Some(Operand::Literal(_)) => fail("a gender literal must be a string"),
            Some(Operand::Param(sp)) => {
                let ev = self.evaluate(tpl, sp, args, depth + 1)?;
                Ok(Some(GenderSpec {
                    gender: Some(ev.gender),
                    traits: ev.traits,
                }))
            }
        }
    }

    fn plural_form(&self, n: i32) -> u32 {
        plural::plural_form(self.current_language().plural_rule, n).unwrap_or(0)
    }

    /// `ParameterEvaluator::Transform`: the named transform, or the default one without a tag.
    fn transform(
        &self,
        tpl: &str,
        p: &Param,
        arg: &Value,
        injected: &[Value],
        depth: u32,
    ) -> Res<Evaluated> {
        // re: 0x08c686d8 ParameterEvaluator::Transform
        let Some((a, b)) = p.tag else {
            return self.default_transform(arg, injected, depth);
        };
        let style = &tpl[a..=b];
        let mut i = b + 1;
        while i < p.end && matches!(tpl.as_bytes()[i], b' ' | b'\t') {
            i += 1;
        }
        let style_args = tpl[i.min(p.end)..p.end].trim_end_matches([' ', '\t', '\r', '\n']);
        let out = self.apply_style(style, arg, injected, style_args, depth)?;
        Ok(Form::plain(out))
    }

    /// `ParameterEvaluator::DefaultTransformation`: numbers in the default number format, strings composed.
    fn default_transform(&self, arg: &Value, injected: &[Value], depth: u32) -> Res<Evaluated> {
        // re: 0x08c68afa ParameterEvaluator::DefaultTransformation
        match arg {
            Value::Str { s, form } => {
                let f = (*form).max(1);
                let text = self.string_text(s, f, injected, depth + 1);
                let mut out = Form::plain(text);
                if let Some(src) = s.form(f as usize) {
                    out.gender = src.gender;
                    out.plurality = src.plurality;
                    out.traits = src.traits.clone();
                }
                Ok(out)
            }
            _ => Ok(Form::plain(
                self.format_number(arg, &self.number_format()?)?,
            )),
        }
    }

    fn number_format(&self) -> Res<DecimalFormat> {
        Ok(DecimalFormat::number(self.symbols()))
    }

    fn format_number(&self, arg: &Value, f: &DecimalFormat) -> Res<String> {
        Ok(match arg {
            Value::Int(v) => f.format_i64(*v as i64),
            Value::Int64(v) => f.format_i64(*v),
            Value::Float(v) => f.format_f64(*v as f64),
            Value::Double(v) => f.format_f64(*v),
            Value::Str { .. } => return fail("a number transform needs a numeric argument"),
        })
    }

    /// The `ms_styletransforms` table: `currency number percent roman spellout ordinal upper lower title
    /// textkey gender plural`.
    fn apply_style(
        &self,
        style: &str,
        arg: &Value,
        injected: &[Value],
        style_args: &str,
        depth: u32,
    ) -> Res<String> {
        // re: 0x0d266250 ParameterEvaluator::ms_styletransforms
        let sym = self.symbols();
        match style {
            // re: 0x08c5b434 ParameterTransforms::FormatCurrency
            "currency" => self.format_number(arg, &DecimalFormat::currency(sym)),
            // re: 0x08c5b7c8 ParameterTransforms::FormatNumber
            "number" => {
                let mut fmt = self.number_format()?;
                if !style_args.is_empty() {
                    let pattern = number_pattern(style_args);
                    if !pattern.is_empty() {
                        fmt = match DecimalFormat::parse(pattern, sym) {
                            Ok(f) => f,
                            Err(e) => return fail(format!("{e} (\"{pattern}\")")),
                        };
                    }
                }
                self.format_number(arg, &fmt)
            }
            // re: 0x08c5b5fe ParameterTransforms::FormatPercent
            "percent" => self.format_number(arg, &DecimalFormat::percent(sym)),
            // re: 0x08c5bc40 ParameterTransforms::FormatRoman
            "roman" => {
                let n = match arg {
                    Value::Str { .. } => {
                        return fail("FormatRoman: Unsupported, must be number argument");
                    }
                    v => numeric_i64(v),
                };
                Ok(number::roman_upper(n).unwrap_or_else(|| n.to_string()))
            }
            // re: 0x08c5c016 ParameterTransforms::FormatSpellout
            "spellout" => match arg {
                Value::Int(v) => Ok(number::spellout_en_i64(*v as i64)),
                Value::Int64(v) => Ok(number::spellout_en_i64(*v)),
                Value::Float(v) => Ok(number::spellout_en_f64(*v as f64)),
                Value::Double(v) => Ok(number::spellout_en_f64(*v)),
                Value::Str { .. } => fail("FormatSpellout: Unsupported, must be number argument"),
            },
            // re: 0x08c5c34a ParameterTransforms::FormatOrdinal
            "ordinal" => match arg {
                Value::Str { .. } => fail("FormatOrdinal: Unsupported, must be number argument"),
                v => Ok(number::ordinal_en(numeric_i64(v), sym)),
            },
            // re: 0x08c5c67e ParameterTransforms::FormatUppercase
            "upper" => Ok(self.arg_text(arg, "FormatUppercase", depth)?.to_uppercase()),
            // re: 0x08c5c750 ParameterTransforms::FormatLowercase
            "lower" => Ok(self.arg_text(arg, "FormatLowercase", depth)?.to_lowercase()),
            // re: 0x08c5c822 ParameterTransforms::FormatTitlecase
            "title" => Ok(title_case(&self.arg_text(arg, "FormatTitlecase", depth)?)),
            // re: 0x08c5c8fc ParameterTransforms::FormatTextkey
            "textkey" => {
                let key = self.arg_text(arg, "FormatTextkey", depth)?;
                let looked = self.lookup_string(&key);
                let text = self.string_text(&looked, 1, &[], depth + 1);
                Ok(if text.is_empty() { key } else { text })
            }
            // re: 0x08c5c9f2 ParameterTransforms::ChoiceFormatGender
            "gender" => {
                let Value::Str { s, .. } = arg else {
                    return fail("'gender' transform: Unsupported argument, must be string.");
                };
                self.conditional(style_args, injected, depth, &|key| {
                    GenderSpec::parse(key).is_some_and(|g| s.form_by_gender(&g) == 1)
                })
            }
            // re: 0x08c5ca94 ParameterTransforms::ChoiceFormatPlurality
            "plural" => {
                // re: 0x08c5cb64 PluralityConditionalTransform: the plural form number as text
                let form = match arg {
                    Value::Str { s, .. } => s.form(1).map_or(0, |f| f.plurality),
                    v => self.plural_form(v.as_count().unwrap_or(0)),
                };
                let text = form.to_string();
                self.conditional(style_args, injected, depth, &|key| {
                    text.as_bytes().starts_with(key.as_bytes())
                })
            }
            other => fail(format!("unknown transform \"{other}\"")),
        }
    }

    /// The composed first form of a string argument (`m_str->toUTF8(len, 1)`, no arguments).
    fn arg_text(&self, arg: &Value, what: &str, depth: u32) -> Res<String> {
        match arg {
            Value::Str { s, .. } => Ok(self.string_text(s, 1, &[], depth + 1)),
            _ => fail(format!("{what}: Unsupported argument, must be string.")),
        }
    }

    /// `ConditionalTransform::Transform`: `key?value; key?value; other?value;`.
    fn conditional(
        &self,
        s: &str,
        args: &[Value],
        depth: u32,
        is_match: &dyn Fn(&str) -> bool,
    ) -> Res<String> {
        // re: 0x08c64828 ConditionalTransform::Transform
        if s.is_empty() {
            return fail("conditional transform requires arguments.");
        }
        let b = s.as_bytes();
        let n = b.len();
        let mut pos = 0usize;
        while pos < n {
            let Some(rel) = b[pos..].iter().position(|&c| c == b'?') else {
                return Ok(String::new());
            };
            let q = pos + rel;
            let key = &s[pos..q];
            let hit = is_match(key) || "other".as_bytes().starts_with(key.as_bytes());
            // the clause ends at the first ';' outside nested braces
            let mut depth_br = 0i32;
            let mut end = q;
            let mut found = false;
            while end < n {
                match b[end] {
                    b';' if depth_br == 0 => {
                        found = true;
                        break;
                    }
                    b'}' => depth_br -= 1,
                    b'{' => depth_br += 1,
                    _ => {}
                }
                end += 1;
            }
            if !found {
                return fail("conditional transform: End of string while searching for ';'");
            }
            if hit {
                let value = &s[q + 1..end];
                let ls = LocString::new(value);
                return Ok(self.string_text(&ls, 1, args, depth + 1));
            }
            pos = end + 1;
            while pos < n && matches!(b[pos], b' ' | b'\t') {
                pos += 1;
            }
        }
        Ok(String::new())
    }
}

fn numeric_i64(v: &Value) -> i64 {
    match v {
        Value::Int(v) => *v as i64,
        Value::Int64(v) => *v,
        Value::Float(v) => *v as i64,
        Value::Double(v) => *v as i64,
        Value::Str { .. } => 0,
    }
}

/// The pattern of `{N: number ...}`: the text between the first and last `"` (exe quirk: the length passed to
/// `substr` is `last - 1`, not `last - first - 1`), or the whole argument without quotes.
fn number_pattern(style_args: &str) -> &str {
    let (Some(first), Some(last)) = (style_args.find('"'), style_args.rfind('"')) else {
        return style_args;
    };
    let start = first + 1;
    let len = last.saturating_sub(1);
    let end = start.saturating_add(len).min(style_args.len());
    if start >= style_args.len() || !style_args.is_char_boundary(start) {
        return "";
    }
    let mut end = end;
    while !style_args.is_char_boundary(end) {
        end -= 1;
    }
    &style_args[start..end.max(start)]
}

/// ICU `toTitle` with the default word iterator: the first letter of each word upper, the rest lower.
fn title_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_word = false;
    let chars: Vec<char> = s.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c.is_alphanumeric() {
            if in_word {
                out.extend(c.to_lowercase());
            } else {
                out.extend(c.to_uppercase());
                in_word = true;
            }
        } else {
            // an apostrophe between letters stays inside the word
            let inner_apostrophe = (c == '\'' || c == '\u{2019}')
                && in_word
                && chars.get(i + 1).is_some_and(|n| n.is_alphabetic());
            if !inner_apostrophe {
                in_word = false;
            }
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_pattern_selection() {
        assert_eq!(number_pattern("#.##"), "#.##");
        assert_eq!(number_pattern("\"#.##\""), "#.##");
        assert_eq!(number_pattern("\"'+'##;'-'##\""), "'+'##;'-'##");
        assert_eq!(number_pattern("\"\""), "");
    }

    #[test]
    fn title_case_words() {
        assert_eq!(title_case("hELLO wORLD"), "Hello World");
        assert_eq!(
            title_case("the great wall of china"),
            "The Great Wall Of China"
        );
        assert_eq!(title_case("don't stop"), "Don't Stop");
    }
}
