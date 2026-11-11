//! Selectors and media queries of the profile subset.
//!
//! Selectors: type, `*`, `.class`, `#id`, `:root` in compounds; the combinators descendant and
//! `>`; the dynamic pseudo-classes `:hover :active :focus :focus-visible :disabled`; and
//! `input::placeholder`. Anything else is outside the profile. Media queries: `width` and `height`
//! with `min-`/`max-` or a range, in px, joined by `and`, with the type `all` or `screen`.
//!
//! The strings come from css-tree's generator (no comments, minimal spaces). The static part of a
//! compound is matched by the converter itself on the scanned DOM (the subset needs nothing
//! Chrome-specific); the dynamic parts become the conditions of the style variants.

use ui2_package::Bound;

/// A combinator between two compounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Comb {
    /// Whitespace: any ancestor.
    Descendant,
    /// `>`: the parent.
    Child,
}

/// A dynamic pseudo-class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Pseudo {
    /// `:hover`.
    Hover,
    /// `:active`.
    Active,
    /// `:focus`.
    Focus,
    /// `:focus-visible`.
    FocusVisible,
    /// `:disabled`.
    Disabled,
}

impl Pseudo {
    /// The name without the colon.
    pub fn name(self) -> &'static str {
        match self {
            Pseudo::Hover => "hover",
            Pseudo::Active => "active",
            Pseudo::Focus => "focus",
            Pseudo::FocusVisible => "focus-visible",
            Pseudo::Disabled => "disabled",
        }
    }

    fn from_name(s: &str) -> Option<Pseudo> {
        Some(match s {
            "hover" => Pseudo::Hover,
            "active" => Pseudo::Active,
            "focus" => Pseudo::Focus,
            "focus-visible" => Pseudo::FocusVisible,
            "disabled" => Pseudo::Disabled,
            _ => return None,
        })
    }
}

/// One compound selector.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Compound {
    /// The type selector (lower case), `None` for none or `*`.
    pub tag: Option<String>,
    /// `#id`s.
    pub ids: Vec<String>,
    /// `.class`es.
    pub classes: Vec<String>,
    /// `:root`.
    pub root: bool,
    /// Dynamic pseudo-classes.
    pub pseudos: Vec<Pseudo>,
    /// `::placeholder`.
    pub placeholder: bool,
}

/// A complex selector, left to right; the first combinator is `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selector {
    /// The compounds with the combinator before each.
    pub parts: Vec<(Option<Comb>, Compound)>,
}

impl Selector {
    /// The subject (rightmost) compound.
    pub fn subject(&self) -> &Compound {
        &self.parts[self.parts.len() - 1].1
    }
}

fn ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_' || !c.is_ascii()
}

fn take_ident(chars: &[char], i: &mut usize) -> String {
    let start = *i;
    while *i < chars.len() && ident_char(chars[*i]) {
        *i += 1;
    }
    chars[start..*i].iter().collect()
}

/// Parses one selector of a list. The error says what is outside the profile.
pub fn parse_selector(src: &str) -> Result<Selector, String> {
    let chars: Vec<char> = src.trim().chars().collect();
    if chars.is_empty() {
        return Err("empty selector".into());
    }
    let mut parts: Vec<(Option<Comb>, Compound)> = Vec::new();
    let mut cur = Compound::default();
    let mut cur_empty = true;
    let mut pending: Option<Comb> = None;
    let mut i = 0;
    let finish = |parts: &mut Vec<(Option<Comb>, Compound)>,
                  cur: &mut Compound,
                  cur_empty: &mut bool,
                  pending: &mut Option<Comb>|
     -> Result<(), String> {
        if *cur_empty {
            return Err("a combinator without a compound".into());
        }
        let comb = if parts.is_empty() {
            None
        } else {
            Some(pending.take().unwrap_or(Comb::Descendant))
        };
        parts.push((comb, std::mem::take(cur)));
        *cur_empty = true;
        Ok(())
    };
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\n' | '>' => {
                let mut child = false;
                while i < chars.len() && matches!(chars[i], ' ' | '\t' | '\n' | '>') {
                    if chars[i] == '>' {
                        if child {
                            return Err("`>>` is not a combinator".into());
                        }
                        child = true;
                    }
                    i += 1;
                }
                if i >= chars.len() {
                    if child {
                        return Err("a combinator at the end".into());
                    }
                    break;
                }
                if matches!(chars[i], '+' | '~') {
                    return Err(format!("the combinator `{}`", chars[i]));
                }
                finish(&mut parts, &mut cur, &mut cur_empty, &mut pending)?;
                pending = Some(if child { Comb::Child } else { Comb::Descendant });
            }
            '+' | '~' => return Err(format!("the combinator `{c}`")),
            '[' => return Err("an attribute selector".into()),
            '|' => return Err("a namespace".into()),
            '&' => return Err("nesting".into()),
            '*' => {
                i += 1;
                cur_empty = false;
            }
            '.' | '#' => {
                i += 1;
                let name = take_ident(&chars, &mut i);
                if name.is_empty() {
                    return Err(format!("`{c}` without a name"));
                }
                if c == '.' {
                    cur.classes.push(name);
                } else {
                    cur.ids.push(name);
                }
                cur_empty = false;
            }
            ':' => {
                let element = chars.get(i + 1) == Some(&':');
                i += if element { 2 } else { 1 };
                let name = take_ident(&chars, &mut i).to_ascii_lowercase();
                if chars.get(i) == Some(&'(') {
                    return Err(format!(":{name}()"));
                }
                if element {
                    if name == "placeholder" && cur.tag.as_deref() == Some("input") {
                        cur.placeholder = true;
                    } else {
                        return Err(format!("the pseudo-element ::{name}"));
                    }
                } else if name == "root" {
                    cur.root = true;
                } else if let Some(p) = Pseudo::from_name(&name) {
                    cur.pseudos.push(p);
                } else {
                    return Err(format!("the pseudo-class :{name}"));
                }
                cur_empty = false;
            }
            c if ident_char(c) => {
                if !cur_empty {
                    return Err("a type selector after another part".into());
                }
                let name = take_ident(&chars, &mut i).to_ascii_lowercase();
                cur.tag = Some(name);
                cur_empty = false;
            }
            '\\' => return Err("an escape".into()),
            other => return Err(format!("`{other}`")),
        }
    }
    finish(&mut parts, &mut cur, &mut cur_empty, &mut pending)?;
    Ok(Selector { parts })
}

/// A viewport axis of a media condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Axis {
    /// `width`.
    W,
    /// `height`.
    H,
}

fn px(s: &str) -> Result<f32, String> {
    let n = s
        .strip_suffix("px")
        .ok_or_else(|| format!("`{s}`: only px in a media query"))?;
    match n.parse::<f32>() {
        Ok(v) if v.is_finite() && v >= 0.0 => Ok(v),
        _ => Err(format!("`{s}` is not a length")),
    }
}

fn axis(name: &str) -> Result<Axis, String> {
    match name {
        "width" => Ok(Axis::W),
        "height" => Ok(Axis::H),
        other => Err(format!("the media feature `{other}`")),
    }
}

/// `feature op value` with op one of `< <= > >=`, normalised so the feature is on the left.
fn range_bound(op: &str, value: f32) -> Result<Bound, String> {
    Ok(match op {
        ">=" => Bound {
            px: value,
            upper_inclusive: false,
        },
        ">" => Bound {
            px: value,
            upper_inclusive: true,
        },
        "<" => Bound {
            px: value,
            upper_inclusive: false,
        },
        "<=" => Bound {
            px: value,
            upper_inclusive: true,
        },
        other => return Err(format!("the operator `{other}`")),
    })
}

fn flip(op: &str) -> &str {
    match op {
        "<" => ">",
        "<=" => ">=",
        ">" => "<",
        ">=" => "<=",
        other => other,
    }
}

fn split_ops(s: &str) -> Vec<String> {
    // "1280px<=width<1600px" -> ["1280px", "<=", "width", "<", "1600px"]
    let mut out = Vec::new();
    let mut cur = String::new();
    let b: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < b.len() {
        if matches!(b[i], '<' | '>' | '=') {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            let mut op = String::new();
            while i < b.len() && matches!(b[i], '<' | '>' | '=') {
                op.push(b[i]);
                i += 1;
            }
            out.push(op);
        } else {
            cur.push(b[i]);
            i += 1;
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn feature(body: &str) -> Result<Vec<(Axis, Bound)>, String> {
    let body: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    if let Some((name, value)) = body.split_once(':') {
        let (axis, op) = match name {
            "min-width" => (Axis::W, ">="),
            "max-width" => (Axis::W, "<="),
            "min-height" => (Axis::H, ">="),
            "max-height" => (Axis::H, "<="),
            "width" | "height" => {
                return Err(format!("`({name}:...)`: write min-/max- or a range"));
            }
            other => return Err(format!("the media feature `{other}`")),
        };
        return Ok(vec![(axis, range_bound(op, px(value)?)?)]);
    }
    let t = split_ops(&body);
    match t.as_slice() {
        [a, op, b] => {
            if a == "width" || a == "height" {
                Ok(vec![(axis(a)?, range_bound(op, px(b)?)?)])
            } else {
                Ok(vec![(axis(b)?, range_bound(flip(op), px(a)?)?)])
            }
        }
        [a, op1, f, op2, b] => {
            let ax = axis(f)?;
            Ok(vec![
                (ax, range_bound(flip(op1), px(a)?)?),
                (ax, range_bound(op2, px(b)?)?),
            ])
        }
        _ => Err(format!("`({body})`")),
    }
}

/// Parses a media prelude into its conditions (all must hold). The error says what is outside
/// the profile.
pub fn parse_media(prelude: &str) -> Result<Vec<(Axis, Bound)>, String> {
    let p = prelude.trim().to_ascii_lowercase();
    if p.contains(',') {
        return Err("a list of queries (or)".into());
    }
    let mut out = Vec::new();
    let mut rest = p.as_str();
    let mut first = true;
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        if !first {
            rest = rest
                .strip_prefix("and")
                .ok_or_else(|| format!("`{rest}`: conditions are joined by `and`"))?
                .trim_start();
        }
        first = false;
        if let Some(body) = rest.strip_prefix('(') {
            let close = body
                .find(')')
                .ok_or_else(|| "an unclosed condition".to_string())?;
            let inner = &body[..close];
            if inner.contains('(') {
                return Err("nested conditions".into());
            }
            out.extend(feature(inner)?);
            rest = &body[close + 1..];
        } else {
            let word: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphabetic())
                .collect();
            match word.as_str() {
                "all" | "screen" => rest = &rest[word.len()..],
                "only" => rest = &rest[word.len()..],
                "not" | "or" => return Err(format!("`{word}`")),
                "" => return Err(format!("`{rest}`")),
                other => return Err(format!("the media type `{other}`")),
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sel(s: &str) -> Selector {
        parse_selector(s).unwrap_or_else(|e| panic!("{s}: {e}"))
    }

    #[test]
    fn profile_selectors_parse() {
        let s = sel(".top .grp");
        assert_eq!(s.parts.len(), 2);
        assert_eq!(s.parts[1].0, Some(Comb::Descendant));
        let s = sel(".yields .yield>svg.i");
        assert_eq!(s.parts[2].0, Some(Comb::Child));
        assert_eq!(s.subject().tag.as_deref(), Some("svg"));
        assert_eq!(s.subject().classes, ["i"]);
        let s = sel(".mbtn:hover");
        assert_eq!(s.subject().pseudos, [Pseudo::Hover]);
        assert!(sel(":root").subject().root);
        assert!(sel("*").subject().tag.is_none());
        assert!(sel("input::placeholder").subject().placeholder);
        let s = sel(".a > .b:focus-visible");
        assert_eq!(s.parts[1].0, Some(Comb::Child));
        assert_eq!(s.subject().pseudos, [Pseudo::FocusVisible]);
    }

    #[test]
    fn selectors_outside_the_profile_are_refused() {
        for bad in [
            ".a+.b",
            ".a~.b",
            "li:nth-child(2)",
            ".x::after",
            "[data-x]",
            ".a:first-child",
            "svg|use",
            ".a:not(.b)",
            ".a:focus-within",
        ] {
            assert!(parse_selector(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn media_queries_become_bounds() {
        let b = |px, upper_inclusive| Bound {
            px,
            upper_inclusive,
        };
        assert_eq!(
            parse_media("(min-width:1600px)").unwrap(),
            [(Axis::W, b(1600.0, false))]
        );
        assert_eq!(
            parse_media("screen and (max-height:800px)").unwrap(),
            [(Axis::H, b(800.0, true))]
        );
        assert_eq!(
            parse_media("(width>=1280px) and (height<900px)").unwrap(),
            [(Axis::W, b(1280.0, false)), (Axis::H, b(900.0, false))]
        );
        assert_eq!(
            parse_media("(1280px<=width<1600px)").unwrap(),
            [(Axis::W, b(1280.0, false)), (Axis::W, b(1600.0, false))]
        );
        assert_eq!(
            parse_media("(1600px<width)").unwrap(),
            [(Axis::W, b(1600.0, true))]
        );
    }

    #[test]
    fn media_outside_the_profile_is_refused() {
        for bad in [
            "(prefers-color-scheme:dark)",
            "(orientation:landscape)",
            "(min-width:40em)",
            "print",
            "not screen",
            "(min-width:100px),(max-width:50px)",
            "(min-width:100px) or (max-width:50px)",
            "(width:100px)",
        ] {
            assert!(parse_media(bad).is_err(), "{bad}");
        }
    }
}
