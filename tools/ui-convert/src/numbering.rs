//! Node numbering: the one rule the preview script, the package templates and the runtime's node labels share.
//!
//! A template is the document (everything below `<body>`; `<body>` itself is not a node) or the subtree of an
//! element with `data-bind-for` or `data-bind-if`; that element is node 0 of its own template, not a node of
//! the parent's. An element's number is its preorder index among the elements of its template. Templates are
//! numbered in the order their roots appear in the document, the document being 0. `<svg>` is a node, its
//! content is not; `script style template link meta` and everything under `data-ui2-dev` are not nodes. A text
//! leaf is `#k` after its parent element's label: the k-th text node the converter keeps as a leaf.
//!
//! The preview script writes the numbers into the page (`data-ui2-node`) before any binding runs; the runtime
//! names a node `<template>:<element index>` from the package template (`Node` kinds `Box`, `Layer`, `Image`,
//! `Input` count, `For`, `If` and text do not). The converter numbers the expanded page of the extract mode
//! (where every `if` is drawn and every `for` has exactly one instance, so the page has the shape of the
//! templates) by this rule and checks that the preview agrees, so a template built from the page carries the
//! numbers Chrome's rectangle dump uses.

use std::collections::BTreeMap;

use crate::errors::{ConvError, Place, codes};
use crate::ir::{Dom, Element, Ix};

/// Tags that are never nodes.
pub const NOT_NODES: &[&str] = &["script", "style", "template", "link", "meta"];

/// Whether an element is a template root.
pub fn is_template_root(e: &Element) -> bool {
    e.attr("data-bind-for").is_some() || e.attr("data-bind-if").is_some()
}

/// Whether an element (as a child of a node) is skipped by the numbering.
pub fn skipped(e: &Element) -> bool {
    NOT_NODES.contains(&e.tag.as_str()) || e.attr("data-ui2-dev").is_some()
}

/// The number of every element below `<body>`: IR id → `(template, index)`.
pub fn number(dom: &Dom) -> BTreeMap<u32, (u32, u32)> {
    let mut out = BTreeMap::new();
    let Some(body) = dom.body() else {
        return out;
    };
    let mut next_tpl = 1u32;
    let mut doc = 0u32;
    fn visit(
        e: &Element,
        tpl: u32,
        ctr: &mut u32,
        next_tpl: &mut u32,
        out: &mut BTreeMap<u32, (u32, u32)>,
    ) {
        out.insert(e.id, (tpl, *ctr));
        *ctr += 1;
        if e.is_svg() && e.tag == "svg" {
            return;
        }
        for c in e.elements() {
            if skipped(c) {
                continue;
            }
            if is_template_root(c) {
                let t = *next_tpl;
                *next_tpl += 1;
                visit(c, t, &mut 0, next_tpl, out);
            } else {
                visit(c, tpl, ctr, next_tpl, out);
            }
        }
    }
    for c in dom.elems[body].elements() {
        if skipped(c) {
            continue;
        }
        if is_template_root(c) {
            let t = next_tpl;
            next_tpl += 1;
            visit(c, t, &mut 0, &mut next_tpl, &mut out);
        } else {
            visit(c, 0, &mut doc, &mut next_tpl, &mut out);
        }
    }
    out
}

/// The label of a number.
pub fn label((tpl, idx): (u32, u32)) -> String {
    format!("{tpl}:{idx}")
}

/// Checks the preview's marks against [`number`]: every numbered element carries exactly its label, and no
/// other element carries one. A difference is a converter or preview bug (E9-005).
pub fn check(dom: &Dom) -> (BTreeMap<u32, (u32, u32)>, Vec<ConvError>) {
    let ours = number(dom);
    let mut errs = Vec::new();
    for (ix, e) in dom.elems.iter().enumerate() {
        let want = ours.get(&e.id).map(|n| label(*n));
        if e.mark != want {
            errs.push(mismatch(dom, ix, e.mark.as_deref(), want.as_deref()));
        }
    }
    (ours, errs)
}

fn mismatch(dom: &Dom, ix: Ix, got: Option<&str>, want: Option<&str>) -> ConvError {
    let e = dom.elems[ix];
    let place = if e.src.is_some() {
        e.place()
    } else {
        Place::default()
    };
    ConvError::new(
        codes::PREVIEW,
        place,
        format!(
            "the preview numbered the element {} where the converter numbers it {}",
            got.unwrap_or("-"),
            want.unwrap_or("-")
        ),
    )
    .element(e.describe())
}
