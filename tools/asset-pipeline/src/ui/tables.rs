//! `IconFontTextures`, `IconFontMapping`, `Colors` and `Languages` rows from XML and SQL files
//!. The XML is read section by section and tolerantly: Lekmod's
//! `Override/CIV5Units.xml` has mismatched tag case, so a strict parse of the file is useless.

use std::collections::BTreeMap;

use asset_format::{UiIconMapping, UiLanguage, UiTables};

use super::sql;

/// `IconFontTexture` default of a mapping row (the column default in `CIV5IconFontMapping.xml`).
pub const DEFAULT_ICON_FONT: &str = "ICON_FONT_TEXTURE_DEFAULT";

/// How a row entered its table. `InsertOrAbort` and `InsertOrIgnore` keep an existing row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowKind {
    Replace,
    Keep,
}

/// Columns of one row, names lower-cased.
type Row = BTreeMap<String, String>;

#[derive(Debug, Default, Clone)]
pub struct TableRows {
    pub textures: Vec<(RowKind, String, String)>,
    pub icons: Vec<(RowKind, UiIconMapping)>,
    pub colors: Vec<(RowKind, String, [f32; 4])>,
    pub languages: Vec<(RowKind, UiLanguage)>,
}

impl TableRows {
    pub fn is_empty(&self) -> bool {
        self.textures.is_empty()
            && self.icons.is_empty()
            && self.colors.is_empty()
            && self.languages.is_empty()
    }

    fn add(&mut self, table: &str, kind: RowKind, r: &Row) {
        let get = |k: &str| r.get(k).map(String::as_str).filter(|v| !v.is_empty());
        match table {
            "iconfonttextures" => {
                if let (Some(key), Some(file)) =
                    (get("iconfonttexture"), get("iconfonttexturefile"))
                {
                    self.textures.push((kind, key.to_owned(), file.to_owned()));
                }
            }
            "iconfontmapping" => {
                if let (Some(name), Some(glyph)) = (
                    get("iconname"),
                    get("iconmapping").and_then(|v| v.parse::<u32>().ok()),
                ) {
                    self.icons.push((
                        kind,
                        UiIconMapping {
                            name: name.to_owned(),
                            glyph,
                            texture: get("iconfonttexture")
                                .unwrap_or(DEFAULT_ICON_FONT)
                                .to_owned(),
                        },
                    ));
                }
            }
            "colors" => {
                if let Some(name) = get("type") {
                    let ch = |k: &str| get(k).and_then(|v| v.parse::<f32>().ok());
                    if let (Some(r), Some(g), Some(b)) = (ch("red"), ch("green"), ch("blue")) {
                        self.colors.push((
                            kind,
                            name.to_owned(),
                            [r, g, b, ch("alpha").unwrap_or(1.0)],
                        ));
                    }
                }
            }
            "languages" => {
                if let (Some(lang), Some(rule)) = (
                    get("type"),
                    get("pluralrule").and_then(|v| v.parse::<u32>().ok()),
                ) {
                    self.languages.push((
                        kind,
                        UiLanguage {
                            lang: lang.to_owned(),
                            plural_rule: rule,
                        },
                    ));
                }
            }
            _ => {}
        }
    }
}

/// Rows of the `IconTextureAtlases` sections, read as tolerantly as the other tables.
pub fn parse_atlas_xml(src: &str) -> Vec<super::atlas::AtlasRow> {
    let lower = src.to_ascii_lowercase();
    let (open, close) = ("<icontextureatlases>", "</icontextureatlases>");
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(start) = find_from(&lower, open, at) {
        let body = start + open.len();
        let end = find_from(&lower, close, body).unwrap_or(lower.len());
        for (_, r) in rows_of(&src[body..end]) {
            let num = |k: &str| r.get(k).and_then(|v| v.parse::<u32>().ok());
            let (Some(atlas), Some(size), Some(file)) = (
                r.get("atlas").filter(|v| !v.is_empty()),
                num("iconsize"),
                r.get("filename").filter(|v| !v.is_empty()),
            ) else {
                continue;
            };
            out.push(super::atlas::AtlasRow {
                atlas: atlas.clone(),
                icon_size: size,
                filename: file.clone(),
                icons_per_row: num("iconsperrow").unwrap_or(1),
                icons_per_column: num("iconspercolumn").unwrap_or(1),
            });
        }
        at = end.max(body);
    }
    out
}

const TABLES: [&str; 4] = ["IconFontTextures", "IconFontMapping", "Colors", "Languages"];

fn kind_of(element: &str) -> RowKind {
    let e = element.to_ascii_lowercase();
    if e.contains("abort") || e.contains("ignore") {
        RowKind::Keep
    } else {
        RowKind::Replace
    }
}

/// Finds `needle` (lower-case) in `hay` (lower-case copy) from `from`.
fn find_from(hay: &str, needle: &str, from: usize) -> Option<usize> {
    hay.get(from..)?.find(needle).map(|p| p + from)
}

/// Rows of the table sections of an XML file. Never fails: what cannot be read is skipped.
pub fn parse_xml(src: &str) -> TableRows {
    let mut out = TableRows::default();
    let lower = src.to_ascii_lowercase();
    for table in TABLES {
        let open = format!("<{}>", table.to_ascii_lowercase());
        let close = format!("</{}>", table.to_ascii_lowercase());
        let mut at = 0;
        while let Some(start) = find_from(&lower, &open, at) {
            let body = start + open.len();
            // No closing tag: the section runs to the end of the file.
            let end = find_from(&lower, &close, body).unwrap_or(lower.len());
            for (element, row) in rows_of(&src[body..end]) {
                out.add(&table.to_ascii_lowercase(), kind_of(&element), &row);
            }
            at = end.max(body);
        }
    }
    out
}

/// `<Row>` style children of a section: element name and column values (child elements and attributes).
fn rows_of(section: &str) -> Vec<(String, Row)> {
    let mut rows = Vec::new();
    let lower = section.to_ascii_lowercase();
    let mut i = 0;
    while let Some(p) = section[i..].find('<') {
        let at = i + p;
        let rest = &section[at..];
        if rest.starts_with("<!--") {
            i = rest.find("-->").map_or(section.len(), |e| at + e + 3);
            continue;
        }
        let Some(gt) = rest.find('>') else { break };
        let tag = &rest[1..gt];
        i = at + gt + 1;
        if tag.starts_with('/') || tag.starts_with('?') || tag.starts_with('!') {
            continue;
        }
        let self_closing = tag.ends_with('/');
        let tag = tag.trim_end_matches('/');
        let mut parts = tag.splitn(2, char::is_whitespace);
        let element = parts.next().unwrap_or("").to_owned();
        let mut row = Row::new();
        for (k, v) in attributes(parts.next().unwrap_or("")) {
            row.insert(k, v);
        }
        if !self_closing {
            // Columns up to the matching close of the row (case-insensitive).
            let close = format!("</{}", element.to_ascii_lowercase());
            let stop = find_from(&lower, &close, i).unwrap_or(section.len());
            for (k, v) in columns(&section[i..stop]) {
                row.insert(k, v);
            }
            i = section[stop..]
                .find('>')
                .map_or(section.len(), |e| stop + e + 1);
        }
        rows.push((element, row));
    }
    rows
}

fn attributes(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(eq) = rest.find('=') {
        let name = rest[..eq].trim().to_ascii_lowercase();
        let after = rest[eq + 1..].trim_start();
        let Some(q) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            break;
        };
        let Some(end) = after[1..].find(q) else { break };
        out.push((name, after[1..1 + end].trim().to_owned()));
        rest = &after[end + 2..];
    }
    out
}

/// `<Name>value</Name>` children, tolerant of a close tag of another case.
fn columns(body: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(p) = body[i..].find('<') {
        let at = i + p;
        let rest = &body[at..];
        if rest.starts_with("<!--") {
            i = rest.find("-->").map_or(body.len(), |e| at + e + 3);
            continue;
        }
        let Some(gt) = rest.find('>') else { break };
        let name = &rest[1..gt];
        i = at + gt + 1;
        if name.starts_with('/') || name.ends_with('/') || name.starts_with(['?', '!']) {
            continue;
        }
        let end = body[i..].find('<').map_or(body.len(), |e| i + e);
        out.push((name.to_ascii_lowercase(), body[i..end].trim().to_owned()));
        i = end;
    }
    out
}

/// `INSERT INTO <table> (cols) VALUES ...` rows of the four tables.
pub fn parse_sql(src: &str) -> TableRows {
    let mut out = TableRows::default();
    for ins in sql::parse_inserts(src) {
        let table = ins.table.to_ascii_lowercase();
        if !TABLES.iter().any(|t| t.eq_ignore_ascii_case(&table)) {
            continue;
        }
        for r in &ins.rows {
            let mut row = Row::new();
            for (c, v) in ins.columns.iter().zip(r) {
                if let Some(v) = v.as_str() {
                    row.insert(c.to_ascii_lowercase(), v.trim().to_owned());
                }
            }
            // A bare `NULL` is no value.
            row.retain(|_, v| !(v.eq_ignore_ascii_case("null")));
            out.add(&table, RowKind::Replace, &row);
        }
    }
    out
}

/// The merged tables of all layers, in the order the rows arrive.
#[derive(Debug, Default)]
pub struct TableSet {
    textures: BTreeMap<String, String>,
    icons: BTreeMap<String, UiIconMapping>,
    colors: BTreeMap<String, [f32; 4]>,
    languages: BTreeMap<String, UiLanguage>,
}

fn put<V>(map: &mut BTreeMap<String, V>, kind: RowKind, key: &str, v: V) {
    match kind {
        RowKind::Replace => {
            map.insert(key.to_owned(), v);
        }
        RowKind::Keep => {
            map.entry(key.to_owned()).or_insert(v);
        }
    }
}

impl TableSet {
    pub fn add(&mut self, rows: TableRows) {
        for (k, key, file) in rows.textures {
            put(&mut self.textures, k, &key, file);
        }
        for (k, i) in rows.icons {
            let key = i.name.clone();
            put(&mut self.icons, k, &key, i);
        }
        for (k, name, c) in rows.colors {
            put(&mut self.colors, k, &name, c);
        }
        for (k, l) in rows.languages {
            let key = l.lang.clone();
            put(&mut self.languages, k, &key, l);
        }
    }

    pub fn into_tables(self) -> UiTables {
        UiTables {
            icon_textures: self.textures.into_iter().collect(),
            icons: self.icons.into_values().collect(),
            colors: self.colors.into_iter().collect(),
            languages: self.languages.into_values().collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_sections_rows_and_defaults() {
        let x = r#"<GameData>
  <Table name="IconFontMapping"><Column name="IconName"/></Table>
  <IconFontTextures><Row><IconFontTexture>T_E2</IconFontTexture><IconFontTextureFile>FontIcons_E2</IconFontTextureFile></Row></IconFontTextures>
  <!-- comment -->
  <IconFontMapping>
    <Row><IconName>ICON_GOLD</IconName><IconMapping>3</IconMapping></Row>
    <InsertOrAbort><IconName>ICON_RES_CRAB</IconName><IconMapping>83</IconMapping><IconFontTexture>T_E2</IconFontTexture></InsertOrAbort>
  </IconFontMapping>
  <Colors><Row><Type>COLOR_RED</Type><Red>1</Red><Green>0</Green><Blue>.5</Blue><Alpha>1</Alpha></Row></Colors>
</GameData>"#;
        let r = parse_xml(x);
        assert_eq!(r.textures.len(), 1);
        assert_eq!(r.icons.len(), 2);
        assert_eq!(r.icons[0].1.texture, DEFAULT_ICON_FONT);
        assert_eq!(r.icons[1].0, RowKind::Keep);
        assert_eq!(r.icons[1].1.texture, "T_E2");
        assert_eq!(r.colors[0].2, [1.0, 0.0, 0.5, 1.0]);
    }

    #[test]
    fn broken_tag_case_elsewhere_does_not_hide_a_section() {
        let x = "<GameData><Units><Row><Type>U</Type></row></Units>\
                 <Colors><Row><Type>COLOR_X</Type><Red>0</Red><Green>0</Green><Blue>0</Blue></Row></Colors>\
                 <Buildings><Row></Buildings></GameData>";
        assert_eq!(parse_xml(x).colors.len(), 1);
    }

    #[test]
    fn sql_rows_and_merge_order() {
        let r = parse_sql(
            "INSERT INTO IconFontMapping (IconName, IconMapping, IconFontTexture) VALUES ('ICON_A', 1, 'T'), ('ICON_B', 2, 'T');",
        );
        assert_eq!(r.icons.len(), 2);
        let mut s = TableSet::default();
        s.add(r);
        let mut keep = TableRows::default();
        keep.icons.push((
            RowKind::Keep,
            UiIconMapping {
                name: "ICON_A".into(),
                glyph: 9,
                texture: "T".into(),
            },
        ));
        keep.icons.push((
            RowKind::Replace,
            UiIconMapping {
                name: "ICON_B".into(),
                glyph: 7,
                texture: "T".into(),
            },
        ));
        s.add(keep);
        let t = s.into_tables();
        assert_eq!(t.icons[0].glyph, 1);
        assert_eq!(t.icons[1].glyph, 7);
    }

    #[test]
    fn atlas_rows_of_a_file_that_is_not_xml() {
        let x = "<GameData><IconTextureAtlases><Row><Atlas>A</Atlas><IconSize>64</IconSize><Filename>a.dds</Filename><IconsPerRow>8</IconsPerRow><IconsPerColumn>4</IconsPerColumn></Row></IconTextureAtlases><Units><Row></row></Units></GameData>";
        let r = parse_atlas_xml(x);
        assert_eq!((r.len(), r[0].icons_per_column), (1, 4));
    }

    #[test]
    fn languages_have_a_plural_rule() {
        let x = "<GameData><Languages><Row><ID>0</ID><Type>en_US</Type><Name>English</Name><PluralRule>2</PluralRule></Row></Languages></GameData>";
        let r = parse_xml(x);
        assert_eq!(r.languages[0].1.plural_rule, 2);
    }
}
