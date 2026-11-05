//! The sounds of the units' animations (re-b15 ITEM 4): the sound rows of a model's timed triggers fire as the clip of
//! their `ec` passes their time, each member of a unit for itself, with the macros of the row's script filled from the
//! plot under the member and the art of the unit, and the rows of the other formation sizes left out (the tier).
//!
//! No output and no clock here: [`UnitSoundBook::crossed`] says which rows a clip passed between two looks, the caller
//! plays them (`UiAudio::play_3d`).

use std::collections::HashMap;

use asset_format::{ModelSounds, Sound3DTable, SoundTrigger};

/// The rows of every model, by model (the `.fxsxml` name in lower case).
pub struct UnitSoundBook {
    models: HashMap<String, ModelSounds>,
}

/// The tier of a unit of `members` living members (`GetCurrentTierFromUnitHandle`): 9 and more 2, 4 to 8 one, else 0.
pub fn tier_of(members: usize) -> i8 {
    match members {
        n if n >= 9 => 2,
        4..=8 => 1,
        _ => 0,
    }
}

/// Whether a row of `tier` plays for a unit of `members` (`IsOKForTierSoundToPlay`: `-1` always).
pub fn tier_fits(tier: i8, members: usize) -> bool {
    tier < 0 || tier == tier_of(members)
}

/// `EffectTypeTag` of the plot under a member, the value of `$(BASETERRAIN)` (and of `$(TERRAIN)` for one that does not
/// fly): the feature's tag if it has one, else the terrain's, else `DIRT` (`GetTerrainTagAtCenterPosition`; tags of
/// `CIV5Terrains.xml` / `CIV5Features.xml`).
pub fn terrain_tag(terrain: Option<&str>, feature: Option<&str>) -> &'static str {
    let by_feature = match feature {
        Some("FEATURE_FOREST" | "FEATURE_JUNGLE") => Some("LEAVES"),
        Some("FEATURE_MARSH") => Some("MUD"),
        Some("FEATURE_OASIS") => Some("WATER"),
        Some("FEATURE_ICE") => Some("SNOW"),
        _ => None,
    };
    by_feature.unwrap_or(match terrain {
        Some("TERRAIN_DESERT") => "SAND",
        Some("TERRAIN_SNOW") => "SNOW",
        Some("TERRAIN_COAST" | "TERRAIN_OCEAN") => "WATER",
        _ => "DIRT",
    })
}

/// The values of a row's macros for one member.
#[derive(Debug, Clone, Copy, Default)]
pub struct Macros<'a> {
    /// `$(TERRAIN)` and `$(BASETERRAIN)`: [`terrain_tag`] of the plot under the member.
    pub terrain: &'a str,
    /// `$(WEAPON)`: the member's own impact weapon, else the opponent's.
    pub weapon: Option<&'a str>,
    /// `$(MATERIAL)`.
    pub material: Option<&'a str>,
}

/// The script a row names for a member, its macros filled (`cvEffectVariantSystem::GenerateVariantHandle`); `None` when
/// a macro has no value.
///
/// SIMPLIFIED(SIMP-exe-1201): `$(TERRAIN)` is never `AIR` (the port does not tell a flying member).
pub fn resolve(event: &str, m: &Macros<'_>) -> Option<String> {
    let mut s = event.to_owned();
    for (name, value) in [
        ("$(BASETERRAIN)", Some(m.terrain)),
        ("$(TERRAIN)", Some(m.terrain)),
        ("$(WEAPON)", m.weapon),
        ("$(MATERIAL)", m.material),
    ] {
        if s.contains(name) {
            s = s.replace(name, value.filter(|v| !v.is_empty())?);
        }
    }
    Some(s)
}

impl UnitSoundBook {
    pub fn new(table: &Sound3DTable) -> Self {
        Self {
            models: table
                .models
                .iter()
                .map(|m| (m.model.clone(), m.clone()))
                .collect(),
        }
    }

    pub fn len(&self) -> usize {
        self.models.len()
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    /// The rows of a model by its id (`md:archer` or `archer`).
    pub fn model(&self, id: &str) -> Option<&ModelSounds> {
        let name = id.strip_prefix("md:").unwrap_or(id);
        self.models.get(&name.to_ascii_lowercase())
    }

    /// The rows of `ec` of a model that a clip passed going from `from` to `to` (seconds in the clip): `from` is the
    /// time at the last look, `None` when the clip has just started (then from its start, rows at 0 included). A
    /// looping clip that wrapped (`to < from`) passed the end of the clip and its start.
    pub fn crossed(&self, model: &str, ec: u32, from: Option<f32>, to: f32) -> Vec<&SoundTrigger> {
        let Some(m) = self.model(model) else {
            return Vec::new();
        };
        m.triggers
            .iter()
            .filter(|t| t.ec == ec)
            .filter(|t| match from {
                None => t.time <= to,
                Some(f) if to >= f => t.time > f && t.time <= to,
                Some(f) => t.time > f || t.time <= to,
            })
            .collect()
    }

    /// Every row of `ec` of a model, for a clip the port does not play (the rows are timed from the call).
    pub fn rows(&self, model: &str, ec: u32) -> Vec<&SoundTrigger> {
        self.crossed(model, ec, None, f32::INFINITY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(ec: u32, time: f32, tier: i8, event: &str) -> SoundTrigger {
        SoundTrigger {
            ec,
            time,
            tier,
            event: event.into(),
        }
    }

    fn book() -> UnitSoundBook {
        UnitSoundBook::new(&Sound3DTable {
            scripts: vec![],
            models: vec![ModelSounds {
                model: "archer".into(),
                weapon: Some("ARROW".into()),
                material: Some("CLOTH".into()),
                triggers: vec![
                    row(1400, 0.0, -1, "AS3D_A_FOOT_$(TERRAIN)"),
                    row(1400, 0.4, 0, "AS3D_A_FOOT_$(TERRAIN)"),
                    row(1400, 0.8, 1, "AS3D_A_T1_RUN"),
                    row(1500, 0.1, -1, "AS3D_A_FORTIFY"),
                ],
            }],
            pitch_2d: vec![],
        })
    }

    fn times(rows: Vec<&SoundTrigger>) -> Vec<f32> {
        rows.iter().map(|r| r.time).collect()
    }

    #[test]
    fn a_row_fires_once_as_the_clip_passes_its_time() {
        let b = book();
        assert_eq!(
            times(b.crossed("md:archer", 1400, Some(0.1), 0.3)),
            Vec::<f32>::new()
        );
        assert_eq!(times(b.crossed("md:archer", 1400, Some(0.3), 0.4)), [0.4]);
        assert_eq!(
            times(b.crossed("md:archer", 1400, Some(0.4), 0.5)),
            Vec::<f32>::new(),
            "not twice"
        );
        // A loop that wrapped passed the end and the start.
        assert_eq!(
            times(b.crossed("ARCHER", 1400, Some(0.7), 0.05)),
            [0.0, 0.8]
        );
        // A clip that just began: from its start, the row at 0 included.
        assert_eq!(times(b.crossed("archer", 1500, None, 0.2)), [0.1]);
        assert!(b.crossed("archer", 1500, None, 0.05).is_empty());
        assert!(b.crossed("nope", 1500, None, 9.0).is_empty());
        assert_eq!(b.rows("archer", 1400).len(), 3);
    }

    #[test]
    fn the_tier_follows_the_living_members() {
        assert_eq!(
            (
                tier_of(1),
                tier_of(3),
                tier_of(4),
                tier_of(8),
                tier_of(9),
                tier_of(16)
            ),
            (0, 0, 1, 1, 2, 2)
        );
        assert!(tier_fits(-1, 3) && tier_fits(0, 3) && !tier_fits(1, 3));
        assert!(tier_fits(1, 6) && !tier_fits(0, 6));
    }

    #[test]
    fn macros_take_the_plot_and_the_art_and_a_missing_one_names_nothing() {
        let m = Macros {
            terrain: terrain_tag(Some("TERRAIN_GRASS"), Some("FEATURE_FOREST")),
            weapon: Some("ARROW"),
            material: None,
        };
        assert_eq!(
            resolve("AS3D_A_FOOT_$(TERRAIN)", &m).as_deref(),
            Some("AS3D_A_FOOT_LEAVES")
        );
        assert_eq!(
            resolve("AS3D_A_FALL_$(BASETERRAIN)", &m).as_deref(),
            Some("AS3D_A_FALL_LEAVES")
        );
        assert_eq!(
            resolve("AS3D_A_IMPACT_$(WEAPON)", &m).as_deref(),
            Some("AS3D_A_IMPACT_ARROW")
        );
        assert_eq!(resolve("AS3D_A_HIT_$(MATERIAL)", &m), None);
        assert_eq!(
            resolve("AS3D_A_FORTIFY", &m).as_deref(),
            Some("AS3D_A_FORTIFY")
        );
        assert_eq!(terrain_tag(Some("TERRAIN_DESERT"), None), "SAND");
        assert_eq!(terrain_tag(Some("TERRAIN_COAST"), None), "WATER");
        assert_eq!(
            terrain_tag(Some("TERRAIN_SNOW"), Some("FEATURE_MARSH")),
            "MUD"
        );
        assert_eq!(terrain_tag(None, None), "DIRT");
    }
}
