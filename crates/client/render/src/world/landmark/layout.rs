//! The layout handlers of a landmark row (`LayoutHandler`, world-objects 4.5, WOB4): how the parts
//! of its model stand on the plot. `SNAPSHOT` is in [`super::snapshot`].
//!
//! | handler | what the models are | what we draw |
//! |---|---|---|
//! | `RANDOM` (ruins, camps, kasbah, markets, atolls) | parts named `modelN`, each at its authored `InitialPlacement` | every part where the model puts it, not turned; the plot draws one of the tied rows at random (`select::pick_variant`) |
//! | `FARM` | the same | every part at its authored place, the group on the field nearest the centre and turned by the layout heading and a whole number of quarter turns (`decals::farm_anchor`, placed by the caller) |
//! | `SPECIAL` | the same | every part at its authored place, not turned (docks, statues have a front) |
//! | `ANIMATED` (herds, camps, boats, rigs) | the animals/boats hang on bones of the clip; the parts all stand at the origin | the whole model, minus the parts of another state (pillaged, under construction) |
//! | `ANIMATED_ROTATED` (horses) | the same | as `ANIMATED`, not turned; a random row when no `ANIMATED` row fits |
//!
//! No handler turns a landmark: every piece is `variant[p][rand % n] * base[p]` at the hex centre,
//! the variants the poses of bone 0 of up to four idle clips at t = 0 (`ec` 1000..1003 of the
//! `.fxsxml`), which the herds take with the clip they play (`anim::landmark_tree`); a model with no
//! clips has no variant. The names "RANDOM" and "ANIMATED_ROTATED" mean a random choice among the
//! tied rows. The part names that mark a state are read by eye.
// re: FUN_087d8824 (variant draw), FUN_087d89f2 / FUN_08a1a540 (`* T(hex centre)` only), the feature,
// improvement and resource placers 0x087d83a2, 0x087d8162, 0x087d7f22, 0x08a1a3a6, 0x08a1a5b4: no
// D3DXMatrixRotation (RE-B18 item 3c F4, item 5 F1, F2)

use crate::anim::LandmarkState;

/// A layout handler of the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Snapshot,
    Random,
    Farm,
    Special,
    Animated,
    AnimatedRotated,
}

impl Layout {
    /// The handler a row names; `None` for a name the table does not use.
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "SNAPSHOT" => Self::Snapshot,
            "RANDOM" => Self::Random,
            "FARM" => Self::Farm,
            "SPECIAL" => Self::Special,
            "ANIMATED" | "PASTURE" => Self::Animated,
            "ANIMATED_ROTATED" => Self::AnimatedRotated,
            // The doc of the table names SPRING; no row has it yet.
            "SPRING" => Self::Special,
            _ => return None,
        })
    }
}

/// What the layout needs of a part of the model.
#[derive(Debug, Clone, Copy)]
pub struct PartInfo<'a> {
    pub index: u16,
    /// The name in the gr2 (`model3`, `Pasture_Pillaged`, `Dummy_CowLocation`).
    pub name: &'a str,
    /// Its `InitialPlacement` is the identity: nothing to apply.
    pub identity: bool,
}

/// How the model stands on the plot.
#[derive(Debug, Clone, PartialEq)]
pub struct Arrangement {
    /// The turn of the group about the middle of the plot, radians (about Y, B1's `rotate_y`).
    pub yaw: f32,
    /// `None`: the whole model in one piece. `Some`: these parts, each a piece of its own.
    pub parts: Option<Vec<u16>>,
    /// The part's authored placement is applied to its piece (never to the whole model, whose
    /// placements are in the vertices when the pipeline merged it).
    pub authored: bool,
}

impl Arrangement {
    /// The whole model, as it is.
    pub const WHOLE: Self = Self {
        yaw: 0.0,
        parts: None,
        authored: false,
    };
}

/// True if the part is drawn in `state`. A part that is named for a state belongs to it: `..Pillaged`
/// and `.._PL` to the pillaged one, `..Construction` to the one under construction, the bare
/// `Pasture` to the finished one; every other part (animals, the ground, the nodes) is always
/// there.
pub fn part_in_state(name: &str, state: LandmarkState) -> bool {
    let n = name.to_ascii_lowercase();
    if n.contains("pillaged") || n.ends_with("_pl") || n.starts_with("pl_") || n.starts_with("pl ")
    {
        state == LandmarkState::Pillaged
    } else if n.contains("construction") || n.starts_with("hb_") {
        state == LandmarkState::UnderConstruction
    } else if n == "pasture" {
        state == LandmarkState::Constructed
    } else {
        true
    }
}

/// A part name without its state prefix (`PL_`/`PL `/`HB_`), lowercase, spaces as `_`; `None` if it
/// has none.
fn state_twin_base(name: &str) -> Option<String> {
    let n = name.to_ascii_lowercase().replace(' ', "_");
    n.strip_prefix("pl_")
        .or_else(|| n.strip_prefix("hb_"))
        .map(str::to_owned)
}

/// True if the part is drawn in `state` of a model whose parts are `all`: as [`part_in_state`], and a
/// part that has a pillaged or under-construction twin (`Camp_MID_Deer` beside `PL_Camp_MID_Deer`,
/// `HB_Camp_MID_Deer`) is the finished improvement: only a `Constructed` plot has it, a raw resource
/// does not.
// re: `GetLandmarkEntryIndex` (cvLandmarkLibrary.cpp) matches rows by state only; the resource rows
// (`Resource_Deer.fxsxml` for NONE and for CAMP alike) carry the camp as `Camp_*`/`PL_Camp_*`/`HB_Camp_*`
// meshes of the model, so an unimproved plot must not show the `Camp_*` ones (golden S2).
fn part_shown(name: &str, all: &[PartInfo<'_>], state: LandmarkState) -> bool {
    if !part_in_state(name, state) {
        return false;
    }
    if state == LandmarkState::Constructed || state_twin_base(name).is_some() {
        return true;
    }
    let me = name.to_ascii_lowercase().replace(' ', "_");
    !all.iter()
        .filter_map(|p| state_twin_base(p.name))
        .any(|base| base == me)
}

/// How the parts of a model stand on plot `cell` in `state`.
pub fn arrange(
    layout: Layout,
    _cell: u32,
    parts: &[PartInfo<'_>],
    state: LandmarkState,
) -> Arrangement {
    let all = || Some(parts.iter().map(|p| p.index).collect::<Vec<_>>());
    match layout {
        Layout::Snapshot => Arrangement::WHOLE,
        Layout::Random | Layout::Farm | Layout::Special => Arrangement {
            yaw: 0.0,
            parts: all(),
            authored: true,
        },
        Layout::Animated | Layout::AnimatedRotated => {
            let yaw = 0.0;
            let shown: Vec<u16> = parts
                .iter()
                .filter(|p| part_shown(p.name, parts, state))
                .map(|p| p.index)
                .collect();
            if shown.len() == parts.len() && parts.iter().all(|p| p.identity) {
                Arrangement {
                    yaw,
                    ..Arrangement::WHOLE
                }
            } else {
                Arrangement {
                    yaw,
                    parts: Some(shown),
                    authored: true,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts<'a>(names: &[&'a str]) -> Vec<PartInfo<'a>> {
        names
            .iter()
            .enumerate()
            .map(|(i, n)| PartInfo {
                index: i as u16,
                name: n,
                identity: true,
            })
            .collect()
    }

    #[test]
    fn the_handlers_of_the_table_parse() {
        for (name, want) in [
            ("SNAPSHOT", Layout::Snapshot),
            ("RANDOM", Layout::Random),
            ("FARM", Layout::Farm),
            ("SPECIAL", Layout::Special),
            ("ANIMATED", Layout::Animated),
            ("ANIMATED_ROTATED", Layout::AnimatedRotated),
        ] {
            assert_eq!(Layout::parse(name), Some(want));
        }
        assert_eq!(Layout::parse("nonsense"), None);
    }

    #[test]
    fn a_pasture_shows_the_parts_of_its_state() {
        let names = [
            "Dummy_CowLocation",
            "Pasture_Pillaged",
            "Pasture",
            "Pasture_Construction",
            "Decal_pastureGround",
            "Decal_PastureRockwall_PL",
            "FX_PillagedSmoke",
        ];
        let p = parts(&names);
        let shown = |s| arrange(Layout::Animated, 3, &p, s).parts.unwrap();
        assert_eq!(shown(LandmarkState::Constructed), [0, 2, 4]);
        assert_eq!(shown(LandmarkState::Pillaged), [0, 1, 4, 5, 6]);
        assert_eq!(shown(LandmarkState::UnderConstruction), [0, 3, 4]);
        // A raw resource has no fence.
        assert_eq!(shown(LandmarkState::RawResource), [0, 4]);
        // Every part always on: the whole model, one piece.
        let herd = parts(&["Dummy_CowLocation", "Bone_Main"]);
        let a = arrange(Layout::Animated, 3, &herd, LandmarkState::Constructed);
        assert_eq!(a, Arrangement::WHOLE);
    }

    #[test]
    fn a_raw_resource_does_not_show_the_camp_of_its_model() {
        let names = [
            "Dummy_Buck02",
            "Tree cut",
            "PL Camp MID Deer",
            "PL_Camp_MID_Deer_Ground",
            "HB_Camp_MID_Deer",
            "HB_Camp_MID_Deer_Ground",
            "Camp_MID_Deer",
            "Camp_MID_Deer_Ground",
        ];
        let p = parts(&names);
        let shown = |s| arrange(Layout::Animated, 3, &p, s).parts.unwrap();
        assert_eq!(shown(LandmarkState::RawResource), [0, 1]);
        assert_eq!(shown(LandmarkState::Constructed), [0, 1, 6, 7]);
        assert_eq!(shown(LandmarkState::Pillaged), [0, 1, 2, 3]);
        assert_eq!(shown(LandmarkState::UnderConstruction), [0, 1, 4, 5]);
    }

    #[test]
    fn no_handler_turns_a_landmark() {
        let p = parts(&["model0", "model1", "model2"]);
        for layout in [
            Layout::Random,
            Layout::AnimatedRotated,
            Layout::Animated,
            Layout::Farm,
            Layout::Special,
        ] {
            for c in 0..50 {
                assert_eq!(arrange(layout, c, &p, LandmarkState::Constructed).yaw, 0.0);
            }
        }
        for layout in [Layout::Random, Layout::Farm, Layout::Special] {
            let a = arrange(layout, 41, &p, LandmarkState::Constructed);
            assert_eq!(a.parts.as_deref(), Some(&[0, 1, 2][..]));
            assert!(a.authored);
        }
    }
    #[test]
    fn a_snapshot_is_the_whole_model() {
        let p = parts(&["model0", "model1"]);
        assert_eq!(
            arrange(Layout::Snapshot, 9, &p, LandmarkState::Pillaged),
            Arrangement::WHOLE
        );
    }
}
