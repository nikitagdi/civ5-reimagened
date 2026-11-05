//! 2D sound table (`C5AU`, manifest id `ui:snd`): the scripts and sound definitions of the
//! original (`Audio2DScripts`, `AudioDefines`) and the tables that link game events to scripts
//!.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{FormatError, blob};

pub const AUDIO_TABLE_MAGIC: [u8; 4] = *b"C5AU";
pub const AUDIO_TABLE_ID: &str = "ui:snd";

/// `SoundType` of a script. Postcard writes the variant index: new kinds go at the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SoundKind {
    Sfx,
    Music,
    MusicSting,
    Speech,
    AdvisorSpeech,
    Ambience,
    Test,
    Other,
}

impl SoundKind {
    pub fn from_xml(s: &str) -> Self {
        match s.trim() {
            "GAME_SFX" => Self::Sfx,
            "GAME_MUSIC" => Self::Music,
            "GAME_MUSIC_STINGS" => Self::MusicSting,
            "GAME_SPEECH" => Self::Speech,
            "GAME_ADVISOR_SPEECH" => Self::AdvisorSpeech,
            "GAME_AMBIENCE" => Self::Ambience,
            "GAME_TEST" => Self::Test,
            _ => Self::Other,
        }
    }
}

/// `LoadType` of a sound definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SoundLoad {
    Resident,
    DynamicRes,
    Streamed,
    Other,
}

impl SoundLoad {
    pub fn from_xml(s: &str) -> Self {
        match s.trim() {
            "RESIDENT" => Self::Resident,
            "DYNAMIC_RES" => Self::DynamicRes,
            "STREAMED" => Self::Streamed,
            _ => Self::Other,
        }
    }
}

/// One `Script2DSound`. Times are milliseconds, volumes percent (0..=100 in the files).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoundScript {
    /// `ScriptID`, e.g. `AS2D_INTERFACE_POLICY`.
    pub name: String,
    /// `SoundID`, key of `sounds`.
    pub sound: String,
    pub kind: SoundKind,
    pub min_volume: u16,
    pub max_volume: u16,
    pub looping: bool,
    pub dont_trigger_duplicates: bool,
    /// `iDontPlayMoreThan`; 0 means no limit.
    pub max_concurrent: u16,
    pub min_delay_ms: u32,
    pub max_delay_ms: u32,
    /// `iMinTimeMustNotPlayAgain`.
    pub min_gap_ms: u32,
    pub max_gap_ms: u32,
    /// `iPercentChanceOfPlaying`; 100 when absent.
    pub chance_percent: u8,
    /// `fTaperSoundtrackVolume` in thousandths.
    pub taper_soundtrack_permille: Option<u16>,
    pub is_music: bool,
}

impl SoundScript {
    pub fn new(name: &str, sound: &str, kind: SoundKind) -> Self {
        Self {
            name: name.to_owned(),
            sound: sound.to_owned(),
            kind,
            min_volume: 100,
            max_volume: 100,
            looping: false,
            dont_trigger_duplicates: false,
            max_concurrent: 0,
            min_delay_ms: 0,
            max_delay_ms: 0,
            min_gap_ms: 0,
            max_gap_ms: 0,
            chance_percent: 100,
            taper_soundtrack_permille: None,
            is_music: false,
        }
    }
}

/// One `SoundData`: a sound id and the files it plays (variations: one of them).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoundDef {
    pub id: String,
    /// `Filename` as written, without extension.
    pub filename: String,
    pub load: SoundLoad,
    pub one_variation: bool,
    /// Manifest ids (`ui:au:...`) of the files found; empty when the set has none.
    pub files: Vec<String>,
}

/// Which table a link comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum LinkTable {
    /// `NotificationSounds.xml`: notification type to script.
    Notification,
    /// `ButtonPopupSounds.xml`: popup type to script; the order of the file is significant.
    ButtonPopup,
    /// `EventSoundConnections.xml`: event name to script.
    Event,
    /// `AdvisorSoundConnections.xml`.
    Advisor,
    /// `MinorCivSounds.xml`: a city-state (`MINOR_CIV_*`) to its greeting voice (the exe's `PlayMinorCivSound`).
    MinorCiv,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SoundLink {
    pub table: LinkTable,
    /// Position in the file of the table.
    pub index: u32,
    /// `ID` or `EventLookup`.
    pub key: String,
    pub script: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioTable {
    /// Sorted by name.
    pub scripts: Vec<SoundScript>,
    /// Sorted by id.
    pub sounds: Vec<SoundDef>,
    /// Sorted by (table, index).
    pub links: Vec<SoundLink>,
}

impl AudioTable {
    pub const VERSION: u16 = 1;
    pub const MIN_VERSION: u16 = 1;

    /// Deterministic: everything written sorted.
    pub fn encode(&self) -> Vec<u8> {
        let mut t = self.clone();
        t.scripts.sort_by(|a, b| a.name.cmp(&b.name));
        t.sounds.sort_by(|a, b| a.id.cmp(&b.id));
        for s in &mut t.sounds {
            s.files.sort();
            s.files.dedup();
        }
        t.links.sort_by_key(|l| (l.table, l.index));
        blob::encode_v(AUDIO_TABLE_MAGIC, Self::VERSION, &t)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let (_, t): (u16, Self) =
            blob::decode_range(AUDIO_TABLE_MAGIC, Self::MIN_VERSION, Self::VERSION, bytes)?;
        t.check()?;
        Ok(t)
    }

    pub fn check(&self) -> Result<(), FormatError> {
        if let Some(w) = self.scripts.windows(2).find(|w| w[0].name >= w[1].name) {
            return Err(FormatError::invalid(format!(
                "scripts are not sorted or repeat: {:?} then {:?}",
                w[0].name, w[1].name
            )));
        }
        if let Some(w) = self.sounds.windows(2).find(|w| w[0].id >= w[1].id) {
            return Err(FormatError::invalid(format!(
                "sounds are not sorted or repeat: {:?} then {:?}",
                w[0].id, w[1].id
            )));
        }
        if self
            .links
            .windows(2)
            .any(|w| (w[0].table, w[0].index) >= (w[1].table, w[1].index))
        {
            return Err(FormatError::invalid("links are not sorted or repeat"));
        }
        let ids: BTreeSet<&str> = self.sounds.iter().map(|s| s.id.as_str()).collect();
        for s in &self.scripts {
            if s.min_volume > s.max_volume {
                return Err(FormatError::invalid(format!(
                    "script {:?}: min volume {} is above max volume {}",
                    s.name, s.min_volume, s.max_volume
                )));
            }
            if s.chance_percent > 100 {
                return Err(FormatError::invalid(format!(
                    "script {:?}: chance {} is above 100",
                    s.name, s.chance_percent
                )));
            }
            if !s.sound.is_empty() && !ids.contains(s.sound.as_str()) {
                // A script may name a sound the set does not define; that is a gap for the
                // report, not a broken table.
            }
        }
        Ok(())
    }

    pub fn script(&self, name: &str) -> Option<&SoundScript> {
        self.scripts
            .binary_search_by(|s| s.name.as_str().cmp(name))
            .ok()
            .map(|i| &self.scripts[i])
    }

    pub fn sound(&self, id: &str) -> Option<&SoundDef> {
        self.sounds
            .binary_search_by(|s| s.id.as_str().cmp(id))
            .ok()
            .map(|i| &self.sounds[i])
    }

    /// Script of a link by key, e.g. `(Notification, "NOTIFICATION_POLICY")`.
    pub fn link(&self, table: LinkTable, key: &str) -> Option<&str> {
        self.links
            .iter()
            .find(|l| l.table == table && l.key == key)
            .map(|l| l.script.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> AudioTable {
        let mut policy = SoundScript::new(
            "AS2D_INTERFACE_POLICY",
            "SND_INTERFACE_POLICY",
            SoundKind::Sfx,
        );
        policy.min_volume = 45;
        policy.max_volume = 45;
        policy.dont_trigger_duplicates = true;
        policy.max_concurrent = 1;
        AudioTable {
            scripts: vec![
                policy,
                SoundScript::new("AS2D_IF_SELECT", "SND_INTERFACE_SELECT", SoundKind::Sfx),
            ],
            sounds: vec![SoundDef {
                id: "SND_INTERFACE_SELECT".into(),
                filename: "InterfaceButtonClick_1".into(),
                load: SoundLoad::Resident,
                one_variation: false,
                files: vec!["ui:au:interfacebuttonclick_1.wav".into()],
            }],
            links: vec![
                SoundLink {
                    table: LinkTable::ButtonPopup,
                    index: 1,
                    key: "BUTTONPOPUP_MAIN_MENU".into(),
                    script: "AS2D_PLACEHOLDER_BLANK".into(),
                },
                SoundLink {
                    table: LinkTable::Notification,
                    index: 0,
                    key: "NOTIFICATION_POLICY".into(),
                    script: "AS2D_INTERFACE_POLICY".into(),
                },
            ],
        }
    }

    #[test]
    fn round_trips_sorted_and_deterministic() {
        let bytes = table().encode();
        assert_eq!(&bytes[..4], b"C5AU");
        let back = AudioTable::decode(&bytes).unwrap();
        assert_eq!(back.scripts[0].name, "AS2D_IF_SELECT");
        assert_eq!(back.links[0].table, LinkTable::Notification);
        assert_eq!(back.encode(), bytes);
    }

    #[test]
    fn lookups() {
        let back = AudioTable::decode(&table().encode()).unwrap();
        let p = back.script("AS2D_INTERFACE_POLICY").unwrap();
        assert_eq!((p.min_volume, p.max_concurrent), (45, 1));
        assert!(back.script("AS2D_NOPE").is_none());
        assert_eq!(
            back.sound("SND_INTERFACE_SELECT").unwrap().files,
            ["ui:au:interfacebuttonclick_1.wav"]
        );
        assert_eq!(
            back.link(LinkTable::Notification, "NOTIFICATION_POLICY"),
            Some("AS2D_INTERFACE_POLICY")
        );
        assert_eq!(back.link(LinkTable::Event, "NOTIFICATION_POLICY"), None);
    }

    #[test]
    fn kinds_parse_from_the_xml_names() {
        assert_eq!(
            SoundKind::from_xml("GAME_ADVISOR_SPEECH"),
            SoundKind::AdvisorSpeech
        );
        assert_eq!(
            SoundKind::from_xml("GAME_MUSIC_STINGS"),
            SoundKind::MusicSting
        );
        assert_eq!(SoundKind::from_xml("what"), SoundKind::Other);
        assert_eq!(SoundLoad::from_xml("DYNAMIC_RES"), SoundLoad::DynamicRes);
    }

    #[test]
    fn unsorted_or_inconsistent_tables_are_refused() {
        let mut t = table();
        t.scripts.reverse();
        let bytes = blob::encode_v(AUDIO_TABLE_MAGIC, AudioTable::VERSION, &t);
        assert!(AudioTable::decode(&bytes).is_err());
        let mut t = table();
        t.scripts.sort_by(|a, b| a.name.cmp(&b.name));
        t.links.sort_by_key(|l| (l.table, l.index));
        t.scripts[0].min_volume = 90;
        t.scripts[0].max_volume = 10;
        let bytes = blob::encode_v(AUDIO_TABLE_MAGIC, AudioTable::VERSION, &t);
        assert!(
            AudioTable::decode(&bytes)
                .unwrap_err()
                .to_string()
                .contains("min volume")
        );
    }
}
