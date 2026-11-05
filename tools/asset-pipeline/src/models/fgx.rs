//! `FireGrafixMaterial` (FGX) block to [`MaterialDesc`].
//!
//! The block lists shader inputs as parallel arrays per `ParameterSet`. This module only
//! translates: names come from the FGX dictionary of `granny` plus the names read from the shader
//! catalog ([`NameDict`]), texture paths from a caller-supplied lookup (see
//! [`super::material_desc`]). A name nobody knows stays `#xxxxxxxx` and is listed in
//! [`Unresolved`]; what `ParamSemantics`, `ParamRegs` and `ParamFlags` mean is not known (RW-9a),
//! so they are copied as stored.
//!
//! # Rules taken from the original (`FGrFGXMaterialInfo`)
//! - `Reflections`, `Shadows` and `AlphaSort` count for versions 2 to 4, `Invisible` for 3 and 4,
//!   `Z-Fail` for 4 only; version 0 sets none of them.
//! - The material is skinned when `SkinBoneCount` is above 0, and has parameters when
//!   `ParameterSetCount` is above 0 (it can say up to 19; only `ParameterSet0…3` exist).
//! - `ParamStoreIdx` is an element index into the pooled data: `ParamDataT` for textures (four
//!   slots per texture), `ParamDataF` / `ParamDataI` for constants. A negative index, or an empty
//!   path, means the engine supplies the value.

use std::collections::BTreeMap;

use asset_format::{ConstParam, MaterialDesc, ParamSet, TexKind, TexParam, TexSource};
use granny::extended::{FgxMaterial, FgxParamSet};
use granny::material::NameRef;

use super::ModelError;

/// What an unreadable name was used for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NameRole {
    ShaderSet,
    Block,
    Param,
    Fallback,
}

impl NameRole {
    pub fn label(self) -> &'static str {
        match self {
            Self::ShaderSet => "shader-set",
            Self::Block => "block",
            Self::Param => "param",
            Self::Fallback => "fallback",
        }
    }
}

/// A stripped name that is in no dictionary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Unresolved {
    pub role: NameRole,
    pub hash: u32,
}

impl std::fmt::Display for Unresolved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} #{:08x}", self.role.label(), self.hash)
    }
}

/// Names beyond `granny`'s built-in FGX dictionary: the strings of the shader catalog.
#[derive(Debug, Clone, Default)]
pub struct NameDict {
    extra: BTreeMap<u32, String>,
}

impl NameDict {
    /// Dictionary with the built-in names only.
    pub fn builtin() -> Self {
        Self::default()
    }

    pub fn add(&mut self, name: &str) {
        self.extra
            .entry(granny::name_hash(name))
            .or_insert_with(|| name.to_owned());
    }

    pub fn extend<'a>(&mut self, names: impl IntoIterator<Item = &'a str>) {
        for n in names {
            self.add(n);
        }
    }

    /// Number of names added on top of the built-in ones.
    pub fn len(&self) -> usize {
        self.extra.len()
    }

    pub fn is_empty(&self) -> bool {
        self.extra.is_empty()
    }

    /// The text of `n` if known; `None` for an empty string or an unknown hash.
    pub fn resolve(&self, n: &NameRef) -> Option<String> {
        if let Some(r) = n.resolved() {
            return Some(r.to_owned());
        }
        n.id.and_then(|id| self.extra.get(&id).cloned())
    }

    /// `resolve`, else `#xxxxxxxx` and a note in `unresolved`. An empty string is `""`.
    fn name(&self, n: &NameRef, role: NameRole, unresolved: &mut Vec<Unresolved>) -> String {
        if let Some(t) = self.resolve(n) {
            return t;
        }
        match n.id {
            Some(hash) => {
                unresolved.push(Unresolved { role, hash });
                format!("#{hash:08x}")
            }
            None => String::new(),
        }
    }
}

/// Where a texture parameter's path resolves to; the caller owns the path knowledge.
pub trait TexturePaths {
    /// Source for the path `path` (non-empty, text or hash) of the sampler `sampler`.
    fn source(&self, sampler: &str, path: &NameRef) -> TexSource;

    /// Source for a sampler whose path is empty (or whose store index is negative): `None` is the
    /// engine's texture.
    fn empty(&self, _sampler: &str) -> Option<TexSource> {
        None
    }
}

/// `MaterialDesc` plus the names that stayed unknown.
#[derive(Debug, Clone, PartialEq)]
pub struct Converted {
    pub desc: MaterialDesc,
    pub unresolved: Vec<Unresolved>,
}

fn kind_of(ty: i32) -> Option<TexKind> {
    match ty {
        6 => Some(TexKind::Tex2D),
        7 => Some(TexKind::Cube),
        10 => Some(TexKind::Tex3D),
        _ => None,
    }
}

fn at<T: Copy>(v: &[T], i: usize, what: &str) -> Result<T, ModelError> {
    v.get(i).copied().ok_or_else(|| {
        ModelError::invalid(format!(
            "FGX parameter {i}: `{what}` has only {} entries",
            v.len()
        ))
    })
}

/// A path is empty when the string is null, or is the hash of the empty string.
fn is_empty_path(n: &NameRef) -> bool {
    match (&n.text, n.id) {
        (Some(t), _) => t.is_empty(),
        (None, Some(id)) => id == granny::name_hash(""),
        (None, None) => true,
    }
}

fn convert_set(
    set: &FgxParamSet,
    dict: &NameDict,
    paths: &dyn TexturePaths,
    unresolved: &mut Vec<Unresolved>,
) -> Result<ParamSet, ModelError> {
    let n = usize::try_from(set.count)
        .map_err(|_| ModelError::invalid(format!("FGX ParamCount {}", set.count)))?;
    if set.names.len() < n {
        return Err(ModelError::invalid(format!(
            "FGX set has ParamCount {n} but {} names",
            set.names.len()
        )));
    }
    let active = set.active != 0;
    let mut out = ParamSet {
        blocks: vec![dict.name(&set.block, NameRole::Block, unresolved)],
        ..ParamSet::default()
    };
    for i in 0..n {
        let name = dict.name(&set.names[i], NameRole::Param, unresolved);
        let ty = at(&set.types, i, "ParamTypes")?;
        let semantic = at(&set.semantics, i, "ParamSemantics")?;
        let reg = at(&set.regs, i, "ParamRegs")?;
        let usage = set.usage.get(i).copied().unwrap_or(0);
        let flags = set.flags.get(i).copied().unwrap_or(0);
        let store = at(&set.store_idx, i, "ParamStoreIdx")?;
        if let Some(kind) = kind_of(ty) {
            let src = match usize::try_from(store).ok().and_then(|s| set.data_t.get(s)) {
                Some(p) if !is_empty_path(p) => paths.source(&name, p),
                // Negative index, past the pool, or an empty path.
                _ => paths.empty(&name).unwrap_or(TexSource::Engine),
            };
            out.textures.push(TexParam {
                name,
                kind,
                src,
                semantic,
                reg,
                usage,
                flags,
                active,
            });
        } else if ty == 0 || ty == 2 {
            // Type 2 is a boolean constant (`g_bIsShadowPass`); it is stored like a constant.
            let size = if ty == 2 {
                set.size.get(i).copied().unwrap_or(4)
            } else {
                at(&set.size, i, "ParamSize")?
            };
            let size16 = u16::try_from(size)
                .map_err(|_| ModelError::invalid(format!("constant {name}: size {size}")))?;
            let (mut f, mut iv) = (Vec::new(), Vec::new());
            if let Ok(start) = usize::try_from(store) {
                let len = usize::from(size16) / 4;
                let slice = |total: usize| start.checked_add(len).filter(|&e| e <= total);
                // A set keeps its numbers either as floats or as ints (not seen mixed).
                if !set.data_f.is_empty() {
                    let end = slice(set.data_f.len()).ok_or_else(|| {
                        ModelError::invalid(format!("constant {name}: data out of range"))
                    })?;
                    f = set.data_f[start..end].to_vec();
                } else if !set.data_i.is_empty() {
                    let end = slice(set.data_i.len()).ok_or_else(|| {
                        ModelError::invalid(format!("constant {name}: data out of range"))
                    })?;
                    iv = set.data_i[start..end].to_vec();
                }
            }
            out.constants.push(ConstParam {
                name,
                semantic,
                reg,
                size: size16,
                usage,
                flags,
                f,
                i: iv,
                active,
            });
        } else {
            return Err(ModelError::invalid(format!(
                "FGX parameter {name}: unknown type {ty}"
            )));
        }
    }
    Ok(out)
}

/// Translates one FGX block. `source_name` is the Granny material name when it is readable.
pub fn fgx_to_desc(
    fgx: &FgxMaterial,
    source_name: Option<String>,
    dict: &NameDict,
    paths: &dyn TexturePaths,
) -> Result<Converted, ModelError> {
    let mut unresolved = Vec::new();
    let version = u8::try_from(fgx.version)
        .map_err(|_| ModelError::invalid(format!("FGX version {}", fgx.version)))?;
    let skin_bones = u8::try_from(fgx.skin_bone_count)
        .map_err(|_| ModelError::invalid(format!("SkinBoneCount {}", fgx.skin_bone_count)))?;
    let to_u32 = |v: i32, what: &str| {
        u32::try_from(v).map_err(|_| ModelError::invalid(format!("FGX {what} {v}")))
    };
    let alpha_z = [
        to_u32(fgx.alpha_mode, "AlphaMode")?,
        to_u32(fgx.z_mode, "ZMode")?,
        to_u32(fgx.alpha_ref, "AlphaRef")?,
    ];

    let v2_4 = (2..=4).contains(&version);
    let mut flags = 0u16;
    let mut set_flag = |on: bool, bit: u16| {
        if on {
            flags |= bit;
        }
    };
    set_flag(fgx.skin_bone_count > 0, MaterialDesc::SKINNED);
    set_flag(v2_4 && fgx.shadows != 0, MaterialDesc::SHADOWS);
    set_flag(v2_4 && fgx.reflections != 0, MaterialDesc::REFLECTIONS);
    set_flag(v2_4 && fgx.alpha_sort != 0, MaterialDesc::ALPHA_SORT);
    set_flag(
        (version == 3 || version == 4) && fgx.invisible != 0,
        MaterialDesc::INVISIBLE,
    );
    set_flag(version == 4 && fgx.z_fail != 0, MaterialDesc::ZFAIL);
    set_flag(fgx.use_fallback_mtl != 0, MaterialDesc::USE_FALLBACK);
    set_flag(fgx.parameter_set_count > 0, MaterialDesc::HAS_PARAMS);

    let package = dict.name(&fgx.shader_set, NameRole::ShaderSet, &mut unresolved);
    let mut fallback = None;
    for n in &fgx.fallback_mtl {
        let name = dict.name(n, NameRole::Fallback, &mut unresolved);
        if fallback.is_none() && !name.is_empty() {
            fallback = Some(name);
        }
    }

    // Trailing absent sets are dropped, one in between stays as an empty set.
    let last = fgx.sets.iter().rposition(Option::is_some);
    let mut sets = Vec::new();
    for slot in &fgx.sets[..last.map_or(0, |l| l + 1)] {
        sets.push(match slot {
            Some(s) => convert_set(s, dict, paths, &mut unresolved)?,
            None => ParamSet::default(),
        });
    }

    Ok(Converted {
        desc: MaterialDesc {
            package,
            version,
            flags,
            alpha_z,
            skin_bones,
            fallback,
            sets,
            source_name,
        },
        unresolved,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use asset_format::LogicalId;

    fn id(s: &str) -> NameRef {
        NameRef {
            text: None,
            id: Some(granny::name_hash(s)),
        }
    }

    fn text(s: &str) -> NameRef {
        NameRef {
            text: Some(s.into()),
            id: None,
        }
    }

    /// Resolves hashed paths against a fixed list of full paths.
    struct Paths(Vec<String>);
    impl TexturePaths for Paths {
        fn source(&self, _sampler: &str, p: &NameRef) -> TexSource {
            let hit = self.0.iter().find(|f| match (&p.text, p.id) {
                (Some(t), _) => t == *f,
                (None, Some(h)) => granny::name_hash(f) == h,
                _ => false,
            });
            match hit {
                Some(f) => TexSource::File(LogicalId::new(format!("tx:{}", f.to_lowercase()))),
                None => TexSource::Unresolved(p.id.unwrap_or(0)),
            }
        }
    }

    fn cow(stripped: bool) -> FgxMaterial {
        let n = |s: &str| if stripped { id(s) } else { text(s) };
        let globals = FgxParamSet {
            block: n("BuildingGlobal"),
            count: 2,
            names: vec![n("ViewMatrix"), n("f3FogColor")],
            types: vec![0, 0],
            usage: vec![1, 0],
            regs: vec![327_679, -65531],
            size: vec![64, 16],
            semantics: vec![3, -1],
            store_idx: vec![-1, 0],
            data_f: vec![0.5, 0.25, 0.0, 1.0],
            data_i: vec![],
            data_t: vec![],
            active: 1,
            flags: vec![8, 8],
        };
        let path = |s: &str| n(&format!(r"D:\art\Units\Shared\{s}"));
        let empty = NameRef::default();
        let textures = FgxParamSet {
            block: n("BuildingTextures"),
            count: 3,
            names: vec![n("Diffuse"), n("ColorCorrection"), n("Mystery")],
            types: vec![6, 10, 7],
            usage: vec![0, 0, 0],
            regs: vec![-65536, -65530, -65529],
            size: vec![16, 16, 16],
            semantics: vec![42, -1, -1],
            store_idx: vec![0, -4, 4],
            data_t: vec![
                path("Cow_DIFF.dds"),
                empty.clone(),
                empty.clone(),
                empty.clone(),
                path("Gone.dds"),
                empty.clone(),
                empty.clone(),
                empty,
            ],
            data_f: vec![],
            data_i: vec![0, 0],
            active: 1,
            flags: vec![8, 8, 8],
        };
        FgxMaterial {
            shader_set: n("BuildingShader_Skinned"),
            fallback_mtl: vec![NameRef::default()],
            version: 4,
            alpha_mode: 8,
            z_mode: 4,
            alpha_ref: 128,
            skin_bone_count: 32,
            shadows: 1,
            z_fail: 0,
            parameter_set_count: 2,
            sets: [Some(globals), Some(textures), None, None],
            ..FgxMaterial::default()
        }
    }

    fn paths() -> Paths {
        Paths(vec![r"D:\art\Units\Shared\Cow_DIFF.dds".into()])
    }

    #[test]
    fn cow_block_becomes_a_material_desc() {
        for stripped in [false, true] {
            let c = fgx_to_desc(
                &cow(stripped),
                Some("Game Cow 01".into()),
                &NameDict::builtin(),
                &paths(),
            )
            .unwrap();
            let d = &c.desc;
            assert_eq!(d.package, "BuildingShader_Skinned");
            assert_eq!((d.version, d.alpha_z, d.skin_bones), (4, [8, 4, 128], 32));
            assert_eq!(
                d.flags,
                MaterialDesc::SKINNED | MaterialDesc::SHADOWS | MaterialDesc::HAS_PARAMS
            );
            assert_eq!(d.fallback, None);
            assert_eq!(d.sets.len(), 2);
            assert_eq!(d.sets[0].blocks, ["BuildingGlobal"]);
            assert_eq!(d.sets[1].blocks, ["BuildingTextures"]);

            let fog = d.constant("f3FogColor").unwrap();
            assert_eq!((fog.size, fog.f.clone()), (16, vec![0.5, 0.25, 0.0, 1.0]));
            // No stored data: the engine fills the constant.
            assert!(d.constant("ViewMatrix").unwrap().f.is_empty());

            let diffuse = d.texture("Diffuse").unwrap();
            assert_eq!(diffuse.kind, TexKind::Tex2D);
            assert_eq!(
                diffuse.src,
                TexSource::File(LogicalId::new(r"tx:d:\art\units\shared\cow_diff.dds"))
            );
            let cc = d.texture("ColorCorrection").unwrap();
            assert_eq!((cc.kind, &cc.src), (TexKind::Tex3D, &TexSource::Engine));

            // `Mystery` is in no dictionary; its path is in no list.
            let m = d.texture("Mystery");
            if stripped {
                let hash = granny::name_hash("Mystery");
                assert!(m.is_none());
                let t = d.texture(&format!("#{hash:08x}")).unwrap();
                assert_eq!(t.kind, TexKind::Cube);
                assert!(matches!(t.src, TexSource::Unresolved(_)));
                assert_eq!(
                    c.unresolved,
                    [Unresolved {
                        role: NameRole::Param,
                        hash
                    }]
                );
            } else {
                assert!(m.is_some());
                assert!(c.unresolved.is_empty());
            }
        }
    }

    #[test]
    fn extra_dictionary_names_resolve() {
        let mut dict = NameDict::builtin();
        dict.add("Mystery");
        let c = fgx_to_desc(&cow(true), None, &dict, &paths()).unwrap();
        assert!(c.unresolved.is_empty());
        assert!(c.desc.texture("Mystery").is_some());
    }

    #[test]
    fn version_gates_flags() {
        let mut f = cow(false);
        f.reflections = 1;
        f.alpha_sort = 1;
        f.invisible = 1;
        f.z_fail = 1;
        f.use_fallback_mtl = 1;
        let mut all = |v: i32| {
            f.version = v;
            fgx_to_desc(&f, None, &NameDict::builtin(), &paths())
                .unwrap()
                .desc
                .flags
        };
        let basic = MaterialDesc::SKINNED | MaterialDesc::USE_FALLBACK | MaterialDesc::HAS_PARAMS;
        let v2 = MaterialDesc::SHADOWS | MaterialDesc::REFLECTIONS | MaterialDesc::ALPHA_SORT;
        assert_eq!(all(0), basic);
        assert_eq!(all(2), basic | v2);
        assert_eq!(all(3), basic | v2 | MaterialDesc::INVISIBLE);
        assert_eq!(
            all(4),
            basic | v2 | MaterialDesc::INVISIBLE | MaterialDesc::ZFAIL
        );
    }

    #[test]
    fn absent_middle_set_stays_empty_and_trailing_ones_go() {
        let mut f = cow(false);
        f.sets = [None, None, f.sets[1].take(), None];
        let d = fgx_to_desc(&f, None, &NameDict::builtin(), &paths())
            .unwrap()
            .desc;
        assert_eq!(d.sets.len(), 3);
        assert!(d.sets[0].blocks.is_empty() && d.sets[1].blocks.is_empty());
        assert_eq!(d.sets[2].blocks, ["BuildingTextures"]);
    }

    #[test]
    fn broken_blocks_are_errors_not_panics() {
        let mut f = cow(false);
        f.sets[1].as_mut().unwrap().types[0] = 3;
        assert!(fgx_to_desc(&f, None, &NameDict::builtin(), &paths()).is_err());
        let mut f = cow(false);
        f.sets[0].as_mut().unwrap().data_f.truncate(2);
        assert!(fgx_to_desc(&f, None, &NameDict::builtin(), &paths()).is_err());
        let mut f = cow(false);
        f.sets[1].as_mut().unwrap().semantics.clear();
        assert!(fgx_to_desc(&f, None, &NameDict::builtin(), &paths()).is_err());
        let mut f = cow(false);
        f.skin_bone_count = 300;
        assert!(fgx_to_desc(&f, None, &NameDict::builtin(), &paths()).is_err());
    }
}
