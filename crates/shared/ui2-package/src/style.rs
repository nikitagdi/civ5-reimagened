//! Typed property values: what a baked style stores for a property.
//!
//! These are the leaf types of `StyleRecord` and the results of the profile's value parsers. All
//! lengths are CSS px (the runtime scales them); percentages are in percent units (`50.0` is
//! 50 %). No float may be NaN or infinite: the writer and the reader check.

use serde::{Deserialize, Serialize};

use crate::ids::TokenId;
use crate::profile::PropId;

/// A length: `px` pixels plus `pct` percent of the reference size (`calc(100% - 4px)` is
/// `px = -4, pct = 100`).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Len {
    /// The pixel part.
    pub px: f32,
    /// The percent part, in percent units.
    pub pct: f32,
}

impl Len {
    /// Zero.
    pub const ZERO: Len = Len { px: 0.0, pct: 0.0 };

    /// `px` pixels.
    pub const fn px(px: f32) -> Len {
        Len { px, pct: 0.0 }
    }

    /// `pct` percent.
    pub const fn pct(pct: f32) -> Len {
        Len { px: 0.0, pct }
    }

    /// Both parts are finite.
    pub fn is_finite(&self) -> bool {
        self.px.is_finite() && self.pct.is_finite()
    }
}

/// `auto` or a length.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum LenAuto {
    /// `auto`.
    Auto,
    /// A length.
    Len(Len),
}

/// `none` or a length (`max-width`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum LenNone {
    /// `none`.
    None,
    /// A length.
    Len(Len),
}

/// `normal` or a length (`row-gap`, `letter-spacing`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum LenNormal {
    /// `normal`.
    Normal,
    /// A length.
    Len(Len),
}

/// `auto` or an integer (`z-index`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IntAuto {
    /// `auto`.
    Auto,
    /// An integer.
    Int(i32),
}

/// A color: a literal RGBA, or a reference to a root color token that a theme can change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorV {
    /// Red, green, blue, alpha.
    Lit([u8; 4]),
    /// A root token of `TOKN`.
    Tok(TokenId),
}

/// A color or `none` (`fill`, `stroke`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Paint {
    /// No paint.
    None,
    /// A color.
    Color(ColorV),
}

/// A corner radius: horizontal and vertical (an ellipse when they differ).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Radius {
    /// Horizontal radius.
    pub h: Len,
    /// Vertical radius.
    pub v: Len,
}

/// One shadow of `box-shadow` or `text-shadow`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Shadow {
    /// An inner shadow (`box-shadow` only).
    pub inset: bool,
    /// Horizontal offset, px.
    pub x: f32,
    /// Vertical offset, px.
    pub y: f32,
    /// Blur radius, px, not negative.
    pub blur: f32,
    /// Spread, px.
    pub spread: f32,
    /// The color.
    pub color: ColorV,
}

/// One function of a `transform` list.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum TFn {
    /// `translate`, `translateX`, `translateY` (the missing axis is zero).
    Translate(Len, Len),
    /// `rotate`, degrees.
    Rotate(f32),
    /// `scale`, `scaleX`, `scaleY` (the missing axis is one).
    Scale(f32, f32),
}

/// A transform origin.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Origin {
    /// Horizontal.
    pub x: Len,
    /// Vertical.
    pub y: Len,
}

/// `line-height`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum LineHeight {
    /// `normal`.
    Normal,
    /// A length in px.
    Px(f32),
    /// A multiple of the font size.
    Factor(f32),
}

/// One property of a `transition-property` list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PropSel {
    /// `all`: every animatable property.
    All,
    /// One property.
    Prop(PropId),
}

/// One baked transition: a property, its duration and delay in ms, and the timing function as a
/// cubic Bezier (`linear` is `[0, 0, 1, 1]`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Transition {
    /// The property.
    pub prop: PropId,
    /// Duration, ms.
    pub dur_ms: f32,
    /// Delay, ms.
    pub delay_ms: f32,
    /// `[x1, y1, x2, y2]`.
    pub ease: [f32; 4],
}
