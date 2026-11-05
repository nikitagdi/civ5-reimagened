//! Which `ArtDefine_Landmarks` row a plot draws (world-objects 4.5, WOB4).
//!
//! A row is keyed by the pair (improvement, resource); a feature stands in the resource column of
//! the rows of `ART_DEF_IMPROVEMENT_NONE`. The rows of the pair are matched in this order
//! (RW-15 (a) has not answered; this is the rule by the columns of the DB):
//!
//! 1. the resource column, the most exact first: the name of the resource with the suffixes the plot
//!    asks for (`_HEAVY`, `_MARSH`, `_HILL`, `_WATER`), then the plain name, then `RESOURCE_ALL`, then
//!    `RESOURCE_NONE`; the first of these that has a usable row ends the search;
//! 2. inside it a row must fit the owner of the plot: its `Era` is `Any` or not later than the
//!    owner's, its `State` the plot's or `Any`, its `Tech` one the owner has (a raw resource tests
//!    the era only);
//! 3. of those, a technology row wins over one with none and the rest tie; of tied rows a deciding
//!    handler takes the last in table order, a random one (`RANDOM`, `ANIMATED_ROTATED`) draws one
//!    ([`pick_variant`]).
//!
//! A plot that is pillaged or under construction and has no row for that state draws the finished
//! one.

use asset_format::{ArtTable, LandmarkArt};

use crate::anim::LandmarkState;
use crate::entities::{PlotDecor, ResourceVariant};

pub const IMPROVEMENT_NONE: &str = "ART_DEF_IMPROVEMENT_NONE";
pub const RESOURCE_ALL: &str = "ART_DEF_RESOURCE_ALL";
pub const RESOURCE_NONE: &str = "ART_DEF_RESOURCE_NONE";

/// The era column of the rows: the DB has `Ancient`, `Industrial` and `Any`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LandmarkEra {
    Ancient,
    Industrial,
}

impl LandmarkEra {
    pub fn name(self) -> &'static str {
        match self {
            Self::Ancient => "Ancient",
            Self::Industrial => "Industrial",
        }
    }

    pub fn of(industrial: bool) -> Self {
        if industrial {
            Self::Industrial
        } else {
            Self::Ancient
        }
    }
}

/// What a plot asks of the table.
pub struct Query<'a> {
    pub improvement: &'a str,
    /// The resource names to try, most exact first (see [`resource_names`]).
    pub resources: &'a [String],
    pub era: LandmarkEra,
    pub state: LandmarkState,
    /// Whether the owner has a technology (`Tech` column), by its type name.
    pub techs: &'a dyn Fn(&str) -> bool,
}

/// The rows of one pair, in table order.
pub fn pair_rows<'a>(art: &'a ArtTable, improvement: &str, resource: &str) -> Vec<&'a LandmarkArt> {
    (0..)
        .map_while(|n| art.landmark(improvement, resource, n))
        .collect()
}

/// The `State` column of a state of the animation: the DB has no state for a raw resource.
fn db_state(s: LandmarkState) -> &'static str {
    match s {
        LandmarkState::UnderConstruction => "UnderConstruction",
        LandmarkState::Pillaged => "Pillaged",
        LandmarkState::Constructed | LandmarkState::RawResource => "Constructed",
    }
}

/// The names of a resource a plot can show, most exact first: `IRON_HEAVY_MARSH`, `IRON_HEAVY`,
/// `IRON_MARSH`, `IRON`. A suffix is only tried when the plot asks for it.
pub fn resource_names(base: &str, v: ResourceVariant) -> Vec<String> {
    let terrain = if v.marsh {
        Some("_MARSH")
    } else if v.hill {
        Some("_HILL")
    } else if v.water {
        Some("_WATER")
    } else {
        None
    };
    let mut out = Vec::new();
    let mut add = |heavy: bool, terrain: Option<&str>| {
        let name = format!(
            "{base}{}{}",
            if heavy { "_HEAVY" } else { "" },
            terrain.unwrap_or("")
        );
        if !out.contains(&name) {
            out.push(name);
        }
    };
    if v.heavy {
        add(true, terrain);
    }
    if terrain.is_some() && v.heavy {
        add(true, None);
    }
    add(false, terrain);
    add(false, None);
    out
}

/// The rank of an era of the table: a row of an era fits an owner of that era or a later one.
fn era_rank(era: &str) -> Option<u8> {
    match era {
        "Ancient" => Some(0),
        "Industrial" => Some(1),
        _ => None,
    }
}

/// The rows of `improvement` and `resource` that fit the owner, the best ones: `state` is the DB
/// name of the state wanted.
///
/// A row fits when its era is `Any` or not later than the owner's, its state is `Any` or the
/// plot's and its technology (if any) is one the owner has; of those the technology rows win and
/// the rest tie (an exact state or era is no better than `Any`). A raw resource asks no state and no
/// technology: every row of an era that fits ties.
// re: cvLandmarkLibrary::GetLandmarkEntryIndex (cvLandmarkLibrary.cpp:34-71: score 2 tech owned,
// 1 no tech gate; era `entry.era <= era || entry.era == 0`; state `entry.state == state || Any`;
// the iType < 0 branch of a raw resource tests the era only), RE-B18 item 5 F2, F3
fn best_rows<'a>(
    art: &'a ArtTable,
    improvement: &str,
    resource: &str,
    q: &Query<'_>,
    state: &str,
) -> Vec<&'a LandmarkArt> {
    let owner_era = era_rank(q.era.name()).unwrap_or(0);
    let raw = q.state == LandmarkState::RawResource;
    let mut best: Vec<&LandmarkArt> = Vec::new();
    let mut best_rank = 0u8;
    for row in pair_rows(art, improvement, resource) {
        if era_rank(&row.era).is_some_and(|e| e > owner_era) {
            continue;
        }
        if !raw && row.state != "Any" && row.state != state {
            continue;
        }
        if !raw && row.tech.as_deref().is_some_and(|t| !(q.techs)(t)) {
            continue;
        }
        let rank = u8::from(!raw && row.tech.is_some());
        if best.is_empty() || rank > best_rank {
            best.clear();
            best_rank = rank;
        }
        if rank == best_rank {
            best.push(row);
        }
    }
    best
}
/// The tied rows for the query, and the name in the resource column they were found under. Walks
/// the resource names, then `ALL`, then `NONE`.
pub fn candidates<'a>(art: &'a ArtTable, q: &Query<'_>) -> Option<(String, Vec<&'a LandmarkArt>)> {
    let mut states = vec![db_state(q.state)];
    if states[0] != "Constructed" {
        states.push("Constructed");
    }
    for state in states {
        let names = q
            .resources
            .iter()
            .map(String::as_str)
            .chain([RESOURCE_ALL, RESOURCE_NONE]);
        for name in names {
            let rows = best_rows(art, q.improvement, name, q, state);
            if !rows.is_empty() {
                return Some((name.to_owned(), rows));
            }
        }
    }
    None
}

/// True for a handler whose rows are alternatives drawn at random (`RANDOM`, `ANIMATED_ROTATED`, and the
/// farm's variants); the others take one row by rule.
// re: cvLandmarkSimSystem::Startup 0x087d5a38 (random list = handler RANDOM), cvAnimatedSimSystem
// 0x08a18674 (list B = ANIMATED_ROTATED, the random fallback of list A), OnImprovementCreated 0x08a1a894
// (farm: rand() % numVariants[era])
pub fn random_handler(layout: &str) -> bool {
    matches!(layout, "RANDOM" | "ANIMATED_ROTATED" | "FARM")
}

/// The row of tied rows a plot gets: the last one of a handler that decides (the table order), else
/// one of the random handler's rows. The choice of the engine is `rand`, ours is a generator seeded
/// by the plot (`cell`) and the place of the landmark on it (`salt`).
// re: cvLandmarkLibrary.cpp:34-71 (deterministic: scan from the end, strict `<` keeps the last of a
// tie), :74-106 GetRandomLandmarkEntryIndex (rand() % matches); the deterministic list is asked first
// SIMPLIFIED(SIMP-046): the original draws `GetRandomLandmarkEntryIndex` from its `rand` stream;
// the set of candidates is the same, the pick is ours and stays the same for a plot.
pub fn pick_variant<'a>(rows: &[&'a LandmarkArt], cell: u32, salt: u32) -> Option<&'a LandmarkArt> {
    if let Some(last) = rows.iter().rev().find(|r| !random_handler(&r.layout)) {
        return Some(last);
    }
    if rows.is_empty() {
        return None;
    }
    Some(rows[mix(cell, salt) as usize % rows.len()])
}
/// A number from a plot and a salt (splitmix64 of both).
pub fn mix(cell: u32, salt: u32) -> u32 {
    let mut z = (u64::from(cell) << 32 | u64::from(salt)).wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    ((z ^ (z >> 31)) >> 8) as u32
}

/// The art define of the trading post (`Improvements.ArtDefineTag` of `IMPROVEMENT_TRADING_POST`): the table has no
/// rows under it, only under `..._TRADING_POST1` .. `..._TRADING_POST4`.
pub const TRADING_POST: &str = "ART_DEF_IMPROVEMENT_TRADING_POST";

/// How many numbered trading posts the exe picks from.
pub const TRADING_POST_VARIANTS: u32 = 4;

/// The numbered landmark type a trading post draws (`ART_DEF_IMPROVEMENT_TRADING_POST1` .. `4`); `None` for every
/// other improvement.
// re: CvDLLUtility::GameplayPlotStateChange 0x087a5c70 (when the improvement is the one DoMapSetup 0x0879502c
// looked up as "IMPROVEMENT_TRADING_POST", the landmark type is its ArtDefineTag + itoa(rand() % 4 + 1)); every
// other improvement looks its tag up as it is. The variant is one of the four on any terrain and feature.
// SIMPLIFIED(SIMP-130): the exe draws the number from `rand` at each plot state change; ours is a function of the
// plot (`mix(cell, 3)`), so a plot keeps its trading post across refreshes and between players.
pub fn trading_post_variant(tag: &str, cell: u32) -> Option<String> {
    (tag == TRADING_POST).then(|| format!("{tag}{}", mix(cell, 3) % TRADING_POST_VARIANTS + 1))
}

/// What a plot draws of one kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Improvement,
    Resource,
    Feature,
}

/// One landmark of a plot: the row, what it is, and the state its animation is in.
#[derive(Debug, Clone, Copy)]
pub struct Wanted<'a> {
    pub row: &'a LandmarkArt,
    pub kind: Kind,
    pub state: LandmarkState,
}

/// What lies under the owner of a plot for the choice.
pub struct Owner<'a> {
    pub era: LandmarkEra,
    pub techs: &'a dyn Fn(&str) -> bool,
}

/// The landmarks of a plot, in the order they are drawn: improvement, resource, feature. `cell` is
/// the number of the plot (the variants are chosen from it).
///
/// A resource is drawn alone when the plot has no improvement, and beside the improvement when the
/// improvement's row is generic (`RESOURCE_ALL` or `NONE`); a row for the pair (improvement,
/// resource) is the model of both.
pub fn plan_plot<'a>(
    art: &'a ArtTable,
    plot: &PlotDecor,
    owner: &Owner<'_>,
    cell: u32,
) -> Vec<Wanted<'a>> {
    let mut out = Vec::new();
    let resources: Vec<String> = plot
        .resource
        .as_deref()
        .map(|r| resource_names(r, plot.variant))
        .unwrap_or_default();
    let mut resource_done = false;
    let building = plot.building.as_deref();
    // No resource on the plot: the `NONE` rows are the exact ones, `ALL` the fallback.
    let for_improvement: Vec<String> = if resources.is_empty() {
        vec![RESOURCE_NONE.to_owned()]
    } else {
        resources.clone()
    };
    if let Some(tag) = building.or(plot.improvement.as_deref()) {
        let numbered = trading_post_variant(tag, cell);
        let tag = numbered.as_deref().unwrap_or(tag);
        let state = if building.is_some() {
            LandmarkState::UnderConstruction
        } else if plot.pillaged {
            LandmarkState::Pillaged
        } else {
            LandmarkState::Constructed
        };
        let q = Query {
            improvement: tag,
            resources: &for_improvement,
            era: owner.era,
            state,
            techs: owner.techs,
        };
        if let Some((found, rows)) = candidates(art, &q) {
            if let Some(row) = pick_variant(&rows, cell, 0) {
                out.push(Wanted {
                    row,
                    kind: Kind::Improvement,
                    state,
                });
            }
            resource_done = resources.contains(&found);
        }
    }
    if !resource_done && !resources.is_empty() {
        let q = Query {
            improvement: IMPROVEMENT_NONE,
            resources: &resources,
            era: owner.era,
            state: LandmarkState::RawResource,
            techs: owner.techs,
        };
        if let Some((found, rows)) = candidates(art, &q)
            && found != RESOURCE_ALL
            && found != RESOURCE_NONE
            && let Some(row) = pick_variant(&rows, cell, 1)
        {
            out.push(Wanted {
                row,
                kind: Kind::Resource,
                state: if plot.improvement.is_some() || building.is_some() {
                    LandmarkState::Constructed
                } else {
                    LandmarkState::RawResource
                },
            });
        }
    }
    if let Some(feature) = plot.feature.as_deref() {
        let names = [feature.to_owned()];
        let q = Query {
            improvement: IMPROVEMENT_NONE,
            resources: &names,
            era: owner.era,
            state: LandmarkState::Constructed,
            techs: owner.techs,
        };
        if let Some((found, rows)) = candidates(art, &q)
            && found == feature
            && let Some(row) = pick_variant(&rows, cell, 2)
        {
            out.push(Wanted {
                row,
                kind: Kind::Feature,
                state: LandmarkState::Constructed,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use asset_format::LogicalId;

    use super::*;

    #[allow(clippy::too_many_arguments)]
    fn row(
        imp: &str,
        res: &str,
        era: &str,
        state: &str,
        tech: Option<&str>,
        model: &str,
    ) -> LandmarkArt {
        LandmarkArt {
            improvement: imp.into(),
            resource: res.into(),
            era: era.into(),
            state: state.into(),
            layout: "SNAPSHOT".into(),
            scale: 1.0,
            terrain_contour: 0,
            tech: tech.map(Into::into),
            model: Some(LogicalId::new(model)),
        }
    }

    /// Adds the rows with their numbers inside the pair, as the pipeline does.
    fn table(rows: Vec<LandmarkArt>) -> ArtTable {
        let mut t = ArtTable::default();
        let mut count = std::collections::HashMap::new();
        for r in rows {
            let n = count
                .entry((r.improvement.clone(), r.resource.clone()))
                .or_insert(0usize);
            t.insert_landmark(*n, r);
            *n += 1;
        }
        t
    }

    fn plot(imp: Option<&str>, res: Option<&str>) -> PlotDecor {
        PlotDecor {
            improvement: imp.map(Into::into),
            resource: res.map(Into::into),
            ..PlotDecor::default()
        }
    }

    fn models(w: &[Wanted<'_>]) -> Vec<String> {
        w.iter()
            .map(|w| w.row.model.as_ref().unwrap().as_str().to_owned())
            .collect()
    }

    const MINE: &str = "ART_DEF_IMPROVEMENT_MINE";
    const IRON: &str = "ART_DEF_RESOURCE_IRON";

    fn no_techs(_: &str) -> bool {
        false
    }

    fn owner(era: LandmarkEra) -> Owner<'static> {
        Owner {
            era,
            techs: &no_techs,
        }
    }

    #[test]
    fn the_exact_resource_comes_before_all_and_none() {
        let t = table(vec![
            row(MINE, RESOURCE_NONE, "Any", "Any", None, "md:none"),
            row(MINE, RESOURCE_ALL, "Any", "Any", None, "md:all"),
            row(MINE, IRON, "Any", "Any", None, "md:iron"),
        ]);
        let w = plan_plot(
            &t,
            &plot(Some(MINE), Some(IRON)),
            &owner(LandmarkEra::Ancient),
            5,
        );
        assert_eq!(models(&w), ["md:iron"]);
        // A resource without a row of its own: ALL before NONE.
        let w = plan_plot(
            &t,
            &plot(Some(MINE), Some("ART_DEF_RESOURCE_GOLD")),
            &owner(LandmarkEra::Ancient),
            5,
        );
        assert_eq!(models(&w), ["md:all"]);
        // No resource at all: NONE is the exact one.
        let w = plan_plot(&t, &plot(Some(MINE), None), &owner(LandmarkEra::Ancient), 5);
        assert_eq!(models(&w), ["md:none"]);
    }

    #[test]
    fn the_era_and_the_state_of_the_owner_choose_between_rows() {
        let t = table(vec![
            row(MINE, IRON, "Ancient", "Constructed", None, "md:anc"),
            row(MINE, IRON, "Ancient", "Pillaged", None, "md:anc_pl"),
            row(MINE, IRON, "Industrial", "Constructed", None, "md:ind"),
            row(MINE, IRON, "Any", "UnderConstruction", None, "md:hb"),
        ]);
        let p = plot(Some(MINE), Some(IRON));
        let anc = owner(LandmarkEra::Ancient);
        let ind = owner(LandmarkEra::Industrial);
        assert_eq!(models(&plan_plot(&t, &p, &anc, 1)), ["md:anc"]);
        assert_eq!(models(&plan_plot(&t, &p, &ind, 1)), ["md:ind"]);
        let pillaged = PlotDecor {
            pillaged: true,
            ..p.clone()
        };
        assert_eq!(models(&plan_plot(&t, &pillaged, &anc, 1)), ["md:anc_pl"]);
        // An Ancient row fits an Industrial owner: its pillaged row is the one.
        assert_eq!(models(&plan_plot(&t, &pillaged, &ind, 1)), ["md:anc_pl"]);
        let building = PlotDecor {
            building: Some(MINE.into()),
            improvement: None,
            ..p
        };
        assert_eq!(models(&plan_plot(&t, &building, &ind, 1)), ["md:hb"]);
        assert_eq!(
            plan_plot(&t, &building, &ind, 1)[0].state,
            LandmarkState::UnderConstruction
        );
    }

    #[test]
    fn a_technology_row_needs_the_technology_and_beats_the_plain_one() {
        let polder = "ART_DEF_IMPROVEMENT_POLDER";
        let t = table(vec![
            row(
                polder,
                RESOURCE_ALL,
                "Ancient",
                "Constructed",
                None,
                "md:anc",
            ),
            row(
                polder,
                RESOURCE_ALL,
                "Ancient",
                "Constructed",
                Some("TECH_ECONOMICS"),
                "md:ind",
            ),
        ]);
        let p = plot(Some(polder), None);
        let without = owner(LandmarkEra::Ancient);
        assert_eq!(models(&plan_plot(&t, &p, &without, 3)), ["md:anc"]);
        let has = |t: &str| t == "TECH_ECONOMICS";
        let with = Owner {
            era: LandmarkEra::Ancient,
            techs: &has,
        };
        assert_eq!(models(&plan_plot(&t, &p, &with, 3)), ["md:ind"]);
    }

    #[test]
    fn a_resource_stands_alone_or_beside_a_generic_improvement() {
        let tp = "ART_DEF_IMPROVEMENT_TRADING_POST1";
        let t = table(vec![
            row(IMPROVEMENT_NONE, IRON, "Any", "Any", None, "md:ore"),
            row(tp, RESOURCE_ALL, "Any", "Any", None, "md:post"),
            row(MINE, IRON, "Any", "Any", None, "md:mine_iron"),
        ]);
        let a = owner(LandmarkEra::Ancient);
        // Alone.
        let w = plan_plot(&t, &plot(None, Some(IRON)), &a, 0);
        assert_eq!(models(&w), ["md:ore"]);
        assert_eq!(w[0].state, LandmarkState::RawResource);
        // Beside a generic improvement.
        let w = plan_plot(&t, &plot(Some(tp), Some(IRON)), &a, 0);
        assert_eq!(models(&w), ["md:post", "md:ore"]);
        // The row of the pair is the model of both.
        let w = plan_plot(&t, &plot(Some(MINE), Some(IRON)), &a, 0);
        assert_eq!(models(&w), ["md:mine_iron"]);
    }

    #[test]
    fn a_feature_is_the_resource_column_of_the_none_improvement() {
        let t = table(vec![row(
            IMPROVEMENT_NONE,
            "ART_DEF_FEATURE_OASIS",
            "Any",
            "Any",
            None,
            "md:oasis",
        )]);
        let p = PlotDecor {
            feature: Some("ART_DEF_FEATURE_OASIS".into()),
            ..PlotDecor::default()
        };
        assert_eq!(
            models(&plan_plot(&t, &p, &owner(LandmarkEra::Ancient), 9)),
            ["md:oasis"]
        );
        // A feature without a row draws nothing (and is no `ALL` row of the improvement).
        let q = PlotDecor {
            feature: Some("ART_DEF_FEATURE_FUJI".into()),
            ..PlotDecor::default()
        };
        assert!(plan_plot(&t, &q, &owner(LandmarkEra::Ancient), 9).is_empty());
    }

    #[test]
    fn the_variant_is_a_function_of_the_plot_and_covers_all_rows() {
        let hut = "ART_DEF_IMPROVEMENT_GOODY_HUT";
        let t = table(
            (0..10)
                .map(|i| LandmarkArt {
                    layout: "RANDOM".into(),
                    ..row(
                        hut,
                        RESOURCE_ALL,
                        "Any",
                        "Any",
                        None,
                        &format!("md:ruins{i}"),
                    )
                })
                .collect(),
        );
        let a = owner(LandmarkEra::Ancient);
        let p = plot(Some(hut), None);
        let first = models(&plan_plot(&t, &p, &a, 77));
        assert_eq!(
            first,
            models(&plan_plot(&t, &p, &a, 77)),
            "same plot, same model"
        );
        let mut seen = std::collections::BTreeSet::new();
        for cell in 0..400 {
            seen.extend(models(&plan_plot(&t, &p, &a, cell)));
        }
        assert_eq!(seen.len(), 10, "every variant is somewhere");
    }

    #[test]
    fn a_deciding_handler_takes_the_last_tied_row_and_an_exact_state_or_era_is_no_better() {
        let t = table(vec![
            row(MINE, IRON, "Ancient", "Constructed", None, "md:exact"),
            row(MINE, IRON, "Any", "Any", None, "md:any"),
        ]);
        let p = plot(Some(MINE), Some(IRON));
        for cell in 0..50 {
            assert_eq!(
                models(&plan_plot(&t, &p, &owner(LandmarkEra::Ancient), cell)),
                ["md:any"]
            );
        }
    }

    #[test]
    fn a_raw_resource_takes_the_last_row_of_its_era_whatever_the_state_and_technology() {
        let t = table(vec![
            row(IMPROVEMENT_NONE, IRON, "Any", "Any", None, "md:ore"),
            row(
                IMPROVEMENT_NONE,
                IRON,
                "Any",
                "Pillaged",
                Some("TECH_X"),
                "md:late",
            ),
            row(IMPROVEMENT_NONE, IRON, "Industrial", "Any", None, "md:ind"),
        ]);
        let p = plot(None, Some(IRON));
        assert_eq!(
            models(&plan_plot(&t, &p, &owner(LandmarkEra::Ancient), 0)),
            ["md:late"]
        );
        assert_eq!(
            models(&plan_plot(&t, &p, &owner(LandmarkEra::Industrial), 0)),
            ["md:ind"]
        );
    }

    #[test]
    fn the_names_of_a_resource_go_from_exact_to_plain() {
        let v = ResourceVariant {
            heavy: true,
            marsh: true,
            ..ResourceVariant::default()
        };
        assert_eq!(
            resource_names("R", v),
            ["R_HEAVY_MARSH", "R_HEAVY", "R_MARSH", "R"]
        );
        assert_eq!(resource_names("R", ResourceVariant::default()), ["R"]);
        let hill = ResourceVariant {
            hill: true,
            ..ResourceVariant::default()
        };
        assert_eq!(resource_names("R", hill), ["R_HILL", "R"]);
        let t = table(vec![
            row(
                IMPROVEMENT_NONE,
                "R_HEAVY_MARSH",
                "Any",
                "Any",
                None,
                "md:hm",
            ),
            row(IMPROVEMENT_NONE, "R", "Any", "Any", None, "md:plain"),
        ]);
        let mut p = plot(None, Some("R"));
        p.variant = v;
        assert_eq!(
            models(&plan_plot(&t, &p, &owner(LandmarkEra::Ancient), 0)),
            ["md:hm"]
        );
        p.variant = ResourceVariant::default();
        assert_eq!(
            models(&plan_plot(&t, &p, &owner(LandmarkEra::Ancient), 0)),
            ["md:plain"]
        );
    }

    /// The trading post's tag has no rows of its own: it draws one of the four numbered posts, on any plot (the
    /// jungle of the live report 2026-10-31 included), the same one for a plot every time.
    #[test]
    fn a_trading_post_draws_one_of_the_four_numbered_posts() {
        let rows = (1..=4)
            .flat_map(|n| {
                let imp = format!("{TRADING_POST}{n}");
                [
                    row(
                        &imp,
                        RESOURCE_ALL,
                        "Ancient",
                        "Constructed",
                        None,
                        &format!("md:tp{n}"),
                    ),
                    row(
                        &imp,
                        RESOURCE_ALL,
                        "Ancient",
                        "UnderConstruction",
                        None,
                        &format!("md:hb{n}"),
                    ),
                ]
            })
            .collect();
        let t = table(rows);
        let a = owner(LandmarkEra::Ancient);
        let jungle = PlotDecor {
            improvement: Some(TRADING_POST.into()),
            feature: Some("ART_DEF_FEATURE_JUNGLE".into()),
            ..PlotDecor::default()
        };
        let mut seen = std::collections::BTreeSet::new();
        for cell in 0..200 {
            let w = models(&plan_plot(&t, &jungle, &a, cell));
            assert_eq!(w.len(), 1, "{cell}: {w:?}");
            assert_eq!(w, models(&plan_plot(&t, &jungle, &a, cell)));
            seen.insert(w[0].clone());
        }
        assert_eq!(seen.len(), 4, "{seen:?}");
        // Being built: the numbered post's `UnderConstruction` row.
        let building = PlotDecor {
            building: Some(TRADING_POST.into()),
            ..PlotDecor::default()
        };
        assert!(models(&plan_plot(&t, &building, &a, 7))[0].starts_with("md:hb"));
        assert_eq!(trading_post_variant("ART_DEF_IMPROVEMENT_FARM", 7), None);
    }

    #[test]
    fn a_row_with_no_model_is_a_choice_that_draws_nothing() {
        let mut empty = row(MINE, IRON, "Any", "Any", None, "x");
        empty.model = None;
        let t = table(vec![
            empty,
            row(MINE, RESOURCE_ALL, "Any", "Any", None, "md:all"),
        ]);
        let w = plan_plot(
            &t,
            &plot(Some(MINE), Some(IRON)),
            &owner(LandmarkEra::Ancient),
            0,
        );
        assert_eq!(w.len(), 1);
        assert!(w[0].row.model.is_none());
    }
}
