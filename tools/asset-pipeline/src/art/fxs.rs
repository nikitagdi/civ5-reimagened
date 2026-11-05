//! `.fxsxml` parser: the per-model asset list.

use super::ArtError;

/// Role of a texture, derived from its file name (the fxsxml has no slot attribute).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextureSlot {
    /// `_DIFF`: albedo; alpha is the team-colour mask.
    Diffuse,
    /// `_SREF`: specular/reflection.
    SpecRef,
    Normal,
    Opacity,
    Mask,
    /// Shared dull/sharp environment maps.
    Environment,
    Irradiance,
    CubeLight,
    Other,
}

impl TextureSlot {
    pub fn classify(file: &str) -> Self {
        let lower = file.to_ascii_lowercase();
        let stem = lower.rsplit_once('.').map_or(lower.as_str(), |(s, _)| s);
        // Lighting maps first: `Unit_Environment_Dull` has no role token otherwise.
        if stem.contains("environment") {
            return Self::Environment;
        }
        if stem.contains("irradiance") || stem.contains("irridiance") {
            return Self::Irradiance;
        }
        if stem.contains("cubelight") {
            return Self::CubeLight;
        }
        for token in stem.split('_') {
            match token {
                "diff" | "diffuse" => return Self::Diffuse,
                "sref" => return Self::SpecRef,
                "norm" => return Self::Normal,
                "opac" => return Self::Opacity,
                "mask" => return Self::Mask,
                _ => {}
            }
        }
        Self::Other
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FxsTexture {
    /// File name as written (original case).
    pub file: String,
    pub slot: TextureSlot,
    /// Lower-case stem without the role suffix (`antitankgun_crew` for
    /// `AntiTankGun_Crew_DIFF.dds`); groups the DIFF/SREF pair of one material.
    pub material_key: String,
}

impl FxsTexture {
    fn new(file: String) -> Self {
        let slot = TextureSlot::classify(&file);
        let lower = file.to_ascii_lowercase();
        let stem = lower.rsplit_once('.').map_or(lower.as_str(), |(s, _)| s);
        let material_key = match (slot, stem.rsplit_once('_')) {
            (
                TextureSlot::Diffuse
                | TextureSlot::SpecRef
                | TextureSlot::Normal
                | TextureSlot::Opacity
                | TextureSlot::Mask,
                Some((base, _)),
            ) => base,
            _ => stem,
        }
        .to_owned();
        FxsTexture {
            file,
            slot,
            material_key,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FxsAnim {
    /// gr2 file name as written.
    pub file: String,
    /// Event codes that trigger this clip; empty if `ec` is absent.
    pub event_codes: Vec<u32>,
    /// Explicit label (non-unit assets only).
    pub name: Option<String>,
    /// Layer tag (`None` is written literally in the data and kept as-is).
    pub layer: Option<String>,
}

impl FxsAnim {
    /// `name` if present, else the gr2 file stem (`Warrior_IdleA`).
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or_else(|| {
            self.file
                .rsplit_once('.')
                .map_or(self.file.as_str(), |(s, _)| s)
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FxsModel {
    pub meshes: Vec<String>,
    pub textures: Vec<FxsTexture>,
    pub animations: Vec<FxsAnim>,
    /// `.ftsxml` names.
    pub timed_triggers: Vec<String>,
    /// `.dge` names.
    pub anim_graphs: Vec<String>,
    /// `.fsmxml` names.
    pub state_machines: Vec<String>,
    pub decision_trees: Vec<String>,
    pub bones: Vec<String>,
}

fn parse_event_codes(raw: &str, file: &str) -> Vec<u32> {
    raw.split(',')
        .filter_map(|part| {
            let part = part.trim();
            if part.is_empty() {
                return None;
            }
            match part.parse() {
                Ok(code) => Some(code),
                Err(_) => {
                    tracing::debug!(file, code = part, "unparsable animation event code");
                    None
                }
            }
        })
        .collect()
}

/// Parses `.fxsxml` text. Unknown elements and elements without `file` are skipped with a
/// debug log.
pub fn parse_fxsxml(xml: &str) -> Result<FxsModel, ArtError> {
    let doc = roxmltree::Document::parse(xml).map_err(|source| ArtError::Xml {
        what: "fxsxml".into(),
        source,
    })?;
    let root = doc.root_element();
    if root.tag_name().name() != "Asset" {
        return Err(ArtError::BadRoot {
            what: "fxsxml".into(),
            expected: "Asset",
            found: root.tag_name().name().into(),
        });
    }

    let mut model = FxsModel::default();
    for node in root.children().filter(|n| n.is_element()) {
        let tag = node.tag_name().name();
        if tag == "BoneUsage" {
            model.bones.extend(
                node.children()
                    .filter(|n| n.has_tag_name("Bone"))
                    .filter_map(|n| n.attribute("name"))
                    .map(str::to_owned),
            );
            continue;
        }
        let Some(file) = node
            .attribute("file")
            .map(str::trim)
            .filter(|f| !f.is_empty())
        else {
            tracing::debug!(tag, "fxsxml element without a file attribute, skipped");
            continue;
        };
        let file = file.to_owned();
        match tag {
            "Mesh" => model.meshes.push(file),
            "Texture" => model.textures.push(FxsTexture::new(file)),
            "Animation" => {
                let event_codes = node
                    .attribute("ec")
                    .map_or_else(Vec::new, |ec| parse_event_codes(ec, &file));
                model.animations.push(FxsAnim {
                    event_codes,
                    name: node.attribute("name").map(str::to_owned),
                    layer: node.attribute("layer").map(str::to_owned),
                    file,
                });
            }
            "TimedTrigger" => model.timed_triggers.push(file),
            "AnimGraph" => model.anim_graphs.push(file),
            "StateMachine" => model.state_machines.push(file),
            "DecisionTree" => model.decision_trees.push(file),
            other => tracing::debug!(tag = other, "unknown fxsxml element, skipped"),
        }
    }
    Ok(model)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<Asset>
        <Mesh file="Hero.gr2"/>
        <Animation file="Hero_IdleA.gr2" ec="1000"/>
        <Animation file="Hero_AttackCity.gr2" ec="1160, 1180"/>
        <Animation file="Hero_NoCode.gr2"/>
        <Animation file="Prop_Idle.gr2" ec="1000" name="Idle" layer="None" source="Max"/>
        <Texture file="Unit_Environment_Dull.dds"/>
        <Texture file="Unit_Irradiance.dds"/>
        <Texture file="Hero_SREF.dds"/>
        <Texture file="Hero_DIFF.dds"/>
        <TimedTrigger file="FX_Hero.ftsxml"/>
        <AnimGraph file="IDLE.dge"/>
        <StateMachine file="1_OMNI.fsmxml"/>
        <BoneUsage><Bone name="bone_CLUB"/><Bone name="Base HumanSpine2"/></BoneUsage>
        <Mystery file="x"/>
        <Mesh/>
    </Asset>"#;

    #[test]
    fn parses_all_sections() {
        let m = parse_fxsxml(SAMPLE).unwrap();
        assert_eq!(m.meshes, ["Hero.gr2"]);
        assert_eq!(m.animations.len(), 4);
        assert_eq!(m.animations[0].event_codes, [1000]);
        assert_eq!(m.animations[1].event_codes, [1160, 1180]);
        assert!(m.animations[2].event_codes.is_empty());
        assert_eq!(m.animations[3].name.as_deref(), Some("Idle"));
        assert_eq!(m.animations[3].layer.as_deref(), Some("None"));
        assert_eq!(m.animations[0].label(), "Hero_IdleA");
        assert_eq!(m.animations[3].label(), "Idle");
        assert_eq!(m.timed_triggers, ["FX_Hero.ftsxml"]);
        assert_eq!(m.anim_graphs, ["IDLE.dge"]);
        assert_eq!(m.state_machines, ["1_OMNI.fsmxml"]);
        assert_eq!(m.bones, ["bone_CLUB", "Base HumanSpine2"]);
    }

    #[test]
    fn classifies_texture_slots_in_file_order() {
        let m = parse_fxsxml(SAMPLE).unwrap();
        let slots: Vec<_> = m.textures.iter().map(|t| t.slot).collect();
        assert_eq!(
            slots,
            [
                TextureSlot::Environment,
                TextureSlot::Irradiance,
                TextureSlot::SpecRef,
                TextureSlot::Diffuse
            ]
        );
        assert_eq!(
            TextureSlot::classify("Archer_02_DIFF.dds"),
            TextureSlot::Diffuse
        );
        assert_eq!(
            TextureSlot::classify("CubeLight_Generic_32.dds"),
            TextureSlot::CubeLight
        );
        assert_eq!(TextureSlot::classify("Hero_NORM.dds"), TextureSlot::Normal);
        assert_eq!(TextureSlot::classify("misc.dds"), TextureSlot::Other);
    }

    #[test]
    fn material_key_groups_role_pairs() {
        let m = parse_fxsxml(
            r#"<Asset><Texture file="Gun_Crew_DIFF.dds"/><Texture file="Gun_Crew_SREF.dds"/>
               <Texture file="Unit_Irradiance.dds"/></Asset>"#,
        )
        .unwrap();
        let keys: Vec<_> = m.textures.iter().map(|t| t.material_key.as_str()).collect();
        assert_eq!(keys, ["gun_crew", "gun_crew", "unit_irradiance"]);
    }

    #[test]
    fn bad_event_code_is_dropped_not_fatal() {
        let m = parse_fxsxml(r#"<Asset><Animation file="a.gr2" ec="12, x, 14"/></Asset>"#).unwrap();
        assert_eq!(m.animations[0].event_codes, [12, 14]);
    }

    #[test]
    fn rejects_wrong_root_and_broken_xml() {
        assert!(matches!(
            parse_fxsxml("<Other/>"),
            Err(ArtError::BadRoot { .. })
        ));
        assert!(matches!(parse_fxsxml("<Asset>"), Err(ArtError::Xml { .. })));
    }
}
