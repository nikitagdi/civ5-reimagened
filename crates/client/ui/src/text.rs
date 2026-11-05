//! The border with `ui-text`: the trait the text adapter implements. `ui-core` asks for a
//! layout of a string and draws the quads that come back; fonts, glyphs, markup and wrapping
//! stay on the other side.

use draw_list::TextureId;
use glam::Vec2;

/// Horizontal alignment of the lines of a block (from the horizontal anchor of the control).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextAlign {
    #[default]
    Left,
    Center,
    Right,
}

/// What `ui-core` asks for. One request per text control; the answer is cached until the
/// request changes (as `TextControl::RebuildText` does).
#[derive(Debug, Clone, PartialEq)]
pub struct TextRequest {
    /// UTF-8 with the markup (`[ICON_*]`, `[COLOR_*]`, `[NEWLINE]`).
    pub text: String,
    pub font: String,
    pub style: String,
    /// Colours of the three layers, RGBA 0..1.
    pub colors: [[f32; 4]; 3],
    /// `-1`: no wrapping.
    pub wrap_width: i32,
    /// `-1`: no truncation.
    pub truncate_width: i32,
    pub leading_offset: i32,
    pub align: TextAlign,
    /// Every glyph is drawn as `*` (`Obscure`).
    pub obscured: bool,
    /// Markup is parsed; off for edit boxes.
    pub parse_markup: bool,
}

/// One glyph or icon quad in the coordinates of the text block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextQuad {
    pub texture: TextureId,
    pub min: Vec2,
    pub max: Vec2,
    pub uv_min: Vec2,
    pub uv_max: Vec2,
    /// RGBA8, not premultiplied.
    pub color: [u8; 4],
}

/// The answer: the size of the block and the quads to draw, back to front.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TextLayout {
    pub size: Vec2,
    pub quads: Vec<TextQuad>,
    pub truncated: bool,
}

/// A `LocalizeAndSetText` argument.
#[derive(Debug, Clone, PartialEq)]
pub enum LocArg {
    Bool(bool),
    Number(f64),
    Str(String),
}

pub trait TextEngine {
    fn layout(&mut self, req: &TextRequest) -> TextLayout;
    /// `GetStringSize`: the width of a string in pixels.
    fn measure(&mut self, text: &str, font: &str) -> i32;
    /// `Localization::Lookup(key)` with arguments, as UTF-8. An unknown key returns the key.
    fn localize(&mut self, key: &str, args: &[LocArg]) -> String;
}

/// A fixed-metrics engine for tests and the M1 example until the adapter exists: 8 px per
/// character, 16 px per line, one white quad per visible character. Keys are returned as they are.
#[derive(Debug, Default)]
pub struct StubText {
    /// Texts keyed by `TXT_KEY_*`.
    pub strings: std::collections::HashMap<String, String>,
    /// Layout calls made, for cache tests.
    pub layouts: usize,
}

impl StubText {
    pub const CHAR_W: f32 = 8.0;
    pub const LINE_H: f32 = 16.0;
}

impl TextEngine for StubText {
    fn layout(&mut self, req: &TextRequest) -> TextLayout {
        self.layouts += 1;
        let max_chars = if req.wrap_width > 0 {
            ((req.wrap_width as f32 / Self::CHAR_W).floor() as usize).max(1)
        } else {
            usize::MAX
        };
        let mut lines: Vec<String> = Vec::new();
        for para in req.text.split("[NEWLINE]").flat_map(|p| p.split('\n')) {
            let mut cur = String::new();
            for word in para.split(' ') {
                let need = if cur.is_empty() { 0 } else { 1 } + word.chars().count();
                if !cur.is_empty() && cur.chars().count() + need > max_chars {
                    lines.push(std::mem::take(&mut cur));
                }
                if !cur.is_empty() {
                    cur.push(' ');
                }
                cur.push_str(word);
            }
            lines.push(cur);
        }
        let mut out = TextLayout::default();
        let mut width = 0.0f32;
        for (row, line) in lines.iter().enumerate() {
            let w = line.chars().count() as f32 * Self::CHAR_W;
            width = width.max(w);
            for (i, ch) in line.chars().enumerate() {
                if ch == ' ' {
                    continue;
                }
                let min = Vec2::new(i as f32 * Self::CHAR_W, row as f32 * Self::LINE_H);
                out.quads.push(TextQuad {
                    texture: TextureId::WHITE,
                    min,
                    max: min + Vec2::new(Self::CHAR_W, Self::LINE_H),
                    uv_min: Vec2::ZERO,
                    uv_max: Vec2::ONE,
                    color: [255; 4],
                });
            }
        }
        out.size = Vec2::new(width, lines.len() as f32 * Self::LINE_H);
        out
    }

    fn measure(&mut self, text: &str, _font: &str) -> i32 {
        (text.chars().count() as f32 * Self::CHAR_W) as i32
    }

    fn localize(&mut self, key: &str, args: &[LocArg]) -> String {
        let mut s = self
            .strings
            .get(key)
            .cloned()
            .unwrap_or_else(|| key.to_owned());
        for (i, a) in args.iter().enumerate() {
            let v = match a {
                LocArg::Bool(b) => i32::from(*b).to_string(),
                LocArg::Number(n) => n.to_string(),
                LocArg::Str(t) => t.clone(),
            };
            s = s.replace(&format!("{{{}}}", i + 1), &v);
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(text: &str, wrap: i32) -> TextRequest {
        TextRequest {
            text: text.to_owned(),
            font: "TwCenMT16".into(),
            style: "Base".into(),
            colors: [[1.0; 4]; 3],
            wrap_width: wrap,
            truncate_width: -1,
            leading_offset: 0,
            align: TextAlign::Left,
            obscured: false,
            parse_markup: true,
        }
    }

    #[test]
    fn stub_measures_and_wraps() {
        let mut t = StubText::default();
        let l = t.layout(&req("ab cd", -1));
        assert_eq!(l.size, Vec2::new(40.0, 16.0));
        assert_eq!(l.quads.len(), 4);
        let l = t.layout(&req("ab cd", 24));
        assert_eq!(l.size, Vec2::new(16.0, 32.0));
        assert_eq!(t.layouts, 2);
        assert_eq!(t.localize("TXT_KEY_X", &[]), "TXT_KEY_X");
    }
}
