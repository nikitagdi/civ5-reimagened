//! Unit sound table (`C5S3`, manifest id `ui:snd3d`): the 3D sound scripts of the original (`Audio3DScripts.xml` and
//! the per-unit files it includes, `Script3DSound`), the sound rows of the units' timed triggers (`.ftsxml`,
//! `FTimedTriggerSound`) by model, and the pitch ranges of the 2D scripts (the 2D table has no field for them). The
//! sound definitions the scripts name are in the [`crate::AudioTable`] (`ui:snd`) with the rest.
//!
//! re-b15 ITEM 4: a unit's sounds are not played by the Lua or the DLL; a row fires when the clip of its `ec` passes its
//! `time` in the animation of a member of the unit.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{FormatError, SoundKind, blob};

pub const SOUND3D_TABLE_MAGIC: [u8; 4] = *b"C5S3";
pub const SOUND3D_TABLE_ID: &str = "ui:snd3d";

/// `fMinDistance` of `AS3D_DEFAULT_VALUES` (`Audio3DScripts.xml`), in tiles: full volume up to it.
pub const DEFAULT_MIN_DISTANCE: f32 = 1.5;
/// `fCutoffDistance` of `AS3D_DEFAULT_VALUES`, in tiles: silent from it.
pub const DEFAULT_CUTOFF_DISTANCE: f32 = 6.0;

/// One `Script3DSound`, with the defaults of `AS3D_DEFAULT_VALUES` filled in. Times are milliseconds, volumes
/// percent, distances tiles (`f3dDistanceMultiplier` 64 world units each), pitch semitones.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Script3D {
    /// `ScriptID`, e.g. `AS3D_UNIT_ARCHER_FOOT_DIRT`.
    pub name: String,
    /// `SoundID`, key of the 2D table's `sounds`.
    pub sound: String,
    pub kind: SoundKind,
    pub min_volume: u16,
    pub max_volume: u16,
    /// `iPitchChangeDown` (0 or below) and `iPitchChangeUp`: a draw in `down..=up` per play.
    pub pitch_down: i8,
    pub pitch_up: i8,
    pub looping: bool,
    pub dont_trigger_duplicates: bool,
    /// `iDontPlayMoreThan`; 0 when absent (the engine's own cap then holds).
    pub max_concurrent: u16,
    pub min_delay_ms: u32,
    pub max_delay_ms: u32,
    pub min_gap_ms: u32,
    pub max_gap_ms: u32,
    pub chance_percent: u8,
    /// `fMinDistance`: full volume up to it.
    pub min_distance: f32,
    /// `fCutoffDistance`: silent from it (`fMaxDistance` is not read by the exe).
    pub cutoff_distance: f32,
}

impl Script3D {
    pub fn new(name: &str, sound: &str) -> Self {
        Self {
            name: name.to_owned(),
            sound: sound.to_owned(),
            kind: SoundKind::Sfx,
            min_volume: 100,
            max_volume: 100,
            pitch_down: 0,
            pitch_up: 0,
            looping: false,
            dont_trigger_duplicates: false,
            max_concurrent: 0,
            min_delay_ms: 0,
            max_delay_ms: 0,
            min_gap_ms: 0,
            max_gap_ms: 0,
            chance_percent: 100,
            min_distance: DEFAULT_MIN_DISTANCE,
            cutoff_distance: DEFAULT_CUTOFF_DISTANCE,
        }
    }
}

/// One `FTimedTriggerSound` row of an `.ftsxml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SoundTrigger {
    /// Event code of the clip (`1400` run, `1500` fortify, ...).
    pub ec: u32,
    /// Seconds into the clip.
    pub time: f32,
    /// `-1` always; `0..=2` only for a unit of that many living members (`<= 3`, `4..=8`, `>= 9`).
    pub tier: i8,
    /// The script, possibly with the macros `$(TERRAIN)`, `$(BASETERRAIN)`, `$(WEAPON)`, `$(MATERIAL)`.
    pub event: String,
}

/// The sound rows of one model (one `.fxsxml`) and the tags its art define gives the macros.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelSounds {
    /// `.fxsxml` name without extension, lower case (the `md:` id of the model without the prefix).
    pub model: String,
    /// `WeaponTypeSoundOverrideTag`, else `WeaponTypeTag` of the first weapon of the unit member that uses the model.
    pub weapon: Option<String>,
    /// `MaterialTypeSoundOverrideTag`, else `MaterialTypeTag`.
    pub material: Option<String>,
    /// Sorted by (ec, time).
    pub triggers: Vec<SoundTrigger>,
}

/// The pitch range of a 2D script (`iPitchChangeDown`, `iPitchChangeUp`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PitchRange {
    pub script: String,
    pub down: i8,
    pub up: i8,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Sound3DTable {
    /// Sorted by name.
    pub scripts: Vec<Script3D>,
    /// Sorted by model.
    pub models: Vec<ModelSounds>,
    /// Sorted by script.
    pub pitch_2d: Vec<PitchRange>,
}

impl Sound3DTable {
    pub const VERSION: u16 = 1;
    pub const MIN_VERSION: u16 = 1;

    /// Deterministic: everything written sorted.
    pub fn encode(&self) -> Vec<u8> {
        let mut t = self.clone();
        t.scripts.sort_by(|a, b| a.name.cmp(&b.name));
        t.models.sort_by(|a, b| a.model.cmp(&b.model));
        for m in &mut t.models {
            m.triggers.sort_by(|a, b| {
                (a.ec, a.time)
                    .partial_cmp(&(b.ec, b.time))
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        t.pitch_2d.sort_by(|a, b| a.script.cmp(&b.script));
        blob::encode_v(SOUND3D_TABLE_MAGIC, Self::VERSION, &t)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let (_, t): (u16, Self) =
            blob::decode_range(SOUND3D_TABLE_MAGIC, Self::MIN_VERSION, Self::VERSION, bytes)?;
        t.check()?;
        Ok(t)
    }

    pub fn check(&self) -> Result<(), FormatError> {
        if let Some(w) = self.scripts.windows(2).find(|w| w[0].name >= w[1].name) {
            return Err(FormatError::invalid(format!(
                "3D scripts are not sorted or repeat: {:?} then {:?}",
                w[0].name, w[1].name
            )));
        }
        if let Some(w) = self.models.windows(2).find(|w| w[0].model >= w[1].model) {
            return Err(FormatError::invalid(format!(
                "models are not sorted or repeat: {:?} then {:?}",
                w[0].model, w[1].model
            )));
        }
        if self.pitch_2d.windows(2).any(|w| w[0].script >= w[1].script) {
            return Err(FormatError::invalid(
                "2D pitch ranges are not sorted or repeat",
            ));
        }
        for s in &self.scripts {
            if s.min_volume > s.max_volume || s.pitch_down > s.pitch_up || s.chance_percent > 100 {
                return Err(FormatError::invalid(format!(
                    "3D script {:?}: a range is upside down",
                    s.name
                )));
            }
            if !(s.min_distance >= 0.0 && s.cutoff_distance.is_finite()) {
                return Err(FormatError::invalid(format!(
                    "3D script {:?}: distances {}..{}",
                    s.name, s.min_distance, s.cutoff_distance
                )));
            }
        }
        for m in &self.models {
            if m.triggers.iter().any(|t| !t.time.is_finite()) {
                return Err(FormatError::invalid(format!(
                    "model {:?}: a trigger time is not a number",
                    m.model
                )));
            }
        }
        Ok(())
    }

    pub fn script(&self, name: &str) -> Option<&Script3D> {
        self.scripts
            .binary_search_by(|s| s.name.as_str().cmp(name))
            .ok()
            .map(|i| &self.scripts[i])
    }

    pub fn model(&self, model: &str) -> Option<&ModelSounds> {
        self.models
            .binary_search_by(|m| m.model.as_str().cmp(model))
            .ok()
            .map(|i| &self.models[i])
    }

    /// Every sound id the 3D scripts name (the pipeline puts their files in the set).
    pub fn sounds(&self) -> BTreeSet<&str> {
        self.scripts.iter().map(|s| s.sound.as_str()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> Sound3DTable {
        let mut foot = Script3D::new("AS3D_UNIT_ARCHER_FOOT_DIRT", "SND_UNIT_FOOT_DIRT");
        foot.pitch_down = -4;
        foot.pitch_up = 3;
        Sound3DTable {
            scripts: vec![
                foot,
                Script3D::new("AS3D_UNIT_ARCHER_FORTIFY", "SND_FORTIFY"),
            ],
            models: vec![ModelSounds {
                model: "archer".into(),
                weapon: Some("ARROW".into()),
                material: Some("CLOTH".into()),
                triggers: vec![
                    SoundTrigger {
                        ec: 1500,
                        time: 0.8,
                        tier: -1,
                        event: "AS3D_UNIT_ARCHER_FORTIFY".into(),
                    },
                    SoundTrigger {
                        ec: 1400,
                        time: 0.5,
                        tier: 0,
                        event: "AS3D_UNIT_ARCHER_FOOT_$(TERRAIN)".into(),
                    },
                ],
            }],
            pitch_2d: vec![PitchRange {
                script: "AS2D_X".into(),
                down: -2,
                up: 2,
            }],
        }
    }

    #[test]
    fn a_table_round_trips_sorted_and_deterministic() {
        let bytes = table().encode();
        assert_eq!(&bytes[..4], b"C5S3");
        let back = Sound3DTable::decode(&bytes).unwrap();
        assert_eq!(back.encode(), bytes);
        assert_eq!(back.scripts[0].name, "AS3D_UNIT_ARCHER_FOOT_DIRT");
        let m = back.model("archer").unwrap();
        assert_eq!(m.triggers[0].ec, 1400, "triggers sorted by clip");
        assert_eq!(
            back.script("AS3D_UNIT_ARCHER_FOOT_DIRT")
                .unwrap()
                .pitch_down,
            -4
        );
        assert_eq!(back.sounds().len(), 2);
        assert!(back.model("nope").is_none());
    }

    #[test]
    fn a_new_script_has_the_default_distances_of_the_original() {
        let s = Script3D::new("AS3D_X", "SND_X");
        assert_eq!((s.min_distance, s.cutoff_distance), (1.5, 6.0));
        assert_eq!(
            (s.min_volume, s.max_volume, s.chance_percent),
            (100, 100, 100)
        );
    }

    #[test]
    fn an_upside_down_range_or_another_blob_is_refused() {
        let mut t = table();
        t.scripts[0].pitch_down = 5;
        let bytes = blob::encode_v(SOUND3D_TABLE_MAGIC, Sound3DTable::VERSION, &t);
        assert!(Sound3DTable::decode(&bytes).is_err());
        assert!(Sound3DTable::decode(&crate::AudioTable::default().encode()).is_err());
    }
}
