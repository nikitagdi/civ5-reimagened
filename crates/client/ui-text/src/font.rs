//! `.ggxml` bitmap fonts: a port of `FGXFont::LoadFontFromXML` and its helpers (Civ5XP 0x088bbd84).
//!
//! SIMPLIFIED(SIMP-txt-1): only the installed Tw Cen MT fonts exist, with no Cyrillic glyphs; ru_RU text
//! composes in `loc` but its characters draw as spaces. The engine takes any `.ggxml`, so a font with the
//! glyphs needs no code change: the asset pipeline adds them to the set (SIMP-exe-716), this parser reads them
//! like any other glyph.

use std::collections::HashMap;

/// A font that does not load. The exe shows a message box and gives up; here it is an error value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("font {font}: {message}")]
pub struct FontError {
    pub font: String,
    pub message: String,
}

/// `FGXFontTexture`: the texture name, its file and whether it takes the text color.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontTexture {
    pub name: String,
    pub src: String,
    pub allow_color: bool,
}

/// `FGXFontStyle`: up to three layers, each an index into the font's textures, `-1` when unused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontStyle {
    pub name: String,
    pub layers: [i32; 3],
}

/// `FGXFontGlyph`: texture rectangle, pixel rectangle relative to the pen, and the pen advance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Glyph {
    /// `u0, v0, u1, v1` (0..1).
    pub uv: [f32; 4],
    /// `x0, y0, x1, y1` in pixels, y down from the line top, relative to the pen.
    pub rect: [f32; 4],
    /// `a + b + c`.
    pub advance: f32,
}

impl Glyph {
    /// The pen step: the advance as the exe's `(int)` conversion.
    pub fn step(&self) -> i32 {
        self.advance as i32
    }
}

/// A loaded font.
#[derive(Debug, Clone)]
pub struct Font {
    pub name: String,
    ascent: i32,
    descent: i32,
    size: u32,
    /// Texture size shared by all textures of the font.
    pub texture_size: (u32, u32),
    textures: Vec<FontTexture>,
    styles: Vec<FontStyle>,
    glyphs: Vec<Glyph>,
    index: HashMap<u32, usize>,
}

fn attr_i32(node: roxmltree::Node, name: &str) -> Option<i32> {
    node.attribute(name)?.trim().parse::<i32>().ok()
}

impl Font {
    /// Parses `.ggxml` text.
    ///
    /// Rules (re: 0x088bbd84 `FGXFont::LoadFontFromXML`): `<glyphs spacing descent ascent [ascadj]>` are
    /// required; the font size is `ascent + descent` and the working ascent is `ascent - ascadj` (`- 2`
    /// without `ascadj`); `<textures width height>` with `<texture>` entries, `<styles>` with `<style>`
    /// entries and `<glyphs>` with `<glyph>` entries are all required.
    pub fn parse(name: &str, ggxml: &str) -> Result<Font, FontError> {
        let err = |m: &str| FontError {
            font: name.to_string(),
            message: m.to_string(),
        };
        let doc = roxmltree::Document::parse(ggxml.trim_start_matches('\u{feff}'))
            .map_err(|e| err(&format!("bad xml: {e}")))?;
        let root = doc.root_element();
        let child = |n: &str| {
            root.children()
                .find(|c| c.is_element() && c.tag_name().name() == n)
        };
        let textures_el = child("textures");
        let styles_el = child("styles");
        let glyphs_el = child("glyphs").ok_or_else(|| err("no glyphs"))?;

        let spacing =
            attr_i32(glyphs_el, "spacing").ok_or_else(|| err("No font padding specified"))?;
        let descent =
            attr_i32(glyphs_el, "descent").ok_or_else(|| err("No font descent specified"))?;
        let ascent_raw =
            attr_i32(glyphs_el, "ascent").ok_or_else(|| err("No font ascent specified"))?;
        let size = (descent + ascent_raw) as u32;
        let ascent = ascent_raw - attr_i32(glyphs_el, "ascadj").unwrap_or(2);

        let textures_el = textures_el.ok_or_else(|| err("No texture entries"))?;
        let tex_h =
            attr_i32(textures_el, "height").ok_or_else(|| err("No texture height specified"))?;
        let tex_w =
            attr_i32(textures_el, "width").ok_or_else(|| err("No texture width specified"))?;
        if tex_w <= 0 || tex_h <= 0 {
            return Err(err("texture size must be positive"));
        }
        let textures: Vec<FontTexture> = textures_el
            .children()
            .filter(|c| c.is_element() && c.tag_name().name() == "texture")
            .map(|t| FontTexture {
                name: t.attribute("name").unwrap_or("").to_string(),
                src: t.attribute("src").unwrap_or("").to_string(),
                allow_color: t
                    .attribute("allowcolor")
                    .is_some_and(|v| v.trim() != "0" && !v.trim().eq_ignore_ascii_case("false")),
            })
            .collect();
        if textures.is_empty() {
            return Err(err("No texture entries"));
        }

        let styles_el = styles_el.ok_or_else(|| err("No style entries"))?;
        let mut styles = Vec::new();
        for s in styles_el
            .children()
            .filter(|c| c.is_element() && c.tag_name().name() == "style")
        {
            let Some(sname) = s.attribute("name") else {
                // "Font style must have a name (even if it's empty)": the style is skipped
                continue;
            };
            let layers_el = s
                .children()
                .find(|c| c.is_element() && c.tag_name().name() == "layers")
                .ok_or_else(|| err("No layers entry"))?;
            let mut layers = [-1i32; 3];
            let mut count = 0;
            let mut ok = true;
            for l in layers_el
                .children()
                .filter(|c| c.is_element() && c.tag_name().name() == "layer")
            {
                let tex = l.attribute("tex").unwrap_or("");
                // the exe keeps the last texture with that name
                match textures.iter().rposition(|t| t.name == tex) {
                    Some(i) if count < 3 => {
                        layers[count] = i as i32;
                        count += 1;
                    }
                    Some(_) => {}
                    None => {
                        // "Layer texture not found": the style is dropped
                        ok = false;
                        break;
                    }
                }
            }
            if count == 0 && ok {
                return Err(err("No layer entries"));
            }
            if ok {
                styles.push(FontStyle {
                    name: sname.to_string(),
                    layers,
                });
            }
        }
        if styles.is_empty() {
            return Err(err("No style entries"));
        }

        let mut glyphs = Vec::new();
        let mut index = HashMap::new();
        for g in glyphs_el
            .children()
            .filter(|c| c.is_element() && c.tag_name().name() == "glyph")
        {
            let ch = g
                .attribute("ch")
                .and_then(|v| v.trim().parse::<u32>().ok())
                .ok_or_else(|| err("glyph without ch"))?;
            let v = |n: &str| attr_i32(g, n).unwrap_or(0);
            let (u, vv, mut width, mut height) = (v("u"), v("v"), v("width"), v("height"));
            let (a, b, c, origin_y) = (v("a"), v("b"), v("c"), v("originy"));
            let mut pad = spacing;
            if ch == 0x20 {
                // re: 0x088bd080 FGXFont::AddGlyphFromXMLElement: the space has no size and no padding
                width = 0;
                height = 0;
                pad = 0;
            }
            let (tw, th, pad_f) = (tex_w as f32, tex_h as f32, pad as f32);
            let glyph = Glyph {
                uv: [
                    (u - pad) as f32 / tw,
                    (vv - pad) as f32 / th,
                    (u + pad + width) as f32 / tw,
                    (vv + pad + height) as f32 / th,
                ],
                rect: [
                    a as f32 - pad_f,
                    (-origin_y) as f32 - pad_f + ascent as f32,
                    (width + a) as f32 + pad_f,
                    (height - origin_y) as f32 + pad_f + ascent as f32,
                ],
                advance: (a + b + c) as f32,
            };
            index.insert(ch, glyphs.len());
            glyphs.push(glyph);
        }
        if glyphs.is_empty() {
            return Err(err("No glyph entries"));
        }
        Ok(Font {
            name: name.to_string(),
            ascent,
            descent,
            size,
            texture_size: (tex_w as u32, tex_h as u32),
            textures,
            styles,
            glyphs,
            index,
        })
    }

    /// `GetAscent`: the working ascent, which is also the height of a one-line text.
    pub fn ascent(&self) -> i32 {
        self.ascent
    }

    pub fn descent(&self) -> i32 {
        self.descent
    }

    /// `GetFontSize`: `ascent + descent` of the XML.
    pub fn size(&self) -> u32 {
        self.size
    }

    /// `GetLeading`: the font size times 1.2, truncated.
    pub fn leading(&self) -> i32 {
        // re: 0x088bd5a8 FGXFont::GetLeading (the constant at 0x0914b920 is 1.2f)
        (self.size as f32 * 1.2f32) as i32
    }

    /// `HasGlyph`.
    pub fn has_glyph(&self, ch: u32) -> bool {
        self.index.contains_key(&ch)
    }

    /// `GetGlyphFromIntValue`: a missing character gives the first glyph of the file.
    pub fn glyph(&self, ch: u32) -> &Glyph {
        match self.index.get(&ch) {
            Some(&i) => &self.glyphs[i],
            None => &self.glyphs[0],
        }
    }

    /// `GetStyleIndex`: the first style with exactly that name, 0 if there is none.
    pub fn style_index(&self, name: &str) -> u8 {
        // re: 0x088bd686 FGXFont::GetStyleIndex
        self.styles.iter().position(|s| s.name == name).unwrap_or(0) as u8
    }

    pub fn style(&self, i: usize) -> Option<&FontStyle> {
        self.styles.get(i)
    }

    pub fn styles(&self) -> &[FontStyle] {
        &self.styles
    }

    pub fn textures(&self) -> &[FontTexture] {
        &self.textures
    }

    pub fn glyph_count(&self) -> usize {
        self.glyphs.len()
    }

    /// Characters the font has, ascending.
    pub fn chars(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.index.keys().copied().collect();
        v.sort_unstable();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0"?>
<glyphgen>
 <textures width="100" height="50">
  <texture name="" src="F.dds" allowcolor="1" />
  <texture name="FBase" src="FBase.dds" allowcolor="1" />
 </textures>
 <styles count="2">
  <style name=""><layers count="1"><layer tex="" /></layers></style>
  <style name="Base"><layers count="2"><layer tex="FBase" /><layer tex="" /></layers></style>
 </styles>
 <glyphs ascadj="2" spacing="4" height="16" ascent="14" descent="4">
  <glyph ch="32" u="4" v="4" width="0" height="0" a="0" b="4" c="0" originx="0" originy="0" />
  <glyph ch="65" u="20" v="10" width="10" height="12" a="1" b="9" c="0" originx="0" originy="12" />
 </glyphs>
</glyphgen>"#;

    #[test]
    fn metrics_and_glyph_geometry() {
        let f = Font::parse("T", XML).unwrap();
        assert_eq!(
            (f.ascent(), f.descent(), f.size(), f.leading()),
            (12, 4, 18, 21)
        );
        let a = f.glyph(65);
        assert_eq!(a.advance, 10.0);
        assert_eq!(a.step(), 10);
        // x: a - pad .. width + a + pad ; y: -originy - pad + ascent .. height - originy + pad + ascent
        assert_eq!(a.rect, [-3.0, -4.0, 15.0, 16.0]);
        assert_eq!(a.uv, [16.0 / 100.0, 6.0 / 50.0, 34.0 / 100.0, 26.0 / 50.0]);
        let sp = f.glyph(32);
        assert_eq!(sp.rect, [0.0, 12.0, 0.0, 12.0]);
        assert_eq!(sp.advance, 4.0);
    }

    #[test]
    fn missing_glyph_is_the_first_one() {
        let f = Font::parse("T", XML).unwrap();
        assert!(!f.has_glyph(66));
        assert_eq!(f.glyph(66), f.glyph(32));
        assert!(f.has_glyph(65));
    }

    #[test]
    fn styles() {
        let f = Font::parse("T", XML).unwrap();
        assert_eq!(f.style_index("Base"), 1);
        assert_eq!(f.style_index("Nope"), 0);
        assert_eq!(f.style(1).unwrap().layers, [1, 0, -1]);
        assert_eq!(f.style(0).unwrap().layers, [0, -1, -1]);
    }

    #[test]
    fn default_ascent_adjust_is_two() {
        let f = Font::parse("T", &XML.replace(" ascadj=\"2\"", "")).unwrap();
        assert_eq!(f.ascent(), 12);
        let f = Font::parse("T", &XML.replace("ascadj=\"2\"", "ascadj=\"-6\"")).unwrap();
        assert_eq!(f.ascent(), 20);
    }

    #[test]
    fn missing_required_parts_are_errors() {
        assert!(Font::parse("T", &XML.replace("spacing=\"4\"", "")).is_err());
        assert!(Font::parse("T", &XML.replace("descent=\"4\"", "")).is_err());
        assert!(Font::parse("T", &XML.replace("ascent=\"14\"", "")).is_err());
        assert!(Font::parse("T", &XML.replace("height=\"50\"", "")).is_err());
        assert!(Font::parse("T", "<glyphgen>").is_err());
        assert!(Font::parse("T", "<glyphgen/>").is_err());
    }
}
