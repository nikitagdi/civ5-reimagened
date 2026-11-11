//! Typed property values: what a baked style stores for a property.
//!
//! These are the leaf types of `StyleRecord` and the results of the profile's value parsers. All
//! lengths are CSS px (the runtime scales them); percentages are in percent units (`50.0` is
//! 50 %). No float may be NaN or infinite: the writer and the reader check.

use serde::{Deserialize, Serialize};

use crate::ids::{StrId, TokenId};
use crate::profile::PropId;
#[cfg(feature = "profile")]
use crate::profile::apply::FromValue;
#[cfg(feature = "profile")]
use crate::profile::values::Value;

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

/// One layer of `background-image`. Only gradients for now; picture layers (`url()`) are refused by
/// the profile parser until pictures can be baked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BgImage {
    /// A linear or conic gradient.
    Gradient(Gradient),
}

/// A `linear-gradient`, `conic-gradient` or one of their `repeating-` forms.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Gradient {
    /// A conic gradient (centered; `dir` is its `from` angle) instead of a linear one.
    pub conic: bool,
    /// The `repeating-` form.
    pub repeating: bool,
    /// The direction of a linear gradient, or the start angle of a conic one.
    pub dir: GradDir,
    /// The color stops, 2 to 4.
    pub stops: Vec<Stop>,
}

/// The direction of a gradient.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum GradDir {
    /// Degrees. Linear: the CSS gradient angle (0 points to the top, 90 to the right). Conic: the
    /// `from` angle, clockwise from the top.
    Angle(f32),
    /// `to <side or corner>` (linear only); a corner angle depends on the box size.
    To(Side),
}

/// One color stop. A conic stop position given in degrees is stored as percent of a full turn
/// (`deg / 3.6`); `None` means the position is spread evenly as in CSS.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Stop {
    /// The color.
    pub color: ColorV,
    /// The position along the gradient line.
    pub pos: Option<Len>,
}

/// One layer of `background-size`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum BgSize {
    /// `cover`.
    Cover,
    /// `contain`.
    Contain,
    /// Width and height; `auto` is `Size(Auto, Auto)`.
    Size(LenAuto, LenAuto),
}

/// One layer of `background-repeat`, per axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BgRepeat {
    /// Horizontal.
    pub x: Repeat,
    /// Vertical.
    pub y: Repeat,
}

impl BgRepeat {
    /// `repeat` on both axes.
    pub const REPEAT: BgRepeat = BgRepeat {
        x: Repeat::Repeat,
        y: Repeat::Repeat,
    };
}

// ---------------------------------------------------------------------------------------------
// Keyword enums

macro_rules! keyword_enum {
    ($(#[$doc:meta])* $name:ident { $($var:ident => $kw:literal,)* }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $name {
            $(#[doc = $kw] $var,)*
        }

        impl $name {
            /// The variant of a CSS keyword.
            pub fn from_keyword(keyword: &str) -> Option<Self> {
                match keyword {
                    $($kw => Some(Self::$var),)*
                    _ => None,
                }
            }

            /// The CSS keyword.
            pub const fn keyword(self) -> &'static str {
                match self {
                    $(Self::$var => $kw,)*
                }
            }
        }

        impl Finite for $name {
            fn finite(&self) -> bool {
                true
            }
        }

        #[cfg(feature = "profile")]
        impl FromValue for $name {
            fn from_value(v: &Value, _strs: &mut dyn FnMut(&str) -> StrId) -> Option<Self> {
                match v {
                    Value::Keyword(k) => Self::from_keyword(k),
                    _ => None,
                }
            }
        }
    };
}

keyword_enum! {
    /// `display`.
    Display { None => "none", Block => "block", Flex => "flex", Grid => "grid", Inline => "inline", }
}
keyword_enum! {
    /// `position`.
    Position { Static => "static", Relative => "relative", Absolute => "absolute", }
}
keyword_enum! {
    /// `box-sizing`.
    BoxSizing { BorderBox => "border-box", ContentBox => "content-box", }
}
keyword_enum! {
    /// `overflow-x` and `overflow-y`.
    Overflow { Visible => "visible", Hidden => "hidden", Clip => "clip", Auto => "auto", Scroll => "scroll", }
}
keyword_enum! {
    /// `flex-direction`.
    FlexDirection { Row => "row", RowReverse => "row-reverse", Column => "column", ColumnReverse => "column-reverse", }
}
keyword_enum! {
    /// `flex-wrap`.
    FlexWrap { Nowrap => "nowrap", Wrap => "wrap", }
}
keyword_enum! {
    /// `justify-content` and `align-content`.
    ContentAlign {
        Normal => "normal", FlexStart => "flex-start", FlexEnd => "flex-end", Start => "start",
        End => "end", Center => "center", SpaceBetween => "space-between",
        SpaceAround => "space-around", SpaceEvenly => "space-evenly",
    }
}
keyword_enum! {
    /// `align-items`.
    ItemsAlign {
        Normal => "normal", Stretch => "stretch", FlexStart => "flex-start", FlexEnd => "flex-end",
        Start => "start", End => "end", Center => "center", Baseline => "baseline",
    }
}
keyword_enum! {
    /// `align-self`.
    SelfAlign {
        Auto => "auto", Normal => "normal", Stretch => "stretch", FlexStart => "flex-start",
        FlexEnd => "flex-end", Start => "start", End => "end", Center => "center",
        Baseline => "baseline",
    }
}
keyword_enum! {
    /// `visibility`.
    Visibility { Visible => "visible", Hidden => "hidden", }
}
keyword_enum! {
    /// `border-*-style`.
    BorderStyle { None => "none", Solid => "solid", }
}
keyword_enum! {
    /// `cursor`.
    Cursor {
        Auto => "auto", Default => "default", Pointer => "pointer", Text => "text", Move => "move",
        Grab => "grab", Grabbing => "grabbing", NotAllowed => "not-allowed",
    }
}
keyword_enum! {
    /// `pointer-events`.
    PointerEvents { Auto => "auto", None => "none", }
}
keyword_enum! {
    /// `font-style`.
    FontStyle { Normal => "normal", Italic => "italic", }
}
keyword_enum! {
    /// `text-align`.
    TextAlign { Start => "start", End => "end", Left => "left", Right => "right", Center => "center", }
}
keyword_enum! {
    /// `white-space-collapse`.
    WhiteSpaceCollapse { Collapse => "collapse", Preserve => "preserve", }
}
keyword_enum! {
    /// `text-wrap-mode`.
    TextWrapMode { Wrap => "wrap", Nowrap => "nowrap", }
}
keyword_enum! {
    /// `text-overflow`.
    TextOverflow { Clip => "clip", Ellipsis => "ellipsis", }
}
keyword_enum! {
    /// `overflow-wrap`.
    OverflowWrap { Normal => "normal", BreakWord => "break-word", Anywhere => "anywhere", }
}
keyword_enum! {
    /// `word-break`.
    WordBreak { Normal => "normal", BreakWord => "break-word", }
}
keyword_enum! {
    /// `text-transform`.
    TextTransform { None => "none", }
}
keyword_enum! {
    /// `stroke-linecap`.
    StrokeLinecap { Butt => "butt", Round => "round", Square => "square", }
}
keyword_enum! {
    /// `stroke-linejoin`.
    StrokeLinejoin { Miter => "miter", Round => "round", Bevel => "bevel", }
}
keyword_enum! {
    /// The side or corner of a `to ...` gradient direction (a corner is two words in any order).
    Side {
        Top => "top", Right => "right", Bottom => "bottom", Left => "left",
        TopLeft => "top left", TopRight => "top right", BottomLeft => "bottom left",
        BottomRight => "bottom right",
    }
}
keyword_enum! {
    /// One axis of `background-repeat`.
    Repeat { Repeat => "repeat", NoRepeat => "no-repeat", }
}
keyword_enum! {
    /// `background-origin` and `background-clip`.
    BgBox { BorderBox => "border-box", PaddingBox => "padding-box", ContentBox => "content-box", }
}
keyword_enum! {
    /// `outline-style`.
    OutlineStyle { None => "none", Solid => "solid", }
}

/// `font-variant-numeric`: the figure features the shaper turns on (`tnum`, `pnum`, `lnum`,
/// `onum`). They are never frozen into the font.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NumericFeatures {
    /// `tabular-nums`.
    pub tabular: bool,
    /// `proportional-nums`.
    pub proportional: bool,
    /// `lining-nums`.
    pub lining: bool,
    /// `oldstyle-nums`.
    pub oldstyle: bool,
}

/// `text-decoration-line`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TextDecoration {
    /// `underline`.
    pub underline: bool,
}

// ---------------------------------------------------------------------------------------------
// Finite check and conversion from parsed values

/// Whether a value holds only finite floats; a package with a NaN is refused.
pub trait Finite {
    /// All floats inside are finite.
    fn finite(&self) -> bool;
}

impl Finite for f32 {
    fn finite(&self) -> bool {
        self.is_finite()
    }
}
impl Finite for Len {
    fn finite(&self) -> bool {
        self.is_finite()
    }
}
impl Finite for LenAuto {
    fn finite(&self) -> bool {
        matches!(self, LenAuto::Len(l) if l.is_finite()) || matches!(self, LenAuto::Auto)
    }
}
impl Finite for LenNone {
    fn finite(&self) -> bool {
        matches!(self, LenNone::Len(l) if l.is_finite()) || matches!(self, LenNone::None)
    }
}
impl Finite for LenNormal {
    fn finite(&self) -> bool {
        matches!(self, LenNormal::Len(l) if l.is_finite()) || matches!(self, LenNormal::Normal)
    }
}
impl Finite for Radius {
    fn finite(&self) -> bool {
        self.h.is_finite() && self.v.is_finite()
    }
}
impl Finite for Origin {
    fn finite(&self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}
impl Finite for Shadow {
    fn finite(&self) -> bool {
        [self.x, self.y, self.blur, self.spread]
            .iter()
            .all(|v| v.is_finite())
    }
}
impl Finite for TFn {
    fn finite(&self) -> bool {
        match self {
            TFn::Translate(x, y) => x.is_finite() && y.is_finite(),
            TFn::Rotate(d) => d.is_finite(),
            TFn::Scale(x, y) => x.is_finite() && y.is_finite(),
        }
    }
}
impl Finite for LineHeight {
    fn finite(&self) -> bool {
        match self {
            LineHeight::Normal => true,
            LineHeight::Px(v) | LineHeight::Factor(v) => v.is_finite(),
        }
    }
}
impl Finite for Transition {
    fn finite(&self) -> bool {
        self.dur_ms.is_finite()
            && self.delay_ms.is_finite()
            && self.ease.iter().all(|v| v.is_finite())
    }
}
impl Finite for GradDir {
    fn finite(&self) -> bool {
        match self {
            GradDir::Angle(a) => a.is_finite(),
            GradDir::To(_) => true,
        }
    }
}
impl Finite for Stop {
    fn finite(&self) -> bool {
        self.pos.is_none_or(|p| p.is_finite())
    }
}
impl Finite for Gradient {
    fn finite(&self) -> bool {
        self.dir.finite() && self.stops.finite()
    }
}
impl Finite for BgImage {
    fn finite(&self) -> bool {
        match self {
            BgImage::Gradient(g) => g.finite(),
        }
    }
}
impl Finite for BgSize {
    fn finite(&self) -> bool {
        match self {
            BgSize::Cover | BgSize::Contain => true,
            BgSize::Size(w, h) => w.finite() && h.finite(),
        }
    }
}
impl<T: Finite> Finite for Vec<T> {
    fn finite(&self) -> bool {
        self.iter().all(Finite::finite)
    }
}

macro_rules! always_finite {
    ($($t:ty),*) => {$(
        impl Finite for $t {
            fn finite(&self) -> bool {
                true
            }
        }
    )*};
}
always_finite!(
    i32,
    u16,
    bool,
    ColorV,
    IntAuto,
    Paint,
    StrId,
    NumericFeatures,
    TextDecoration,
    BgRepeat
);

// ---------------------------------------------------------------------------------------------
// The record

const BLACK: ColorV = ColorV::Lit([0, 0, 0, 255]);
const CLEAR: ColorV = ColorV::Lit([0, 0, 0, 0]);
const ZERO: LenAuto = LenAuto::Len(Len::ZERO);

macro_rules! style_record {
    ($($field:ident : $ty:ty = $prop:ident, $default:expr;)*) => {
        /// A node's computed style as Chrome baked it: one field per property of the profile
        /// (the transition properties share `transitions`). Every field holds a value, so a
        /// record is complete; variants of a node are other records (`StyleSet`).
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        pub struct StyleRecord {
            $(
                #[doc = concat!("`", stringify!($prop), "`.")]
                pub $field: $ty,
            )*
            /// The baked transitions, from the four `transition-*` properties.
            pub transitions: Vec<Transition>,
        }

        impl StyleRecord {
            /// Which properties each field holds. A test keeps it equal to `PropId::ALL` and to
            /// the rows of `profile.toml`.
            pub const FIELDS: &'static [(&'static str, &'static [PropId])] = &[
                $((stringify!($field), &[PropId::$prop]),)*
                ("transitions", &[
                    PropId::TransitionProperty,
                    PropId::TransitionDuration,
                    PropId::TransitionDelay,
                    PropId::TransitionTimingFunction,
                ]),
            ];

            /// The record of a node with every property at its initial value.
            pub fn initial() -> Self {
                Self {
                    $($field: $default,)*
                    transitions: Vec::new(),
                }
            }

            /// No float in the record is NaN or infinite.
            pub fn is_finite(&self) -> bool {
                true $(&& self.$field.finite())* && self.transitions.finite()
            }

            /// Sets the field of `prop` from a parsed value. Font families are interned through
            /// `strs`. The transition properties are not set here: use `compose_transitions`.
            #[cfg(feature = "profile")]
            pub fn set_value(
                &mut self,
                prop: PropId,
                value: &Value,
                strs: &mut dyn FnMut(&str) -> StrId,
            ) -> Result<(), StyleError> {
                match prop {
                    $(PropId::$prop => {
                        self.$field = <$ty as FromValue>::from_value(value, strs)
                            .ok_or(StyleError::Mismatch(prop))?;
                        Ok(())
                    })*
                    PropId::TransitionProperty
                    | PropId::TransitionDuration
                    | PropId::TransitionDelay
                    | PropId::TransitionTimingFunction => Err(StyleError::Transition(prop)),
                }
            }
        }
    };
}

style_record! {
    display: Display = Display, Display::Inline;
    position: Position = Position, Position::Static;
    z_index: IntAuto = ZIndex, IntAuto::Auto;
    box_sizing: BoxSizing = BoxSizing, BoxSizing::ContentBox;
    top: LenAuto = Top, LenAuto::Auto;
    right: LenAuto = Right, LenAuto::Auto;
    bottom: LenAuto = Bottom, LenAuto::Auto;
    left: LenAuto = Left, LenAuto::Auto;
    width: LenAuto = Width, LenAuto::Auto;
    height: LenAuto = Height, LenAuto::Auto;
    min_width: LenAuto = MinWidth, LenAuto::Auto;
    min_height: LenAuto = MinHeight, LenAuto::Auto;
    max_width: LenNone = MaxWidth, LenNone::None;
    max_height: LenNone = MaxHeight, LenNone::None;
    margin_top: LenAuto = MarginTop, ZERO;
    margin_right: LenAuto = MarginRight, ZERO;
    margin_bottom: LenAuto = MarginBottom, ZERO;
    margin_left: LenAuto = MarginLeft, ZERO;
    padding_top: Len = PaddingTop, Len::ZERO;
    padding_right: Len = PaddingRight, Len::ZERO;
    padding_bottom: Len = PaddingBottom, Len::ZERO;
    padding_left: Len = PaddingLeft, Len::ZERO;
    overflow_x: Overflow = OverflowX, Overflow::Visible;
    overflow_y: Overflow = OverflowY, Overflow::Visible;
    flex_direction: FlexDirection = FlexDirection, FlexDirection::Row;
    flex_wrap: FlexWrap = FlexWrap, FlexWrap::Nowrap;
    flex_grow: f32 = FlexGrow, 0.0;
    flex_shrink: f32 = FlexShrink, 1.0;
    flex_basis: LenAuto = FlexBasis, LenAuto::Auto;
    order: i32 = Order, 0;
    justify_content: ContentAlign = JustifyContent, ContentAlign::Normal;
    align_items: ItemsAlign = AlignItems, ItemsAlign::Normal;
    align_self: SelfAlign = AlignSelf, SelfAlign::Auto;
    align_content: ContentAlign = AlignContent, ContentAlign::Normal;
    row_gap: LenNormal = RowGap, LenNormal::Normal;
    column_gap: LenNormal = ColumnGap, LenNormal::Normal;
    color: ColorV = Color, BLACK;
    background_color: ColorV = BackgroundColor, CLEAR;
    opacity: f32 = Opacity, 1.0;
    visibility: Visibility = Visibility, Visibility::Visible;
    border_top_width: Len = BorderTopWidth, Len::ZERO;
    border_right_width: Len = BorderRightWidth, Len::ZERO;
    border_bottom_width: Len = BorderBottomWidth, Len::ZERO;
    border_left_width: Len = BorderLeftWidth, Len::ZERO;
    border_top_style: BorderStyle = BorderTopStyle, BorderStyle::None;
    border_right_style: BorderStyle = BorderRightStyle, BorderStyle::None;
    border_bottom_style: BorderStyle = BorderBottomStyle, BorderStyle::None;
    border_left_style: BorderStyle = BorderLeftStyle, BorderStyle::None;
    border_top_color: ColorV = BorderTopColor, BLACK;
    border_right_color: ColorV = BorderRightColor, BLACK;
    border_bottom_color: ColorV = BorderBottomColor, BLACK;
    border_left_color: ColorV = BorderLeftColor, BLACK;
    border_top_left_radius: Radius = BorderTopLeftRadius, Radius::default();
    border_top_right_radius: Radius = BorderTopRightRadius, Radius::default();
    border_bottom_right_radius: Radius = BorderBottomRightRadius, Radius::default();
    border_bottom_left_radius: Radius = BorderBottomLeftRadius, Radius::default();
    box_shadow: Vec<Shadow> = BoxShadow, Vec::new();
    text_shadow: Vec<Shadow> = TextShadow, Vec::new();
    transform: Vec<TFn> = Transform, Vec::new();
    transform_origin: Origin = TransformOrigin, Origin { x: Len::pct(50.0), y: Len::pct(50.0) };
    cursor: Cursor = Cursor, Cursor::Auto;
    pointer_events: PointerEvents = PointerEvents, PointerEvents::Auto;
    font_family: Vec<StrId> = FontFamily, Vec::new();
    font_size: Len = FontSize, Len::px(16.0);
    font_weight: u16 = FontWeight, 400;
    font_style: FontStyle = FontStyle, FontStyle::Normal;
    font_variant_numeric: NumericFeatures = FontVariantNumeric, NumericFeatures::default();
    line_height: LineHeight = LineHeight, LineHeight::Normal;
    letter_spacing: LenNormal = LetterSpacing, LenNormal::Normal;
    word_spacing: LenNormal = WordSpacing, LenNormal::Normal;
    text_align: TextAlign = TextAlign, TextAlign::Start;
    white_space_collapse: WhiteSpaceCollapse = WhiteSpaceCollapse, WhiteSpaceCollapse::Collapse;
    text_wrap_mode: TextWrapMode = TextWrapMode, TextWrapMode::Wrap;
    text_overflow: TextOverflow = TextOverflow, TextOverflow::Clip;
    overflow_wrap: OverflowWrap = OverflowWrap, OverflowWrap::Normal;
    word_break: WordBreak = WordBreak, WordBreak::Normal;
    text_transform: TextTransform = TextTransform, TextTransform::None;
    text_decoration_line: TextDecoration = TextDecorationLine, TextDecoration::default();
    fill: Paint = Fill, Paint::Color(BLACK);
    stroke: Paint = Stroke, Paint::None;
    stroke_width: Len = StrokeWidth, Len::px(1.0);
    stroke_linecap: StrokeLinecap = StrokeLinecap, StrokeLinecap::Butt;
    stroke_linejoin: StrokeLinejoin = StrokeLinejoin, StrokeLinejoin::Miter;
    background_image: Vec<BgImage> = BackgroundImage, Vec::new();
    background_size: Vec<BgSize> = BackgroundSize, vec![BgSize::Size(LenAuto::Auto, LenAuto::Auto)];
    background_position_x: Vec<Len> = BackgroundPositionX, vec![Len::ZERO];
    background_position_y: Vec<Len> = BackgroundPositionY, vec![Len::ZERO];
    background_repeat: Vec<BgRepeat> = BackgroundRepeat, vec![BgRepeat::REPEAT];
    background_origin: Vec<BgBox> = BackgroundOrigin, vec![BgBox::PaddingBox];
    background_clip: Vec<BgBox> = BackgroundClip, vec![BgBox::BorderBox];
    outline_style: OutlineStyle = OutlineStyle, OutlineStyle::None;
    // Chrome reports the `medium` width (3px) even while the style is `none`.
    outline_width: Len = OutlineWidth, Len::px(3.0);
    // The initial `currentcolor`, as Chrome resolves it for the initial `color`.
    outline_color: ColorV = OutlineColor, BLACK;
    outline_offset: Len = OutlineOffset, Len::ZERO;
}

impl Default for StyleRecord {
    fn default() -> Self {
        Self::initial()
    }
}

/// A parsed value could not be stored in the record.
#[cfg(feature = "profile")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StyleError {
    /// The value's kind does not fit the property's field.
    #[error("the value does not fit property {}", .0.name())]
    Mismatch(PropId),
    /// Transition properties are composed together, not set one by one.
    #[error("{} is a transition property: use compose_transitions", .0.name())]
    Transition(PropId),
}
