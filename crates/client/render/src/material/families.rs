//! The table of shader families: `MaterialDesc.package` (the `ShaderSet`
//! of the FGX block: `UnitShader`, `UnitShader_Skinned`, `BuildingShader`, `LandmarkShader_Stencil`
//! ...) to the family that draws it. A `world-*` module registers its family with **one row** here.
//!
//! A package that is not in the table is logged once and its calls are skipped: **no other family
//! is put in its place** (a wrong shader draws something that looks right and is not).

use std::collections::HashSet;
use std::sync::Mutex;

use super::desc::{MaterialDesc, PassKind};
use crate::shaders::Variant;

/// The entries and `override` values a family takes for a material in a pass.
#[derive(Clone, Debug, PartialEq)]
pub struct Selection {
    /// Entry point of the vertex stage, in the family's module (`unit_vs`).
    pub vs: &'static str,
    /// Entry point of the fragment stage (`unit_ps`).
    pub fs: &'static str,
    /// Values of the `override` constants of the module.
    pub variant: Variant,
}

/// A family of shaders of one package of the original.
#[derive(Clone, Copy)]
pub struct ShaderFamily {
    /// Name, for the log and the cache key.
    pub name: &'static str,
    /// Values of `MaterialDesc.package` the family draws.
    pub packages: &'static [&'static str],
    /// The module of `shaders::catalog()` with its entry points.
    pub module: &'static str,
    /// By the flags of the material and the kind of pass: the pair of entries and the `override`
    /// values, or `None` when the family does not draw such a material in such a pass (a family
    /// that has not ported its shadow entry says `None` for [`PassKind::Shadow`]).
    pub select: fn(&MaterialDesc, PassKind) -> Option<Selection>,
}

impl std::fmt::Debug for ShaderFamily {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShaderFamily")
            .field("name", &self.name)
            .field("packages", &self.packages)
            .field("module", &self.module)
            .finish()
    }
}

/// The families, one row each. `world-objects`, `world-terrain` and the others add theirs.
pub static FAMILIES: &[ShaderFamily] = &[
    // The pilot (RC-15): the rigid opaque `UnitVS` + `UnitPS`. The skinned, alpha, `_CODE` and
    // shadow entries are not ported yet, so those packages and passes are skipped.
    ShaderFamily {
        name: "UnitShader",
        packages: &["UnitShader"],
        module: "unit",
        select: unit_select,
    },
    // WOB1: the buildings of a city (rigid, opaque; the skinned and `_CODE` entries and the
    // shadow variants are not ported).
    ShaderFamily {
        name: "BuildingShader",
        packages: &["BuildingShader"],
        module: "building",
        select: building_select,
    },
    // WOB1: the trees. The pixel shader is the catalog's: the screen-space overlay in the main
    // pass, the reflection shader in the reflection pass (`ForestTopDownOverlayPS` is ported and
    // not selected: no pass of the plan asks for it).
    ShaderFamily {
        name: "ForestShader",
        packages: &["ForestShader"],
        module: "forest",
        select: forest_select,
    },
    // WAN3/WOB1: the skinned units, on the bone texture and the instance of `world-objects`.
    ShaderFamily {
        name: "UnitShader_Skinned",
        packages: &["UnitShader_Skinned"],
        module: "unit_skinned",
        select: unit_skinned_select,
    },
    // WAN3/WOB1: models lit by one cube (rigid and skinned).
    ShaderFamily {
        name: "SimpleShader",
        packages: &["SimpleShader", "SimpleShader_Skinned"],
        module: "simple",
        select: simple_select,
    },
    // WOB1: the effect models (city damage smoke and fire, pillage smoke, reef caustics), blended
    // by the premultiplied rule (`BlendMode::FxPremultiplied`).
    ShaderFamily {
        name: "FXShader",
        packages: &["FXShader", "FXShader_Skinned"],
        module: "fx",
        select: fx_select,
    },
    // WOV6: the models of the movement path (`Center*`, `Connector*`, `SelectedAni*`).
    ShaderFamily {
        name: "UIShader",
        packages: &["UIShader"],
        module: "ui",
        select: ui_select,
    },
    // WOB1: improvements, resources, features, ruins; the stencil package differs in its states.
    ShaderFamily {
        name: "LandmarkShader",
        packages: &["LandmarkShader", "LandmarkShader_Stencil"],
        module: "landmark",
        select: landmark_select,
    },
];

/// The pixel entry of the skinned unit (and of the rigid one on the instance of `world-objects`):
/// opaque or sorted by distance, and in the reflection pass the `*_reflect_ps` twin that cuts at the
/// water plane. The opaque one is the original's `UnitSkinnedPS_CODE` (`unit_code_ps`), what the
/// D3D11 build draws the units with in S1-S4; the sorted one stays `UnitSkinnedAlphaPS` (no call of
/// it or of its `_CODE` twin in the captures).
fn skinned_ps(desc: &MaterialDesc, pass: PassKind) -> &'static str {
    match (
        desc.has(MaterialDesc::ALPHA_SORT),
        pass == PassKind::Reflection,
    ) {
        (false, false) => UNIT_CODE_PS,
        (true, false) => "unit_skinned_alpha_ps",
        (false, true) => UNIT_CODE_REFLECT_PS,
        (true, true) => "unit_skinned_alpha_reflect_ps",
    }
}

/// The `_CODE` pixel entries of the units (`unit_code_world.wgsl`); their main-pass calls blend as
/// S1a does ([`crate::material::BlendMode::UnitCode`]).
pub const UNIT_CODE_PS: &str = "unit_code_ps";
pub const UNIT_CODE_REFLECT_PS: &str = "unit_code_reflect_ps";

fn unit_select(desc: &MaterialDesc, pass: PassKind) -> Option<Selection> {
    match pass {
        // A material sorted by distance is the transparent unit (`UnitAlphaPS`).
        PassKind::Main => Some(Selection {
            vs: "unit_vs",
            fs: if desc.has(MaterialDesc::ALPHA_SORT) {
                "unit_alpha_ps"
            } else {
                "unit_ps"
            },
            variant: Variant::new(),
        }),
        _ => None,
    }
}

/// The skinned unit: `UnitSkinnedVS` with `UnitSkinnedPS` (`UnitSkinnedAlphaPS` for a material
/// sorted by distance) in the main and the `ZFail` passes (the pass changes the state of the depth
/// test, not the shader), the shadow variant in the shadow map. The reflection pass is RW-14 (g): the
/// catalog has no reflection entry of the unit, so the main entries run with the cut at the water
/// plane (the `*_reflect_ps` twins), as the original's clip plane did.
fn unit_skinned_select(desc: &MaterialDesc, pass: PassKind) -> Option<Selection> {
    match pass {
        PassKind::Main | PassKind::ZFail | PassKind::Reflection => Some(Selection {
            vs: "unit_skinned_vs",
            fs: skinned_ps(desc, pass),
            variant: Variant::new(),
        }),
        // The caster `UnitSkinnedPS_CODE_Shadow` (S1a ev 313).
        PassKind::Shadow => Some(Selection {
            vs: "unit_skinned_shadow_vs",
            fs: "unit_code_shadow_ps",
            variant: Variant::new(),
        }),
        _ => None,
    }
}

/// `SimpleShader`: the skinned pair for a skinned material, the rigid one otherwise; the main pass.
fn simple_select(desc: &MaterialDesc, pass: PassKind) -> Option<Selection> {
    match pass {
        PassKind::Main | PassKind::Reflection => Some(if desc.has(MaterialDesc::SKINNED) {
            Selection {
                vs: "simple_skinned_vs",
                fs: "simple_skinned_ps",
                variant: Variant::new(),
            }
        } else {
            Selection {
                vs: "simple_vs",
                fs: "simple_ps",
                variant: Variant::new(),
            }
        }),
        _ => None,
    }
}

/// The most layers of an effect material the module draws (`FXPS_CODE1..3`; the original has
/// `FXPS_CODE4` and `FXPS_CODE5` too, not ported).
pub const FX_MAX_LAYERS: usize = 3;

/// The number of layers of an effect material: its `BaseTextureMapN` samplers that the FGX gives a
/// file, at least one and at most [`FX_MAX_LAYERS`].
///
/// Every FX material of the install declares all five `BaseTextureMap0..4` (with their scroll and
/// alpha maps) and gives paths only to the layers it uses; the others are empty (`Engine`). Counting
/// the empty ones picked `FXPS_CODE3` for one- and two-layer effects, whose third layer then read
/// the white the draw binds for a missing texture: an opaque white sheet (the pillaged smoke and
/// the city damage fire, user report fb3).
pub fn fx_layers(desc: &MaterialDesc) -> usize {
    let layers = desc
        .sets
        .iter()
        .flat_map(|s| &s.textures)
        .filter(|t| {
            !matches!(t.src, asset_format::TexSource::Engine)
                && t.name
                    .strip_prefix("BaseTextureMap")
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        })
        .count();
    layers.clamp(1, FX_MAX_LAYERS)
}

/// `FXShader`: the skinned or the rigid vertex shader with the pixel shader of as many layers as the
/// material has textures (`FXPS_CODE1..3`; the skinned pixel shaders of the original are the same code),
/// in the main and the reflection pass. The effects throw no shadow and have no z-fail pass.
/// The object draw reaches this through [`resolve_object`] (the generic [`resolve`] branch).
fn fx_select(desc: &MaterialDesc, pass: PassKind) -> Option<Selection> {
    match pass {
        PassKind::Main | PassKind::Reflection => {
            let skinned = desc.has(MaterialDesc::SKINNED) || desc.package == "FXShader_Skinned";
            Some(Selection {
                // SIMPLIFIED(SIMP-exe-1030): FXPS_CODE3 is the variant of the three layers (inferred).
                // The god ray reads its clock (the time since the highlight, unwrapped) from the instance.
                vs: if skinned {
                    "fx_skinned_vs"
                } else if crate::material::desc::is_god_ray(desc) {
                    "fx_god_ray_vs"
                } else {
                    "fx_vs"
                },
                fs: match fx_layers(desc) {
                    1 => "fx_ps_code1",
                    2 => "fx_ps_code2",
                    _ => "fx_ps_code3",
                },
                variant: Variant::new(),
            })
        }
        _ => None,
    }
}

/// `UIShader`: `UserInterfaceVS` with `UserInterfaceSkinnedPS`, in the main pass; the path throws no
/// shadow and has no reflection.
fn ui_select(_desc: &MaterialDesc, pass: PassKind) -> Option<Selection> {
    match pass {
        PassKind::Main => Some(Selection {
            vs: "ui_vs",
            fs: "ui_ps",
            variant: Variant::new(),
        }),
        _ => None,
    }
}

fn landmark_select(_desc: &MaterialDesc, pass: PassKind) -> Option<Selection> {
    match pass {
        // The original draws the landmarks in the reflection with the same entries; they stand on
        // the land, above the plane, and their pixel stage has no world position to cut at.
        PassKind::Main | PassKind::Reflection => Some(Selection {
            vs: "landmark_vs",
            fs: "landmark_ps",
            variant: Variant::new(),
        }),
        _ => None,
    }
}

fn building_select(_desc: &MaterialDesc, pass: PassKind) -> Option<Selection> {
    match pass {
        PassKind::Main => Some(Selection {
            vs: "building_vs",
            fs: "building_ps",
            variant: Variant::new(),
        }),
        // The same entry that also drops what lies below the water.
        PassKind::Reflection => Some(Selection {
            vs: "building_vs",
            fs: "building_reflect_ps",
            variant: Variant::new(),
        }),
        _ => None,
    }
}

fn forest_select(_desc: &MaterialDesc, pass: PassKind) -> Option<Selection> {
    match pass {
        PassKind::Main => Some(Selection {
            vs: "forest_vs",
            fs: "forest_ss_overlay_ps",
            variant: Variant::new(),
        }),
        PassKind::Reflection => Some(Selection {
            vs: "forest_vs",
            fs: "forest_ss_reflect_ps",
            variant: Variant::new(),
        }),
        _ => None,
    }
}

/// WOB2: the rigid `UnitShader` on the instance of `world-objects` (`ObjectInstance`), the vertex
/// stage `unit_object_vs` of the module that has the skinned units too, with their pixel shaders.
/// B1's `UnitShader` row above is the static model of B1 (`StaticInstance`); the two are never the
/// same draw, so this row is not in [`FAMILIES`] and only [`resolve_object`] finds it.
pub static UNIT_OBJECT: ShaderFamily = ShaderFamily {
    name: "UnitShader_Object",
    packages: &["UnitShader"],
    module: "unit_skinned",
    select: unit_object_select,
};

fn unit_object_select(desc: &MaterialDesc, pass: PassKind) -> Option<Selection> {
    match pass {
        PassKind::Main | PassKind::ZFail | PassKind::Reflection => Some(Selection {
            vs: "unit_object_vs",
            fs: skinned_ps(desc, pass),
            variant: Variant::new(),
        }),
        PassKind::Shadow => Some(Selection {
            vs: "unit_shadow_vs",
            fs: "unit_shadow_ps",
            variant: Variant::new(),
        }),
        _ => None,
    }
}

/// The `BuildingShader` and `BuildingShader_Skinned` meshes of the landmarks on the instance of
/// `world-objects` (the rocks of a deposit, the animals of a camp): B1's `BuildingShader` row is the
/// static model of a city, so this row is not in [`FAMILIES`] either. The meshes whose material has
/// `Shadows` cast into the shadow map (S1a: `BuildingVS_Instanced_Shadow`, `BuildingSkinnedVS_Shadow`).
pub static BUILDING_OBJECT: ShaderFamily = ShaderFamily {
    name: "BuildingShader_Object",
    packages: &["BuildingShader", "BuildingShader_Skinned"],
    module: "building_object",
    select: building_object_select,
};

fn building_object_select(desc: &MaterialDesc, pass: PassKind) -> Option<Selection> {
    let skinned = desc.package == "BuildingShader_Skinned";
    let vs = if skinned {
        "building_skinned_vs"
    } else {
        "building_object_vs"
    };
    match pass {
        PassKind::Shadow => Some(Selection {
            vs: if skinned {
                "building_skinned_shadow_vs"
            } else {
                "building_object_shadow_vs"
            },
            fs: "building_shadow_ps",
            variant: Variant::new(),
        }),
        PassKind::Main => Some(Selection {
            vs,
            fs: "building_object_ps",
            variant: Variant::new(),
        }),
        PassKind::Reflection => Some(Selection {
            vs,
            fs: "building_object_reflect_ps",
            variant: Variant::new(),
        }),
        _ => None,
    }
}

/// The entries of [`BUILDING_OBJECT`] for the buildings of the cities: the ported colour pass of the
/// D3D11 build instead of `BuildingPS`, `BuildingPS_CODE` for a material that blends and
/// `BuildingPS_CODE_Opaque` for the others (as the S3 draws pair them with the blend state); the
/// casters are the same.
pub fn building_code_select(desc: &MaterialDesc, pass: PassKind) -> Option<Selection> {
    let skinned = desc.package == "BuildingShader_Skinned";
    let vs = if skinned {
        "building_skinned_code_vs"
    } else {
        "building_object_code_vs"
    };
    let alpha = crate::material::blends(desc);
    match pass {
        PassKind::Main => Some(Selection {
            vs,
            fs: if alpha {
                "building_object_code_alpha_ps"
            } else {
                "building_object_code_ps"
            },
            variant: Variant::new(),
        }),
        PassKind::Reflection => Some(Selection {
            vs,
            fs: if alpha {
                "building_object_code_alpha_reflect_ps"
            } else {
                "building_object_code_reflect_ps"
            },
            variant: Variant::new(),
        }),
        _ => building_object_select(desc, pass),
    }
}

/// [`resolve`] for the draws of `world-objects` on the instance of the spec (WOB1/WOB2): the
/// materials of the original are the same, but a rigid `UnitShader` mesh goes to
/// `unit_object_vs` and not to B1's `unit_vs`, and a mesh sitting on a bone (`bind`) says so by the
/// `override`s `HAS_BIND` and `BIND_BONE` (WA-D8).
pub fn resolve_object(
    desc: &MaterialDesc,
    pass: PassKind,
    bind: Option<u16>,
) -> Result<(&'static ShaderFamily, Selection), Skip> {
    let rigid_unit = desc.package == "UnitShader" && !desc.has(MaterialDesc::SKINNED);
    let (family, mut selection) = if rigid_unit {
        match unit_object_select(desc, pass) {
            Some(s) => (&UNIT_OBJECT, s),
            None => {
                return Err(Skip::NotInPass {
                    family: UNIT_OBJECT.name,
                    pass,
                });
            }
        }
    } else if BUILDING_OBJECT.packages.contains(&desc.package.as_str()) {
        match building_object_select(desc, pass) {
            Some(s) => (&BUILDING_OBJECT, s),
            None => {
                return Err(Skip::NotInPass {
                    family: BUILDING_OBJECT.name,
                    pass,
                });
            }
        }
    } else if desc.package == "UnitShader" {
        // A skinned material of the plain package is the skinned unit.
        let skinned = MaterialDesc {
            package: "UnitShader_Skinned".to_owned(),
            ..desc.clone()
        };
        resolve(&skinned, pass)?
    } else {
        resolve(desc, pass)?
    };
    if let Some(bone) = bind {
        selection.variant = selection
            .variant
            .with_flag("HAS_BIND", true)
            .with("BIND_BONE", f64::from(bone));
    }
    Ok((family, selection))
}

/// Why a call is not drawn.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Skip {
    /// No family for the package: not ported yet, or the name is not one of the original.
    #[error("no shader family for the package `{0}`")]
    UnknownPackage(String),
    /// The family does not draw this material in this pass.
    #[error("the family `{family}` does not draw this material in the {pass:?} pass")]
    NotInPass {
        family: &'static str,
        pass: PassKind,
    },
    /// The family names an entry that its module does not have.
    #[error("the module `{module}` has no entry point `{entry}`")]
    MissingEntry {
        module: &'static str,
        entry: &'static str,
    },
    #[error("the layout of the vertices is unusable: {0}")]
    Layout(String),
    #[error("the pipeline could not be made: {0}")]
    Pipeline(String),
}

/// The family of `package` in `table`.
pub fn family_in<'a>(table: &'a [ShaderFamily], package: &str) -> Option<&'a ShaderFamily> {
    table.iter().find(|f| f.packages.contains(&package))
}

/// The family of `package` in [`FAMILIES`].
pub fn family_for(package: &str) -> Option<&'static ShaderFamily> {
    family_in(FAMILIES, package)
}

/// Packages already reported, so a model with a hundred instances writes one line.
static REPORTED: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Writes `why` to the log the first time it is seen for `key`.
pub(crate) fn report_once(key: &str, why: &Skip) {
    let mut guard = REPORTED.lock().unwrap_or_else(|e| e.into_inner());
    if guard
        .get_or_insert_with(HashSet::new)
        .insert(key.to_owned())
    {
        tracing::warn!("{why}; the call is skipped");
    }
}

/// The family and the entries for `desc` in `pass`, looked up in `table`. An unknown package is
/// logged once and is a [`Skip`].
pub fn resolve_in<'a>(
    table: &'a [ShaderFamily],
    desc: &MaterialDesc,
    pass: PassKind,
) -> Result<(&'a ShaderFamily, Selection), Skip> {
    let Some(family) = family_in(table, &desc.package) else {
        let why = Skip::UnknownPackage(desc.package.clone());
        report_once(&format!("package:{}", desc.package), &why);
        return Err(why);
    };
    match (family.select)(desc, pass) {
        Some(selection) => Ok((family, selection)),
        None => Err(Skip::NotInPass {
            family: family.name,
            pass,
        }),
    }
}

/// [`resolve_in`] over [`FAMILIES`].
pub fn resolve(
    desc: &MaterialDesc,
    pass: PassKind,
) -> Result<(&'static ShaderFamily, Selection), Skip> {
    resolve_in(FAMILIES, desc, pass)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe_family() -> ShaderFamily {
        fn select(d: &MaterialDesc, pass: PassKind) -> Option<Selection> {
            match pass {
                PassKind::Main => Some(Selection {
                    vs: "vs_fullscreen",
                    fs: "fs_override",
                    variant: Variant::new().with_flag("PROBE_ON", d.has(MaterialDesc::SKINNED)),
                }),
                _ => None,
            }
        }
        ShaderFamily {
            name: "probe",
            packages: &["ProbeShader", "ProbeShader_Skinned"],
            module: "common_probe",
            select,
        }
    }

    fn desc(package: &str, flags: u16) -> MaterialDesc {
        MaterialDesc {
            package: package.into(),
            flags,
            ..MaterialDesc::default()
        }
    }

    #[test]
    fn a_family_is_found_by_the_package_string() {
        let table = [probe_family()];
        assert_eq!(family_in(&table, "ProbeShader").unwrap().name, "probe");
        assert_eq!(
            family_in(&table, "ProbeShader_Skinned").unwrap().name,
            "probe"
        );
        assert!(
            family_in(&table, "probeshader").is_none(),
            "names are exact"
        );
        assert!(family_in(&table, "").is_none());
    }

    #[test]
    fn the_flags_and_the_pass_pick_the_entries_and_the_overrides() {
        let table = [probe_family()];
        let (f, s) = resolve_in(&table, &desc("ProbeShader", 0), PassKind::Main).unwrap();
        assert_eq!(
            (f.name, s.vs, s.fs),
            ("probe", "vs_fullscreen", "fs_override")
        );
        assert_eq!(s.variant.key(), "PROBE_ON=0");
        let (_, s) = resolve_in(
            &table,
            &desc("ProbeShader", MaterialDesc::SKINNED),
            PassKind::Main,
        )
        .unwrap();
        assert_eq!(s.variant.key(), "PROBE_ON=1");
        assert_eq!(
            resolve_in(&table, &desc("ProbeShader", 0), PassKind::Shadow).unwrap_err(),
            Skip::NotInPass {
                family: "probe",
                pass: PassKind::Shadow
            }
        );
    }

    /// WAN3/WOB1: the packages of the skinned units, the simple models and the landmarks have a
    /// family, and the pass and the flags pick the entries.
    #[test]
    fn the_skinned_unit_simple_and_landmark_families_pick_their_entries() {
        let pick = |package: &str, flags: u16, pass: PassKind| {
            resolve(&desc(package, flags), pass).map(|(f, s)| (f.name, f.module, s.vs, s.fs))
        };
        assert_eq!(
            pick("UnitShader_Skinned", MaterialDesc::SKINNED, PassKind::Main).unwrap(),
            (
                "UnitShader_Skinned",
                "unit_skinned",
                "unit_skinned_vs",
                "unit_code_ps"
            )
        );
        assert_eq!(
            pick(
                "UnitShader_Skinned",
                MaterialDesc::ALPHA_SORT,
                PassKind::ZFail
            )
            .unwrap()
            .3,
            "unit_skinned_alpha_ps"
        );
        let caster = pick("UnitShader_Skinned", 0, PassKind::Shadow).unwrap();
        assert_eq!(
            (caster.2, caster.3),
            ("unit_skinned_shadow_vs", "unit_code_shadow_ps")
        );
        // No reflection entry in the catalog: the main entries with the cut at the water plane.
        let reflection = pick("UnitShader_Skinned", 0, PassKind::Reflection).unwrap();
        assert_eq!(
            (reflection.2, reflection.3),
            ("unit_skinned_vs", "unit_code_reflect_ps")
        );
        assert_eq!(
            pick(
                "UnitShader_Skinned",
                MaterialDesc::ALPHA_SORT,
                PassKind::Reflection
            )
            .unwrap()
            .3,
            "unit_skinned_alpha_reflect_ps"
        );
        assert_eq!(
            pick("SimpleShader", 0, PassKind::Main).unwrap().2,
            "simple_vs"
        );
        assert_eq!(
            pick(
                "SimpleShader_Skinned",
                MaterialDesc::SKINNED,
                PassKind::Main
            )
            .unwrap()
            .3,
            "simple_skinned_ps"
        );
        for package in ["LandmarkShader", "LandmarkShader_Stencil"] {
            assert_eq!(
                pick(package, 0, PassKind::Main).unwrap(),
                ("LandmarkShader", "landmark", "landmark_vs", "landmark_ps")
            );
        }
        // Every entry a row names is in its module of the catalog.
        for family in FAMILIES {
            let module = crate::shaders::catalog()
                .iter()
                .find(|m| m.name == family.module)
                .unwrap_or_else(|| panic!("module {}", family.module));
            for flags in [
                0,
                MaterialDesc::SKINNED,
                MaterialDesc::ALPHA_SORT,
                MaterialDesc::SKINNED | MaterialDesc::ALPHA_SORT,
            ] {
                for pass in [
                    PassKind::Main,
                    PassKind::Shadow,
                    PassKind::Reflection,
                    PassKind::ZFail,
                    PassKind::Mask,
                ] {
                    let Some(s) = (family.select)(&desc(family.packages[0], flags), pass) else {
                        continue;
                    };
                    for entry in [s.vs, s.fs] {
                        assert!(
                            module.entries.iter().any(|e| e.name == entry),
                            "{} {pass:?}: no entry `{entry}` in `{}`",
                            family.name,
                            module.name
                        );
                    }
                }
            }
        }
    }

    /// WOB2: a rigid `UnitShader` mesh of `world-objects` takes the object entries, a skinned one
    /// the skinned entries, a mesh on a bone the `override`s of the bind; B1's table is as it was.
    #[test]
    fn the_object_table_picks_the_entries_of_the_instance_of_the_spec() {
        let pick = |package: &str, flags: u16, pass: PassKind, bind: Option<u16>| {
            resolve_object(&desc(package, flags), pass, bind)
                .map(|(f, s)| (f.name, f.module, s.vs, s.fs, s.variant.key()))
        };
        assert_eq!(
            pick("UnitShader", 0, PassKind::Main, None).unwrap(),
            (
                "UnitShader_Object",
                "unit_skinned",
                "unit_object_vs",
                "unit_code_ps",
                String::new()
            )
        );
        assert_eq!(
            pick(
                "UnitShader",
                MaterialDesc::ALPHA_SORT,
                PassKind::ZFail,
                None
            )
            .unwrap()
            .3,
            "unit_skinned_alpha_ps"
        );
        let shadow = pick("UnitShader", 0, PassKind::Shadow, None).unwrap();
        assert_eq!((shadow.2, shadow.3), ("unit_shadow_vs", "unit_shadow_ps"));
        let reflection = pick("UnitShader", 0, PassKind::Reflection, None).unwrap();
        assert_eq!(
            (reflection.2, reflection.3, reflection.4.as_str()),
            ("unit_object_vs", "unit_code_reflect_ps", "")
        );
        // A mesh on bone 7: the two constants, both in the key of the pipeline.
        let bound = pick("UnitShader", 0, PassKind::Main, Some(7)).unwrap();
        assert_eq!(bound.4, "BIND_BONE=7,HAS_BIND=1");
        // A skinned material of the package `UnitShader` is the skinned unit.
        assert_eq!(
            pick("UnitShader", MaterialDesc::SKINNED, PassKind::Main, None)
                .unwrap()
                .2,
            "unit_skinned_vs"
        );
        // The other packages are the rows of the table.
        assert_eq!(
            pick("UnitShader_Skinned", 0, PassKind::Shadow, None)
                .unwrap()
                .2,
            "unit_skinned_shadow_vs"
        );
        assert!(pick("#1a2b3c4d", 0, PassKind::Main, None).is_err());
        // B1's row for the same package is still the static model of B1.
        let (f, s) = resolve(&desc("UnitShader", 0), PassKind::Main).unwrap();
        assert_eq!((f.module, s.vs), ("unit", "unit_vs"));
        // Every entry the object row names is in the module.
        let module = crate::shaders::catalog()
            .iter()
            .find(|m| m.name == UNIT_OBJECT.module)
            .unwrap();
        for pass in [PassKind::Main, PassKind::ZFail, PassKind::Shadow] {
            let s = unit_object_select(&desc("UnitShader", 0), pass).unwrap();
            for entry in [s.vs, s.fs] {
                assert!(module.entries.iter().any(|e| e.name == entry), "{entry}");
            }
        }
    }

    /// The models of the movement path are `UIShader`: the main pass only (no shadow, no reflection),
    /// and the entries are in the module.
    #[test]
    fn the_uishader_package_of_the_path_models_has_a_row() {
        let d = desc("UIShader", 0);
        let (f, s) = resolve_object(&d, PassKind::Main, None).unwrap();
        assert_eq!((f.module, s.vs, s.fs), ("ui", "ui_vs", "ui_ps"));
        assert!(resolve_object(&d, PassKind::Shadow, None).is_err());
        assert!(resolve_object(&d, PassKind::Reflection, None).is_err());
        let module = crate::shaders::catalog()
            .iter()
            .find(|m| m.name == f.module)
            .unwrap();
        for entry in [s.vs, s.fs] {
            assert!(module.entries.iter().any(|e| e.name == entry), "{entry}");
        }
        // A mesh on a bone says so in the `override`s, as it does for the units.
        let (_, bound) = resolve_object(&d, PassKind::Main, Some(3)).unwrap();
        assert!(bound.variant != s.variant);
    }

    fn fx_desc(package: &str, base_layers: usize) -> MaterialDesc {
        use asset_format::{ParamSet, TexKind, TexParam, TexSource};
        let param = |name: String, file: bool| TexParam {
            src: if file {
                TexSource::File(asset_format::LogicalId::new(format!("tx:{name}.dds")))
            } else {
                TexSource::Engine
            },
            name,
            kind: TexKind::Tex2D,
            semantic: 0,
            reg: 0,
            usage: 0,
            flags: 0,
            active: true,
        };
        // As the install's materials: all five layers declared, paths only on the used ones.
        let mut textures: Vec<TexParam> = (0..base_layers.max(5))
            .flat_map(|n| {
                let file = n < base_layers;
                [
                    param(format!("BaseTextureMap{n}"), file),
                    param(format!("UVScrollingMap{n}"), file),
                    param(format!("AlphaLookupMap{n}"), file),
                ]
            })
            .collect();
        textures.push(param("Unrelated".into(), true));
        MaterialDesc {
            package: package.into(),
            sets: vec![ParamSet {
                textures,
                ..ParamSet::default()
            }],
            ..MaterialDesc::default()
        }
    }

    /// The effect models: the layers of the material pick the pixel entry, the package the vertex
    /// entry; the main and the reflection pass draw, the shadow and the z-fail pass do not.
    #[test]
    fn an_effect_material_picks_the_pixel_entry_by_its_layers() {
        let (family, s) = resolve_object(&fx_desc("FXShader", 2), PassKind::Main, None).unwrap();
        assert_eq!((family.name, family.module), ("FXShader", "fx"));
        assert_eq!((s.vs, s.fs), ("fx_vs", "fx_ps_code2"));
        for pass in [PassKind::Shadow, PassKind::ZFail, PassKind::Mask] {
            assert!(
                resolve_object(&fx_desc("FXShader", 2), pass, None).is_err(),
                "{pass:?}"
            );
        }
        let skinned = fx_desc("FXShader_Skinned", 1);
        let (_, s) = resolve_object(&skinned, PassKind::Reflection, None).unwrap();
        assert_eq!((s.vs, s.fs), ("fx_skinned_vs", "fx_ps_code1"));
        // More layers than the module has take the highest it has; none take the first.
        assert_eq!(fx_layers(&fx_desc("FXShader", 5)), FX_MAX_LAYERS);
        assert_eq!(fx_layers(&fx_desc("FXShader", 0)), 1);
        let (_, s) = resolve_object(&fx_desc("FXShader", 3), PassKind::Main, None).unwrap();
        assert_eq!(s.fs, "fx_ps_code3");
        // Every entry the row names is in the module.
        let module = crate::shaders::catalog()
            .iter()
            .find(|m| m.name == "fx")
            .unwrap();
        for n in 1..=3 {
            for package in ["FXShader", "FXShader_Skinned"] {
                let (_, s) = resolve_object(&fx_desc(package, n), PassKind::Main, None).unwrap();
                for entry in [s.vs, s.fs] {
                    assert!(module.entries.iter().any(|e| e.name == entry), "{entry}");
                }
            }
        }
    }

    #[test]
    fn the_building_materials_of_a_landmark_go_to_the_object_entries() {
        let rigid = desc("BuildingShader", 0);
        let (family, s) = resolve_object(&rigid, PassKind::Main, None).unwrap();
        assert_eq!(family.name, "BuildingShader_Object");
        assert_eq!((s.vs, s.fs), ("building_object_vs", "building_object_ps"));
        let skinned = desc("BuildingShader_Skinned", MaterialDesc::SKINNED);
        let (_, s) = resolve_object(&skinned, PassKind::Reflection, None).unwrap();
        assert_eq!(
            (s.vs, s.fs),
            ("building_skinned_vs", "building_object_reflect_ps")
        );
        // The casters of the shadow map.
        let (_, s) = resolve_object(&rigid, PassKind::Shadow, None).unwrap();
        assert_eq!(
            (s.vs, s.fs),
            ("building_object_shadow_vs", "building_shadow_ps")
        );
        let (_, s) = resolve_object(&skinned, PassKind::Shadow, None).unwrap();
        assert_eq!(s.vs, "building_skinned_shadow_vs");
        // B1's city keeps its own row.
        assert_eq!(
            resolve(&rigid, PassKind::Main).unwrap().0.name,
            "BuildingShader"
        );
    }

    #[test]
    fn an_unknown_package_is_skipped_and_nothing_stands_in() {
        let table = [probe_family()];
        for package in ["UnitShader", "#1a2b3c4d", ""] {
            let err = resolve_in(&table, &desc(package, 0), PassKind::Main).unwrap_err();
            assert_eq!(err, Skip::UnknownPackage(package.into()));
        }
        // Packages that are not ported yet are unknown to the real table too, with no panic.
        for package in ["FXShader_Unknown", "BuildingShader_Skinned", "#00000000"] {
            assert!(family_for(package).is_none(), "{package}");
            let d = desc(package, 0);
            assert!(resolve(&d, PassKind::Main).is_err());
        }
    }
}
