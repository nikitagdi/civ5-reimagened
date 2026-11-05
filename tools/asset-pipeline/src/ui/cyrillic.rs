//! Cyrillic glyphs for the Tw Cen MT bitmap fonts.
//!
//! SIMPLIFIED(SIMP-exe-716): the shipped `TwCenMT14..24` atlases cover codes 32-345 only, so every Russian letter
//! of a chat line, a nickname or a city name drew as a space (`Font::glyph` falls back to the first glyph). The
//! installed game has no other font. Here the pipeline appends the missing letters (and the other Slavic ones) to
//! each Tw Cen MT font: the glyphs are rasterised from Ubuntu Light (Ubuntu Font Licence 1.0, see
//! `third_party/NOTICE-ui-cyrillic-font.md`) scaled so its capital H is as tall as the one of the Tw Cen MT size,
//! packed into the free rows below the existing glyphs of each atlas, and described by new `<glyph>` entries. The
//! layers other than the plain one (`Soft`, `Shadow`, `Stroke`) are derived from the coverage by a dilation or an
//! offset, not drawn by the game's glyph generator. Original: a Russian install ships its own fonts.

use std::collections::BTreeSet;

use ab_glyph::{Font as _, FontRef, PxScale, ScaleFont as _, point};
use asset_format::{TextureBlob, TextureFormat};

/// Ubuntu Light, the source of the glyph shapes.
const TTF: &[u8] = include_bytes!("../../fonts/Ubuntu-Light.ttf");

/// Letters added to a font that lacks them: Russian, Ukrainian, Belarusian, the numero sign.
pub fn added() -> Vec<u32> {
    let mut v: Vec<u32> = (0x410..=0x44f).collect();
    v.extend([
        0x401, 0x451, 0x404, 0x454, 0x406, 0x456, 0x407, 0x457, 0x40e, 0x45e, 0x490, 0x491, 0x2116,
    ]);
    v
}

/// The light weight of Ubuntu Light antialiases to grey next to the full-ink stems of the game's fonts; the
/// coverage is lifted by this exponent.
const INK_GAMMA: f32 = 0.6;

/// What a texture of a font holds: the plain glyph or one of the effect layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// `TwCenMT16.dds`, `TwCenMT16Base.dds`: the ink.
    Base,
    /// The ink with a faint halo.
    Soft,
    /// The ink with a drop shadow to the lower right.
    Shadow,
    /// The ink widened to an outline.
    Stroke,
}

/// The layer a texture of `font_stem` is, from its name (`twcenmt16soft.dds` is `Soft` of `twcenmt16`).
pub fn role_of(texture: &str, font_stem: &str) -> Role {
    let t = texture.to_ascii_lowercase();
    let t = t.strip_suffix(".dds").unwrap_or(&t);
    match t
        .strip_prefix(&font_stem.to_ascii_lowercase())
        .unwrap_or("")
    {
        "soft" => Role::Soft,
        "shadow" => Role::Shadow,
        "stroke" => Role::Stroke,
        _ => Role::Base,
    }
}

/// Whether a `.ggxml` file name is one of the Tw Cen MT text fonts.
pub fn is_twcen(file_name: &str) -> bool {
    let n = file_name.to_ascii_lowercase();
    n.strip_suffix(".ggxml")
        .is_some_and(|s| s.starts_with("twcenmt") && s[7..].chars().all(|c| c.is_ascii_digit()))
}

/// One glyph to add: its position in the atlas, its coverage and its metrics.
#[derive(Debug, Clone)]
struct NewGlyph {
    ch: u32,
    /// Top left of the glyph bitmap (the cell starts `pad` before it).
    u: u32,
    v: u32,
    w: u32,
    h: u32,
    a: i32,
    b: i32,
    c: i32,
    origin_y: i32,
    /// `w * h` coverage.
    cov: Vec<u8>,
}

/// The glyphs added to one font and the atlas size they need.
#[derive(Debug, Clone)]
pub struct FontPlan {
    pad: u32,
    tex_w: u32,
    tex_h: u32,
    /// Height of the atlases after the glyphs are in (at least `tex_h`).
    pub new_height: u32,
    glyphs: Vec<NewGlyph>,
    /// Ink was found in the rows the glyphs are put in (a font with no free rows grows its atlases).
    pub grown: bool,
}

impl FontPlan {
    /// Characters the plan adds.
    pub fn chars(&self) -> Vec<u32> {
        self.glyphs.iter().map(|g| g.ch).collect()
    }

    /// `<glyph>` entries for the `.ggxml`.
    fn entries(&self) -> String {
        let mut s = String::new();
        for g in &self.glyphs {
            s.push_str(&format!(
                "        <glyph ch=\"{}\" u=\"{}\" v=\"{}\" width=\"{}\" height=\"{}\" a=\"{}\" b=\"{}\" c=\"{}\" originx=\"0\" originy=\"{}\" />\n",
                g.ch, g.u, g.v, g.w, g.h, g.a, g.b, g.c, g.origin_y
            ));
        }
        s
    }

    /// The `.ggxml` text with the new glyphs and, when the atlas had to grow, its new height.
    pub fn ggxml(&self, src: &str) -> String {
        let mut out = src.to_owned();
        if let Some(at) = out.find("</glyphs>") {
            // The line break before the closing tag stays after the entries.
            let line_start = out[..at].rfind('\n').map_or(at, |p| p + 1);
            let at = if out[line_start..at].trim().is_empty() {
                line_start
            } else {
                at
            };
            out.insert_str(at, &self.entries());
        }
        out = replace_attr(&out, "<glyphs", "count", &|old| {
            old.parse::<usize>()
                .map_or(old.to_owned(), |n| (n + self.glyphs.len()).to_string())
        });
        if self.new_height != self.tex_h {
            out = replace_attr(&out, "<textures", "height", &|_| {
                self.new_height.to_string()
            });
        }
        out
    }

    /// One atlas of the font with the glyphs drawn in. The atlas must be the `R8` image the font has.
    pub fn apply(&self, role: Role, blob: TextureBlob) -> Result<TextureBlob, String> {
        if blob.format != TextureFormat::R8 || blob.mips.len() != 1 {
            return Err(format!(
                "Tw Cen MT atlas is {:?} with {} mips, not a single R8 image",
                blob.format,
                blob.mips.len()
            ));
        }
        if (blob.width, blob.height) != (self.tex_w, self.tex_h) {
            return Err(format!(
                "Tw Cen MT atlas is {}x{}, its .ggxml says {}x{}",
                blob.width, blob.height, self.tex_w, self.tex_h
            ));
        }
        let (w, new_h) = (self.tex_w as usize, self.new_height as usize);
        let mut px = blob.mips[0].clone();
        px.resize(w * new_h, 0);
        let pad = self.pad as usize;
        for g in &self.glyphs {
            let cell = layer(role, &g.cov, g.w as usize, g.h as usize, pad);
            let (cw, ch) = (g.w as usize + 2 * pad, g.h as usize + 2 * pad);
            let (x0, y0) = (g.u as usize - pad, g.v as usize - pad);
            for y in 0..ch {
                px[(y0 + y) * w + x0..(y0 + y) * w + x0 + cw]
                    .copy_from_slice(&cell[y * cw..(y + 1) * cw]);
            }
        }
        Ok(TextureBlob {
            height: self.new_height,
            mips: vec![px],
            ..blob
        })
    }
}

/// `tag ... attr="old" ...` in the first tag that starts with `tag`: `attr` takes `new(old)`.
fn replace_attr(src: &str, tag: &str, attr: &str, new: &dyn Fn(&str) -> String) -> String {
    let Some(start) = src.find(tag) else {
        return src.to_owned();
    };
    let end = src[start..].find('>').map_or(src.len(), |e| start + e);
    let head = &src[start..end];
    let key = format!(" {attr}=\"");
    let Some(k) = head.find(&key) else {
        return src.to_owned();
    };
    let vs = start + k + key.len();
    let Some(ve) = src[vs..].find('"') else {
        return src.to_owned();
    };
    format!(
        "{}{}{}",
        &src[..vs],
        new(&src[vs..vs + ve]),
        &src[vs + ve..]
    )
}

/// The cell (`w + 2 pad` by `h + 2 pad`) of a glyph in one layer, from its coverage.
fn layer(role: Role, cov: &[u8], w: usize, h: usize, pad: usize) -> Vec<u8> {
    let (cw, ch) = (w + 2 * pad, h + 2 * pad);
    let mut base = vec![0u8; cw * ch];
    for y in 0..h {
        base[(y + pad) * cw + pad..(y + pad) * cw + pad + w]
            .copy_from_slice(&cov[y * w..(y + 1) * w]);
    }
    let at = |x: i32, y: i32| -> f32 {
        if x < 0 || y < 0 || x >= cw as i32 || y >= ch as i32 {
            0.0
        } else {
            f32::from(base[y as usize * cw + x as usize])
        }
    };
    // The maximum of the coverage within a distance, each ring weighted.
    let reach = |x: i32, y: i32, rings: &[(f32, f32)]| -> f32 {
        let r = rings.iter().map(|r| r.0).fold(0.0f32, f32::max).ceil() as i32;
        let mut best = 0.0f32;
        for dy in -r..=r {
            for dx in -r..=r {
                let d = ((dx * dx + dy * dy) as f32).sqrt();
                if let Some(&(_, wgt)) = rings.iter().find(|&&(rad, _)| d <= rad) {
                    best = best.max(at(x + dx, y + dy) * wgt);
                }
            }
        }
        best
    };
    let mut out = vec![0u8; cw * ch];
    for y in 0..ch as i32 {
        for x in 0..cw as i32 {
            let own = at(x, y);
            let v = match role {
                Role::Base => own,
                Role::Soft => own.max(reach(x, y, &[(1.5, 0.45), (3.0, 0.2)])),
                Role::Shadow => own
                    .max(at(x - 1, y - 1) * 0.7)
                    .max(at(x - 1, y) * 0.4)
                    .max(at(x, y - 1) * 0.4),
                Role::Stroke => reach(x, y, &[(1.5, 1.0), (2.3, 0.6)]),
            };
            out[y as usize * cw + x as usize] = v.round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// What the plan needs to know of a `.ggxml`.
struct Layout {
    pad: u32,
    tex_w: u32,
    tex_h: u32,
    /// The capital H of the font: the height the added glyphs match.
    cap_height: Option<u32>,
    have: BTreeSet<u32>,
    /// Lowest row any existing glyph cell reaches.
    bottom: u32,
}

fn read_layout(ggxml: &str) -> Result<Layout, String> {
    let doc = roxmltree::Document::parse(ggxml.trim_start_matches('\u{feff}'))
        .map_err(|e| format!("bad xml: {e}"))?;
    let root = doc.root_element();
    let child = |n: &str| {
        root.children()
            .find(|c| c.is_element() && c.tag_name().name() == n)
    };
    let int = |n: roxmltree::Node, a: &str| -> Option<i32> { n.attribute(a)?.trim().parse().ok() };
    let glyphs = child("glyphs").ok_or("no glyphs")?;
    let textures = child("textures").ok_or("no textures")?;
    let pad = int(glyphs, "spacing").ok_or("no spacing")?.max(0) as u32;
    let tex_w = int(textures, "width").ok_or("no texture width")?.max(1) as u32;
    let tex_h = int(textures, "height").ok_or("no texture height")?.max(1) as u32;
    let mut have = BTreeSet::new();
    let (mut bottom, mut cap_height) = (0u32, None);
    for g in glyphs
        .children()
        .filter(|c| c.is_element() && c.tag_name().name() == "glyph")
    {
        let Some(ch) = g.attribute("ch").and_then(|v| v.trim().parse::<u32>().ok()) else {
            continue;
        };
        have.insert(ch);
        let (v, h) = (int(g, "v").unwrap_or(0), int(g, "height").unwrap_or(0));
        bottom = bottom.max((v + h + pad as i32).max(0) as u32);
        if ch == u32::from(b'H') {
            cap_height = Some(h.max(1) as u32);
        }
    }
    Ok(Layout {
        pad,
        tex_w,
        tex_h,
        cap_height,
        have,
        bottom,
    })
}

/// The glyphs to add to a Tw Cen MT font and where they go, or `None` when it has every one of them.
pub fn plan(ggxml: &str) -> Result<Option<FontPlan>, String> {
    let lay = read_layout(ggxml)?;
    let font = FontRef::try_from_slice(TTF).map_err(|e| format!("Ubuntu Light: {e}"))?;
    // The pixel size at which the capital H of Ubuntu Light is as tall as the font's own.
    let cap = lay.cap_height.unwrap_or(11) as f32;
    let h100 = font
        .outline_glyph(
            font.glyph_id('H')
                .with_scale_and_position(PxScale::from(100.0), point(0.0, 0.0)),
        )
        .map(|o| o.px_bounds().height())
        .filter(|h| *h > 0.0)
        .ok_or("Ubuntu Light has no H")?;
    let scale = PxScale::from(100.0 * cap / h100);
    let scaled = font.as_scaled(scale);

    struct Raster {
        ch: u32,
        w: u32,
        h: u32,
        a: i32,
        advance: i32,
        origin_y: i32,
        cov: Vec<u8>,
    }
    let mut rasters = Vec::new();
    for code in added() {
        if lay.have.contains(&code) {
            continue;
        }
        let Some(c) = char::from_u32(code) else {
            continue;
        };
        let id = font.glyph_id(c);
        if id.0 == 0 {
            continue;
        }
        let glyph = id.with_scale_and_position(scale, point(0.0, 0.0));
        let Some(outline) = font.outline_glyph(glyph) else {
            continue;
        };
        let b = outline.px_bounds();
        let (w, h) = (
            (b.width().ceil() as u32).max(1),
            (b.height().ceil() as u32).max(1),
        );
        let mut cov = vec![0u8; (w * h) as usize];
        outline.draw(|x, y, v| {
            if x < w && y < h {
                cov[(y * w + x) as usize] =
                    (v.clamp(0.0, 1.0).powf(INK_GAMMA) * 255.0).round() as u8;
            }
        });
        rasters.push(Raster {
            ch: code,
            w,
            h,
            a: b.min.x.floor() as i32,
            advance: scaled.h_advance(id).round() as i32,
            origin_y: (-b.min.y).round() as i32,
            cov,
        });
    }
    if rasters.is_empty() {
        return Ok(None);
    }

    // Shelf packing into the rows under the existing cells.
    let pad = lay.pad;
    let mut glyphs = Vec::new();
    let (mut x, mut y, mut row_h) = (0u32, lay.bottom + 1, 0u32);
    for r in rasters {
        let (cw, chh) = (r.w + 2 * pad, r.h + 2 * pad);
        if x + cw > lay.tex_w {
            x = 0;
            y += row_h;
            row_h = 0;
        }
        let b = r.w as i32;
        glyphs.push(NewGlyph {
            ch: r.ch,
            u: x + pad,
            v: y + pad,
            w: r.w,
            h: r.h,
            a: r.a,
            b,
            c: (r.advance - r.a - b).max(0),
            origin_y: r.origin_y,
            cov: r.cov,
        });
        x += cw;
        row_h = row_h.max(chh);
    }
    let needed = y + row_h;
    let new_height = needed.max(lay.tex_h);
    Ok(Some(FontPlan {
        pad,
        tex_w: lay.tex_w,
        tex_h: lay.tex_h,
        new_height,
        glyphs,
        grown: new_height > lay.tex_h,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use asset_format::TexDim;

    /// A small Tw Cen MT: space, H, one glyph with ink near the bottom of a 64 x 64 atlas.
    fn xml(h: u32) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" ?>
<glyphgen>
    <textures width="64" height="{h}">
        <texture name="" src="T.dds" allowcolor="1" />
        <texture name="TBase" src="TBase.dds" allowcolor="1" />
    </textures>
    <styles count="1">
        <style name=""><layers count="1"><layer tex="" /></layers></style>
    </styles>
    <glyphs fudgeadv="0" ascadj="2" spacing="3" count="3" height="16" ascent="14" descent="4">
        <glyph ch="32" u="3" v="3" width="0" height="0" a="0" b="4" c="0" originx="0" originy="0" />
        <glyph ch="72" u="10" v="10" width="9" height="11" a="0" b="9" c="0" originx="0" originy="11" />
        <glyph ch="1040" u="30" v="10" width="9" height="11" a="0" b="9" c="0" originx="0" originy="11" />
    </glyphs>
</glyphgen>"#
        )
    }

    fn blob(w: u32, h: u32) -> TextureBlob {
        TextureBlob {
            format: TextureFormat::R8,
            dim: TexDim::D2,
            srgb: false,
            width: w,
            height: h,
            depth: 1,
            mips: vec![vec![0u8; (w * h) as usize]],
        }
    }

    #[test]
    fn plan_adds_the_missing_letters_and_skips_the_present_ones() {
        let p = plan(&xml(64)).unwrap().unwrap();
        let chars = p.chars();
        assert!(!chars.contains(&0x410), "present in the .ggxml already");
        for c in (0x411..=0x44f).chain([0x401, 0x451, 0x2116]) {
            assert!(chars.contains(&c), "U+{c:04X}");
        }
        // 64 wide, ~16 px cells: the letters do not fit the 64 rows left: the atlas grows.
        assert!(p.grown && p.new_height > 64, "{}", p.new_height);
    }

    #[test]
    fn ggxml_gets_entries_count_and_height() {
        let p = plan(&xml(64)).unwrap().unwrap();
        let out = p.ggxml(&xml(64));
        let doc = roxmltree::Document::parse(&out).unwrap();
        let glyphs: Vec<_> = doc
            .descendants()
            .filter(|n| n.has_tag_name("glyph"))
            .collect();
        assert_eq!(glyphs.len(), 3 + p.chars().len());
        let g = doc
            .descendants()
            .find(|n| n.has_tag_name("glyphs"))
            .unwrap();
        assert_eq!(
            g.attribute("count").unwrap().parse::<usize>().unwrap(),
            3 + p.chars().len()
        );
        let t = doc
            .descendants()
            .find(|n| n.has_tag_name("textures"))
            .unwrap();
        assert_eq!(t.attribute("height").unwrap(), p.new_height.to_string());
        assert_eq!(t.attribute("width").unwrap(), "64");
        // Cap height: the added Ж is as tall as the H (11) within a pixel.
        let d = glyphs
            .iter()
            .find(|n| n.attribute("ch") == Some("1046"))
            .unwrap();
        let h: i32 = d.attribute("height").unwrap().parse().unwrap();
        assert!((10..=13).contains(&h), "height of Ж {h}");
        // The original text is otherwise untouched.
        assert!(out.contains("fudgeadv=\"0\""));
        assert!(out.contains(r#"<glyph ch="72" u="10""#));
    }

    #[test]
    fn a_tall_atlas_keeps_its_height() {
        let p = plan(&xml(2048)).unwrap().unwrap();
        assert!(!p.grown);
        assert_eq!(p.new_height, 2048);
        assert!(p.ggxml(&xml(2048)).contains(r#"height="2048""#));
    }

    #[test]
    fn cells_are_inside_the_atlas_and_do_not_overlap() {
        let p = plan(&xml(64)).unwrap().unwrap();
        let mut rects: Vec<(u32, u32, u32, u32)> = Vec::new();
        for g in &p.glyphs {
            let r = (
                g.u - p.pad,
                g.v - p.pad,
                g.u + g.w + p.pad,
                g.v + g.h + p.pad,
            );
            assert!(
                r.2 <= p.tex_w && r.3 <= p.new_height,
                "U+{:04X} {r:?}",
                g.ch
            );
            assert!(r.1 >= 10 + 11 + 3, "below the existing glyphs: {r:?}");
            for o in &rects {
                assert!(
                    r.2 <= o.0 || o.2 <= r.0 || r.3 <= o.1 || o.3 <= r.1,
                    "{r:?} {o:?}"
                );
            }
            rects.push(r);
        }
    }

    #[test]
    fn every_added_glyph_has_ink_in_every_layer_and_the_old_pixels_stay() {
        let p = plan(&xml(64)).unwrap().unwrap();
        let mut src = blob(64, 64);
        src.mips[0][5 * 64 + 5] = 77;
        for role in [Role::Base, Role::Soft, Role::Shadow, Role::Stroke] {
            let out = p.apply(role, src.clone()).unwrap();
            assert_eq!((out.width, out.height), (64, p.new_height));
            assert_eq!(out.mips[0].len(), (64 * p.new_height) as usize);
            assert_eq!(out.mips[0][5 * 64 + 5], 77);
            for g in &p.glyphs {
                let ink = (g.v..g.v + g.h)
                    .flat_map(|y| (g.u..g.u + g.w).map(move |x| (x, y)))
                    .filter(|&(x, y)| out.mips[0][(y * 64 + x) as usize] > 0)
                    .count();
                assert!(ink > 0, "{role:?} U+{:04X}", g.ch);
            }
        }
        // The base layer is exactly the coverage, the outline is wider than it.
        let base = p.apply(Role::Base, src.clone()).unwrap();
        let stroke = p.apply(Role::Stroke, src).unwrap();
        let sum = |b: &TextureBlob| {
            b.mips[0][64 * 64..]
                .iter()
                .map(|&v| u64::from(v))
                .sum::<u64>()
        };
        assert!(sum(&stroke) > 2 * sum(&base));
    }

    #[test]
    fn a_mismatched_atlas_is_an_error() {
        let p = plan(&xml(64)).unwrap().unwrap();
        assert!(p.apply(Role::Base, blob(64, 32)).is_err());
        let mut rgba = blob(64, 64);
        rgba.format = TextureFormat::Rgba8;
        assert!(p.apply(Role::Base, rgba).is_err());
    }

    #[test]
    fn names_and_roles() {
        assert!(is_twcen("TwCenMT16.ggxml") && is_twcen("twcenmt24.ggxml"));
        assert!(!is_twcen("FontIcons.ggxml") && !is_twcen("TwCenMT16Base.ggxml"));
        assert_eq!(role_of("twcenmt16.dds", "TwCenMT16"), Role::Base);
        assert_eq!(role_of("twcenmt16base.dds", "TwCenMT16"), Role::Base);
        assert_eq!(role_of("twcenmt16soft.dds", "TwCenMT16"), Role::Soft);
        assert_eq!(role_of("TwCenMT16Shadow.dds", "TwCenMT16"), Role::Shadow);
        assert_eq!(role_of("twcenmt16stroke.dds", "TwCenMT16"), Role::Stroke);
    }

    #[test]
    fn added_list_is_the_russian_alphabet_and_friends() {
        let all = added();
        assert_eq!(all.len(), 77);
        assert_eq!(&all[..2], &[0x410, 0x411]);
        assert!(all.contains(&0x401) && all.contains(&0x451) && all.contains(&0x44f));
        let mut v = all.clone();
        v.sort_unstable();
        v.dedup();
        assert_eq!(v.len(), all.len());
    }

    /// The installed game: every Tw Cen MT size takes the letters in its own free rows (or grows), each with ink,
    /// a size near the Latin capitals and an advance a text can use.
    #[test]
    fn game_fonts_take_the_letters() {
        use std::path::PathBuf;
        let Some(game) = std::env::var_os("CIV5_GAME_DIR").map(PathBuf::from) else {
            eprintln!("SKIPPED: game files not found (set CIV5_GAME_DIR)");
            return;
        };
        let dir = game.join("Assets/UI/Fonts/Tw Cent MT");
        let mut n = 0;
        for size in [14, 16, 18, 20, 22, 24] {
            let src = std::fs::read_to_string(dir.join(format!("TwCenMT{size}.ggxml"))).unwrap();
            let lay = read_layout(&src).unwrap();
            let p = plan(&src).unwrap().unwrap();
            assert!(!lay.have.contains(&0x416), "the game font has no Ж");
            let cyr = p
                .chars()
                .iter()
                .filter(|&&c| (0x410..=0x44f).contains(&c))
                .count();
            assert_eq!(cyr, 64, "size {size}");
            let out = p.ggxml(&src);
            let doc = roxmltree::Document::parse(&out).unwrap();
            let h = doc
                .descendants()
                .find(|n| n.has_tag_name("glyph") && n.attribute("ch") == Some("1046"))
                .unwrap_or_else(|| panic!("Ж at size {size}"));
            let gh: u32 = h.attribute("height").unwrap().parse().unwrap();
            let cap = lay.cap_height.unwrap();
            assert!(
                gh + 1 >= cap && gh <= cap + 3,
                "size {size}: Ж {gh} vs H {cap}"
            );
            for g in &p.glyphs {
                let adv = g.a + g.b + g.c;
                assert!(
                    (1..=(size * 2)).contains(&adv),
                    "size {size} U+{:04X} adv {adv}",
                    g.ch
                );
            }
            eprintln!(
                "TwCenMT{size}: +{} glyphs, atlas {}x{} -> {}x{}",
                p.glyphs.len(),
                p.tex_w,
                p.tex_h,
                p.tex_w,
                p.new_height
            );
            n += 1;
        }
        assert_eq!(n, 6);
    }
}
