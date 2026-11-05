//! `IconTextureAtlases` rows from XML and SQL files.

use std::collections::BTreeMap;

use roxmltree::{Document, Node};

use super::sql::{self, Val};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtlasRow {
    pub atlas: String,
    pub icon_size: u32,
    /// Texture file as written (`Name.dds`, maybe with a path).
    pub filename: String,
    pub icons_per_row: u32,
    pub icons_per_column: u32,
}

fn child_text(n: Node<'_, '_>, tag: &str) -> Option<String> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == tag)
        .map(|c| c.text().unwrap_or("").trim().to_owned())
}

fn num(s: Option<String>) -> Option<u32> {
    s?.trim().parse().ok()
}

/// Rows of `<IconTextureAtlases>` blocks. A file without any is an empty result.
pub fn parse_xml(src: &str) -> Result<Vec<AtlasRow>, String> {
    let src = src.trim_start_matches('\u{feff}');
    let doc = Document::parse(src).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for block in doc
        .root_element()
        .children()
        .filter(|c| c.is_element() && c.tag_name().name() == "IconTextureAtlases")
    {
        for r in block
            .children()
            .filter(|c| c.is_element() && matches!(c.tag_name().name(), "Row" | "Replace"))
        {
            let (Some(atlas), Some(size), Some(file)) = (
                child_text(r, "Atlas"),
                num(child_text(r, "IconSize")),
                child_text(r, "Filename"),
            ) else {
                continue;
            };
            out.push(AtlasRow {
                atlas,
                icon_size: size,
                filename: file,
                icons_per_row: num(child_text(r, "IconsPerRow")).unwrap_or(1),
                icons_per_column: num(child_text(r, "IconsPerColumn")).unwrap_or(1),
            });
        }
    }
    Ok(out)
}

pub fn parse_sql(src: &str) -> Vec<AtlasRow> {
    let mut out = Vec::new();
    for ins in sql::parse_inserts(src) {
        if !ins.table.eq_ignore_ascii_case("IconTextureAtlases") {
            continue;
        }
        let col = |name: &str| {
            ins.columns
                .iter()
                .position(|c| c.eq_ignore_ascii_case(name))
        };
        let (Some(a), Some(s), Some(f)) = (col("Atlas"), col("IconSize"), col("Filename")) else {
            continue;
        };
        let (r, c) = (col("IconsPerRow"), col("IconsPerColumn"));
        for row in &ins.rows {
            let get = |i: usize| row.get(i).and_then(Val::as_str).map(str::to_owned);
            let (Some(atlas), Some(size), Some(file)) = (get(a), num(get(s)), get(f)) else {
                continue;
            };
            out.push(AtlasRow {
                atlas,
                icon_size: size,
                filename: file,
                icons_per_row: r.and_then(|i| num(get(i))).unwrap_or(1),
                icons_per_column: c.and_then(|i| num(get(i))).unwrap_or(1),
            });
        }
    }
    out
}

/// Rows keyed by (atlas, icon size); a later row replaces an earlier one.
#[derive(Debug, Default)]
pub struct AtlasSet {
    pub rows: BTreeMap<(String, u32), AtlasRow>,
    pub replaced: usize,
}

impl AtlasSet {
    pub fn add(&mut self, rows: impl IntoIterator<Item = AtlasRow>) {
        for r in rows {
            if self
                .rows
                .insert((r.atlas.clone(), r.icon_size), r)
                .is_some()
            {
                self.replaced += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_rows() {
        let x = r#"<GameData><IconTextureAtlases>
          <Row><Atlas>TECH_ATLAS</Atlas><IconSize>64</IconSize><Filename>TechAtlas64.dds</Filename><IconsPerRow>8</IconsPerRow><IconsPerColumn>8</IconsPerColumn></Row>
          <Row><Atlas>NOSIZE</Atlas><Filename>x.dds</Filename></Row>
        </IconTextureAtlases><Units/></GameData>"#;
        let r = parse_xml(x).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(
            r[0],
            AtlasRow {
                atlas: "TECH_ATLAS".into(),
                icon_size: 64,
                filename: "TechAtlas64.dds".into(),
                icons_per_row: 8,
                icons_per_column: 8
            }
        );
    }

    #[test]
    fn sql_rows_and_replacement() {
        let r = parse_sql(
            "INSERT INTO IconTextureAtlases (Atlas, IconSize, Filename, IconsPerRow, IconsPerColumn) VALUES ('A', 45, 'a45.dds', 4, 2), ('A', 45, 'b45.dds', 4, 2);",
        );
        assert_eq!(r.len(), 2);
        let mut s = AtlasSet::default();
        s.add(r);
        assert_eq!(s.rows.len(), 1);
        assert_eq!(s.replaced, 1);
        assert_eq!(s.rows[&("A".to_owned(), 45)].filename, "b45.dds");
    }
}
