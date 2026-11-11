//! Style records to taffy styles, once per record when the skin loads.
//!
//! Records hold Chrome's computed values in CSS px, so the mapping is one to one and does not depend on the
//! viewport or the UI scale. Not mapped, by design of the profile: `order` (the converter sorts children),
//! `display: contents` (the converter flattens), `calc()` in layout properties (rejected by the converter;
//! a length with both a px and a % part keeps the px part here).
//!
//! Grid: the explicit tracks (`grid-template-*`, with `repeat(N, ...)`), the implicit ones
//! (`grid-auto-*`), the flow and the four placements map one to one; named lines and areas are not in
//! the profile. A single track breadth `b` is `minmax(b, b)` as in CSS, except `Nfr`, which is
//! `minmax(auto, Nfr)`; an `fr` minimum (not valid CSS) is read as `auto`.

use taffy::geometry::{Line, MinMax};
use taffy::style::{
    GridAutoFlow as TGridAutoFlow, GridPlacement, GridTemplateComponent, GridTemplateRepetition,
    MaxTrackSizingFunction, MinTrackSizingFunction, RepetitionCount, TrackSizingFunction,
};
use taffy::{
    AlignContent, AlignItems, BoxSizing as TBoxSizing, Dimension, Display as TDisplay,
    FlexDirection as TFlexDirection, FlexWrap as TFlexWrap, LengthPercentage, LengthPercentageAuto,
    Overflow as TOverflow, Point, Position as TPosition, Rect, Size, Style,
};
use ui2_package::style::{
    AspectRatio, BoxSizing, ContentAlign, Display, FlexDirection, FlexWrap, GridAutoFlow, GridLine,
    ItemsAlign, Len, LenAuto, LenNone, LenNormal, Overflow, Position, SelfAlign, StyleRecord,
    Track, TrackBreadth, TrackSize,
};

/// Whether a length is a pure percentage (taffy gets a percent), else its px part is used.
fn is_pct(l: Len) -> bool {
    l.pct != 0.0 && l.px == 0.0
}

fn track_min(b: TrackBreadth) -> MinTrackSizingFunction {
    match b {
        TrackBreadth::Len(l) if is_pct(l) => MinTrackSizingFunction::percent(l.pct / 100.0),
        TrackBreadth::Len(l) => MinTrackSizingFunction::length(l.px),
        TrackBreadth::Fr(_) | TrackBreadth::Auto => MinTrackSizingFunction::auto(),
        TrackBreadth::MinContent => MinTrackSizingFunction::min_content(),
        TrackBreadth::MaxContent => MinTrackSizingFunction::max_content(),
    }
}

fn track_max(b: TrackBreadth) -> MaxTrackSizingFunction {
    match b {
        TrackBreadth::Len(l) if is_pct(l) => MaxTrackSizingFunction::percent(l.pct / 100.0),
        TrackBreadth::Len(l) => MaxTrackSizingFunction::length(l.px),
        TrackBreadth::Fr(f) => MaxTrackSizingFunction::fr(f),
        TrackBreadth::Auto => MaxTrackSizingFunction::auto(),
        TrackBreadth::MinContent => MaxTrackSizingFunction::min_content(),
        TrackBreadth::MaxContent => MaxTrackSizingFunction::max_content(),
    }
}

fn track(t: TrackSize) -> TrackSizingFunction {
    match t {
        TrackSize::Breadth(b) => MinMax {
            min: track_min(b),
            max: track_max(b),
        },
        TrackSize::MinMax(a, b) => MinMax {
            min: track_min(a),
            max: track_max(b),
        },
    }
}

fn template(tracks: &[Track]) -> Vec<GridTemplateComponent<String>> {
    tracks
        .iter()
        .map(|t| match t {
            Track::Size(s) => GridTemplateComponent::Single(track(*s)),
            Track::Repeat(n, v) => GridTemplateComponent::Repeat(GridTemplateRepetition {
                count: RepetitionCount::Count(*n),
                tracks: v.iter().copied().map(track).collect(),
                line_names: Vec::new(),
            }),
        })
        .collect()
}

fn placement(g: GridLine) -> GridPlacement<String> {
    match g {
        GridLine::Auto => GridPlacement::Auto,
        GridLine::Line(i) => taffy::style_helpers::line(i),
        GridLine::Span(n) => GridPlacement::Span(n),
    }
}

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

/// `flex-basis`. A zero length is a percentage: `flex: 1` computes to `flex-basis: 0%`, which behaves as
/// `content` against a container of indefinite main size (a column without a height) and as zero
/// otherwise; taffy does the same with a percentage. The record keeps no unit for zero, so an explicit
/// `0px` is read the same way and keeps its content size in such a column, where Chrome collapses it.
fn flex_basis(l: LenAuto) -> Dimension {
    match l {
        LenAuto::Len(l) if l.px == 0.0 && l.pct == 0.0 => Dimension::percent(0.0),
        _ => dim(l),
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
        flex_basis: flex_basis(r.flex_basis),
        flex_grow: r.flex_grow,
        flex_shrink: r.flex_shrink,
        aspect_ratio: match r.aspect_ratio {
            AspectRatio::Auto => None,
            AspectRatio::Ratio(w, h) if w > 0.0 && h > 0.0 => Some(w / h),
            AspectRatio::Ratio(..) => None,
        },
        justify_items: items(r.justify_items),
        justify_self: self_align(r.justify_self),
        grid_template_columns: template(&r.grid_template_columns),
        grid_template_rows: template(&r.grid_template_rows),
        grid_auto_columns: r.grid_auto_columns.iter().copied().map(track).collect(),
        grid_auto_rows: r.grid_auto_rows.iter().copied().map(track).collect(),
        grid_auto_flow: match r.grid_auto_flow {
            GridAutoFlow::Row => TGridAutoFlow::Row,
            GridAutoFlow::Column => TGridAutoFlow::Column,
            GridAutoFlow::RowDense => TGridAutoFlow::RowDense,
            GridAutoFlow::ColumnDense => TGridAutoFlow::ColumnDense,
        },
        grid_row: Line {
            start: placement(r.grid_row_start),
            end: placement(r.grid_row_end),
        },
        grid_column: Line {
            start: placement(r.grid_column_start),
            end: placement(r.grid_column_end),
        },
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
        assert_eq!(
            s.flex_basis,
            Dimension::percent(0.0),
            "`flex: 1` has a 0% basis"
        );
        assert_eq!(s.gap.width, LengthPercentage::length(4.0));
        assert_eq!(s.align_self, None);
    }
}
