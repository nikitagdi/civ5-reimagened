//! The page scan of the Node side (`node/ir.mjs`), as Rust types.
//!
//! The IR is the DOM as Chrome built it (elements and text nodes in document order, ids in that
//! order, the source position of each start tag) and the stylesheets as reduced css-tree ASTs with
//! positions. This module only reads it; it adds an element arena with parent links for the
//! checks and the selector matching.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::errors::Place;

/// The scan of one page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Ir {
    /// IR format version.
    pub version: u32,
    /// The sample state the page showed.
    #[serde(default)]
    pub state: String,
    /// The entry file.
    pub entry: String,
    /// The viewport of the scan.
    pub viewport: [u32; 2],
    /// `document.compatMode`.
    #[serde(default)]
    pub compat: String,
    /// `document.title`.
    #[serde(default)]
    pub title: String,
    /// The `html` element.
    pub dom: Node,
    /// Stylesheets in document order.
    #[serde(default)]
    pub sheets: Vec<Sheet>,
    /// `document.fonts`.
    #[serde(default)]
    pub fonts: Vec<FontFace>,
    /// Requests the local server answered.
    #[serde(default)]
    pub resources: Vec<Resource>,
    /// Problems found while loading (E9).
    #[serde(default)]
    pub errors: Vec<IrError>,
}

/// An anonymous box a node belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Anon {
    /// `flex`, `grid` or `block`.
    pub kind: AnonKind,
    /// The run number among the anonymous runs of the parent.
    pub run: u32,
}

/// The kind of anonymous box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnonKind {
    /// Text directly in a flex container.
    Flex,
    /// Text directly in a grid container.
    Grid,
    /// Inline content between block siblings.
    Block,
}

/// A DOM node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Node {
    /// An element.
    Element(Element),
    /// A text node.
    Text(Text),
}

impl Node {
    /// The node id.
    pub fn id(&self) -> u32 {
        match self {
            Node::Element(e) => e.id,
            Node::Text(t) => t.id,
        }
    }
}

/// An element.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Element {
    /// Document-order id.
    pub id: u32,
    /// `localName`.
    pub tag: String,
    /// `"svg"` for SVG elements.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ns: Option<String>,
    /// `<file>:<line>:<col>` of the start tag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src: Option<String>,
    /// The preview's node mark `<tpl>:<idx>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark: Option<String>,
    /// The preview's instance keys.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inst: Option<String>,
    /// The id of the tooltip template this element is the root of (the preview's extract mode draws the root in
    /// place of its `<template data-layer="tooltip">`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tooltip: Option<String>,
    /// The attributes in DOM order (internal ones removed).
    #[serde(default)]
    pub attrs: Vec<(String, String)>,
    /// The natural size of an `img`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub natural: Option<[u32; 2]>,
    /// The anonymous box this element sits in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anon: Option<Anon>,
    /// Children.
    #[serde(default)]
    pub children: Vec<Node>,
}

impl Element {
    /// The value of an attribute.
    pub fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// Whether the element is an SVG element.
    pub fn is_svg(&self) -> bool {
        self.ns.as_deref() == Some("svg")
    }

    /// The static classes.
    pub fn classes(&self) -> Vec<&str> {
        self.attr("class")
            .map(|c| c.split_ascii_whitespace().collect())
            .unwrap_or_default()
    }

    /// The source place of the start tag.
    pub fn place(&self) -> Place {
        self.src
            .as_deref()
            .and_then(Place::parse_src)
            .unwrap_or_default()
    }

    /// A short description: `<span class="t">`.
    pub fn describe(&self) -> String {
        let mut s = format!("<{}", self.tag);
        for name in ["id", "class", "data-part"] {
            if let Some(v) = self.attr(name) {
                s.push_str(&format!(" {name}=\"{v}\""));
            }
        }
        s.push('>');
        s
    }

    /// Child elements.
    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|c| match c {
            Node::Element(e) => Some(e),
            Node::Text(_) => None,
        })
    }
}

/// A text node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Text {
    /// Document-order id.
    pub id: u32,
    /// The text as the DOM holds it (NBSP stays U+00A0).
    pub text: String,
    /// Only ASCII whitespace.
    #[serde(default)]
    pub ws: bool,
    /// The anonymous box this text sits in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anon: Option<Anon>,
}

/// A stylesheet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sheet {
    /// `link` or `style`.
    pub kind: String,
    /// The element id.
    pub node: u32,
    /// The element's source place.
    #[serde(default)]
    pub src: Option<String>,
    /// `href` of a link.
    #[serde(default)]
    pub href: Option<String>,
    /// The file of a link, relative to the skin.
    #[serde(default)]
    pub file: Option<String>,
    /// The link points outside (a scheme or `//`).
    #[serde(default)]
    pub external: bool,
    /// The link's file does not exist.
    #[serde(default)]
    pub missing: bool,
    /// Top-level items.
    #[serde(default)]
    pub rules: Vec<CssItem>,
    /// css-tree parse errors.
    #[serde(default, rename = "parseErrors")]
    pub parse_errors: Vec<CssParseError>,
}

impl Sheet {
    /// The file the positions of this sheet refer to.
    pub fn source_file(&self) -> String {
        if let Some(f) = &self.file {
            return f.clone();
        }
        self.src
            .as_deref()
            .and_then(Place::parse_src)
            .map(|p| p.file)
            .unwrap_or_default()
    }
}

/// A css-tree parse error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CssParseError {
    /// The message.
    pub message: String,
    /// Line, 1-based.
    #[serde(default)]
    pub line: u32,
    /// Column, 1-based.
    #[serde(default)]
    pub col: u32,
}

/// An item of a stylesheet or a block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum CssItem {
    /// A style rule.
    Rule {
        /// Line.
        #[serde(default)]
        line: u32,
        /// Column.
        #[serde(default)]
        col: u32,
        /// The whole prelude.
        selector: String,
        /// The selectors of the list.
        selectors: Vec<String>,
        /// The declarations (and anything else in the block).
        items: Vec<CssItem>,
    },
    /// An at-rule.
    Atrule {
        /// The name without `@`.
        name: String,
        /// The prelude as css-tree generates it.
        #[serde(default)]
        prelude: String,
        /// Line.
        #[serde(default)]
        line: u32,
        /// Column.
        #[serde(default)]
        col: u32,
        /// The block, if any.
        #[serde(default)]
        items: Option<Vec<CssItem>>,
    },
    /// A declaration.
    Decl {
        /// The property, as written (lower case for standard ones).
        prop: String,
        /// The value as css-tree generates it.
        value: String,
        /// `!important`.
        #[serde(default)]
        important: bool,
        /// Line.
        #[serde(default)]
        line: u32,
        /// Column.
        #[serde(default)]
        col: u32,
    },
    /// Anything else (raw text css-tree could not parse).
    #[serde(other)]
    Other,
}

/// A font face of `document.fonts`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FontFace {
    /// The family.
    pub family: String,
    /// `normal` / `italic`.
    pub style: String,
    /// The weight or weight range.
    pub weight: String,
    /// The load status.
    pub status: String,
}

/// A request the server answered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Resource {
    /// The URL path.
    pub path: String,
    /// The HTTP status.
    pub status: u16,
}

/// A load problem.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IrError {
    /// The code.
    pub code: String,
    /// The message.
    pub message: String,
    /// The URL.
    #[serde(default)]
    pub url: Option<String>,
}

/// Index of an element in the [`Dom`] arena.
pub type Ix = usize;

/// The elements of the page in document order with parent links.
#[derive(Debug, Clone)]
pub struct Dom<'a> {
    /// The elements, document order.
    pub elems: Vec<&'a Element>,
    /// The parent element of each.
    pub parent: Vec<Option<Ix>>,
    /// The depth (html = 0).
    pub depth: Vec<u32>,
    /// The element of each IR id.
    pub by_id: BTreeMap<u32, Ix>,
}

impl<'a> Dom<'a> {
    /// Builds the arena.
    pub fn new(root: &'a Node) -> Dom<'a> {
        let mut d = Dom {
            elems: Vec::new(),
            parent: Vec::new(),
            depth: Vec::new(),
            by_id: BTreeMap::new(),
        };
        if let Node::Element(e) = root {
            d.add(e, None, 0);
        }
        d
    }

    fn add(&mut self, e: &'a Element, parent: Option<Ix>, depth: u32) {
        let ix = self.elems.len();
        self.elems.push(e);
        self.parent.push(parent);
        self.depth.push(depth);
        self.by_id.insert(e.id, ix);
        for c in e.elements() {
            self.add(c, Some(ix), depth + 1);
        }
    }

    /// The `k`-th ancestor element (`k = 0` is the element).
    pub fn ancestor(&self, ix: Ix, k: u32) -> Option<Ix> {
        let mut cur = ix;
        for _ in 0..k {
            cur = self.parent[cur]?;
        }
        Some(cur)
    }

    /// The ancestors from the parent up.
    pub fn ancestors(&self, ix: Ix) -> impl Iterator<Item = Ix> + '_ {
        std::iter::successors(self.parent[ix], move |&p| self.parent[p])
    }

    /// The `body` element.
    pub fn body(&self) -> Option<Ix> {
        self.elems
            .iter()
            .position(|e| e.tag == "body" && !e.is_svg())
    }

    /// Whether `ix` is inside an element with `data-ui2-dev` (or is one).
    pub fn is_dev(&self, ix: Ix) -> bool {
        std::iter::once(ix)
            .chain(self.ancestors(ix))
            .any(|a| self.elems[a].attr("data-ui2-dev").is_some())
    }

    /// The element's position and description, for the `at:` line of an error.
    pub fn at(&self, ix: Ix) -> String {
        let e = self.elems[ix];
        format!("{} {}", e.place(), e.describe())
    }
}

/// Every text node below `e` (document order), with the element that holds it.
pub fn texts(e: &Element) -> Vec<(&Element, &Text)> {
    let mut out = Vec::new();
    fn walk<'a>(e: &'a Element, out: &mut Vec<(&'a Element, &'a Text)>) {
        for c in &e.children {
            match c {
                Node::Element(k) => walk(k, out),
                Node::Text(t) => out.push((e, t)),
            }
        }
    }
    walk(e, &mut out);
    out
}
