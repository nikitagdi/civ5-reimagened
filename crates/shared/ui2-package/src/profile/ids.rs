//! `PropId` and `Tag`: the properties and elements of the profile, as enums.
//!
//! They live outside feature `profile` because the package stores them (`Transition::prop`,
//! `NodeKind::Box { tag }`). Postcard writes the variant index, so **the lists are append-only**:
//! a new property or element goes at the end and no entry is ever moved or removed (a change to an
//! existing one is a new package major). `profile.toml` carries the same names and a test keeps
//! the two in step.

use serde::{Deserialize, Serialize};

macro_rules! named_enum {
    ($(#[$doc:meta])* $name:ident { $($(#[$vdoc:meta])* $var:ident => $text:literal,)* }) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub enum $name {
            $($(#[$vdoc])* $var,)*
        }

        impl $name {
            /// Every variant, in declaration (and wire) order.
            pub const ALL: &'static [$name] = &[$($name::$var,)*];

            /// The CSS or HTML name.
            pub const fn name(self) -> &'static str {
                match self {
                    $($name::$var => $text,)*
                }
            }

            /// The variant of a name, case-sensitive and lower case as Chrome writes it.
            pub fn from_name(name: &str) -> Option<$name> {
                match name {
                    $($text => Some($name::$var),)*
                    _ => None,
                }
            }

            /// The position in `ALL`.
            pub const fn index(self) -> usize {
                self as usize
            }
        }
    };
}

named_enum! {
    /// A CSS property of the profile (longhands only: Chrome bakes the shorthands).
    PropId {
        Display => "display",
        Position => "position",
        ZIndex => "z-index",
        BoxSizing => "box-sizing",
        Top => "top",
        Right => "right",
        Bottom => "bottom",
        Left => "left",
        Width => "width",
        Height => "height",
        MinWidth => "min-width",
        MinHeight => "min-height",
        MaxWidth => "max-width",
        MaxHeight => "max-height",
        MarginTop => "margin-top",
        MarginRight => "margin-right",
        MarginBottom => "margin-bottom",
        MarginLeft => "margin-left",
        PaddingTop => "padding-top",
        PaddingRight => "padding-right",
        PaddingBottom => "padding-bottom",
        PaddingLeft => "padding-left",
        OverflowX => "overflow-x",
        OverflowY => "overflow-y",
        FlexDirection => "flex-direction",
        FlexWrap => "flex-wrap",
        FlexGrow => "flex-grow",
        FlexShrink => "flex-shrink",
        FlexBasis => "flex-basis",
        Order => "order",
        JustifyContent => "justify-content",
        AlignItems => "align-items",
        AlignSelf => "align-self",
        AlignContent => "align-content",
        RowGap => "row-gap",
        ColumnGap => "column-gap",
        Color => "color",
        BackgroundColor => "background-color",
        Opacity => "opacity",
        Visibility => "visibility",
        BorderTopWidth => "border-top-width",
        BorderRightWidth => "border-right-width",
        BorderBottomWidth => "border-bottom-width",
        BorderLeftWidth => "border-left-width",
        BorderTopStyle => "border-top-style",
        BorderRightStyle => "border-right-style",
        BorderBottomStyle => "border-bottom-style",
        BorderLeftStyle => "border-left-style",
        BorderTopColor => "border-top-color",
        BorderRightColor => "border-right-color",
        BorderBottomColor => "border-bottom-color",
        BorderLeftColor => "border-left-color",
        BorderTopLeftRadius => "border-top-left-radius",
        BorderTopRightRadius => "border-top-right-radius",
        BorderBottomRightRadius => "border-bottom-right-radius",
        BorderBottomLeftRadius => "border-bottom-left-radius",
        BoxShadow => "box-shadow",
        TextShadow => "text-shadow",
        Transform => "transform",
        TransformOrigin => "transform-origin",
        Cursor => "cursor",
        PointerEvents => "pointer-events",
        TransitionProperty => "transition-property",
        TransitionDuration => "transition-duration",
        TransitionDelay => "transition-delay",
        TransitionTimingFunction => "transition-timing-function",
        FontFamily => "font-family",
        FontSize => "font-size",
        FontWeight => "font-weight",
        FontStyle => "font-style",
        FontVariantNumeric => "font-variant-numeric",
        LineHeight => "line-height",
        LetterSpacing => "letter-spacing",
        WordSpacing => "word-spacing",
        TextAlign => "text-align",
        WhiteSpaceCollapse => "white-space-collapse",
        TextWrapMode => "text-wrap-mode",
        TextOverflow => "text-overflow",
        OverflowWrap => "overflow-wrap",
        WordBreak => "word-break",
        TextTransform => "text-transform",
        TextDecorationLine => "text-decoration-line",
        Fill => "fill",
        Stroke => "stroke",
        StrokeWidth => "stroke-width",
        StrokeLinecap => "stroke-linecap",
        StrokeLinejoin => "stroke-linejoin",
        BackgroundImage => "background-image",
        BackgroundSize => "background-size",
        BackgroundPositionX => "background-position-x",
        BackgroundPositionY => "background-position-y",
        BackgroundRepeat => "background-repeat",
        BackgroundOrigin => "background-origin",
        BackgroundClip => "background-clip",
        OutlineStyle => "outline-style",
        OutlineWidth => "outline-width",
        OutlineColor => "outline-color",
        OutlineOffset => "outline-offset",
        JustifyItems => "justify-items",
        JustifySelf => "justify-self",
        AspectRatio => "aspect-ratio",
        ScrollbarWidth => "scrollbar-width",
        GridTemplateColumns => "grid-template-columns",
        GridTemplateRows => "grid-template-rows",
        GridAutoColumns => "grid-auto-columns",
        GridAutoRows => "grid-auto-rows",
        GridAutoFlow => "grid-auto-flow",
        GridRowStart => "grid-row-start",
        GridRowEnd => "grid-row-end",
        GridColumnStart => "grid-column-start",
        GridColumnEnd => "grid-column-end",
        BorderImageSource => "border-image-source",
        BorderImageSlice => "border-image-slice",
        BorderImageWidth => "border-image-width",
        BorderImageRepeat => "border-image-repeat",
        BorderImageOutset => "border-image-outset",
        ClipPath => "clip-path",
        TextIndent => "text-indent",
        TextWrapStyle => "text-wrap-style",
        VerticalAlign => "vertical-align",
        Direction => "direction",
        WritingMode => "writing-mode",
    }
}

named_enum! {
    /// An element of the profile (HTML section 2 of the profile spec).
    Tag {
        Html => "html",
        Head => "head",
        Body => "body",
        Meta => "meta",
        Title => "title",
        Style => "style",
        Template => "template",
        Link => "link",
        Script => "script",
        Div => "div",
        Section => "section",
        Header => "header",
        Footer => "footer",
        Aside => "aside",
        Nav => "nav",
        Main => "main",
        Article => "article",
        P => "p",
        H1 => "h1",
        H2 => "h2",
        H3 => "h3",
        H4 => "h4",
        H5 => "h5",
        H6 => "h6",
        Ul => "ul",
        Ol => "ol",
        Li => "li",
        Label => "label",
        Hr => "hr",
        Span => "span",
        B => "b",
        Strong => "strong",
        I => "i",
        Em => "em",
        Small => "small",
        Button => "button",
        Input => "input",
        Img => "img",
        Svg => "svg",
        Use => "use",
        Symbol => "symbol",
        Defs => "defs",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_are_unique() {
        for &p in PropId::ALL {
            assert_eq!(PropId::from_name(p.name()), Some(p));
            assert_eq!(PropId::ALL[p.index()], p);
        }
        for &t in Tag::ALL {
            assert_eq!(Tag::from_name(t.name()), Some(t));
        }
        let mut names: Vec<_> = PropId::ALL.iter().map(|p| p.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), PropId::ALL.len());
        assert_eq!(PropId::from_name("Color"), None);
    }
}
