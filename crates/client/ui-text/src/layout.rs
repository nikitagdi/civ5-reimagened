//! Wrapping, truncation, sizing and glyph quads: `TextControl::ProcessWordwrap` (0x0869a9b8),
//! `ShouldWrapHere` (0x086a0f06), `CalculateTruncation` (0x0869aea4), `CalculateSize` (0x0869aefc),
//! `BuildBuffersFromBlocks` (0x0869c166) and `InsertTruncationTrail` (0x0869bbae).
//!
//! Positions are integers: the exe keeps the pen as `unsigned`.

use crate::FontId;
use crate::font::Font;
use crate::icons::Rgba;
use crate::markup::Block;

/// The characters a line may break at (`m_acBreakChars`).
const BREAK_CHARS: &[u32] = &[b' ' as u32];
/// The truncation trail (`GetStringSize("...")`, `InsertTruncationTrail(..., "...")`).
pub(crate) const TRAIL: &str = "...";

/// Everything the layout reads besides the blocks.
pub(crate) struct Env<'a> {
    pub fonts: &'a [Font],
    pub main: FontId,
    pub text: &'a str,
    pub obscured: bool,
    pub wrap_width: i32,
    pub truncate_width: i32,
    pub leading_offset: i32,
}

impl<'a> Env<'a> {
    fn font(&self) -> &'a Font {
        &self.fonts[self.main.0 as usize]
    }

    fn icon_font(&self, id: FontId) -> &'a Font {
        &self.fonts[id.0 as usize]
    }

    /// The character at byte `offset`: the glyph to use (`*` when obscured, a space when the font has none)
    /// and the number of extra bytes (`UTF8::NextUnsafe`).
    fn glyph_at(&self, offset: usize) -> (u32, usize) {
        let rest = self.text.get(offset..).unwrap_or("");
        let (ch, extra) = match rest.chars().next() {
            Some(c) => (c as u32, c.len_utf8() - 1),
            None => (0, 0),
        };
        (self.fold(ch), extra)
    }

    fn fold(&self, ch: u32) -> u32 {
        let ch = if self.obscured { b'*' as u32 } else { ch };
        if self.font().has_glyph(ch) {
            ch
        } else {
            b' ' as u32
        }
    }

    /// The glyph value of the character that ends just before byte `end`.
    fn glyph_before(&self, end: usize) -> u32 {
        let head = self.text.get(..end).unwrap_or("");
        self.fold(head.chars().next_back().map_or(0, |c| c as u32))
    }

    fn is_break(&self, g: u32) -> bool {
        BREAK_CHARS.contains(&g)
    }

    /// `GetStringSize`: the pen width of a string.
    pub fn string_size(&self, s: &str) -> i32 {
        // re: 0x0869ba3e TextControl::GetStringSize
        s.chars()
            .map(|c| self.font().glyph(self.fold(c as u32)).step())
            .sum()
    }
}

/// `ShouldWrapHere`: should a line break come before the character at byte `offset` of block `bi`?
fn should_wrap_here(env: &Env, blocks: &[Block], bi: usize, offset: usize, pen: f32) -> bool {
    // re: 0x086a0f06 TextControl::ShouldWrapHere
    if env.wrap_width == -1 {
        return false;
    }
    let wrap = env.wrap_width as f32;
    let blk = &blocks[bi];
    let glyph = match blk.icon {
        None => env.font().glyph(env.glyph_at(offset).0),
        Some((f, code)) => env.icon_font(f).glyph(code),
    };
    if glyph.advance + pen > wrap {
        return true;
    }
    // Even for an icon block the exe tests the text character at `offset`.
    if env.is_break(env.glyph_at(offset).0) {
        return false;
    }
    let at_word_start;
    if blk.offset == offset || blk.icon.is_some() {
        // walk back to the last non-empty text block and test its final character
        let mut prev = bi;
        loop {
            if prev == 0 {
                at_word_start = true;
                break;
            }
            prev -= 1;
            let p = &blocks[prev];
            if p.icon.is_some() {
                return false;
            }
            if p.len == 0 {
                continue;
            }
            at_word_start = env.is_break(env.glyph_before(p.offset + p.len));
            break;
        }
    } else {
        at_word_start = env.is_break(env.glyph_before(offset));
    }
    if !at_word_start {
        return false;
    }
    // measure the word that starts here, across blocks, up to the next break character
    let mut word = 0.0f32;
    let mut total = pen;
    let mut ub = bi;
    let mut a3 = offset;
    let mut cont = true;
    while cont {
        let b = &blocks[ub];
        let mut keep = cont;
        match b.icon {
            None => {
                if a3 == b.offset && b.newline {
                    break;
                }
                let (g, extra) = env.glyph_at(a3);
                a3 += extra;
                if env.is_break(g) {
                    keep = false;
                } else {
                    let adv = env.font().glyph(g).advance;
                    total += adv;
                    word += adv;
                }
            }
            Some((f, code)) => {
                let adv = env.icon_font(f).glyph(code).advance;
                total += adv;
                word += adv;
            }
        }
        a3 += 1;
        if b.len + b.offset <= a3 || b.icon.is_some() {
            ub += 1;
            cont = false;
            if ub != blocks.len() {
                a3 = blocks[ub].offset;
                cont = keep;
            }
        } else {
            cont = keep;
        }
    }
    if (word as i32) < env.wrap_width {
        return env.wrap_width <= total as i32;
    }
    false
}

/// `ProcessWordwrap`: splits blocks at the positions `ShouldWrapHere` chooses and marks the pieces that start
/// a line. It does nothing when a truncate width is set.
pub(crate) fn wordwrap(env: &Env, input: &[Block]) -> Vec<Block> {
    // re: 0x0869a9b8 TextControl::ProcessWordwrap
    if env.truncate_width >= 1 {
        return input.to_vec();
    }
    let mut out: Vec<Block> = Vec::with_capacity(input.len() + 4);
    let mut pen: u32 = 0;
    let mut just_wrapped = false;
    for (bi, src) in input.iter().enumerate() {
        let mut cur = src.clone();
        if src.newline {
            pen = 0;
        }
        match src.icon {
            None => {
                let mut used = 0usize;
                cur.len = 0;
                let end = src.offset + src.len;
                let mut off = src.offset;
                while off < end {
                    let (g, extra) = env.glyph_at(off);
                    if should_wrap_here(env, input, bi, off, pen as f32) {
                        out.push(cur.clone());
                        cur.offset += used;
                        cur.newline = true;
                        just_wrapped = true;
                        used = 0;
                        pen = 0;
                    }
                    used += extra + 1;
                    cur.len = used;
                    let step = env.font().glyph(g).step() as u32;
                    if !just_wrapped {
                        pen += step;
                    } else if env.is_break(g) {
                        // a space at the start of a wrapped line is dropped
                        cur.offset += used;
                        cur.len = 0;
                        used = 0;
                    } else {
                        just_wrapped = false;
                        pen += step;
                    }
                    off += 1 + extra;
                }
                if used != 0 || cur.newline || src.newline {
                    out.push(cur);
                }
            }
            Some((f, code)) => {
                let glyph = env.icon_font(f).glyph(code);
                if should_wrap_here(env, input, bi, 0, pen as f32) {
                    cur.newline = true;
                    pen = 0;
                }
                out.push(cur);
                pen += glyph.step() as u32;
                just_wrapped = false;
            }
        }
    }
    out
}

/// One text line of the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Line {
    /// Top of the line relative to the block origin.
    pub y: i32,
    /// Pen width of the line.
    pub width: i32,
}

/// Result of [`measure_blocks`].
pub(crate) struct Measured {
    pub width: i32,
    pub height: i32,
    pub lines: Vec<Line>,
}

/// `CalculateSize`. With `truncated`, measuring stops at the first character that would pass the
/// truncate width, and the trail width is added.
pub(crate) fn measure_blocks(env: &Env, blocks: &[Block], truncated: bool) -> Measured {
    // re: 0x0869aefc TextControl::CalculateSize
    let trail = env.string_size(TRAIL);
    let leading = env.font().leading();
    let mut y = 0i32;
    let mut x = 0i32;
    let mut max_w = 0i32;
    let mut lines: Vec<Line> = Vec::new();
    let mut line_top = 0i32;
    let check = |x: i32, adv: f32| {
        env.truncate_width > 0
            && truncated
            && env.truncate_width < (x as f32 + adv + trail as f32) as i32
    };
    'blocks: for b in blocks {
        if b.newline {
            lines.push(Line {
                y: line_top,
                width: x,
            });
            y += leading + env.leading_offset;
            line_top = y;
            x = 0;
        }
        match b.icon {
            None => {
                let end = b.offset + b.len;
                let mut off = b.offset;
                while off < end {
                    let (g, extra) = env.glyph_at(off);
                    let glyph = env.font().glyph(g);
                    if check(x, glyph.advance) {
                        x += trail;
                        break 'blocks;
                    }
                    x += glyph.step();
                    max_w = max_w.max(x);
                    off += 1 + extra;
                }
            }
            Some((f, code)) => {
                let glyph = env.icon_font(f).glyph(code);
                if check(x, glyph.advance) {
                    x += trail;
                    break 'blocks;
                }
                x += glyph.step();
                max_w = max_w.max(x);
            }
        }
    }
    lines.push(Line {
        y: line_top,
        width: x,
    });
    max_w = max_w.max(x);
    Measured {
        width: max_w,
        height: env.font().ascent() + y,
        lines,
    }
}

/// `CalculateTruncation`: whether the text is wider than the truncate width.
pub(crate) fn is_truncated(env: &Env, blocks: &[Block]) -> bool {
    // re: 0x0869aea4 TextControl::CalculateTruncation
    if env.truncate_width <= 0 {
        return false;
    }
    measure_blocks(env, blocks, false).width > env.truncate_width
}

/// One glyph rectangle ready to draw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphQuad {
    /// Index into [`TextLayout::textures`].
    pub texture: u32,
    /// `x0, y0, x1, y1` in pixels from the block origin.
    pub rect: [f32; 4],
    /// `u0, v0, u1, v1`.
    pub uv: [f32; 4],
    pub color: Rgba,
    /// Layer of the style (0 = fill, drawn last; shadow and outline are layers 1 and 2).
    pub layer: u8,
    /// Style row of the block.
    pub style: u8,
    /// An icon (white, drawn after all text).
    pub icon: bool,
}

/// A font texture a quad refers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextureRef {
    pub font: String,
    pub texture: String,
    pub src: String,
    pub allow_color: bool,
}

/// The laid-out text.
#[derive(Debug, Clone, PartialEq)]
pub struct TextLayout {
    /// Block size: widest line, and ascent plus the line steps (`GetSizeX/Y` before padding).
    pub size: [i32; 2],
    pub lines: Vec<Line>,
    /// The text was wider than the truncate width and ends with `...`.
    pub truncated: bool,
    /// Quads in draw order: text by style row, layers 2 then 1 then 0, then icons.
    pub quads: Vec<GlyphQuad>,
    pub textures: Vec<TextureRef>,
}

const WHITE: Rgba = [255; 4];

struct Builder<'a> {
    env: &'a Env<'a>,
    textures: Vec<TextureRef>,
    tex_index: std::collections::HashMap<(u32, usize), u32>,
    /// `[style][layer]` groups of text quads.
    groups: Vec<Vec<GlyphQuad>>,
    icons: Vec<GlyphQuad>,
}

impl Builder<'_> {
    fn texture(&mut self, font: FontId, tex: usize) -> u32 {
        if let Some(&i) = self.tex_index.get(&(font.0, tex)) {
            return i;
        }
        let f = &self.env.fonts[font.0 as usize];
        let t = &f.textures()[tex];
        let i = self.textures.len() as u32;
        self.textures.push(TextureRef {
            font: f.name.clone(),
            texture: t.name.clone(),
            src: t.src.clone(),
            allow_color: t.allow_color,
        });
        self.tex_index.insert((font.0, tex), i);
        i
    }

    /// The quads of one text glyph, one per used style layer.
    fn text_glyph(&mut self, b: &Block, g: u32, pen: i32, line_y: i32) {
        let font = self.env.font();
        let glyph = font.glyph(g);
        let Some(style) = font.style(b.style as usize).or_else(|| font.style(0)) else {
            return;
        };
        let rect = [
            glyph.rect[0] + pen as f32,
            glyph.rect[1] + line_y as f32,
            glyph.rect[2] + pen as f32,
            glyph.rect[3] + line_y as f32,
        ];
        if rect[0] == rect[2] || rect[1] == rect[3] {
            return;
        }
        let row = (b.style as usize).min(self.groups.len() / 3 - 1);
        for layer in 0..3 {
            let tex = style.layers[layer];
            if tex < 0 {
                continue;
            }
            let texture = self.texture(self.env.main, tex as usize);
            self.groups[row * 3 + layer].push(GlyphQuad {
                texture,
                rect,
                uv: glyph.uv,
                color: b.colors[layer],
                layer: layer as u8,
                style: row as u8,
                icon: false,
            });
        }
    }

    /// `InsertTruncationTrail`: the `...` glyphs at the pen, in the colors and style of block `b`.
    fn trail(&mut self, b: &Block, mut pen: i32, line_y: i32) {
        // re: 0x0869bbae TextControl::InsertTruncationTrail
        for ch in TRAIL.chars() {
            let g = self.env.fold(ch as u32);
            self.text_glyph(b, g, pen, line_y);
            pen += self.env.font().glyph(g).step();
        }
    }
}

/// `BuildBuffersFromBlocks`: the quads of all blocks.
pub(crate) fn build_quads(
    env: &Env,
    blocks: &[Block],
    truncated: bool,
) -> (Vec<GlyphQuad>, Vec<TextureRef>) {
    // re: 0x0869c166 TextControl::BuildBuffersFromBlocks
    let rows = env.font().styles().len().max(6);
    let mut bld = Builder {
        env,
        textures: Vec::new(),
        tex_index: Default::default(),
        groups: vec![Vec::new(); rows * 3],
        icons: Vec::new(),
    };
    let trail = env.string_size(TRAIL);
    let leading = env.font().leading();
    let mut pen = 0i32;
    let mut line_y = 0i32;
    let check = |x: i32, adv: f32| {
        env.truncate_width > 0
            && truncated
            && env.truncate_width < (x as f32 + adv + trail as f32) as i32
    };
    'blocks: for b in blocks {
        if b.newline {
            line_y += leading + env.leading_offset;
            pen = 0;
        }
        match b.icon {
            None => {
                let end = b.offset + b.len;
                let mut off = b.offset;
                while off < end {
                    let (g, extra) = env.glyph_at(off);
                    let glyph = env.font().glyph(g);
                    if check(pen, glyph.advance) {
                        bld.trail(b, pen, line_y);
                        break 'blocks;
                    }
                    bld.text_glyph(b, g, pen, line_y);
                    pen += glyph.step();
                    off += 1 + extra;
                }
            }
            Some((f, code)) => {
                let font = env.icon_font(f);
                let glyph = font.glyph(code);
                if check(pen, glyph.advance) {
                    bld.trail(b, pen, line_y);
                    break 'blocks;
                }
                let ascent = env.font().ascent() as f32;
                let rect = [
                    glyph.rect[0] + pen as f32,
                    glyph.rect[1] + line_y as f32 + ascent,
                    glyph.rect[2] + pen as f32,
                    glyph.rect[3] + line_y as f32 + ascent,
                ];
                let texture = bld.texture(f, 0);
                bld.icons.push(GlyphQuad {
                    texture,
                    rect,
                    uv: glyph.uv,
                    color: WHITE,
                    layer: 0,
                    style: 0,
                    icon: true,
                });
                pen += glyph.step();
            }
        }
    }
    let mut quads = Vec::new();
    let rows = bld.groups.len() / 3;
    for row in 0..rows {
        for layer in (0..3).rev() {
            quads.append(&mut bld.groups[row * 3 + layer]);
        }
    }
    quads.append(&mut bld.icons);
    (quads, bld.textures)
}
