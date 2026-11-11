//! Theme colors.
//!
//! A record color is a literal or a root token of the package. Tokens resolve at paint time under the current
//! theme (`Tokens::resolve`), so switching the theme reselects nothing and lays out nothing: only nodes whose
//! record refers to a token repaint.

use ui2_package::Tokens;
use ui2_package::style::{BgImage, ColorV, Paint, StyleRecord};

/// The color of a record color under a theme.
pub fn resolve(tokens: &Tokens, theme: u16, color: ColorV) -> [u8; 4] {
    tokens.resolve(theme, color)
}

/// Whether any color of the record is a token reference.
pub fn has_tokens(r: &StyleRecord) -> bool {
    let tok = |c: &ColorV| matches!(c, ColorV::Tok(_));
    let paint = |p: &Paint| matches!(p, Paint::Color(ColorV::Tok(_)));
    [
        &r.color,
        &r.background_color,
        &r.border_top_color,
        &r.border_right_color,
        &r.border_bottom_color,
        &r.border_left_color,
        &r.outline_color,
    ]
    .into_iter()
    .any(tok)
        || r.background_image
            .iter()
            .any(|BgImage::Gradient(g)| g.stops.iter().any(|s| tok(&s.color)))
        || r.box_shadow.iter().any(|s| tok(&s.color))
        || r.text_shadow.iter().any(|s| tok(&s.color))
        || paint(&r.fill)
        || paint(&r.stroke)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ui2_package::TokenId;
    use ui2_package::style::{GradDir, Gradient, Shadow, Side, Stop};

    #[test]
    fn every_color_field_is_seen() {
        let t = ColorV::Tok(TokenId(0));
        let base = StyleRecord::initial();
        assert!(!has_tokens(&base));
        let edits: [fn(&mut StyleRecord, ColorV); 12] = [
            |r, c| r.outline_color = c,
            |r, c| {
                let lit = ColorV::Lit([0, 0, 0, 255]);
                r.background_image = vec![BgImage::Gradient(Gradient {
                    conic: false,
                    repeating: false,
                    dir: GradDir::To(Side::Right),
                    stops: vec![
                        Stop {
                            color: lit,
                            pos: None,
                        },
                        Stop {
                            color: c,
                            pos: None,
                        },
                    ],
                })]
            },
            |r, c| r.color = c,
            |r, c| r.background_color = c,
            |r, c| r.border_top_color = c,
            |r, c| r.border_right_color = c,
            |r, c| r.border_bottom_color = c,
            |r, c| r.border_left_color = c,
            |r, c| r.fill = Paint::Color(c),
            |r, c| r.stroke = Paint::Color(c),
            |r, c| {
                r.box_shadow = vec![Shadow {
                    inset: false,
                    x: 0.0,
                    y: 0.0,
                    blur: 0.0,
                    spread: 0.0,
                    color: c,
                }]
            },
            |r, c| {
                r.text_shadow = vec![Shadow {
                    inset: false,
                    x: 0.0,
                    y: 0.0,
                    blur: 0.0,
                    spread: 0.0,
                    color: c,
                }]
            },
        ];
        for edit in edits {
            let mut r = base.clone();
            edit(&mut r, t);
            assert!(has_tokens(&r));
        }
    }
}
