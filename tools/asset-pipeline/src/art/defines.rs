//! `Civ5ArtDefines_*Units.xml` / `*UnitMembers.xml` loading and merging.

use std::collections::BTreeMap;

use crate::game::{GameSource, Layer};

use super::{ArtError, decode_xml};

/// Which file (and layer) a row came from, for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefOrigin {
    pub file: String,
    pub layer: Layer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberRef {
    pub member_id: String,
    /// Figures of this variant in a full-strength stack.
    pub count: u32,
}

/// `UnitArtInfo`: a unit's formation and the member variants that make up its stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitArt {
    pub id: String,
    pub formation: Option<String>,
    pub damage_states: Option<u32>,
    pub members: Vec<MemberRef>,
    pub origin: DefOrigin,
}

/// `UnitMemberArtInfo`: one figure model.
#[derive(Debug, Clone, PartialEq)]
pub struct UnitMemberArt {
    pub id: String,
    pub scale: f32,
    pub z_offset: Option<f32>,
    /// `Granny` path as written, e.g. `Assets/Units/Warrior/Warrior.fxsxml`.
    pub granny_path: String,
    /// Flat lower-case name of the fxsxml, the `GameSource` key.
    pub granny_fxsxml: String,
    pub domain: Option<String>,
    pub material_tag: Option<String>,
    /// `Combat/Defaults` template id.
    pub combat_template: Option<String>,
    pub origin: DefOrigin,
}

/// Merged unit art tables from the base game, expansions and DLC.
#[derive(Debug, Default)]
pub struct ArtDefines {
    pub units: BTreeMap<String, UnitArt>,
    pub members: BTreeMap<String, UnitMemberArt>,
    /// Rows that replaced an earlier row with the same id.
    pub overridden_rows: usize,
    /// Files or rows that could not be used.
    pub warnings: Vec<String>,
    /// Definition files applied, in application order.
    pub files: Vec<String>,
}

/// Keys are case-insensitive.
fn key(id: &str) -> String {
    id.to_ascii_uppercase()
}

const CANONICAL_UNITS: &str = "civ5artdefines_units.xml";
const CANONICAL_MEMBERS: &str = "civ5artdefines_unitmembers.xml";

#[derive(Clone, Copy, PartialEq, Eq)]
enum FileKind {
    Units,
    Members,
}

fn classify(name: &str) -> Option<FileKind> {
    // `_sv_` files are strategic-view icons with a different schema.
    if !name.starts_with("civ5artdefines_") || name.contains("_sv_") {
        return None;
    }
    if name.ends_with("unitmembers.xml") {
        Some(FileKind::Members)
    } else if name.ends_with("units.xml") {
        Some(FileKind::Units)
    } else {
        None
    }
}

fn child_text<'a>(node: roxmltree::Node<'a, '_>, tag: &str) -> Option<&'a str> {
    node.children()
        .find(|n| n.has_tag_name(tag))
        .and_then(|n| n.text())
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

pub(super) fn parse_units(xml: &str, origin: &DefOrigin) -> Result<Vec<UnitArt>, ArtError> {
    let doc = roxmltree::Document::parse(xml).map_err(|source| ArtError::Xml {
        what: origin.file.clone(),
        source,
    })?;
    let mut out = Vec::new();
    for node in doc.descendants().filter(|n| n.has_tag_name("UnitArtInfo")) {
        let Some(id) = child_text(node, "Type") else {
            tracing::debug!(file = %origin.file, "UnitArtInfo without Type, skipped");
            continue;
        };
        let members = node
            .children()
            .filter(|n| n.has_tag_name("UnitMemberArt"))
            .filter_map(|m| {
                let member_id = child_text(m, "MemberType")?.to_owned();
                let count = child_text(m, "MemberCount")
                    .and_then(|c| c.parse().ok())
                    .unwrap_or(1);
                Some(MemberRef { member_id, count })
            })
            .collect();
        out.push(UnitArt {
            id: id.to_owned(),
            formation: child_text(node, "Formation").map(str::to_owned),
            damage_states: child_text(node, "DamageStates").and_then(|d| d.parse().ok()),
            members,
            origin: origin.clone(),
        });
    }
    Ok(out)
}

pub(super) fn parse_members(
    xml: &str,
    origin: &DefOrigin,
    warnings: &mut Vec<String>,
) -> Result<Vec<UnitMemberArt>, ArtError> {
    let doc = roxmltree::Document::parse(xml).map_err(|source| ArtError::Xml {
        what: origin.file.clone(),
        source,
    })?;
    let mut out = Vec::new();
    for node in doc
        .descendants()
        .filter(|n| n.has_tag_name("UnitMemberArtInfo"))
    {
        let Some(id) = child_text(node, "Type") else {
            tracing::debug!(file = %origin.file, "UnitMemberArtInfo without Type, skipped");
            continue;
        };
        let Some(granny_path) = child_text(node, "Granny") else {
            warnings.push(format!("{}: {id} has no <Granny>", origin.file));
            continue;
        };
        let float = |tag: &str| child_text(node, tag).and_then(|v| v.parse::<f32>().ok());
        let scale = float("fScale").unwrap_or_else(|| {
            warnings.push(format!(
                "{}: {id} has no valid <fScale>, using 1",
                origin.file
            ));
            1.0
        });
        out.push(UnitMemberArt {
            id: id.to_owned(),
            scale,
            z_offset: float("fZOffset"),
            granny_path: granny_path.to_owned(),
            granny_fxsxml: super::flat_name(granny_path),
            domain: child_text(node, "Domain").map(str::to_owned),
            material_tag: child_text(node, "MaterialTypeTag").map(str::to_owned),
            combat_template: node
                .children()
                .find(|n| n.has_tag_name("Combat"))
                .and_then(|c| child_text(c, "Defaults"))
                .map(str::to_owned),
            origin: origin.clone(),
        });
    }
    Ok(out)
}

impl ArtDefines {
    /// Loads and merges every unit art definition file visible in `source`.
    pub fn load(source: &GameSource) -> Result<Self, ArtError> {
        // (source rank, non-canonical, name): canonical base files first, so extra files from the
        // same source override them.
        // Every copy of a file, not only the one a lookup by name returns: the game reads the art
        // definitions of every content package into its database (they are not found by name), so
        // a package's copy adds to or replaces the rows of the earlier ones.
        let mut files: Vec<_> = source
            .iter_ext("xml")
            .flat_map(|e| source.copies(&e.name))
            .filter_map(|e| {
                let kind = classify(&e.name)?;
                let canonical = e.name == CANONICAL_UNITS || e.name == CANONICAL_MEMBERS;
                Some(((e.rank, !canonical, e.name.clone()), kind, e))
            })
            .collect();
        files.sort_by(|a, b| a.0.cmp(&b.0));

        let mut defs = ArtDefines::default();
        for ((_, _, name), kind, entry) in files {
            let origin = DefOrigin {
                file: name.clone(),
                layer: entry.layer,
            };
            let xml = decode_xml(&source.read(entry)?);
            defs.files.push(name.clone());
            match kind {
                FileKind::Units => match parse_units(&xml, &origin) {
                    Ok(rows) => defs.merge_units(rows),
                    Err(e) => defs.warnings.push(e.to_string()),
                },
                FileKind::Members => match parse_members(&xml, &origin, &mut defs.warnings) {
                    Ok(rows) => defs.merge_members(rows),
                    Err(e) => defs.warnings.push(e.to_string()),
                },
            }
        }
        tracing::debug!(
            units = defs.units.len(),
            members = defs.members.len(),
            overridden = defs.overridden_rows,
            "art defines loaded"
        );
        Ok(defs)
    }

    pub(super) fn merge_units(&mut self, rows: Vec<UnitArt>) {
        for row in rows {
            if self.units.insert(key(&row.id), row).is_some() {
                self.overridden_rows += 1;
            }
        }
    }

    pub(super) fn merge_members(&mut self, rows: Vec<UnitMemberArt>) {
        for row in rows {
            if self.members.insert(key(&row.id), row).is_some() {
                self.overridden_rows += 1;
            }
        }
    }

    pub fn unit(&self, id: &str) -> Option<&UnitArt> {
        self.units.get(&key(id))
    }

    pub fn member(&self, id: &str) -> Option<&UnitMemberArt> {
        self.members.get(&key(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin(file: &str, layer: Layer) -> DefOrigin {
        DefOrigin {
            file: file.into(),
            layer,
        }
    }

    const UNITS: &str = r#"<UnitArtInfos>
        <UnitArtInfo>
            <Type>ART_DEF_UNIT_HERO</Type>
            <Formation>Line</Formation>
            <DamageStates>1</DamageStates>
            <UnitMemberArt><MemberType>ART_DEF_UNIT_MEMBER_HERO</MemberType><MemberCount>6</MemberCount></UnitMemberArt>
            <UnitMemberArt><MemberType>ART_DEF_UNIT_MEMBER_HERO_V2</MemberType><MemberCount>3</MemberCount></UnitMemberArt>
        </UnitArtInfo>
        <UnitArtInfo><DamageStates>1</DamageStates></UnitArtInfo>
    </UnitArtInfos>"#;

    const MEMBERS: &str = r#"<UnitMemberArtInfos>
        <UnitMemberArtInfo>
            <Type>ART_DEF_UNIT_MEMBER_HERO</Type>
            <fScale>0.14</fScale>
            <fZOffset>1.5</fZOffset>
            <Granny>Assets/Units/Hero/Hero.fxsxml</Granny>
            <Combat><Defaults>ART_DEF_TEMPLATE_HERO</Defaults></Combat>
            <Domain>Sea</Domain>
            <MaterialTypeTag>CLOTH</MaterialTypeTag>
        </UnitMemberArtInfo>
        <UnitMemberArtInfo><Type>NO_GRANNY</Type><fScale>1</fScale></UnitMemberArtInfo>
    </UnitMemberArtInfos>"#;

    #[test]
    fn parses_unit_rows() {
        let rows = parse_units(UNITS, &origin("u.xml", Layer::Base)).unwrap();
        assert_eq!(rows.len(), 1);
        let u = &rows[0];
        assert_eq!(u.formation.as_deref(), Some("Line"));
        assert_eq!(u.damage_states, Some(1));
        assert_eq!(u.members.len(), 2);
        assert_eq!(u.members[1].member_id, "ART_DEF_UNIT_MEMBER_HERO_V2");
        assert_eq!(u.members[0].count, 6);
    }

    #[test]
    fn parses_member_rows_and_warns_on_gaps() {
        let mut warnings = Vec::new();
        let rows = parse_members(MEMBERS, &origin("m.xml", Layer::Base), &mut warnings).unwrap();
        assert_eq!(rows.len(), 1);
        let m = &rows[0];
        assert!((m.scale - 0.14).abs() < 1e-6);
        assert_eq!(m.z_offset, Some(1.5));
        assert_eq!(m.granny_fxsxml, "hero.fxsxml");
        assert_eq!(m.combat_template.as_deref(), Some("ART_DEF_TEMPLATE_HERO"));
        assert_eq!(m.domain.as_deref(), Some("Sea"));
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn later_row_replaces_earlier_whole() {
        let mut defs = ArtDefines::default();
        let base = parse_units(UNITS, &origin("base.xml", Layer::Base)).unwrap();
        defs.merge_units(base);
        let patch = UNITS.replace("Line", "Box").replace(
            r#"<UnitMemberArt><MemberType>ART_DEF_UNIT_MEMBER_HERO_V2</MemberType><MemberCount>3</MemberCount></UnitMemberArt>"#,
            "",
        );
        let patch = parse_units(&patch, &origin("exp.xml", Layer::Expansion)).unwrap();
        defs.merge_units(patch);

        let u = defs.unit("art_def_unit_hero").unwrap();
        assert_eq!(u.formation.as_deref(), Some("Box"));
        assert_eq!(u.members.len(), 1);
        assert_eq!(u.origin.layer, Layer::Expansion);
        assert_eq!(defs.overridden_rows, 1);
    }

    #[test]
    fn file_classification() {
        assert!(classify("civ5artdefines_units.xml") == Some(FileKind::Units));
        assert!(classify("civ5artdefines_korea_unitmembers.xml") == Some(FileKind::Members));
        assert!(classify("civ5artdefines_sv_units.xml").is_none());
        assert!(classify("civ5artdefines_unitmembertemplates.xml").is_none());
        assert!(classify("civ5units.xml").is_none());
    }
}
