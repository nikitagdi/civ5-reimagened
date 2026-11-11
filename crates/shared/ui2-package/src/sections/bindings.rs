//! `BIND`: expressions, text templates, the bindings and events of nodes, the model paths and
//! intents the skin uses and the skin's state with its defaults.

use serde::{Deserialize, Serialize};

use crate::container::SectionTag;
use crate::error::PackageError;
use crate::ids::{ExprId, StrId, TemplateId, TplId};
use crate::profile::action::{Action, AnchorKind, ControlKind, Event};
use crate::profile::expr::Expr;
use crate::profile::{PropId, TypeSig};
use crate::sections::templates::Templates;

/// One piece of a text template.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Seg {
    /// Text copied as it is.
    Lit(StrId),
    /// An insert.
    Expr(ExprId),
}

/// A scalar of a state list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Scalar {
    /// A bool.
    Bool(bool),
    /// An integer.
    Int(i64),
    /// A finite float.
    Float(f64),
    /// A string.
    Str(String),
}

impl Scalar {
    fn same_kind(&self, other: &Scalar) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }

    fn sound(&self) -> bool {
        !matches!(self, Scalar::Float(f) if !f.is_finite())
    }

    /// The type of the value.
    pub fn type_sig(&self) -> TypeSig {
        match self {
            Scalar::Bool(_) => TypeSig::Bool,
            Scalar::Int(_) => TypeSig::Int,
            Scalar::Float(_) => TypeSig::Float,
            Scalar::Str(_) => TypeSig::Str,
        }
    }
}

/// A default value of a state path. The type of the path is the type of the value; a list is not
/// empty and holds one kind of scalar.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Lit {
    /// A bool.
    Bool(bool),
    /// An integer.
    Int(i64),
    /// A finite float.
    Float(f64),
    /// A string.
    Str(String),
    /// A non-empty list of one kind of scalar.
    List(Vec<Scalar>),
}

impl Lit {
    /// The type of the path this value is the default of.
    pub fn type_sig(&self) -> Option<TypeSig> {
        Some(match self {
            Lit::Bool(_) => TypeSig::Bool,
            Lit::Int(_) => TypeSig::Int,
            Lit::Float(_) => TypeSig::Float,
            Lit::Str(_) => TypeSig::Str,
            Lit::List(items) => TypeSig::List(Box::new(items.first()?.type_sig())),
        })
    }

    fn sound(&self) -> bool {
        match self {
            Lit::Float(f) => f.is_finite(),
            Lit::List(items) => {
                !items.is_empty()
                    && items.iter().all(Scalar::sound)
                    && items.windows(2).all(|w| w[0].same_kind(&w[1]))
            }
            _ => true,
        }
    }
}

/// What a binding does. Postcard writes the variant index: new variants go at the end.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BindKind {
    /// The node's text.
    Text(TplId),
    /// A class that is on while the condition is true.
    ClassToggle {
        /// The class.
        class: StrId,
        /// The condition.
        cond: ExprId,
    },
    /// A style property set from a template.
    Style {
        /// The property.
        prop: PropId,
        /// The value.
        tpl: TplId,
    },
    /// The picture of an image (a game asset reference).
    Src(ExprId),
    /// Whether the node is disabled.
    Disabled(ExprId),
    /// The skin state path a control edits (without the `state.` prefix).
    Model(StrId),
    /// An event handler.
    Event {
        /// The event.
        ev: Event,
        /// The steps.
        actions: Vec<Action>,
    },
    /// The map object a node follows.
    Anchor {
        /// What kind of object.
        kind: AnchorKind,
        /// Its key.
        id: ExprId,
    },
    /// A control's behaviour (`data-control`).
    Control {
        /// The behaviour.
        kind: ControlKind,
        /// The value of a radio.
        value: Option<StrId>,
        /// Minimum, maximum and step of a slider.
        range: Option<[f32; 3]>,
        /// The items of a select.
        options: Option<ExprId>,
    },
    /// Single-line text that shrinks to fit (`data-fit`), down to `min` px.
    Fit {
        /// The smallest font size, when limited.
        min: Option<f32>,
    },
    /// A node that shows the name of its hotkey (`data-hotkey-label`).
    HotkeyLabel,
}

/// One binding of one node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Binding {
    /// The template the node is in.
    pub tpl: TemplateId,
    /// The node's index in that template.
    pub node: u32,
    /// What it does.
    pub kind: BindKind,
}

/// The section.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Bindings {
    /// The expression trees; an `ExprId` is the index. No repeats, in order of first use.
    pub exprs: Vec<Expr>,
    /// The text templates; a `TplId` is the index. No repeats, in order of first use.
    pub tpls: Vec<Vec<Seg>>,
    /// The bindings, ascending by `(tpl, node)`; several bindings of one node keep their order.
    pub items: Vec<Binding>,
    /// The model paths the skin reads, with the types it expects. Ascending by path.
    pub used_paths: Vec<(StrId, TypeSig)>,
    /// The catalogue intents the skin sends, with their argument types. Ascending by name.
    pub intents: Vec<(StrId, Vec<TypeSig>)>,
    /// The skin's state paths with their default values. Ascending by path.
    pub state: Vec<(StrId, Lit)>,
    /// The names of the script handlers markup calls. Ascending.
    pub handlers: Vec<StrId>,
}

fn ascending<T>(items: &[T], key: impl Fn(&T) -> StrId, strs: usize) -> bool {
    items.iter().all(|i| key(i).index() < strs) && items.windows(2).all(|w| key(&w[0]) < key(&w[1]))
}

impl Bindings {
    pub(crate) fn validate(
        &self,
        at: SectionTag,
        strs: usize,
        templates: &Templates,
    ) -> Result<(), PackageError> {
        let bad = |what: String| Err(PackageError::invalid(at, what));
        if u32::try_from(self.exprs.len()).is_err() || u32::try_from(self.tpls.len()).is_err() {
            return bad("too many expressions".into());
        }
        if let Some(i) = self.exprs.iter().position(|e| !e.is_sound()) {
            return bad(format!("expression {i} is not well formed"));
        }
        let (exprs, tpls) = (self.exprs.len(), self.tpls.len());
        let str_ok = |s: StrId| s.index() < strs;
        let expr_ok = |e: ExprId| e.index() < exprs;
        for (i, tpl) in self.tpls.iter().enumerate() {
            let ok = tpl.iter().all(|seg| match seg {
                Seg::Lit(s) => str_ok(*s),
                Seg::Expr(e) => expr_ok(*e),
            });
            if !ok {
                return bad(format!("text template {i}: an index is out of range"));
            }
        }
        let mut last = (0usize, 0u32);
        for (i, b) in self.items.iter().enumerate() {
            let where_ = |what: &str| format!("binding {i}: {what}");
            let Some(template) = templates.items.get(b.tpl.index()) else {
                return bad(where_("the template does not exist"));
            };
            if b.node as usize >= template.nodes.len() {
                return bad(where_("the node does not exist"));
            }
            if (b.tpl.index(), b.node) < last {
                return bad(where_("the bindings are not ordered by node"));
            }
            last = (b.tpl.index(), b.node);
            let ok = match &b.kind {
                BindKind::Text(t) | BindKind::Style { tpl: t, .. } => t.index() < tpls,
                BindKind::ClassToggle { class, cond } => str_ok(*class) && expr_ok(*cond),
                BindKind::Src(e) | BindKind::Disabled(e) | BindKind::Anchor { id: e, .. } => {
                    expr_ok(*e)
                }
                BindKind::Model(s) => str_ok(*s),
                BindKind::Event { actions, .. } => {
                    if actions.is_empty() {
                        return bad(where_("an event without actions"));
                    }
                    actions.iter().all(action_sound)
                }
                BindKind::Control {
                    value,
                    range,
                    options,
                    ..
                } => {
                    value.is_none_or(str_ok)
                        && options.is_none_or(expr_ok)
                        && range.is_none_or(|[lo, hi, step]| {
                            lo.is_finite()
                                && hi.is_finite()
                                && step.is_finite()
                                && lo < hi
                                && step > 0.0
                        })
                }
                BindKind::Fit { min } => min.is_none_or(|m| m.is_finite() && m > 0.0),
                BindKind::HotkeyLabel => true,
            };
            if !ok {
                return bad(where_("an index or a value is out of range"));
            }
        }
        if !ascending(&self.used_paths, |p| p.0, strs) {
            return bad("the used paths are not ascending names of the string table".into());
        }
        if !ascending(&self.intents, |p| p.0, strs) {
            return bad("the intents are not ascending names of the string table".into());
        }
        if !ascending(&self.state, |p| p.0, strs) {
            return bad("the state paths are not ascending names of the string table".into());
        }
        if self.state.iter().any(|(_, lit)| !lit.sound()) {
            return bad("a state default is not well formed".into());
        }
        if !ascending(&self.handlers, |h| *h, strs) {
            return bad("the handlers are not ascending names of the string table".into());
        }
        Ok(())
    }
}

fn action_sound(a: &Action) -> bool {
    match a {
        Action::Intent { args, .. } | Action::Call { args, .. } => {
            args.iter().all(|a| a.value.is_sound())
        }
        Action::Set { value, .. } => value.is_sound(),
        Action::Toggle { .. } | Action::Stop => true,
    }
}

#[cfg(feature = "write")]
impl crate::remap::Remapped for Seg {
    fn remap(&mut self, r: &crate::remap::Remap) {
        if let Seg::Lit(s) = self {
            r.str(s);
        }
    }
}

#[cfg(feature = "write")]
impl crate::remap::Remapped for Binding {
    fn remap(&mut self, r: &crate::remap::Remap) {
        match &mut self.kind {
            BindKind::ClassToggle { class, .. } => r.str(class),
            BindKind::Model(s) => r.str(s),
            BindKind::Control { value, .. } => r.opt_str(value),
            _ => {}
        }
    }
}
