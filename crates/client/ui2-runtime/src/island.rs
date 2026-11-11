//! Script islands: parts of the page that markup does not bind and a script drives by key.
//!
//! An island (`data-ui2-island="<id>"`) is a subtree of a template; its keyed nodes (`data-ui2-key`) are
//! named in the package (`EXTN`). A script command ([`IslandCmd`]) addresses one keyed node by island and key
//! and does one thing to every live instance of it: replaces its text, switches a run-time class, sets a
//! bindable style property, or hides and shows it. The node is marked for exactly the passes the change
//! needs, so a command touches only its node's chunk, and nothing happens between commands.
//!
//! A command to a node that bindings drive (a bound text, a class toggle, a style binding, ...) is refused:
//! markup owns that node. Every refused command is an error with the island and key, logged once.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use ui2_model::Value as MValue;
use ui2_model::wire::{IslandCmd, IslandOp, ScriptCtxId};
use ui2_package::style::Display;
use ui2_package::{BindKind, NodeKind, NodeRef, PropId, TextSrc};

use crate::arena::{Dirty, Node, NodeClass, NodeId};
use crate::error::{ErrorCode, UiError};
use crate::style::{self, OverValue};
use crate::{Runtime, bind, instantiate};

/// Where the keyed nodes of the islands live: template node → live nodes, rebuilt after structural changes
/// (only when a command arrives).
#[derive(Debug, Default)]
pub(crate) struct IslandIndex {
    /// The arena revision the map was built at.
    revision: Option<u64>,
    nodes: HashMap<NodeRef, Vec<NodeId>>,
}

/// One island as the runtime uses it: its root and its keyed nodes by key text.
#[derive(Debug, Clone)]
pub(crate) struct IslandDef {
    pub keys: Vec<(String, NodeRef)>,
}

/// The text a command value shows.
fn shown_text(rt: &Runtime, v: &MValue) -> String {
    match v {
        MValue::None => String::new(),
        MValue::Bool(b) => b.to_string(),
        MValue::I32(i) => i.to_string(),
        MValue::U32(u) => u.to_string(),
        MValue::F32(f) => f.to_string(),
        MValue::Str(s) => s.to_string(),
        MValue::Text(t) => rt.binder.loc.model_text(&rt.pkg, t),
        MValue::Asset(a) => a.0.to_string(),
        MValue::Enum(e) => e.to_string(),
        MValue::Key(k) => k.to_js(),
        MValue::Struct(_) | MValue::List(_) => String::new(),
    }
}

/// A CSS property name as a script may write it (`backgroundColor` or `background-color`).
fn prop_of(name: &str) -> Option<PropId> {
    let mut kebab = String::with_capacity(name.len() + 4);
    for c in name.trim().chars() {
        if c.is_ascii_uppercase() {
            kebab.push('-');
            kebab.push(c.to_ascii_lowercase());
        } else {
            kebab.push(c);
        }
    }
    PropId::from_name(&kebab)
}

/// Whether bindings drive a template node (scripts must not touch it).
fn bound(pkg: &crate::Pkg, r: NodeRef) -> bool {
    pkg.bindings_of(r.0, r.1).iter().any(|b| {
        matches!(
            b.kind,
            BindKind::Text(_)
                | BindKind::ClassToggle { .. }
                | BindKind::Style { .. }
                | BindKind::Src(_)
                | BindKind::Disabled(_)
                | BindKind::Model(_)
                | BindKind::Anchor { .. }
        )
    })
}

impl Runtime {
    /// The island `name` of a script context: the skin's islands for the skin, a mod's fragment islands for
    /// the mod.
    fn island_def(&self, ctx: &ScriptCtxId, name: &str) -> Option<IslandDef> {
        self.pkg.island(ctx, name)
    }

    /// The live nodes made from a template node (all instances), from the index.
    fn island_nodes(&mut self, r: NodeRef) -> Vec<NodeId> {
        let rev = self.arena.revision();
        if self.islands.revision != Some(rev) {
            let keyed: HashSet<NodeRef> = self.pkg.island_nodes().into_iter().collect();
            let mut nodes: HashMap<NodeRef, Vec<NodeId>> = HashMap::new();
            for id in self.arena.ids() {
                if let Some(n) = self.arena.get(id)
                    && n.class() != NodeClass::Text
                    && keyed.contains(&n.tmpl())
                {
                    nodes.entry(n.tmpl()).or_default().push(id);
                }
            }
            self.islands.nodes = nodes;
            self.islands.revision = Some(rev);
        }
        self.islands.nodes.get(&r).cloned().unwrap_or_default()
    }

    /// Runs script commands on island nodes (step 5 of the host's frame, after
    /// [`apply_delta`](Self::apply_delta)): each marks its node for the passes it needs; the next
    /// [`frame`](Self::frame) does the work. Returns the errors of refused commands, each `(island and key,
    /// kind)` once per runtime: a command repeated every frame is reported the first time only.
    pub fn apply_island(&mut self, ctx: &ScriptCtxId, cmds: &[IslandCmd]) -> Vec<UiError> {
        let mut out = Vec::new();
        for cmd in cmds {
            if let Err(msg) = self.island_cmd(ctx, cmd) {
                let who = match ctx {
                    ScriptCtxId::Skin => "skin".to_string(),
                    ScriptCtxId::Mod(id) => format!("mod {id}"),
                };
                let op = match &cmd.op {
                    IslandOp::Text(_) => "text".to_string(),
                    IslandOp::Cls(c, _) => format!("cls {c}"),
                    IslandOp::Style(p, _) => format!("style {p}"),
                    IslandOp::Show(_) => "show".to_string(),
                };
                // One report per island node, command kind and argument name.
                let e = UiError {
                    path: format!("island {}#{} ({who}) {op}", cmd.island, cmd.key),
                    panel: None,
                    code: ErrorCode::Island,
                    msg,
                };
                if self.errors.note(&e) {
                    out.push(e);
                }
            }
        }
        out
    }

    fn island_cmd(&mut self, ctx: &ScriptCtxId, cmd: &IslandCmd) -> Result<(), String> {
        let def = self
            .island_def(ctx, &cmd.island)
            .ok_or_else(|| format!("there is no island {}", cmd.island))?;
        let r = def
            .keys
            .iter()
            .find(|(k, _)| **k == *cmd.key)
            .map(|(_, r)| *r)
            .ok_or_else(|| format!("island {} has no key {}", cmd.island, cmd.key))?;
        if bound(&self.pkg, r) {
            return Err("the node is bound by markup; scripts cannot change it".into());
        }
        // An island that is not on the page now (a closed `if`, an empty list) takes the command as done.
        for id in self.island_nodes(r) {
            self.island_op(id, &cmd.op)?;
        }
        Ok(())
    }

    fn island_op(&mut self, id: NodeId, op: &IslandOp) -> Result<(), String> {
        match op {
            IslandOp::Text(v) => {
                let text: Arc<str> = shown_text(self, v).into();
                let leaf = match bind::text_target(&self.arena, id) {
                    Some(leaf) => {
                        let kind = self
                            .arena
                            .get(leaf)
                            .and_then(|n| instantiate::template_node(&*self.pkg, n.tmpl()))
                            .map(|t| t.kind.clone());
                        if matches!(
                            kind,
                            Some(NodeKind::Text(TextSrc::Bound(_) | TextSrc::L10n { .. }))
                        ) {
                            return Err(
                                "the text is bound by markup; scripts cannot change it".into()
                            );
                        }
                        leaf
                    }
                    None => self.text_leaf(id).ok_or("the node cannot hold text")?,
                };
                bind::set_text(&mut self.arena, leaf, &text);
                Ok(())
            }
            IslandOp::Cls(name, on) => {
                // A class no style condition of the package tests changes nothing.
                if let Some(class) = self.pkg.str_id(name) {
                    style::select::set_class(&mut self.arena, id, class, *on);
                }
                Ok(())
            }
            IslandOp::Style(prop, v) => {
                let p = prop_of(prop).ok_or_else(|| format!("{prop} is not a property"))?;
                if !style::bindable(p) {
                    return Err(format!("{} cannot be set by a script", p.name()));
                }
                let text = shown_text(self, v);
                let value = if text.trim().is_empty() {
                    None
                } else {
                    Some(
                        bind::style::parse(&self.pkg, p, &text)
                            .ok_or_else(|| format!("{text:?} is not a value of {}", p.name()))?,
                    )
                };
                self.override_prop(id, p, value);
                Ok(())
            }
            IslandOp::Show(on) => {
                let value = (!on).then_some(OverValue::Display(Display::None));
                self.override_prop(id, PropId::Display, value);
                Ok(())
            }
        }
    }

    /// Sets or clears a property over a node's record and marks the passes the change needs.
    pub(crate) fn override_prop(&mut self, id: NodeId, prop: PropId, value: Option<OverValue>) {
        let bits = style::set_override(
            &mut self.arena,
            self.pkg.styles(),
            &self.style_index,
            id,
            prop,
            value,
        );
        self.arena.mark(id, bits);
    }

    /// Gives an empty element a text leaf of its own (styled like it, as a template's text leaf is), for a
    /// script to write into. `None` for a node that cannot hold text (a picture, a text field, a list).
    fn text_leaf(&mut self, id: NodeId) -> Option<NodeId> {
        let n = self.arena.get(id)?;
        let t = instantiate::template_node(&*self.pkg, n.tmpl())?;
        if !matches!(t.kind, NodeKind::Box { .. }) || n.class() != NodeClass::Element {
            return None;
        }
        let mut leaf = Node::new(n.tmpl(), NodeClass::Text, Some(id));
        leaf.set = n.set;
        leaf.chunk = n.chunk;
        leaf.text = Some(Arc::from(""));
        let leaf = self.arena.alloc(leaf);
        self.arena.attach(Some(id), usize::MAX, leaf);
        style::attach(&mut self.arena, self.pkg.styles(), &self.style_env, leaf);
        self.arena
            .mark(leaf, Dirty::TEXT | Dirty::LAYOUT | Dirty::PAINT);
        self.arena.mark(id, Dirty::TEXT | Dirty::LAYOUT);
        self.arena.bump_revision();
        Some(leaf)
    }
}
