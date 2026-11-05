//! Style sheets and the colour atlas: `UIManager::GetStyleElement`, `GetColorByName`,
//! `ExtractColor` and `cvUISkinColors`.

use std::collections::HashMap;
use std::rc::Rc;

use crate::values;
use crate::xml::XmlElem;

/// Three colours (`Color0..2`) of a text control; `ColorSet` in the exe.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColorSet {
    pub colors: [[f32; 4]; 3],
}

impl Default for ColorSet {
    fn default() -> Self {
        ColorSet {
            colors: [[1.0; 4]; 3],
        }
    }
}

#[derive(Default)]
struct Sheet {
    exact: HashMap<String, Rc<XmlElem>>,
    lower: HashMap<String, Rc<XmlElem>>,
}

impl Sheet {
    fn new(root: &XmlElem) -> Sheet {
        let mut s = Sheet::default();
        for c in &root.children {
            // `GetFirstChild(name)` finds the first element of that name.
            s.exact.entry(c.name.clone()).or_insert_with(|| c.clone());
            s.lower
                .entry(c.name.to_ascii_lowercase())
                .or_insert_with(|| c.clone());
        }
        s
    }
}

/// Everything the builder looks up by name: styles, colours, colour sets.
#[derive(Default)]
pub struct StyleBook {
    /// Highest priority first: the skins by priority, then the base `Styles.xml`.
    sheets: Vec<Sheet>,
    colors: HashMap<String, [f32; 4]>,
    color_sets: HashMap<String, ColorSet>,
    /// Styles found only by a different letter case.
    pub case_fallbacks: std::cell::Cell<u32>,
}

impl StyleBook {
    /// `skin_styles` and `skin_colors` come lowest priority first; the base files come last in
    /// the search. A name is taken from the first sheet that defines it.
    pub fn new(
        skin_styles: &[Rc<XmlElem>],
        base_styles: Option<&Rc<XmlElem>>,
        skin_colors: &[Rc<XmlElem>],
        base_colors: Option<&Rc<XmlElem>>,
    ) -> StyleBook {
        let mut book = StyleBook::default();
        for s in skin_styles.iter().rev().chain(base_styles) {
            book.sheets.push(Sheet::new(s));
        }
        let color_files: Vec<&Rc<XmlElem>> = skin_colors.iter().rev().chain(base_colors).collect();
        // Plain colours first, so that a colour set can name them (`Color1="Black,200"`).
        for root in &color_files {
            for c in root.children_named("Color") {
                let (Some(name), Some(value)) = (c.attr("Name", false), c.attr("Color", false))
                else {
                    continue;
                };
                if !book.colors.contains_key(name)
                    && let Some(v) = values::color_from_xml(value)
                {
                    book.colors.insert(name.to_owned(), v);
                }
            }
        }
        for root in &color_files {
            for c in root.children_named("ColorSet") {
                let Some(name) = c.attr("Name", false) else {
                    continue;
                };
                if book.color_sets.contains_key(name) {
                    continue;
                }
                let mut set = ColorSet::default();
                for (i, slot) in set.colors.iter_mut().enumerate() {
                    if let Some(v) = c.attr(&format!("Color{i}"), false)
                        && let Some(col) = book.extract_color(v)
                    {
                        *slot = col;
                    }
                }
                book.color_sets.insert(name.to_owned(), set);
            }
        }
        book
    }

    /// `UIManager::GetStyleElement`.
    pub fn style(&self, name: &str) -> Option<Rc<XmlElem>> {
        for s in &self.sheets {
            if let Some(e) = s.exact.get(name) {
                return Some(e.clone());
            }
        }
        for s in &self.sheets {
            if let Some(e) = s.lower.get(&name.to_ascii_lowercase()) {
                self.case_fallbacks.set(self.case_fallbacks.get() + 1);
                return Some(e.clone());
            }
        }
        None
    }

    /// `UIManager::GetColorByName`.
    pub fn color(&self, name: &str) -> Option<[f32; 4]> {
        self.colors.get(name).copied()
    }

    /// `UIManager::GetColorSetByName`.
    pub fn color_set(&self, name: &str) -> Option<ColorSet> {
        self.color_sets.get(name).copied()
    }

    /// `UIManager::ExtractColor`: a name from the atlas with an optional alpha (`Black.128`),
    /// else `r,g,b,a` in 0..255.
    pub fn extract_color(&self, text: &str) -> Option<[f32; 4]> {
        let (name, alpha) = values::color_name_alpha(text)?;
        if !name.is_empty()
            && let Some(mut c) = self.color(name)
        {
            if let Some(a) = alpha {
                let a = a.max(0);
                c[3] = if a < 256 { a as f32 / 255.0 } else { 1.0 };
            }
            return Some(c);
        }
        values::color_from_xml(text)
    }

    /// True when the colour text starts with a name that is not in the atlas and is not a number.
    pub fn is_unknown_name(&self, text: &str) -> bool {
        match values::color_name_alpha(text) {
            Some((name, _)) => {
                !name.is_empty() && self.color(name).is_none() && name.parse::<i32>().is_err()
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::xml::parse;

    fn book() -> StyleBook {
        let base = parse(
            "Styles.xml",
            b"<StyleSheet><Big Size=\"10,10\"/><Small Size=\"1,1\"/></StyleSheet>",
        )
        .unwrap();
        let skin = parse(
            "Styles_X.xml",
            b"<StyleSheet><Big Size=\"20,20\"/></StyleSheet>",
        )
        .unwrap();
        let atlas = parse(
            "ColorAtlas.xml",
            b"<ColorAtlas><Color Name=\"Black\" Color=\"0,0,0,255\"/><Color Name=\"Red\" Color=\"255,0,0,255\"/>\
              <ColorSet Name=\"Beige_Black\" Color0=\"Red\" Color1=\"Black,200\"/></ColorAtlas>",
        )
        .unwrap();
        StyleBook::new(&[skin], Some(&base), &[], Some(&atlas))
    }

    #[test]
    fn skin_style_wins_over_the_base_and_base_fills_the_rest() {
        // UIManager::GetStyleElement: the skins first, then Styles.xml.
        let b = book();
        assert_eq!(b.style("Big").unwrap().attr("Size", false), Some("20,20"));
        assert_eq!(b.style("Small").unwrap().attr("Size", false), Some("1,1"));
        assert!(b.style("Nope").is_none());
    }

    #[test]
    fn style_lookup_falls_back_to_other_letter_case_and_counts_it() {
        let b = book();
        assert!(b.style("big").is_some());
        assert_eq!(b.case_fallbacks.get(), 1);
    }

    #[test]
    fn colors_by_name_with_alpha_or_numbers() {
        // UIManager::ExtractColor
        let b = book();
        assert_eq!(b.extract_color("Red"), Some([1.0, 0.0, 0.0, 1.0]));
        let c = b.extract_color("Black.128").unwrap();
        assert_eq!(c[..3], [0.0, 0.0, 0.0]);
        assert!((c[3] - 128.0 / 255.0).abs() < 1e-6);
        let c = b.extract_color("Red,220").unwrap();
        assert!((c[3] - 220.0 / 255.0).abs() < 1e-6);
        assert_eq!(b.extract_color("0,255,0,255"), Some([0.0, 1.0, 0.0, 1.0]));
        assert!(b.is_unknown_name("Nope"));
        assert!(!b.is_unknown_name("Red"));
        assert!(!b.is_unknown_name("12,3,4"));
    }

    #[test]
    fn color_sets_name_atlas_colors() {
        let b = book();
        let s = b.color_set("Beige_Black").unwrap();
        assert_eq!(s.colors[0], [1.0, 0.0, 0.0, 1.0]);
        assert!((s.colors[1][3] - 200.0 / 255.0).abs() < 1e-6);
        assert_eq!(s.colors[2], [1.0; 4]);
    }
}
