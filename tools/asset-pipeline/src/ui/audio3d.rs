//! The unit sounds (re-b15 ITEM 4): the 3D sound scripts (`Script3DSound`, `Audio3DScripts.xml` and the per-unit
//! files), the sound rows of the models' timed triggers (`.ftsxml`, `FTimedTriggerSound`), the weapon and material tags
//! of the unit members (`Civ5ArtDefines_UnitMembers.xml`), and the pitch ranges of the 2D scripts. Written as the
//! `ui:snd3d` table; the files of the scripts the rows can name go into the sound set with the 2D ones.

use std::collections::{BTreeMap, BTreeSet};

use asset_format::{ModelSounds, PitchRange, Script3D, Sound3DTable, SoundKind, SoundTrigger};
use roxmltree::{Document, Node};

/// The values `$(TERRAIN)` / `$(BASETERRAIN)` take (`EffectTypeTag` of the terrains and features, `AIR` for a flying
/// member), `$(WEAPON)` (`WeaponTypeTag` / `WeaponTypeSoundOverrideTag`) and `$(MATERIAL)` are not listed: a row with a
/// macro keeps every script its fixed parts match.
const MACROS: [&str; 4] = ["$(TERRAIN)", "$(BASETERRAIN)", "$(WEAPON)", "$(MATERIAL)"];

#[derive(Debug, Default)]
pub struct Parsed3D {
    pub scripts: BTreeMap<String, Script3D>,
    /// `.ftsxml` name (lower case) to its sound rows.
    pub triggers: BTreeMap<String, Vec<SoundTrigger>>,
    /// `.fxsxml` stem (lower case) to its `.ftsxml` name (lower case).
    pub model_triggers: BTreeMap<String, String>,
    /// `.fxsxml` stem (lower case) to (weapon, material) of the member that uses it (a later file replaces).
    pub member_tags: BTreeMap<String, (Option<String>, Option<String>)>,
    /// 2D script to its pitch range, for the scripts that have one.
    pub pitch_2d: BTreeMap<String, (i8, i8)>,
}

fn child<'a>(n: Node<'a, 'a>, tag: &str) -> Option<String> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == tag)
        .map(|c| c.text().unwrap_or("").trim().to_owned())
}

fn int(n: Node<'_, '_>, tag: &str) -> Option<i64> {
    child(n, tag)?.parse::<f64>().ok().map(|v| v as i64)
}

fn real(n: Node<'_, '_>, tag: &str) -> Option<f32> {
    child(n, tag)?.parse::<f32>().ok().filter(|v| v.is_finite())
}

fn flag(n: Node<'_, '_>, tag: &str) -> Option<bool> {
    let t = child(n, tag)?.to_ascii_lowercase();
    Some(t.is_empty() || t == "true" || t == "1")
}

fn semitones(v: i64) -> i8 {
    v.clamp(-48, 48) as i8
}

/// Applies the tags of one `Script3DSound` (or of the `Default3DSound` block) on `s`: absent tags keep the value.
fn apply(n: Node<'_, '_>, s: &mut Script3D) {
    if let Some(k) = child(n, "SoundType") {
        s.kind = SoundKind::from_xml(&k);
    }
    let pct = |v: i64| v.clamp(0, i64::from(u16::MAX)) as u16;
    let ms = |v: i64| v.clamp(0, i64::from(u32::MAX)) as u32;
    if let Some(v) = int(n, "iMinVolume") {
        s.min_volume = pct(v);
    }
    if let Some(v) = int(n, "iMaxVolume") {
        s.max_volume = pct(v);
    }
    if let Some(v) = int(n, "iPitchChangeDown") {
        s.pitch_down = semitones(v);
    }
    if let Some(v) = int(n, "iPitchChangeUp") {
        s.pitch_up = semitones(v);
    }
    if let Some(v) = flag(n, "bLooping") {
        s.looping = v;
    }
    if let Some(v) = flag(n, "bDontTriggerDuplicates") {
        s.dont_trigger_duplicates = v;
    }
    if let Some(v) = int(n, "iDontPlayMoreThan") {
        // (-1: unset, the engine's own cap)
        s.max_concurrent = v.clamp(0, 65535) as u16;
    }
    if let Some(v) = int(n, "iMinTimeDelay") {
        s.min_delay_ms = ms(v);
    }
    if let Some(v) = int(n, "iMaxTimeDelay") {
        s.max_delay_ms = ms(v);
    }
    if let Some(v) = int(n, "iMinTimeMustNotPlayAgain") {
        s.min_gap_ms = ms(v);
    }
    if let Some(v) = int(n, "iMaxTimeMustNotPlayAgain") {
        s.max_gap_ms = ms(v);
    }
    if let Some(v) = int(n, "iPercentChanceOfPlaying") {
        s.chance_percent = v.clamp(0, 100) as u8;
    }
    if let Some(v) = real(n, "fMinDistance") {
        s.min_distance = v.max(0.0);
    }
    if let Some(v) = real(n, "fCutoffDistance") {
        s.cutoff_distance = v.max(0.0);
    }
}

/// Puts the ranges of a script in order (the files have a few upside down) and the exe's rules for gaps: a minimum
/// without a maximum is twice the minimum (re-b15 E3).
fn settle(s: &mut Script3D) {
    if s.max_volume < s.min_volume {
        std::mem::swap(&mut s.min_volume, &mut s.max_volume);
    }
    if s.pitch_up < s.pitch_down {
        std::mem::swap(&mut s.pitch_up, &mut s.pitch_down);
    }
    if s.max_delay_ms < s.min_delay_ms {
        s.max_delay_ms = s.min_delay_ms;
    }
    if s.min_gap_ms > 0 && s.max_gap_ms == 0 {
        s.max_gap_ms = 2 * s.min_gap_ms;
    }
    if s.max_gap_ms < s.min_gap_ms {
        s.max_gap_ms = s.min_gap_ms;
    }
}

/// One `Script3DFile`. The `Default3DSound` block of the master file holds the defaults of every script; the shipped
/// defaults are [`Script3D::new`]'s, so a file read before the master gets the same values.
pub fn parse_scripts_3d(src: &str, out: &mut Parsed3D) -> Result<(), String> {
    let doc = Document::parse(src.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())?;
    let mut base = Script3D::new("", "");
    if let Some(d) = doc
        .descendants()
        .find(|n| n.tag_name().name() == "Default3DSound")
    {
        apply(d, &mut base);
    }
    for s in doc
        .descendants()
        .filter(|n| n.tag_name().name() == "Script3DSound")
    {
        let (Some(name), Some(sound)) = (child(s, "ScriptID"), child(s, "SoundID")) else {
            continue;
        };
        let mut sc = Script3D {
            name: name.clone(),
            sound,
            ..base.clone()
        };
        apply(s, &mut sc);
        settle(&mut sc);
        out.scripts.insert(name, sc);
    }
    Ok(())
}

/// The pitch ranges of a `Script2DFile` (the 2D table has no field for them).
pub fn parse_pitch_2d(src: &str, out: &mut Parsed3D) -> Result<(), String> {
    let doc = Document::parse(src.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())?;
    for s in doc
        .descendants()
        .filter(|n| n.tag_name().name() == "Script2DSound")
    {
        let Some(name) = child(s, "ScriptID") else {
            continue;
        };
        let down = int(s, "iPitchChangeDown").map_or(0, semitones);
        let up = int(s, "iPitchChangeUp").map_or(0, semitones);
        if down != 0 || up != 0 {
            out.pitch_2d.insert(name, (down.min(up), down.max(up)));
        } else {
            out.pitch_2d.remove(&name);
        }
    }
    Ok(())
}

/// The sound rows of one `.ftsxml` (`FTimedTriggerSound`, and the exe's own `cvSoundEffectTrigger`).
pub fn parse_ftsxml(src: &str) -> Result<Vec<SoundTrigger>, String> {
    let doc = Document::parse(src.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())?;
    let mut rows = Vec::new();
    for t in doc
        .descendants()
        .filter(|n| n.tag_name().name() == "trigger")
    {
        let kind = t.attribute("type").unwrap_or("");
        if kind != "FTimedTriggerSound" && kind != "cvSoundEffectTrigger" {
            continue;
        }
        let event = t.attribute("event").unwrap_or("").trim();
        let (Some(ec), Some(time)) = (
            t.attribute("ec").and_then(|v| v.trim().parse::<u32>().ok()),
            t.attribute("time")
                .and_then(|v| v.trim().parse::<f32>().ok())
                .filter(|v| v.is_finite()),
        ) else {
            continue;
        };
        if event.is_empty() {
            continue;
        }
        let tier = t
            .attribute("tier")
            .and_then(|v| v.trim().parse::<i32>().ok())
            .unwrap_or(-1)
            .clamp(-1, 2) as i8;
        rows.push(SoundTrigger {
            ec,
            time: time.max(0.0),
            tier,
            event: event.to_ascii_uppercase(),
        });
    }
    Ok(rows)
}

/// The `.ftsxml` an `.fxsxml` names (`<TimedTrigger file="..."/>`), lower case.
pub fn timed_trigger_of(fxsxml: &str) -> Option<String> {
    let doc = Document::parse(fxsxml.trim_start_matches('\u{feff}')).ok()?;
    doc.descendants()
        .find(|n| n.tag_name().name() == "TimedTrigger")
        .and_then(|n| n.attribute("file"))
        .map(|f| f.trim().to_ascii_lowercase())
        .filter(|f| !f.is_empty())
}

/// One `UnitMemberArtInfos` file: the weapon and material tags of each member's model (`Granny`).
pub fn parse_member_tags(src: &str, out: &mut Parsed3D) -> Result<(), String> {
    let doc = Document::parse(src.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())?;
    for m in doc
        .descendants()
        .filter(|n| n.tag_name().name() == "UnitMemberArtInfo")
    {
        let Some(granny) = child(m, "Granny") else {
            continue;
        };
        let stem = granny
            .to_ascii_lowercase()
            .trim_end_matches(".fxsxml")
            .to_owned();
        if stem.is_empty() {
            continue;
        }
        let weapon = m
            .descendants()
            .find(|n| n.tag_name().name() == "Weapon")
            .and_then(|w| {
                child(w, "WeaponTypeSoundOverrideTag")
                    .filter(|t| !t.is_empty())
                    .or_else(|| child(w, "WeaponTypeTag"))
            })
            .filter(|t| !t.is_empty());
        let material = child(m, "MaterialTypeSoundOverrideTag")
            .filter(|t| !t.is_empty())
            .or_else(|| child(m, "MaterialTypeTag"))
            .filter(|t| !t.is_empty());
        out.member_tags.insert(stem, (weapon, material));
    }
    Ok(())
}

/// Does `name` fit the event `pattern` (its macros stand for any run of letters, digits and underscores)?
pub fn fits(pattern: &str, name: &str) -> bool {
    let mut parts: Vec<&str> = vec![pattern];
    for m in MACROS {
        parts = parts.iter().flat_map(|p| p.split(m)).collect();
    }
    if parts.len() == 1 {
        return pattern == name;
    }
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if !name.starts_with(first) || !name.ends_with(last) || name.len() < first.len() + last.len() {
        return false;
    }
    let mut rest = &name[first.len()..name.len() - last.len()];
    let middle = &parts[1..parts.len() - 1];
    for p in middle {
        // A macro takes at least one character before the next fixed part.
        let Some(i) = rest.get(1..).and_then(|r| r.find(p)) else {
            return false;
        };
        rest = &rest[i + 1 + p.len()..];
    }
    !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// The table and the 3D scripts the rows can name (whose files go into the set).
pub fn table(p: &Parsed3D, wanted_2d: &BTreeSet<String>) -> Sound3DTable {
    let mut models = Vec::new();
    for (model, trig) in &p.model_triggers {
        let Some(rows) = p.triggers.get(trig).filter(|r| !r.is_empty()) else {
            continue;
        };
        let (weapon, material) = p.member_tags.get(model).cloned().unwrap_or_default();
        models.push(ModelSounds {
            model: model.clone(),
            weapon,
            material,
            triggers: rows.clone(),
        });
    }
    // The scripts the rows can name: a row without a macro names one; one with a macro every script its fixed parts fit
    // (found in the range of names that start with the part before the first macro).
    let events: BTreeSet<&str> = models
        .iter()
        .flat_map(|m| m.triggers.iter().map(|t| t.event.as_str()))
        .collect();
    let mut used: BTreeSet<&str> = BTreeSet::new();
    for e in events {
        let head = MACROS
            .iter()
            .filter_map(|m| e.find(m))
            .min()
            .map_or(e, |i| &e[..i]);
        if head.len() == e.len() {
            if p.scripts.contains_key(e) {
                used.insert(e);
            }
            continue;
        }
        for (name, _) in p.scripts.range(head.to_owned()..) {
            if !name.starts_with(head) {
                break;
            }
            if fits(e, name) {
                used.insert(name.as_str());
            }
        }
    }
    Sound3DTable {
        scripts: used.iter().map(|n| p.scripts[*n].clone()).collect(),
        models,
        pitch_2d: p
            .pitch_2d
            .iter()
            .filter(|(s, _)| wanted_2d.contains(*s))
            .map(|(s, (down, up))| PitchRange {
                script: s.clone(),
                down: *down,
                up: *up,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MASTER: &str = r#"<Script3DFile>
      <IncludeXmlFiles><IncludeXmlFile>Archer3DScripts.xml</IncludeXmlFile></IncludeXmlFiles>
      <Default3DSound><ScriptID>AS3D_DEFAULT_VALUES</ScriptID><SoundType>GAME_SFX</SoundType><iMinVolume>100</iMinVolume>
        <iMaxVolume>100</iMaxVolume><iDontPlayMoreThan>-1</iDontPlayMoreThan><fMinDistance>1.5</fMinDistance><fCutoffDistance>6</fCutoffDistance></Default3DSound>
      <Script3DSounds>
        <Script3DSound><ScriptID>AS3D_UNIT_DIE_VOX_SHORT</ScriptID><SoundID>SND_DIE</SoundID><iMaxVolume>55</iMaxVolume><iMinVolume>35</iMinVolume></Script3DSound>
        <Script3DSound><ScriptID>AS3D_UNIT_BATTLE</ScriptID><SoundID>SND_B</SoundID><iMinTimeMustNotPlayAgain>10000</iMinTimeMustNotPlayAgain>
          <iDontPlayMoreThan>1</iDontPlayMoreThan><fCutoffDistance>9</fCutoffDistance><bLooping>1</bLooping></Script3DSound>
      </Script3DSounds></Script3DFile>"#;
    const ARCHER: &str = r#"<Script3DFile><Script3DSounds>
        <Script3DSound><ScriptID>AS3D_UNIT_ARCHER_FOOT_DIRT</ScriptID><SoundID>SND_UNIT_FOOT_DIRT</SoundID><SoundType>GAME_SFX</SoundType>
          <iMaxVolume>75</iMaxVolume><iMinVolume>50</iMinVolume><iPitchChangeUp>3</iPitchChangeUp><iPitchChangeDown>-4</iPitchChangeDown></Script3DSound>
        <Script3DSound><ScriptID>AS3D_UNIT_ARCHER_FOOT_SAND</ScriptID><SoundID>SND_UNIT_FOOT_SAND</SoundID></Script3DSound>
        <Script3DSound><ScriptID>AS3D_UNIT_ARCHER_FORTIFY</ScriptID><SoundID>SND_F</SoundID></Script3DSound>
        <Script3DSound><ScriptID>AS3D_UNIT_ARCHER_UNUSED</ScriptID><SoundID>SND_U</SoundID></Script3DSound>
        <Script3DSound><ScriptID>AS3D_UNIT_ARCHER_IMPACT_ARROW</ScriptID><SoundID>SND_I</SoundID></Script3DSound>
      </Script3DSounds></Script3DFile>"#;
    const FTS: &str = r#"<trigger_system type="FGrannyTimedTriggerSystem"><event_tracks><event_track ec="1000"><track id="1" name="Sound" /></event_track></event_tracks>
      <trigger type="FTimedTriggerSound" id="1" time="0.5" duration="0" repeat="0" ec="1400" track="1" tier="0" event="AS3D_UNIT_ARCHER_FOOT_$(TERRAIN)" bone="" />
      <trigger type="FTimedTriggerSound" id="2" time="0.005" duration="0" repeat="0" ec="1500" track="1" tier="-1" event="AS3D_UNIT_ARCHER_FORTIFY" bone="" />
      <trigger type="cvSoundEffectTrigger" id="3" time="0.1" ec="1200" track="1" tier="-1" event="as3d_unit_archer_impact_$(WEAPON)" />
      <trigger type="FTimedTriggerEffect" id="4" time="0.1" ec="1200" track="2" event="ART_DEF_VEFFECT_X" />
      <trigger type="FTimedTriggerSound" id="5" time="nan" ec="1200" track="1" event="AS3D_UNIT_ARCHER_FORTIFY" />
    </trigger_system>"#;

    #[test]
    fn a_3d_script_takes_the_defaults_and_its_own_tags() {
        let mut p = Parsed3D::default();
        parse_scripts_3d(MASTER, &mut p).unwrap();
        parse_scripts_3d(ARCHER, &mut p).unwrap();
        let foot = &p.scripts["AS3D_UNIT_ARCHER_FOOT_DIRT"];
        assert_eq!((foot.min_volume, foot.max_volume), (50, 75));
        assert_eq!((foot.pitch_down, foot.pitch_up), (-4, 3));
        assert_eq!((foot.min_distance, foot.cutoff_distance), (1.5, 6.0));
        assert_eq!(foot.max_concurrent, 0, "-1 is unset");
        let battle = &p.scripts["AS3D_UNIT_BATTLE"];
        assert_eq!(
            (battle.min_gap_ms, battle.max_gap_ms),
            (10_000, 20_000),
            "max = 2 * min"
        );
        assert_eq!((battle.max_concurrent, battle.cutoff_distance), (1, 9.0));
        assert!(battle.looping);
        assert_eq!(p.scripts["AS3D_UNIT_DIE_VOX_SHORT"].kind, SoundKind::Sfx);
    }

    #[test]
    fn the_sound_rows_of_a_trigger_file() {
        let rows = parse_ftsxml(FTS).unwrap();
        assert_eq!(rows.len(), 3, "{rows:?}");
        assert_eq!((rows[0].ec, rows[0].tier), (1400, 0));
        assert_eq!(rows[1].tier, -1);
        assert_eq!(
            rows[2].event, "AS3D_UNIT_ARCHER_IMPACT_$(WEAPON)",
            "upper case"
        );
        assert_eq!(
            timed_trigger_of(
                r#"<Asset><Mesh file="a.gr2"/><TimedTrigger file="FX_Triggers_Archer.ftsxml"/></Asset>"#
            ),
            Some("fx_triggers_archer.ftsxml".to_owned())
        );
        assert_eq!(timed_trigger_of("<Asset/>"), None);
    }

    #[test]
    fn a_macro_matches_any_tag_and_nothing_else() {
        assert!(fits("AS3D_A_FOOT_$(TERRAIN)", "AS3D_A_FOOT_DIRT"));
        assert!(!fits("AS3D_A_FOOT_$(TERRAIN)", "AS3D_A_FOOT_"));
        assert!(!fits("AS3D_A_FOOT_$(TERRAIN)", "AS3D_A_FOOTSTEP"));
        assert!(fits(
            "AS3D_A_$(WEAPON)_HIT_$(MATERIAL)",
            "AS3D_A_ARROW_HIT_CLOTH"
        ));
        assert!(!fits(
            "AS3D_A_$(WEAPON)_HIT_$(MATERIAL)",
            "AS3D_A_ARROW_MISS_CLOTH"
        ));
        assert!(fits("AS3D_PLAIN", "AS3D_PLAIN") && !fits("AS3D_PLAIN", "AS3D_PLAINS"));
    }

    #[test]
    fn the_table_keeps_the_scripts_the_rows_can_name_and_the_member_tags() {
        let mut p = Parsed3D::default();
        parse_scripts_3d(MASTER, &mut p).unwrap();
        parse_scripts_3d(ARCHER, &mut p).unwrap();
        p.triggers.insert(
            "fx_triggers_archer.ftsxml".into(),
            parse_ftsxml(FTS).unwrap(),
        );
        p.model_triggers
            .insert("archer".into(), "fx_triggers_archer.ftsxml".into());
        p.model_triggers
            .insert("no_rows".into(), "fx_none.ftsxml".into());
        parse_member_tags(
            r#"<UnitMemberArtInfos><UnitMemberArtInfo><Type>ART_DEF_UNIT_MEMBER_ARCHER</Type><Granny>Archer.fxsxml</Granny>
              <Combat><Weapon><WeaponTypeTag>ARROW</WeaponTypeTag></Weapon><Weapon><WeaponTypeTag>FLAMING_ARROW</WeaponTypeTag></Weapon></Combat>
              <MaterialTypeTag>CLOTH</MaterialTypeTag><MaterialTypeSoundOverrideTag>FLESH</MaterialTypeSoundOverrideTag></UnitMemberArtInfo></UnitMemberArtInfos>"#,
            &mut p,
        )
        .unwrap();
        parse_pitch_2d(
            "<Script2DFile><Script2DSound><ScriptID>AS2D_P</ScriptID><iPitchChangeUp>2</iPitchChangeUp></Script2DSound>\
             <Script2DSound><ScriptID>AS2D_Q</ScriptID></Script2DSound></Script2DFile>",
            &mut p,
        )
        .unwrap();
        let t = table(&p, &["AS2D_P".to_owned()].into());
        let names: Vec<&str> = t.scripts.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "AS3D_UNIT_ARCHER_FOOT_DIRT",
                "AS3D_UNIT_ARCHER_FOOT_SAND",
                "AS3D_UNIT_ARCHER_FORTIFY",
                "AS3D_UNIT_ARCHER_IMPACT_ARROW"
            ]
        );
        assert_eq!(t.models.len(), 1, "a model without rows is left out");
        assert_eq!(
            (
                t.models[0].weapon.as_deref(),
                t.models[0].material.as_deref()
            ),
            (Some("ARROW"), Some("FLESH"))
        );
        assert_eq!(
            t.pitch_2d,
            [PitchRange {
                script: "AS2D_P".into(),
                down: 0,
                up: 2
            }]
        );
        assert_eq!(Sound3DTable::decode(&t.encode()).unwrap(), {
            let mut s = t.clone();
            s.models[0]
                .triggers
                .sort_by(|a, b| (a.ec, a.time).partial_cmp(&(b.ec, b.time)).unwrap());
            s
        });
    }
}
