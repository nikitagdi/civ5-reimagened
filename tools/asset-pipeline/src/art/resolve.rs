//! Resolving a unit member into everything the model step needs.

use std::fmt;

use crate::game::GameSource;

use super::defines::ArtDefines;
use super::fxs::{FxsAnim, FxsModel, FxsTexture, TextureSlot, parse_fxsxml};
use super::{ArtError, decode_xml};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileRole {
    Fxsxml,
    Mesh,
    Texture,
    Animation,
    TimedTrigger,
    AnimGraph,
    StateMachine,
    DecisionTree,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtWarning {
    /// A referenced file is not in the install.
    MissingFile { role: FileRole, name: String },
    /// The fxsxml exists but could not be parsed.
    BadFxsxml { name: String, error: String },
}

impl fmt::Display for ArtWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingFile { role, name } => write!(f, "missing {role:?} file {name}"),
            Self::BadFxsxml { name, error } => write!(f, "bad fxsxml {name}: {error}"),
        }
    }
}

/// A unit member with every reference checked against the install.
#[derive(Debug, Clone)]
pub struct ResolvedMember {
    pub id: String,
    pub scale: f32,
    pub z_offset: Option<f32>,
    /// Flat lower-case fxsxml name.
    pub fxsxml: String,
    pub model: FxsModel,
    pub warnings: Vec<ArtWarning>,
}

impl ResolvedMember {
    /// gr2 mesh names; Civ5 unit fxsxml files have exactly one.
    pub fn meshes(&self) -> &[String] {
        &self.model.meshes
    }

    pub fn texture(&self, slot: TextureSlot) -> Option<&FxsTexture> {
        self.model.textures.iter().find(|t| t.slot == slot)
    }

    pub fn animations(&self) -> &[FxsAnim] {
        &self.model.animations
    }

    pub fn missing_files(&self) -> usize {
        self.warnings
            .iter()
            .filter(|w| matches!(w, ArtWarning::MissingFile { .. }))
            .count()
    }
}

fn check<'a>(
    source: &GameSource,
    role: FileRole,
    names: impl IntoIterator<Item = &'a String>,
    warnings: &mut Vec<ArtWarning>,
) {
    for name in names {
        if source.lookup(name).is_none() {
            warnings.push(ArtWarning::MissingFile {
                role,
                name: name.clone(),
            });
        }
    }
}

impl ArtDefines {
    pub fn resolve_member(
        &self,
        source: &GameSource,
        member_id: &str,
    ) -> Result<ResolvedMember, ArtError> {
        let member = self
            .member(member_id)
            .ok_or_else(|| ArtError::UnknownMember(member_id.to_owned()))?;
        resolve_fxsxml(
            source,
            &member.id,
            &member.granny_fxsxml,
            member.scale,
            member.z_offset,
        )
    }
}

/// Resolves the model `fxsxml` (a path as written in the art defines or the DB; only the file
/// name is used) under the id `id`, without the art defines: the entry point for models named by
/// the DB snapshot (`ArtDefine_UnitMemberInfos.Model`) and for non-unit models such as cities.
pub fn resolve_fxsxml(
    source: &GameSource,
    id: &str,
    fxsxml: &str,
    scale: f32,
    z_offset: Option<f32>,
) -> Result<ResolvedMember, ArtError> {
    resolve_fxsxml_text(source, id, fxsxml, scale, z_offset, None)
}

/// [`resolve_fxsxml`] for a model whose asset list is given as text (`text`) instead of being read
/// from the install: the tree models (`forest_europe.gr2` and its texture have no `.fxsxml`).
pub fn resolve_fxsxml_text(
    source: &GameSource,
    id: &str,
    fxsxml: &str,
    scale: f32,
    z_offset: Option<f32>,
    text: Option<&str>,
) -> Result<ResolvedMember, ArtError> {
    let fxsxml = super::flat_name(fxsxml);
    let mut warnings = Vec::new();
    let bytes = match text {
        Some(t) => Some(t.as_bytes().to_vec()),
        None => source.read_by_name(&fxsxml)?,
    };
    let model = match bytes {
        None => {
            warnings.push(ArtWarning::MissingFile {
                role: FileRole::Fxsxml,
                name: fxsxml.clone(),
            });
            FxsModel::default()
        }
        Some(bytes) => match parse_fxsxml(&decode_xml(&bytes)) {
            Ok(m) => m,
            Err(e) => {
                warnings.push(ArtWarning::BadFxsxml {
                    name: fxsxml.clone(),
                    error: e.to_string(),
                });
                FxsModel::default()
            }
        },
    };

    check(source, FileRole::Mesh, &model.meshes, &mut warnings);
    check(
        source,
        FileRole::Texture,
        model.textures.iter().map(|t| &t.file),
        &mut warnings,
    );
    check(
        source,
        FileRole::Animation,
        model.animations.iter().map(|a| &a.file),
        &mut warnings,
    );
    check(
        source,
        FileRole::TimedTrigger,
        &model.timed_triggers,
        &mut warnings,
    );
    check(
        source,
        FileRole::AnimGraph,
        &model.anim_graphs,
        &mut warnings,
    );
    check(
        source,
        FileRole::StateMachine,
        &model.state_machines,
        &mut warnings,
    );
    check(
        source,
        FileRole::DecisionTree,
        &model.decision_trees,
        &mut warnings,
    );

    Ok(ResolvedMember {
        id: id.to_owned(),
        scale,
        z_offset,
        fxsxml,
        model,
        warnings,
    })
}

/// Convenience for one-off use; loads the definitions each call, so batch callers should
/// build an [`ArtDefines`] once and use [`ArtDefines::resolve_member`].
pub fn resolve_unit_member(
    source: &GameSource,
    member_id: &str,
) -> Result<ResolvedMember, ArtError> {
    ArtDefines::load(source)?.resolve_member(source, member_id)
}
