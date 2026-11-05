//! Text of the original UI: `.ggxml` bitmap fonts, the `[ICON_*]` / `[COLOR_*]` / `[NEWLINE]` markup,
//! wrapping, truncation, line sizes and glyph quads, as `TextControl` does them (Civ5XP reverse; each rule
//! carries the address of its exe function).
//!
//! `ui-text` loads no textures: it works in UI pixels and hands out [`GlyphQuad`]s that name font textures
//! ([`TextureRef`]); [`TextLayout::emit`] turns them into draw-list triangles through [`FontTextures`].
//!
//! SIMPLIFIED(SIMP-txt-4): the IME font (`FGXIMEFont`) and CJK input are not supported.
//!
//! The control tree (anchors, `ColorSet`, per-line alignment of the block) is `ui-core`'s. `TextControl`
//! itself has no per-line alignment: every line starts at the block's left edge. [`Line::width`] lets the
//! owner align lines if it wants to.

mod emit;
mod font;
mod icons;
mod layout;
mod markup;

use std::collections::HashMap;
use std::sync::Arc;

pub use emit::FontTextures;
pub use font::{Font, FontError, FontStyle, FontTexture, Glyph};
pub use icons::{ColorTable, FontSource, IconTable, Rgba, channel_u8, pack_control_color};
pub use layout::{GlyphQuad, Line, TextLayout, TextureRef};
pub use markup::Block;

use layout::Env;
use markup::TagResolver;

/// Handle of a loaded font (a main font or an icon font).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FontId(pub u32);

/// A layout request: the text and the state of the `TextControl` that owns it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TextParams {
    pub text: String,
    pub font: FontId,
    /// Style row of the font (`SetStyleByID`; see [`TextSystem::style_index`]).
    pub style: u8,
    /// Colors of the three layers (base, shadow, soft/stroke), from the control's `Color0..2` / `ColorSet`.
    pub colors: [Rgba; 3],
    /// `SetWrapWidth`: wrap at this width, `-1` for no wrapping.
    pub wrap_width: i32,
    /// `SetTruncateWidth`: cut at this width and add `...`, `-1` (or 0) for no truncation. Ignored by the
    /// wrapper: with a truncate width of 1 or more nothing wraps.
    pub truncate_width: i32,
    /// `LeadingOffset` of the control, added to the font leading between lines.
    pub leading_offset: i32,
    /// `SetParseMode`: handle `[...]` tags; off for edit boxes.
    pub parse: bool,
    /// `SetTextObscured`: every character shows as `*`.
    pub obscured: bool,
}

impl TextParams {
    /// Defaults of a fresh `TextControl`: no wrap, no truncation, markup on, white fill.
    pub fn new(text: &str, font: FontId) -> TextParams {
        TextParams {
            text: text.to_string(),
            font,
            style: 0,
            colors: [[255; 4]; 3],
            wrap_width: -1,
            truncate_width: -1,
            leading_offset: 0,
            parse: true,
            obscured: false,
        }
    }
}

/// Counters for the frame statistics (`ui.text.layouts`, `ui.text.quads`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TextStats {
    /// Layouts computed (cache misses).
    pub layouts: u64,
    /// Requests answered from the cache.
    pub cache_hits: u64,
    /// Quads produced by the computed layouts.
    pub quads: u64,
    /// `[ICON_*]` / `[COLOR_*]` names that were not in the tables (dropped from the text).
    pub unresolved_tags: u64,
}

const CACHE_LIMIT: usize = 4096;

/// Fonts, icon and color tables and the layout cache.
pub struct TextSystem {
    fonts: Vec<Font>,
    by_name: HashMap<String, FontId>,
    icons: IconTable,
    colors: ColorTable,
    source: Option<Box<dyn FontSource>>,
    /// Icon font file stem -> loaded font (`None`: the file is missing or does not parse).
    icon_fonts: HashMap<String, Option<FontId>>,
    cache: HashMap<TextParams, Arc<TextLayout>>,
    stats: TextStats,
}

impl TextSystem {
    pub fn new(icons: IconTable, colors: ColorTable) -> TextSystem {
        TextSystem {
            fonts: Vec::new(),
            by_name: HashMap::new(),
            icons,
            colors,
            source: None,
            icon_fonts: HashMap::new(),
            cache: HashMap::new(),
            stats: TextStats::default(),
        }
    }

    /// Where icon-font files come from; they load on the first use of one of their icons.
    pub fn set_font_source(&mut self, source: Box<dyn FontSource>) {
        self.source = Some(source);
    }

    /// Parses and registers a `.ggxml` font under `name` (a font of the same name is replaced).
    pub fn load_font(&mut self, name: &str, ggxml: &str) -> Result<FontId, FontError> {
        let font = Font::parse(name, ggxml)?;
        self.cache.clear();
        if let Some(&id) = self.by_name.get(name) {
            self.fonts[id.0 as usize] = font;
            return Ok(id);
        }
        let id = FontId(self.fonts.len() as u32);
        self.fonts.push(font);
        self.by_name.insert(name.to_string(), id);
        Ok(id)
    }

    /// A loaded font by name.
    pub fn font(&self, name: &str) -> Option<FontId> {
        self.by_name.get(name).copied()
    }

    pub fn font_data(&self, id: FontId) -> &Font {
        &self.fonts[id.0 as usize]
    }

    /// `GetStyleIndex` for `FontStyle="Shadow"` and friends.
    pub fn style_index(&self, font: FontId, style: &str) -> u8 {
        self.fonts[font.0 as usize].style_index(style)
    }

    pub fn stats(&self) -> TextStats {
        self.stats
    }

    pub fn icons(&self) -> &IconTable {
        &self.icons
    }

    pub fn colors(&self) -> &ColorTable {
        &self.colors
    }

    /// `GetStringSize`: the width of `text` in `font`, without markup (tags count as ordinary characters).
    pub fn measure(&self, text: &str, font: FontId, obscured: bool) -> i32 {
        // re: 0x0869ba3e TextControl::GetStringSize
        let env = Env {
            fonts: &self.fonts,
            main: font,
            text,
            obscured,
            wrap_width: -1,
            truncate_width: -1,
            leading_offset: 0,
        };
        env.string_size(text)
    }

    /// The blocks `ParseTextIntoBlocks` + `ProcessWordwrap` produce for a request. Block offsets index
    /// the text with `[SPACE]` / `[TAB]` expanded (see [`expand_spacing_tags`]), which is `p.text` when
    /// it holds neither tag.
    pub fn blocks(&mut self, p: &TextParams) -> Vec<Block> {
        let p = &normalized(p);
        let mut resolver = Resolver {
            fonts: &mut self.fonts,
            by_name: &mut self.by_name,
            icons: &self.icons,
            colors: &self.colors,
            source: self.source.as_deref(),
            icon_fonts: &mut self.icon_fonts,
            unresolved: &mut self.stats.unresolved_tags,
        };
        let blocks = markup::parse_blocks(&p.text, p.colors, p.style, p.parse, &mut resolver);
        let env = Env {
            fonts: &self.fonts,
            main: p.font,
            text: &p.text,
            obscured: p.obscured,
            wrap_width: p.wrap_width,
            truncate_width: p.truncate_width,
            leading_offset: p.leading_offset,
        };
        layout::wordwrap(&env, &blocks)
    }

    /// Lays a text out: the work of `RebuildText` (parse, wrap, truncation, size) and of
    /// `BuildBuffersFromBlocks` (quads). Repeated requests are answered from a cache.
    pub fn layout(&mut self, p: &TextParams) -> Arc<TextLayout> {
        if let Some(hit) = self.cache.get(p) {
            self.stats.cache_hits += 1;
            return hit.clone();
        }
        let blocks = self.blocks(p);
        let norm = normalized(p);
        let np = &*norm;
        let env = Env {
            fonts: &self.fonts,
            main: np.font,
            text: &np.text,
            obscured: np.obscured,
            wrap_width: np.wrap_width,
            truncate_width: np.truncate_width,
            leading_offset: np.leading_offset,
        };
        let truncated = layout::is_truncated(&env, &blocks);
        let m = layout::measure_blocks(&env, &blocks, truncated);
        let (quads, textures) = layout::build_quads(&env, &blocks, truncated);
        self.stats.layouts += 1;
        self.stats.quads += quads.len() as u64;
        let out = Arc::new(TextLayout {
            size: [m.width, m.height],
            lines: m.lines,
            truncated,
            quads,
            textures,
        });
        if self.cache.len() >= CACHE_LIMIT {
            self.cache.clear();
        }
        self.cache.insert(p.clone(), out.clone());
        out
    }
}

/// Width of `[TAB]`, in spaces.
pub const TAB_SPACES: usize = 4;

/// Turns `[SPACE]` into one space and `[TAB]` into [`TAB_SPACES`] spaces.
///
/// SIMPLIFIED(SIMP-txt-6): `ParseTextIntoBlocks` drops tags it does not know, yet the game's own texts use
/// `[TAB]` (indentation of quotes) and `[SPACE]` (a leading space that the XML loader would trim) and the
/// original shows them as white space, so some layer before the parser expands them; which one and how wide
/// a tab is are not reverse-engineered. Remove when the exe path is known.
pub fn expand_spacing_tags(text: &str) -> String {
    text.replace("[SPACE]", " ")
        .replace("[TAB]", &" ".repeat(TAB_SPACES))
}

fn normalized(p: &TextParams) -> std::borrow::Cow<'_, TextParams> {
    if p.parse && (p.text.contains("[SPACE]") || p.text.contains("[TAB]")) {
        std::borrow::Cow::Owned(TextParams {
            text: expand_spacing_tags(&p.text),
            ..p.clone()
        })
    } else {
        std::borrow::Cow::Borrowed(p)
    }
}

struct Resolver<'a> {
    fonts: &'a mut Vec<Font>,
    by_name: &'a mut HashMap<String, FontId>,
    icons: &'a IconTable,
    colors: &'a ColorTable,
    source: Option<&'a dyn FontSource>,
    icon_fonts: &'a mut HashMap<String, Option<FontId>>,
    unresolved: &'a mut u64,
}

impl TagResolver for Resolver<'_> {
    fn icon(&mut self, name: &str) -> Option<(FontId, u32)> {
        // re: 0x086cc5c2 UIManager::AcquirGameplayIconFontAndIndexByName (mapping row, then the font file)
        let Some((stem, code)) = self.icons.get(name) else {
            *self.unresolved += 1;
            return None;
        };
        let stem = stem.to_string();
        let id = match self.icon_fonts.get(&stem) {
            Some(id) => *id,
            None => {
                let loaded = self.source.and_then(|s| s.ggxml(&stem)).and_then(|text| {
                    match Font::parse(&stem, &text) {
                        Ok(f) => {
                            let id = FontId(self.fonts.len() as u32);
                            self.fonts.push(f);
                            self.by_name.entry(stem.clone()).or_insert(id);
                            Some(id)
                        }
                        Err(e) => {
                            tracing::warn!("icon font {stem}: {e}");
                            None
                        }
                    }
                });
                self.icon_fonts.insert(stem, loaded);
                loaded
            }
        };
        match id {
            Some(id) => Some((id, code)),
            None => {
                *self.unresolved += 1;
                None
            }
        }
    }

    fn color(&mut self, name: &str) -> Option<Rgba> {
        let c = self.colors.get(name);
        if c.is_none() {
            *self.unresolved += 1;
        }
        c
    }
}
