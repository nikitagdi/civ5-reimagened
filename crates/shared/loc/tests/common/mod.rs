//! Loads the en_US text rows of the installed game (base, expansions, DLC, Lekmod, EUI) into a `MapTable`.
//! Rows are read from the XML `<Language_en_US>` sections; later files replace earlier rows, `<Delete>` removes.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use loc::MapTable;

/// The `Assets` folder of the game, or `None` (the test then skips).
pub fn assets_dir() -> Option<PathBuf> {
    let candidates = [std::env::var_os("CIV5_GAME_DIR").map(PathBuf::from)];
    for dir in candidates.into_iter().flatten() {
        let assets = dir.join("Assets");
        if assets.join("Gameplay").is_dir() {
            return Some(assets);
        }
    }
    eprintln!("SKIPPED: game files not found (set CIV5_GAME_DIR)");
    None
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("xml")) {
            out.push(p);
        }
    }
}

fn priority(p: &Path, assets: &Path) -> u32 {
    let rel = p
        .strip_prefix(assets)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/");
    if rel.starts_with("Gameplay/") {
        0
    } else if rel.starts_with("DLC/Expansion2/") {
        2
    } else if rel.starts_with("DLC/Expansion/") {
        1
    } else if rel.starts_with("DLC/LEKMOD") {
        4
    } else if rel.ends_with("EUI_text_en_us.xml") {
        5
    } else {
        3
    }
}

/// Every XML file under `Assets` that has an `<Language_en_US>` section, in load order.
pub fn text_files(assets: &Path) -> Vec<PathBuf> {
    let mut all = Vec::new();
    walk(assets, &mut all);
    let mut out: Vec<PathBuf> = all
        .into_iter()
        .filter(|p| {
            std::fs::read(p).is_ok_and(|b| {
                let s = String::from_utf8_lossy(&b);
                s.contains("<Language_en_US>") || s.contains("<Language_EN_US>")
            })
        })
        .collect();
    out.sort_by_key(|p| priority(p, assets));
    out
}

fn child_text(row: roxmltree::Node, name: &str) -> Option<String> {
    row.children()
        .find(|c| c.is_element() && c.tag_name().name().eq_ignore_ascii_case(name))
        .map(|c| c.text().unwrap_or("").trim().to_string())
}

/// Loads one file into `table`; returns the number of rows read.
pub fn load_file(path: &Path, table: &mut MapTable) -> usize {
    let bytes = std::fs::read(path).expect("read text file");
    let text = String::from_utf8_lossy(&bytes);
    let text = text.trim_start_matches('\u{feff}');
    let Ok(doc) = roxmltree::Document::parse_with_options(
        text,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    ) else {
        eprintln!("unparsable xml: {}", path.display());
        return 0;
    };
    let mut n = 0;
    for section in doc
        .descendants()
        .filter(|d| d.is_element() && d.tag_name().name().eq_ignore_ascii_case("Language_en_US"))
    {
        for row in section.children().filter(|c| c.is_element()) {
            let Some(tag) = row.attribute("Tag") else {
                continue;
            };
            match row.tag_name().name() {
                "Delete" => table.remove(tag),
                "Row" | "Replace" | "Update" => {
                    if let Some(t) = child_text(row, "Text") {
                        let g = child_text(row, "Gender").filter(|s| !s.is_empty());
                        let p = child_text(row, "Plurality").filter(|s| !s.is_empty());
                        table.insert(tag, &t, g.as_deref(), p.as_deref());
                        n += 1;
                    }
                }
                _ => {}
            }
        }
    }
    n
}

/// All en_US rows of the installation.
pub fn load_en_us(assets: &Path) -> MapTable {
    let mut table = MapTable::new();
    for f in text_files(assets) {
        load_file(&f, &mut table);
    }
    table
}
