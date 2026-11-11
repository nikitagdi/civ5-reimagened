//! The document's tables as the passes read them.
//!
//! [`Doc`] answers the same questions as the skin's [`Package`] (templates, styles, strings, expressions,
//! bindings, extension tables) under the same names, so the passes are written against it once.

use std::sync::Arc;

use ui2_model::wire::ScriptCtxId;
use ui2_package::profile::expr::Expr;
use ui2_package::{
    ARefId, Binding, Hotkey, Images, Lit, NodeRef, Package, Seg, StrId, Styles, Template,
    TemplateId, Tokens, TplId,
};

use crate::island::IslandDef;

/// The tables of the document.
#[derive(Debug, Clone)]
pub struct Doc {
    skin: Arc<Package>,
}

impl Doc {
    /// The document of a skin.
    pub fn new(skin: Arc<Package>) -> Self {
        Self { skin }
    }

    /// The skin's package.
    pub fn skin(&self) -> &Arc<Package> {
        &self.skin
    }

    /// The templates; template 0 is the skin's document.
    pub fn templates(&self) -> &[Template] {
        self.skin.templates()
    }

    /// The styles.
    pub fn styles(&self) -> &Styles {
        self.skin.styles()
    }

    /// The color tokens of the skin.
    pub fn tokens(&self) -> &Tokens {
        self.skin.tokens()
    }

    /// A string.
    pub fn str(&self, id: StrId) -> Option<&str> {
        self.skin.str(id)
    }

    /// The id of a string.
    pub fn str_id(&self, s: &str) -> Option<StrId> {
        self.skin.str_id(s)
    }

    /// An expression.
    pub fn expr(&self, id: ui2_package::ExprId) -> Option<&Expr> {
        self.skin.expr(id)
    }

    /// A text template.
    pub fn tpl(&self, id: TplId) -> Option<&[Seg]> {
        self.skin.tpl(id)
    }

    /// The bindings of a template node.
    pub fn bindings_of(&self, tpl: TemplateId, node: u32) -> &[Binding] {
        self.skin.bindings_of(tpl, node)
    }

    /// The skin's pictures.
    pub fn images(&self) -> &Images {
        self.skin.images()
    }

    /// A game asset reference.
    pub fn asset_ref(&self, id: ARefId) -> Option<&str> {
        self.skin.asset_ref(id)
    }

    /// The tooltips: node and template, ascending by node.
    pub fn tooltips(&self) -> &[(NodeRef, TemplateId)] {
        self.skin.tooltips()
    }

    /// The hotkeys.
    pub fn hotkeys(&self) -> &[Hotkey] {
        self.skin.hotkeys()
    }

    /// The skin's languages.
    pub fn languages(&self) -> &[String] {
        self.skin.languages()
    }

    /// The text template of a key in a language.
    pub fn l10n(&self, lang: &str, key: &str) -> Option<&str> {
        self.skin.l10n(lang, key)
    }

    /// The skin's state defaults.
    pub fn state_defaults(&self) -> impl Iterator<Item = (&str, &Lit)> + '_ {
        self.skin.state_defaults()
    }

    /// The script context the markup of a template belongs to.
    pub fn ctx_of(&self, _tpl: TemplateId) -> ScriptCtxId {
        ScriptCtxId::Skin
    }

    /// The model path of a control's state path (`data-bind-model` keeps the path below the state root).
    pub fn state_path(&self, _tpl: TemplateId, path: &str) -> String {
        format!("state.{path}")
    }

    /// An island of a script context by name.
    pub(crate) fn island(&self, ctx: &ScriptCtxId, name: &str) -> Option<IslandDef> {
        if *ctx != ScriptCtxId::Skin {
            return None;
        }
        let island = self
            .skin
            .islands()
            .iter()
            .find(|i| self.skin.str(i.id) == Some(name))?;
        Some(IslandDef {
            keys: island
                .keys
                .iter()
                .filter_map(|(k, n)| {
                    Some((self.skin.str(*k)?.to_string(), NodeRef(island.node.0, *n)))
                })
                .collect(),
        })
    }

    /// Every keyed node of every island.
    pub(crate) fn island_nodes(&self) -> Vec<NodeRef> {
        self.skin
            .islands()
            .iter()
            .flat_map(|i| i.keys.iter().map(|(_, n)| NodeRef(i.node.0, *n)))
            .collect()
    }
}

impl crate::instantiate::Source for Doc {
    fn templates(&self) -> &[Template] {
        Doc::templates(self)
    }

    fn styles(&self) -> &Styles {
        Doc::styles(self)
    }

    fn str(&self, id: StrId) -> Option<&str> {
        Doc::str(self, id)
    }

    fn str_id(&self, s: &str) -> Option<StrId> {
        Doc::str_id(self, s)
    }
}
