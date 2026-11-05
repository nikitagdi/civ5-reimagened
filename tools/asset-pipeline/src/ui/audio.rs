//! 2D sound scripts and the sound files UI needs.

use std::collections::{BTreeMap, BTreeSet};

use asset_format::{
    AudioTable, LinkTable, ScapeBinding, ScapeBindingKind, ScapeElement, SoundDef, SoundKind,
    SoundLink, SoundLoad, SoundScript, Soundscape, SoundscapeTable, ui_sound_file_id,
};
use roxmltree::{Document, Node};

use super::vfs::{UiVfs, ext_of};

/// Prefixes of the scripts the interface plays itself (class `core`). `AS2D_SELECT_` / `AS2D_BIRTH_` are the unit
/// selection sounds (`UnitGameplay2DScripts`, played by the exe) and `AS2D_AMBIENCE_CITY_` the city screen's ambience.
pub const CORE_PREFIXES: &[&str] = &[
    "AS2D_INTERFACE_",
    "AS2D_IF_",
    "AS2D_EVENT_",
    "AS2D_BUILD_",
    "AS2D_SELECT_",
    "AS2D_BIRTH_",
    "AS2D_AMBIENCE_CITY_",
    // The endgame screen plays `Victories.Audio` (a script built from the data, so no literal in the Lua names it); the
    // few lines are in the set without `--ui-voice`, or the victory screen was silent and logged an unknown script.
    "AS2D_VICTORY_SPEECH_",
];
/// Scripts the exe plays itself that no prefix covers: the "look here" effect's own sound (`AS2D_WORLD_RAY_OF_GOD`, the
/// VisEffect of `ART_DEF_VEFFECT_LOOK_AT`; SIMP-exe-972).
pub const EXTRA_SCRIPTS: &[&str] = &["AS2D_WORLD_RAY_OF_GOD"];
/// Prefixes of the spoken lines of the interface (class `voice`, only with `--ui-voice`).
pub const VOICE_PREFIXES: &[&str] = &[
    "AS2D_TECH_",
    "AS2D_HEADING_TECH_",
    "AS2D_ERA_SPEECH_",
    "AS2D_WONDER_SPEECH_",
    "AS2D_DOM_SPEECH_",
    "AS2D_MINOR_CIV_",
];
/// Prefixes of the music (class `music`, only with `--ui-music`; re-b15 ITEM 3): the civilizations' playlists
/// (`SONG_<TAG>_<PEACE|WAR>_<n>`) and the leaders' themes (`AS2D_LEADER_MUSIC_<LEADER>_<PEACE|WAR>`). About 1.5 GB of
/// files, streamed by the client while they play (never fetched ahead). SIMPLIFIED(SIMP-exe-1196): only the
/// `Script2DFile` XML is read; a mod's music added as `Audio_2DSounds` database rows (Lekmod's custom leaders, e.g.
/// `AS2D_LEADER_MUSIC_DJSH_CORVINUS_*`) is not in the set, and such a leader's theme is silent (then the playlist).
pub const MUSIC_PREFIXES: &[&str] = &["SONG_", "AS2D_LEADER_MUSIC_"];
/// The main menu loop (the `_51` surround variant is not used).
pub const MUSIC_SCRIPTS: &[&str] = &["AS2D_MUSIC_MAIN_MENU"];
const PLACEHOLDER: &str = "AS2D_PLACEHOLDER_BLANK";
const AUDIO_EXTS: &[&str] = &["wav", "mp3", "ogg"];

#[derive(Debug, Default)]
pub struct ParsedAudio {
    pub scripts: BTreeMap<String, SoundScript>,
    /// `SoundID` to (filename, load, one variation).
    pub defs: BTreeMap<String, (String, SoundLoad, bool)>,
    pub links: Vec<SoundLink>,
    /// `ScriptSoundscape` by `ScriptID` (`AudioSoundscapeScripts.xml`).
    pub scapes: BTreeMap<String, Soundscape>,
    /// `WorldSoundscapeAudioScript` of the terrain, feature and improvement rows: (kind, `Type`) to soundscape. A later
    /// file (an expansion) replaces an earlier one.
    pub bindings: BTreeMap<(ScapeBindingKind, String), String>,
    pub errors: Vec<String>,
}

fn child<'a>(n: Node<'a, 'a>, tag: &str) -> Option<String> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == tag)
        .map(|c| c.text().unwrap_or("").trim().to_owned())
}

fn num(n: Node<'_, '_>, tag: &str) -> Option<u32> {
    child(n, tag)?
        .parse::<f64>()
        .ok()
        .map(|v| v.max(0.0) as u32)
}

fn flag(n: Node<'_, '_>, tag: &str) -> bool {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == tag)
        .is_some_and(|c| {
            let t = c.text().unwrap_or("").trim().to_ascii_lowercase();
            t.is_empty() || t == "true" || t == "1"
        })
}

/// One `Script2DFile`.
pub fn parse_scripts(src: &str, out: &mut ParsedAudio) -> Result<(), String> {
    let doc = Document::parse(src.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())?;
    for s in doc
        .descendants()
        .filter(|n| n.tag_name().name() == "Script2DSound")
    {
        let (Some(name), Some(sound)) = (child(s, "ScriptID"), child(s, "SoundID")) else {
            continue;
        };
        let kind = SoundKind::from_xml(&child(s, "SoundType").unwrap_or_default());
        let mut sc = SoundScript::new(&name, &sound, kind);
        sc.min_volume = num(s, "iMinVolume").unwrap_or(100).min(u32::from(u16::MAX)) as u16;
        sc.max_volume = num(s, "iMaxVolume")
            .unwrap_or(u32::from(sc.min_volume))
            .min(u32::from(u16::MAX)) as u16;
        if sc.max_volume < sc.min_volume {
            std::mem::swap(&mut sc.min_volume, &mut sc.max_volume);
        }
        sc.looping = flag(s, "bLooping");
        sc.dont_trigger_duplicates = flag(s, "bDontTriggerDuplicates");
        sc.max_concurrent = num(s, "iDontPlayMoreThan").unwrap_or(0).min(65535) as u16;
        sc.min_delay_ms = num(s, "iMinTimeDelay").unwrap_or(0);
        sc.max_delay_ms = num(s, "iMaxTimeDelay").unwrap_or(sc.min_delay_ms);
        sc.min_gap_ms = num(s, "iMinTimeMustNotPlayAgain").unwrap_or(0);
        sc.max_gap_ms = num(s, "iMaxTimeMustNotPlayAgain").unwrap_or(sc.min_gap_ms);
        sc.chance_percent = num(s, "iPercentChanceOfPlaying").unwrap_or(100).min(100) as u8;
        sc.taper_soundtrack_permille = child(s, "fTaperSoundtrackVolume")
            .and_then(|v| v.parse::<f64>().ok())
            .map(|v| (v * 1000.0).round().clamp(0.0, 65535.0) as u16);
        sc.is_music = flag(s, "bIsMusic");
        out.scripts.insert(name, sc);
    }
    Ok(())
}

/// One `AudioDefinesFile`.
pub fn parse_defines(src: &str, out: &mut ParsedAudio) -> Result<(), String> {
    let doc = Document::parse(src.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())?;
    for s in doc
        .descendants()
        .filter(|n| n.tag_name().name() == "SoundData")
    {
        let (Some(id), Some(file)) = (child(s, "SoundID"), child(s, "Filename")) else {
            continue;
        };
        out.defs.insert(
            id,
            (
                file,
                SoundLoad::from_xml(&child(s, "LoadType").unwrap_or_default()),
                flag(s, "bOnlyLoadOneVariationEachTime"),
            ),
        );
    }
    Ok(())
}

/// `NotificationSounds`-shaped files: a list of elements with `ID` or `EventLookup` and `AudioScript`.
pub fn parse_links(src: &str, table: LinkTable, out: &mut ParsedAudio) -> Result<(), String> {
    let doc = Document::parse(src.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())?;
    for (i, e) in doc
        .root_element()
        .children()
        .filter(Node::is_element)
        .enumerate()
    {
        let key = child(e, "ID").or_else(|| child(e, "EventLookup"));
        if let (Some(key), Some(script)) = (key, child(e, "AudioScript")) {
            out.links.push(SoundLink {
                table,
                index: i as u32,
                key,
                script,
            });
        }
    }
    Ok(())
}

/// One `ScriptSoundScapesFile`: the world ambience loops (SIMP-exe-1025).
pub fn parse_soundscapes(src: &str, out: &mut ParsedAudio) -> Result<(), String> {
    let doc = Document::parse(src.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())?;
    for s in doc
        .descendants()
        .filter(|n| n.tag_name().name() == "ScriptSoundscape")
    {
        let Some(id) = child(s, "ScriptID") else {
            continue;
        };
        let real = |tag: &str, default: f32| {
            child(s, tag)
                .and_then(|v| v.parse::<f32>().ok())
                .filter(|v| v.is_finite())
                .unwrap_or(default)
        };
        let min_distance = real("fMinGridDistance", 0.0).max(0.0);
        let max_distance = real("fMaxGridDistance", min_distance).max(min_distance);
        let mut min_volume = num(s, "iMinVolume").unwrap_or(100).min(u32::from(u16::MAX)) as u16;
        let mut max_volume = num(s, "iMaxVolume")
            .unwrap_or(u32::from(min_volume))
            .min(u32::from(u16::MAX)) as u16;
        if max_volume < min_volume {
            std::mem::swap(&mut min_volume, &mut max_volume);
        }
        let mut elements = Vec::new();
        for e in s
            .children()
            .filter(|c| c.is_element() && c.tag_name().name() == "SoundscapeElement")
        {
            // Only 2D scripts are in the set (the file has no other kind).
            if child(e, "ScriptType").is_some_and(|t| !t.eq_ignore_ascii_case("2D")) {
                continue;
            }
            let scripts: Vec<String> = e
                .children()
                .filter(|c| c.is_element() && c.tag_name().name() == "ScriptID")
                .filter_map(|c| c.text())
                .map(|t| t.trim().to_owned())
                .filter(|t| !t.is_empty())
                .collect();
            if scripts.is_empty() {
                continue;
            }
            let min_gap_ms = num(e, "iMinTimeMustNotPlayAgain").unwrap_or(0);
            elements.push(ScapeElement {
                scripts,
                bed: flag(e, "bIsSoundBed"),
                min_gap_ms,
                max_gap_ms: num(e, "iMaxTimeMustNotPlayAgain")
                    .unwrap_or(min_gap_ms)
                    .max(min_gap_ms),
            });
        }
        out.scapes.insert(
            id.clone(),
            Soundscape {
                id,
                min_distance,
                max_distance,
                min_volume,
                max_volume,
                priority: real("fPriority", 1.0),
                elements,
            },
        );
    }
    Ok(())
}

/// A gameplay table file (`CIV5Terrains.xml`, `CIV5Features.xml`, `CIV5Improvements.xml`): the rows that name a
/// `WorldSoundscapeAudioScript`.
pub fn parse_scape_bindings(src: &str, out: &mut ParsedAudio) -> Result<(), String> {
    let doc = Document::parse(src.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())?;
    for row in doc.descendants().filter(Node::is_element) {
        let (Some(ty), Some(scape)) =
            (child(row, "Type"), child(row, "WorldSoundscapeAudioScript"))
        else {
            continue;
        };
        let kind = if ty.starts_with("TERRAIN_") {
            ScapeBindingKind::Terrain
        } else if ty.starts_with("FEATURE_") {
            ScapeBindingKind::Feature
        } else if ty.starts_with("IMPROVEMENT_") {
            ScapeBindingKind::Improvement
        } else {
            continue;
        };
        if !scape.is_empty() {
            out.bindings.insert((kind, ty), scape);
        }
    }
    Ok(())
}

/// The soundscape table of what was parsed.
pub fn soundscape_table(p: &ParsedAudio) -> SoundscapeTable {
    SoundscapeTable {
        scapes: p.scapes.values().cloned().collect(),
        bindings: p
            .bindings
            .iter()
            .map(|((kind, type_name), scape)| ScapeBinding {
                kind: *kind,
                type_name: type_name.clone(),
                scape: scape.clone(),
            })
            .collect(),
    }
}

/// Audio file names (lower case) the VFS holds for a `Filename`: `f.wav`, or the numbered
/// variations `f-000.wav`, `f-001.wav`...
pub fn resolve_files(names: &BTreeSet<String>, filename: &str) -> Vec<String> {
    let f = filename.to_ascii_lowercase();
    let mut out = Vec::new();
    for ext in AUDIO_EXTS {
        let plain = format!("{f}.{ext}");
        if names.contains(&plain) {
            out.push(plain);
        }
    }
    if !out.is_empty() {
        return out;
    }
    let prefix = format!("{f}-");
    for n in names.range(prefix.clone()..) {
        let Some(rest) = n.strip_prefix(&prefix) else {
            break;
        };
        let Some((num, ext)) = rest.rsplit_once('.') else {
            continue;
        };
        if num.len() == 3 && num.bytes().all(|b| b.is_ascii_digit()) && AUDIO_EXTS.contains(&ext) {
            out.push(n.clone());
        }
    }
    out
}

/// Which scripts get their files in the set.
pub fn wanted(p: &ParsedAudio, literal_names: &BTreeSet<String>, voice: bool) -> BTreeSet<String> {
    let mut w = BTreeSet::new();
    let core = |n: &str| {
        CORE_PREFIXES.iter().any(|p| n.starts_with(p))
            || n == PLACEHOLDER
            || EXTRA_SCRIPTS.contains(&n)
    };
    let voicey = |n: &str| VOICE_PREFIXES.iter().any(|p| n.starts_with(p));
    for n in p.scripts.keys() {
        if core(n) || (voice && voicey(n)) {
            w.insert(n.clone());
        }
    }
    for n in literal_names {
        if p.scripts.contains_key(n) {
            w.insert(n.clone());
        }
    }
    for l in &p.links {
        let take = match l.table {
            LinkTable::Notification | LinkTable::ButtonPopup | LinkTable::Event => true,
            LinkTable::Advisor | LinkTable::MinorCiv => voice,
        };
        if take && p.scripts.contains_key(&l.script) {
            w.insert(l.script.clone());
        }
    }
    w
}

/// The music scripts the files define ([`MUSIC_PREFIXES`], [`MUSIC_SCRIPTS`]).
pub fn wanted_music(p: &ParsedAudio) -> BTreeSet<String> {
    p.scripts
        .keys()
        .filter(|n| {
            MUSIC_PREFIXES.iter().any(|m| n.starts_with(m)) || MUSIC_SCRIPTS.contains(&n.as_str())
        })
        .cloned()
        .collect()
}

/// A script or sound the set could not complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gap {
    pub script: String,
    pub reason: String,
}

#[derive(Debug, Default)]
pub struct AudioBuild {
    pub table: AudioTable,
    /// Audio files to store: lower-case bare name.
    pub files: BTreeSet<String>,
    pub gaps: Vec<Gap>,
    pub scripts_total: usize,
    pub scripts_kept: usize,
}

/// Builds the table for the wanted scripts. Scripts outside the wanted set are not in the table.
pub fn build(
    p: &ParsedAudio,
    audio_names: &BTreeSet<String>,
    wanted: &BTreeSet<String>,
) -> AudioBuild {
    let mut b = AudioBuild {
        scripts_total: p.scripts.len(),
        ..AudioBuild::default()
    };
    let mut sounds: BTreeMap<String, SoundDef> = BTreeMap::new();
    for name in wanted {
        let Some(sc) = p.scripts.get(name) else {
            continue;
        };
        b.scripts_kept += 1;
        b.table.scripts.push(sc.clone());
        if name == PLACEHOLDER || sounds.contains_key(&sc.sound) {
            continue;
        }
        match p.defs.get(&sc.sound) {
            None => b.gaps.push(Gap {
                script: name.clone(),
                reason: format!("sound {} has no definition", sc.sound),
            }),
            Some((file, load, one)) => {
                let found = resolve_files(audio_names, file);
                if found.is_empty() {
                    b.gaps.push(Gap {
                        script: name.clone(),
                        reason: format!("no file for {file} ({})", sc.sound),
                    });
                }
                b.files.extend(found.iter().cloned());
                sounds.insert(
                    sc.sound.clone(),
                    SoundDef {
                        id: sc.sound.clone(),
                        filename: file.clone(),
                        load: *load,
                        one_variation: *one,
                        files: found.iter().map(|f| ui_sound_file_id(f)).collect(),
                    },
                );
            }
        }
    }
    b.table.sounds = sounds.into_values().collect();
    b.table.links = p
        .links
        .iter()
        .filter(|l| wanted.contains(&l.script))
        .cloned()
        .collect();
    b
}

/// Adds the definitions and files of the sound ids the table does not have yet (the sounds of the 3D scripts, which are
/// in the unit sound table); a sound with no definition or no file is a gap of `owner`. Returns how many it added.
pub fn add_sounds<'a>(
    b: &mut AudioBuild,
    p: &ParsedAudio,
    audio_names: &BTreeSet<String>,
    sounds: impl IntoIterator<Item = &'a str>,
    owner: &str,
) -> usize {
    let mut have: BTreeSet<String> = b.table.sounds.iter().map(|s| s.id.clone()).collect();
    let mut added = 0;
    for id in sounds {
        if !have.insert(id.to_owned()) {
            continue;
        }
        let Some((file, load, one)) = p.defs.get(id) else {
            b.gaps.push(Gap {
                script: owner.to_owned(),
                reason: format!("sound {id} has no definition"),
            });
            continue;
        };
        let found = resolve_files(audio_names, file);
        if found.is_empty() {
            b.gaps.push(Gap {
                script: owner.to_owned(),
                reason: format!("no file for {file} ({id})"),
            });
        }
        b.files.extend(found.iter().cloned());
        b.table.sounds.push(SoundDef {
            id: id.to_owned(),
            filename: file.clone(),
            load: *load,
            one_variation: *one,
            files: found.iter().map(|f| ui_sound_file_id(f)).collect(),
        });
        added += 1;
    }
    b.table.sounds.sort_by(|a, b| a.id.cmp(&b.id));
    added
}

/// Names of the audio files in the VFS (lower case).
pub fn audio_names(vfs: &UiVfs) -> BTreeSet<String> {
    vfs.iter_names()
        .filter(|(n, _)| AUDIO_EXTS.contains(&ext_of(n)))
        .map(|(n, _)| n.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPTS: &str = r#"<Script2DFile><Script2DSounds>
      <Script2DSound><ScriptID>AS2D_INTERFACE_POLICY</ScriptID><SoundID>SND_P</SoundID><SoundType>GAME_SFX</SoundType>
        <iMinVolume>45</iMinVolume><iMaxVolume>45</iMaxVolume><bDontTriggerDuplicates>True</bDontTriggerDuplicates><iDontPlayMoreThan>1</iDontPlayMoreThan></Script2DSound>
      <Script2DSound><ScriptID>AS2D_IF_SELECT</ScriptID><SoundID>SND_S</SoundID><SoundType>GAME_SFX</SoundType></Script2DSound>
      <Script2DSound><ScriptID>AS2D_TECH_X</ScriptID><SoundID>SND_T</SoundID><SoundType>GAME_SPEECH</SoundType><fTaperSoundtrackVolume>0.25</fTaperSoundtrackVolume><bLooping/></Script2DSound>
      <Script2DSound><ScriptID>SONG_X</ScriptID><SoundID>SND_M</SoundID><SoundType>GAME_MUSIC</SoundType><iPercentChanceOfPlaying>50</iPercentChanceOfPlaying></Script2DSound>
      <Script2DSound><ScriptID>AS2D_SOUND_TRAIT_LIT</ScriptID><SoundID>SND_L</SoundID><SoundType>GAME_SFX</SoundType></Script2DSound>
    </Script2DSounds></Script2DFile>"#;
    const DEFS: &str = r#"<AudioDefinesFile><SoundDatas>
      <SoundData><SoundID>SND_P</SoundID><Filename>NotificationPolicyRev2</Filename><LoadType>DYNAMIC_RES</LoadType><bOnlyLoadOneVariationEachTime/></SoundData>
      <SoundData><SoundID>SND_S</SoundID><Filename>Click</Filename><LoadType>RESIDENT</LoadType></SoundData>
      <SoundData><SoundID>SND_T</SoundID><Filename>Tech</Filename><LoadType>STREAMED</LoadType></SoundData>
    </SoundDatas></AudioDefinesFile>"#;

    fn parsed() -> ParsedAudio {
        let mut p = ParsedAudio::default();
        parse_scripts(SCRIPTS, &mut p).unwrap();
        parse_defines(DEFS, &mut p).unwrap();
        p
    }

    #[test]
    fn script_fields() {
        let p = parsed();
        let s = &p.scripts["AS2D_INTERFACE_POLICY"];
        assert_eq!((s.min_volume, s.max_volume, s.max_concurrent), (45, 45, 1));
        assert!(s.dont_trigger_duplicates);
        let t = &p.scripts["AS2D_TECH_X"];
        assert_eq!(t.taper_soundtrack_permille, Some(250));
        assert!(t.looping);
        assert_eq!(p.scripts["SONG_X"].chance_percent, 50);
        assert_eq!(p.scripts["AS2D_IF_SELECT"].chance_percent, 100);
        assert_eq!(p.defs["SND_P"].1, SoundLoad::DynamicRes);
        assert!(p.defs["SND_P"].2);
    }

    #[test]
    fn files_plain_and_numbered_variations() {
        let names: BTreeSet<String> = [
            "click.wav",
            "tech-000.wav",
            "tech-001.wav",
            "tech-002.mp3",
            "tech-final.wav",
            "techno.wav",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        assert_eq!(resolve_files(&names, "Click"), ["click.wav"]);
        assert_eq!(
            resolve_files(&names, "Tech"),
            ["tech-000.wav", "tech-001.wav", "tech-002.mp3"]
        );
        assert!(resolve_files(&names, "Nope").is_empty());
    }

    #[test]
    fn classes_literals_and_links() {
        let mut p = parsed();
        parse_links(
            "<NotificationSounds><NotificationSound><ID>N_X</ID><AudioScript>SONG_X</AudioScript></NotificationSound></NotificationSounds>",
            LinkTable::Notification,
            &mut p,
        )
        .unwrap();
        let lit: BTreeSet<String> =
            ["AS2D_SOUND_TRAIT_LIT".to_owned(), "AS2D_NOPE".to_owned()].into();
        let w = wanted(&p, &lit, false);
        assert!(w.contains("AS2D_IF_SELECT") && w.contains("AS2D_INTERFACE_POLICY"));
        assert!(w.contains("AS2D_SOUND_TRAIT_LIT") && w.contains("SONG_X"));
        assert!(!w.contains("AS2D_TECH_X"));
        assert!(wanted(&p, &lit, true).contains("AS2D_TECH_X"));
    }

    #[test]
    fn build_reports_gaps_and_collects_files() {
        let p = parsed();
        let names: BTreeSet<String> = ["click.wav", "notificationpolicyrev2.wav"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let lit = BTreeSet::from(["AS2D_SOUND_TRAIT_LIT".to_owned()]);
        let b = build(&p, &names, &wanted(&p, &lit, false));
        assert_eq!(b.files.len(), 2);
        assert_eq!(b.scripts_kept, 3);
        assert_eq!(b.gaps.len(), 1);
        assert!(b.gaps[0].reason.contains("SND_L"));
        let enc = b.table.encode();
        let back = AudioTable::decode(&enc).unwrap();
        assert_eq!(back.sound("SND_S").unwrap().files, ["ui:au:click.wav"]);
    }

    const SCAPES: &str = r#"<ScriptSoundScapesFile><ScriptSoundscapes>
      <ScriptSoundscape><ScriptID>ASSS_AMBIENCE_FOREST</ScriptID><fMinGridDistance>3</fMinGridDistance><fMaxGridDistance>5</fMaxGridDistance>
        <iMinVolume>40</iMinVolume><iMaxVolume>40</iMaxVolume><fPriority>6</fPriority>
        <SoundscapeElement><ScriptType>2D</ScriptType><ScriptID>AS2D_AMBIENCE_FOREST_BED</ScriptID><bIsSoundBed>1</bIsSoundBed></SoundscapeElement>
        <SoundscapeElement><iMinTimeMustNotPlayAgain>4000</iMinTimeMustNotPlayAgain><iMaxTimeMustNotPlayAgain>12000</iMaxTimeMustNotPlayAgain>
          <ScriptType>2D</ScriptType><ScriptID>AS2D_AMBIENCE_FOREST_OWL</ScriptID><ScriptID>AS2D_AMBIENCE_FOREST_BIRD1</ScriptID></SoundscapeElement>
        <SoundscapeElement><ScriptType>3D</ScriptType><ScriptID>AS3D_NOPE</ScriptID></SoundscapeElement>
      </ScriptSoundscape>
      <ScriptSoundscape><ScriptID>ASSS_AMBIENCE_RIVER</ScriptID><fMinGridDistance>3</fMinGridDistance><fMaxGridDistance>5</fMaxGridDistance>
        <iMinVolume>40</iMinVolume><iMaxVolume>40</iMaxVolume>
        <SoundscapeElement><ScriptType>2D</ScriptType><ScriptID>AS2D_AMBIENCE_RIVER_BED</ScriptID><bIsSoundBed>1</bIsSoundBed></SoundscapeElement>
      </ScriptSoundscape>
      <CitySoundscape><ScriptID>ASSS_AMBIENCE_ANCIENT_SM</ScriptID></CitySoundscape>
    </ScriptSoundscapes></ScriptSoundScapesFile>"#;
    const TERRAINS: &str = r#"<GameData><Table name="Terrains"><Column name="WorldSoundscapeAudioScript" type="text"/></Table>
      <Terrains><Row><ID>0</ID><Type>TERRAIN_GRASS</Type><WorldSoundscapeAudioScript>ASSS_AMBIENCE_GRASSLANDS</WorldSoundscapeAudioScript></Row>
        <Row><ID>1</ID><Type>TERRAIN_TUNDRA</Type><WorldSoundscapeAudioScript>ASSS_AMBIENCE_TUNDRA</WorldSoundscapeAudioScript></Row>
        <Row><ID>2</ID><Type>TERRAIN_MOUNTAIN</Type></Row>
        <Row><ID>3</ID><Type>CIVILIZATION_X</Type><WorldSoundscapeAudioScript>ASSS_AMBIENCE_X</WorldSoundscapeAudioScript></Row></Terrains></GameData>"#;

    #[test]
    fn soundscapes_and_bindings_become_a_table() {
        let mut p = ParsedAudio::default();
        parse_soundscapes(SCAPES, &mut p).unwrap();
        parse_scape_bindings(TERRAINS, &mut p).unwrap();
        // A later file replaces the row of an earlier one (an expansion).
        parse_scape_bindings(
            "<GameData><Terrains><Row><Type>TERRAIN_GRASS</Type><WorldSoundscapeAudioScript>ASSS_AMBIENCE_FOREST</WorldSoundscapeAudioScript></Row></Terrains></GameData>",
            &mut p,
        )
        .unwrap();
        let t = soundscape_table(&p);
        let forest = t.scape("ASSS_AMBIENCE_FOREST").unwrap();
        assert_eq!((forest.min_distance, forest.max_distance), (3.0, 5.0));
        assert_eq!(
            (forest.min_volume, forest.max_volume, forest.priority),
            (40, 40, 6.0)
        );
        // The 3D element and the city soundscape are not in it; the bed and the two-script element are.
        assert_eq!(forest.elements.len(), 2);
        assert!(
            forest.elements[0].bed && forest.elements[0].scripts == ["AS2D_AMBIENCE_FOREST_BED"]
        );
        assert_eq!(
            forest.elements[1].scripts,
            ["AS2D_AMBIENCE_FOREST_OWL", "AS2D_AMBIENCE_FOREST_BIRD1"]
        );
        assert_eq!(
            (forest.elements[1].min_gap_ms, forest.elements[1].max_gap_ms),
            (4000, 12000)
        );
        assert_eq!(t.scape("ASSS_AMBIENCE_RIVER").unwrap().priority, 1.0);
        assert!(t.scape("ASSS_AMBIENCE_ANCIENT_SM").is_none());
        assert_eq!(
            t.binding(ScapeBindingKind::Terrain, "TERRAIN_GRASS"),
            Some("ASSS_AMBIENCE_FOREST")
        );
        // The dangling tundra name stays a binding (the client decides it is silent); rows without the column and
        // rows that are no terrain, feature or improvement are not.
        assert_eq!(
            t.binding(ScapeBindingKind::Terrain, "TERRAIN_TUNDRA"),
            Some("ASSS_AMBIENCE_TUNDRA")
        );
        assert_eq!(t.bindings.len(), 2);
        assert_eq!(SoundscapeTable::decode(&t.encode()).unwrap(), t);
    }

    #[test]
    fn the_scripts_of_the_soundscapes_are_in_the_set() {
        let mut p = ParsedAudio::default();
        parse_soundscapes(SCAPES, &mut p).unwrap();
        for n in ["AS2D_AMBIENCE_FOREST_BED", "AS2D_AMBIENCE_FOREST_OWL"] {
            p.scripts.insert(
                n.to_owned(),
                SoundScript::new(n, "SND", SoundKind::Ambience),
            );
        }
        // No prefix names them: the soundscape table does.
        assert!(wanted(&p, &BTreeSet::new(), false).is_empty());
        let names: BTreeSet<String> = soundscape_table(&p)
            .scripts()
            .into_iter()
            .map(str::to_owned)
            .collect();
        let w = wanted(&p, &names, false);
        assert!(w.contains("AS2D_AMBIENCE_FOREST_BED") && w.contains("AS2D_AMBIENCE_FOREST_OWL"));
        assert_eq!(w.len(), 2, "scripts the file does not have are not wanted");
    }

    #[test]
    fn unit_selection_and_city_ambience_scripts_are_in_the_set() {
        let mut p = ParsedAudio::default();
        for n in [
            "AS2D_SELECT_ARCHER",
            "AS2D_BIRTH_ARCHER",
            "AS2D_AMBIENCE_CITY_ERA_ANCIENT_SMALL",
            "AS2D_AMBIENCE_DESERT_BED",
        ] {
            p.scripts
                .insert(n.to_owned(), SoundScript::new(n, "SND", SoundKind::Sfx));
        }
        let w = wanted(&p, &BTreeSet::new(), false);
        assert!(w.contains("AS2D_SELECT_ARCHER") && w.contains("AS2D_BIRTH_ARCHER"));
        assert!(w.contains("AS2D_AMBIENCE_CITY_ERA_ANCIENT_SMALL"));
        assert!(!w.contains("AS2D_AMBIENCE_DESERT_BED"));
    }

    /// SIMPLIFIED(SIMP-exe-1001): the promotion sound is in the set (the unit panel and the notification play it);
    /// the goody hut scripts `GoodyHuts.Sound` names do not exist in the game data, so there is nothing to include.
    #[test]
    fn the_promotion_sound_is_in_the_set_and_the_goody_hut_sounds_do_not_exist() {
        let mut p = ParsedAudio::default();
        for n in [
            "AS2D_INTERFACE_UNIT_PROMOTION",
            "AS2D_INTERFACE_ANCIENT_RUINS",
        ] {
            p.scripts
                .insert(n.to_owned(), SoundScript::new(n, "SND", SoundKind::Sfx));
        }
        // A literal in the Lua that names a script the data lacks (the goody sounds) is skipped, not an error.
        let lit: BTreeSet<String> = ["AS2D_GOODY_GOLD".to_owned()].into();
        let w = wanted(&p, &lit, false);
        assert!(w.contains("AS2D_INTERFACE_UNIT_PROMOTION"));
        assert!(w.contains("AS2D_INTERFACE_ANCIENT_RUINS"));
        assert!(!w.contains("AS2D_GOODY_GOLD"));
    }

    #[test]
    fn the_music_is_wanted_only_by_its_own_list() {
        let mut p = ParsedAudio::default();
        for n in [
            "SONG_AMERICA_PEACE_1",
            "SONG_ROME_WAR_3",
            "AS2D_LEADER_MUSIC_WASHINGTON_PEACE",
            "AS2D_MUSIC_MAIN_MENU",
            "AS2D_MUSIC_MAIN_MENU_51",
            "AS2D_LEADER_SPEECH_WASHINGTON_HELLO",
        ] {
            p.scripts
                .insert(n.to_owned(), SoundScript::new(n, "SND", SoundKind::Music));
        }
        // Without `--ui-music` none of it is in the set.
        assert!(wanted(&p, &BTreeSet::new(), true).is_empty());
        let m = wanted_music(&p);
        assert_eq!(m.len(), 4, "{m:?}");
        assert!(
            !m.contains("AS2D_MUSIC_MAIN_MENU_51")
                && !m.contains("AS2D_LEADER_SPEECH_WASHINGTON_HELLO")
        );
    }

    #[test]
    fn victory_speech_and_the_look_here_sound_are_in_the_set_without_voice() {
        let mut p = ParsedAudio::default();
        for n in [
            "AS2D_VICTORY_SPEECH_CULTURE_VICTORY",
            "AS2D_WORLD_RAY_OF_GOD",
            "AS2D_WORLD_OTHER",
            "AS2D_TECH_X",
        ] {
            p.scripts
                .insert(n.to_owned(), SoundScript::new(n, "SND", SoundKind::Sfx));
        }
        let w = wanted(&p, &BTreeSet::new(), false);
        assert!(w.contains("AS2D_VICTORY_SPEECH_CULTURE_VICTORY"));
        assert!(w.contains("AS2D_WORLD_RAY_OF_GOD"));
        assert!(!w.contains("AS2D_WORLD_OTHER") && !w.contains("AS2D_TECH_X"));
    }
}
