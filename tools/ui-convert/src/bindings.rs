//! Bindings: every attribute of the profile's grammar typed against the model schema (through
//! `SchemaView`) in the scope the element sits in (loop variables of the enclosing
//! `data-bind-for`, `state.*` from `skin.toml [state]`). A path the schema does not have, a loop
//! over something that is not a list, an intent argument of the wrong type... are E4 errors at the
//! element's source position. The checked attributes keep what the schema decided (named intent
//! arguments, a loop's key) for the package writer.
//!
//! The samples (`sample/<state>.json`) are checked here too: they must fit the schema (E4-040)
//! and use only the skin's state (E4-041).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use ui2_model::{Sample, Schema};
use ui2_package::profile::action::Action;
use ui2_package::profile::attrs::codes::NO_KEY;
use ui2_package::profile::attrs::{self, AttrCtx, Checked, NodeAttrs};
use ui2_package::profile::expr::codes::NOT_LIST;
use ui2_package::profile::expr::{Expr, Func, Scope, TplSeg};
use ui2_package::profile::{SchemaView, TypeSig};
use ui2_package::sections::templates::Layer;

use crate::check::Statics;
use crate::errors::{ConvError, Place, codes};
use crate::ir::{Dom, Ix};
use crate::skin::Skin;

/// One element's checked bindings.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeBinding {
    /// The IR id.
    pub id: u32,
    /// The attributes, with named intent arguments and derived loop keys.
    pub attrs: NodeAttrs,
    /// What checking found out (paths, intents, handlers, the children's scope).
    pub checked: Checked,
}

/// A text key used by the markup, with where.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct KeyUse {
    /// The key.
    pub key: String,
    /// The place of the element (or `skin.toml`).
    pub place: Place,
    /// The element, for the message.
    pub element: String,
    /// How many arguments the use passes (`None`: not known statically).
    pub args: Option<usize>,
}

/// Everything the bindings of a skin read, send and show.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Bound {
    /// Checked elements, by IR id (elements with no profile attribute are included too).
    pub nodes: BTreeMap<u32, NodeBinding>,
    /// Model paths with the types the skin expects.
    pub paths: BTreeMap<String, TypeSig>,
    /// Intents with their argument types.
    pub intents: BTreeMap<String, Vec<TypeSig>>,
    /// Script handlers named by `call`.
    pub handlers: BTreeSet<String>,
    /// Text keys the markup uses.
    pub keys: Vec<KeyUse>,
    /// Literal text the bindings write (template literals), for the font subsets.
    pub literals: BTreeSet<String>,
}

/// The skin's state as the expressions see it.
pub fn state_scope(skin: &Skin) -> Scope {
    let mut scope = Scope::new();
    for (path, v) in &skin.state {
        scope = scope.with_state(path, v.type_sig());
    }
    scope
}

fn exprs_of(a: &NodeAttrs) -> Vec<&Expr> {
    let mut out: Vec<&Expr> = Vec::new();
    if let Some(t) = &a.value {
        for s in &t.segs {
            if let TplSeg::Expr(e) = s {
                out.push(e);
            }
        }
    }
    out.extend(a.cond.iter());
    if let Some(f) = &a.for_ {
        out.push(&f.list);
        out.extend(f.key.iter());
        out.extend(f.sort.iter());
        out.extend(f.sort_desc.iter());
        out.extend(f.filter.iter());
    }
    out.extend(a.class_toggle.iter().map(|c| &c.cond));
    for s in &a.style {
        for seg in &s.tpl.segs {
            if let TplSeg::Expr(e) = seg {
                out.push(e);
            }
        }
    }
    out.extend(a.src.iter());
    out.extend(a.disabled.iter());
    out.extend(a.l10n_args.iter());
    if let Some((_, e)) = &a.anchor {
        out.push(e);
    }
    if let Some(c) = &a.control {
        out.extend(c.options.iter());
    }
    for ev in &a.events {
        for act in &ev.actions {
            match act {
                Action::Intent { args, .. } | Action::Call { args, .. } => {
                    out.extend(args.iter().map(|x| &x.value));
                }
                Action::Set { value, .. } => out.push(value),
                Action::Toggle { .. } | Action::Stop => {}
            }
        }
    }
    out
}

/// `loc('key', args...)` calls inside an expression: (key, argument count).
pub fn loc_calls(e: &Expr, out: &mut Vec<(String, usize)>) {
    match e {
        Expr::Call(Func::Loc, args) => {
            if let Some(Expr::Str(k)) = args.first() {
                out.push((k.clone(), args.len() - 1));
            }
            for a in args {
                loc_calls(a, out);
            }
        }
        Expr::Call(_, args) => args.iter().for_each(|a| loc_calls(a, out)),
        Expr::Unary(_, a) => loc_calls(a, out),
        Expr::Binary(_, a, b) => {
            loc_calls(a, out);
            loc_calls(b, out);
        }
        Expr::Cond(a, b, c) => {
            loc_calls(a, out);
            loc_calls(b, out);
            loc_calls(c, out);
        }
        _ => {}
    }
}

fn ctx_of(dom: &Dom, ix: Ix) -> AttrCtx {
    let e = dom.elems[ix];
    let mut ctx = AttrCtx {
        has_children: e.elements().next().is_some(),
        ..AttrCtx::default()
    };
    for a in dom.ancestors(ix) {
        let ae = dom.elems[a];
        ctx.in_for |= ae.attr("data-bind-for").is_some();
        ctx.in_island |= ae.attr("data-ui2-island").is_some();
    }
    ctx.layer = std::iter::once(ix).chain(dom.ancestors(ix)).find_map(|a| {
        match dom.elems[a].attr("data-layer") {
            Some("map") => Some(Layer::Map),
            Some("hud") => Some(Layer::Hud),
            Some("window") => Some(Layer::Window),
            Some("tooltip") => Some(Layer::Tooltip),
            _ => None,
        }
    });
    ctx
}

/// Types every element's bindings. `view` is the model schema.
pub fn check(
    dom: &Dom,
    statics: &Statics,
    skin: &Skin,
    view: &dyn SchemaView,
) -> (Bound, Vec<ConvError>) {
    let mut out = Bound::default();
    let mut errs = Vec::new();
    let ui: BTreeSet<Ix> = statics.ui.iter().copied().collect();
    let mut kids: BTreeMap<Ix, Vec<Ix>> = BTreeMap::new();
    for &ix in &statics.ui {
        if let Some(p) = dom.parent[ix] {
            kids.entry(p).or_default().push(ix);
        }
    }
    let Some(body) = dom.body() else {
        return (out, errs);
    };
    let base = state_scope(skin);
    let mut marks = BTreeSet::new();
    // Depth-first with the scope of each element's children.
    let mut stack: Vec<(Ix, Scope)> = kids
        .get(&body)
        .map(|v| v.iter().rev().map(|&k| (k, base.clone())).collect())
        .unwrap_or_default();
    while let Some((ix, scope)) = stack.pop() {
        if !ui.contains(&ix) {
            continue;
        }
        let e = dom.elems[ix];
        if let Some(m) = &e.mark
            && !marks.insert(m.clone())
        {
            // Another instance of a loop body: its template is checked once.
            continue;
        }
        let Some(parsed) = statics.attrs.get(&ix) else {
            // The grammar failed (already reported); the children are checked in the same scope.
            push_children(&mut stack, &kids, ix, &scope);
            continue;
        };
        let mut attrs = parsed.clone();
        let ctx = ctx_of(dom, ix);
        // A template's content starts a new scope (tooltips): only the skin state.
        let inner_base = if e.tag == "template" { &base } else { &scope };
        match attrs::check(&mut attrs, inner_base, view, &ctx) {
            Ok(checked) => {
                for (p, t) in &checked.paths {
                    out.paths.insert(p.clone(), t.clone());
                }
                for (i, a) in &checked.intents {
                    out.intents.insert(i.clone(), a.clone());
                }
                out.handlers.extend(checked.handlers.iter().cloned());
                collect_keys(&mut out, &attrs, e.place(), e.describe());
                push_children(&mut stack, &kids, ix, &checked.scope);
                out.nodes.insert(
                    e.id,
                    NodeBinding {
                        id: e.id,
                        attrs,
                        checked,
                    },
                );
            }
            Err(list) => {
                // A loop over something that is not a list has no key either: that follows from the
                // first error and is not reported again.
                let not_list = list.iter().any(|pe| pe.code == NOT_LIST);
                for pe in list {
                    if not_list && pe.code == NO_KEY {
                        continue;
                    }
                    let mut why = pe.message.clone();
                    if let Some(at) = pe.at {
                        why.push_str(&format!(" (at byte {at} of the value)"));
                    }
                    errs.push(
                        ConvError::new(pe.code, e.place(), why)
                            .element(e.describe())
                            .prop(pe.attr.clone()),
                    );
                }
                // A loop whose scope could not be built: its body would only repeat the error.
                if attrs.for_.is_none() {
                    push_children(&mut stack, &kids, ix, inner_base);
                }
            }
        }
    }
    // The title of the skin is a text key too.
    out.keys.push(KeyUse {
        key: skin.skin.title.clone(),
        place: Place::file("skin.toml"),
        element: "[skin] title".to_string(),
        args: Some(0),
    });
    out.keys.sort();
    (out, errs)
}

fn push_children(
    stack: &mut Vec<(Ix, Scope)>,
    kids: &BTreeMap<Ix, Vec<Ix>>,
    ix: Ix,
    scope: &Scope,
) {
    if let Some(v) = kids.get(&ix) {
        for &k in v.iter().rev() {
            stack.push((k, scope.clone()));
        }
    }
}

fn collect_keys(out: &mut Bound, a: &NodeAttrs, place: Place, element: String) {
    if let Some(k) = &a.l10n {
        out.keys.push(KeyUse {
            key: k.clone(),
            place: place.clone(),
            element: element.clone(),
            args: Some(a.l10n_args.len()),
        });
    }
    if let Some(k) = &a.l10n_placeholder {
        out.keys.push(KeyUse {
            key: k.clone(),
            place: place.clone(),
            element: element.clone(),
            args: Some(0),
        });
    }
    let mut calls = Vec::new();
    for e in exprs_of(a) {
        loc_calls(e, &mut calls);
    }
    for (key, n) in calls {
        out.keys.push(KeyUse {
            key,
            place: place.clone(),
            element: element.clone(),
            args: Some(n),
        });
    }
    let mut lits = |t: &ui2_package::profile::expr::Template| {
        for s in &t.segs {
            if let TplSeg::Lit(l) = s {
                out.literals.insert(l.clone());
            }
        }
    };
    if let Some(t) = &a.value {
        lits(t);
    }
}

/// Checks `sample/*.json` against the schema and the skin's state.
pub fn check_samples(
    skin_dir: &Path,
    skin: &Skin,
    schema: &Schema,
) -> (Vec<String>, Vec<ConvError>) {
    let mut errs = Vec::new();
    let mut texts = Vec::new();
    let dir = skin_dir.join("sample");
    let mut files: Vec<String> = std::fs::read_dir(&dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| n.ends_with(".json"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    for state in &skin.verify.states {
        if !files.contains(&format!("{state}.json")) {
            errs.push(ConvError::new(
                codes::SAMPLE,
                Place::file("skin.toml"),
                format!("[verify] states names `{state}`, but sample/{state}.json does not exist"),
            ));
        }
    }
    for f in files {
        let rel = format!("sample/{f}");
        let text = match std::fs::read_to_string(dir.join(&f)) {
            Ok(t) => t.replace("\r\n", "\n"),
            Err(e) => {
                errs.push(ConvError::new(
                    codes::SAMPLE,
                    Place::file(&rel),
                    e.to_string(),
                ));
                continue;
            }
        };
        match Sample::parse(schema, &text) {
            Ok(sample) => {
                for key in sample.state().keys() {
                    let known = skin
                        .state
                        .keys()
                        .any(|p| p == key || p.starts_with(&format!("{key}.")));
                    if !known {
                        errs.push(
                            ConvError::new(
                                codes::SAMPLE_STATE,
                                Place::file(&rel),
                                format!("`state.{key}` is not declared in skin.toml [state]"),
                            )
                            .prop(format!("state.{key}")),
                        );
                    }
                }
                texts.push(text);
            }
            Err(list) => {
                for m in list {
                    errs.push(ConvError::new(codes::SAMPLE, Place::file(&rel), m));
                }
            }
        }
    }
    (texts, errs)
}
