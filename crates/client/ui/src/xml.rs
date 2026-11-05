//! XML of the UI files: an owned, line-numbered tree from a tolerant parser (the files of the
//! game are not well-formed XML), and the attribute lookup with the case rule of the config.
//!
//! The exe reads attributes by name (`FXmlElement::GetFirstAttributeVal`) and silently skips the
//! ones it does not know; the builder here warns about them, once per file and name (AC-F1).

use std::rc::Rc;

/// One element: name, attributes in document order, children, and where it is.
#[derive(Debug, Clone)]
pub struct XmlElem {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Rc<XmlElem>>,
    pub line: u32,
    pub file: Rc<str>,
}

/// A file that does not parse; the context becomes empty.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{file}:{line}: {message}")]
pub struct XmlError {
    pub file: String,
    pub line: u32,
    pub message: String,
    /// The file has no root element (`TechPopup.xml` is a declaration only): an empty
    /// context, not an error.
    pub empty: bool,
}

impl XmlElem {
    /// First attribute with this name (`GetFirstAttributeVal`). The case rule comes from the config.
    pub fn attr(&self, name: &str, case_insensitive: bool) -> Option<&str> {
        self.attr_index(name, case_insensitive)
            .map(|i| self.attrs[i].1.as_str())
    }

    pub fn attr_index(&self, name: &str, case_insensitive: bool) -> Option<usize> {
        self.attrs.iter().position(|(k, _)| {
            if case_insensitive {
                k.eq_ignore_ascii_case(name)
            } else {
                k == name
            }
        })
    }

    /// First child with this name (`FXmlElement::GetFirstChild(name)`).
    pub fn child(&self, name: &str) -> Option<&Rc<XmlElem>> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Rc<XmlElem>> {
        self.children.iter().filter(move |c| c.name == name)
    }
}

/// Parses a file. A BOM is dropped; bytes that are not UTF-8 are decoded as Latin-1, as the
/// files of the games that are not UTF-8 are in Western code pages.
///
/// The parser is as tolerant as the one of the exe on the points the game files need: a `<` or
/// `>` inside an attribute value, no space between two attributes, a repeated attribute
/// (the first one wins, as `GetFirstAttributeVal` finds it). Structure errors (a closing tag
/// that does not match, an unclosed element) are errors.
pub fn parse(file: &str, bytes: &[u8]) -> Result<Rc<XmlElem>, XmlError> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let text: std::borrow::Cow<'_, str> = match std::str::from_utf8(bytes) {
        Ok(s) => s.into(),
        Err(_) => bytes.iter().map(|&b| b as char).collect::<String>().into(),
    };
    let file_rc: Rc<str> = Rc::from(file);
    let mut p = Parser {
        s: text.as_bytes(),
        text: &text,
        pos: 0,
        line: 1,
        file: file_rc,
    };
    p.document().map_err(|(line, message, empty)| XmlError {
        file: file.to_owned(),
        line,
        message,
        empty,
    })
}

type PErr = (u32, String, bool);

struct Parser<'a> {
    s: &'a [u8],
    text: &'a str,
    pos: usize,
    line: u32,
    file: Rc<str>,
}

struct Open {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Rc<XmlElem>>,
    line: u32,
}

impl Parser<'_> {
    fn err<T>(&self, msg: impl Into<String>) -> Result<T, PErr> {
        Err((self.line, msg.into(), false))
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.pos).copied()
    }

    fn bump(&mut self) {
        if self.peek() == Some(b'\n') {
            self.line += 1;
        }
        self.pos += 1;
    }

    fn starts_with(&self, lit: &str) -> bool {
        self.s[self.pos..].starts_with(lit.as_bytes())
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.bump();
        }
    }

    /// Skips to just after `end`; false at end of input.
    fn skip_past(&mut self, end: &str) -> bool {
        while self.pos < self.s.len() {
            if self.starts_with(end) {
                for _ in 0..end.len() {
                    self.bump();
                }
                return true;
            }
            self.bump();
        }
        false
    }

    fn name(&mut self) -> String {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_whitespace() || matches!(c, b'=' | b'/' | b'>' | b'<' | b'"' | b'\'') {
                break;
            }
            self.pos += 1;
        }
        self.text[start..self.pos].to_owned()
    }

    fn document(&mut self) -> Result<Rc<XmlElem>, PErr> {
        let mut stack: Vec<Open> = Vec::new();
        let mut root: Option<Rc<XmlElem>> = None;
        loop {
            // Text between tags is ignored.
            while self.peek().is_some_and(|c| c != b'<') {
                self.bump();
            }
            if self.peek().is_none() {
                break;
            }
            if self.starts_with("<?") {
                if !self.skip_past("?>") {
                    return self.err("unterminated processing instruction");
                }
            } else if self.starts_with("<!--") {
                if !self.skip_past("-->") {
                    return self.err("unterminated comment");
                }
            } else if self.starts_with("<![CDATA[") {
                if !self.skip_past("]]>") {
                    return self.err("unterminated CDATA");
                }
            } else if self.starts_with("<!") {
                if !self.skip_past(">") {
                    return self.err("unterminated declaration");
                }
            } else if self.starts_with("</") {
                self.pos += 2;
                let name = self.name();
                self.skip_ws();
                if self.peek() != Some(b'>') {
                    return self.err(format!("bad closing tag </{name}"));
                }
                self.bump();
                let Some(open) = stack.pop() else {
                    return self.err(format!("closing tag </{name}> without an element"));
                };
                if open.name != name {
                    return self.err(format!(
                        "closing tag </{name}> does not match <{}> from line {}",
                        open.name, open.line
                    ));
                }
                let done = self.finish(open);
                match stack.last_mut() {
                    Some(parent) => parent.children.push(done),
                    None => {
                        root = Some(done);
                        break;
                    }
                }
            } else {
                self.bump();
                let line = self.line;
                let name = self.name();
                if name.is_empty() {
                    return self.err("element without a name");
                }
                let (attrs, empty) = self.attributes()?;
                let open = Open {
                    name,
                    attrs,
                    children: Vec::new(),
                    line,
                };
                if empty {
                    let done = self.finish(open);
                    match stack.last_mut() {
                        Some(parent) => parent.children.push(done),
                        None => {
                            root = Some(done);
                            break;
                        }
                    }
                } else {
                    stack.push(open);
                }
            }
        }
        if let Some(open) = stack.last() {
            return self.err(format!(
                "<{}> from line {} is not closed",
                open.name, open.line
            ));
        }
        root.ok_or((
            self.line,
            "the document does not have a root element".to_owned(),
            true,
        ))
    }

    fn finish(&self, o: Open) -> Rc<XmlElem> {
        Rc::new(XmlElem {
            name: o.name,
            attrs: o.attrs,
            children: o.children,
            line: o.line,
            file: self.file.clone(),
        })
    }

    /// Attributes up to `>` or `/>`; true for the empty-element form.
    fn attributes(&mut self) -> Result<(Vec<(String, String)>, bool), PErr> {
        let mut attrs = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                None => return self.err("unexpected end of file in a tag"),
                Some(b'>') => {
                    self.bump();
                    return Ok((attrs, false));
                }
                Some(b'/') => {
                    self.bump();
                    self.skip_ws();
                    if self.peek() == Some(b'>') {
                        self.bump();
                        return Ok((attrs, true));
                    }
                    return self.err("stray '/' in a tag");
                }
                Some(b'<') => return self.err("'<' in a tag"),
                Some(_) => {}
            }
            let key = self.name();
            if key.is_empty() {
                // A stray quote or '=': skip it so that the loop moves on.
                self.bump();
                continue;
            }
            self.skip_ws();
            let mut value = String::new();
            if self.peek() == Some(b'=') {
                self.bump();
                self.skip_ws();
                match self.peek() {
                    Some(q @ (b'"' | b'\'')) => {
                        self.bump();
                        let start = self.pos;
                        while self.peek().is_some_and(|c| c != q) {
                            self.bump();
                        }
                        if self.peek().is_none() {
                            return self.err("unterminated attribute value");
                        }
                        value = decode_entities(&self.text[start..self.pos]);
                        self.bump();
                    }
                    _ => {
                        let start = self.pos;
                        while self
                            .peek()
                            .is_some_and(|c| !c.is_ascii_whitespace() && c != b'>')
                        {
                            self.bump();
                        }
                        value = decode_entities(&self.text[start..self.pos]);
                    }
                }
            }
            attrs.push((key, value));
        }
    }
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';') else { break };
        let ent = &rest[1..end];
        let rep = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => ent
                .strip_prefix('#')
                .and_then(|n| match n.strip_prefix(['x', 'X']) {
                    Some(h) => u32::from_str_radix(h, 16).ok(),
                    None => n.parse().ok(),
                })
                .and_then(char::from_u32),
        };
        match rep {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names_attributes_and_lines() {
        let x = parse(
            "t.xml",
            b"\xEF\xBB\xBF<Context Font=\"A\">\n  <Button ID=\"B\"/>\n  <!-- c -->\n  <Label Text=\"x\"/>\n</Context>",
        )
        .unwrap();
        assert_eq!(x.name, "Context");
        assert_eq!(x.attr("Font", false), Some("A"));
        assert_eq!(x.children.len(), 2);
        assert_eq!(x.children[0].line, 2);
        assert_eq!(x.children[1].line, 4);
        assert_eq!(&*x.children[1].file, "t.xml");
    }

    #[test]
    fn attribute_case_rule() {
        let x = parse("t.xml", b"<A anchorside=\"O.O\"/>").unwrap();
        assert_eq!(x.attr("AnchorSide", false), None);
        assert_eq!(x.attr("AnchorSide", true), Some("O.O"));
    }

    #[test]
    fn broken_xml_is_an_error_with_a_line() {
        let e = parse("bad.xml", b"<A>\n<B>\n</A>").unwrap_err();
        assert_eq!(e.file, "bad.xml");
        assert!(e.line >= 2, "{e}");
    }

    #[test]
    fn declaration_only_file_is_an_empty_document() {
        let e = parse("e.xml", b"<?xml version=\"1.0\"?>").unwrap_err();
        assert!(e.empty);
    }

    #[test]
    fn tolerates_what_the_game_files_do() {
        // '<' in a value, no space between attributes, a repeated attribute: the first wins.
        let x = parse(
            "t.xml",
            b"<A><Label String=\"<\" Offset=\"1,2\" Offset=\"3,4\"/><B ID=\"x\"Anchor=\"L,B\"/></A>",
        )
        .unwrap();
        assert_eq!(x.children[0].attr("String", false), Some("<"));
        assert_eq!(x.children[0].attr("Offset", false), Some("1,2"));
        assert_eq!(x.children[1].attr("Anchor", false), Some("L,B"));
        let y = parse("t.xml", b"<A v=\"a&amp;b&#65;\"/>").unwrap();
        assert_eq!(y.attr("v", false), Some("a&bA"));
    }

    #[test]
    fn mismatched_and_unclosed_elements_are_errors() {
        assert!(parse("t.xml", b"<A><B></A>").is_err());
        assert!(parse("t.xml", b"<A><B>").is_err());
    }
}
