//! The table of highlight calls (world-overlays 4.4, WOV5, RW-20): what `client-app` switches on
//! where the Lua UI of the original does. Each row names the style of `Highlights.xml`, the colour
//! of the call and the place in the Lua of Lekmod (`Assets/DLC/LEKMOD_v34.15/Lua/UI/`, read only);
//! lines are checked against the files of the install.
//!
//! The ring of the selected unit is not a call: the original has no style for it (`InGame.lua`'s
//! `OnUnitHexHighlight` only asks for yields), it is `UnitSelectionHighlight` of the overlay art
//! defines, which the renderer draws (`render::world::overlay::ring`).
//!
//! The colour of a call is the one of the Lua; a `SplineBorder` style takes its own (the comment at
//! `Bombardment.lua:6`; the exact rule is RW-20 (a), `render::world::overlay::highlights::resolve_color`).

use render::entities::Highlights;
use render::world::overlay::highlights::{HighlightCell, HighlightSet};

/// One row of the table.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Call {
    pub situation: &'static str,
    /// The style, by its name in `Highlights.xml`.
    pub style: &'static str,
    /// RGBA, 0 to 1, as the Lua writes it (`Vector4`).
    pub color: [f32; 4],
    /// The Lua of Lekmod: file and lines of the colour and of the call.
    pub lua: &'static str,
}

/// The cells a unit reaches in this turn.
// re: WorldView.lua:72 (`turn1Color` = 0, 1, 0, 0.25), :76 (style `MovementRangeBorder`), :936 (call
// in `OnAddUnitMoveHexRangeHex`, :932-938); cleared at :920-930
pub const MOVE_RANGE: Call = Call {
    situation: "selected unit: cells it reaches this turn",
    style: "MovementRangeBorder",
    color: [0.0, 1.0, 0.0, 0.25],
    lua: "WorldView.lua:72, :76, :936 (clear :920-930)",
};

/// A cell with a visible enemy the move tree ends on (a move-attack).
// re: WorldView.lua:77 (style `AMRBorder`), :934 (call, inside `OnAddUnitMoveHexRangeHex`, :932-938)
pub const ATTACK_MOVE: Call = Call {
    situation: "selected unit: cell with a visible enemy it can attack by moving",
    style: "AMRBorder",
    color: [0.0, 1.0, 0.0, 0.25],
    lua: "WorldView.lua:72, :77, :934 (clear :920-930)",
};

/// The radius of a ranged attack of a unit or a city.
// re: Bombardment.lua:6 (`highlightColor` = 0.7, 0.7, 0, 1), :75 and :78 (style `FireRangeBorder`)
pub const FIRE_RANGE: Call = Call {
    situation: "ranged-attack mode of a unit or a city: the radius",
    style: "FireRangeBorder",
    color: [0.7, 0.7, 0.0, 1.0],
    lua: "Bombardment.lua:6, :75, :78",
};

/// The targets inside the radius.
// re: Bombardment.lua:5 (`redColor` = 0.7, 0, 0, 1), :76 (style `ValidFireTargetBorder`)
pub const FIRE_TARGET: Call = Call {
    situation: "ranged-attack mode: the targets in the radius",
    style: "ValidFireTargetBorder",
    color: [0.7, 0.0, 0.0, 1.0],
    lua: "Bombardment.lua:5, :76",
};

/// The radius of an air unit.
// re: Bombardment.lua:122-123 and :126-127 (`AirRangeFill`, `AirRangeBorder`, `highlightColor`);
// :124 (`ValidFireTargetBorder`); :170-171 is the same pair in another air mode
pub const AIR_RANGE: [Call; 2] = [
    Call {
        situation: "air unit: the radius (fill)",
        style: "AirRangeFill",
        color: [0.7, 0.7, 0.0, 1.0],
        lua: "Bombardment.lua:122-123, :170",
    },
    Call {
        situation: "air unit: the radius (border)",
        style: "AirRangeBorder",
        color: [0.7, 0.7, 0.0, 1.0],
        lua: "Bombardment.lua:126-127, :171",
    },
];

/// The sweep (interception) of an air unit.
// re: Bombardment.lua:344-345 (`AirSweepFill`, `AirSweepBorder`, literal 0.7, 0.7, 0, 1); cleared at
// :358-359
pub const AIR_SWEEP: [Call; 2] = [
    Call {
        situation: "air unit: the sweep (fill)",
        style: "AirSweepFill",
        color: [0.7, 0.7, 0.0, 1.0],
        lua: "Bombardment.lua:344, :358",
    },
    Call {
        situation: "air unit: the sweep (border)",
        style: "AirSweepBorder",
        color: [0.7, 0.7, 0.0, 1.0],
        lua: "Bombardment.lua:345, :359",
    },
];

/// The modes of the interface that mark cells with `GUHB` (`InGame.lua:29` names the style).
// re: InGame.lua:467 (gift units, 1, 1, 0, 1), :508 (`embarkColor` 1, 1, 0, 0.65), :534
// (`upgradeColor` 0, 1, 0.5, 0.65), :730 (`vGiftTileImprovementColor` 1, 0, 1, 1), :753
// (`vPossibleTradeRouteColor` 1, 0, 1, 1); cleared at :545 and :554
pub const MODES: [Call; 5] = [
    Call {
        situation: "gift of a unit to a city-state: the plots",
        style: "GUHB",
        color: [1.0, 1.0, 0.0, 1.0],
        lua: "InGame.lua:467",
    },
    Call {
        situation: "embark: the plots",
        style: "GUHB",
        color: [1.0, 1.0, 0.0, 0.65],
        lua: "InGame.lua:508",
    },
    Call {
        situation: "upgrade: the plots",
        style: "GUHB",
        color: [0.0, 1.0, 0.5, 0.65],
        lua: "InGame.lua:534",
    },
    Call {
        situation: "gift of an improvement to a city-state: the plots",
        style: "GUHB",
        color: [1.0, 0.0, 1.0, 1.0],
        lua: "InGame.lua:730",
    },
    Call {
        situation: "possible trade route: the cities",
        style: "GUHB",
        color: [1.0, 0.0, 1.0, 1.0],
        lua: "InGame.lua:753",
    },
];

/// Not switched on in B2 (WV-D11): the city window and the banner.
// re: CityView.lua:1371-1461 (`WorkedFill`, `OverlapFill`, `VacantFill`, `EnemyFill`, `BuyFill`,
// `CityLimits`, `Culture`), CityBannerManager.lua:1205-1236
pub const NOT_IN_B2: &str = "CityView.lua:1371-1555, CityBannerManager.lua:1205-1236";

/// What the controller shows: the state the calls follow.
pub struct CallInput<'a> {
    /// The cells of the move tree reached this turn (`turn` 0).
    pub reachable: &'a [(i32, i32)],
    /// The cells to attack: with an enemy on them (the tree's `ATTACK` nodes and the hovered enemy),
    /// or, in the ranged-attack mode, the targets in the radius.
    pub targets: &'a [(i32, i32)],
    /// The ranged-attack mode is on.
    pub ranged: bool,
    /// The cells of the radius of the mode, if the context gives them.
    pub radius: &'a [(i32, i32)],
}

fn cells(out: &mut Vec<HighlightCell>, call: &Call, cells: &[(i32, i32)]) {
    out.extend(cells.iter().map(|&(x, y)| HighlightCell {
        style: call.style.to_string(),
        x,
        y,
        color: Some(call.color),
    }));
}

/// The set of styles for the state: nothing selected gives the empty set (every style is cleared, as
/// `ClearHexHighlightStyle` does when the selection goes, `WorldView.lua:920-930`).
pub fn highlight_set(input: &CallInput<'_>) -> HighlightSet {
    let mut out = Vec::new();
    if input.ranged {
        cells(&mut out, &FIRE_RANGE, input.radius);
        cells(&mut out, &FIRE_TARGET, input.targets);
    } else {
        cells(&mut out, &MOVE_RANGE, input.reachable);
        cells(&mut out, &ATTACK_MOVE, input.targets);
    }
    HighlightSet { cells: out }
}

/// The set for B1's `Highlights` (what the controller computes) and the mode it is in.
pub fn from_highlights(h: &Highlights, ranged: bool) -> HighlightSet {
    highlight_set(&CallInput {
        reachable: &h.reachable,
        targets: &h.targets,
        ranged,
        radius: &[],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn styles(set: &HighlightSet) -> Vec<(&str, i32, i32)> {
        set.cells
            .iter()
            .map(|c| (c.style.as_str(), c.x, c.y))
            .collect()
    }

    #[test]
    fn a_selected_unit_gets_the_range_and_the_attack_cells() {
        let set = highlight_set(&CallInput {
            reachable: &[(9, 11), (9, 12)],
            targets: &[(9, 10)],
            ranged: false,
            radius: &[],
        });
        assert_eq!(
            styles(&set),
            [
                ("MovementRangeBorder", 9, 11),
                ("MovementRangeBorder", 9, 12),
                ("AMRBorder", 9, 10)
            ]
        );
        assert_eq!(set.cells[0].color, Some([0.0, 1.0, 0.0, 0.25]));
    }

    #[test]
    fn the_ranged_mode_shows_the_radius_and_the_targets_and_not_the_reach() {
        let set = highlight_set(&CallInput {
            reachable: &[(9, 11)],
            targets: &[(10, 11)],
            ranged: true,
            radius: &[(9, 11), (10, 11), (11, 11)],
        });
        assert_eq!(
            styles(&set),
            [
                ("FireRangeBorder", 9, 11),
                ("FireRangeBorder", 10, 11),
                ("FireRangeBorder", 11, 11),
                ("ValidFireTargetBorder", 10, 11)
            ]
        );
        assert_eq!(set.cells[3].color, Some([0.7, 0.0, 0.0, 1.0]));
    }

    #[test]
    fn nothing_selected_clears_every_style() {
        let set = highlight_set(&CallInput {
            reachable: &[],
            targets: &[],
            ranged: false,
            radius: &[],
        });
        assert!(set.is_empty());
    }

    #[test]
    fn every_row_names_its_lua_and_a_style_of_the_lekmod_file() {
        let rows: Vec<&Call> = [&MOVE_RANGE, &ATTACK_MOVE, &FIRE_RANGE, &FIRE_TARGET]
            .into_iter()
            .chain(AIR_RANGE.iter())
            .chain(AIR_SWEEP.iter())
            .chain(MODES.iter())
            .collect();
        for r in rows {
            assert!(
                r.lua.contains(".lua:"),
                "{}: a file and a line",
                r.situation
            );
            assert!(r.color.iter().all(|c| (0.0..=1.0).contains(c)));
            assert!(!r.style.is_empty());
        }
    }
}
