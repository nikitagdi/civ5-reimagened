//! art-defines: unit art definitions plus `.fxsxml` model descriptions.
//!
//! # Where the data lives
//! Unit art is plain XML, not SQL. The chain for a unit is
//! `ArtDefine UnitArtInfo` (`Civ5ArtDefines_Units.xml`: formation, member ids and counts)
//! -> `UnitMemberArtInfo` (`Civ5ArtDefines_UnitMembers.xml`: `fScale`, `Granny` = path of the
//! `.fxsxml`) -> `.fxsxml` (mesh, textures, animations, other refs).
//! Base files are packed in `UnitModels.fpk` with loose duplicates under `Assets/Units`;
//! every expansion/DLC adds its own `Civ5ArtDefines_*Units.xml` / `*UnitMembers.xml`.
//! Strategic-view files (`Civ5ArtDefines_SV_*`) use another schema and are skipped.
//!
//! # Override rule
//! Files are applied in `GameSource` override order (`Entry::rank`: the root, then the content packages by path);
//! within a layer the canonical `Civ5ArtDefines_Units.xml`/`_UnitMembers.xml` goes first,
//! the rest by file name. A later row with the same `Type` replaces the earlier row whole
//! (no field merging), matching how `GameSource` lets later layers shadow files.
//!
//! # `.fxsxml` structure
//! Root `<Asset>` with flat children, all attributes optional in practice:
//! `Mesh@file` (gr2; at most one per file in the whole install), `Texture@file`,
//! `Animation@file/@ec` (`ec` is one event code or a comma list, e.g. `"1160, 1180"`;
//! `@name`/`@layer` only appear on non-unit assets), `TimedTrigger@file` (ftsxml),
//! `AnimGraph@file` (dge), `StateMachine@file` (fsmxml), `DecisionTree@file`,
//! `BoneUsage/Bone@name`. `@source` is an authoring-tool tag and is ignored.
//!
//! # Texture binding
//! The fxsxml has no per-material or per-mesh binding: textures are one flat list, and the gr2
//! materials keep only hashes of their names and source paths. A unit fxsxml has exactly one
//! mesh file, so every texture belongs to it; which Granny material uses which texture is
//! recovered by `crate::models::materials`. The role of a texture is encoded only in its file
//! name (`_DIFF` albedo with team-colour alpha, `_SREF` specular/reflection, `_NORM`, `_OPAC`, `_MASK`), while
//! `Unit_Environment_*`, `Environment_*_Dull/Sharp_*`, `*Irradiance`, `CubeLight_*` are shared
//! lighting maps. Variants such as `Archer_02_DIFF.dds` simply replace the pair per fxsxml.
//! See [`TextureSlot`].

mod defines;
mod fxs;
mod resolve;

pub use defines::{ArtDefines, DefOrigin, MemberRef, UnitArt, UnitMemberArt};
pub use fxs::{FxsAnim, FxsModel, FxsTexture, TextureSlot, parse_fxsxml};
pub use resolve::{
    ArtWarning, FileRole, ResolvedMember, resolve_fxsxml, resolve_fxsxml_text, resolve_unit_member,
};

use crate::game::GameError;

#[derive(Debug, thiserror::Error)]
pub enum ArtError {
    #[error(transparent)]
    Game(#[from] GameError),
    #[error("{what}: invalid XML: {source}")]
    Xml {
        what: String,
        #[source]
        source: roxmltree::Error,
    },
    #[error("{what}: unexpected root element <{found}>, expected <{expected}>")]
    BadRoot {
        what: String,
        expected: &'static str,
        found: String,
    },
    #[error("unknown unit member art {0:?}")]
    UnknownMember(String),
}

/// Decodes game XML: tolerates a BOM and stray non-UTF-8 bytes in comments.
pub(crate) fn decode_xml(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    text.trim_start_matches('\u{feff}').to_owned()
}

/// Last path segment, lower-case: `Assets/Units/Warrior/Warrior.fxsxml` -> `warrior.fxsxml`.
pub(crate) fn flat_name(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .next()
        .unwrap_or(path)
        .trim()
        .to_ascii_lowercase()
}
