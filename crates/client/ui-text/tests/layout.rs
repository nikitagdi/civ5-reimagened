//! Layout rules on a synthetic font: letters advance 10, space 4, `.` 4, spacing 0, ascent 12 (size 18).

use draw_list::{TextureId, UiDrawList};
use glam::Vec2;
use ui_text::{
    ColorTable, FontSource, FontTextures, IconTable, TextParams, TextSystem, TextureRef,
};

fn font_xml(name: &str, ascent: i32, ascadj: i32, extra_glyphs: &str) -> String {
    let mut g = String::new();
    g.push_str("<glyph ch=\"32\" u=\"0\" v=\"0\" width=\"0\" height=\"0\" a=\"0\" b=\"4\" c=\"0\" originy=\"0\"/>");
    for ch in ('A'..='Z').chain('a'..='z').chain('0'..='9') {
        g.push_str(&format!(
            "<glyph ch=\"{}\" u=\"{}\" v=\"8\" width=\"8\" height=\"10\" a=\"1\" b=\"9\" c=\"0\" originy=\"10\"/>",
            ch as u32,
            (ch as u32 % 20) * 10
        ));
    }
    for ch in ['.', '*'] {
        g.push_str(&format!(
            "<glyph ch=\"{}\" u=\"0\" v=\"40\" width=\"2\" height=\"2\" a=\"0\" b=\"4\" c=\"0\" originy=\"2\"/>",
            ch as u32
        ));
    }
    g.push_str(extra_glyphs);
    format!(
        r#"<glyphgen>
 <textures width="256" height="128">
  <texture name="" src="{name}.dds" allowcolor="1"/>
  <texture name="{name}Base" src="{name}Base.dds" allowcolor="1"/>
  <texture name="{name}Shadow" src="{name}Shadow.dds" allowcolor="1"/>
  <texture name="{name}Soft" src="{name}Soft.dds" allowcolor="1"/>
 </textures>
 <styles count="3">
  <style name=""><layers><layer tex=""/></layers></style>
  <style name="Shadow"><layers><layer tex="{name}Base"/><layer tex="{name}Shadow"/></layers></style>
  <style name="SoftShadow"><layers><layer tex="{name}Base"/><layer tex="{name}Shadow"/><layer tex="{name}Soft"/></layers></style>
 </styles>
 <glyphs spacing="0" ascent="{ascent}" ascadj="{ascadj}" descent="4">{g}</glyphs>
</glyphgen>"#
    )
}

struct Icons;
impl FontSource for Icons {
    fn ggxml(&self, stem: &str) -> Option<String> {
        (stem == "TestIcons").then(|| {
            r#"<glyphgen><textures width="64" height="64"><texture name="" src="TestIcons.dds" allowcolor="1"/></textures>
<styles><style name=""><layers><layer tex=""/></layers></style></styles>
<glyphs spacing="0" ascent="0" ascadj="-6" descent="0">
<glyph ch="1" u="0" v="0" width="22" height="22" a="0" b="22" c="0" originy="22"/>
</glyphs></glyphgen>"#
                .to_string()
        })
    }
}

fn system() -> (TextSystem, ui_text::FontId) {
    let mut icons = IconTable::new();
    icons.add_texture("T", "TestIcons");
    icons.add_icon("ICON_GOLD", 1, "T");
    icons.add_icon("ICON_MISSING_FILE", 1, "T2");
    let mut colors = ColorTable::new();
    colors.add("COLOR_RED", [1.0, 0.0, 0.0, 1.0]);
    let mut ts = TextSystem::new(icons, colors);
    ts.set_font_source(Box::new(Icons));
    let f = ts.load_font("Test", &font_xml("Test", 14, 2, "")).unwrap();
    (ts, f)
}

const LEADING: i32 = 21;
const ASCENT: i32 = 12;

#[test]
fn measure_counts_steps_and_obscures() {
    let (ts, f) = system();
    assert_eq!(ts.measure("AAA", f, false), 30);
    assert_eq!(ts.measure("a b", f, false), 24);
    // no glyph -> space (4)
    assert_eq!(ts.measure("\u{416}", f, false), 4);
    // obscured: every character is '*' (4)
    assert_eq!(ts.measure("abcdef", f, true), 24);
}

/// A font that has Cyrillic glyphs (the asset pipeline adds them to Tw Cen MT, SIMP-exe-716) draws a Russian
/// line, wraps it at its spaces and measures it by the glyph advances, in the UTF-8 text a chat line arrives in.
#[test]
fn cyrillic_glyphs_of_a_font_are_drawn() {
    let mut cyr = String::new();
    // "привет мир": п р и в е т, м и р; every letter advances 10 (a + b)
    for ch in "приветмир".chars() {
        cyr.push_str(&format!(
            "<glyph ch=\"{}\" u=\"{}\" v=\"60\" width=\"8\" height=\"10\" a=\"1\" b=\"9\" c=\"0\" originy=\"10\"/>",
            ch as u32,
            (ch as u32 % 20) * 10
        ));
    }
    let (mut ts, _) = system();
    let f = ts.load_font("Cyr", &font_xml("Cyr", 14, 2, &cyr)).unwrap();
    // 9 letters of 10 and one space of 4
    assert_eq!(ts.measure("привет мир", f, false), 9 * 10 + 4);
    let l = ts.layout(&TextParams::new("привет мир", f));
    assert_eq!(l.quads.len(), 9, "one quad per letter, none for the space");
    assert_eq!(l.size[0], 9 * 10 + 4);
    let mut p = TextParams::new("привет мир", f);
    p.wrap_width = 60;
    assert_eq!(ts.layout(&p).lines.len(), 2);
    // A letter the font lacks is still a space, as before.
    assert_eq!(ts.measure("ж", f, false), 4);
}

#[test]
fn single_line_size() {
    let (mut ts, f) = system();
    let l = ts.layout(&TextParams::new("abc", f));
    assert_eq!(l.size, [30, ASCENT]);
    assert_eq!(l.lines.len(), 1);
    assert!(!l.truncated);
    // 3 glyphs, one layer of the empty-name style
    assert_eq!(l.quads.len(), 3);
    assert_eq!(l.textures.len(), 1);
    assert_eq!(l.textures[0].src, "Test.dds");
}

#[test]
fn newline_tag_starts_a_line() {
    let (mut ts, f) = system();
    let l = ts.layout(&TextParams::new("ab[NEWLINE]c", f));
    assert_eq!(l.size, [20, ASCENT + LEADING]);
    assert_eq!(l.lines.len(), 2);
    assert_eq!(l.lines[1].y, LEADING);
    let mut p = TextParams::new("ab[NEWLINE]c", f);
    p.leading_offset = -3;
    assert_eq!(ts.layout(&p).size[1], ASCENT + LEADING - 3);
    // the 'c' quad sits one line down
    let c = l.quads.last().unwrap();
    let a = &l.quads[0];
    assert_eq!(c.rect[1] - a.rect[1], LEADING as f32);
}

#[test]
fn wraps_at_spaces() {
    let (mut ts, f) = system();
    let mut p = TextParams::new("aaaa bbbb cccc", f);
    p.wrap_width = 50;
    let l = ts.layout(&p);
    assert_eq!(l.lines.len(), 3, "{:?}", l.lines);
    assert_eq!(
        l.size[0], 44,
        "the trailing space of a wrapped line stays on it"
    );
    assert_eq!(l.size[1], ASCENT + 2 * LEADING);
    // wide enough for everything: one line
    p.wrap_width = 200;
    assert_eq!(ts.layout(&p).lines.len(), 1);
    // no wrapping at -1
    p.wrap_width = -1;
    assert_eq!(ts.layout(&p).lines.len(), 1);
}

#[test]
fn a_word_that_reaches_the_width_wraps() {
    let (mut ts, f) = system();
    let mut p = TextParams::new("aaa bbb", f);
    // The second word ends at 34 + 30 = 64. The exe wraps when the word reaches the width (`<=`), so a
    // width of 64 breaks and 65 does not.
    p.wrap_width = 65;
    assert_eq!(ts.layout(&p).lines.len(), 1);
    p.wrap_width = 64;
    assert_eq!(ts.layout(&p).lines.len(), 2);
}

#[test]
fn a_word_wider_than_the_width_breaks_by_characters() {
    let (mut ts, f) = system();
    let mut p = TextParams::new("abcdefgh", f);
    p.wrap_width = 25;
    let l = ts.layout(&p);
    assert_eq!(l.lines.len(), 4);
    assert_eq!(l.size[0], 20);
}

#[test]
fn space_at_the_start_of_a_wrapped_line_is_dropped() {
    let (mut ts, f) = system();
    let mut p = TextParams::new("aaaaaaaa bb", f);
    p.wrap_width = 80;
    let l = ts.layout(&p);
    assert_eq!(l.lines.len(), 2);
    // 8 letters, then the second line: only "bb" (the space was swallowed by the break)
    let second_line_quads = l.quads.iter().filter(|q| q.rect[1] > 10.0).count();
    assert_eq!(second_line_quads, 2);
}

#[test]
fn newline_tag_and_wrap_together() {
    let (mut ts, f) = system();
    let mut p = TextParams::new("aaaa bbbb[NEWLINE]cc", f);
    p.wrap_width = 50;
    let l = ts.layout(&p);
    assert_eq!(l.lines.len(), 3);
}

#[test]
fn truncation_adds_the_trail() {
    let (mut ts, f) = system();
    let mut p = TextParams::new("abcdefgh", f);
    p.truncate_width = 45;
    let l = ts.layout(&p);
    assert!(l.truncated);
    // abc (30) + "..." (12)
    assert_eq!(l.size[0], 42);
    assert_eq!(l.quads.len(), 3 + 3);
    // fits exactly: no trail
    p.truncate_width = 80;
    let l = ts.layout(&p);
    assert!(!l.truncated);
    assert_eq!(l.quads.len(), 8);
    // a truncate width turns wrapping off
    let mut p = TextParams::new("aaaa bbbb cccc", f);
    p.truncate_width = 400;
    p.wrap_width = 50;
    assert_eq!(ts.layout(&p).lines.len(), 1);
}

#[test]
fn style_layers_draw_back_to_front() {
    let (mut ts, f) = system();
    let style = ts.style_index(f, "SoftShadow");
    assert_eq!(style, 2);
    let mut p = TextParams::new("a", f);
    p.style = style;
    p.colors = [[255, 255, 255, 255], [0, 0, 0, 200], [1, 2, 3, 4]];
    let l = ts.layout(&p);
    let layers: Vec<u8> = l.quads.iter().map(|q| q.layer).collect();
    assert_eq!(layers, [2, 1, 0]);
    let colors: Vec<[u8; 4]> = l.quads.iter().map(|q| q.color).collect();
    assert_eq!(colors, [[1, 2, 3, 4], [0, 0, 0, 200], [255; 4]]);
    let names: Vec<&str> = l
        .quads
        .iter()
        .map(|q| l.textures[q.texture as usize].texture.as_str())
        .collect();
    assert_eq!(names, ["TestSoft", "TestShadow", "TestBase"]);
}

#[test]
fn color_tags_recolor_the_fill_layer() {
    let (mut ts, f) = system();
    let p = TextParams::new("a[COLOR_RED]b[ENDCOLOR]c", f);
    let l = ts.layout(&p);
    let colors: Vec<[u8; 4]> = l.quads.iter().map(|q| q.color).collect();
    assert_eq!(colors, [[255; 4], [255, 0, 0, 255], [255; 4]]);
    // unknown colors and tags vanish
    let l = ts.layout(&TextParams::new("a[COLOR_NOPE]b[NOPE]c", f));
    assert_eq!(l.quads.len(), 3);
    assert!(ts.stats().unresolved_tags >= 1);
}

#[test]
fn icons_load_lazily_and_ride_the_ascent() {
    let (mut ts, f) = system();
    assert!(ts.font("TestIcons").is_none());
    let l = ts.layout(&TextParams::new("a[ICON_GOLD]b", f));
    assert!(ts.font("TestIcons").is_some());
    let icon = l.quads.iter().find(|q| q.icon).unwrap();
    // pen is 10 after 'a'; glyph rect x 0..22, y -22+... with the main ascent added
    assert_eq!(icon.rect[0], 10.0);
    assert_eq!(icon.rect[2], 32.0);
    // icon font ascent 0 - (-6) = 6 -> y0 = -22 + 6? the icon glyph uses its own ascent (6) plus the main one (12)
    assert_eq!(icon.rect[1], -22.0 + 6.0 + ASCENT as f32);
    assert_eq!(icon.color, [255; 4]);
    // icons come after text; 'b' starts after the icon (10 + 22)
    assert!(l.quads.last().unwrap().icon);
    let b = l
        .quads
        .iter()
        .find(|q| !q.icon && q.rect[0] > 30.0)
        .unwrap();
    assert_eq!(b.rect[0], 32.0 + 1.0);
    // a missing icon file: the tag disappears
    let l = ts.layout(&TextParams::new("a[ICON_MISSING_FILE]b", f));
    assert_eq!(l.quads.len(), 2);
    let l = ts.layout(&TextParams::new("a[ICON_UNKNOWN]b", f));
    assert_eq!(l.quads.len(), 2);
}

#[test]
fn icon_widths_count_for_wrapping() {
    let (mut ts, f) = system();
    let mut p = TextParams::new("aa[ICON_GOLD]bb", f);
    // 20 + 22 = 42, then 'b' would end at 52
    p.wrap_width = 45;
    let l = ts.layout(&p);
    assert!(l.lines.len() >= 2);
}

#[test]
fn parse_mode_off_shows_tags_as_text() {
    let (mut ts, f) = system();
    let mut p = TextParams::new("a[b]", f);
    p.parse = false;
    // '[' and ']' have no glyph -> spaces (no quad), 'a' and 'b' stay
    let l = ts.layout(&p);
    assert_eq!(l.quads.len(), 2);
    assert_eq!(l.size[0], 10 + 4 + 10 + 4);
}

#[test]
fn obscured_text_is_stars() {
    let (mut ts, f) = system();
    let mut p = TextParams::new("secret", f);
    p.obscured = true;
    let l = ts.layout(&p);
    assert_eq!(l.size[0], 24);
}

#[test]
fn layouts_are_cached() {
    let (mut ts, f) = system();
    let p = TextParams::new("cache me", f);
    let a = ts.layout(&p);
    let b = ts.layout(&p);
    assert!(std::sync::Arc::ptr_eq(&a, &b));
    assert_eq!(ts.stats().layouts, 1);
    assert_eq!(ts.stats().cache_hits, 1);
}

struct Map;
impl FontTextures for Map {
    fn texture_id(&self, t: &TextureRef) -> Option<TextureId> {
        match t.texture.as_str() {
            "" => Some(TextureId(5)),
            "TestBase" => Some(TextureId(6)),
            _ => None,
        }
    }
}

#[test]
fn emit_scales_alpha_and_offsets() {
    let (mut ts, f) = system();
    let mut p = TextParams::new("a", f);
    p.colors[0] = [10, 20, 30, 200];
    let l = ts.layout(&p);
    let mut dl = UiDrawList::new();
    l.emit(&mut dl, Vec2::new(100.0, 50.0), 0.5, None, &Map);
    assert_eq!(dl.vertices().len(), 4);
    assert_eq!(dl.vertices()[0].color, [10, 20, 30, 100]);
    assert_eq!(dl.vertices()[0].pos, [101.0, 50.0 + l.quads[0].rect[1]]);
    assert_eq!(dl.commands()[0].texture, TextureId(5));
    // a style whose textures the map does not know draws nothing
    let mut p = TextParams::new("a", f);
    p.style = 2;
    let l = ts.layout(&p);
    let mut dl = UiDrawList::new();
    l.emit(&mut dl, Vec2::ZERO, 1.0, None, &Map);
    assert_eq!(dl.vertices().len(), 4, "only the base layer has a texture");
}

#[test]
fn space_and_tab_tags_become_white_space() {
    let (mut ts, f) = system();
    let plain = ts.layout(&TextParams::new("a b", f));
    let tagged = ts.layout(&TextParams::new("a[SPACE]b", f));
    assert_eq!(tagged.size, plain.size);
    assert_eq!(tagged.quads.len(), plain.quads.len());
    // a leading [SPACE] pushes the text right (the XML loader trims a plain leading space)
    let lead = ts.layout(&TextParams::new("[SPACE]ab", f));
    let bare = ts.layout(&TextParams::new("ab", f));
    assert_eq!(
        lead.quads[0].rect[0] - bare.quads[0].rect[0],
        ts.measure(" ", f, false) as f32
    );
    // [TAB] is TAB_SPACES spaces, and with parsing off the tags stay text
    let tab = ts.layout(&TextParams::new("[TAB]a", f));
    let spaces = ts.layout(&TextParams::new("    a", f));
    assert_eq!(tab.quads[0].rect[0], spaces.quads[0].rect[0]);
    let mut raw = TextParams::new("[TAB]a", f);
    raw.parse = false;
    assert!(ts.layout(&raw).quads.len() > 2);
}
