//! The plan: which nodes to read, under which conditions, at which viewports, with which token
//! beacons. It is computed from the IR alone; Chromium answers it (`extract`).
//!
//! - **Nodes.** Every element of the skin tree (first instance of each preview mark), plus `html`
//!   and `body` for the page checks.
//! - **Conditions `D(e)`.** A rule whose selector has dynamic parts (`:hover :active :focus
//!   :focus-visible :disabled`, or a class that a binding toggles) gives the matched element its
//!   conditions; each condition belongs to the element its compound matched (`up` = how many
//!   element levels above the node). The static parts are matched here on the IR: the profile's
//!   selectors need nothing a browser adds. When such a rule declares an inherited property, its
//!   conditions also reach every descendant (with a larger `up`). At most four per node.
//! - **Cells.** The media breakpoints split each viewport axis; a cell is read at the lower end
//!   of its interval (or one below the upper end, or 1920 x 1080 without breakpoints).
//! - **Variants.** Every non-empty mask of `D(e)` in every cell, read with the conditions forced.
//! - **Beacons.** The color custom properties of `:root` are replaced by marker colors in a second
//!   pass; a value that turns into a marker came from that token.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use ui2_package::profile::select::{self, Comb, Pseudo, SelInfo, StaticCompound};
use ui2_package::profile::{Profile, PropId, Tag, ValueKind};
use ui2_package::{Bound, Cond, CondKind, StrId, Styles};

use crate::check::{SHORTHANDS, Statics};
use crate::errors::ConvError;
use crate::ir::{Dom, Ix};

/// What a condition tests.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CondK {
    /// A dynamic pseudo-class.
    Pseudo(Pseudo),
    /// A class a binding toggles.
    Class(String),
}

/// One condition of a node.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CondSpec {
    /// 0 for the node, k for its k-th ancestor element.
    pub up: u8,
    /// What is tested.
    pub kind: CondK,
}

impl CondSpec {
    fn to_json(&self) -> Value {
        match &self.kind {
            CondK::Pseudo(p) => json!({ "up": self.up, "kind": p.name() }),
            CondK::Class(c) => json!({ "up": self.up, "kind": "class", "class": c }),
        }
    }
}

/// A node to read.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanNode {
    /// The element in the arena.
    pub ix: Ix,
    /// The IR id.
    pub id: u32,
    /// `html` or `body`: read for the page checks only.
    pub page: bool,
    /// The conditions, in mask bit order.
    pub conds: Vec<CondSpec>,
}

/// How a property is read in Chrome (spike: Typed OM keeps the computed form of the layout
/// properties; `getComputedStyle` is needed where Typed OM is wrong or truncated).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Api {
    /// `computedStyleMap().get(p).toString()`.
    Typed,
    /// `getComputedStyle(el).getPropertyValue(p)`.
    Computed,
}

/// The read API of a property.
pub fn api(p: PropId) -> Api {
    use PropId as P;
    match p {
        // Typed OM ignores border-style: none for the width; the transition lists are truncated
        // to their first item by `get()`, and so are the per-layer lists of background size, origin
        // and clip, and the implicit track lists of a grid (the explicit ones must stay Typed OM:
        // there `getComputedStyle` answers the used track sizes in px).
        P::BorderTopWidth
        | P::BackgroundSize
        | P::BackgroundOrigin
        | P::BackgroundClip
        | P::GridAutoColumns
        | P::GridAutoRows
        | P::BorderRightWidth
        | P::BorderBottomWidth
        | P::BorderLeftWidth
        | P::TransitionProperty
        | P::TransitionDuration
        | P::TransitionDelay
        | P::TransitionTimingFunction => Api::Computed,
        _ => Api::Typed,
    }
}

/// Whether a property may hold a token reference (read again in the beacon pass).
pub fn token_capable(p: PropId) -> bool {
    Profile::v1().def(p).token
}

/// The plan.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// Width breakpoints, ascending.
    pub media_w: Vec<Bound>,
    /// Height breakpoints, ascending.
    pub media_h: Vec<Bound>,
    /// The viewport of each cell, in cell order.
    pub cells: Vec<[u32; 2]>,
    /// The nodes, document order.
    pub nodes: Vec<PlanNode>,
    /// Custom properties of `:root` that may be color tokens, in declaration order.
    pub tokens: Vec<String>,
    /// Synonyms: `--ui-text` -> `--bone`.
    pub aliases: Vec<(String, String)>,
    /// Extra computed values the element table requires (`appearance`, `list-style-type`).
    pub extra: Vec<String>,
}

/// Whether a declared property is inherited (so a condition on its rule reaches descendants).
fn inherited(name: &str) -> bool {
    if name.starts_with("--") {
        return true;
    }
    let long = SHORTHANDS
        .iter()
        .find(|(s, _)| *s == name)
        .and_then(|(_, l)| l.first().copied())
        .unwrap_or(name);
    PropId::from_name(long).is_some_and(|p| Profile::v1().def(p).inherited)
}

struct Matcher<'a> {
    dom: &'a Dom<'a>,
    dyn_classes: &'a BTreeMap<String, BTreeSet<Ix>>,
}

impl Matcher<'_> {
    /// The conditions an element must meet to match one compound (its static part `c` and its dynamic
    /// pseudo-classes), or `None` when it cannot.
    fn compound(&self, c: &StaticCompound, pseudos: &[Pseudo], ix: Ix) -> Option<Vec<CondK>> {
        let e = self.dom.elems[ix];
        if let Some(tag) = c.tag
            && Tag::from_name(&e.tag.to_ascii_lowercase()) != Some(tag)
        {
            return None;
        }
        if c.root && (e.tag != "html" || e.is_svg()) {
            return None;
        }
        let id = e.attr("id");
        if c.ids.iter().any(|i| Some(i.as_str()) != id) {
            return None;
        }
        let statics = e.classes();
        let mut conds = Vec::new();
        for class in &c.classes {
            if statics.contains(&class.as_str()) {
                continue;
            }
            if self
                .dyn_classes
                .get(class)
                .is_some_and(|owners| owners.contains(&ix))
            {
                conds.push(CondK::Class(class.clone()));
            } else {
                return None;
            }
        }
        for &p in pseudos {
            if p == Pseudo::Disabled && !matches!(e.tag.as_str(), "button" | "input") {
                return None;
            }
            conds.push(CondK::Pseudo(p));
        }
        Some(conds)
    }

    /// Matches compounds `0..=k` with compound `k` on `ix`; returns the element and conditions of
    /// each compound (nearest ancestors first, the first solution).
    fn select(&self, s: &SelInfo, k: usize, ix: Ix) -> Option<Vec<(Ix, Vec<CondK>)>> {
        let pseudos: Vec<Pseudo> = s
            .conds
            .iter()
            .filter(|c| c.part == k)
            .map(|c| c.pseudo)
            .collect();
        let conds = self.compound(&s.static_part[k].1, &pseudos, ix)?;
        if k == 0 {
            return Some(vec![(ix, conds)]);
        }
        let candidates: Vec<Ix> = match s.static_part[k].0 {
            Some(Comb::Child) => self.dom.parent[ix].into_iter().collect(),
            _ => self.dom.ancestors(ix).collect(),
        };
        for a in candidates {
            if let Some(mut rest) = self.select(s, k - 1, a) {
                rest.push((ix, conds));
                return Some(rest);
            }
        }
        None
    }
}

/// Builds the plan. Errors: more than four conditions on a node (E3-005).
pub fn plan(dom: &Dom, statics: &Statics) -> (Plan, Vec<ConvError>) {
    let mut errs = Vec::new();
    let media_w = statics.cells.media_w.clone();
    let media_h = statics.cells.media_h.clone();
    let cell_list = statics.cells.viewports();
    debug_assert!({
        let s = Styles {
            media_w: media_w.clone(),
            media_h: media_h.clone(),
            ..Styles::default()
        };
        cell_list
            .iter()
            .enumerate()
            .all(|(i, v)| usize::from(s.cell([v[0] as f32, v[1] as f32])) == i)
    });

    // The nodes: the skin tree without template contents, scripts and styles; one instance per
    // preview mark.
    let mut ixs: Vec<(Ix, bool)> = Vec::new();
    let mut marks = BTreeSet::new();
    for (ix, e) in dom.elems.iter().enumerate() {
        if (e.tag == "html" || e.tag == "body") && !e.is_svg() {
            ixs.push((ix, true));
        }
    }
    for &ix in &statics.ui {
        let e = dom.elems[ix];
        if matches!(e.tag.as_str(), "script" | "style" | "template" | "link")
            || dom.ancestors(ix).any(|a| dom.elems[a].tag == "template")
        {
            continue;
        }
        if let Some(m) = &e.mark
            && !marks.insert(m.clone())
        {
            continue;
        }
        ixs.push((ix, false));
    }
    ixs.sort();
    ixs.dedup_by_key(|x| x.0);
    let wanted: BTreeSet<Ix> = ixs.iter().map(|x| x.0).collect();

    let matcher = Matcher {
        dom,
        dyn_classes: &statics.dyn_classes,
    };
    let mut d: BTreeMap<Ix, BTreeSet<CondSpec>> = BTreeMap::new();
    for rule in &statics.rules {
        let spreads = rule.props.iter().any(|p| inherited(p));
        for sel in &rule.selectors {
            // `input::placeholder` styles the placeholder text, not the element.
            if sel.placeholder || !sel.is_dynamic(&|k| statics.dyn_classes.contains_key(k)) {
                continue;
            }
            for &(ix, _) in &ixs {
                let Some(found) = matcher.select(sel, sel.static_part.len() - 1, ix) else {
                    continue;
                };
                let mut conds = Vec::new();
                for (owner, ks) in &found {
                    let up = dom.depth[ix].saturating_sub(dom.depth[*owner]);
                    for k in ks {
                        conds.push((up, k.clone()));
                    }
                }
                if conds.is_empty() {
                    continue;
                }
                let mut targets = vec![(ix, 0u32)];
                if spreads {
                    for &(dx, _) in &ixs {
                        if dom.ancestors(dx).any(|a| a == ix) {
                            targets.push((dx, dom.depth[dx] - dom.depth[ix]));
                        }
                    }
                }
                for (t, extra) in targets {
                    if !wanted.contains(&t) {
                        continue;
                    }
                    for (up, k) in &conds {
                        let up = (up + extra).min(255) as u8;
                        d.entry(t).or_default().insert(CondSpec {
                            up,
                            kind: k.clone(),
                        });
                    }
                }
            }
        }
    }

    let mut nodes = Vec::new();
    for (ix, page) in ixs {
        let mut conds: Vec<CondSpec> = if page {
            Vec::new()
        } else {
            d.remove(&ix).unwrap_or_default().into_iter().collect()
        };
        // The profile's limit on a node's conditions (a class stands in by its position: the conditions
        // are distinct already).
        let probe = conds.iter().enumerate().map(|(i, c)| Cond {
            up: c.up,
            kind: match &c.kind {
                CondK::Pseudo(p) => p.cond_kind(),
                CondK::Class(_) => CondKind::Class(StrId(i as u32)),
            },
        });
        if let Err(why) = select::node_conds(probe) {
            let list: Vec<String> = conds
                .iter()
                .map(|c| match &c.kind {
                    CondK::Pseudo(p) => format!(":{} (up {})", p.name(), c.up),
                    CondK::Class(k) => format!(".{k} (up {})", c.up),
                })
                .collect();
            let e = dom.elems[ix];
            errs.push(
                ConvError::new(
                    why.code,
                    e.place(),
                    format!("{}: {}", why.message, list.join(", ")),
                )
                .element(e.describe()),
            );
            conds.clear();
        }
        nodes.push(PlanNode {
            ix,
            id: dom.elems[ix].id,
            page,
            conds,
        });
    }

    // Token candidates and synonyms from the `:root` rules as written.
    let mut tokens = Vec::new();
    let mut aliases = Vec::new();
    let mut seen = BTreeSet::new();
    for v in &statics.root_vars {
        if !seen.insert(v.name.clone()) {
            continue;
        }
        let value = v.value.trim();
        let alias = value
            .strip_prefix("var(")
            .and_then(|r| r.strip_suffix(')'))
            .map(str::trim)
            .filter(|r| r.starts_with("--") && !r.contains([',', ' ', '(']));
        match alias {
            Some(target) => aliases.push((v.name.clone(), target.to_string())),
            None => tokens.push(v.name.clone()),
        }
    }

    let mut extra = BTreeSet::new();
    for &tag in ui2_package::Tag::ALL {
        if let Some(def) = Profile::v1().element(tag) {
            for k in def.require.keys() {
                extra.insert(if k == "list-style" {
                    "list-style-type".to_string()
                } else {
                    k.clone()
                });
            }
        }
    }

    let plan = Plan {
        media_w,
        media_h,
        cells: cell_list,
        nodes,
        tokens,
        aliases,
        extra: extra.into_iter().collect(),
    };
    (plan, errs)
}

impl Plan {
    /// The properties in `PropId` order with their read API.
    pub fn props() -> Vec<(PropId, Api)> {
        PropId::ALL.iter().map(|&p| (p, api(p))).collect()
    }

    /// The `extract` request for the Node side.
    pub fn request(&self) -> Value {
        let props: Vec<Value> = Self::props()
            .iter()
            .map(|(p, a)| {
                json!({ "name": p.name(), "api": if *a == Api::Typed { "T" } else { "C" } })
            })
            .collect();
        let beacon: Vec<usize> = PropId::ALL
            .iter()
            .enumerate()
            .filter(|(_, p)| token_capable(**p))
            .map(|(i, _)| i)
            .collect();
        let nodes: Vec<Value> = self
            .nodes
            .iter()
            .map(|n| {
                json!({
                    "id": n.id,
                    "conds": n.conds.iter().map(CondSpec::to_json).collect::<Vec<_>>(),
                })
            })
            .collect();
        json!({
            "cells": self.cells,
            "props": props,
            "beacon_props": beacon,
            "extra": self.extra,
            "tokens": self.tokens,
            "nodes": nodes,
        })
    }

    /// The number of (node, cell, mask) reads the plan asks for.
    pub fn reads(&self) -> usize {
        self.nodes
            .iter()
            .map(|n| self.cells.len() * select::variants(n.conds.len()))
            .sum()
    }
}

/// Whether a property's value is a color (for the beacon decoding).
pub fn color_kind(p: PropId) -> bool {
    Profile::v1().def(p).kind == ValueKind::Color
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(px: f32, upper_inclusive: bool) -> Bound {
        Bound {
            px,
            upper_inclusive,
        }
    }

    use ui2_package::profile::media::{self, Cells};

    /// The cells of the media preludes of a skin, through the profile's parser.
    fn cells(preludes: &[&str]) -> (Vec<Bound>, Vec<Bound>, Vec<[u32; 2]>) {
        let conds: Vec<_> = preludes
            .iter()
            .flat_map(|p| media::parse(p).expect("a media query of the profile"))
            .collect();
        let c = Cells::new(&conds).expect("cells");
        (c.media_w.clone(), c.media_h.clone(), c.viewports())
    }

    #[test]
    fn styles_cells_without_breakpoints_are_one_full_hd_cell() {
        let (w, h, c) = cells(&[]);
        assert!(w.is_empty() && h.is_empty());
        assert_eq!(c, [[1920, 1080]]);
    }

    #[test]
    fn styles_cells_take_the_lower_end_of_each_interval() {
        let (w, h, c) = cells(&[
            "(min-width: 1600px)",
            "screen and (width >= 1280px)",
            "(max-height: 800px)",
            "(min-width: 1600px)",
        ]);
        assert_eq!(w, [b(1280.0, false), b(1600.0, false)]);
        assert_eq!(h, [b(800.0, true)]);
        assert_eq!(
            c,
            [
                [1279, 800],
                [1279, 801],
                [1280, 800],
                [1280, 801],
                [1600, 800],
                [1600, 801]
            ]
        );
        let s = Styles {
            media_w: w,
            media_h: h,
            ..Styles::default()
        };
        for (i, v) in c.iter().enumerate() {
            assert_eq!(usize::from(s.cell([v[0] as f32, v[1] as f32])), i);
        }
    }

    #[test]
    fn styles_inherited_properties_spread_conditions() {
        assert!(inherited("color"));
        assert!(inherited("font"));
        assert!(inherited("--x"));
        assert!(!inherited("background-color"));
        assert!(!inherited("padding"));
    }
}
