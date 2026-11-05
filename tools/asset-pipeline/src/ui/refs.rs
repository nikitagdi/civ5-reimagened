//! What UI files name: textures, styles, contexts, includes, sounds, atlases
//!. Text scans; names built at run time are not seen (SIMP-uia-5).

use std::collections::BTreeSet;

use roxmltree::Document;

/// Last path segment, lower case, trimmed.
pub fn bare(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .trim()
        .to_ascii_lowercase()
}

/// `.dds` names in a text: the quoted or delimited token that ends in `.dds`.
pub fn dds_in_text(text: &str) -> BTreeSet<String> {
    let lower = text.to_ascii_lowercase();
    let mut out = BTreeSet::new();
    let mut from = 0;
    while let Some(p) = lower[from..].find(".dds") {
        let end = from + p + 4;
        from = end;
        let after = lower[end..].chars().next();
        if after.is_some_and(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        let head = &text[..end];
        let start = head
            .rfind(|c: char| {
                matches!(
                    c,
                    '"' | '\''
                        | '<'
                        | '>'
                        | '='
                        | ','
                        | ';'
                        | '('
                        | ')'
                        | '{'
                        | '}'
                        | '|'
                        | '\n'
                        | '\r'
                        | '\t'
                )
            })
            .map_or(0, |i| i + 1);
        let tok = head[start..].trim();
        if tok.len() > 4 {
            out.insert(bare(tok));
        }
    }
    out
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct XmlRefs {
    pub dds: BTreeSet<String>,
    pub styles: BTreeSet<String>,
    /// `LuaContext FileName` (bare, lower case) and whether `Hidden` is set.
    pub contexts: Vec<(String, bool)>,
}

/// `Style`, `MouseOverStyle`...; `FontStyle` names a font layer, not a style sheet entry.
/// On a `TextButton` the state styles (`MouseOverStyle`, `ButtonDownStyle`, `DisabledStyle`) are font layers too
/// (`TextButton` keeps one font style per state, e.g. `MouseOverStyle="SoftShadow"`), so they name no style sheet entry.
fn is_style_attr(tag: &str, name: &str) -> bool {
    name.ends_with("Style")
        && name != "FontStyle"
        && !(tag == "TextButton"
            && matches!(name, "MouseOverStyle" | "ButtonDownStyle" | "DisabledStyle"))
}

fn truthy(v: &str) -> bool {
    matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true")
}

pub fn scan_xml(src: &str) -> Result<XmlRefs, String> {
    let src = src.trim_start_matches('\u{feff}');
    let doc = Document::parse(src).map_err(|e| e.to_string())?;
    let mut r = XmlRefs::default();
    for n in doc.descendants().filter(|n| n.is_element()) {
        for a in n.attributes() {
            let v = a.value();
            if v.to_ascii_lowercase().contains(".dds") {
                r.dds.extend(dds_in_text(v));
            }
            if is_style_attr(n.tag_name().name(), a.name()) && !v.is_empty() {
                r.styles.insert(v.trim().to_owned());
            }
        }
        if n.tag_name().name() == "LuaContext"
            && let Some(f) = n.attribute("FileName")
        {
            r.contexts
                .push((bare(f), n.attribute("Hidden").is_some_and(truthy)));
        }
    }
    Ok(r)
}

/// For a file the strict parser refuses (Civ5 files repeat attributes and break quotes): the
/// same facts from the raw text. An empty file or one that is only comments is an empty result.
pub fn scan_xml_lenient(src: &str) -> XmlRefs {
    let mut r = XmlRefs {
        dds: dds_in_text(src),
        ..XmlRefs::default()
    };
    let mut rest = src;
    while let Some(p) = rest.find("Style=\"") {
        let head = &rest[..p];
        let attr_start = head.rfind(|c: char| c.is_whitespace()).map_or(0, |i| i + 1);
        let name = format!("{}Style", &head[attr_start..]);
        let abs = src.len() - rest.len() + p;
        let tag = src[..abs].rfind('<').map_or("", |i| {
            src[i + 1..abs]
                .split(|c: char| c.is_whitespace())
                .next()
                .unwrap_or("")
        });
        let after = &rest[p + 7..];
        if let Some(q) = after.find('"') {
            if is_style_attr(tag, &name) && q > 0 {
                r.styles.insert(after[..q].trim().to_owned());
            }
            rest = &after[q..];
        } else {
            break;
        }
    }
    let mut rest = src;
    while let Some(p) = rest.find("<LuaContext") {
        let tag_end = rest[p..].find('>').map_or(rest.len(), |e| p + e);
        let tag = &rest[p..tag_end];
        if let Some(f) = tag.find("FileName=\"") {
            let v = &tag[f + 10..];
            if let Some(q) = v.find('"') {
                let hidden = tag.find("Hidden=\"").is_some_and(|h| {
                    let hv = &tag[h + 8..];
                    hv.find('"').is_some_and(|e| truthy(&hv[..e]))
                });
                r.contexts.push((bare(&v[..q]), hidden));
            }
        }
        rest = &rest[tag_end.min(rest.len())..];
    }
    r
}

/// Style definitions of a `StyleSheet`: top-level element name to what it names.
pub fn scan_styles(src: &str) -> Result<Vec<(String, XmlRefs)>, String> {
    let src = src.trim_start_matches('\u{feff}');
    let doc = Document::parse(src).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for n in doc.root_element().children().filter(|n| n.is_element()) {
        let mut r = XmlRefs::default();
        for d in n.descendants().filter(|d| d.is_element()) {
            for a in d.attributes() {
                let v = a.value();
                if v.to_ascii_lowercase().contains(".dds") {
                    r.dds.extend(dds_in_text(v));
                }
                if is_style_attr(d.tag_name().name(), a.name()) && !v.is_empty() {
                    r.styles.insert(v.trim().to_owned());
                }
            }
        }
        out.push((n.tag_name().name().to_owned(), r));
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Ident(String),
    Str(String),
    Concat,
    Punct(char),
}

fn lua_tokens(src: &str) -> Vec<Tok> {
    let ch: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let long_close = |i: usize, ch: &[char]| -> Option<(usize, usize)> {
        // At `[` : `[[` or `[=*[`; returns (level, open length).
        if ch.get(i) != Some(&'[') {
            return None;
        }
        let mut j = i + 1;
        while ch.get(j) == Some(&'=') {
            j += 1;
        }
        (ch.get(j) == Some(&'[')).then_some((j - i - 1, j - i + 1))
    };
    let skip_long = |mut i: usize, ch: &[char], level: usize, open: usize| -> usize {
        i += open;
        while i < ch.len() {
            if ch[i] == ']' {
                let mut j = i + 1;
                let mut n = 0;
                while ch.get(j) == Some(&'=') {
                    n += 1;
                    j += 1;
                }
                if n == level && ch.get(j) == Some(&']') {
                    return j + 1;
                }
            }
            i += 1;
        }
        ch.len()
    };
    while i < ch.len() {
        let c = ch[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '-' && ch.get(i + 1) == Some(&'-') {
            i += 2;
            if let Some((lvl, open)) = long_close(i, &ch) {
                i = skip_long(i, &ch, lvl, open);
            } else {
                while i < ch.len() && ch[i] != '\n' {
                    i += 1;
                }
            }
        } else if c == '"' || c == '\'' {
            let mut s = String::new();
            i += 1;
            while i < ch.len() && ch[i] != c && ch[i] != '\n' {
                if ch[i] == '\\' && i + 1 < ch.len() {
                    i += 1;
                }
                s.push(ch[i]);
                i += 1;
            }
            i += 1;
            out.push(Tok::Str(s));
        } else if c == '[' && long_close(i, &ch).is_some() {
            let (lvl, open) = long_close(i, &ch).expect("checked");
            let end = skip_long(i, &ch, lvl, open);
            let body: String = ch[(i + open).min(end)..end.saturating_sub(open).max(i + open)]
                .iter()
                .collect();
            out.push(Tok::Str(body));
            i = end;
        } else if c == '.' && ch.get(i + 1) == Some(&'.') {
            i += if ch.get(i + 2) == Some(&'.') { 3 } else { 2 };
            out.push(Tok::Concat);
        } else if c.is_alphabetic() || c == '_' {
            let mut s = String::new();
            while i < ch.len() && (ch[i].is_alphanumeric() || ch[i] == '_') {
                s.push(ch[i]);
                i += 1;
            }
            out.push(Tok::Ident(s));
        } else {
            out.push(Tok::Punct(c));
            i += 1;
        }
    }
    out
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LuaRefs {
    pub strings: BTreeSet<String>,
    /// `include("X")`, as written.
    pub includes: BTreeSet<String>,
    /// `include("Dir\%w+Popup.lua", true)`: Lua patterns that name every file they match (GenericPopup).
    pub wild_includes: BTreeSet<String>,
    /// `LoadNewContext("X")`.
    pub contexts: BTreeSet<String>,
    /// `AudioPlay2DSound("X")`.
    pub sounds: BTreeSet<String>,
    /// Whole `.dds` literals (not parts of a concatenation).
    pub dds: BTreeSet<String>,
}

pub fn scan_lua(src: &str) -> LuaRefs {
    let t = lua_tokens(src);
    let mut r = LuaRefs::default();
    for (i, tok) in t.iter().enumerate() {
        let Tok::Str(s) = tok else { continue };
        r.strings.insert(s.clone());
        let joined = i.checked_sub(1).is_some_and(|p| t[p] == Tok::Concat)
            || t.get(i + 1) == Some(&Tok::Concat);
        if !joined && s.to_ascii_lowercase().ends_with(".dds") {
            r.dds.insert(bare(s));
        }
        // include ( "pattern" , true ): every file the pattern matches (the exe's `include(pattern, true)`)
        if i >= 2
            && t[i - 1] == Tok::Punct('(')
            && t[i - 2] == Tok::Ident("include".to_owned())
            && t.get(i + 1) == Some(&Tok::Punct(','))
            && t.get(i + 2) == Some(&Tok::Ident("true".to_owned()))
            && t.get(i + 3) == Some(&Tok::Punct(')'))
        {
            r.wild_includes.insert(s.clone());
        }
        // name ( "literal"
        if i >= 2
            && t[i - 1] == Tok::Punct('(')
            && let Tok::Ident(f) = &t[i - 2]
            && t.get(i + 1) == Some(&Tok::Punct(')'))
        {
            match f.as_str() {
                "include" => {
                    r.includes.insert(s.clone());
                }
                "LoadNewContext" => {
                    r.contexts.insert(s.clone());
                }
                "AudioPlay2DSound" => {
                    r.sounds.insert(s.clone());
                }
                _ => {}
            }
        }
    }
    r
}

/// `string.find(target, pattern)` of Lua 5.1 as a yes/no (classes, `.`, `+ * - ?`, `^`, `$`, no captures or sets).
pub fn lua_pattern_matches(target: &str, pattern: &str) -> bool {
    let t: Vec<char> = target.chars().collect();
    let p: Vec<char> = pattern.chars().collect();
    if p.first() == Some(&'^') {
        return pat_match(&t, 0, &p, 1);
    }
    (0..=t.len()).any(|i| pat_match(&t, i, &p, 0))
}

fn class_matches(c: char, class: char) -> bool {
    let r = match class.to_ascii_lowercase() {
        'a' => c.is_ascii_alphabetic(),
        'd' => c.is_ascii_digit(),
        'l' => c.is_ascii_lowercase(),
        'u' => c.is_ascii_uppercase(),
        's' => c.is_ascii_whitespace(),
        'w' => c.is_ascii_alphanumeric(),
        'p' => c.is_ascii_punctuation(),
        'x' => c.is_ascii_hexdigit(),
        _ => return c == class,
    };
    if class.is_ascii_uppercase() { !r } else { r }
}

/// The length of the single-char item at `p[pi]` and whether it matches `c`.
fn item_matches(c: char, p: &[char], pi: usize) -> (usize, bool) {
    match p[pi] {
        '%' if pi + 1 < p.len() => (2, class_matches(c, p[pi + 1])),
        '.' => (1, true),
        x => (1, x == c),
    }
}

fn pat_match(t: &[char], ti: usize, p: &[char], pi: usize) -> bool {
    if pi >= p.len() {
        return true;
    }
    if p[pi] == '$' && pi + 1 == p.len() {
        return ti == t.len();
    }
    let len = if p[pi] == '%' && pi + 1 < p.len() {
        2
    } else {
        1
    };
    let q = p.get(pi + len).copied();
    let here = ti < t.len() && item_matches(t[ti], p, pi).1;
    match q {
        Some('?') => {
            (here && pat_match(t, ti + 1, p, pi + len + 1)) || pat_match(t, ti, p, pi + len + 1)
        }
        Some('+') => {
            if !here {
                return false;
            }
            let mut n = ti + 1;
            while n < t.len() && item_matches(t[n], p, pi).1 {
                n += 1;
            }
            (ti + 1..=n).rev().any(|k| pat_match(t, k, p, pi + len + 1))
        }
        Some('*') => {
            let mut n = ti;
            while n < t.len() && item_matches(t[n], p, pi).1 {
                n += 1;
            }
            (ti..=n).rev().any(|k| pat_match(t, k, p, pi + len + 1))
        }
        Some('-') => {
            let mut k = ti;
            loop {
                if pat_match(t, k, p, pi + len + 1) {
                    return true;
                }
                if k < t.len() && item_matches(t[k], p, pi).1 {
                    k += 1;
                } else {
                    return false;
                }
            }
        }
        _ => here && pat_match(t, ti + 1, p, pi + len),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lua_patterns() {
        let pat = r"InGame\PopupsGeneric\%w+Popup.lua";
        assert!(lua_pattern_matches(
            r"UI\InGame\PopupsGeneric\AnnexCityPopup.lua",
            pat
        ));
        assert!(!lua_pattern_matches(
            r"UI\InGame\Popups\GenericPopup.lua",
            pat
        ));
        assert!(!lua_pattern_matches(
            r"UI\InGame\PopupsGeneric\Annex_Popup.xml",
            pat
        ));
    }

    #[test]
    fn wild_include_is_scanned() {
        let r = scan_lua(
            r#"local f = include("A\\%w+Popup.lua", true)
include("Lib")"#,
        );
        assert_eq!(
            r.wild_includes.into_iter().collect::<Vec<_>>(),
            [r"A\%w+Popup.lua"]
        );
        assert_eq!(r.includes.into_iter().collect::<Vec<_>>(), ["Lib"]);
    }

    #[test]
    fn dds_tokens_in_xml_text() {
        let s = r#"<Image Texture="assets\UI\Art\Controls\BlueBar500x3.dds"/><A Texture='x/Y Z.DDS' B="q.ddsx" C="Plain.dds"/>"#;
        let d = dds_in_text(s);
        assert_eq!(
            d.into_iter().collect::<Vec<_>>(),
            ["bluebar500x3.dds", "plain.dds", "y z.dds"]
        );
    }

    #[test]
    fn xml_refs_styles_and_contexts() {
        let r = scan_xml(
            r#"<Context><LuaContext FileName="Assets/UI/InGame/TopPanel" Hidden="True"/>
              <LuaContext FileName="Other"/><Grid Style="Grid9Frame27"/><Button MouseOverStyle="SmallButton" Texture="A.dds"/></Context>"#,
        )
        .unwrap();
        assert_eq!(
            r.contexts,
            [("toppanel".to_owned(), true), ("other".to_owned(), false)]
        );
        assert!(r.styles.contains("Grid9Frame27") && r.styles.contains("SmallButton"));
        assert!(r.dds.contains("a.dds"));
    }

    #[test]
    fn text_button_state_styles_are_font_layers_not_style_sheet_names() {
        let s = r#"<Context><TextButton Style="Font14or20" FontStyle="Shadow" MouseOverStyle="SoftShadow"/><Button MouseOverStyle="SmallButton"/></Context>"#;
        let r = scan_xml(s).unwrap();
        assert!(r.styles.contains("Font14or20") && r.styles.contains("SmallButton"));
        assert!(!r.styles.contains("SoftShadow"));
        let l = scan_xml_lenient(s);
        assert!(l.styles.contains("SmallButton") && !l.styles.contains("SoftShadow"));
    }

    #[test]
    fn lenient_scan_of_a_broken_file() {
        let s = r#"<Context><LuaContext FileName="Assets/UI/X/Pop" Hidden="1"/><Grid Style="G9" Anchor="L,T" Anchor="C,C"/><Label FontStyle="Shadow" Texture="a/B.dds <broken"/></Context"#;
        assert!(scan_xml(s).is_err());
        let r = scan_xml_lenient(s);
        assert_eq!(r.contexts, [("pop".to_owned(), true)]);
        assert_eq!(r.styles.into_iter().collect::<Vec<_>>(), ["G9"]);
        assert!(r.dds.contains("b.dds"));
    }

    #[test]
    fn style_sheet_entries() {
        let s = scan_styles(
            r#"<StyleSheet><Up Texture="a/B.dds"/><Btn><GridData Style="G"/><Image Texture="c.dds"/></Btn></StyleSheet>"#,
        )
        .unwrap();
        assert_eq!(s[0].0, "Up");
        assert!(s[0].1.dds.contains("b.dds"));
        assert!(s[1].1.styles.contains("G") && s[1].1.dds.contains("c.dds"));
    }

    #[test]
    fn lua_calls_comments_and_concatenation() {
        let l = r#"
          include("IconSupport")
          -- include("Commented")
          --[[ include("Block") ]]
          ContextPtr:LoadNewContext( 'Popups/X' )
          Events.AudioPlay2DSound("AS2D_INTERFACE_POLICY")
          local a = "Art/Full.dds"
          local b = "citizen" .. n .. ".dds"
          local long = [[ text.dds ]]
          print("x" .. "y")
        "#;
        let r = scan_lua(l);
        assert_eq!(r.includes.into_iter().collect::<Vec<_>>(), ["IconSupport"]);
        assert!(r.contexts.contains("Popups/X"));
        assert!(r.sounds.contains("AS2D_INTERFACE_POLICY"));
        assert_eq!(r.dds.into_iter().collect::<Vec<_>>(), ["full.dds"]);
        assert!(r.strings.contains("AS2D_INTERFACE_POLICY"));
    }
}
