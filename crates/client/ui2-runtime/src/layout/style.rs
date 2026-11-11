//! Style records to taffy styles, once per record when the skin loads.
//!
//! Records hold Chrome's computed values in CSS px, so the mapping is one to one and does not depend on the
//! viewport or the UI scale. Not mapped, by design of the profile: `order` (the converter sorts children),
//! `display: contents` (the converter flattens), `calc()` in layout properties (rejected by the converter;
//! a length with both a px and a % part keeps the px part here).
// The record has no grid template fields yet; grid containers lay out with implicit tracks only.

use taffy::{
    AlignContent, AlignItems, BoxSizing as TBoxSizing, Dimension, Display as TDisplay,
    FlexDirection as TFlexDirection, FlexWrap as TFlexWrap, LengthPercentage, LengthPercentageAuto,
    Overflow as TOverflow, Point, Position as TPosition, Rect, Size, Style,
};
use ui2_package::style::{
    BoxSizing, ContentAlign, Display, FlexDirection, FlexWrap, ItemsAlign, Len, LenAuto, LenNone,
    LenNormal, Overflow, Position, SelfAlign, StyleRecord,
};

fn lp(l: Len) -> LengthPercentage {
    if l.pct != 0.0 && l.px == 0.0 {
        LengthPercentage::percent(l.pct / 100.0)
    } else {
        LengthPercentage::length(l.px)
    }
}

fn lpa(l: LenAuto) -> LengthPercentageAuto {
    match l {
        LenAuto::Auto => LengthPercentageAuto::auto(),
        LenAuto::Len(l) if l.pct != 0.0 && l.px == 0.0 => {
            LengthPercentageAuto::percent(l.pct / 100.0)
        }
        LenAuto::Len(l) => LengthPercentageAuto::length(l.px),
    }
}

fn dim(l: LenAuto) -> Dimension {
    match l {
        LenAuto::Auto => Dimension::auto(),
        LenAuto::Len(l) if l.pct != 0.0 && l.px == 0.0 => Dimension::percent(l.pct / 100.0),
        LenAuto::Len(l) => Dimension::length(l.px),
    }
}

fn max(l: LenNone) -> LengthPercentageAuto {
    match l {
        LenNone::None => LengthPercentageAuto::auto(),
        LenNone::Len(l) => lpa(LenAuto::Len(l)),
    }
}

fn gap(l: LenNormal) -> LengthPercentage {
    match l {
        LenNormal::Normal => LengthPercentage::length(0.0),
        LenNormal::Len(l) => lp(l),
    }
}

fn overflow(o: Overflow) -> TOverflow {
    match o {
        Overflow::Visible => TOverflow::Visible,
        Overflow::Clip => TOverflow::Clip,
        Overflow::Hidden => TOverflow::Hidden,
        Overflow::Auto | Overflow::Scroll => TOverflow::Scroll,
    }
}

fn content(a: ContentAlign) -> Option<AlignContent> {
    Some(match a {
        ContentAlign::Normal => return None,
        ContentAlign::FlexStart => AlignContent::FLEX_START,
        ContentAlign::FlexEnd => AlignContent::FLEX_END,
        ContentAlign::Start => AlignContent::START,
        ContentAlign::End => AlignContent::END,
        ContentAlign::Center => AlignContent::CENTER,
        ContentAlign::SpaceBetween => AlignContent::SPACE_BETWEEN,
        ContentAlign::SpaceAround => AlignContent::SPACE_AROUND,
        ContentAlign::SpaceEvenly => AlignContent::SPACE_EVENLY,
    })
}

fn items(a: ItemsAlign) -> AlignItems {
    match a {
        ItemsAlign::Normal => AlignItems::NORMAL,
        ItemsAlign::Stretch => AlignItems::STRETCH,
        ItemsAlign::FlexStart => AlignItems::FLEX_START,
        ItemsAlign::FlexEnd => AlignItems::FLEX_END,
        ItemsAlign::Start => AlignItems::START,
        ItemsAlign::End => AlignItems::END,
        ItemsAlign::Center => AlignItems::CENTER,
        ItemsAlign::Baseline => AlignItems::BASELINE,
    }
}

fn self_align(a: SelfAlign) -> Option<AlignItems> {
    Some(match a {
        SelfAlign::Auto => return None,
        SelfAlign::Normal => AlignItems::NORMAL,
        SelfAlign::Stretch => AlignItems::STRETCH,
        SelfAlign::FlexStart => AlignItems::FLEX_START,
        SelfAlign::FlexEnd => AlignItems::FLEX_END,
        SelfAlign::Start => AlignItems::START,
        SelfAlign::End => AlignItems::END,
        SelfAlign::Center => AlignItems::CENTER,
        SelfAlign::Baseline => AlignItems::BASELINE,
    })
}

/// The taffy style of a record.
pub fn taffy_style(r: &StyleRecord) -> Style {
    let display = match r.display {
        Display::None => TDisplay::None,
        Display::Flex => TDisplay::Flex,
        Display::Grid => TDisplay::Grid,
        // `inline` outside a text leaf is not in the profile; such a box lays out as a block.
        Display::Block | Display::Inline => TDisplay::Block,
    };
    let position = match r.position {
        Position::Static => TPosition::Static,
        Position::Relative => TPosition::Relative,
        Position::Absolute => TPosition::Absolute,
    };
    let inset = if r.position == Position::Static {
        Rect {
            left: LengthPercentageAuto::auto(),
            right: LengthPercentageAuto::auto(),
            top: LengthPercentageAuto::auto(),
            bottom: LengthPercentageAuto::auto(),
        }
    } else {
        Rect {
            left: lpa(r.left),
            right: lpa(r.right),
            top: lpa(r.top),
            bottom: lpa(r.bottom),
        }
    };
    Style {
        display,
        box_sizing: match r.box_sizing {
            BoxSizing::BorderBox => TBoxSizing::BorderBox,
            BoxSizing::ContentBox => TBoxSizing::ContentBox,
        },
        overflow: Point {
            x: overflow(r.overflow_x),
            y: overflow(r.overflow_y),
        },
        scrollbar_width: 0.0,
        position,
        inset,
        size: Size {
            width: dim(r.width),
            height: dim(r.height),
        },
        min_size: Size {
            width: lpa(r.min_width),
            height: lpa(r.min_height),
        },
        max_size: Size {
            width: max(r.max_width),
            height: max(r.max_height),
        },
        margin: Rect {
            left: lpa(r.margin_left),
            right: lpa(r.margin_right),
            top: lpa(r.margin_top),
            bottom: lpa(r.margin_bottom),
        },
        padding: Rect {
            left: lp(r.padding_left),
            right: lp(r.padding_right),
            top: lp(r.padding_top),
            bottom: lp(r.padding_bottom),
        },
        border: Rect {
            left: lp(r.border_left_width),
            right: lp(r.border_right_width),
            top: lp(r.border_top_width),
            bottom: lp(r.border_bottom_width),
        },
        align_items: items(r.align_items),
        align_self: self_align(r.align_self),
        align_content: content(r.align_content).unwrap_or(AlignContent::NORMAL),
        justify_content: content(r.justify_content).unwrap_or(AlignContent::NORMAL),
        gap: Size {
            width: gap(r.column_gap),
            height: gap(r.row_gap),
        },
        flex_direction: match r.flex_direction {
            FlexDirection::Row => TFlexDirection::Row,
            FlexDirection::RowReverse => TFlexDirection::RowReverse,
            FlexDirection::Column => TFlexDirection::Column,
            FlexDirection::ColumnReverse => TFlexDirection::ColumnReverse,
        },
        flex_wrap: match r.flex_wrap {
            FlexWrap::Nowrap => TFlexWrap::NoWrap,
            FlexWrap::Wrap => TFlexWrap::Wrap,
        },
        flex_basis: dim(r.flex_basis),
        flex_grow: r.flex_grow,
        flex_shrink: r.flex_shrink,
        ..Style::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_keep_px_and_percent() {
        let mut r = StyleRecord::initial();
        r.display = Display::Flex;
        r.position = Position::Static;
        r.left = LenAuto::Len(Len::px(5.0));
        r.width = LenAuto::Len(Len::pct(62.0));
        r.flex_basis = LenAuto::Len(Len::pct(0.0));
        r.column_gap = LenNormal::Len(Len::px(4.0));
        let s = taffy_style(&r);
        assert_eq!(s.display, TDisplay::Flex);
        assert!(s.inset.left.is_auto(), "a static box ignores its insets");
        assert_eq!(s.size.width, Dimension::percent(0.62));
        assert_eq!(s.flex_basis, Dimension::length(0.0));
        assert_eq!(s.gap.width, LengthPercentage::length(4.0));
        assert_eq!(s.align_self, None);
    }
}
