//! Index types shared by the sections. Every one is an index into a table of the same package; the
//! reader checks the range of each (`Package::read`).

use serde::{Deserialize, Serialize};

macro_rules! id_type {
    ($(#[$doc:meta] $name:ident),* $(,)?) => {$(
        #[$doc]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub struct $name(pub u32);

        impl $name {
            /// The index as a `usize`.
            pub fn index(self) -> usize {
                self.0 as usize
            }
        }
    )*};
}

id_type! {
    /// An entry of the `STRS` table (identifiers, classes, keys, paths), sorted by bytes.
    StrId,
    /// A template of `TMPL`; template 0 is the document.
    TemplateId,
    /// A `StyleSet` of `STYL`.
    StyleSetId,
    /// A `StyleRecord` of `STYL`.
    StyleId,
    /// An expression of `BIND`.
    ExprId,
    /// A text template (segments) of `BIND`.
    TplId,
    /// A root color token of `TOKN`.
    TokenId,
    /// A picture of `IMGS`.
    ImgId,
    /// A game asset reference of `AREF`.
    ARefId,
}

/// A node of a template: the template and the index in its preorder node list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct NodeRef(pub TemplateId, pub u32);
