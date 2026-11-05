//! World soundscape table (`C5SC`, manifest id `ui:scape`): the ambient loops of the map
//! (`Sounds\XML\AudioSoundscapeScripts.xml`) and which terrain, feature and improvement rows use which
//! (`WorldSoundscapeAudioScript` of the gameplay tables). The 2D scripts the elements name are in the
//! [`crate::AudioTable`] (`ui:snd`); this table only orders them.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{FormatError, blob};

pub const SOUNDSCAPE_TABLE_MAGIC: [u8; 4] = *b"C5SC";
pub const SOUNDSCAPE_TABLE_ID: &str = "ui:scape";

/// One `SoundscapeElement`: a looping bed, or sounds picked at random now and then.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScapeElement {
    /// `ScriptID`s of the element (2D script names). A bed has one; a random element picks one of them each time.
    pub scripts: Vec<String>,
    /// `bIsSoundBed`: the element loops as long as the soundscape is heard.
    pub bed: bool,
    /// `iMinTimeMustNotPlayAgain`, `iMaxTimeMustNotPlayAgain` (ms): the wait before the element may sound again.
    pub min_gap_ms: u32,
    pub max_gap_ms: u32,
}

/// One `ScriptSoundscape`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Soundscape {
    /// `ScriptID`, e.g. `ASSS_AMBIENCE_FOREST`.
    pub id: String,
    /// `fMinGridDistance`, `fMaxGridDistance`: full volume up to the first, silent from the second.
    pub min_distance: f32,
    pub max_distance: f32,
    /// `iMinVolume`, `iMaxVolume` (percent).
    pub min_volume: u16,
    pub max_volume: u16,
    /// `fPriority` (1 when absent): the weight of the loop when the dominant sound of an area is chosen.
    pub priority: f32,
    pub elements: Vec<ScapeElement>,
}

/// Which table a binding comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ScapeBindingKind {
    Terrain,
    Feature,
    Improvement,
}

/// A `WorldSoundscapeAudioScript` of a gameplay row.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ScapeBinding {
    pub kind: ScapeBindingKind,
    /// `Type` of the row, e.g. `TERRAIN_GRASS`.
    pub type_name: String,
    /// `ScriptID` of the soundscape (it may be one the file does not define: `ASSS_AMBIENCE_TUNDRA`).
    pub scape: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SoundscapeTable {
    /// Sorted by id.
    pub scapes: Vec<Soundscape>,
    /// Sorted by (kind, type name).
    pub bindings: Vec<ScapeBinding>,
}

impl SoundscapeTable {
    pub const VERSION: u16 = 1;
    pub const MIN_VERSION: u16 = 1;

    /// Deterministic: everything written sorted.
    pub fn encode(&self) -> Vec<u8> {
        let mut t = self.clone();
        t.scapes.sort_by(|a, b| a.id.cmp(&b.id));
        t.bindings.sort();
        t.bindings.dedup();
        blob::encode_v(SOUNDSCAPE_TABLE_MAGIC, Self::VERSION, &t)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let (_, t): (u16, Self) = blob::decode_range(
            SOUNDSCAPE_TABLE_MAGIC,
            Self::MIN_VERSION,
            Self::VERSION,
            bytes,
        )?;
        t.check()?;
        Ok(t)
    }

    pub fn check(&self) -> Result<(), FormatError> {
        if let Some(w) = self.scapes.windows(2).find(|w| w[0].id >= w[1].id) {
            return Err(FormatError::invalid(format!(
                "soundscapes are not sorted or repeat: {:?} then {:?}",
                w[0].id, w[1].id
            )));
        }
        if self.bindings.windows(2).any(|w| w[0] >= w[1]) {
            return Err(FormatError::invalid(
                "soundscape bindings are not sorted or repeat",
            ));
        }
        for s in &self.scapes {
            if s.min_volume > s.max_volume {
                return Err(FormatError::invalid(format!(
                    "soundscape {:?}: min volume {} is above max volume {}",
                    s.id, s.min_volume, s.max_volume
                )));
            }
            if !(s.min_distance >= 0.0 && s.max_distance >= s.min_distance) {
                return Err(FormatError::invalid(format!(
                    "soundscape {:?}: distances {}..{} are not an ascending range",
                    s.id, s.min_distance, s.max_distance
                )));
            }
            if !s.priority.is_finite() {
                return Err(FormatError::invalid(format!(
                    "soundscape {:?}: priority is not a number",
                    s.id
                )));
            }
        }
        Ok(())
    }

    pub fn scape(&self, id: &str) -> Option<&Soundscape> {
        self.scapes
            .binary_search_by(|s| s.id.as_str().cmp(id))
            .ok()
            .map(|i| &self.scapes[i])
    }

    /// The soundscape a row uses (`Terrain`, `"TERRAIN_GRASS"`).
    pub fn binding(&self, kind: ScapeBindingKind, type_name: &str) -> Option<&str> {
        self.bindings
            .iter()
            .find(|b| b.kind == kind && b.type_name == type_name)
            .map(|b| b.scape.as_str())
    }

    /// Every 2D script the soundscapes name (the pipeline puts them in the sound set).
    pub fn scripts(&self) -> BTreeSet<&str> {
        self.scapes
            .iter()
            .flat_map(|s| s.elements.iter())
            .flat_map(|e| e.scripts.iter().map(String::as_str))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scape(id: &str) -> Soundscape {
        Soundscape {
            id: id.into(),
            min_distance: 3.0,
            max_distance: 5.0,
            min_volume: 40,
            max_volume: 40,
            priority: 6.0,
            elements: vec![
                ScapeElement {
                    scripts: vec!["AS2D_AMBIENCE_FOREST_BED".into()],
                    bed: true,
                    min_gap_ms: 0,
                    max_gap_ms: 0,
                },
                ScapeElement {
                    scripts: vec![
                        "AS2D_AMBIENCE_FOREST_OWL".into(),
                        "AS2D_AMBIENCE_FOREST_BIRD1".into(),
                    ],
                    bed: false,
                    min_gap_ms: 4000,
                    max_gap_ms: 12000,
                },
            ],
        }
    }

    fn table() -> SoundscapeTable {
        SoundscapeTable {
            scapes: vec![scape("ASSS_AMBIENCE_FOREST")],
            bindings: vec![
                ScapeBinding {
                    kind: ScapeBindingKind::Terrain,
                    type_name: "TERRAIN_TUNDRA".into(),
                    scape: "ASSS_AMBIENCE_TUNDRA".into(),
                },
                ScapeBinding {
                    kind: ScapeBindingKind::Feature,
                    type_name: "FEATURE_FOREST".into(),
                    scape: "ASSS_AMBIENCE_FOREST".into(),
                },
            ],
        }
    }

    #[test]
    fn round_trips_deterministic_and_looks_up() {
        let bytes = table().encode();
        assert_eq!(&bytes[..4], b"C5SC");
        let back = SoundscapeTable::decode(&bytes).unwrap();
        assert_eq!(back.encode(), bytes);
        assert_eq!(back.scape("ASSS_AMBIENCE_FOREST").unwrap().priority, 6.0);
        assert_eq!(
            back.binding(ScapeBindingKind::Feature, "FEATURE_FOREST"),
            Some("ASSS_AMBIENCE_FOREST")
        );
        assert_eq!(
            back.binding(ScapeBindingKind::Terrain, "FEATURE_FOREST"),
            None
        );
        // A binding may name a soundscape the file does not define (the tundra rows).
        assert!(back.scape("ASSS_AMBIENCE_TUNDRA").is_none());
        assert_eq!(back.scripts().len(), 3);
    }

    #[test]
    fn the_sound_table_is_not_a_soundscape_table() {
        let audio = crate::AudioTable::default().encode();
        assert!(SoundscapeTable::decode(&audio).is_err());
    }

    #[test]
    fn unsorted_or_broken_tables_are_refused() {
        let mut t = table();
        t.scapes.push(scape("ASSS_AAA"));
        let bytes = blob::encode_v(SOUNDSCAPE_TABLE_MAGIC, SoundscapeTable::VERSION, &t);
        assert!(SoundscapeTable::decode(&bytes).is_err());
        let mut t = table();
        t.scapes[0].min_distance = 9.0;
        let bytes = blob::encode_v(SOUNDSCAPE_TABLE_MAGIC, SoundscapeTable::VERSION, &t);
        assert!(
            SoundscapeTable::decode(&bytes)
                .unwrap_err()
                .to_string()
                .contains("distances")
        );
    }
}
