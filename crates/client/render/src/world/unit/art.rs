//! Which art a unit draws: the record of the unit type in
//! the table `tb:art-b2` (`UnitRecordB2`), the variant of the culture of the owner, then the variant
//! of the era of the owner, then the landing art when the unit is embarked.
//!
//! No marker stands in for a missing model (WO-D15, SIMP-011): a unit without a record or without
//! figures has an empty list and a line in the log; the report of the pipeline is where it is an
//! error (AC-R3.1).

use asset_format::{ArtRecord, ArtTable, FigureB2, LogicalId, UnitArtB2, UnitRecordB2};

/// Which art was picked, for the tests and the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtSource {
    Base,
    /// The variant of the culture (`ArtStyleSuffix`).
    Style,
    /// The variant of the era of the owner.
    Era,
    /// The landing art of an embarked unit.
    Embarked,
}

/// The eras the tags of the era variants name, oldest first (`ERA_TAGS` of the pipeline, without
/// `_LATE`).
pub const ERA_ORDER: [&str; 8] = [
    "_ANCIENT",
    "_CLASSICAL",
    "_MEDIEVAL",
    "_RENAISSANCE",
    "_INDUSTRIAL",
    "_MODERN",
    "_POSTMODERN",
    "_FUTURE",
];

/// The era of an owner as an index into [`ERA_ORDER`] (the tag of the era in `PlayerView.era`).
pub type EraIndex = u8;

/// The position of an era tag in [`ERA_ORDER`]. `_LATE` is the late art of the civilian units of
/// the base game; it stands for the Industrial era.
// TODO(RW-14): how the engine picks an era variant is not read. The rule here is the latest variant
// whose era is not later than the owner's; the base art when there is none.
pub fn era_of_tag(tag: &str) -> Option<EraIndex> {
    if tag == "_LATE" {
        return era_of_tag("_INDUSTRIAL");
    }
    ERA_ORDER.iter().position(|t| *t == tag).map(|i| i as u8)
}

/// The era from which an embarked unit is a transport (`Defines.LAST_EMBARK_ART_ERA`, 4: Industrial).
pub const LAST_EMBARK_ART_ERA: EraIndex = 4;

/// The era of the technology that lets a team embark onto every water plot (`TECH_ASTRONOMY`,
/// `Technologies.EmbarkedAllWaterPassage`, `ERA_RENAISSANCE` in the DB).
pub const ALL_WATER_EMBARK_ERA: EraIndex = 3;

/// The art info of the boat of an embarked unit (its record is `unit/<art info>` of `tb:art-b2`):
/// the player's override (the Polynesian war canoe, the Danish longboat), then the transport from
/// [`LAST_EMBARK_ART_ERA`] on, then the galleon for a team that embarks onto every water plot, else
/// the galley. `era` is the owner's (`None`: unknown, the galley).
// re: CvDLLUtility::GameplayUnitEmbark 0x087a3924: `GetEmbarkedGraphicOverride()` when it names a
// known art info (0x087a3cd8..0x087a3d73); else `ICvTeam1::GetCurrentEra() >= GetLAST_EMBARK_ART_ERA()`
// gives ART_DEF_UNIT_TRANSPORT, `ICvTeam1::CanEmbarkAllWaterPassage()` ART_DEF_UNIT_GALLEON, the rest
// ART_DEF_UNIT_GALLEY (0x087a3d79..0x087a3e23).
// TODO(RW-14 (g)): the view carries neither the override nor CanEmbarkAllWaterPassage; the override
// is `None` from the caller and the passage is taken from the era (a team in the Renaissance before
// Astronomy shows the galleon a few turns early).
pub fn embark_art_info(era: Option<EraIndex>, override_art: Option<&str>) -> &str {
    if let Some(o) = override_art.filter(|o| !o.is_empty()) {
        return o;
    }
    match era {
        Some(e) if e >= LAST_EMBARK_ART_ERA => "ART_DEF_UNIT_TRANSPORT",
        Some(e) if e >= ALL_WATER_EMBARK_ERA => "ART_DEF_UNIT_GALLEON",
        _ => "ART_DEF_UNIT_GALLEY",
    }
}

/// The art for a unit record, its owner and its state.
pub struct ArtChoice<'a> {
    pub art: &'a UnitArtB2,
    pub source: ArtSource,
}

/// Picks the art. `style` is the `ArtStyleSuffix` of the owner's culture (`_EURO`), `era` the era of
/// the owner (`None`: unknown, no era variant), `embark` the landing art of the unit when it has
/// one (its place in the table is RW-14 (g): the caller finds it).
pub fn select_art<'a>(
    record: &'a UnitRecordB2,
    style: Option<&str>,
    era: Option<EraIndex>,
    embarked: bool,
    embark: Option<&'a UnitArtB2>,
) -> ArtChoice<'a> {
    if embarked && let Some(art) = embark {
        return ArtChoice {
            art,
            source: ArtSource::Embarked,
        };
    }
    // The era variant belongs to the base type: it replaces the whole art of the unit.
    if record.era_variation
        && let Some(era) = era
        && let Some(v) = record
            .era_variants
            .iter()
            .filter_map(|v| era_of_tag(&v.suffix).map(|e| (e, v)))
            .filter(|(e, _)| *e <= era)
            .max_by_key(|(e, _)| *e)
            .map(|(_, v)| v)
    {
        return ArtChoice {
            art: &v.art,
            source: ArtSource::Era,
        };
    }
    if let Some(v) = style.and_then(|s| record.style_variants.iter().find(|v| v.suffix == s)) {
        return ArtChoice {
            art: &v.art,
            source: ArtSource::Style,
        };
    }
    ArtChoice {
        art: &record.base,
        source: ArtSource::Base,
    }
}

/// The scale and the z offset of a figure whose cell is empty in the table: the defaults B1 took
/// (`sets/units.rs`).
// TODO(RW-14 (d)): the engine's own defaults for an empty `Scale` and `ZOffset`.
pub const DEFAULT_FIGURE_SCALE: f32 = 1.0;
pub const DEFAULT_FIGURE_Z_OFFSET: f32 = 0.0;

/// One figure of a squad: its model, scale and z offset, in the order the table lists them.
#[derive(Debug, Clone, PartialEq)]
pub struct FigureSpec {
    pub model: LogicalId,
    pub scale: f32,
    pub z_offset: f32,
}

/// The figures of `art`, the kinds repeated by their counts.
pub fn figure_specs(art: &UnitArtB2) -> Vec<FigureSpec> {
    art.figures
        .iter()
        .flat_map(|f: &FigureB2| {
            std::iter::repeat_n(
                FigureSpec {
                    model: f.model.clone(),
                    scale: f.scale.unwrap_or(DEFAULT_FIGURE_SCALE),
                    z_offset: f.z_offset.unwrap_or(DEFAULT_FIGURE_Z_OFFSET),
                },
                f.count as usize,
            )
        })
        .collect()
}

/// The unit types of `table` that would be drawn with no figure at all: the completeness check of
/// the test world (AC-R3.1, "a line of the log is an error of the test"). Empty when every record
/// has figures in its base art and in each variant.
pub fn empty_records(table: &ArtTable) -> Vec<String> {
    let mut bad = Vec::new();
    for (key, rec) in table.iter() {
        let (ArtRecord::UnitB2(rec), Some(unit)) = (rec, key.strip_prefix("unit/")) else {
            continue;
        };
        let arts = std::iter::once(("base", &rec.base))
            .chain(
                rec.style_variants
                    .iter()
                    .map(|v| (v.suffix.as_str(), &v.art)),
            )
            .chain(rec.era_variants.iter().map(|v| (v.suffix.as_str(), &v.art)));
        for (which, art) in arts {
            if figure_specs(art).is_empty() {
                bad.push(format!("{unit} ({which}): no figures"));
            }
        }
    }
    bad
}

#[cfg(test)]
mod tests {
    use asset_format::UnitVariantB2;

    use super::*;

    fn art(tag: &str, count: u32) -> UnitArtB2 {
        UnitArtB2 {
            art_info: format!("ART_{tag}"),
            figures: vec![FigureB2 {
                member: format!("M_{tag}"),
                model: LogicalId::new(format!("md:{tag}")),
                count,
                scale: Some(0.14),
                z_offset: None,
                domain: None,
            }],
            damage_states: None,
            formation: None,
        }
    }

    fn record() -> UnitRecordB2 {
        let v = |suffix: &str, tag: &str| UnitVariantB2 {
            suffix: suffix.into(),
            art: art(tag, 3),
        };
        UnitRecordB2 {
            base: art("worker", 3),
            style_variants: vec![v("_ASIA", "worker_asia"), v("_EURO", "worker_euro")],
            era_variation: true,
            era_variants: vec![v("_LATE", "worker_late"), v("_MODERN", "worker_modern")],
        }
    }

    fn model_of(c: &ArtChoice<'_>) -> String {
        c.art.figures[0].model.as_str().to_owned()
    }

    #[test]
    fn the_style_variant_follows_the_owner_and_the_base_is_the_fallback() {
        let r = record();
        let c = select_art(&r, Some("_ASIA"), None, false, None);
        assert_eq!(
            (model_of(&c), c.source),
            ("md:worker_asia".into(), ArtSource::Style)
        );
        let c = select_art(&r, Some("_MED"), None, false, None);
        assert_eq!(
            (model_of(&c), c.source),
            ("md:worker".into(), ArtSource::Base)
        );
        let c = select_art(&r, None, None, false, None);
        assert_eq!(c.source, ArtSource::Base);
    }

    #[test]
    fn the_era_variant_is_the_latest_not_later_than_the_owner() {
        let r = record();
        let era = |tag: &str| era_of_tag(tag).unwrap();
        // Before every variant: the style or the base.
        let c = select_art(&r, Some("_EURO"), Some(era("_CLASSICAL")), false, None);
        assert_eq!(
            (model_of(&c), c.source),
            ("md:worker_euro".into(), ArtSource::Style)
        );
        // `_LATE` is the Industrial era.
        let c = select_art(&r, Some("_EURO"), Some(era("_INDUSTRIAL")), false, None);
        assert_eq!(
            (model_of(&c), c.source),
            ("md:worker_late".into(), ArtSource::Era)
        );
        let c = select_art(&r, None, Some(era("_POSTMODERN")), false, None);
        assert_eq!(model_of(&c), "md:worker_modern");
        // A unit whose art does not change with the era ignores them.
        let mut fixed = record();
        fixed.era_variation = false;
        let c = select_art(&fixed, None, Some(era("_FUTURE")), false, None);
        assert_eq!(c.source, ArtSource::Base);
    }

    #[test]
    fn the_landing_art_wins_when_embarked() {
        let r = record();
        let landing = art("worker_embarked", 1);
        let c = select_art(&r, Some("_ASIA"), None, true, Some(&landing));
        assert_eq!(c.source, ArtSource::Embarked);
        // Embarked with no landing art known: the usual art.
        let c = select_art(&r, Some("_ASIA"), None, true, None);
        assert_eq!(c.source, ArtSource::Style);
        // The landing art is for the embarked only.
        let c = select_art(&r, None, None, false, Some(&landing));
        assert_eq!(c.source, ArtSource::Base);
    }

    #[test]
    fn the_boat_of_an_embarked_unit_follows_the_era_and_the_override() {
        assert_eq!(embark_art_info(None, None), "ART_DEF_UNIT_GALLEY");
        assert_eq!(embark_art_info(Some(2), None), "ART_DEF_UNIT_GALLEY");
        assert_eq!(embark_art_info(Some(3), None), "ART_DEF_UNIT_GALLEON");
        assert_eq!(embark_art_info(Some(4), None), "ART_DEF_UNIT_TRANSPORT");
        assert_eq!(embark_art_info(Some(7), None), "ART_DEF_UNIT_TRANSPORT");
        let canoe = "ART_DEF_UNIT_U_POLYNESIAN_WAR_CANOE";
        assert_eq!(embark_art_info(Some(6), Some(canoe)), canoe);
        assert_eq!(embark_art_info(Some(0), Some("")), "ART_DEF_UNIT_GALLEY");
    }

    #[test]
    fn the_figures_repeat_by_count_with_the_defaults_for_empty_cells() {
        let mut a = art("w", 3);
        a.figures.push(FigureB2 {
            member: "M2".into(),
            model: LogicalId::new("md:m2"),
            count: 2,
            scale: None,
            z_offset: Some(4.0),
            domain: None,
        });
        let f = figure_specs(&a);
        assert_eq!(f.len(), 5);
        assert_eq!((f[0].scale, f[0].z_offset), (0.14, 0.0));
        assert_eq!((f[3].scale, f[3].z_offset), (DEFAULT_FIGURE_SCALE, 4.0));
        assert_eq!(f[3].model.as_str(), "md:m2");
    }

    #[test]
    fn a_record_without_figures_is_reported() {
        let mut t = ArtTable::new();
        t.insert_unit_b2("UNIT_WORKER", record());
        assert!(empty_records(&t).is_empty());
        let mut r = record();
        r.style_variants[0].art.figures.clear();
        t.insert_unit_b2("UNIT_ODD", r);
        assert_eq!(empty_records(&t), ["UNIT_ODD (_ASIA): no figures"]);
    }

    #[test]
    fn era_tags_are_in_order() {
        assert_eq!(era_of_tag("_ANCIENT"), Some(0));
        assert_eq!(era_of_tag("_FUTURE"), Some(7));
        assert_eq!(era_of_tag("_LATE"), era_of_tag("_INDUSTRIAL"));
        assert_eq!(era_of_tag("_NOPE"), None);
    }
}
