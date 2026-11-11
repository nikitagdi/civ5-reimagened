//! Values of style bindings (`data-bind-style-<prop>`): the text a template renders, parsed by the type of the
//! property, as the profile allows for bindable properties: lengths (`12px`, `62%`, `auto`), numbers, colors
//! (`#rgb`, `#rrggbb`, `#rrggbbaa`, `rgb()`, `rgba()`, `transparent`, `var(--token)`) and transform lists
//! (`translate`, `translateX`, `translateY`, `rotate`, `scale`, `scaleX`, `scaleY`).

use ui2_package::style::{ColorV, Len, LenAuto, TFn};
use ui2_package::{Package, PropId, TokenId};

use crate::style::OverValue;

fn number(s: &str) -> Option<f32> {
    let v: f32 = s.trim().parse().ok()?;
    v.is_finite().then_some(v)
}

/// `12px`, `62%`, `0`.
fn len(s: &str) -> Option<Len> {
    let s = s.trim();
    if let Some(px) = s.strip_suffix("px") {
        return number(px).map(Len::px);
    }
    if let Some(pct) = s.strip_suffix('%') {
        return number(pct).map(Len::pct);
    }
    (number(s)? == 0.0).then_some(Len::ZERO)
}

fn hex(s: &str) -> Option<[u8; 4]> {
    let digits: Vec<u8> = s
        .chars()
        .map(|c| c.to_digit(16).map(|d| d as u8))
        .collect::<Option<_>>()?;
    let pair = |i: usize| digits[i] * 16 + digits[i + 1];
    Some(match digits.len() {
        3 => [digits[0] * 17, digits[1] * 17, digits[2] * 17, 255],
        4 => [
            digits[0] * 17,
            digits[1] * 17,
            digits[2] * 17,
            digits[3] * 17,
        ],
        6 => [pair(0), pair(2), pair(4), 255],
        8 => [pair(0), pair(2), pair(4), pair(6)],
        _ => return None,
    })
}

fn channel(s: &str) -> Option<u8> {
    let s = s.trim();
    let v = match s.strip_suffix('%') {
        Some(p) => number(p)? * 2.55,
        None => number(s)?,
    };
    Some(v.round().clamp(0.0, 255.0) as u8)
}

fn color(pkg: &Package, s: &str) -> Option<ColorV> {
    let s = s.trim();
    if s == "transparent" {
        return Some(ColorV::Lit([0, 0, 0, 0]));
    }
    if let Some(h) = s.strip_prefix('#') {
        return hex(h).map(ColorV::Lit);
    }
    if let Some(name) = s.strip_prefix("var(--").and_then(|r| r.strip_suffix(')')) {
        let want = format!("--{}", name.trim());
        let id = pkg.str_id(&want).or_else(|| pkg.str_id(name.trim()))?;
        let t = pkg.tokens();
        if let Some(i) = t.roots.iter().position(|(n, _)| *n == id) {
            return Some(ColorV::Tok(TokenId(i as u32)));
        }
        return t
            .aliases
            .iter()
            .find(|(n, _)| *n == id)
            .map(|(_, tok)| ColorV::Tok(*tok));
    }
    let inner = s
        .strip_prefix("rgba(")
        .or_else(|| s.strip_prefix("rgb("))?
        .strip_suffix(')')?;
    let parts: Vec<&str> = inner
        .split([',', '/', ' '])
        .filter(|p| !p.trim().is_empty())
        .collect();
    let (r, g, b) = (
        channel(parts.first()?)?,
        channel(parts.get(1)?)?,
        channel(parts.get(2)?)?,
    );
    let a = match parts.get(3) {
        Some(a) => match a.trim().strip_suffix('%') {
            Some(p) => number(p)? / 100.0,
            None => number(a)?,
        },
        None => 1.0,
    };
    Some(ColorV::Lit([
        r,
        g,
        b,
        (a.clamp(0.0, 1.0) * 255.0).round() as u8,
    ]))
}

fn angle(s: &str) -> Option<f32> {
    let s = s.trim();
    if let Some(d) = s.strip_suffix("deg") {
        return number(d);
    }
    if let Some(t) = s.strip_suffix("turn") {
        return number(t).map(|t| t * 360.0);
    }
    if let Some(r) = s.strip_suffix("rad") {
        return number(r).map(f32::to_degrees);
    }
    (number(s)? == 0.0).then_some(0.0)
}

fn transform(s: &str) -> Option<Vec<TFn>> {
    let s = s.trim();
    if s == "none" || s.is_empty() {
        return Some(Vec::new());
    }
    let mut out = Vec::new();
    let mut rest = s;
    while !rest.trim().is_empty() {
        let open = rest.find('(')?;
        let close = rest.find(')')?;
        let name = rest[..open].trim();
        let args: Vec<&str> = rest[open + 1..close].split(',').map(str::trim).collect();
        rest = &rest[close + 1..];
        let f = match (name, args.as_slice()) {
            ("translate", [x]) => TFn::Translate(len(x)?, Len::ZERO),
            ("translate", [x, y]) => TFn::Translate(len(x)?, len(y)?),
            ("translateX", [x]) => TFn::Translate(len(x)?, Len::ZERO),
            ("translateY", [y]) => TFn::Translate(Len::ZERO, len(y)?),
            ("rotate", [a]) => TFn::Rotate(angle(a)?),
            ("scale", [v]) => {
                let v = number(v)?;
                TFn::Scale(v, v)
            }
            ("scale", [x, y]) => TFn::Scale(number(x)?, number(y)?),
            ("scaleX", [x]) => TFn::Scale(number(x)?, 1.0),
            ("scaleY", [y]) => TFn::Scale(1.0, number(y)?),
            _ => return None,
        };
        out.push(f);
    }
    Some(out)
}

/// The value of a bindable property from the text a binding rendered; `None` when it does not parse (the
/// property keeps its previous value).
pub(crate) fn parse(pkg: &Package, prop: PropId, text: &str) -> Option<OverValue> {
    let t = text.trim();
    match prop {
        PropId::Top
        | PropId::Right
        | PropId::Bottom
        | PropId::Left
        | PropId::Width
        | PropId::Height => {
            if t == "auto" {
                Some(OverValue::Len(LenAuto::Auto))
            } else {
                len(t).map(|l| OverValue::Len(LenAuto::Len(l)))
            }
        }
        PropId::FlexGrow => number(t).filter(|v| *v >= 0.0).map(OverValue::Num),
        PropId::Opacity => match t.strip_suffix('%') {
            Some(p) => number(p).map(|v| OverValue::Num(v / 100.0)),
            None => number(t).map(OverValue::Num),
        },
        PropId::Color | PropId::BackgroundColor => color(pkg, t).map(OverValue::Color),
        PropId::Transform => transform(t).map(OverValue::Transform),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_colors_and_transforms_parse() {
        assert_eq!(len("62%"), Some(Len::pct(62.0)));
        assert_eq!(len(" 12.5px"), Some(Len::px(12.5)));
        assert_eq!(len("0"), Some(Len::ZERO));
        assert_eq!(len("12"), None);
        assert_eq!(hex("f80"), Some([255, 136, 0, 255]));
        assert_eq!(hex("11223344"), Some([0x11, 0x22, 0x33, 0x44]));
        assert_eq!(hex("12345"), None);
        assert_eq!(angle("0.5turn"), Some(180.0));
        assert_eq!(
            transform("translate(4px, 50%) rotate(90deg) scaleY(2)"),
            Some(vec![
                TFn::Translate(Len::px(4.0), Len::pct(50.0)),
                TFn::Rotate(90.0),
                TFn::Scale(1.0, 2.0),
            ])
        );
        assert_eq!(transform("skew(3deg)"), None);
    }
}
