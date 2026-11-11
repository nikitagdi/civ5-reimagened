//! Making nodes from package templates, and naming them.
//!
//! A template is a preorder node list with subtree sizes. [`instantiate`] walks it once, allocates one arena
//! node per template node, links parents and children, selects each node's style record and queues the new
//! nodes for layout, paint and bindings. `for` and `if` nodes become service nodes without children; their
//! bodies are instantiated later, when a binding asks for them ([`instantiate_body`]), and are inserted as
//! siblings right after the service node.
//!
//! Nothing here panics on a bad id: a missing template or style set gives a placeholder node and one error with
//! the node's path.

use ui2_model::Key;
use ui2_package::{NodeKind, NodeRef, Package, StrId, Styles, Template, TemplateId};

use crate::arena::{Arena, Dirty, Node, NodeClass, NodeId, PLACEHOLDER_STYLE};
use crate::error::{ErrorCode, ErrorSink, NodePath, UiError};
use crate::style::{self, StyleEnv};

/// Most nodes one template instance may have; a larger one is replaced by a placeholder (a panel limit).
pub const MAX_INSTANCE_NODES: usize = 20_000;

/// What instantiation reads from a package. `Package` implements it; tests may use plain tables.
pub trait Source {
    /// The templates; template 0 is the document.
    fn templates(&self) -> &[Template];
    /// The baked styles.
    fn styles(&self) -> &Styles;
    /// A string of the string table.
    fn str(&self, id: StrId) -> Option<&str>;
    /// The id of a string.
    fn str_id(&self, s: &str) -> Option<StrId>;
}

impl Source for Package {
    fn templates(&self) -> &[Template] {
        Package::templates(self)
    }

    fn styles(&self) -> &Styles {
        Package::styles(self)
    }

    fn str(&self, id: StrId) -> Option<&str> {
        Package::str(self, id)
    }

    fn str_id(&self, s: &str) -> Option<StrId> {
        Package::str_id(self, s)
    }
}

/// Where and how to make a template instance.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Spawn {
    pub tpl: TemplateId,
    /// The parent element, `None` for the top of the document.
    pub parent: Option<NodeId>,
    /// The position of the first top-level node among the parent's children.
    pub at: usize,
    /// The list key of a `for` item.
    pub key: Option<Key>,
    /// The service node the instance belongs to.
    pub owner: Option<NodeId>,
}

/// The arena class of a template node kind.
pub(crate) fn class_of(kind: &NodeKind) -> NodeClass {
    match kind {
        NodeKind::Box { .. } | NodeKind::Image(_) | NodeKind::Input { .. } => NodeClass::Element,
        NodeKind::Layer(_) => NodeClass::Layer,
        NodeKind::Text(_) => NodeClass::Text,
        NodeKind::For { .. } | NodeKind::If { .. } => NodeClass::Service,
    }
}

fn is_element_kind(kind: &NodeKind) -> bool {
    class_of(kind).is_element()
}

/// The template node a node was made from.
pub(crate) fn template_node<S: Source + ?Sized>(src: &S, r: NodeRef) -> Option<&ui2_package::Node> {
    src.templates().get(r.0.index())?.nodes.get(r.1 as usize)
}

/// The chunk of a new node: a top-level node or a child of a layer root starts a chunk, others paint into
/// their parent's.
fn chunk_of(arena: &Arena, id: NodeId, parent: Option<NodeId>) -> u32 {
    match parent.and_then(|p| arena.get(p)) {
        Some(p) if p.class != NodeClass::Layer => p.chunk,
        _ => id.index(),
    }
}

fn placeholder(arena: &mut Arena, s: &Spawn, tmpl: NodeRef) -> NodeId {
    let mut node = Node::new(tmpl, NodeClass::Placeholder, s.parent);
    node.style = PLACEHOLDER_STYLE;
    node.key = s.key;
    node.owner = s.owner;
    let id = arena.alloc(node);
    let chunk = chunk_of(arena, id, s.parent);
    if let Some(n) = arena.get_mut(id) {
        n.chunk = chunk;
    }
    arena.attach(s.parent, s.at, id);
    arena.mark(id, Dirty::LAYOUT | Dirty::PAINT);
    id
}

fn add_instance(arena: &mut Arena, owner: Option<NodeId>, key: Option<Key>, roots: &[NodeId]) {
    if let Some(owner) = owner.and_then(|o| arena.get_mut(o)) {
        owner.instances.push(crate::arena::Instance {
            key,
            roots: roots.to_vec(),
        });
    }
}

/// Makes an instance of a template. Returns its top-level nodes (a placeholder when the template cannot be
/// built).
pub(crate) fn instantiate<S: Source + ?Sized>(
    arena: &mut Arena,
    src: &S,
    env: &StyleEnv,
    errors: &mut ErrorSink,
    s: Spawn,
) -> Vec<NodeId> {
    let fail = |arena: &mut Arena, errors: &mut ErrorSink, code: ErrorCode, msg: String| {
        let ph = placeholder(arena, &s, NodeRef(s.tpl, 0));
        add_instance(arena, s.owner, s.key, &[ph]);
        let path = node_path(arena, src, ph);
        errors.report(UiError::at(&path, code, msg));
        arena.bump_revision();
        vec![ph]
    };
    let Some(template) = src.templates().get(s.tpl.index()) else {
        let msg = format!("template {} does not exist", s.tpl.0);
        return fail(arena, errors, ErrorCode::MissingTemplate, msg);
    };
    if template.nodes.len() > MAX_INSTANCE_NODES {
        let msg = format!(
            "template {} has {} nodes, at most {MAX_INSTANCE_NODES}",
            s.tpl.0,
            template.nodes.len()
        );
        return fail(arena, errors, ErrorCode::NodeLimit, msg);
    }

    let mut roots = Vec::new();
    // Open subtrees: (end index in the template, node).
    let mut open: Vec<(usize, NodeId)> = Vec::new();
    for (i, t) in template.nodes.iter().enumerate() {
        while open.last().is_some_and(|&(end, _)| end <= i) {
            open.pop();
        }
        let parent = open.last().map(|&(_, id)| id).or(s.parent);
        let top = open.is_empty();
        let class = class_of(&t.kind);
        let mut node = Node::new(NodeRef(s.tpl, i as u32), class, parent);
        let mut missing_set = false;
        if class != NodeClass::Service {
            if src.styles().sets.get(t.style.index()).is_some() {
                node.set = Some(t.style);
            } else {
                node.class = NodeClass::Placeholder;
                node.style = PLACEHOLDER_STYLE;
                missing_set = true;
            }
        }
        if top {
            node.key = s.key;
            node.owner = s.owner;
        }
        let id = arena.alloc(node);
        let chunk = chunk_of(arena, id, parent);
        if let Some(n) = arena.get_mut(id) {
            n.chunk = chunk;
        }
        if top {
            arena.attach(parent, s.at + roots.len(), id);
            roots.push(id);
        } else {
            arena.attach(parent, usize::MAX, id);
        }
        style::attach(arena, src.styles(), env, id);
        let mut bits = Dirty::LAYOUT | Dirty::PAINT | Dirty::BIND;
        if class == NodeClass::Text {
            bits |= Dirty::TEXT;
        }
        arena.mark(id, bits);
        if missing_set {
            let path = node_path(arena, src, id);
            errors.report(UiError::at(
                &path,
                ErrorCode::MissingStyleSet,
                format!("style set {} does not exist", t.style.0),
            ));
        }
        if t.subtree > 1 {
            open.push((i + t.subtree as usize, id));
        }
    }
    add_instance(arena, s.owner, s.key, &roots);
    if let Some(p) = s.parent {
        arena.mark(p, Dirty::LAYOUT);
    }
    arena.bump_revision();
    roots
}

/// The body template of a service node.
fn body_template<S: Source + ?Sized>(src: &S, r: NodeRef) -> Option<TemplateId> {
    match &template_node(src, r)?.kind {
        NodeKind::For { tpl, .. } | NodeKind::If { tpl, .. } => Some(*tpl),
        _ => None,
    }
}

/// Makes one more instance of a `for` or `if` body, after the existing ones. Returns its top-level nodes;
/// a handle that is not a live service node gives an error and nothing.
pub(crate) fn instantiate_body<S: Source + ?Sized>(
    arena: &mut Arena,
    src: &S,
    env: &StyleEnv,
    errors: &mut ErrorSink,
    service: NodeId,
    key: Option<Key>,
) -> Vec<NodeId> {
    let Some(node) = arena.get(service) else {
        errors.report(UiError::global(
            ErrorCode::BadNode,
            "instantiate_body on a removed node",
        ));
        return Vec::new();
    };
    let Some(tpl) = body_template(src, node.tmpl) else {
        let path = node_path(arena, src, service);
        errors.report(UiError::at(
            &path,
            ErrorCode::BadNode,
            "instantiate_body on a node that is not a for or if",
        ));
        return Vec::new();
    };
    let parent = node.parent;
    let owned: usize = node.instances.iter().map(|i| i.roots.len()).sum();
    let siblings = arena.siblings(parent);
    let base = siblings
        .iter()
        .position(|c| *c == service)
        .unwrap_or(siblings.len());
    let spawn = Spawn {
        tpl,
        parent,
        at: base + 1 + owned,
        key,
        owner: Some(service),
    };
    instantiate(arena, src, env, errors, spawn)
}

/// The preorder index of a template node among the template's elements.
fn element_index(template: &Template, i: usize) -> usize {
    template.nodes[..i.min(template.nodes.len())]
        .iter()
        .filter(|n| is_element_kind(&n.kind))
        .count()
}

/// The template parent of node `i` and the number of text leaves before `i` among that parent's children.
fn text_position(template: &Template, i: usize) -> (Option<usize>, usize) {
    let nodes = &template.nodes;
    // Find the innermost subtree that contains `i`.
    let mut parent = None;
    let mut j = 0;
    let mut end = nodes.len();
    while j < i && j < end {
        let sub_end = j + nodes[j].subtree.max(1) as usize;
        if i < sub_end {
            parent = Some(j);
            end = sub_end;
            j += 1;
        } else {
            j = sub_end;
        }
    }
    let mut k = 0;
    let mut j = parent.map_or(0, |p| p + 1);
    while j < i {
        if matches!(nodes[j].kind, NodeKind::Text(_)) {
            k += 1;
        }
        j += nodes[j].subtree.max(1) as usize;
    }
    (parent, k)
}

/// The node's label `<template>:<element index>[#k]` (the converter's rectangle dump format).
pub(crate) fn node_label<S: Source + ?Sized>(arena: &Arena, src: &S, id: NodeId) -> String {
    let Some(node) = arena.get(id) else {
        return "?".into();
    };
    let NodeRef(tpl, i) = node.tmpl;
    let i = i as usize;
    let Some(template) = src.templates().get(tpl.index()) else {
        return format!("{}:?", tpl.0);
    };
    match node.class {
        NodeClass::Text => {
            let (parent, k) = text_position(template, i);
            match parent {
                Some(p) => format!("{}:{}#{k}", tpl.0, element_index(template, p)),
                None => match node.parent {
                    Some(p) => format!("{}#{k}", node_label(arena, src, p)),
                    None => format!("{}:-#{k}", tpl.0),
                },
            }
        }
        // Service nodes have no box and no element index: name them by template position.
        NodeClass::Service => format!("{}:@{i}", tpl.0),
        _ => format!("{}:{}", tpl.0, element_index(template, i)),
    }
}

/// The full path of a node: label, instance keys and parts.
pub(crate) fn node_path<S: Source + ?Sized>(arena: &Arena, src: &S, id: NodeId) -> NodePath {
    let mut keys = Vec::new();
    let mut part = None;
    let mut panel = None;
    let mut at = Some(id);
    while let Some(cur) = at {
        let Some(node) = arena.get(cur) else {
            break;
        };
        if let Some(k) = node.key {
            keys.push(k.to_string());
        }
        let name = template_node(src, node.tmpl)
            .and_then(|t| t.part)
            .and_then(|p| src.str(p));
        if let Some(name) = name {
            if part.is_none() {
                part = Some(name.to_string());
            }
            panel = Some(name.to_string());
        }
        at = node.parent;
    }
    keys.reverse();
    NodePath {
        label: node_label(arena, src, id),
        keys: keys.join("/"),
        part,
        panel,
    }
}

/// The first node in document order whose template node has the part name.
pub(crate) fn find_part<S: Source + ?Sized>(arena: &Arena, src: &S, name: &str) -> Option<NodeId> {
    let id = src.str_id(name)?;
    arena.preorder().into_iter().find(|&n| {
        arena
            .get(n)
            .and_then(|node| template_node(src, node.tmpl))
            .is_some_and(|t| t.part == Some(id))
    })
}

#[cfg(test)]
mod tests {
    //! Templates built by hand: `for`/`if` need expressions, which packages cannot carry yet.

    use super::*;
    use ui2_package::{
        ExprId, Layer, Node as TNode, StrId, StyleId, StyleRecord, StyleSet, StyleSetId, Tag,
        TextSrc,
    };

    struct Raw {
        templates: Vec<Template>,
        styles: Styles,
        strs: Vec<String>,
    }

    impl Source for Raw {
        fn templates(&self) -> &[Template] {
            &self.templates
        }
        fn styles(&self) -> &Styles {
            &self.styles
        }
        fn str(&self, id: StrId) -> Option<&str> {
            self.strs.get(id.index()).map(String::as_str)
        }
        fn str_id(&self, s: &str) -> Option<StrId> {
            self.strs
                .iter()
                .position(|x| x == s)
                .map(|i| StrId(i as u32))
        }
    }

    fn n(kind: NodeKind, subtree: u32) -> TNode {
        TNode {
            kind,
            subtree,
            style: StyleSetId(0),
            part: None,
            flags: 0,
        }
    }

    fn div(subtree: u32) -> TNode {
        n(NodeKind::Box { tag: Tag::Div }, subtree)
    }

    fn text() -> TNode {
        n(NodeKind::Text(TextSrc::Static(StrId(0))), 1)
    }

    /// Three template levels: the document has a `for` whose body has an `if`.
    fn raw() -> Raw {
        let mut layer = n(NodeKind::Layer(Layer::Hud), 5);
        layer.part = Some(StrId(1));
        let mut list = div(3);
        list.part = Some(StrId(2));
        let doc = Template {
            name: StrId(0),
            nodes: vec![
                layer,
                list,
                text(),
                n(
                    NodeKind::For {
                        tpl: TemplateId(1),
                        item: StrId(0),
                        index: None,
                        list: ExprId(0),
                        key: ExprId(0),
                        virt: false,
                        sort: None,
                        sort_desc: None,
                        filter: None,
                    },
                    1,
                ),
                div(1),
            ],
        };
        let row = Template {
            name: StrId(0),
            nodes: vec![
                div(4),
                text(),
                n(
                    NodeKind::If {
                        tpl: TemplateId(2),
                        cond: ExprId(0),
                    },
                    1,
                ),
                text(),
            ],
        };
        let badge = Template {
            name: StrId(0),
            nodes: vec![div(2), text()],
        };
        let styles = Styles {
            records: vec![StyleRecord::initial()],
            sets: vec![StyleSet::plain(vec![StyleId(0)])],
            ..Styles::default()
        };
        Raw {
            templates: vec![doc, row, badge],
            styles,
            strs: vec!["".into(), "hud".into(), "list".into()],
        }
    }

    fn spawn(tpl: u32) -> Spawn {
        Spawn {
            tpl: TemplateId(tpl),
            parent: None,
            at: 0,
            key: None,
            owner: None,
        }
    }

    #[test]
    fn three_template_levels_are_made_named_and_removed() {
        let src = raw();
        let env = StyleEnv::default();
        let mut errors = ErrorSink::new();
        let mut a = Arena::new();
        let roots = instantiate(&mut a, &src, &env, &mut errors, spawn(0));
        assert_eq!(roots.len(), 1);
        assert_eq!(a.len(), 5);
        let list = find_part(&a, &src, "list").expect("list part");
        let service = a.get(list).expect("list").children()[1];
        assert_eq!(a.get(service).map(Node::class), Some(NodeClass::Service));

        // Two list items, then an `if` body inside the second one.
        let k1 = Key::new(0, 17);
        let k2 = Key::new(0, 18);
        let item1 = instantiate_body(&mut a, &src, &env, &mut errors, service, Some(k1));
        let item2 = instantiate_body(&mut a, &src, &env, &mut errors, service, Some(k2));
        let children = a.get(list).expect("list").children().to_vec();
        assert_eq!(children, vec![children[0], service, item1[0], item2[0]]);
        let inner_if = a.get(item2[0]).expect("row").children()[1];
        let badge = instantiate_body(&mut a, &src, &env, &mut errors, inner_if, None);
        assert_eq!(a.len(), 5 + 4 + 4 + 2);
        assert!(errors.take().is_empty());

        // Paths: element numbering per template, text leaves as `#k`, keys outermost first.
        let label = |id| node_label(&a, &src, id);
        assert_eq!(label(roots[0]), "0:0");
        assert_eq!(label(children[0]), "0:1#0");
        assert_eq!(label(item2[0]), "1:0");
        let row_texts: Vec<NodeId> = a
            .get(item2[0])
            .expect("row")
            .children()
            .iter()
            .copied()
            .filter(|&c| a.get(c).is_some_and(|n| n.class() == NodeClass::Text))
            .collect();
        assert_eq!(label(row_texts[1]), "1:0#1");
        let badge_text = a.get(badge[0]).expect("badge").children()[0];
        let path = node_path(&a, &src, badge_text);
        assert_eq!(path.label, "2:0#0");
        assert_eq!(path.keys, "0:18");
        assert_eq!(path.part.as_deref(), Some("list"));
        assert_eq!(path.panel.as_deref(), Some("hud"));

        // Removing the `for` node removes its items (siblings) and their nested bodies.
        let before = a.len();
        assert_eq!(a.remove(service), 1 + 4 + 4 + 2);
        assert_eq!(a.len(), before - 11);
        assert_eq!(a.get(list).expect("list").children().len(), 1);
        assert!(!a.contains(badge_text));
        // Removing the document empties the arena; slots are reused afterwards.
        a.remove(roots[0]);
        assert!(a.is_empty());
        let again = instantiate(&mut a, &src, &env, &mut errors, spawn(0));
        assert_eq!(a.len(), 5);
        assert_ne!(again[0], roots[0]);
    }

    #[test]
    fn removing_one_item_keeps_the_others() {
        let src = raw();
        let env = StyleEnv::default();
        let mut errors = ErrorSink::new();
        let mut a = Arena::new();
        instantiate(&mut a, &src, &env, &mut errors, spawn(0));
        let list = find_part(&a, &src, "list").expect("list");
        let service = a.get(list).expect("list").children()[1];
        let item1 = instantiate_body(&mut a, &src, &env, &mut errors, service, Some(Key(1)));
        let item2 = instantiate_body(&mut a, &src, &env, &mut errors, service, Some(Key(2)));
        assert_eq!(a.remove(item1[0]), 4);
        let s = a.get(service).expect("service");
        assert_eq!(s.instances().len(), 1);
        assert_eq!(s.instances()[0].roots, item2);
        // A new item goes after the remaining one.
        let item3 = instantiate_body(&mut a, &src, &env, &mut errors, service, Some(Key(3)));
        let children = a.get(list).expect("list").children();
        assert_eq!(&children[2..], &[item2[0], item3[0]]);
    }

    #[test]
    fn bad_ids_give_placeholders_and_one_error() {
        let mut src = raw();
        src.templates[2].nodes[1].style = StyleSetId(9);
        let env = StyleEnv::default();
        let mut errors = ErrorSink::new();
        let mut a = Arena::new();
        let ph = instantiate(&mut a, &src, &env, &mut errors, spawn(7));
        assert_eq!(a.get(ph[0]).map(Node::class), Some(NodeClass::Placeholder));
        let errs = errors.take();
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].code, ErrorCode::MissingTemplate);
        assert_eq!(errs[0].path, "7:?");
        let badge = instantiate(&mut a, &src, &env, &mut errors, spawn(2));
        let text = a.get(badge[0]).expect("badge").children()[0];
        assert_eq!(a.get(text).map(Node::class), Some(NodeClass::Placeholder));
        let errs = errors.take();
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].code, ErrorCode::MissingStyleSet);
        // Not a service node: an error, no nodes.
        assert!(instantiate_body(&mut a, &src, &env, &mut errors, badge[0], None).is_empty());
        assert_eq!(errors.take().len(), 1);
    }
}
