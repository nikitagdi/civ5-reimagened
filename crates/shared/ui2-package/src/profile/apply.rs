//! From parsed values to record fields, and the transition longhands composed into `Transition`s.

use super::ids::PropId;
use super::props::Profile;
use super::values::Value;
use crate::ids::StrId;
use crate::style::{
    BgBox, BgImage, BgRepeat, BgSize, ColorV, IntAuto, Len, LenAuto, LenNone, LenNormal,
    LineHeight, NumericFeatures, Origin, Paint, PropSel, Radius, Shadow, TFn, TextDecoration,
    Transition,
};

/// A field type that can take a parsed value.
pub trait FromValue: Sized {
    /// The field value, or `None` when the value's kind does not fit.
    fn from_value(value: &Value, strs: &mut dyn FnMut(&str) -> StrId) -> Option<Self>;
}

fn len_or_keyword<T>(value: &Value, keyword: &str, kw: T, wrap: impl Fn(Len) -> T) -> Option<T> {
    match value {
        Value::Keyword(k) if k == keyword => Some(kw),
        Value::Length(l) => Some(wrap(*l)),
        _ => None,
    }
}

impl FromValue for Len {
    fn from_value(value: &Value, _: &mut dyn FnMut(&str) -> StrId) -> Option<Self> {
        match value {
            Value::Length(l) => Some(*l),
            _ => None,
        }
    }
}

impl FromValue for LenAuto {
    fn from_value(value: &Value, _: &mut dyn FnMut(&str) -> StrId) -> Option<Self> {
        len_or_keyword(value, "auto", LenAuto::Auto, LenAuto::Len)
    }
}

impl FromValue for LenNone {
    fn from_value(value: &Value, _: &mut dyn FnMut(&str) -> StrId) -> Option<Self> {
        len_or_keyword(value, "none", LenNone::None, LenNone::Len)
    }
}

impl FromValue for LenNormal {
    fn from_value(value: &Value, _: &mut dyn FnMut(&str) -> StrId) -> Option<Self> {
        len_or_keyword(value, "normal", LenNormal::Normal, LenNormal::Len)
    }
}

impl FromValue for IntAuto {
    fn from_value(value: &Value, _: &mut dyn FnMut(&str) -> StrId) -> Option<Self> {
        match value {
            Value::Keyword(k) if k == "auto" => Some(IntAuto::Auto),
            Value::Integer(i) => Some(IntAuto::Int(*i)),
            _ => None,
        }
    }
}

impl FromValue for f32 {
    fn from_value(value: &Value, _: &mut dyn FnMut(&str) -> StrId) -> Option<Self> {
        match value {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }
}

impl FromValue for i32 {
    fn from_value(value: &Value, _: &mut dyn FnMut(&str) -> StrId) -> Option<Self> {
        match value {
            Value::Integer(n) => Some(*n),
            _ => None,
        }
    }
}

impl FromValue for u16 {
    fn from_value(value: &Value, _: &mut dyn FnMut(&str) -> StrId) -> Option<Self> {
        match value {
            Value::Integer(n) => u16::try_from(*n).ok(),
            _ => None,
        }
    }
}

impl FromValue for ColorV {
    fn from_value(value: &Value, _: &mut dyn FnMut(&str) -> StrId) -> Option<Self> {
        match value {
            Value::Color(c) => Some(*c),
            _ => None,
        }
    }
}

impl FromValue for Paint {
    fn from_value(value: &Value, _: &mut dyn FnMut(&str) -> StrId) -> Option<Self> {
        match value {
            Value::Keyword(k) if k == "none" => Some(Paint::None),
            Value::Color(c) => Some(Paint::Color(*c)),
            _ => None,
        }
    }
}

impl FromValue for Radius {
    fn from_value(value: &Value, _: &mut dyn FnMut(&str) -> StrId) -> Option<Self> {
        match value {
            Value::Radius(r) => Some(*r),
            _ => None,
        }
    }
}

impl FromValue for Origin {
    fn from_value(value: &Value, _: &mut dyn FnMut(&str) -> StrId) -> Option<Self> {
        match value {
            Value::Origin(o) => Some(*o),
            _ => None,
        }
    }
}

impl FromValue for LineHeight {
    fn from_value(value: &Value, _: &mut dyn FnMut(&str) -> StrId) -> Option<Self> {
        match value {
            Value::LineHeight(l) => Some(*l),
            _ => None,
        }
    }
}

impl FromValue for Vec<Shadow> {
    fn from_value(value: &Value, _: &mut dyn FnMut(&str) -> StrId) -> Option<Self> {
        match value {
            Value::Shadows(s) => Some(s.clone()),
            _ => None,
        }
    }
}

impl FromValue for Vec<TFn> {
    fn from_value(value: &Value, _: &mut dyn FnMut(&str) -> StrId) -> Option<Self> {
        match value {
            Value::Transform(t) => Some(t.clone()),
            _ => None,
        }
    }
}

impl FromValue for Vec<BgImage> {
    fn from_value(value: &Value, _: &mut dyn FnMut(&str) -> StrId) -> Option<Self> {
        match value {
            Value::Images(v) => Some(v.clone()),
            _ => None,
        }
    }
}

impl FromValue for Vec<BgSize> {
    fn from_value(value: &Value, _: &mut dyn FnMut(&str) -> StrId) -> Option<Self> {
        match value {
            Value::BgSizes(v) => Some(v.clone()),
            _ => None,
        }
    }
}

impl FromValue for Vec<Len> {
    fn from_value(value: &Value, _: &mut dyn FnMut(&str) -> StrId) -> Option<Self> {
        match value {
            Value::Lengths(v) => Some(v.clone()),
            _ => None,
        }
    }
}

impl FromValue for Vec<BgRepeat> {
    fn from_value(value: &Value, _: &mut dyn FnMut(&str) -> StrId) -> Option<Self> {
        match value {
            Value::Repeats(v) => Some(v.clone()),
            _ => None,
        }
    }
}

impl FromValue for Vec<BgBox> {
    fn from_value(value: &Value, _: &mut dyn FnMut(&str) -> StrId) -> Option<Self> {
        match value {
            Value::Keywords(v) => v.iter().map(|k| BgBox::from_keyword(k)).collect(),
            _ => None,
        }
    }
}

/// Font families become string-table ids, in order.
impl FromValue for Vec<StrId> {
    fn from_value(value: &Value, strs: &mut dyn FnMut(&str) -> StrId) -> Option<Self> {
        match value {
            Value::Families(names) => Some(names.iter().map(|n| strs(n)).collect()),
            _ => None,
        }
    }
}

impl FromValue for NumericFeatures {
    fn from_value(value: &Value, _: &mut dyn FnMut(&str) -> StrId) -> Option<Self> {
        let Value::KeywordSet(set) = value else {
            return None;
        };
        let has = |name: &str| set.iter().any(|k| k == name);
        Some(NumericFeatures {
            tabular: has("tabular-nums"),
            proportional: has("proportional-nums"),
            lining: has("lining-nums"),
            oldstyle: has("oldstyle-nums"),
        })
    }
}

impl FromValue for TextDecoration {
    fn from_value(value: &Value, _: &mut dyn FnMut(&str) -> StrId) -> Option<Self> {
        let Value::KeywordSet(set) = value else {
            return None;
        };
        Some(TextDecoration {
            underline: set.iter().any(|k| k == "underline"),
        })
    }
}

/// Composes the four `transition-*` longhands into the transitions of a record.
///
/// As in CSS, the property list decides the count and the other lists repeat cyclically; `all`
/// expands to every animatable property in `PropId` order; `none` gives no transitions. An empty
/// duration, delay or easing list falls back to the initial value of its property.
pub fn compose_transitions(
    profile: &Profile,
    properties: &[PropSel],
    durations: &[f32],
    delays: &[f32],
    easings: &[[f32; 4]],
) -> Vec<Transition> {
    fn nth<T: Copy>(list: &[T], i: usize, fallback: T) -> T {
        if list.is_empty() {
            fallback
        } else {
            list[i % list.len()]
        }
    }
    let mut out = Vec::new();
    // `all` covers several properties but is one item of the lists.
    for (item, sel) in properties.iter().enumerate() {
        let group: Vec<PropId> = match sel {
            PropSel::All => PropId::ALL
                .iter()
                .copied()
                .filter(|&p| profile.def(p).anim)
                .collect(),
            PropSel::Prop(p) => vec![*p],
        };
        for prop in group {
            out.push(Transition {
                prop,
                dur_ms: nth(durations, item, 0.0),
                delay_ms: nth(delays, item, 0.0),
                ease: nth(easings, item, [0.25, 0.1, 0.25, 1.0]),
            });
        }
    }
    out
}
