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

use ui2_package::style::StyleRecord;
use ui2_package::{StyleId, StyleSetId, Styles};

use crate::Env;
use crate::arena::{Dirty, HIDDEN_STYLE, PLACEHOLDER_STYLE};

pub(crate) use select::attach;

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
        border_left_width,
    );
    group!(Dirty::LAYOUT | Dirty::TEXT;
        font_family, font_size, font_weight, font_style, font_variant_numeric, line_height,
        letter_spacing, word_spacing, white_space_collapse, text_wrap_mode, overflow_wrap, word_break,
        text_transform,
    );
    group!(Dirty::TEXT; text_align, text_overflow);
    group!(Dirty::PAINT;
        z_index, color, background_color, opacity, visibility, border_top_style, border_right_style,
        border_bottom_style, border_left_style, border_top_color, border_right_color,
        border_bottom_color, border_left_color, border_top_left_radius, border_top_right_radius,
        border_bottom_right_radius, border_bottom_left_radius, box_shadow, text_shadow, transform,
        transform_origin, text_decoration_line, fill, stroke, stroke_width, stroke_linecap,
        stroke_linejoin,
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
