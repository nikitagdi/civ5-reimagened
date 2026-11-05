//! The smoke over a pillaged improvement (world-objects WOB4, task R5.13): a model effect, not a
//! Fork particle.
//!
//! re: the `.ftsxml` triggers of the pillaged landmark models (98 files of the install, found by
//! the name) make `ART_DEF_VEFFECT_PILLAGED_SMOKE_01` (the mills, castles, polders...) or one of
//! `PASTURE_PILLAGED_SMOKE_01..03` (the animals: 01 cow, horse, sheep, truffles; 02 bison, deer;
//! 03 furs, ivory) at the bone `FX_PillagedSmoke` of the model, from the start of the idle clip.
//! The visual effects (`civ5artdefines_viseffects_shared.xml`, which overrides the base file) name
//! the model effects `PILLAGED_SMOKE_NEW_2` and `PILLAGED_FARM_SMOKE_NEW_2`, the files
//! `FXM_Pillaged_Smoke_02` and `FXM_Pillaged_Farm_Smoke_02`; the base file's `_01` models are the
//! old look. No trigger names `FARM_PILLAGED_SMOKE_01` or `OIL_RIG_PILLAGED_SMOKE_01` (the farm's
//! has the sound `AS2D_INTERFACE_PILLAGE`): code plays those, not read here.
//!
//! STAND-IN (not a SIMP): the trigger data is not in the model blobs (`ModelBlobV2::effects` is
//! reserved), so the effect is not at the bone of the pillaged part but at the middle of the plot,
//! with the transform of its visual effect (translation turned from the original's basis into
//! ours, scale), chosen by the improvement of the plot: farm -> `FARM_PILLAGED_SMOKE_01`, pasture ->
//! `PASTURE_PILLAGED_SMOKE_01..03` by the resource, offshore platform -> `OIL_RIG_PILLAGED_SMOKE_01`,
//! camp -> the same by the animal (the deer, bison, furs, ivory and truffles models of the camp rows
//! carry the pasture triggers), any other improvement -> `PILLAGED_SMOKE_01`. The bone belongs to the
//! model of the improvement's row, so the effect takes that row's `Scale` on top of its own (the
//! pasture `3.7` is meant for a cow model drawn at `0.08`: `0.3` in the world, as the other smokes;
//! without it the pasture smoke stood twelve times too large, user report fb4). Like the damage
//! models of cities it shows on visible plots only.

use glam::{Mat4, Vec2, Vec3};

use crate::entities::PlotDecor;

/// The slot (`Placed::slot`) of the smoke among the landmarks of a plot; the rows take 0 and up.
pub const SMOKE_SLOT: u16 = 200;

/// `ART_DEF_MEFFECT_PILLAGED_SMOKE_NEW_2` (`FXM_Pillaged_Smoke_02`).
pub const SMOKE_MODEL: &str = "md:fxm_pillaged_smoke_02";
/// `ART_DEF_MEFFECT_PILLAGED_FARM_SMOKE_NEW_2` (`FXM_Pillaged_Farm_Smoke_02`).
pub const FARM_SMOKE_MODEL: &str = "md:fxm_pillaged_farm_smoke_02";

/// One visual effect of the pillaged smoke: its model and its `Transform` (the original's basis).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Smoke {
    pub veffect: &'static str,
    pub model: &'static str,
    pub translation: [f32; 3],
    pub scale: f32,
}

const fn smoke(
    veffect: &'static str,
    model: &'static str,
    translation: [f32; 3],
    scale: f32,
) -> Smoke {
    Smoke {
        veffect,
        model,
        translation,
        scale,
    }
}

const AT: [f32; 3] = [0.0, -0.4, 0.3];

pub const PILLAGED: Smoke = smoke("ART_DEF_VEFFECT_PILLAGED_SMOKE_01", SMOKE_MODEL, AT, 0.3);
pub const PASTURE_MD: Smoke = smoke(
    "ART_DEF_VEFFECT_PASTURE_PILLAGED_SMOKE_01",
    SMOKE_MODEL,
    AT,
    3.7,
);
pub const PASTURE_SM: Smoke = smoke(
    "ART_DEF_VEFFECT_PASTURE_PILLAGED_SMOKE_02",
    SMOKE_MODEL,
    AT,
    2.35,
);
pub const PASTURE_LG: Smoke = smoke(
    "ART_DEF_VEFFECT_PASTURE_PILLAGED_SMOKE_03",
    SMOKE_MODEL,
    AT,
    4.0,
);
pub const FARM: Smoke = smoke(
    "ART_DEF_VEFFECT_FARM_PILLAGED_SMOKE_01",
    FARM_SMOKE_MODEL,
    AT,
    0.28,
);
pub const OIL_RIG: Smoke = smoke(
    "ART_DEF_VEFFECT_OIL_RIG_PILLAGED_SMOKE_01",
    SMOKE_MODEL,
    [-14.0, 10.4, 0.3],
    0.3,
);

/// Every effect of the smoke, for the models to ask for.
pub const ALL: [Smoke; 6] = [PILLAGED, PASTURE_MD, PASTURE_SM, PASTURE_LG, FARM, OIL_RIG];

/// The smoke a plot shows: `None` unless its improvement is pillaged.
pub fn smoke_of(plot: &PlotDecor) -> Option<Smoke> {
    let improvement = plot.improvement.as_deref()?;
    if !plot.pillaged || plot.special {
        return None;
    }
    let has = |s: &str| improvement.contains(s);
    Some(if has("_FARM") {
        FARM
    } else if has("_PASTURE") || improvement == "ART_DEF_IMPROVEMENT_CAMP" {
        pasture(plot.resource.as_deref().unwrap_or(""))
    } else if has("OFFSHORE_PLATFORM") {
        OIL_RIG
    } else {
        PILLAGED
    })
}

/// The pasture smoke by the animal: the size of the herd's model (the triggers of the install).
fn pasture(resource: &str) -> Smoke {
    let has = |s: &str| resource.contains(s);
    if has("BISON") || has("DEER") {
        PASTURE_SM
    } else if has("FUR") || has("IVORY") {
        PASTURE_LG
    } else {
        PASTURE_MD
    }
}

/// The matrix of the smoke over the plot whose middle is `centre` (before the height of the
/// surface): the scale `host` of the model whose bone carries the effect (its row's `Scale`), then
/// the translation of the effect, turned into our basis (x, y up, z south), and its scale.
pub fn matrix(centre: Vec2, host: f32, s: &Smoke) -> Mat4 {
    let [tx, ty, tz] = s.translation;
    Mat4::from_translation(Vec3::new(centre.x, 0.0, centre.y))
        * Mat4::from_scale(Vec3::splat(host))
        * Mat4::from_translation(Vec3::new(tx, tz, -ty))
        * Mat4::from_scale(Vec3::splat(s.scale))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plot(improvement: Option<&str>, resource: Option<&str>, pillaged: bool) -> PlotDecor {
        PlotDecor {
            improvement: improvement.map(str::to_owned),
            resource: resource.map(str::to_owned),
            pillaged,
            ..PlotDecor::default()
        }
    }

    #[test]
    fn only_a_pillaged_improvement_smokes() {
        let farm = Some("ART_DEF_IMPROVEMENT_FARM");
        assert_eq!(smoke_of(&plot(farm, None, false)), None);
        assert_eq!(
            smoke_of(&plot(None, None, true)),
            None,
            "nothing to pillage"
        );
        let mut camp = plot(Some("ART_DEF_IMPROVEMENT_BARBARIAN_CAMP"), None, true);
        camp.special = true;
        assert_eq!(smoke_of(&camp), None);
    }

    #[test]
    fn the_improvement_chooses_the_effect() {
        let of = |imp: &str, res: Option<&str>| smoke_of(&plot(Some(imp), res, true)).unwrap();
        assert_eq!(of("ART_DEF_IMPROVEMENT_FARM", None), FARM);
        assert_eq!(of("ART_DEF_IMPROVEMENT_OFFSHORE_PLATFORM", None), OIL_RIG);
        assert_eq!(of("ART_DEF_IMPROVEMENT_MINE", None), PILLAGED);
        assert_eq!(of("ART_DEF_IMPROVEMENT_TRADING_POST", None), PILLAGED);
        assert_eq!(
            of("ART_DEF_IMPROVEMENT_CAMP", Some("ART_DEF_RESOURCE_DEER")),
            PASTURE_SM
        );
        assert_eq!(
            of("ART_DEF_IMPROVEMENT_CAMP", Some("ART_DEF_RESOURCE_IVORY")),
            PASTURE_LG
        );
        assert_eq!(of("ART_DEF_IMPROVEMENT_BRAZILWOOD_CAMP", None), PILLAGED);
        let pasture = "ART_DEF_IMPROVEMENT_PASTURE";
        assert_eq!(of(pasture, Some("ART_DEF_RESOURCE_COW")), PASTURE_MD);
        assert_eq!(of(pasture, Some("ART_DEF_RESOURCE_BISON")), PASTURE_SM);
        assert_eq!(of(pasture, Some("ART_DEF_RESOURCE_FUR")), PASTURE_LG);
        assert_eq!(of(pasture, None), PASTURE_MD);
        // Two models only; the farm has its own.
        assert_eq!(of("ART_DEF_IMPROVEMENT_FARM", None).model, FARM_SMOKE_MODEL);
        assert!(ALL.iter().filter(|s| s.model == FARM_SMOKE_MODEL).count() == 1);
    }

    #[test]
    fn the_smoke_stands_on_the_plot_in_our_basis() {
        let m = matrix(Vec2::new(100.0, 200.0), 1.0, &PILLAGED);
        let at = m.transform_point3(Vec3::ZERO);
        // The translation (0, -0.4, 0.3) of the original: 0.3 up, 0.4 south.
        assert!((at - Vec3::new(100.0, 0.3, 200.4)).length() < 1e-4, "{at}");
        assert!((m.transform_vector3(Vec3::X).length() - 0.3).abs() < 1e-5);
        // The oil rig stands 14 west and 10.4 north of the middle.
        let rig = matrix(Vec2::ZERO, 1.0, &OIL_RIG).transform_point3(Vec3::ZERO);
        assert!(
            (rig - Vec3::new(-14.0, 0.3, -10.4)).length() < 1e-4,
            "{rig}"
        );
    }

    #[test]
    fn the_pasture_smoke_takes_the_scale_of_the_animal() {
        // The cow of a pasture stands at 0.08: the smoke of its bone is as large as the others.
        let cow = matrix(Vec2::ZERO, 0.08, &PASTURE_MD)
            .transform_vector3(Vec3::X)
            .length();
        let mine = matrix(Vec2::ZERO, 1.0, &PILLAGED)
            .transform_vector3(Vec3::X)
            .length();
        assert!((cow - 0.296).abs() < 1e-4, "{cow}");
        assert!((cow / mine - 1.0).abs() < 0.02);
        // The deer of a camp (0.12) and the elephant (0.07) too.
        for (host, s) in [(0.12, PASTURE_SM), (0.07, PASTURE_LG)] {
            let k = matrix(Vec2::ZERO, host, &s)
                .transform_vector3(Vec3::X)
                .length();
            assert!((0.25..0.33).contains(&k), "{k}");
        }
    }
}
