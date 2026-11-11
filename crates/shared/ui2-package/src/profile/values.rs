//! Value parsers: the strings Chrome serializes (computed or specified values) into typed values.
//!
//! One parser per `ValueKind`. A parser never guesses: anything outside the profile is an error
//! with a code (`E2-...`, texts in `profile.toml`). NaN, infinities, viewport units and any unit
//! other than `px`, `%`, `s`, `ms`, `deg` are errors.

use super::ids::PropId;
use super::props::{Profile, PropDef, ValueKind};
use crate::style::{
    AspectRatio, BgRepeat, BgSize, ColorV, GradDir, Gradient, GridLine, ImageRepeat, ImageRepeats,
    ImageSlice, ImageWidth, Len, LenAuto, LineHeight, Origin, Point, PropSel, Radius, Repeat,
    Shadow, Side, Stop, TFn, Track, TrackBreadth, TrackSize, convex_at,
};

/// Most color stops one gradient may have.
pub const MAX_GRADIENT_STOPS: usize = 4;

/// The boxes (CSS px) a `clip-path` polygon must be convex in. The parser cannot know the size of
/// the node; a polygon mixing px and % is checked again at the node's size by the converter
/// (`style::convex_at`).
pub const CLIP_REFERENCE_BOXES: [[f32; 2]; 3] =
    [[1024.0, 1024.0], [4096.0, 512.0], [512.0, 4096.0]];

/// Error codes of the value parsers; the texts are in `profile.toml` (`error.<code>`).
pub mod codes {
    /// The value is not one the profile allows for the property.
    pub const OUTSIDE: &str = "E2-002";
    /// A viewport, container, font-relative or absolute unit instead of `px`.
    pub const UNIT: &str = "E2-003";
    /// A number that is NaN or infinite.
    pub const NOT_FINITE: &str = "E2-004";
    /// A number outside the property's range.
    pub const RANGE: &str = "E2-005";
    /// `currentcolor`, or a color function other than `rgb()`/`rgba()`.
    pub const COLOR: &str = "E2-006";
    /// `calc()` where the profile does not allow it.
    pub const CALC: &str = "E2-007";
    /// A list longer than the property allows.
    pub const TOO_MANY: &str = "E2-008";
    /// A grid feature outside the profile (`auto-fill`, named lines, `subgrid`, `fit-content()`).
    pub const GRID: &str = "E2-020";
    /// A clip shape other than a polygon.
    pub const CLIP: &str = "E2-021";
    /// A clip polygon that is not convex.
    pub const CONCAVE: &str = "E2-022";
    /// A picture reference outside the profile (format, `data:` URL, query).
    pub const PICTURE: &str = "E2-023";

    /// Every code, for the tests.
    pub const ALL: &[&str] = &[
        OUTSIDE, UNIT, NOT_FINITE, RANGE, COLOR, CALC, TOO_MANY, GRID, CLIP, CONCAVE, PICTURE,
    ];
}

/// A value the profile does not allow.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {message}")]
pub struct ValueError {
    /// The error code (`E2-...`).
    pub code: &'static str,
    /// What was wrong with this value.
    pub message: String,
}

fn err<T>(code: &'static str, message: impl Into<String>) -> Result<T, ValueError> {
    Err(ValueError {
        code,
        message: message.into(),
    })
}

/// A parsed value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// One keyword.
    Keyword(String),
    /// A `keyword-set` value: the combinable keywords present, in table order; empty for the
    /// standalone keyword (`normal`, `none`).
    KeywordSet(Vec<String>),
    /// A length.
    Length(Len),
    /// A float.
    Number(f32),
    /// An integer.
    Integer(i32),
    /// A color (always a literal: tokens are decided by the converter).
    Color(ColorV),
    /// A corner radius.
    Radius(Radius),
    /// A shadow list (empty for `none`).
    Shadows(Vec<Shadow>),
    /// A transform list (empty for `none`).
    Transform(Vec<TFn>),
    /// A transform origin.
    Origin(Origin),
    /// Times in ms.
    Times(Vec<f32>),
    /// Cubic Beziers `[x1, y1, x2, y2]`.
    Easings(Vec<[f32; 4]>),
    /// A `transition-property` list (empty for `none`).
    Properties(Vec<PropSel>),
    /// Font family names, unquoted.
    Families(Vec<String>),
    /// A line height.
    LineHeight(LineHeight),
    /// Background layers (empty for `none`).
    Images(Vec<BgLayer>),
    /// Background sizes, one per layer.
    BgSizes(Vec<BgSize>),
    /// A comma list of lengths.
    Lengths(Vec<Len>),
    /// Background repeats, one per layer.
    Repeats(Vec<BgRepeat>),
    /// A comma list of keywords.
    Keywords(Vec<String>),
    /// `aspect-ratio`.
    Ratio(AspectRatio),
    /// A grid track list (empty for `none`).
    Tracks(Vec<Track>),
    /// A grid placement.
    GridLine(GridLine),
    /// A picture as Chrome wrote it (`url("...")` without the wrapper): an absolute URL on the
    /// converter's server, which the converter turns into the path inside the skin.
    Url(String),
    /// `border-image-slice`.
    Slice(ImageSlice),
    /// `border-image-width`: top, right, bottom, left.
    ImageWidths([ImageWidth; 4]),
    /// `border-image-repeat`.
    ImageRepeats(ImageRepeats),
    /// A `clip-path` polygon (empty for `none`).
    Polygon(Vec<Point>),
}

/// One layer of a parsed `background-image`.
#[derive(Debug, Clone, PartialEq)]
pub enum BgLayer {
    /// A gradient.
    Gradient(Gradient),
    /// A picture, as Chrome wrote its URL (see `Value::Url`).
    Url(String),
}

/// Parses `text` as a value of `prop` against `Profile::v1()`.
pub fn parse_value(prop: PropId, text: &str) -> Result<Value, ValueError> {
    parse_value_in(Profile::v1(), prop, text)
}

/// Parses `text` as a value of `prop` against `profile`.
pub fn parse_value_in(profile: &Profile, prop: PropId, text: &str) -> Result<Value, ValueError> {
    let def = profile.def(prop);
    let s = text.trim();
    if s.is_empty() {
        return err(codes::OUTSIDE, "empty value");
    }
    let lower = s.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "inherit" | "initial" | "unset" | "revert" | "revert-layer"
    ) || lower.contains("var(")
    {
        return err(
            codes::OUTSIDE,
            "CSS-wide keywords and var() are resolved by Chrome before baking",
        );
    }
    match def.kind {
        ValueKind::Keyword if prop == PropId::OutlineStyle => {
            parse_keyword(def, s).map_err(|e| ValueError {
                code: e.code,
                message: format!(
                    "outline-style `{s}`: the browser's focus ring is not part of the profile; \
                     style :focus-visible yourself with `outline: <width> solid <color>`"
                ),
            })
        }
        ValueKind::Keyword => parse_keyword(def, s),
        ValueKind::KeywordSet => parse_keyword_set(def, s),
        ValueKind::Length => parse_length_value(def, s),
        ValueKind::Color => match keyword_of(def, s) {
            Some(k) => Ok(Value::Keyword(k)),
            None => Ok(Value::Color(ColorV::Lit(parse_color(s)?))),
        },
        ValueKind::Number => match keyword_of(def, s) {
            Some(k) => Ok(Value::Keyword(k)),
            None => parse_number_value(def, s),
        },
        ValueKind::Integer => match keyword_of(def, s) {
            Some(k) => Ok(Value::Keyword(k)),
            None => parse_integer_value(def, s),
        },
        ValueKind::Radius => parse_radius(def, s),
        ValueKind::Shadows => parse_shadows(def, prop, s),
        ValueKind::Transform => parse_transform(def, s),
        ValueKind::Origin => parse_origin(def, s),
        ValueKind::Times => parse_times(def, s),
        ValueKind::Easings => parse_easings(def, s),
        ValueKind::Properties => parse_properties(profile, s),
        ValueKind::Families => parse_families(s),
        ValueKind::LineHeight => parse_line_height(def, s),
        ValueKind::Images => parse_images(def, s),
        ValueKind::BgSizes => parse_bg_sizes(def, s),
        ValueKind::Lengths => parse_lengths(def, s),
        ValueKind::Repeats => parse_repeats(def, s),
        ValueKind::Keywords => parse_keywords(def, s),
        ValueKind::Ratio => parse_ratio(s),
        ValueKind::Tracks => parse_tracks(def, s, true),
        ValueKind::TrackSizes => parse_tracks(def, s, false),
        ValueKind::GridLine => parse_grid_line(s),
        ValueKind::Url => match keyword_of(def, s) {
            Some(k) => Ok(Value::Keyword(k)),
            None => Ok(Value::Url(parse_url(prop, s)?)),
        },
        ValueKind::Slice => parse_slice(s),
        ValueKind::ImageWidths => parse_image_widths(s),
        ValueKind::ImageRepeat => parse_image_repeat(def, s),
        ValueKind::Polygon => parse_polygon(def, s),
    }
}

// ---------------------------------------------------------------------------------------------
// Tokens

/// Splits at top-level occurrences of `sep` (outside parentheses and quotes), trimming the parts.
fn split_top(s: &str, sep: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut depth, mut quote, mut start) = (0i32, None::<char>, 0usize);
    let mut prev_backslash = false;
    for (i, c) in s.char_indices() {
        if let Some(q) = quote {
            if c == q && !prev_backslash {
                quote = None;
            }
            prev_backslash = c == '\\' && !prev_backslash;
            continue;
        }
        match c {
            '"' | '\'' => quote = Some(c),
            '(' => depth += 1,
            ')' => depth -= 1,
            c if c == sep && depth == 0 => {
                parts.push(s[start..i].trim());
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(s[start..].trim());
    parts
}

/// Splits at top-level whitespace, dropping empty parts.
fn split_ws_top(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0i32, None::<usize>);
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        if c.is_whitespace() && depth == 0 {
            if let Some(st) = start.take() {
                parts.push(&s[st..i]);
            }
        } else if start.is_none() {
            start = Some(i);
        }
    }
    if let Some(st) = start {
        parts.push(&s[st..]);
    }
    parts
}

/// A number with its unit (lower case, possibly empty).
#[derive(Debug, Clone)]
struct Dim {
    v: f64,
    unit: String,
}

fn is_nan_or_inf(lower: &str) -> bool {
    let t = lower.trim_start_matches(['+', '-']);
    matches!(t, "nan" | "inf" | "infinity")
}

fn parse_dim(tok: &str) -> Result<Dim, ValueError> {
    let lower = tok.trim().to_ascii_lowercase();
    if is_nan_or_inf(&lower) {
        return err(codes::NOT_FINITE, format!("{tok:?} is not a finite number"));
    }
    let b = lower.as_bytes();
    let mut i = 0;
    if matches!(b.first(), Some(b'+' | b'-')) {
        i += 1;
    }
    let digits_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let mut seen = i > digits_start;
    if i < b.len() && b[i] == b'.' {
        let frac = i + 1;
        let mut j = frac;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > frac || seen {
            seen = seen || j > frac;
            i = j;
        }
    }
    if !seen {
        return err(codes::OUTSIDE, format!("{tok:?} is not a number"));
    }
    // An exponent needs digits after it: `1em` is a unit, `1e3` a number.
    if i < b.len() && b[i] == b'e' {
        let mut j = i + 1;
        if j < b.len() && matches!(b[j], b'+' | b'-') {
            j += 1;
        }
        let exp = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > exp {
            i = j;
        }
    }
    let v: f64 = lower[..i].parse().map_err(|_| ValueError {
        code: codes::OUTSIDE,
        message: format!("{tok:?} is not a number"),
    })?;
    if !v.is_finite() {
        return err(codes::NOT_FINITE, format!("{tok:?} is not a finite number"));
    }
    Ok(Dim {
        v,
        unit: lower[i..].to_string(),
    })
}

fn bad_unit<T>(unit: &str) -> Result<T, ValueError> {
    const VIEWPORT: &[&str] = &[
        "vw", "vh", "vmin", "vmax", "vi", "vb", "svw", "svh", "svi", "svb", "lvw", "lvh", "lvi",
        "lvb", "dvw", "dvh", "dvi", "dvb",
    ];
    if VIEWPORT.contains(&unit) || unit.starts_with("cq") {
        err(
            codes::UNIT,
            format!("`{unit}` is viewport- or container-relative and would be baked into px"),
        )
    } else {
        err(
            codes::UNIT,
            format!("unit `{unit}` is not allowed here; Chrome serializes lengths in px"),
        )
    }
}

fn to_f32(v: f64) -> Result<f32, ValueError> {
    let f = v as f32;
    if f.is_finite() {
        Ok(f)
    } else {
        err(codes::NOT_FINITE, "the number does not fit a float")
    }
}

fn keyword_of(def: &PropDef, s: &str) -> Option<String> {
    def.values.iter().find(|v| v.as_str() == s).cloned()
}

// ---------------------------------------------------------------------------------------------
// Keywords

fn parse_keyword(def: &PropDef, s: &str) -> Result<Value, ValueError> {
    match keyword_of(def, s) {
        Some(k) => Ok(Value::Keyword(k)),
        None => err(
            codes::OUTSIDE,
            format!("`{s}` is not one of: {}", def.values.join(", ")),
        ),
    }
}

fn parse_keyword_set(def: &PropDef, s: &str) -> Result<Value, ValueError> {
    let tokens = split_ws_top(s);
    let (alone, parts) = def.values.split_first().ok_or_else(|| ValueError {
        code: codes::OUTSIDE,
        message: "the property has no keywords".into(),
    })?;
    if tokens.len() == 1 && tokens[0] == alone {
        return Ok(Value::KeywordSet(Vec::new()));
    }
    let mut present = vec![false; parts.len()];
    for t in &tokens {
        match parts.iter().position(|p| p == t) {
            Some(i) if !present[i] => present[i] = true,
            _ => {
                return err(
                    codes::OUTSIDE,
                    format!(
                        "`{t}` is not allowed here (use `{alone}` alone or: {})",
                        parts.join(", ")
                    ),
                );
            }
        }
    }
    let set = parts
        .iter()
        .zip(&present)
        .filter(|(_, p)| **p)
        .map(|(k, _)| k.clone())
        .collect();
    Ok(Value::KeywordSet(set))
}

// ---------------------------------------------------------------------------------------------
// Lengths

fn check_len_range(def: &PropDef, len: Len, text: &str) -> Result<Len, ValueError> {
    if !len.is_finite() {
        return err(codes::NOT_FINITE, format!("{text:?} is not finite"));
    }
    let low = |v: f32| def.min.is_some_and(|m| f64::from(v) < m);
    let high = |v: f32| def.max.is_some_and(|m| f64::from(v) > m);
    let out = if len.pct == 0.0 {
        low(len.px) || high(len.px)
    } else if len.px == 0.0 {
        low(len.pct)
    } else {
        false
    };
    if out {
        return err(
            codes::RANGE,
            format!("{text:?} is outside the allowed range"),
        );
    }
    Ok(len)
}

fn parse_len(def: &PropDef, tok: &str) -> Result<Len, ValueError> {
    let lower = tok.trim().to_ascii_lowercase();
    if lower.starts_with("calc(") {
        if !def.calc {
            return err(
                codes::CALC,
                "calc() is only allowed in decoration properties, not here",
            );
        }
        let len = parse_calc(&lower, def.pct)?;
        return check_len_range(def, len, tok);
    }
    let dim = parse_dim(tok)?;
    let len = match dim.unit.as_str() {
        "px" => Len::px(to_f32(dim.v)?),
        "%" if def.pct => Len::pct(to_f32(dim.v)?),
        "%" => return err(codes::OUTSIDE, "a percentage is not allowed here"),
        "" if dim.v == 0.0 => Len::ZERO,
        "" => return err(codes::OUTSIDE, format!("{tok:?} needs a unit (px)")),
        unit => return bad_unit(unit),
    };
    check_len_range(def, len, tok)
}

/// `calc(<term> (+|-) <term> ...)` over `px` and `%` terms: the forms Chrome serializes.
fn parse_calc(lower: &str, pct_ok: bool) -> Result<Len, ValueError> {
    let inner = lower
        .strip_prefix("calc(")
        .and_then(|r| r.strip_suffix(')'))
        .ok_or_else(|| ValueError {
            code: codes::OUTSIDE,
            message: format!("malformed calc: {lower:?}"),
        })?;
    if inner.contains("nan") || inner.contains("infinity") {
        return err(codes::NOT_FINITE, "calc() evaluates to a non-finite number");
    }
    let tokens: Vec<&str> = inner.split_whitespace().collect();
    let outside = || {
        err(
            codes::OUTSIDE,
            format!("calc form outside the profile (sums of px and % only): {lower:?}"),
        )
    };
    if tokens.is_empty() || tokens.len().is_multiple_of(2) {
        return outside();
    }
    let (mut px, mut pct) = (0f64, 0f64);
    let mut sign = 1f64;
    for (i, t) in tokens.iter().enumerate() {
        if i % 2 == 1 {
            sign = match *t {
                "+" => 1.0,
                "-" => -1.0,
                _ => return outside(),
            };
            continue;
        }
        let dim = parse_dim(t)?;
        match dim.unit.as_str() {
            "px" => px += sign * dim.v,
            "%" if pct_ok => pct += sign * dim.v,
            "%" => return err(codes::OUTSIDE, "a percentage is not allowed here"),
            "" if dim.v == 0.0 => {}
            "" => return outside(),
            unit => return bad_unit(unit),
        }
        sign = 1.0;
    }
    Ok(Len {
        px: to_f32(px)?,
        pct: to_f32(pct)?,
    })
}

fn parse_length_value(def: &PropDef, s: &str) -> Result<Value, ValueError> {
    if let Some(k) = keyword_of(def, s) {
        return Ok(Value::Keyword(k));
    }
    Ok(Value::Length(parse_len(def, s)?))
}

fn parse_radius(def: &PropDef, s: &str) -> Result<Value, ValueError> {
    let parts = split_ws_top(s);
    match parts.as_slice() {
        [one] => {
            let l = parse_len(def, one)?;
            Ok(Value::Radius(Radius { h: l, v: l }))
        }
        [h, v] => Ok(Value::Radius(Radius {
            h: parse_len(def, h)?,
            v: parse_len(def, v)?,
        })),
        _ => err(codes::OUTSIDE, "a radius is one or two lengths"),
    }
}

fn parse_origin(def: &PropDef, s: &str) -> Result<Value, ValueError> {
    let parts = split_ws_top(s);
    let axis = |tok: &str, horizontal: bool| -> Result<Len, ValueError> {
        let lower = tok.to_ascii_lowercase();
        let named = match (lower.as_str(), horizontal) {
            ("left", true) | ("top", false) => Some(0.0),
            ("center", _) => Some(50.0),
            ("right", true) | ("bottom", false) => Some(100.0),
            _ => None,
        };
        match named {
            Some(p) => Ok(Len::pct(p)),
            None => parse_len(def, tok),
        }
    };
    match parts.as_slice() {
        [x] => Ok(Value::Origin(Origin {
            x: axis(x, true)?,
            y: Len::pct(50.0),
        })),
        [x, y] => Ok(Value::Origin(Origin {
            x: axis(x, true)?,
            y: axis(y, false)?,
        })),
        _ => err(codes::OUTSIDE, "a transform origin is one or two values"),
    }
}

fn parse_line_height(def: &PropDef, s: &str) -> Result<Value, ValueError> {
    if s == "normal" {
        return Ok(Value::LineHeight(LineHeight::Normal));
    }
    let dim = parse_dim(s)?;
    let out = match dim.unit.as_str() {
        "px" => LineHeight::Px(to_f32(dim.v)?),
        "" => LineHeight::Factor(to_f32(dim.v)?),
        "%" => return err(codes::OUTSIDE, "line-height in % is baked to px by Chrome"),
        unit => return bad_unit(unit),
    };
    if def.min.is_some_and(|m| dim.v < m) {
        return err(codes::RANGE, format!("{s:?} is outside the allowed range"));
    }
    Ok(Value::LineHeight(out))
}

// ---------------------------------------------------------------------------------------------
// Numbers

fn check_range(def: &PropDef, v: f64, text: &str) -> Result<(), ValueError> {
    if def.min.is_some_and(|m| v < m) || def.max.is_some_and(|m| v > m) {
        return err(
            codes::RANGE,
            format!("{text:?} is outside the allowed range"),
        );
    }
    Ok(())
}

fn parse_number_value(def: &PropDef, s: &str) -> Result<Value, ValueError> {
    let dim = parse_dim(s)?;
    if !dim.unit.is_empty() {
        return err(codes::OUTSIDE, format!("{s:?} must be a plain number"));
    }
    check_range(def, dim.v, s)?;
    Ok(Value::Number(to_f32(dim.v)?))
}

fn parse_integer_value(def: &PropDef, s: &str) -> Result<Value, ValueError> {
    let dim = parse_dim(s)?;
    if !dim.unit.is_empty() || dim.v.fract() != 0.0 {
        return err(codes::OUTSIDE, format!("{s:?} must be an integer"));
    }
    check_range(def, dim.v, s)?;
    if dim.v < f64::from(i32::MIN) || dim.v > f64::from(i32::MAX) {
        return err(codes::RANGE, format!("{s:?} does not fit an integer"));
    }
    Ok(Value::Integer(dim.v as i32))
}

// ---------------------------------------------------------------------------------------------
// Colors

/// The CSS named colors (without `transparent`), sorted by name.
const NAMED: &[(&str, u32)] = &[
    ("aliceblue", 0xf0f8ff),
    ("antiquewhite", 0xfaebd7),
    ("aqua", 0x00ffff),
    ("aquamarine", 0x7fffd4),
    ("azure", 0xf0ffff),
    ("beige", 0xf5f5dc),
    ("bisque", 0xffe4c4),
    ("black", 0x000000),
    ("blanchedalmond", 0xffebcd),
    ("blue", 0x0000ff),
    ("blueviolet", 0x8a2be2),
    ("brown", 0xa52a2a),
    ("burlywood", 0xdeb887),
    ("cadetblue", 0x5f9ea0),
    ("chartreuse", 0x7fff00),
    ("chocolate", 0xd2691e),
    ("coral", 0xff7f50),
    ("cornflowerblue", 0x6495ed),
    ("cornsilk", 0xfff8dc),
    ("crimson", 0xdc143c),
    ("cyan", 0x00ffff),
    ("darkblue", 0x00008b),
    ("darkcyan", 0x008b8b),
    ("darkgoldenrod", 0xb8860b),
    ("darkgray", 0xa9a9a9),
    ("darkgreen", 0x006400),
    ("darkgrey", 0xa9a9a9),
    ("darkkhaki", 0xbdb76b),
    ("darkmagenta", 0x8b008b),
    ("darkolivegreen", 0x556b2f),
    ("darkorange", 0xff8c00),
    ("darkorchid", 0x9932cc),
    ("darkred", 0x8b0000),
    ("darksalmon", 0xe9967a),
    ("darkseagreen", 0x8fbc8f),
    ("darkslateblue", 0x483d8b),
    ("darkslategray", 0x2f4f4f),
    ("darkslategrey", 0x2f4f4f),
    ("darkturquoise", 0x00ced1),
    ("darkviolet", 0x9400d3),
    ("deeppink", 0xff1493),
    ("deepskyblue", 0x00bfff),
    ("dimgray", 0x696969),
    ("dimgrey", 0x696969),
    ("dodgerblue", 0x1e90ff),
    ("firebrick", 0xb22222),
    ("floralwhite", 0xfffaf0),
    ("forestgreen", 0x228b22),
    ("fuchsia", 0xff00ff),
    ("gainsboro", 0xdcdcdc),
    ("ghostwhite", 0xf8f8ff),
    ("gold", 0xffd700),
    ("goldenrod", 0xdaa520),
    ("gray", 0x808080),
    ("green", 0x008000),
    ("greenyellow", 0xadff2f),
    ("grey", 0x808080),
    ("honeydew", 0xf0fff0),
    ("hotpink", 0xff69b4),
    ("indianred", 0xcd5c5c),
    ("indigo", 0x4b0082),
    ("ivory", 0xfffff0),
    ("khaki", 0xf0e68c),
    ("lavender", 0xe6e6fa),
    ("lavenderblush", 0xfff0f5),
    ("lawngreen", 0x7cfc00),
    ("lemonchiffon", 0xfffacd),
    ("lightblue", 0xadd8e6),
    ("lightcoral", 0xf08080),
    ("lightcyan", 0xe0ffff),
    ("lightgoldenrodyellow", 0xfafad2),
    ("lightgray", 0xd3d3d3),
    ("lightgreen", 0x90ee90),
    ("lightgrey", 0xd3d3d3),
    ("lightpink", 0xffb6c1),
    ("lightsalmon", 0xffa07a),
    ("lightseagreen", 0x20b2aa),
    ("lightskyblue", 0x87cefa),
    ("lightslategray", 0x778899),
    ("lightslategrey", 0x778899),
    ("lightsteelblue", 0xb0c4de),
    ("lightyellow", 0xffffe0),
    ("lime", 0x00ff00),
    ("limegreen", 0x32cd32),
    ("linen", 0xfaf0e6),
    ("magenta", 0xff00ff),
    ("maroon", 0x800000),
    ("mediumaquamarine", 0x66cdaa),
    ("mediumblue", 0x0000cd),
    ("mediumorchid", 0xba55d3),
    ("mediumpurple", 0x9370db),
    ("mediumseagreen", 0x3cb371),
    ("mediumslateblue", 0x7b68ee),
    ("mediumspringgreen", 0x00fa9a),
    ("mediumturquoise", 0x48d1cc),
    ("mediumvioletred", 0xc71585),
    ("midnightblue", 0x191970),
    ("mintcream", 0xf5fffa),
    ("mistyrose", 0xffe4e1),
    ("moccasin", 0xffe4b5),
    ("navajowhite", 0xffdead),
    ("navy", 0x000080),
    ("oldlace", 0xfdf5e6),
    ("olive", 0x808000),
    ("olivedrab", 0x6b8e23),
    ("orange", 0xffa500),
    ("orangered", 0xff4500),
    ("orchid", 0xda70d6),
    ("palegoldenrod", 0xeee8aa),
    ("palegreen", 0x98fb98),
    ("paleturquoise", 0xafeeee),
    ("palevioletred", 0xdb7093),
    ("papayawhip", 0xffefd5),
    ("peachpuff", 0xffdab9),
    ("peru", 0xcd853f),
    ("pink", 0xffc0cb),
    ("plum", 0xdda0dd),
    ("powderblue", 0xb0e0e6),
    ("purple", 0x800080),
    ("rebeccapurple", 0x663399),
    ("red", 0xff0000),
    ("rosybrown", 0xbc8f8f),
    ("royalblue", 0x4169e1),
    ("saddlebrown", 0x8b4513),
    ("salmon", 0xfa8072),
    ("sandybrown", 0xf4a460),
    ("seagreen", 0x2e8b57),
    ("seashell", 0xfff5ee),
    ("sienna", 0xa0522d),
    ("silver", 0xc0c0c0),
    ("skyblue", 0x87ceeb),
    ("slateblue", 0x6a5acd),
    ("slategray", 0x708090),
    ("slategrey", 0x708090),
    ("snow", 0xfffafa),
    ("springgreen", 0x00ff7f),
    ("steelblue", 0x4682b4),
    ("tan", 0xd2b48c),
    ("teal", 0x008080),
    ("thistle", 0xd8bfd8),
    ("tomato", 0xff6347),
    ("turquoise", 0x40e0d0),
    ("violet", 0xee82ee),
    ("wheat", 0xf5deb3),
    ("white", 0xffffff),
    ("whitesmoke", 0xf5f5f5),
    ("yellow", 0xffff00),
    ("yellowgreen", 0x9acd32),
];

fn hex_digit(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

fn channel(v: f64) -> u8 {
    (v.clamp(0.0, 255.0) + 0.5).floor() as u8
}

/// One component of `rgb()`: a number (0..255) or a percentage.
fn rgb_component(tok: &str) -> Result<f64, ValueError> {
    let dim = parse_dim(tok)?;
    match dim.unit.as_str() {
        "" => Ok(dim.v),
        "%" => Ok(dim.v * 255.0 / 100.0),
        unit => bad_unit(unit),
    }
}

fn alpha_component(tok: &str) -> Result<u8, ValueError> {
    let dim = parse_dim(tok)?;
    let a = match dim.unit.as_str() {
        "" => dim.v,
        "%" => dim.v / 100.0,
        unit => return bad_unit(unit),
    };
    Ok(channel(a.clamp(0.0, 1.0) * 255.0))
}

/// Parses a color in the forms the profile allows. `currentcolor`, `hsl()`, `color-mix()` and the
/// rest are errors: Chrome resolves them to `rgb()` before the converter reads the value.
pub(crate) fn parse_color(s: &str) -> Result<[u8; 4], ValueError> {
    let lower = s.trim().to_ascii_lowercase();
    if let Some(hex) = lower.strip_prefix('#') {
        let d: Option<Vec<u8>> = hex.bytes().map(hex_digit).collect();
        let Some(d) = d else {
            return err(codes::OUTSIDE, format!("{s:?} is not a hex color"));
        };
        return match d.as_slice() {
            [r, g, b] => Ok([r * 17, g * 17, b * 17, 255]),
            [r, g, b, a] => Ok([r * 17, g * 17, b * 17, a * 17]),
            [r1, r2, g1, g2, b1, b2] => Ok([r1 * 16 + r2, g1 * 16 + g2, b1 * 16 + b2, 255]),
            [r1, r2, g1, g2, b1, b2, a1, a2] => {
                Ok([r1 * 16 + r2, g1 * 16 + g2, b1 * 16 + b2, a1 * 16 + a2])
            }
            _ => err(codes::OUTSIDE, format!("{s:?} is not a hex color")),
        };
    }
    if lower == "transparent" {
        return Ok([0, 0, 0, 0]);
    }
    if lower.contains("nan") || lower.contains("infinity") {
        return err(codes::NOT_FINITE, format!("{s:?} is not finite"));
    }
    for prefix in ["rgb(", "rgba("] {
        if let Some(inner) = lower.strip_prefix(prefix) {
            let Some(inner) = inner.strip_suffix(')') else {
                return err(codes::OUTSIDE, format!("{s:?} is not a color"));
            };
            let flat = inner.replace([',', '/'], " ");
            let parts: Vec<&str> = flat.split_whitespace().collect();
            return match parts.as_slice() {
                [r, g, b] => Ok([
                    channel(rgb_component(r)?),
                    channel(rgb_component(g)?),
                    channel(rgb_component(b)?),
                    255,
                ]),
                [r, g, b, a] => Ok([
                    channel(rgb_component(r)?),
                    channel(rgb_component(g)?),
                    channel(rgb_component(b)?),
                    alpha_component(a)?,
                ]),
                _ => err(codes::OUTSIDE, format!("{s:?} is not a color")),
            };
        }
    }
    if let Ok(i) = NAMED.binary_search_by(|(n, _)| (*n).cmp(lower.as_str())) {
        let rgb = NAMED[i].1;
        return Ok([(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 255]);
    }
    if lower == "currentcolor"
        || lower.contains('(')
        || lower.chars().all(|c| c.is_ascii_alphabetic())
    {
        return err(
            codes::COLOR,
            format!("{s:?}: only #hex, rgb(), rgba(), transparent and color names are baked"),
        );
    }
    err(codes::OUTSIDE, format!("{s:?} is not a color"))
}

// ---------------------------------------------------------------------------------------------
// Shadows

fn parse_shadows(def: &PropDef, prop: PropId, s: &str) -> Result<Value, ValueError> {
    if s == "none" {
        return Ok(Value::Shadows(Vec::new()));
    }
    let items = split_top(s, ',');
    if let Some(max) = def.max_items
        && items.len() > max
    {
        return err(
            codes::TOO_MANY,
            format!("at most {max} shadows are allowed"),
        );
    }
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        out.push(parse_shadow(prop == PropId::TextShadow, item)?);
    }
    Ok(Value::Shadows(out))
}

/// One shadow: `inset`, a color and 2 to 4 lengths in any order that keeps the lengths together.
/// A text shadow has no `inset` and no spread.
fn parse_shadow(text_shadow: bool, item: &str) -> Result<Shadow, ValueError> {
    let mut inset = false;
    let mut color = None;
    let mut lengths: Vec<f32> = Vec::new();
    let mut run_ended = false;
    for tok in split_ws_top(item) {
        let lower = tok.to_ascii_lowercase();
        let first = lower.as_bytes().first().copied().unwrap_or(b' ');
        let is_length =
            first.is_ascii_digit() || matches!(first, b'+' | b'-' | b'.') || is_nan_or_inf(&lower);
        if is_length {
            if run_ended {
                return err(
                    codes::OUTSIDE,
                    format!("{item:?}: lengths must be contiguous"),
                );
            }
            let dim = parse_dim(tok)?;
            let px = match dim.unit.as_str() {
                "px" => dim.v,
                "" if dim.v == 0.0 => 0.0,
                "" => return err(codes::OUTSIDE, format!("{tok:?} needs a unit (px)")),
                "%" => return err(codes::OUTSIDE, "a percentage is not allowed in a shadow"),
                unit => return bad_unit(unit),
            };
            lengths.push(to_f32(px)?);
            continue;
        }
        if !lengths.is_empty() {
            run_ended = true;
        }
        if lower == "inset" {
            if inset || text_shadow {
                return err(codes::OUTSIDE, "`inset` is not allowed here");
            }
            inset = true;
        } else if color.is_some() {
            return err(codes::OUTSIDE, format!("{item:?}: more than one color"));
        } else {
            color = Some(parse_color(tok)?);
        }
    }
    let Some(color) = color else {
        return err(codes::OUTSIDE, format!("{item:?}: a shadow needs a color"));
    };
    let (x, y, blur, spread) = match lengths.as_slice() {
        [x, y] => (*x, *y, 0.0, 0.0),
        [x, y, b] => (*x, *y, *b, 0.0),
        [x, y, b, sp] => (*x, *y, *b, *sp),
        _ => {
            return err(
                codes::OUTSIDE,
                format!("{item:?}: a shadow has 2 to 4 lengths"),
            );
        }
    };
    if blur < 0.0 {
        return err(codes::RANGE, "a shadow blur cannot be negative");
    }
    if text_shadow && spread != 0.0 {
        return err(codes::OUTSIDE, "a text shadow has no spread");
    }
    Ok(Shadow {
        inset,
        x,
        y,
        blur,
        spread,
        color: ColorV::Lit(color),
    })
}
// ---------------------------------------------------------------------------------------------
// Transforms

fn parse_transform(def: &PropDef, s: &str) -> Result<Value, ValueError> {
    if s == "none" {
        return Ok(Value::Transform(Vec::new()));
    }
    let mut out = Vec::new();
    for part in split_ws_top(s) {
        let Some(open) = part.find('(') else {
            return err(
                codes::OUTSIDE,
                format!("{part:?} is not a transform function"),
            );
        };
        let Some(inner) = part[open + 1..].strip_suffix(')') else {
            return err(
                codes::OUTSIDE,
                format!("{part:?} is not a transform function"),
            );
        };
        let name = part[..open].to_ascii_lowercase();
        let args = split_top(inner, ',');
        let len = |i: usize| parse_len(def, args[i]);
        let scale = |i: usize| -> Result<f32, ValueError> {
            let d = parse_dim(args[i])?;
            if !d.unit.is_empty() {
                return err(
                    codes::OUTSIDE,
                    format!("{:?} must be a plain number", args[i]),
                );
            }
            to_f32(d.v)
        };
        let f = match (name.as_str(), args.len()) {
            ("translate", 1) => TFn::Translate(len(0)?, Len::ZERO),
            ("translate", 2) => TFn::Translate(len(0)?, len(1)?),
            ("translatex", 1) => TFn::Translate(len(0)?, Len::ZERO),
            ("translatey", 1) => TFn::Translate(Len::ZERO, len(0)?),
            ("rotate", 1) => {
                let d = parse_dim(args[0])?;
                match d.unit.as_str() {
                    "deg" => TFn::Rotate(to_f32(d.v)?),
                    "" if d.v == 0.0 => TFn::Rotate(0.0),
                    unit => {
                        return err(
                            codes::OUTSIDE,
                            format!("rotate takes degrees (`deg`), not `{unit}`"),
                        );
                    }
                }
            }
            ("scale", 1) => {
                let k = scale(0)?;
                TFn::Scale(k, k)
            }
            ("scale", 2) => TFn::Scale(scale(0)?, scale(1)?),
            ("scalex", 1) => TFn::Scale(scale(0)?, 1.0),
            ("scaley", 1) => TFn::Scale(1.0, scale(0)?),
            _ => {
                return err(
                    codes::OUTSIDE,
                    format!(
                        "transform function `{name}` with {} arguments is outside the profile",
                        args.len()
                    ),
                );
            }
        };
        out.push(f);
    }
    Ok(Value::Transform(out))
}

// ---------------------------------------------------------------------------------------------
// Transitions

fn parse_times(def: &PropDef, s: &str) -> Result<Value, ValueError> {
    let mut out = Vec::new();
    for item in split_top(s, ',') {
        let dim = parse_dim(item)?;
        let ms = match dim.unit.as_str() {
            "s" => dim.v * 1000.0,
            "ms" => dim.v,
            "" => return err(codes::OUTSIDE, format!("{item:?} needs a unit (s or ms)")),
            unit => return bad_unit(unit),
        };
        check_range(def, ms, item)?;
        out.push(to_f32(ms)?);
    }
    Ok(Value::Times(out))
}

fn parse_easings(_def: &PropDef, s: &str) -> Result<Value, ValueError> {
    let mut out = Vec::new();
    for item in split_top(s, ',') {
        let lower = item.to_ascii_lowercase();
        let bezier = match lower.as_str() {
            "linear" => [0.0, 0.0, 1.0, 1.0],
            "ease" => [0.25, 0.1, 0.25, 1.0],
            "ease-in" => [0.42, 0.0, 1.0, 1.0],
            "ease-out" => [0.0, 0.0, 0.58, 1.0],
            "ease-in-out" => [0.42, 0.0, 0.58, 1.0],
            _ => {
                let Some(inner) = lower
                    .strip_prefix("cubic-bezier(")
                    .and_then(|r| r.strip_suffix(')'))
                else {
                    return err(
                        codes::OUTSIDE,
                        format!("timing function {item:?} is outside the profile"),
                    );
                };
                let nums: Vec<&str> = inner.split(',').map(str::trim).collect();
                if nums.len() != 4 {
                    return err(codes::OUTSIDE, "cubic-bezier takes four numbers");
                }
                let mut b = [0f32; 4];
                for (slot, tok) in b.iter_mut().zip(&nums) {
                    let d = parse_dim(tok)?;
                    if !d.unit.is_empty() {
                        return err(codes::OUTSIDE, format!("{tok:?} must be a plain number"));
                    }
                    *slot = to_f32(d.v)?;
                }
                if !(0.0..=1.0).contains(&b[0]) || !(0.0..=1.0).contains(&b[2]) {
                    return err(codes::RANGE, "cubic-bezier x values must be within 0..1");
                }
                b
            }
        };
        out.push(bezier);
    }
    Ok(Value::Easings(out))
}

fn parse_properties(profile: &Profile, s: &str) -> Result<Value, ValueError> {
    if s == "none" {
        return Ok(Value::Properties(Vec::new()));
    }
    let items = split_top(s, ',');
    if items.contains(&"all") {
        return if items.len() == 1 {
            Ok(Value::Properties(vec![PropSel::All]))
        } else {
            err(
                codes::OUTSIDE,
                "`all` cannot be listed with other properties",
            )
        };
    }
    let mut out = Vec::new();
    for item in items {
        let Some(id) = PropId::from_name(item) else {
            return err(
                codes::OUTSIDE,
                format!("`{item}` is not a property of the profile"),
            );
        };
        if !profile.def(id).anim {
            return err(
                codes::OUTSIDE,
                format!(
                    "`{item}` cannot be transitioned (only colors, opacity, shadows and transforms)"
                ),
            );
        }
        out.push(PropSel::Prop(id));
    }
    Ok(Value::Properties(out))
}

// ---------------------------------------------------------------------------------------------
// Font families

fn parse_families(s: &str) -> Result<Value, ValueError> {
    let mut out = Vec::new();
    for item in split_top(s, ',') {
        let name = if let Some(q) = item.chars().next().filter(|c| matches!(c, '"' | '\'')) {
            let Some(body) = item[1..].strip_suffix(q) else {
                return err(codes::OUTSIDE, format!("{item:?}: unterminated quote"));
            };
            body.replace("\\\"", "\"")
                .replace("\\'", "'")
                .replace("\\\\", "\\")
        } else {
            item.split_whitespace().collect::<Vec<_>>().join(" ")
        };
        if name.is_empty() {
            return err(codes::OUTSIDE, "empty font family name");
        }
        out.push(name);
    }
    Ok(Value::Families(out))
}

// ---------------------------------------------------------------------------------------------
// Background layers

/// The items of a comma list, at most `max_items` of them.
fn list_items<'a>(def: &PropDef, s: &'a str, what: &str) -> Result<Vec<&'a str>, ValueError> {
    let items = split_top(s, ',');
    if let Some(max) = def.max_items
        && items.len() > max
    {
        return err(
            codes::TOO_MANY,
            format!("at most {max} {what} are allowed, got {}", items.len()),
        );
    }
    if items.iter().any(|i| i.is_empty()) {
        return err(codes::OUTSIDE, format!("{s:?}: empty list item"));
    }
    Ok(items)
}

fn parse_images(def: &PropDef, s: &str) -> Result<Value, ValueError> {
    if s == "none" {
        return Ok(Value::Images(Vec::new()));
    }
    let mut out = Vec::new();
    for item in list_items(def, s, "background layers")? {
        if item.to_ascii_lowercase().starts_with("url(") {
            out.push(BgLayer::Url(parse_url(PropId::BackgroundImage, item)?));
        } else {
            out.push(BgLayer::Gradient(parse_gradient(def, item)?));
        }
    }
    Ok(Value::Images(out))
}

fn parse_gradient(def: &PropDef, item: &str) -> Result<Gradient, ValueError> {
    let lower = item.to_ascii_lowercase();
    let Some(open) = lower.find('(') else {
        return err(
            codes::OUTSIDE,
            format!(
                "{item:?} is not a gradient; background-image takes linear and conic gradients"
            ),
        );
    };
    let (conic, repeating) = match lower[..open].trim() {
        "linear-gradient" => (false, false),
        "repeating-linear-gradient" => (false, true),
        "conic-gradient" => (true, false),
        "repeating-conic-gradient" => (true, true),
        "image-set" | "-webkit-image-set" => {
            return err(
                codes::PICTURE,
                "image-set() is outside the profile; write url() and put the 2x file next to it as <name>@2x.<ext>",
            );
        }
        name => {
            return err(
                codes::OUTSIDE,
                format!("`{name}()` is outside the profile (linear and conic gradients only)"),
            );
        }
    };
    let Some(inner) = item[open + 1..].strip_suffix(')') else {
        return err(codes::OUTSIDE, format!("{item:?}: malformed gradient"));
    };
    let args = split_top(inner, ',');
    let head = args[0].to_ascii_lowercase();
    let head_first = split_ws_top(&head).first().copied().unwrap_or("");
    let has_head = matches!(head_first, "to" | "from" | "at" | "in")
        || head_first
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.'));
    let dir = match (has_head, conic) {
        (false, false) => GradDir::To(Side::Bottom),
        (false, true) => GradDir::Angle(0.0),
        (true, false) => parse_linear_head(&head)?,
        (true, true) => parse_conic_head(&head)?,
    };
    let stops = if has_head { &args[1..] } else { &args[..] };
    if stops.len() > MAX_GRADIENT_STOPS {
        return err(
            codes::TOO_MANY,
            format!(
                "a gradient has at most {MAX_GRADIENT_STOPS} color stops, got {}",
                stops.len()
            ),
        );
    }
    if stops.len() < 2 {
        return err(codes::OUTSIDE, "a gradient needs at least two color stops");
    }
    let stops = stops
        .iter()
        .map(|st| parse_stop(def, conic, st))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Gradient {
        conic,
        repeating,
        dir,
        stops,
    })
}

/// An angle in degrees (`0` alone is allowed).
fn parse_deg(tok: &str) -> Result<f32, ValueError> {
    let d = parse_dim(tok)?;
    match d.unit.as_str() {
        "deg" => to_f32(d.v),
        "" if d.v == 0.0 => Ok(0.0),
        unit => err(
            codes::OUTSIDE,
            format!("a gradient angle is in degrees (`deg`), not `{unit}`"),
        ),
    }
}

/// `<angle>` or `to <side>` / `to <corner>` (the two words of a corner in either order).
fn parse_linear_head(head: &str) -> Result<GradDir, ValueError> {
    let words = split_ws_top(head);
    match words.as_slice() {
        ["in", ..] => err(
            codes::OUTSIDE,
            "a color interpolation method (`in ...`) is outside the profile",
        ),
        ["to", sides @ ..] if !sides.is_empty() && sides.len() <= 2 => {
            let (mut h, mut v) = (None, None);
            for w in sides {
                let slot = match *w {
                    "left" | "right" => &mut h,
                    "top" | "bottom" => &mut v,
                    _ => return err(codes::OUTSIDE, format!("`to {w}` is not a side")),
                };
                if slot.replace(*w).is_some() {
                    return err(codes::OUTSIDE, format!("{head:?}: two sides of one axis"));
                }
            }
            let side = match (h, v) {
                (Some("left"), None) => Side::Left,
                (Some(_), None) => Side::Right,
                (None, Some("top")) => Side::Top,
                (None, _) => Side::Bottom,
                (Some("left"), Some("top")) => Side::TopLeft,
                (Some(_), Some("top")) => Side::TopRight,
                (Some("left"), Some(_)) => Side::BottomLeft,
                (Some(_), Some(_)) => Side::BottomRight,
            };
            Ok(GradDir::To(side))
        }
        [angle] => Ok(GradDir::Angle(parse_deg(angle)?)),
        _ => err(
            codes::OUTSIDE,
            format!("{head:?} is not a gradient direction (an angle in deg or `to <side>`)"),
        ),
    }
}

/// `from <angle>` and `at center` (Chrome drops a centered `at`).
fn parse_conic_head(head: &str) -> Result<GradDir, ValueError> {
    let words = split_ws_top(head);
    let mut angle = 0.0;
    let mut i = 0;
    if words.first() == Some(&"from") {
        let Some(a) = words.get(1) else {
            return err(codes::OUTSIDE, "`from` needs an angle");
        };
        angle = parse_deg(a)?;
        i = 2;
    }
    if words.get(i) == Some(&"at") {
        let at = &words[i + 1..];
        if at.is_empty() || !at.iter().all(|w| matches!(*w, "center" | "50%")) {
            return err(
                codes::OUTSIDE,
                "a conic gradient is centered: `at` other than `center` is outside the profile",
            );
        }
        i = words.len();
    }
    if words.get(i) == Some(&"in") {
        return err(
            codes::OUTSIDE,
            "a color interpolation method (`in ...`) is outside the profile",
        );
    }
    if i != words.len() || i == 0 {
        return err(
            codes::OUTSIDE,
            format!("{head:?} is not a conic gradient start (`from <angle>`)"),
        );
    }
    Ok(GradDir::Angle(angle))
}

/// `<color> [<position>]`. Linear positions are lengths (calc allowed); conic positions are
/// degrees or percentages, stored as percent of a turn.
fn parse_stop(def: &PropDef, conic: bool, item: &str) -> Result<Stop, ValueError> {
    let parts = split_ws_top(item);
    let Some(first) = parts.first() else {
        return err(codes::OUTSIDE, "empty color stop");
    };
    let starts_numeric = first
        .bytes()
        .next()
        .is_some_and(|b| b.is_ascii_digit() || matches!(b, b'+' | b'-' | b'.'))
        || first.to_ascii_lowercase().starts_with("calc(");
    if starts_numeric {
        return err(
            codes::OUTSIDE,
            format!("{item:?}: a color hint (a position without a color) is outside the profile"),
        );
    }
    let color = ColorV::Lit(parse_color(first)?);
    let pos = match &parts[1..] {
        [] => None,
        [p] if conic => {
            let d = parse_dim(p).map_err(|e| ValueError {
                code: e.code,
                message: format!("{p:?}: a conic stop position is an angle (deg) or a percentage"),
            })?;
            let pct = match d.unit.as_str() {
                "deg" => d.v / 3.6,
                "%" => d.v,
                "" if d.v == 0.0 => 0.0,
                unit => {
                    return err(
                        codes::OUTSIDE,
                        format!("a conic stop position is in deg or %, not `{unit}`"),
                    );
                }
            };
            Some(Len::pct(to_f32(pct)?))
        }
        [p] => Some(parse_len(def, p)?),
        _ => {
            return err(
                codes::OUTSIDE,
                format!("{item:?}: a color stop is a color and at most one position"),
            );
        }
    };
    Ok(Stop { color, pos })
}

fn parse_bg_sizes(def: &PropDef, s: &str) -> Result<Value, ValueError> {
    let len_auto = |t: &str| -> Result<LenAuto, ValueError> {
        if t == "auto" {
            Ok(LenAuto::Auto)
        } else {
            Ok(LenAuto::Len(parse_len(def, t)?))
        }
    };
    let mut out = Vec::new();
    for item in list_items(def, s, "background sizes")? {
        let size = match item {
            "cover" => BgSize::Cover,
            "contain" => BgSize::Contain,
            _ => match split_ws_top(item).as_slice() {
                [w] => BgSize::Size(len_auto(w)?, LenAuto::Auto),
                [w, h] => BgSize::Size(len_auto(w)?, len_auto(h)?),
                _ => {
                    return err(
                        codes::OUTSIDE,
                        format!(
                            "{item:?}: a background size is cover, contain or one or two lengths"
                        ),
                    );
                }
            },
        };
        out.push(size);
    }
    Ok(Value::BgSizes(out))
}

fn parse_lengths(def: &PropDef, s: &str) -> Result<Value, ValueError> {
    let mut out = Vec::new();
    for item in list_items(def, s, "positions")? {
        out.push(parse_len(def, item)?);
    }
    Ok(Value::Lengths(out))
}

fn parse_repeats(def: &PropDef, s: &str) -> Result<Value, ValueError> {
    let axis = |w: &str| -> Result<Repeat, ValueError> {
        match w {
            "repeat" => Ok(Repeat::Repeat),
            "no-repeat" => Ok(Repeat::NoRepeat),
            "space" | "round" => err(
                codes::OUTSIDE,
                format!(
                    "background-repeat `{w}` is outside the profile (repeat, no-repeat, repeat-x, repeat-y)"
                ),
            ),
            _ => err(codes::OUTSIDE, format!("`{w}` is not a background repeat")),
        }
    };
    let mut out = Vec::new();
    for item in list_items(def, s, "background repeats")? {
        let (x, y) = match split_ws_top(item).as_slice() {
            ["repeat-x"] => (Repeat::Repeat, Repeat::NoRepeat),
            ["repeat-y"] => (Repeat::NoRepeat, Repeat::Repeat),
            [one] => {
                let r = axis(one)?;
                (r, r)
            }
            [x, y] => (axis(x)?, axis(y)?),
            _ => {
                return err(
                    codes::OUTSIDE,
                    format!("{item:?}: a background repeat is one or two keywords"),
                );
            }
        };
        out.push(BgRepeat { x, y });
    }
    Ok(Value::Repeats(out))
}

fn parse_keywords(def: &PropDef, s: &str) -> Result<Value, ValueError> {
    let mut out = Vec::new();
    for item in list_items(def, s, "items")? {
        match keyword_of(def, item) {
            Some(k) => out.push(k),
            None => {
                return err(
                    codes::OUTSIDE,
                    format!("`{item}` is not one of: {}", def.values.join(", ")),
                );
            }
        }
    }
    Ok(Value::Keywords(out))
}

// ---------------------------------------------------------------------------------------------
// Pictures

/// The picture formats a property may refer to.
fn picture_formats(prop: PropId) -> &'static [&'static str] {
    match prop {
        // A 9-slice frame is a raster picture.
        PropId::BorderImageSource => &["png", "webp"],
        _ => &["png", "webp", "svg"],
    }
}

/// `url("...")` (or unquoted) of a picture file: the URL as written, checked for its format.
fn parse_url(prop: PropId, s: &str) -> Result<String, ValueError> {
    let lower = s.to_ascii_lowercase();
    if !lower.starts_with("url(") {
        return err(
            codes::OUTSIDE,
            format!("{s:?}: {} takes none or url() of a picture", prop.name()),
        );
    }
    let Some(inner) = s[4..].strip_suffix(')').map(str::trim) else {
        return err(codes::OUTSIDE, format!("{s:?}: malformed url()"));
    };
    let url = match inner.chars().next() {
        Some(q @ ('"' | '\'')) => match inner[1..].strip_suffix(q) {
            Some(body) => body.replace(&format!("\\{q}"), &q.to_string()),
            None => return err(codes::OUTSIDE, format!("{s:?}: unterminated quote")),
        },
        _ => inner.to_string(),
    };
    let formats = picture_formats(prop);
    let want = || format!("a {} file of the skin", formats.join(", "));
    if url.is_empty() {
        return err(codes::PICTURE, format!("an empty url(): {}", want()));
    }
    if url.to_ascii_lowercase().starts_with("data:") {
        return err(
            codes::PICTURE,
            format!("an inline data: picture; save it as {}", want()),
        );
    }
    if url.contains(['?', '#']) {
        return err(
            codes::PICTURE,
            format!("{url:?}: a picture reference has no query or fragment"),
        );
    }
    let file = url.rsplit('/').next().unwrap_or(&url);
    let ext = file
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .unwrap_or_default();
    if !formats.contains(&ext.as_str()) {
        return err(
            codes::PICTURE,
            format!("{url:?}: {} takes {}", prop.name(), want()),
        );
    }
    Ok(url)
}

// ---------------------------------------------------------------------------------------------
// Grid

fn parse_ratio(s: &str) -> Result<Value, ValueError> {
    if s == "auto" {
        return Ok(Value::Ratio(AspectRatio::Auto));
    }
    let words = split_ws_top(s);
    if words.contains(&"auto") {
        return err(
            codes::OUTSIDE,
            format!(
                "aspect-ratio {s:?}: `auto` with a ratio is outside the profile; write the ratio alone"
            ),
        );
    }
    let num = |t: &str| -> Result<f32, ValueError> {
        let d = parse_dim(t)?;
        if !d.unit.is_empty() {
            return err(
                codes::OUTSIDE,
                format!("{t:?}: a ratio is two plain numbers"),
            );
        }
        if d.v <= 0.0 {
            return err(
                codes::RANGE,
                format!("aspect-ratio {s:?}: both numbers must be positive"),
            );
        }
        to_f32(d.v)
    };
    match s.split_once('/') {
        Some((w, h)) => Ok(Value::Ratio(AspectRatio::Ratio(
            num(w.trim())?,
            num(h.trim())?,
        ))),
        None => Ok(Value::Ratio(AspectRatio::Ratio(num(s)?, 1.0))),
    }
}

fn grid_feature<T>(what: impl std::fmt::Display) -> Result<T, ValueError> {
    err(codes::GRID, format!("{what} is outside the profile"))
}

/// One breadth: a length (px, %), `<n>fr`, `auto`, `min-content`, `max-content`.
fn parse_breadth(def: &PropDef, t: &str) -> Result<TrackBreadth, ValueError> {
    let lower = t.to_ascii_lowercase();
    match lower.as_str() {
        "auto" => return Ok(TrackBreadth::Auto),
        "min-content" => return Ok(TrackBreadth::MinContent),
        "max-content" => return Ok(TrackBreadth::MaxContent),
        _ => {}
    }
    if lower.starts_with("fit-content(") {
        return grid_feature("the track size fit-content()");
    }
    if !lower.starts_with("calc(")
        && let Ok(d) = parse_dim(t)
        && d.unit == "fr"
    {
        if d.v < 0.0 {
            return err(
                codes::RANGE,
                format!("{t:?}: a flexible track is not negative"),
            );
        }
        return Ok(TrackBreadth::Fr(to_f32(d.v)?));
    }
    Ok(TrackBreadth::Len(parse_len(def, t)?))
}

fn parse_track_size(def: &PropDef, t: &str) -> Result<TrackSize, ValueError> {
    let lower = t.to_ascii_lowercase();
    if let Some(inner) = lower
        .strip_prefix("minmax(")
        .and_then(|r| r.strip_suffix(')'))
    {
        let args = split_top(inner, ',');
        let [min, max] = args.as_slice() else {
            return err(codes::OUTSIDE, format!("{t:?}: minmax() takes two sizes"));
        };
        let min = parse_breadth(def, min)?;
        if matches!(min, TrackBreadth::Fr(_)) {
            return err(
                codes::OUTSIDE,
                format!("{t:?}: the minimum of minmax() is not fr"),
            );
        }
        return Ok(TrackSize::MinMax(min, parse_breadth(def, max)?));
    }
    if lower.starts_with("repeat(") {
        return err(
            codes::OUTSIDE,
            format!("{t:?}: repeat() is not allowed here"),
        );
    }
    Ok(TrackSize::Breadth(parse_breadth(def, t)?))
}

/// A track list: `none` (templates only), sizes and `repeat(N, sizes)` (templates only).
fn parse_tracks(def: &PropDef, s: &str, template: bool) -> Result<Value, ValueError> {
    let lower = s.to_ascii_lowercase();
    if template && lower == "none" {
        return Ok(Value::Tracks(Vec::new()));
    }
    if lower.contains('[') {
        return grid_feature("a named grid line ([name])");
    }
    if matches!(lower.as_str(), "subgrid" | "masonry") || lower.starts_with("subgrid ") {
        return grid_feature(format!("`{lower}`"));
    }
    let mut out = Vec::new();
    for t in split_ws_top(s) {
        let tl = t.to_ascii_lowercase();
        let Some(inner) = tl.strip_prefix("repeat(").and_then(|r| r.strip_suffix(')')) else {
            out.push(Track::Size(parse_track_size(def, t)?));
            continue;
        };
        if !template {
            return err(
                codes::OUTSIDE,
                format!("{t:?}: repeat() is not allowed here"),
            );
        }
        let args = split_top(inner, ',');
        let [count, sizes] = args.as_slice() else {
            return err(
                codes::OUTSIDE,
                format!("{t:?}: repeat() takes a count and tracks"),
            );
        };
        if matches!(*count, "auto-fill" | "auto-fit") {
            return grid_feature(format!("repeat({count}, ...)"));
        }
        let d = parse_dim(count)?;
        if !d.unit.is_empty() || d.v.fract() != 0.0 {
            return err(
                codes::OUTSIDE,
                format!("{count:?}: a repeat count is an integer"),
            );
        }
        if !(1.0..=f64::from(u16::MAX)).contains(&d.v) {
            return err(
                codes::RANGE,
                format!("{count:?}: a repeat count is 1 or more"),
            );
        }
        let list = split_ws_top(sizes)
            .into_iter()
            .map(|x| parse_track_size(def, x))
            .collect::<Result<Vec<_>, _>>()?;
        if list.is_empty() {
            return err(codes::OUTSIDE, format!("{t:?}: repeat() without tracks"));
        }
        out.push(Track::Repeat(d.v as u16, list));
    }
    if out.is_empty() {
        return err(codes::OUTSIDE, "an empty track list");
    }
    Ok(Value::Tracks(out))
}

/// `auto`, a line number or `span N`; named lines are outside the profile.
fn parse_grid_line(s: &str) -> Result<Value, ValueError> {
    if s == "auto" {
        return Ok(Value::GridLine(GridLine::Auto));
    }
    let int = |t: &str| -> Option<f64> {
        parse_dim(t)
            .ok()
            .filter(|d| d.unit.is_empty() && d.v.fract() == 0.0)
            .map(|d| d.v)
    };
    match split_ws_top(s).as_slice() {
        ["span", n] => match int(n) {
            Some(v) if (1.0..=f64::from(u16::MAX)).contains(&v) => {
                Ok(Value::GridLine(GridLine::Span(v as u16)))
            }
            Some(_) => err(codes::RANGE, format!("{s:?}: span is 1 or more")),
            None => grid_feature(format!("the named span {s:?}")),
        },
        [n] => match int(n) {
            Some(v) if v != 0.0 && v.abs() <= f64::from(i16::MAX) => {
                Ok(Value::GridLine(GridLine::Line(v as i16)))
            }
            Some(_) => err(codes::RANGE, format!("{s:?}: a line number is not 0")),
            None => grid_feature(format!("the named line {s:?}")),
        },
        _ => grid_feature(format!("the grid placement {s:?}")),
    }
}

// ---------------------------------------------------------------------------------------------
// Border image

/// CSS's one-to-four expansion: top, right, bottom, left.
fn four<T: Copy>(v: &[T]) -> Option<[T; 4]> {
    match *v {
        [a] => Some([a, a, a, a]),
        [a, b] => Some([a, b, a, b]),
        [a, b, c] => Some([a, b, c, b]),
        [a, b, c, d] => Some([a, b, c, d]),
        _ => None,
    }
}

fn parse_slice(s: &str) -> Result<Value, ValueError> {
    let mut fill = false;
    let mut edges = Vec::new();
    for t in split_ws_top(s) {
        if t.eq_ignore_ascii_case("fill") {
            if fill {
                return err(codes::OUTSIDE, "`fill` twice");
            }
            fill = true;
            continue;
        }
        let d = parse_dim(t)?;
        if d.v < 0.0 {
            return err(codes::RANGE, format!("{t:?}: a slice is not negative"));
        }
        edges.push(match d.unit.as_str() {
            "" => Len::px(to_f32(d.v)?),
            "%" => Len::pct(to_f32(d.v)?),
            unit => {
                return err(
                    codes::OUTSIDE,
                    format!("a slice is a number of picture pixels or a percentage, not `{unit}`"),
                );
            }
        });
    }
    match four(&edges) {
        Some(edges) => Ok(Value::Slice(ImageSlice { edges, fill })),
        None => err(
            codes::OUTSIDE,
            format!("{s:?}: a slice is one to four values"),
        ),
    }
}

fn parse_image_widths(s: &str) -> Result<Value, ValueError> {
    let mut list = Vec::new();
    for t in split_ws_top(s) {
        if t == "auto" {
            return err(
                codes::OUTSIDE,
                "border-image-width `auto` is outside the profile; give px or a multiple of the border width",
            );
        }
        let d = parse_dim(t)?;
        if d.v < 0.0 {
            return err(codes::RANGE, format!("{t:?}: a width is not negative"));
        }
        list.push(match d.unit.as_str() {
            "" => ImageWidth::Factor(to_f32(d.v)?),
            "px" => ImageWidth::Px(to_f32(d.v)?),
            "%" => {
                return err(
                    codes::OUTSIDE,
                    "a border image width in % is outside the profile; give px or a number",
                );
            }
            unit => return bad_unit(unit),
        });
    }
    match four(&list) {
        Some(w) => Ok(Value::ImageWidths(w)),
        None => err(
            codes::OUTSIDE,
            format!("{s:?}: a width is one to four values"),
        ),
    }
}

fn parse_image_repeat(def: &PropDef, s: &str) -> Result<Value, ValueError> {
    let one = |t: &str| -> Result<ImageRepeat, ValueError> {
        keyword_of(def, t)
            .and_then(|k| ImageRepeat::from_keyword(&k))
            .ok_or_else(|| ValueError {
                code: codes::OUTSIDE,
                message: format!("`{t}` is not one of: {}", def.values.join(", ")),
            })
    };
    match split_ws_top(s).as_slice() {
        [a] => {
            let r = one(a)?;
            Ok(Value::ImageRepeats(ImageRepeats { x: r, y: r }))
        }
        [a, b] => Ok(Value::ImageRepeats(ImageRepeats {
            x: one(a)?,
            y: one(b)?,
        })),
        _ => err(
            codes::OUTSIDE,
            format!("{s:?}: a repeat is one or two keywords"),
        ),
    }
}

// ---------------------------------------------------------------------------------------------
// Clip path

fn parse_polygon(def: &PropDef, s: &str) -> Result<Value, ValueError> {
    let lower = s.to_ascii_lowercase();
    if lower == "none" {
        return Ok(Value::Polygon(Vec::new()));
    }
    let Some(inner) = lower
        .strip_prefix("polygon(")
        .and_then(|r| r.strip_suffix(')'))
    else {
        let shape = lower.split(['(', ' ']).next().unwrap_or("");
        return err(
            codes::CLIP,
            format!("the clip shape `{shape}` is outside the profile: use none or polygon()"),
        );
    };
    let mut args = split_top(inner, ',');
    if matches!(args.first(), Some(&"evenodd" | &"nonzero")) {
        // A convex polygon fills the same either way.
        args.remove(0);
    }
    if let Some(max) = def.max_items
        && args.len() > max
    {
        return err(
            codes::TOO_MANY,
            format!(
                "a clip polygon has at most {max} points, got {}",
                args.len()
            ),
        );
    }
    if args.len() < 3 {
        return err(codes::CLIP, "a clip polygon needs at least three points");
    }
    let mut points = Vec::with_capacity(args.len());
    for a in &args {
        let parts = split_ws_top(a);
        let [x, y] = parts.as_slice() else {
            return err(codes::OUTSIDE, format!("{a:?}: a point is two lengths"));
        };
        points.push(Point {
            x: parse_len(def, x)?,
            y: parse_len(def, y)?,
        });
    }
    if let Some(b) = CLIP_REFERENCE_BOXES
        .iter()
        .find(|b| !convex_at(&points, **b))
    {
        return err(
            codes::CONCAVE,
            format!(
                "the clip polygon is not convex (checked in a {}x{} px box)",
                b[0], b[1]
            ),
        );
    }
    Ok(Value::Polygon(points))
}

// ---------------------------------------------------------------------------------------------
// Canonical text (the form the test vectors compare)

fn len_text(l: Len) -> String {
    match (l.px == 0.0, l.pct == 0.0) {
        (_, true) => format!("{}px", l.px),
        (true, false) => format!("{}%", l.pct),
        (false, false) if l.px < 0.0 => format!("{}%-{}px", l.pct, -l.px),
        (false, false) => format!("{}%+{}px", l.pct, l.px),
    }
}

fn color_text(c: ColorV) -> String {
    match c {
        ColorV::Lit([r, g, b, a]) => format!("rgba({r},{g},{b},{a})"),
        ColorV::Tok(t) => format!("tok{}", t.0),
    }
}

impl Value {
    /// A fixed text form, the same on every machine: what the vectors compare.
    pub fn canon(&self) -> String {
        match self {
            Value::Keyword(k) => format!("kw:{k}"),
            Value::KeywordSet(v) => format!("set:{}", v.join(" ")),
            Value::Length(l) => format!("len:{}", len_text(*l)),
            Value::Number(n) => format!("num:{n}"),
            Value::Integer(n) => format!("int:{n}"),
            Value::Color(c) => color_text(*c),
            Value::Radius(r) => format!("radius:{} {}", len_text(r.h), len_text(r.v)),
            Value::Shadows(v) => {
                let items: Vec<String> = v
                    .iter()
                    .map(|s| {
                        format!(
                            "{}{} {} {} {} {}",
                            if s.inset { "inset " } else { "" },
                            s.x,
                            s.y,
                            s.blur,
                            s.spread,
                            color_text(s.color)
                        )
                    })
                    .collect();
                format!("shadows:{}", items.join("; "))
            }
            Value::Transform(v) => {
                let items: Vec<String> = v
                    .iter()
                    .map(|f| match f {
                        TFn::Translate(x, y) => {
                            format!("translate({},{})", len_text(*x), len_text(*y))
                        }
                        TFn::Rotate(d) => format!("rotate({d})"),
                        TFn::Scale(x, y) => format!("scale({x},{y})"),
                    })
                    .collect();
                format!("transform:{}", items.join(" "))
            }
            Value::Origin(o) => format!("origin:{} {}", len_text(o.x), len_text(o.y)),
            Value::Times(v) => {
                let items: Vec<String> = v.iter().map(|t| format!("{t}ms")).collect();
                format!("times:{}", items.join(","))
            }
            Value::Easings(v) => {
                let items: Vec<String> = v
                    .iter()
                    .map(|b| format!("[{} {} {} {}]", b[0], b[1], b[2], b[3]))
                    .collect();
                format!("easings:{}", items.join(","))
            }
            Value::Properties(v) => {
                let items: Vec<&str> = v
                    .iter()
                    .map(|p| match p {
                        PropSel::All => "all",
                        PropSel::Prop(id) => id.name(),
                    })
                    .collect();
                format!("props:{}", items.join(","))
            }
            Value::Families(v) => format!("families:{}", v.join("|")),
            Value::LineHeight(l) => match l {
                LineHeight::Normal => "lh:normal".to_string(),
                LineHeight::Px(p) => format!("lh:{p}px"),
                LineHeight::Factor(f) => format!("lh:{f}x"),
            },
            Value::Images(v) => {
                let items: Vec<String> = v
                    .iter()
                    .map(|layer| {
                        let g = match layer {
                            BgLayer::Gradient(g) => g,
                            BgLayer::Url(u) => return format!("url({u})"),
                        };
                        let dir = match g.dir {
                            GradDir::Angle(a) => format!("{a}deg"),
                            GradDir::To(side) => format!("to {}", side.keyword()),
                        };
                        let stops: Vec<String> = g
                            .stops
                            .iter()
                            .map(|st| match st.pos {
                                Some(p) => format!("{} {}", color_text(st.color), len_text(p)),
                                None => color_text(st.color),
                            })
                            .collect();
                        format!(
                            "{}{}({dir}: {})",
                            if g.repeating { "repeating-" } else { "" },
                            if g.conic { "conic" } else { "linear" },
                            stops.join(", ")
                        )
                    })
                    .collect();
                format!("images:{}", items.join("; "))
            }
            Value::BgSizes(v) => {
                let la = |l: LenAuto| match l {
                    LenAuto::Auto => "auto".to_string(),
                    LenAuto::Len(l) => len_text(l),
                };
                let items: Vec<String> = v
                    .iter()
                    .map(|s| match s {
                        BgSize::Cover => "cover".to_string(),
                        BgSize::Contain => "contain".to_string(),
                        BgSize::Size(w, h) => format!("{} {}", la(*w), la(*h)),
                    })
                    .collect();
                format!("sizes:{}", items.join(", "))
            }
            Value::Lengths(v) => {
                let items: Vec<String> = v.iter().map(|l| len_text(*l)).collect();
                format!("lens:{}", items.join(", "))
            }
            Value::Repeats(v) => {
                let items: Vec<String> = v
                    .iter()
                    .map(|r| format!("{} {}", r.x.keyword(), r.y.keyword()))
                    .collect();
                format!("repeats:{}", items.join(", "))
            }
            Value::Keywords(v) => format!("kws:{}", v.join(",")),
            Value::Ratio(AspectRatio::Auto) => "ratio:auto".to_string(),
            Value::Ratio(AspectRatio::Ratio(w, h)) => format!("ratio:{w}/{h}"),
            Value::Tracks(v) => {
                let items: Vec<String> = v
                    .iter()
                    .map(|t| match t {
                        Track::Size(s) => track_size_text(s),
                        Track::Repeat(n, list) => {
                            let l: Vec<String> = list.iter().map(track_size_text).collect();
                            format!("repeat({n}: {})", l.join(" "))
                        }
                    })
                    .collect();
                format!("tracks:{}", items.join(" "))
            }
            Value::GridLine(GridLine::Auto) => "line:auto".to_string(),
            Value::GridLine(GridLine::Line(n)) => format!("line:{n}"),
            Value::GridLine(GridLine::Span(n)) => format!("line:span {n}"),
            Value::Url(u) => format!("url:{u}"),
            Value::Slice(s) => {
                let e: Vec<String> = s
                    .edges
                    .iter()
                    .map(|l| {
                        if l.pct != 0.0 {
                            format!("{}%", l.pct)
                        } else {
                            format!("{}", l.px)
                        }
                    })
                    .collect();
                format!("slice:{}{}", e.join(" "), if s.fill { " fill" } else { "" })
            }
            Value::ImageWidths(w) => {
                let e: Vec<String> = w
                    .iter()
                    .map(|x| match x {
                        ImageWidth::Px(p) => format!("{p}px"),
                        ImageWidth::Factor(f) => format!("{f}x"),
                    })
                    .collect();
                format!("widths:{}", e.join(" "))
            }
            Value::ImageRepeats(r) => format!("irepeat:{} {}", r.x.keyword(), r.y.keyword()),
            Value::Polygon(v) => {
                let p: Vec<String> = v
                    .iter()
                    .map(|p| format!("{} {}", len_text(p.x), len_text(p.y)))
                    .collect();
                format!("polygon:{}", p.join(", "))
            }
        }
    }
}

fn breadth_text(b: &TrackBreadth) -> String {
    match b {
        TrackBreadth::Len(l) => len_text(*l),
        TrackBreadth::Fr(f) => format!("{f}fr"),
        TrackBreadth::Auto => "auto".into(),
        TrackBreadth::MinContent => "min-content".into(),
        TrackBreadth::MaxContent => "max-content".into(),
    }
}

fn track_size_text(s: &TrackSize) -> String {
    match s {
        TrackSize::Breadth(b) => breadth_text(b),
        TrackSize::MinMax(a, b) => format!("minmax({},{})", breadth_text(a), breadth_text(b)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_named_colors_are_sorted_and_complete() {
        assert!(NAMED.windows(2).all(|w| w[0].0 < w[1].0));
        assert_eq!(NAMED.len(), 148);
    }

    #[test]
    fn profile_split_helpers_respect_parentheses_and_quotes() {
        assert_eq!(
            split_top("a, rgb(1, 2, 3), b", ','),
            ["a", "rgb(1, 2, 3)", "b"]
        );
        assert_eq!(split_top("\"A, B\", c", ','), ["\"A, B\"", "c"]);
        assert_eq!(
            split_ws_top("0 1px rgb(1, 2, 3) inset"),
            ["0", "1px", "rgb(1, 2, 3)", "inset"]
        );
    }

    #[test]
    fn profile_dim_scanner_separates_exponent_and_unit() {
        let d = parse_dim("1em").expect("em");
        assert_eq!((d.v, d.unit.as_str()), (1.0, "em"));
        let d = parse_dim("1e3px").expect("exp");
        assert_eq!((d.v, d.unit.as_str()), (1000.0, "px"));
        let d = parse_dim("-.5%").expect("pct");
        assert_eq!((d.v, d.unit.as_str()), (-0.5, "%"));
        assert!(parse_dim("auto").is_err());
        assert_eq!(
            parse_dim("NaN").err().map(|e| e.code),
            Some(codes::NOT_FINITE)
        );
    }
}
