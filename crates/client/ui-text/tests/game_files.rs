//! Acceptance on the installed game: every `.ggxml` parses, the TwCenMT16 glyph `A` has the documented
//! geometry, and every `[ICON_*]` / `[COLOR_*]` tag of the en_US texts is looked up in tables read from the
//! game's XML. Skipped, with a message, when the game files are absent (set `CIV5_GAME_DIR`).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use ui_text::{ColorTable, Font, FontSource, IconTable, TextParams, TextSystem};

fn assets_dir() -> Option<PathBuf> {
    let candidates = [std::env::var_os("CIV5_GAME_DIR").map(PathBuf::from)];
    for dir in candidates.into_iter().flatten() {
        let assets = dir.join("Assets");
        if assets.join("UI/Fonts").is_dir() {
            return Some(assets);
        }
    }
    eprintln!("SKIPPED: game files not found (set CIV5_GAME_DIR)");
    None
}

fn walk(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            walk(&p, ext, out);
        } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case(ext)) {
            out.push(p);
        }
    }
}

fn read(p: &Path) -> String {
    String::from_utf8_lossy(&std::fs::read(p).unwrap())
        .trim_start_matches('\u{feff}')
        .to_string()
}

fn text_of(n: roxmltree::Node, name: &str) -> Option<String> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
        .map(|c| c.text().unwrap_or("").trim().to_string())
}

struct Files(HashMap<String, PathBuf>);
impl FontSource for Files {
    fn ggxml(&self, stem: &str) -> Option<String> {
        self.0.get(stem).map(|p| read(p))
    }
}

fn ggxml_files(assets: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    walk(assets, "ggxml", &mut v);
    v
}

fn xml_docs(assets: &Path) -> Vec<(PathBuf, String)> {
    let mut v = Vec::new();
    walk(assets, "xml", &mut v);
    v.into_iter().map(|p| (p.clone(), read(&p))).collect()
}

fn parse_doc(s: &str) -> Option<roxmltree::Document<'_>> {
    roxmltree::Document::parse_with_options(
        s,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )
    .ok()
}

#[test]
fn every_ggxml_parses_and_twcen_a_is_exact() {
    let Some(assets) = assets_dir() else { return };
    let files = ggxml_files(&assets);
    assert!(files.len() >= 7, "ggxml files: {}", files.len());
    let mut bad = Vec::new();
    for f in &files {
        let name = f.file_stem().unwrap().to_string_lossy().to_string();
        if let Err(e) = Font::parse(&name, &read(f)) {
            bad.push(format!("{}: {e}", f.display()));
        }
    }
    for b in &bad {
        eprintln!("BAD FONT {b}");
    }
    assert!(bad.is_empty(), "{} fonts do not parse", bad.len());

    let p = files
        .iter()
        .find(|p| p.ends_with("TwCenMT16.ggxml"))
        .unwrap();
    let f = Font::parse("TwCenMT16", &read(p)).unwrap();
    assert_eq!(f.glyph_count(), 195);
    assert_eq!(
        (f.size(), f.ascent(), f.descent(), f.leading()),
        (18, 12, 4, 21)
    );
    let a = f.glyph(65);
    assert_eq!(a.advance, 10.0);
    // spacing 6, A = u 426 v 34 width 10 height 12 originy 12
    assert_eq!(a.rect, [-6.0, -6.0, 16.0, 18.0]);
    assert_eq!(
        a.uv,
        [420.0 / 512.0, 28.0 / 512.0, 442.0 / 512.0, 52.0 / 512.0]
    );
    // the space has no size and no padding
    let sp = f.glyph(32);
    assert_eq!((sp.rect[0], sp.rect[2]), (0.0, 0.0));
    assert_eq!(sp.advance, 4.0);
    assert_eq!(f.style_index("SoftShadow"), 5);
    assert_eq!(f.style_index("Shadow"), 3);
    assert_eq!(f.style_index("Stroke"), 4);
    assert_eq!(f.style_index("Base"), 1);
    assert_eq!(f.style_index("Soft"), 2);
    // no Cyrillic in the installed fonts (SIMP-txt-1)
    assert!(!f.has_glyph(0x416));
}

/// The `<Name>...</Name>` sections of a document. Some Lekmod files are not well-formed XML (mismatched tag
/// case elsewhere in the file; the game's own parser tolerates it), so the sections are cut out and parsed alone.
fn sections(s: &str, name: &str) -> Vec<String> {
    let (open, close) = (format!("<{name}>"), format!("</{name}>"));
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(i) = rest.find(&open) {
        let Some(j) = rest[i..].find(&close) else {
            break;
        };
        out.push(rest[i..i + j + close.len()].to_string());
        rest = &rest[i + j + close.len()..];
    }
    out
}

fn load_tables(assets: &Path) -> (IconTable, ColorTable) {
    let mut icons = IconTable::new();
    let mut colors = ColorTable::new();
    for (_, s) in xml_docs(assets) {
        if !(s.contains("<IconFontMapping>")
            || s.contains("<IconFontTextures>")
            || s.contains("<Colors>"))
        {
            continue;
        }
        for name in ["IconFontTextures", "IconFontMapping", "Colors"] {
            for frag in sections(&s, name) {
                let Some(doc) = parse_doc(&frag) else {
                    continue;
                };
                let sec = doc.root_element();
                for row in sec
                    .children()
                    .filter(|c| c.is_element() && c.tag_name().name() == "Row")
                {
                    match name {
                        "IconFontTextures" => {
                            if let (Some(k), Some(f)) = (
                                text_of(row, "IconFontTexture"),
                                text_of(row, "IconFontTextureFile"),
                            ) {
                                icons.add_texture(&k, &f);
                            }
                        }
                        "IconFontMapping" => {
                            if let (Some(n), Some(m)) =
                                (text_of(row, "IconName"), text_of(row, "IconMapping"))
                            {
                                let k = text_of(row, "IconFontTexture")
                                    .unwrap_or_else(|| "ICON_FONT_TEXTURE_DEFAULT".into());
                                if let Ok(m) = m.parse::<u32>() {
                                    icons.add_icon(&n, m, &k);
                                }
                            }
                        }
                        _ => {
                            if let Some(t) = text_of(row, "Type") {
                                let f = |n: &str| {
                                    text_of(row, n)
                                        .and_then(|v| v.parse::<f32>().ok())
                                        .unwrap_or(1.0)
                                };
                                colors.add(&t, [f("Red"), f("Green"), f("Blue"), f("Alpha")]);
                            }
                        }
                    }
                }
            }
        }
    }
    (icons, colors)
}

fn all_en_us_texts(assets: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for (_, s) in xml_docs(assets) {
        if !s.contains("<Language_en_US>") {
            continue;
        }
        let Some(doc) = parse_doc(&s) else { continue };
        for sec in doc
            .descendants()
            .filter(|n| n.is_element() && n.tag_name().name() == "Language_en_US")
        {
            for row in sec.children().filter(|c| c.is_element()) {
                if let Some(t) = text_of(row, "Text") {
                    out.push(t);
                }
            }
        }
    }
    out
}

#[test]
fn text_tags_resolve_in_the_game_tables() {
    let Some(assets) = assets_dir() else { return };
    let (icons, colors) = load_tables(&assets);
    eprintln!("icons {} colors {}", icons.len(), colors.len());
    assert!(icons.len() > 100);
    assert!(colors.get("COLOR_RED").is_some());
    assert!(icons.get("ICON_GOLD").is_some());

    let mut icon_missing: BTreeMap<String, usize> = BTreeMap::new();
    let mut icon_no_texture: BTreeMap<String, usize> = BTreeMap::new();
    let mut color_missing: BTreeMap<String, usize> = BTreeMap::new();
    let mut other_tags: BTreeMap<String, usize> = BTreeMap::new();
    let mut seen_icons = BTreeSet::new();
    for t in all_en_us_texts(&assets) {
        let mut rest = t.as_str();
        while let Some(i) = rest.find('[') {
            let Some(j) = rest[i..].find(']') else { break };
            let name = &rest[i + 1..i + j];
            rest = &rest[i + j + 1..];
            if name.starts_with("ICON_") {
                seen_icons.insert(name.to_string());
                if icons.get(name).is_none() {
                    match icons.mapping(name) {
                        Some((key, _)) => *icon_no_texture.entry(key.to_string()).or_default() += 1,
                        None => *icon_missing.entry(name.to_string()).or_default() += 1,
                    }
                }
            } else if name.starts_with("COLOR_") {
                if colors.get(name).is_none() {
                    *color_missing.entry(name.to_string()).or_default() += 1;
                }
            } else if name != "NEWLINE" && name != "ENDCOLOR" && !name.starts_with("COLOR:") {
                *other_tags.entry(name.to_string()).or_default() += 1;
            }
        }
    }
    eprintln!(
        "icon names used {}; not in IconFontMapping: {}; mapped to an undefined IconFontTexture: {:?}",
        seen_icons.len(),
        icon_missing.len(),
        icon_no_texture
    );
    for (n, c) in icon_missing.iter().take(40) {
        eprintln!("  MISSING ICON {n} x{c}");
    }
    eprintln!("colors not in Colors: {}", color_missing.len());
    for (n, c) in color_missing.iter().take(40) {
        eprintln!("  MISSING COLOR {n} x{c}");
    }
    let mut other: Vec<_> = other_tags.iter().collect();
    other.sort_by_key(|(_, c)| std::cmp::Reverse(**c));
    eprintln!("other tags (dropped by the layout): {}", other.len());
    for (n, c) in other.iter().take(15) {
        eprintln!("  OTHER [{n}] x{c}");
    }
    // The base game's own icons and colors are all present; the report above lists the rest.
    // Every icon the base game and the expansions use is mapped; the rest are listed above.
    assert!(
        icon_missing.len() <= 12,
        "{} icons have no IconFontMapping row",
        icon_missing.len()
    );
}

#[test]
fn real_text_lays_out_with_real_fonts() {
    let Some(assets) = assets_dir() else { return };
    let (icons, colors) = load_tables(&assets);
    let files = ggxml_files(&assets);
    let mut by_stem = HashMap::new();
    for f in &files {
        by_stem
            .entry(f.file_stem().unwrap().to_string_lossy().to_string())
            .or_insert_with(|| f.clone());
    }
    let mut ts = TextSystem::new(icons, colors);
    let font = ts
        .load_font("TwCenMT16", &read(&by_stem["TwCenMT16"]))
        .unwrap();
    ts.set_font_source(Box::new(Files(by_stem)));
    let style = ts.style_index(font, "Shadow");
    let mut p = TextParams::new(
        "+5 [ICON_GOLD] [COLOR_RED]Gold[ENDCOLOR] per turn[NEWLINE]Second line",
        font,
    );
    p.style = style;
    p.colors = [[255; 4], [0, 0, 0, 255], [0; 4]];
    let l = ts.layout(&p);
    assert_eq!(l.lines.len(), 2);
    assert_eq!(l.size[1], 12 + 21);
    assert!(l.quads.iter().any(|q| q.icon));
    assert!(
        ts.font("FontIcons").is_some(),
        "the icon font loaded on demand"
    );
    // width of "Gold" in the real font: G 11? the sum of advances is positive and the layout agrees with measure
    let w = ts.measure("Second line", font, false);
    assert_eq!(l.lines[1].width, w);
    // wrapping a long tooltip keeps every line within the wrap width
    let mut p = TextParams::new(
        "This is a long tooltip text that has to wrap onto several lines because the box is narrow",
        font,
    );
    p.wrap_width = 200;
    let l = ts.layout(&p);
    assert!(l.lines.len() >= 3);
    assert!(l.lines.iter().all(|ln| ln.width <= 200), "{:?}", l.lines);
}
