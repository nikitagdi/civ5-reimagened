//! The visual-effect index `fx:index`: `Civ5ArtDefines_VisEffects*.xml`
//! (name, transform, duration, FOW mask, `ParticleEffect` names) joined with
//! `Civ5ArtDefines_VisEffectParticles*.xml` (`ART_DEF_PEFFECT_*` to the `.psb` file).

use std::collections::{BTreeMap, BTreeSet};

use asset_format::{FxIndex, FxVisEffect, LogicalId, VisEffectArt};

/// Id of the effect of a `.psb` file: `fx:` and the file name without extension, lower case.
pub fn fx_id(file: &str) -> LogicalId {
    let f = super::psb::file_name(file).to_ascii_lowercase();
    let stem = f.strip_suffix(".psb").unwrap_or(&f);
    LogicalId::new(format!("fx:{stem}"))
}

/// True for `civ5artdefines_viseffectparticles.xml` and its DLC files.
pub fn is_particles_file(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.ends_with(".xml")
        && (n == "civ5artdefines_viseffectparticles.xml"
            || n.starts_with("civ5artdefines_viseffectparticles_"))
}

/// `VisEffectParticleArtInfo` rows: name and `.psb` file (as written, with its folders).
pub fn parse_particles(xml: &str) -> Result<Vec<(String, String)>, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| format!("invalid XML: {e}"))?;
    let mut out = Vec::new();
    for e in doc
        .root_element()
        .children()
        .filter(|n| n.has_tag_name("VisEffectParticleArtInfo"))
    {
        let text = |tag: &str| {
            e.children()
                .find(|n| n.has_tag_name(tag))
                .and_then(|n| n.text())
                .map(|t| t.trim().to_owned())
        };
        if let (Some(name), Some(file)) = (text("Name"), text("File")) {
            out.push((name, file));
        }
    }
    Ok(out)
}

/// Particle names of several files merged: base file first, then the others by name; a later
/// row with the same name wins.
pub fn merge_particles(files: &[(String, Vec<(String, String)>)]) -> BTreeMap<String, String> {
    let mut order: Vec<&(String, Vec<(String, String)>)> = files.iter().collect();
    order.sort_by_key(|(n, _)| (n != "civ5artdefines_viseffectparticles.xml", n.clone()));
    let mut out = BTreeMap::new();
    for (_, rows) in order {
        for (n, f) in rows {
            out.insert(n.clone(), f.clone());
        }
    }
    out
}

/// The index: every visual effect with at least one particle effect whose `.psb` was built
/// (`built` holds the `fx:` ids). Returns the index and the particle names that have no file or
/// no built effect.
pub fn build_index(
    vis: &[VisEffectArt],
    particles: &BTreeMap<String, String>,
    built: &BTreeSet<LogicalId>,
) -> (FxIndex, Vec<String>) {
    let mut missing = BTreeSet::new();
    let mut effects: Vec<FxVisEffect> = Vec::new();
    for v in vis {
        let ids: Vec<LogicalId> = v
            .particles
            .iter()
            .filter_map(|p| match particles.get(p).map(|f| fx_id(f)) {
                Some(id) if built.contains(&id) => Some(id),
                _ => {
                    missing.insert(p.clone());
                    None
                }
            })
            .collect();
        if ids.is_empty() {
            continue;
        }
        effects.push(FxVisEffect {
            name: v.name.clone(),
            translation: v.translation,
            rotation: v.rotation,
            scale: v.scale,
            duration: v.base_duration * v.base_duration_scale,
            // re: cvVisEffectInfo +0xba = FOWMask & 3, default 2 (answers-b update 9)
            fow_mask: v.fow_mask.map_or(2, |m| m & 3),
            particles: ids,
        });
    }
    effects.sort_by(|a, b| a.name.cmp(&b.name));
    effects.dedup_by(|a, b| a.name == b.name);
    (FxIndex { effects }, missing.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vis(name: &str, particles: &[&str]) -> VisEffectArt {
        VisEffectArt {
            name: name.into(),
            translation: [0.0; 3],
            rotation: [90.0, 0.0, 0.0],
            scale: 4.2,
            base_duration: 1.1,
            base_duration_scale: 2.0,
            movement_speed: None,
            fow_mask: Some(7),
            particles: particles.iter().map(|s| (*s).to_owned()).collect(),
            models: vec![],
            children: vec![],
            sounds: vec![],
        }
    }

    #[test]
    fn the_index_joins_names_to_built_files() {
        let xml = r#"<VisEffectParticleArtInfos>
          <VisEffectParticleArtInfo><Name>P_A</Name><File>Assets\Effects\Level_Up\FX_Look_At_Me.psb</File></VisEffectParticleArtInfo>
          <!-- <VisEffectParticleArtInfo><Name>P_C</Name><File>x.psb</File></VisEffectParticleArtInfo> -->
          <VisEffectParticleArtInfo><Name>P_B</Name><File>gone.psb</File></VisEffectParticleArtInfo>
        </VisEffectParticleArtInfos>"#;
        let rows = parse_particles(xml).unwrap();
        assert_eq!(rows.len(), 2);
        let dlc = vec![("P_B".to_owned(), "fx_b.psb".to_owned())];
        let map = merge_particles(&[
            ("civ5artdefines_viseffectparticles_dlc_01.xml".into(), dlc),
            ("civ5artdefines_viseffectparticles.xml".into(), rows),
        ]);
        assert_eq!(map["P_B"], "fx_b.psb", "the DLC file wins");
        let built: BTreeSet<LogicalId> = [LogicalId::new("fx:fx_look_at_me")].into();
        let (idx, missing) = build_index(
            &[vis("V_LOOK", &["P_A", "P_B"]), vis("V_NONE", &["P_B"])],
            &map,
            &built,
        );
        assert_eq!(idx.effects.len(), 1);
        let e = idx.get("V_LOOK").unwrap();
        assert_eq!(e.particles, [LogicalId::new("fx:fx_look_at_me")]);
        assert_eq!(e.fow_mask, 3);
        assert!((e.duration - 2.2).abs() < 1e-6);
        assert_eq!(missing, ["P_B"]);
        assert!(is_particles_file(
            "CIV5ArtDefines_VisEffectParticles_DLC_02.xml"
        ));
        assert!(!is_particles_file("civ5artdefines_viseffects.xml"));
        assert_eq!(fx_id("Assets\\Effects\\FX_A.PSB").as_str(), "fx:fx_a");
    }
}
