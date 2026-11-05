//! `shaders::catalog()`: the list of WGSL modules. Read by `shader-re`
//! (check with naga, GLSL ES for WebGL2, comparison with the oracle) and by `golden` (replay).
//!
//! The file uses only `std` and `include_str!` paths relative to itself, so `shader-re` includes
//! it with `#[path]` and needs no `wgpu`. A module is built by joining its parts (files) with a
//! newline, in the order given: the same text as `wgsl_module!("a.wgsl", "b.wgsl")`.

/// A file of a module.
#[derive(Clone, Copy, Debug)]
pub struct Part {
    /// Path under `shaders/`.
    pub path: &'static str,
    pub text: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Vertex,
    Fragment,
}

/// The entry of the original a function was taken from: `// re: <Package>::<Entry> (sha1 ...)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReRef {
    pub input: &'static str,
    /// First 12 characters of the sha1 of the SM3 block.
    pub sha1: &'static str,
}

/// An entry point of a module.
#[derive(Clone, Copy, Debug)]
pub struct EntryDesc {
    pub name: &'static str,
    pub stage: Stage,
    /// The entry of the original it ports, if it does.
    pub re: Option<ReRef>,
}

/// A module: parts in the order they are joined, and its entry points.
#[derive(Clone, Copy, Debug)]
pub struct ModuleDesc {
    pub name: &'static str,
    /// The shader family it belongs to (`common` for the shared files).
    pub family: &'static str,
    pub parts: &'static [Part],
    pub entries: &'static [EntryDesc],
}

impl ModuleDesc {
    /// The WGSL text of the module.
    pub fn source(&self) -> String {
        let mut text = String::new();
        for part in self.parts {
            text.push_str(part.text);
            text.push('\n');
        }
        text
    }
}

macro_rules! part {
    ($path:literal) => {
        Part {
            path: $path,
            text: include_str!($path),
        }
    };
}

const fn entry(name: &'static str, stage: Stage) -> EntryDesc {
    EntryDesc {
        name,
        stage,
        re: None,
    }
}

/// The shared files with `common/probe.wgsl`, whose entry points use them: what `shader-re` can
/// translate for WebGL2, and what the GPU test of `sm3.wgsl` runs.
const COMMON_PROBE: ModuleDesc = ModuleDesc {
    name: "common_probe",
    family: "common",
    parts: &[
        part!("common/frame.wgsl"),
        part!("common/sm3.wgsl"),
        part!("common/gamma.wgsl"),
        part!("common/fog.wgsl"),
        part!("common/lut.wgsl"),
        part!("common/shadow.wgsl"),
        part!("common/skin.wgsl"),
        part!("common/probe.wgsl"),
    ],
    entries: &[
        entry("vs_fullscreen", Stage::Vertex),
        entry("fs_sm3_a", Stage::Fragment),
        entry("fs_sm3_b", Stage::Fragment),
        entry("fs_sm3_c", Stage::Fragment),
        entry("fs_gamma", Stage::Fragment),
        entry("fs_override", Stage::Fragment),
    ],
};

/// The fragment of the class mask pass (`frame::mask`): a pipeline per class through the
/// `MASK_CLASS` override, with the vertex stage of the subsystem that draws.
const MASK: ModuleDesc = ModuleDesc {
    name: "mask",
    family: "common",
    parts: &[part!("common/mask.wgsl")],
    entries: &[entry("fs_mask", Stage::Fragment)],
};

/// `UnitShader::UnitVS` and `UnitPS` (rigid, opaque): the pilot port (RC-15). The parts are the
/// ones `shader-re compare` needs as `--module` arguments, in this order.
const UNIT: ModuleDesc = ModuleDesc {
    name: "unit",
    family: "UnitShader",
    parts: &[
        part!("common/frame.wgsl"),
        part!("common/sm3.wgsl"),
        part!("unit/unit.wgsl"),
    ],
    entries: &[
        EntryDesc {
            name: "unit_vs",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "UnitShader::UnitVS",
                sha1: "ade84e670527",
            }),
        },
        EntryDesc {
            name: "unit_ps",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "UnitShader::UnitPS",
                sha1: "cb0f37374433",
            }),
        },
        EntryDesc {
            name: "unit_alpha_ps",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "UnitShader::UnitAlphaPS",
                sha1: "7f4640bb3ad1",
            }),
        },
    ],
};

/// `BuildingShader::BuildingVS` and `BuildingPS` (WOB1): the city of B1 drawn with the shaders of
/// the original. `BuildingVS_Instanced` is DX11 only; `building_vs` takes its matrix from the
/// instance and so stands for both.
const BUILDING: ModuleDesc = ModuleDesc {
    name: "building",
    family: "BuildingShader",
    parts: &[
        part!("common/frame.wgsl"),
        part!("common/sm3.wgsl"),
        part!("building/building.wgsl"),
    ],
    entries: &[
        EntryDesc {
            name: "building_vs",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "BuildingShader::BuildingVS",
                sha1: "ade9d3190110",
            }),
        },
        EntryDesc {
            name: "building_ps",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "BuildingShader::BuildingPS",
                sha1: "18d02f5dc05f",
            }),
        },
        // Ours: `building_ps` that also drops what lies below the water (the reflection pass; the
        // original cut with `SetClipPlane`).
        entry("building_reflect_ps", Stage::Fragment),
    ],
};

/// `ForestShader::ForestVS` and the three pixel shaders (WOB1). Ported; drawn by B1's static
/// pipeline until the inputs of the main variants exist (see `forest.wgsl`).
const FOREST: ModuleDesc = ModuleDesc {
    name: "forest",
    family: "ForestShader",
    parts: &[
        part!("common/frame.wgsl"),
        part!("common/sm3.wgsl"),
        part!("forest/forest.wgsl"),
    ],
    entries: &[
        EntryDesc {
            name: "forest_vs",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "ForestShader::ForestVS",
                sha1: "6bf5913abad8",
            }),
        },
        EntryDesc {
            name: "forest_ss_overlay_ps",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "ForestShader::ForestSSOverlayPS",
                sha1: "f4440041ff99",
            }),
        },
        EntryDesc {
            name: "forest_top_down_overlay_ps",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "ForestShader::ForestTopDownOverlayPS",
                sha1: "c34fbed1d748",
            }),
        },
        EntryDesc {
            name: "forest_ss_reflect_ps",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "ForestShader::ForestSSReflectPS",
                sha1: "cabe64e8906c",
            }),
        },
        // The class mask of `golden` (render-core 4.14): not an entry of the original.
        EntryDesc {
            name: "forest_mask_ps",
            stage: Stage::Fragment,
            re: None,
        },
    ],
};

/// `Shadow::BlurAndProcess` and `Shadow::VSFallThrough` (RC-11): self-contained, so `shader-re
/// compare` takes the one file.
const SHADOW_BLUR: ModuleDesc = ModuleDesc {
    name: "shadow_blur",
    family: "Shadow",
    parts: &[part!("shadow/blur.wgsl")],
    entries: &[
        EntryDesc {
            name: "blur_vs",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "Shadow::VSFallThrough",
                sha1: "6c7b9ddc9db1",
            }),
        },
        EntryDesc {
            name: "blur_ps",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "Shadow::BlurAndProcess",
                sha1: "47a739394e40",
            }),
        },
    ],
};

/// The casters into the raw shadow map and the `?debug=shadow` view: ours, not a port.
const SHADOW_CASTER: ModuleDesc = ModuleDesc {
    name: "shadow_caster",
    family: "Shadow",
    parts: &[part!("shadow/caster.wgsl")],
    entries: &[
        entry("caster_vs", Stage::Vertex),
        entry("caster_ps", Stage::Fragment),
        entry("debug_vs", Stage::Vertex),
        entry("debug_ps", Stage::Fragment),
    ],
};

/// The ground of the milestone M1 (WT-0): the light of the original on the geometry of B1. Its
/// formulas are those of `TERRAIN_BAKE_CACHE` and `TERRAIN_SCREEN_PS` (`terrain/common.wgsl`); it
/// is ours, not a port of one entry.
const TERRAIN_M1: ModuleDesc = ModuleDesc {
    name: "terrain_m1",
    family: "TerrainShader",
    parts: &[
        part!("common/frame.wgsl"),
        part!("common/sm3.wgsl"),
        part!("common/gamma.wgsl"),
        part!("terrain/common.wgsl"),
        part!("terrain/m1_forward.wgsl"),
    ],
    entries: &[
        entry("terrain_m1_vs", Stage::Vertex),
        entry("terrain_m1_ps", Stage::Fragment),
    ],
};

/// "Terrain Cache" (WT-0, WT-3): `TerrainRenderCacheDiffuse` and `TerrainRenderCacheDiffuseAndSpec`.
/// The parts are the ones `shader-re compare` needs as `--module` arguments, in this order.
const TERRAIN_BAKE_CACHE: ModuleDesc = ModuleDesc {
    name: "terrain_bake_cache",
    family: "TerrainShader",
    parts: &[
        part!("common/frame.wgsl"),
        part!("common/sm3.wgsl"),
        part!("terrain/common.wgsl"),
        part!("terrain/bake_cache.wgsl"),
    ],
    entries: &[
        EntryDesc {
            name: "terrain_render_vs_screen_space",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "TerrainShader::TerrainRenderVSScreenSpace",
                sha1: "1a614daba14b",
            }),
        },
        EntryDesc {
            name: "terrain_cache_diffuse_ps",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "TerrainShader::TerrainRenderCacheDiffuse",
                sha1: "4849b2744930",
            }),
        },
        EntryDesc {
            name: "terrain_cache_diffuse_and_spec_ps",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "TerrainShader::TerrainRenderCacheDiffuseAndSpec",
                sha1: "5ac206dc4a8a",
            }),
        },
    ],
};

/// The cached page on the screen (WT-0, WT-4): `TerrainRenderCachedDiffusePS` and
/// `TerrainRenderCachedDiffuseAndSpecPS`.
const TERRAIN_SCREEN_PS: ModuleDesc = ModuleDesc {
    name: "terrain_screen_ps",
    family: "TerrainShader",
    parts: &[
        part!("common/frame.wgsl"),
        part!("common/sm3.wgsl"),
        part!("terrain/common.wgsl"),
        part!("terrain/page_2d.wgsl"),
        part!("terrain/screen_ps.wgsl"),
    ],
    entries: &[
        EntryDesc {
            name: "terrain_render_cached_diffuse_ps",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "TerrainShader::TerrainRenderCachedDiffusePS",
                sha1: "c2a0240dd9c9",
            }),
        },
        EntryDesc {
            name: "terrain_render_cached_diffuse_and_spec_ps",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "TerrainShader::TerrainRenderCachedDiffuseAndSpecPS",
                sha1: "edd7096eb6ab",
            }),
        },
    ],
};

/// "HeightMap" (WT-3): the composite of eight textures, `TerrainShaderScreenPS`.
const TERRAIN_BAKE_COMPOSITE: ModuleDesc = ModuleDesc {
    name: "terrain_bake_composite",
    family: "TerrainShader",
    parts: &[
        part!("common/sm3.wgsl"),
        part!("terrain/bake_composite.wgsl"),
    ],
    entries: &[
        // Compared with the slots (v4, v5) limited to 0..=95 (status.toml input_ranges).
        EntryDesc {
            name: "terrain_shader_screen_vs",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "TerrainShader::TerrainShaderScreenVS",
                sha1: "acc72e60604d",
            }),
        },
        EntryDesc {
            name: "terrain_shader_screen_ps",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "TerrainShader::TerrainShaderScreenPS",
                sha1: "63a33c1f1bee",
            }),
        },
        // The highlight composite: a reconstruction, no bytecode to compare with (SIMP-063).
        EntryDesc {
            name: "terrain_spec_composite_ps",
            stage: Stage::Fragment,
            re: None,
        },
    ],
};

/// "Macro Shadow" (WT-8): `TerrainShaderScreenShadowVS` and `TerrainShaderShadowPS` of `TerrainShader`.
const TERRAIN_BAKE_SHADOW: ModuleDesc = ModuleDesc {
    name: "terrain_bake_shadow",
    family: "TerrainShader",
    parts: &[part!("common/sm3.wgsl"), part!("terrain/bake_shadow.wgsl")],
    entries: &[
        EntryDesc {
            name: "terrain_shader_screen_shadow_vs",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "TerrainShader::TerrainShaderScreenShadowVS",
                sha1: "69b5ac81f756",
            }),
        },
        EntryDesc {
            name: "terrain_shader_shadow_ps",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "TerrainShader::TerrainShaderShadowPS",
                sha1: "be270504c723",
            }),
        },
    ],
};

/// "Decal Layer" (WT-6): `DecalVSCompressed`, `DecalPS` and `DecalPSSpec` of `TerrainShader`.
const TERRAIN_DECAL: ModuleDesc = ModuleDesc {
    name: "terrain_decal",
    family: "TerrainShader",
    parts: &[part!("common/sm3.wgsl"), part!("terrain/decal.wgsl")],
    entries: &[
        EntryDesc {
            name: "decal_vs_compressed",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "TerrainShader::DecalVSCompressed",
                sha1: "1f02afbecc20",
            }),
        },
        EntryDesc {
            name: "decal_ps",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "TerrainShader::DecalPS",
                sha1: "e3625359ea30",
            }),
        },
        EntryDesc {
            name: "decal_ps_spec",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "TerrainShader::DecalPSSpec",
                sha1: "7d8988c5b07a",
            }),
        },
        // `DecalPS` into the one target of the macro shadow; not a shader of the original.
        entry("decal_ps_shadow", Stage::Fragment),
    ],
};

/// "Normal Calculation" (WT-3): `MakeNormalPS` and `VSFallThrough` of `TerrainShader`.
const TERRAIN_BAKE_NORMAL: ModuleDesc = ModuleDesc {
    name: "terrain_bake_normal",
    family: "TerrainShader",
    parts: &[part!("common/sm3.wgsl"), part!("terrain/bake_normal.wgsl")],
    entries: &[
        EntryDesc {
            name: "terrain_vs_fall_through",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "TerrainShader::VSFallThrough",
                sha1: "ac60879a1630",
            }),
        },
        EntryDesc {
            name: "make_normal_ps",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "TerrainShader::MakeNormalPS",
                sha1: "e7aef5ac5ab8",
            }),
        },
    ],
};

/// The batched cells on the screen (render-opt B3): `TERRAIN_CELL` with the pages read from their
/// arrays, the layers given by the instance, so the cells of an array share one draw. The bodies are
/// those of `TERRAIN_CELL`; only the sampling of the page is `page_array.wgsl`.
const TERRAIN_CELL_ARRAY: ModuleDesc = ModuleDesc {
    name: "terrain_cell_array",
    family: "TerrainShader",
    parts: &[
        part!("common/frame.wgsl"),
        part!("common/sm3.wgsl"),
        part!("terrain/common.wgsl"),
        part!("terrain/cell_vs.wgsl"),
        part!("terrain/page_array.wgsl"),
        part!("terrain/screen_ps.wgsl"),
        part!("terrain/cell_array.wgsl"),
    ],
    // The entries of the parts it shares with `TERRAIN_CELL` are in the text too (unused here; they
    // read the arrays at layer 0).
    entries: &[
        entry("terrain_shader_vs", Stage::Vertex),
        entry("terrain_render_cached_diffuse_ps", Stage::Fragment),
        entry("terrain_render_cached_diffuse_and_spec_ps", Stage::Fragment),
        entry("terrain_shader_vs_array", Stage::Vertex),
        entry("terrain_render_cached_diffuse_array_ps", Stage::Fragment),
        entry(
            "terrain_render_cached_diffuse_and_spec_array_ps",
            Stage::Fragment,
        ),
    ],
};

/// The cells on the screen (WT-4): `TerrainShaderVS` and the pixel stage of the cached page, the two
/// stages of one pipeline. The entries of the pixel stage are compared in `TERRAIN_SCREEN_PS`.
const TERRAIN_CELL: ModuleDesc = ModuleDesc {
    name: "terrain_cell",
    family: "TerrainShader",
    parts: &[
        part!("common/frame.wgsl"),
        part!("common/sm3.wgsl"),
        part!("terrain/common.wgsl"),
        part!("terrain/cell_vs.wgsl"),
        part!("terrain/page_2d.wgsl"),
        part!("terrain/screen_ps.wgsl"),
    ],
    entries: &[
        EntryDesc {
            name: "terrain_shader_vs",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "TerrainShader::TerrainShaderVS",
                sha1: "dba830761636",
            }),
        },
        entry("terrain_render_cached_diffuse_ps", Stage::Fragment),
        entry("terrain_render_cached_diffuse_and_spec_ps", Stage::Fragment),
    ],
};

/// The fog mask (`FOWDrawMask`, world-overlays WOV1): `VSFowMask` and the pixel shaders of its three
/// classes. `SolidPS` has the block of `PSFog` (the same sha1).
const FOW_MASK: ModuleDesc = ModuleDesc {
    name: "fow_mask",
    family: "FOW",
    parts: &[part!("fow/mask.wgsl")],
    entries: &[
        EntryDesc {
            name: "fowmask_vs",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "FOW::VSFowMask",
                sha1: "293adcac99b0",
            }),
        },
        EntryDesc {
            name: "visible_ps",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "FOW::Visible",
                sha1: "d6a30d350665",
            }),
        },
        EntryDesc {
            name: "visiblefluid_ps",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "FOW::VisibleFluid",
                sha1: "01dd2a488404",
            }),
        },
        EntryDesc {
            name: "transparent_ps",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "FOW::Transparent",
                sha1: "80f4f25bccf7",
            }),
        },
        EntryDesc {
            name: "solid_ps",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "FOW::SolidPS",
                sha1: "9502fba404b8",
            }),
        },
    ],
};

/// The cull layer of the fluid fog (`FOWDrawMask` subset 3, WOV1): kept as the reference of the port, the
/// product culls on the CPU.
const FOW_CULL: ModuleDesc = ModuleDesc {
    name: "fow_cull",
    family: "FOW",
    parts: &[part!("fow/cull.wgsl")],
    entries: &[
        EntryDesc {
            name: "fowmaskcull_vs",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "FOW::VSFowMaskCull",
                sha1: "fba35bfdeb15",
            }),
        },
        EntryDesc {
            name: "fowmaskcullshaded_vs",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "FOW::VSFowMaskCullShaded",
                sha1: "effd8bc5e162",
            }),
        },
        EntryDesc {
            name: "cullmask_ps",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "FOW::CullMask",
                sha1: "27f861de2216",
            }),
        },
        EntryDesc {
            name: "cullmaskshaded_ps",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "FOW::CullMaskShaded",
                sha1: "c1be76d4ab6a",
            }),
        },
    ],
};

/// The cloud layers of the fluid fog (`FOWBaseLayer`, WOV1).
const FOW_BASE: ModuleDesc = ModuleDesc {
    name: "fow_base",
    family: "FOW",
    parts: &[part!("fow/base.wgsl")],
    entries: &[
        EntryDesc {
            name: "fogbaselayer_vs",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "FOW::VSFogBaseLayer",
                sha1: "b6f3d5ec8647",
            }),
        },
        EntryDesc {
            name: "baseps",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "FOW::BasePS",
                sha1: "311bec4adaae",
            }),
        },
    ],
};

/// The cloud layers in the class mask pass: the vertex stage of `FOW_BASE` and a fragment that
/// writes the class bit where the clouds show. Not a port.
const FOW_BASE_MASK: ModuleDesc = ModuleDesc {
    name: "fow_base_mask",
    family: "FOW",
    parts: &[part!("fow/base.wgsl"), part!("fow/base_mask.wgsl")],
    entries: &[
        entry("fogbaselayer_vs", Stage::Vertex),
        entry("baseps", Stage::Fragment),
        entry("fogbase_mask_ps", Stage::Fragment),
    ],
};

/// The classic fog of war (`FOWLevel` 0, WOV1) and the textured parts through the mask.
const FOW_CLASSIC: ModuleDesc = ModuleDesc {
    name: "fow_classic",
    family: "FOW",
    parts: &[part!("fow/classic.wgsl")],
    entries: &[
        EntryDesc {
            name: "vsfog",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "FOW::VSFog",
                sha1: "6b6746c33bc2",
            }),
        },
        EntryDesc {
            name: "vsfogtrans",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "FOW::VSFogTrans",
                sha1: "e7e765d7ac4d",
            }),
        },
        EntryDesc {
            name: "psfog",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "FOW::PSFog",
                sha1: "9502fba404b8",
            }),
        },
        EntryDesc {
            name: "psfogtrans",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "FOW::PSFogTrans",
                sha1: "7a85d061c75b",
            }),
        },
        EntryDesc {
            name: "renderscenevs",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "FOW::RenderSceneVS",
                sha1: "5feb3e3a1f60",
            }),
        },
        EntryDesc {
            name: "renderscenetexonlyvs",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "FOW::RenderSceneTexOnlyVS",
                sha1: "26f222494d7a",
            }),
        },
        EntryDesc {
            name: "textureps",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "FOW::TexturePS",
                sha1: "55f399129a11",
            }),
        },
    ],
};

/// The hex grid of the overlay map (`OverlayShaders::VSOverlay`, `PSSimpleTexture`, WOV2) and the
/// grid's own pixel shader that applies the colour of `HexGrid.xml`.
const OVERLAY_GRID: ModuleDesc = ModuleDesc {
    name: "overlay_grid",
    family: "OverlayShaders",
    parts: &[part!("overlay/overlay.wgsl")],
    entries: &[
        EntryDesc {
            name: "vsoverlay",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "OverlayShaders::VSOverlay",
                sha1: "9ffd8f9118e1",
            }),
        },
        EntryDesc {
            name: "pssimpletexture",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "OverlayShaders::PSSimpleTexture",
                sha1: "05af36e52ea2",
            }),
        },
        entry("grid_ps", Stage::Fragment),
    ],
};

/// The borders of the civilizations (`OverlayShaders::VSOverlayCountry`, `PSCountryBorder`, WOV4).
const OVERLAY_COUNTRY: ModuleDesc = ModuleDesc {
    name: "overlay_country",
    family: "OverlayShaders",
    parts: &[part!("overlay/country.wgsl")],
    entries: &[
        EntryDesc {
            name: "vsoverlaycountry",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "OverlayShaders::VSOverlayCountry",
                sha1: "b567dfe098c1",
            }),
        },
        EntryDesc {
            name: "pscountryborder",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "OverlayShaders::PSCountryBorder",
                sha1: "14813ec24959",
            }),
        },
    ],
};

/// The pixel shader of the selection ring (`OverlayShaders::PSValidMoves`, WOV5).
const OVERLAY_RING: ModuleDesc = ModuleDesc {
    name: "overlay_ring",
    family: "OverlayShaders",
    parts: &[part!("overlay/ring.wgsl")],
    entries: &[EntryDesc {
        name: "psvalidmoves",
        stage: Stage::Fragment,
        re: Some(ReRef {
            input: "OverlayShaders::PSValidMoves",
            sha1: "ba0d489b9bba",
        }),
    }],
};

/// The pixel shaders of the highlights (`OverlayShaders::PSContour`, `PSNoTexture`, `PSOverlay`, WOV5).
const OVERLAY_HIGHLIGHT: ModuleDesc = ModuleDesc {
    name: "overlay_highlight",
    family: "OverlayShaders",
    parts: &[part!("overlay/highlight.wgsl")],
    entries: &[
        EntryDesc {
            name: "pscontour",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "OverlayShaders::PSContour",
                sha1: "310753c72601",
            }),
        },
        EntryDesc {
            name: "psnotexture",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "OverlayShaders::PSNoTexture",
                sha1: "3e793bd16091",
            }),
        },
        EntryDesc {
            name: "psoverlay",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "OverlayShaders::PSOverlay",
                sha1: "f07d61f7ae4c",
            }),
        },
    ],
};

/// `UIShader`, the part the models of the movement path use (WOV6): the rigid vertex stage and the
/// pixel stage `UserInterfaceSkinnedPS` (the code of `UserInterfacePS` too).
const UI: ModuleDesc = ModuleDesc {
    name: "ui",
    family: "UIShader",
    parts: &[
        part!("common/frame.wgsl"),
        part!("common/sm3.wgsl"),
        part!("common/skin.wgsl"),
        part!("skin/common.wgsl"),
        part!("unit/ui.wgsl"),
    ],
    entries: &[
        EntryDesc {
            name: "ui_vs",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "UIShader::UserInterfaceVS",
                sha1: "218379a532f3",
            }),
        },
        EntryDesc {
            name: "ui_ps",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "UIShader::UserInterfaceSkinnedPS",
                sha1: "fad4a8f01fa0",
            }),
        },
    ],
};

/// The cells of the ground into the screen overlay (`TerrainShader::ScreenSpaceOverlayPS`, WOV2).
const OVERLAY_SCREEN_SPACE: ModuleDesc = ModuleDesc {
    name: "overlay_screen_space",
    family: "TerrainShader",
    parts: &[
        part!("overlay/screen_space_ps.wgsl"),
        part!("overlay/screen_space.wgsl"),
    ],
    entries: &[
        entry("cell_vs", Stage::Vertex),
        EntryDesc {
            name: "screenspaceoverlayps",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "TerrainShader::ScreenSpaceOverlayPS",
                sha1: "bb683c6e6475",
            }),
        },
    ],
};

/// The same pass through the cells of `world-terrain`: its vertex stage (`TerrainShaderVS`, compared
/// in `TERRAIN_CELL`) and the body of `ScreenSpaceOverlayPS` (compared above) behind a pixel entry
/// that finds the place in the map of the overlay from the point in the world.
const OVERLAY_CELLS: ModuleDesc = ModuleDesc {
    name: "overlay_cells",
    family: "TerrainShader",
    parts: &[
        part!("common/sm3.wgsl"),
        part!("terrain/cell_vs.wgsl"),
        part!("overlay/screen_space_ps.wgsl"),
        part!("overlay/cells_ps.wgsl"),
    ],
    entries: &[
        entry("terrain_shader_vs", Stage::Vertex),
        entry("screenspaceoverlay_cells_ps", Stage::Fragment),
    ],
};

const fn minimap_entry(
    name: &'static str,
    stage: Stage,
    input: &'static str,
    sha1: &'static str,
) -> EntryDesc {
    EntryDesc {
        name,
        stage,
        re: Some(ReRef { input, sha1 }),
    }
}

/// The minimap (`MiniMapShader`, WOV3): the hex meshes, the quads, the sprites and the camera
/// rectangle.
const MINIMAP: ModuleDesc = ModuleDesc {
    name: "minimap",
    family: "MinimapShader",
    parts: &[part!("common/sm3.wgsl"), part!("minimap/minimap.wgsl")],
    entries: &[
        minimap_entry(
            "vs_minimap",
            Stage::Vertex,
            "MinimapShader::VS_MiniMap",
            "5ab35345d840",
        ),
        minimap_entry(
            "vs_minimap_ocean",
            Stage::Vertex,
            "MinimapShader::VS_MiniMap_Ocean",
            "7d947db45f69",
        ),
        minimap_entry(
            "vs_minimap_ice",
            Stage::Vertex,
            "MinimapShader::VS_MinimapIce",
            "7c4bdb5f4daa",
        ),
        minimap_entry(
            "vs_sprite",
            Stage::Vertex,
            "MinimapShader::VS_Sprite",
            "297c4c83b0b9",
        ),
        minimap_entry(
            "vs_camerarect",
            Stage::Vertex,
            "MinimapShader::VS_CameraRect",
            "af0956fb930e",
        ),
        minimap_entry(
            "vs_debugcopy",
            Stage::Vertex,
            "MinimapShader::VS_DebugCopy",
            "cb7757777d2d",
        ),
        minimap_entry(
            "ps_land",
            Stage::Fragment,
            "MinimapShader::PS_Land",
            "d825f819faab",
        ),
        minimap_entry(
            "ps_ocean",
            Stage::Fragment,
            "MinimapShader::PS_Ocean",
            "1a692b822f64",
        ),
        minimap_entry(
            "ps_shallows",
            Stage::Fragment,
            "MinimapShader::PS_Shallows",
            "3ce6f50e745b",
        ),
        minimap_entry(
            "ps_river",
            Stage::Fragment,
            "MinimapShader::PS_River",
            "3ce6f50e745b",
        ),
        minimap_entry(
            "ps_minimapice",
            Stage::Fragment,
            "MinimapShader::PS_MinimapIce",
            "c37494803c08",
        ),
        minimap_entry(
            "ps_culture",
            Stage::Fragment,
            "MinimapShader::PS_Culture",
            "d4f816e6f6cd",
        ),
        minimap_entry(
            "ps_fog",
            Stage::Fragment,
            "MinimapShader::PS_Fog",
            "a6928b0d83b1",
        ),
        minimap_entry(
            "ps_dot",
            Stage::Fragment,
            "MinimapShader::PS_Dot",
            "fe62fa2c22eb",
        ),
        minimap_entry(
            "ps_texturedsprite",
            Stage::Fragment,
            "MinimapShader::PS_TexturedSprite",
            "1f7b7614fda5",
        ),
        minimap_entry(
            "ps_camerarect",
            Stage::Fragment,
            "MinimapShader::PS_CameraRect",
            "060e65dd7ae1",
        ),
        minimap_entry(
            "ps_debugcopy",
            Stage::Fragment,
            "MinimapShader::PS_DebugCopy",
            "fec77ddfb281",
        ),
    ],
};

/// The ocean and the lakes of the water (`WaterShader`, world-water WW-1): `VSWaterOcean` and the
/// three pixel shaders by `TerrainWaterQuality`.
const WATER_OCEAN: ModuleDesc = ModuleDesc {
    name: "water_ocean",
    family: "WaterShader",
    parts: &[part!("common/sm3.wgsl"), part!("water/ocean.wgsl")],
    entries: &[
        EntryDesc {
            name: "vswaterocean",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "WaterShader::VSWaterOcean",
                sha1: "8c7df6f4f161",
            }),
        },
        EntryDesc {
            name: "psoceanhigh",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "WaterShader::PSOceanHigh",
                sha1: "9b0080e09057",
            }),
        },
        EntryDesc {
            name: "psoceanmedium",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "WaterShader::PSOceanMedium",
                sha1: "19ce65aeff0e",
            }),
        },
        EntryDesc {
            name: "psoceanlow",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "WaterShader::PSOceanLow",
                sha1: "e0c64489fa3d",
            }),
        },
    ],
};

/// The rivers of the water (`WaterShader`, world-water WW-1): `VSWaterRiver` and the two pixel
/// shaders by `TerrainWaterQuality`, and the river banners of `RiverShader` (`VSRiver`, `PSRiver`)
/// that live in the same file.
const WATER_RIVER: ModuleDesc = ModuleDesc {
    name: "water_river",
    family: "WaterShader",
    parts: &[part!("common/sm3.wgsl"), part!("water/river.wgsl")],
    entries: &[
        EntryDesc {
            name: "vswaterriver",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "WaterShader::VSWaterRiver",
                sha1: "9964ed9b8cec",
            }),
        },
        EntryDesc {
            name: "psriverhigh",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "WaterShader::PSRiverHigh",
                sha1: "f781f5a8efb5",
            }),
        },
        EntryDesc {
            name: "psriverlow",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "WaterShader::PSRiverLow",
                sha1: "1ca0a6959556",
            }),
        },
        EntryDesc {
            name: "vsriver",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "RiverShader::VSRiver",
                sha1: "37eeafee9116",
            }),
        },
        EntryDesc {
            name: "psriver",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "RiverShader::PSRiver",
                sha1: "cb53c618b5be",
            }),
        },
    ],
};

/// The map of the ocean currents (`WorldCurrents`, world-water WW-2): the five entries; the
/// product draws with `vsfallthrough` and `pscombined`.
const WATER_CURRENTS: ModuleDesc = ModuleDesc {
    name: "water_currents",
    family: "WorldCurrents",
    parts: &[part!("common/sm3.wgsl"), part!("water/currents.wgsl")],
    entries: &[
        EntryDesc {
            name: "vsfallthrough",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "WorldCurrents::VSFallThrough",
                sha1: "6c7b9ddc9db1",
            }),
        },
        EntryDesc {
            name: "psrivercurrent",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "WorldCurrents::PSRiverCurrent",
                sha1: "f29747ef33b9",
            }),
        },
        EntryDesc {
            name: "pscurrent",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "WorldCurrents::PSCurrent",
                sha1: "492e367d06ef",
            }),
        },
        EntryDesc {
            name: "pscurlnoise",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "WorldCurrents::PSCurlNoise",
                sha1: "0932b5de0669",
            }),
        },
        EntryDesc {
            name: "pscombined",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "WorldCurrents::PSCombined",
                sha1: "3b7cd0b362fe",
            }),
        },
    ],
};

/// The skinned entries of `UnitShader` (WAN3, WOB1): `UnitSkinnedVS` and its shadow variant read the
/// bone texture; `UnitSkinnedPS` and `UnitSkinnedAlphaPS` are the code of `UnitPS` and
/// `UnitAlphaPS`. The parts are the ones `shader-re compare` needs as `--module` arguments, in this
/// order.
const UNIT_SKINNED: ModuleDesc = ModuleDesc {
    name: "unit_skinned",
    family: "UnitShader_Skinned",
    parts: &[
        part!("common/frame.wgsl"),
        part!("common/sm3.wgsl"),
        part!("common/skin.wgsl"),
        part!("skin/common.wgsl"),
        part!("unit/unit.wgsl"),
        part!("unit/unit_object.wgsl"),
        part!("skin/unit_skinned.wgsl"),
        part!("unit/unit_code.wgsl"),
        part!("unit/unit_code_world.wgsl"),
    ],
    entries: &[
        EntryDesc {
            name: "unit_skinned_vs",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "UnitShader::UnitSkinnedVS",
                sha1: "d2b2d4ddbb1a",
            }),
        },
        EntryDesc {
            name: "unit_skinned_shadow_vs",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "UnitShader::UnitSkinnedVS_Shadow",
                sha1: "6585865a75c4",
            }),
        },
        // The entries of `unit.wgsl` (B1's layout; ported in the module `unit`), which this module
        // includes for the bodies of `UnitPS` and `UnitAlphaPS`.
        entry("unit_vs", Stage::Vertex),
        entry("unit_ps", Stage::Fragment),
        entry("unit_alpha_ps", Stage::Fragment),
        // The rigid `UnitVS` on the instance of `world-objects` (the port is `unit.wgsl`'s).
        entry("unit_object_vs", Stage::Vertex),
        EntryDesc {
            name: "unit_shadow_vs",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "UnitShader::UnitVS_Shadow",
                sha1: "5a5d130dd35f",
            }),
        },
        // Ours: the mark of a caster in the raw shadow map.
        entry("unit_shadow_ps", Stage::Fragment),
        EntryDesc {
            name: "unit_skinned_ps",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "UnitShader::UnitSkinnedPS",
                sha1: "2f3587c3459d",
            }),
        },
        EntryDesc {
            name: "unit_skinned_alpha_ps",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "UnitShader::UnitSkinnedAlphaPS",
                sha1: "7f4640bb3ad1",
            }),
        },
        // Ours: the two pixel entries above that also drop what lies below the water (the
        // reflection pass; the catalog has no reflection entry of the units, RW-14 (g)).
        entry("unit_skinned_reflect_ps", Stage::Fragment),
        entry("unit_skinned_alpha_reflect_ps", Stage::Fragment),
        // The entries of `unit_code.wgsl` (ported in the module `unit_code`), which this module
        // includes for the bodies of the `_CODE` stages.
        entry("unit_skinned_ps_code", Stage::Fragment),
        entry("unit_skinned_shadow_ps_code", Stage::Fragment),
        // Ours: the `_CODE` stages on the instance of `world-objects` (`unit_code_world.wgsl`),
        // what the skinned units draw with in the main, z-fail, reflection and shadow passes.
        entry("unit_code_ps", Stage::Fragment),
        entry("unit_code_reflect_ps", Stage::Fragment),
        entry("unit_code_shadow_ps", Stage::Fragment),
    ],
};

/// `SimpleShader` (WAN3, WOB1): the skinned vertex shader (four bones a vertex, four registers a
/// bone), the rigid pair, and the pixel shader of the skinned one.
const SIMPLE: ModuleDesc = ModuleDesc {
    name: "simple",
    family: "SimpleShader",
    parts: &[
        part!("common/frame.wgsl"),
        part!("common/sm3.wgsl"),
        part!("common/skin.wgsl"),
        part!("skin/common.wgsl"),
        part!("skin/simple_skinned.wgsl"),
        part!("unit/simple.wgsl"),
    ],
    entries: &[
        EntryDesc {
            name: "simple_skinned_vs",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "SimpleShader::Skinned_SimpleVS_Skinned",
                sha1: "11b175a2ecd4",
            }),
        },
        EntryDesc {
            name: "simple_vs",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "SimpleShader::SimpleVS",
                sha1: "2171825d3f57",
            }),
        },
        EntryDesc {
            name: "simple_ps",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "SimpleShader::SimplePS",
                sha1: "b0d8891bd192",
            }),
        },
        EntryDesc {
            name: "simple_skinned_ps",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "SimpleShader::Skinned_SimplePS_Skinned",
                sha1: "7fbf881d9eca",
            }),
        },
    ],
};

/// `LandmarkShader` (WOB1): improvements, resources, features, ruins; the animated ones sit on a
/// bone of the instance.
const LANDMARK: ModuleDesc = ModuleDesc {
    name: "landmark",
    family: "LandmarkShader",
    parts: &[
        part!("common/frame.wgsl"),
        part!("common/skin.wgsl"),
        part!("skin/common.wgsl"),
        part!("unit/landmark.wgsl"),
    ],
    entries: &[
        EntryDesc {
            name: "landmark_vs",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "LandmarkShader::LandmarkVS",
                sha1: "373d86bbea7d",
            }),
        },
        EntryDesc {
            name: "landmark_ps",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "LandmarkShader::LandmarkPS",
                sha1: "92451f5fce86",
            }),
        },
    ],
};

/// `BuildingShader` and `BuildingShader_Skinned` on the instance of `world-objects`: the meshes of the
/// landmarks (rocks, animals, fruit trees) and the buildings of the cities that `UnitDraw` draws.
/// The landmarks' pixel stage is `BuildingPS` of the `building` module; the cities' is the ported
/// `BuildingPS_CODE_Opaque` of `building_code` (the `*_code_*` entries), and the casters run the
/// ported `BuildingVS_Instanced_Shadow` and `BuildingSkinnedVS_Shadow` bodies.
const BUILDING_OBJECT: ModuleDesc = ModuleDesc {
    name: "building_object",
    family: "BuildingShader_Object",
    parts: &[
        part!("common/frame.wgsl"),
        part!("common/sm3.wgsl"),
        part!("common/skin.wgsl"),
        part!("skin/common.wgsl"),
        part!("building/building_code.wgsl"),
        part!("skin/building_object.wgsl"),
    ],
    entries: &[
        entry("building_object_vs", Stage::Vertex),
        entry("building_skinned_vs", Stage::Vertex),
        entry("building_object_ps", Stage::Fragment),
        entry("building_object_reflect_ps", Stage::Fragment),
        // The buildings of the cities.
        entry("building_object_code_vs", Stage::Vertex),
        entry("building_skinned_code_vs", Stage::Vertex),
        entry("building_object_code_ps", Stage::Fragment),
        entry("building_object_code_reflect_ps", Stage::Fragment),
        entry("building_object_code_alpha_ps", Stage::Fragment),
        entry("building_object_code_alpha_reflect_ps", Stage::Fragment),
        // The entries of `building_code.wgsl` come with it (`building_code` compares and replays them).
        entry("building_ps_code", Stage::Fragment),
        entry("building_ps_code_opaque", Stage::Fragment),
        entry("building_ps_code_base_shadow", Stage::Fragment),
        entry("building_ps_code_base_shadow_opaque", Stage::Fragment),
        entry("building_vs_shadow", Stage::Vertex),
        entry("building_vs_instanced_shadow", Stage::Vertex),
        entry("building_skinned_vs_shadow", Stage::Vertex),
        // The casters of the shadow map (the meshes whose material has `Shadows`).
        entry("building_object_shadow_vs", Stage::Vertex),
        entry("building_skinned_shadow_vs", Stage::Vertex),
        entry("building_shadow_ps", Stage::Fragment),
    ],
};

/// The shore waves of the water (`WaveShader`, world-water).
const WATER_WAVES: ModuleDesc = ModuleDesc {
    name: "water_waves",
    family: "WaveShader",
    parts: &[part!("common/sm3.wgsl"), part!("water/waves.wgsl")],
    entries: &[
        EntryDesc {
            name: "vs_wave",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "WaveShader::VS_Wave",
                sha1: "799a8edd24a8",
            }),
        },
        EntryDesc {
            name: "ps_wave",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "WaveShader::PS_Wave",
                sha1: "f228f4f63ef9",
            }),
        },
    ],
};

/// The movement arrows of the map (`ArrowShader`, world-overlays).
const OVERLAY_ARROW: ModuleDesc = ModuleDesc {
    name: "overlay_arrow",
    family: "ArrowShader",
    parts: &[part!("common/sm3.wgsl"), part!("overlay/arrow.wgsl")],
    entries: &[
        EntryDesc {
            name: "vs_arrow",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "ArrowShader::VSArrow",
                sha1: "5dfd35c166d9",
            }),
        },
        EntryDesc {
            name: "ps_arrow",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "ArrowShader::PSArrow",
                sha1: "8048cbde7585",
            }),
        },
    ],
};

/// The trade route lines (`TradeArrowShader`, card R5.9). The bytecode is not in the game's files,
/// so the entries have no input to compare with.
const OVERLAY_TRADE: ModuleDesc = ModuleDesc {
    name: "overlay_trade",
    family: "TradeArrowShader",
    parts: &[part!("overlay/trade.wgsl")],
    entries: &[
        entry("vs_trade", Stage::Vertex),
        entry("ps_trade", Stage::Fragment),
    ],
};

/// The ice sheets on the sea (`IceShader`, world-water).
const WATER_ICE: ModuleDesc = ModuleDesc {
    name: "water_ice",
    family: "IceShader",
    parts: &[part!("common/sm3.wgsl"), part!("water/ice.wgsl")],
    entries: &[
        EntryDesc {
            name: "vs_ice",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "IceShader::RenderSceneVS",
                sha1: "39668cf48ead",
            }),
        },
        EntryDesc {
            name: "ps_ice",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "IceShader::PS_Ice",
                sha1: "1dc27fc552ef",
            }),
        },
    ],
};

/// `FXShader` (effect models `FXM_*.gr2`: city damage smoke and fire, pillage smoke, reef caustics):
/// the rigid and the skinned vertex shaders and the pixel shaders of one to three layers. The
/// skinned pixel shaders of the original repeat the code of these.
const FX: ModuleDesc = ModuleDesc {
    name: "fx",
    family: "FXShader",
    parts: &[
        part!("common/frame.wgsl"),
        part!("common/sm3.wgsl"),
        part!("common/skin.wgsl"),
        part!("skin/common.wgsl"),
        part!("unit/fx.wgsl"),
    ],
    entries: &[
        EntryDesc {
            name: "fx_vs",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "FXShader::FXVS",
                sha1: "218379a532f3",
            }),
        },
        // The rigid vertex shader with the clock of the god ray (FXM_God_Ray, re-b11 4).
        entry("fx_god_ray_vs", Stage::Vertex),
        EntryDesc {
            name: "fx_skinned_vs",
            stage: Stage::Vertex,
            re: Some(ReRef {
                input: "FXShader::FXSkinnedVS",
                sha1: "9384a34c0999",
            }),
        },
        EntryDesc {
            name: "fx_ps_code1",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "FXShader::FXPS_CODE1",
                sha1: "fe2238051b8d",
            }),
        },
        EntryDesc {
            name: "fx_ps_code2",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "FXShader::FXPS_CODE2",
                sha1: "6cc19a163f33",
            }),
        },
        EntryDesc {
            name: "fx_ps_code3",
            stage: Stage::Fragment,
            re: Some(ReRef {
                input: "FXShader::FXPS_CODE3",
                sha1: "b5ed4495c42c",
            }),
        },
    ],
};

/// The Fork particles (R5.12): stand-ins of ForkDefaultShader, ForkFakeVolumeShader and
/// ForkBrightnessShader, whose bodies are not read (SIMP-048).
const FORK: ModuleDesc = ModuleDesc {
    name: "fork",
    family: "ForkShaders",
    parts: &[part!("common/frame.wgsl"), part!("fork/fork.wgsl")],
    entries: &[
        entry("fork_vs", Stage::Vertex),
        entry("fork_ps", Stage::Fragment),
    ],
};

const fn re_entry(
    name: &'static str,
    stage: Stage,
    input: &'static str,
    sha1: &'static str,
) -> EntryDesc {
    EntryDesc {
        name,
        stage,
        re: Some(ReRef { input, sha1 }),
    }
}

/// The `_CODE` and shadow stages of `BuildingShader` the D3D11 build draws in S1-S4: the colour pass
/// with the colour correction, the fog of war and the distance fog, and the casters of the shadow
/// map. `BuildingPS_CODE_Opaque` is DX11 only (`ported-dx11`).
const BUILDING_CODE: ModuleDesc = ModuleDesc {
    name: "building_code",
    family: "BuildingShader",
    parts: &[
        part!("common/frame.wgsl"),
        part!("common/sm3.wgsl"),
        part!("common/skin.wgsl"),
        part!("skin/common.wgsl"),
        part!("building/building_code.wgsl"),
    ],
    entries: &[
        re_entry(
            "building_ps_code",
            Stage::Fragment,
            "BuildingShader::BuildingPS_CODE",
            "d92314717b01",
        ),
        re_entry(
            "building_ps_code_opaque",
            Stage::Fragment,
            "BuildingShader::BuildingPS_CODE_Opaque",
            "5154367387fa",
        ),
        re_entry(
            "building_ps_code_base_shadow",
            Stage::Fragment,
            "BuildingShader::BuildingPS_CODE_BASE_SHADOW",
            "8e17a57e70f9",
        ),
        re_entry(
            "building_ps_code_base_shadow_opaque",
            Stage::Fragment,
            "BuildingShader::BuildingPS_CODE_BASE_SHADOW_OPAQUE",
            "53a357768fd6",
        ),
        re_entry(
            "building_vs_shadow",
            Stage::Vertex,
            "BuildingShader::BuildingVS_Shadow",
            "0d9e501bcfd3",
        ),
        re_entry(
            "building_vs_instanced_shadow",
            Stage::Vertex,
            "BuildingShader::BuildingVS_Instanced_Shadow",
            "bebb4b952411",
        ),
        re_entry(
            "building_skinned_vs_shadow",
            Stage::Vertex,
            "BuildingShader::BuildingSkinnedVS_Shadow",
            "22390584ec44",
        ),
    ],
};

/// The `_CODE` pixel stages of the skinned units (the colour pass and the caster) that the D3D11
/// build draws in S1-S4.
const UNIT_CODE: ModuleDesc = ModuleDesc {
    name: "unit_code",
    family: "UnitShader",
    parts: &[
        part!("common/frame.wgsl"),
        part!("common/sm3.wgsl"),
        part!("unit/unit.wgsl"),
        part!("unit/unit_code.wgsl"),
    ],
    entries: &[
        // `unit.wgsl` has its own entry points; they are part of the text.
        entry("unit_vs", Stage::Vertex),
        entry("unit_ps", Stage::Fragment),
        entry("unit_alpha_ps", Stage::Fragment),
        re_entry(
            "unit_skinned_ps_code",
            Stage::Fragment,
            "UnitShader::UnitSkinnedPS_CODE",
            "b42daf79f3f8",
        ),
        re_entry(
            "unit_skinned_shadow_ps_code",
            Stage::Fragment,
            "UnitShader::UnitSkinnedPS_CODE_Shadow",
            "a3ef2ead9aba",
        ),
    ],
};

/// `cvResolvePass`: the copy of the resolved back buffer (S3).
const POST_RESOLVE: ModuleDesc = ModuleDesc {
    name: "post_resolve",
    family: "cvResolvePass",
    parts: &[part!("post/resolve.wgsl")],
    entries: &[
        re_entry(
            "resolve_vs",
            Stage::Vertex,
            "cvResolvePass::VS",
            "003e91a284e9",
        ),
        re_entry(
            "resolve_ps",
            Stage::Fragment,
            "cvResolvePass::PS",
            "df80586f12fe",
        ),
    ],
};

static CATALOG: [ModuleDesc; 45] = [
    COMMON_PROBE,
    MASK,
    UNIT,
    UNIT_SKINNED,
    BUILDING_OBJECT,
    SIMPLE,
    FX,
    LANDMARK,
    SHADOW_BLUR,
    SHADOW_CASTER,
    BUILDING,
    FOREST,
    TERRAIN_M1,
    TERRAIN_BAKE_CACHE,
    TERRAIN_SCREEN_PS,
    TERRAIN_BAKE_COMPOSITE,
    TERRAIN_BAKE_NORMAL,
    TERRAIN_CELL,
    TERRAIN_CELL_ARRAY,
    TERRAIN_DECAL,
    TERRAIN_BAKE_SHADOW,
    FOW_MASK,
    FOW_CULL,
    FOW_BASE,
    FOW_BASE_MASK,
    FOW_CLASSIC,
    OVERLAY_GRID,
    OVERLAY_COUNTRY,
    OVERLAY_HIGHLIGHT,
    OVERLAY_RING,
    OVERLAY_SCREEN_SPACE,
    OVERLAY_CELLS,
    UI,
    MINIMAP,
    WATER_OCEAN,
    WATER_RIVER,
    WATER_CURRENTS,
    WATER_WAVES,
    OVERLAY_ARROW,
    WATER_ICE,
    FORK,
    OVERLAY_TRADE,
    BUILDING_CODE,
    UNIT_CODE,
    POST_RESOLVE,
];

/// Every module of the products, in the order they were added.
pub fn catalog() -> &'static [ModuleDesc] {
    &CATALOG
}
