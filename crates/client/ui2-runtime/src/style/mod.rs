//! Styles: the runtime only *selects* records; the converter resolved the cascade.
//!
//! A node's template names a `StyleSet`: up to four conditions (`:hover`, a class toggled by a binding, ... on
//! the node or on its k-th ancestor element) and a full table of records indexed by the viewport cell and the
//! condition mask. Selecting is `set.get(cell, mask)`: no matching, no cache, no record building.
//!
//! - [`select`]: condition masks, the reverse index from a subject node to the nodes whose conditions test it,
//!   state and class changes, the style pass.
//! - [`tokens`]: theme colors.
//! - [`Scale`]: UI scale and pixel density; layout runs in CSS px on the logical viewport.
//! - [`diff`]: which passes a record change needs (layout, text, paint).

pub mod select;
pub mod tokens;

use ui2_package::PropId;
use ui2_package::style::{BgSize, ColorV, LenAuto, StyleRecord, TFn};
use ui2_package::{StyleId, StyleSetId, Styles};

use crate::Env;
use crate::arena::{Dirty, HIDDEN_STYLE, Node, PLACEHOLDER_STYLE};

pub(crate) use select::attach;

/// A value a style binding (`data-bind-style-<prop>`) sets, already parsed for its property.
#[derive(Debug, Clone, PartialEq)]
pub enum OverValue {
    /// `auto` or a length (`width`, `top`, ...).
    Len(LenAuto),
    /// A number (`opacity`, `flex-grow`).
    Num(f32),
    /// A color (`color`, `background-color`).
    Color(ColorV),
    /// A transform list (`transform`).
    Transform(Vec<TFn>),
    /// Background sizes, one per layer (`background-size`).
    BgSize(Vec<BgSize>),
}

/// The style properties bindings set on a node, on top of its selected record (like an inline style in
/// Chrome), and the record and layout style they give. Rebuilt when the selected record or a value changes.
#[derive(Debug, Clone)]
pub struct Overrides {
    /// The properties, in the order they were first set.
    pub props: Vec<(PropId, OverValue)>,
    /// The selected record with the properties applied.
    pub record: StyleRecord,
    /// The layout style of `record`.
    pub taffy: taffy::Style,
}

impl Overrides {
    /// The overrides of `props` over a base record.
    pub fn new(base: &StyleRecord, props: Vec<(PropId, OverValue)>) -> Self {
        let mut record = base.clone();
        for (prop, value) in &props {
            apply_over(&mut record, *prop, value);
        }
        let taffy = crate::layout::style::taffy_style(&record);
        Self {
            props,
            record,
            taffy,
        }
    }
}

/// Whether a style binding can set the property (flag B of the profile) and the record has a field for it.
pub fn bindable(prop: PropId) -> bool {
    matches!(
        prop,
        PropId::Top
            | PropId::Right
            | PropId::Bottom
            | PropId::Left
            | PropId::Width
            | PropId::Height
            | PropId::FlexGrow
            | PropId::Color
            | PropId::BackgroundColor
            | PropId::Opacity
            | PropId::Transform
            | PropId::BackgroundSize
    )
}

/// Writes one bound value into a record. A value of the wrong kind for the property changes nothing (the
/// binding parsed it for that property).
fn apply_over(r: &mut StyleRecord, prop: PropId, value: &OverValue) {
    match (prop, value) {
        (PropId::Top, OverValue::Len(l)) => r.top = *l,
        (PropId::Right, OverValue::Len(l)) => r.right = *l,
        (PropId::Bottom, OverValue::Len(l)) => r.bottom = *l,
        (PropId::Left, OverValue::Len(l)) => r.left = *l,
        (PropId::Width, OverValue::Len(l)) => r.width = *l,
        (PropId::Height, OverValue::Len(l)) => r.height = *l,
        (PropId::FlexGrow, OverValue::Num(v)) => r.flex_grow = v.max(0.0),
        (PropId::Opacity, OverValue::Num(v)) => r.opacity = v.clamp(0.0, 1.0),
        (PropId::Color, OverValue::Color(c)) => r.color = *c,
        (PropId::BackgroundColor, OverValue::Color(c)) => r.background_color = *c,
        (PropId::Transform, OverValue::Transform(t)) => r.transform = t.clone(),
        (PropId::BackgroundSize, OverValue::BgSize(s)) => r.background_size = s.clone(),
        _ => {}
    }
}

/// Sets (`Some`) or clears (`None`) a bound property on a node and returns the passes the change needs.
pub(crate) fn set_override(
    arena: &mut crate::arena::Arena,
    styles: &Styles,
    index: &StyleIndex,
    id: crate::arena::NodeId,
    prop: PropId,
    value: Option<OverValue>,
) -> Dirty {
    let Some(node) = arena.get_mut(id) else {
        return Dirty::NONE;
    };
    let base = index.record(styles, node.style);
    let mut props = node
        .over
        .as_ref()
        .map(|o| o.props.clone())
        .unwrap_or_default();
    match (props.iter().position(|(p, _)| *p == prop), value) {
        (Some(at), Some(v)) => props[at].1 = v,
        (None, Some(v)) => props.push((prop, v)),
        (Some(at), None) => {
            props.remove(at);
        }
        (None, None) => return Dirty::NONE,
    }
    let old = node.over.take();
    let old_record = old.as_ref().map_or(base, |o| &o.record);
    if props.is_empty() {
        return diff(old_record, base);
    }
    let new = Overrides::new(base, props);
    let bits = diff(old_record, &new.record);
    node.over = Some(Box::new(new));
    bits
}

/// The record a node is drawn and laid out with: its selected record, or the one its style bindings give.
pub fn node_record<'a>(
    index: &'a StyleIndex,
    styles: &'a Styles,
    node: &'a Node,
) -> &'a StyleRecord {
    match node.overrides() {
        Some(o) => &o.record,
        None => index.record(styles, node.style()),
    }
}

/// The factor between CSS px and device pixels, and the logical viewport layout runs in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scale {
    /// `ui_scale * dpr`: device pixels per layout px.
    pub k: f32,
    /// The window in layout (CSS) px: physical size / k. Breakpoint cells compare against it, as with browser
    /// zoom.
    pub viewport_css: [f32; 2],
}

impl Default for Scale {
    fn default() -> Self {
        Self {
            k: 1.0,
            viewport_css: [1920.0, 1080.0],
        }
    }
}

impl Scale {
    /// The scale of an environment. A non-finite or non-positive factor is replaced by 1; the flag says so.
    pub fn of(env: &Env) -> (Scale, bool) {
        let sane = |v: f32| v.is_finite() && v > 0.0;
        let mut ok = true;
        let mut factor = |v: f32| {
            if sane(v) {
                v
            } else {
                ok = false;
                1.0
            }
        };
        let k = factor(env.ui_scale) * factor(env.dpr);
        let (w, h) = env.viewport_px;
        let scale = Scale {
            k,
            viewport_css: [w as f32 / k, h as f32 / k],
        };
        (scale, ok)
    }
}

/// What selection depends on besides the node.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StyleEnv {
    /// The viewport cell (`Styles::cell` of the logical viewport).
    pub cell: u8,
    /// The theme colors resolve under.
    pub theme: u16,
    /// UI scale and logical viewport.
    pub scale: Scale,
}

impl StyleEnv {
    /// The style environment of an environment; the flag is false when the environment was out of range.
    pub fn of(styles: &Styles, env: &Env) -> (StyleEnv, bool) {
        let (scale, ok) = Scale::of(env);
        let se = StyleEnv {
            cell: styles.cell(scale.viewport_css),
            theme: env.theme,
            scale,
        };
        (se, ok)
    }
}

/// Facts about a package's sets and records, computed once when the skin loads.
#[derive(Debug, Clone)]
pub struct StyleIndex {
    set_varies: Vec<bool>,
    record_tokens: Vec<bool>,
    hidden: StyleRecord,
    placeholder: StyleRecord,
}

impl StyleIndex {
    /// Scans the sets and records.
    pub fn new(styles: &Styles) -> Self {
        let set_varies = styles
            .sets
            .iter()
            .map(|set| {
                let per_cell = 1usize << set.conds.len();
                let mut cells = set.table.chunks(per_cell);
                let first = cells.next();
                cells.any(|c| Some(c) != first)
            })
            .collect();
        let record_tokens = styles.records.iter().map(tokens::has_tokens).collect();
        let mut hidden = StyleRecord::initial();
        hidden.display = ui2_package::style::Display::None;
        let mut placeholder = StyleRecord::initial();
        placeholder.display = ui2_package::style::Display::Block;
        Self {
            set_varies,
            record_tokens,
            hidden,
            placeholder,
        }
    }

    /// Whether the set picks different records in different viewport cells (its nodes restyle when the
    /// cell changes; the others do not).
    pub fn varies_by_cell(&self, set: StyleSetId) -> bool {
        self.set_varies.get(set.index()).copied().unwrap_or(false)
    }

    /// Whether a record refers to a theme token (its nodes repaint when the theme changes).
    pub fn has_tokens(&self, id: StyleId) -> bool {
        self.record_tokens.get(id.index()).copied().unwrap_or(false)
    }

    /// The record of an id, the built-in records for service nodes and placeholders included. An unknown id
    /// (never the case for a read package) answers the hidden record.
    pub fn record<'a>(&'a self, styles: &'a Styles, id: StyleId) -> &'a StyleRecord {
        match id {
            PLACEHOLDER_STYLE => &self.placeholder,
            HIDDEN_STYLE => &self.hidden,
            _ => styles.record(id).unwrap_or(&self.hidden),
        }
    }
}

/// The passes a change from record `a` to record `b` needs: LAYOUT for box, flex/grid and font metrics,
/// TEXT for what shaping and line breaking read, PAINT for what only drawing reads. `cursor` and
/// `pointer-events` (read by input) and `transitions` (read by the animation step) need no pass.
///
/// Every field is named here: a new property of the profile does not compile until it is given a group.
pub fn diff(a: &StyleRecord, b: &StyleRecord) -> Dirty {
    let StyleRecord {
        display,
        position,
        z_index,
        box_sizing,
        top,
        right,
        bottom,
        left,
        width,
        height,
        min_width,
        min_height,
        max_width,
        max_height,
        margin_top,
        margin_right,
        margin_bottom,
        margin_left,
        padding_top,
        padding_right,
        padding_bottom,
        padding_left,
        overflow_x,
        overflow_y,
        flex_direction,
        flex_wrap,
        flex_grow,
        flex_shrink,
        flex_basis,
        order,
        justify_content,
        align_items,
        align_self,
        align_content,
        row_gap,
        column_gap,
        color,
        background_color,
        opacity,
        visibility,
        border_top_width,
        border_right_width,
        border_bottom_width,
        border_left_width,
        border_top_style,
        border_right_style,
        border_bottom_style,
        border_left_style,
        border_top_color,
        border_right_color,
        border_bottom_color,
        border_left_color,
        border_top_left_radius,
        border_top_right_radius,
        border_bottom_right_radius,
        border_bottom_left_radius,
        box_shadow,
        text_shadow,
        transform,
        transform_origin,
        cursor: _,
        pointer_events: _,
        font_family,
        font_size,
        font_weight,
        font_style,
        font_variant_numeric,
        line_height,
        letter_spacing,
        word_spacing,
        text_align,
        white_space_collapse,
        text_wrap_mode,
        text_overflow,
        overflow_wrap,
        word_break,
        text_transform,
        text_decoration_line,
        fill,
        stroke,
        stroke_width,
        stroke_linecap,
        stroke_linejoin,
        background_image,
        background_size,
        background_position_x,
        background_position_y,
        background_repeat,
        background_origin,
        background_clip,
        outline_style,
        outline_width,
        outline_color,
        outline_offset,
        justify_items,
        justify_self,
        aspect_ratio,
        scrollbar_width,
        grid_template_columns,
        grid_template_rows,
        grid_auto_columns,
        grid_auto_rows,
        grid_auto_flow,
        grid_row_start,
        grid_row_end,
        grid_column_start,
        grid_column_end,
        border_image_source,
        border_image_slice,
        border_image_width,
        border_image_repeat,
        border_image_outset,
        clip_path,
        text_indent,
        text_wrap_style,
        vertical_align,
        direction,
        writing_mode,
        transitions: _,
    } = a;
    let mut d = Dirty::NONE;
    macro_rules! group {
        ($bits:expr; $($f:ident),* $(,)?) => {
            if false $(|| *$f != b.$f)* {
                d |= $bits;
            }
        };
    }
    group!(Dirty::LAYOUT;
        display, position, box_sizing, top, right, bottom, left, width, height, min_width, min_height,
        max_width, max_height, margin_top, margin_right, margin_bottom, margin_left, padding_top,
        padding_right, padding_bottom, padding_left, overflow_x, overflow_y, flex_direction, flex_wrap,
        flex_grow, flex_shrink, flex_basis, order, justify_content, align_items, align_self,
        align_content, row_gap, column_gap, border_top_width, border_right_width, border_bottom_width,
        border_left_width, justify_items, justify_self, aspect_ratio, scrollbar_width,
        grid_template_columns, grid_template_rows, grid_auto_columns, grid_auto_rows, grid_auto_flow,
        grid_row_start, grid_row_end, grid_column_start, grid_column_end,
    );
    group!(Dirty::LAYOUT | Dirty::TEXT;
        font_family, font_size, font_weight, font_style, font_variant_numeric, line_height,
        letter_spacing, word_spacing, white_space_collapse, text_wrap_mode, overflow_wrap, word_break,
        text_transform, text_indent, text_wrap_style, vertical_align, direction, writing_mode,
    );
    group!(Dirty::TEXT; text_align, text_overflow);
    group!(Dirty::PAINT;
        z_index, color, background_color, opacity, visibility, border_top_style, border_right_style,
        border_bottom_style, border_left_style, border_top_color, border_right_color,
        border_bottom_color, border_left_color, border_top_left_radius, border_top_right_radius,
        border_bottom_right_radius, border_bottom_left_radius, box_shadow, text_shadow, transform,
        transform_origin, text_decoration_line, fill, stroke, stroke_width, stroke_linecap,
        stroke_linejoin, background_image, background_size, background_position_x,
        background_position_y, background_repeat, background_origin, background_clip,
        // An outline takes no space: it is paint only.
        outline_style, outline_width, outline_color, outline_offset,
        // A border image paints over the border box (its outset is always 0); a clip only masks.
        border_image_source, border_image_slice, border_image_width, border_image_repeat,
        border_image_outset, clip_path,
    );
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use ui2_package::StyleSet;
    use ui2_package::style::{ColorV, Cursor, Len, LenAuto};

    #[test]
    fn diff_groups_fields_by_pass() {
        let a = StyleRecord::initial();
        let mut b = a.clone();
        assert_eq!(diff(&a, &b), Dirty::NONE);
        b.cursor = Cursor::Pointer;
        assert_eq!(diff(&a, &b), Dirty::NONE);
        b.background_color = ColorV::Lit([1, 2, 3, 4]);
        assert_eq!(diff(&a, &b), Dirty::PAINT);
        b.width = LenAuto::Len(Len::px(10.0));
        assert_eq!(diff(&a, &b), Dirty::PAINT | Dirty::LAYOUT);
        let mut c = a.clone();
        c.font_size = Len::px(20.0);
        assert_eq!(diff(&a, &c), Dirty::LAYOUT | Dirty::TEXT);
        let mut c = a.clone();
        c.text_overflow = ui2_package::style::TextOverflow::Ellipsis;
        assert_eq!(diff(&a, &c), Dirty::TEXT);
        let mut c = a.clone();
        c.outline_width = Len::px(8.0);
        c.background_size = vec![ui2_package::style::BgSize::Cover];
        assert_eq!(diff(&a, &c), Dirty::PAINT);
    }

    #[test]
    fn scale_divides_the_viewport_and_survives_bad_input() {
        let env = Env {
            viewport_px: (1920, 1080),
            dpr: 1.0,
            ui_scale: 1.5,
            theme: 0,
        };
        let (s, ok) = Scale::of(&env);
        assert!(ok);
        assert_eq!(s.k, 1.5);
        assert_eq!(s.viewport_css, [1280.0, 720.0]);
        let (s, ok) = Scale::of(&Env {
            dpr: f32::NAN,
            ..env
        });
        assert!(!ok);
        assert_eq!(s.k, 1.5);
    }

    #[test]
    fn sets_that_vary_by_cell_are_found() {
        let styles = Styles {
            media_w: vec![ui2_package::Bound {
                px: 1600.0,
                upper_inclusive: false,
            }],
            records: vec![StyleRecord::initial(); 3],
            sets: vec![
                StyleSet::plain(vec![StyleId(0), StyleId(0)]),
                StyleSet::plain(vec![StyleId(0), StyleId(1)]),
            ],
            ..Styles::default()
        };
        let idx = StyleIndex::new(&styles);
        assert!(!idx.varies_by_cell(StyleSetId(0)));
        assert!(idx.varies_by_cell(StyleSetId(1)));
        assert!(!idx.varies_by_cell(StyleSetId(9)));
        assert_eq!(
            idx.record(&styles, HIDDEN_STYLE).display,
            ui2_package::style::Display::None
        );
    }
}
