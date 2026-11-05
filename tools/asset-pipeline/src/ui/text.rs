//! Game texts from XML and SQL files.

use std::collections::BTreeMap;

use roxmltree::{Document, Node};

use super::sql::{self, Val};

/// One text row of one language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextRow {
    /// As written after `Language_`: `en_US`, `ru_RU`.
    pub lang: String,
    pub tag: String,
    pub text: String,
    /// Raw `Gender` / `Plurality` columns, trimmed; empty when the row has none.
    pub gender: String,
    pub plurality: String,
}

/// What an XML text file held.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct XmlText {
    pub rows: Vec<TextRow>,
    /// Elements that are not `Row` or `Replace` (for instance `Update`): language, element.
    pub skipped: Vec<(String, String)>,
}

fn text_of(n: Node<'_, '_>) -> String {
    let mut s = String::new();
    for d in n.descendants() {
        if d.is_text() {
            s.push_str(d.text().unwrap_or(""));
        }
    }
    s.trim().to_owned()
}

/// `<Language_xx_XX>` blocks of a `GameData` file. A file without any is an empty result.
pub fn parse_xml(src: &str) -> Result<XmlText, String> {
    let src = src.trim_start_matches('\u{feff}');
    let doc = Document::parse(src).map_err(|e| e.to_string())?;
    let mut out = XmlText::default();
    for lang_node in doc.root_element().children().filter(Node::is_element) {
        let Some(lang) = lang_node.tag_name().name().strip_prefix("Language_") else {
            continue;
        };
        for e in lang_node.children().filter(Node::is_element) {
            let name = e.tag_name().name();
            if matches!(name, "Row" | "Replace") {
                let tag = e
                    .attribute("Tag")
                    .map(str::to_owned)
                    .or_else(|| {
                        e.children()
                            .find(|c| c.is_element() && c.tag_name().name() == "Tag")
                            .map(text_of)
                    })
                    .unwrap_or_default();
                let text = e
                    .children()
                    .find(|c| c.is_element() && c.tag_name().name() == "Text")
                    .map(text_of);
                let column = |name: &str| {
                    e.children()
                        .find(|c| c.is_element() && c.tag_name().name() == name)
                        .map(text_of)
                        .unwrap_or_default()
                };
                if let (false, Some(text)) = (tag.is_empty(), text) {
                    out.rows.push(TextRow {
                        lang: lang.to_owned(),
                        tag,
                        text,
                        gender: column("Gender"),
                        plurality: column("Plurality"),
                    });
                }
            } else {
                out.skipped.push((lang.to_owned(), name.to_owned()));
            }
        }
    }
    Ok(out)
}

/// `INSERT INTO Language_xx_XX (Tag, Text) VALUES ...` statements.
pub fn parse_sql(src: &str) -> Vec<TextRow> {
    let mut out = Vec::new();
    for ins in sql::parse_inserts(src) {
        let Some(lang) = ins.table.strip_prefix("Language_") else {
            continue;
        };
        let col = |name: &str, default: usize| {
            ins.columns
                .iter()
                .position(|c| c.eq_ignore_ascii_case(name))
                .unwrap_or(default)
        };
        let (ti, xi) = (col("Tag", 0), col("Text", 1));
        let (gi, pi) = (
            ins.columns
                .iter()
                .position(|c| c.eq_ignore_ascii_case("Gender")),
            ins.columns
                .iter()
                .position(|c| c.eq_ignore_ascii_case("Plurality")),
        );
        let column = |r: &[Val], i: Option<usize>| match i.and_then(|i| r.get(i)) {
            Some(Val::Str(v)) => v.trim().to_owned(),
            _ => String::new(),
        };
        for r in &ins.rows {
            let (Some(Val::Str(tag) | Val::Raw(tag)), Some(Val::Str(text))) =
                (r.get(ti), r.get(xi))
            else {
                continue;
            };
            out.push(TextRow {
                lang: lang.to_owned(),
                tag: tag.clone(),
                text: text.trim().to_owned(),
                gender: column(r, gi),
                plurality: column(r, pi),
            });
        }
    }
    out
}

/// A later row replaced an earlier one with different text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replaced {
    pub lang: String,
    pub tag: String,
    /// The text that was replaced.
    pub old: String,
    /// File that replaced it.
    pub by: String,
}

/// Texts per language, merged in the order the rows come: a later row replaces an earlier one.
#[derive(Debug, Default)]
pub struct TextSet {
    /// Key: lower-case language. Value: spelling of the language, tag to text.
    pub langs: BTreeMap<String, (String, BTreeMap<String, String>)>,
    /// Key: lower-case language. Value: tag to (`Gender`, `Plurality`), only rows that have one.
    pub grammar: BTreeMap<String, BTreeMap<String, (String, String)>>,
    /// Replacements that changed the text (the first 200 are kept).
    pub replaced: Vec<Replaced>,
    pub replaced_total: usize,
    pub rows_total: usize,
}

impl TextSet {
    /// `source` names the file for the replacement list.
    pub fn add(&mut self, source: &str, rows: impl IntoIterator<Item = TextRow>) {
        for r in rows {
            self.rows_total += 1;
            let (_, map) = self
                .langs
                .entry(r.lang.to_ascii_lowercase())
                .or_insert_with(|| (r.lang.clone(), BTreeMap::new()));
            let from = map.get(&r.tag).map(|old| old != &r.text).unwrap_or(false);
            if from {
                self.replaced_total += 1;
                if self.replaced.len() < 200 {
                    self.replaced.push(Replaced {
                        lang: r.lang.clone(),
                        tag: r.tag.clone(),
                        old: map[&r.tag].clone(),
                        by: source.to_owned(),
                    });
                }
            }
            let g = self.grammar.entry(r.lang.to_ascii_lowercase()).or_default();
            if r.gender.is_empty() && r.plurality.is_empty() {
                g.remove(&r.tag);
            } else {
                g.insert(r.tag.clone(), (r.gender, r.plurality));
            }
            map.insert(r.tag, r.text);
        }
    }

    pub fn grammar_table(&self, lang: &str) -> Option<&BTreeMap<String, (String, String)>> {
        self.grammar.get(&lang.to_ascii_lowercase())
    }

    pub fn table(&self, lang: &str) -> Option<&BTreeMap<String, String>> {
        self.langs.get(&lang.to_ascii_lowercase()).map(|(_, m)| m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_rows_replace_and_skipped() {
        let x = r#"<?xml version="1.0" encoding="utf-8"?>
<GameData>
  <Language_en_US>
    <Row Tag="TXT_A"><Text>Alpha</Text></Row>
    <Replace Tag="TXT_B">
        <Text>
            Beta [NEWLINE]two
        </Text>
    </Replace>
    <Update><Where Tag="TXT_C"/><Set Text="x"/></Update>
    <Row Tag="TXT_D"><Text><![CDATA[<b>]]></Text></Row>
  </Language_en_US>
  <Language_ru_RU><Row Tag="TXT_A"><Text>Альфа</Text></Row></Language_ru_RU>
  <Units><Row><Type>UNIT_X</Type></Row></Units>
</GameData>"#;
        let t = parse_xml(x).unwrap();
        assert_eq!(t.rows.len(), 4);
        assert_eq!(t.rows[1].text, "Beta [NEWLINE]two");
        assert_eq!(t.rows[2].text, "<b>");
        assert_eq!(t.rows[3].lang, "ru_RU");
        assert_eq!(t.skipped, [("en_US".to_owned(), "Update".to_owned())]);
    }

    #[test]
    fn sql_rows() {
        let r = parse_sql(
            "INSERT OR REPLACE INTO Language_en_US (Tag, Text) VALUES ('TXT_X', 'It''s'), ('TXT_Y', 'y'); INSERT INTO Other (a) VALUES ('q');",
        );
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].text, "It's");
        assert_eq!(r[0].lang, "en_US");
    }

    #[test]
    fn later_rows_win_and_changes_are_counted() {
        let mut s = TextSet::default();
        let row = |t: &str, x: &str| TextRow {
            lang: "en_US".into(),
            tag: t.into(),
            text: x.into(),
            gender: String::new(),
            plurality: String::new(),
        };
        s.add("base.xml", [row("A", "1"), row("B", "2")]);
        s.add("bnw.xml", [row("A", "1"), row("B", "3"), row("C", "4")]);
        let en = s.table("EN_us").unwrap();
        assert_eq!(en["B"], "3");
        assert_eq!(en.len(), 3);
        assert_eq!(s.replaced_total, 1);
        assert_eq!(s.replaced[0].tag, "B");
        assert_eq!(s.rows_total, 5);
    }

    #[test]
    fn broken_xml_is_an_error_and_a_file_without_text_is_empty() {
        assert!(parse_xml("<GameData>").is_err());
        assert!(
            parse_xml("<GameData><Units/></GameData>")
                .unwrap()
                .rows
                .is_empty()
        );
    }
}
