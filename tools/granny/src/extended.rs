//! `ExtendedData` of a material: the `FireGrafixMaterial` block Civ5's exporter attaches.
//!
//! The block is a Granny variant: an object whose type is stored with the data. Its top level
//! names the shader package and render state; up to four `ParameterSet`s list the shader inputs
//! (a constant buffer or a texture group) as parallel arrays indexed by parameter. Packed files
//! strip every string, so a [`NameRef`] holds either the text or the hash that stands in for it
//! ([`crate::name_hash`]). Members are looked up by hash and work in both cases; the strings
//! resolve through the dictionary in `data/fgx-names.txt` ([`crate::fgx_name`]).
//!
//! What `ParamSemantics`, `ParamRegs` and `ParamFlags` mean, and which set applies when, is not
//! known (RW-9a); they are kept as stored.

use crate::bytes::le_u32;
use crate::material::NameRef;
use crate::{GrannyError, GrannyFile, Object, Value, fgx_name, name_hash};

/// Value of the `typeName` member that marks the block.
pub const FGX_TYPE_NAME: &str = "FireGrafixMaterial";
/// `ParameterSet0` ..= `ParameterSet3`.
pub const MAX_PARAMETER_SETS: usize = 4;
/// Versions of the block seen in the game files.
pub const KNOWN_VERSIONS: [i32; 4] = [0, 2, 3, 4];

impl NameRef {
    /// The text if stored, else its name from the FGX dictionary.
    pub fn resolved(&self) -> Option<&str> {
        match (&self.text, self.id) {
            (Some(t), _) => Some(t),
            (None, Some(id)) => fgx_name(id),
            (None, None) => None,
        }
    }

    /// `resolved`, or `#xxxxxxxx` for an unknown hash; `None` for a null string.
    pub fn display(&self) -> Option<String> {
        match (self.resolved(), self.id) {
            (Some(t), _) => Some(t.to_owned()),
            (None, Some(id)) => Some(format!("#{id:08x}")),
            (None, None) => None,
        }
    }

    /// True for a stripped string whose hash is not in the dictionary.
    pub fn is_unresolved(&self) -> bool {
        self.text.is_none() && self.id.is_some_and(|id| fgx_name(id).is_none())
    }
}

/// One `ParameterSetN`: a constant buffer or texture group. All `param_*` vectors and `names`
/// are indexed by parameter (`count` entries); `data_*` are pooled and addressed by `store_idx`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FgxParamSet {
    /// `ParamBlock`, e.g. `BuildingGlobal`, `BuildingTextures`.
    pub block: NameRef,
    pub count: i32,
    /// `ParamNames`: sampler or constant names from the shader's constant table.
    pub names: Vec<NameRef>,
    /// `ParamTypes`: 0 number, 6 Tex2D, 7 cube, 10 volume.
    pub types: Vec<i32>,
    pub usage: Vec<i32>,
    pub regs: Vec<i32>,
    /// Bytes of each constant (textures: 16).
    pub size: Vec<i32>,
    pub semantics: Vec<i32>,
    pub store_idx: Vec<i32>,
    pub data_f: Vec<f32>,
    pub data_i: Vec<i32>,
    /// Texture paths (hashed when stripped), 4 slots per texture; null means the engine supplies
    /// the texture.
    pub data_t: Vec<NameRef>,
    pub active: i32,
    pub flags: Vec<i32>,
}

/// The `FireGrafixMaterial` block of one material. Members a version lacks read as 0.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FgxMaterial {
    pub shader_set: NameRef,
    pub fallback_mtl: Vec<NameRef>,
    pub use_fallback_mtl: i32,
    pub alpha_mode: i32,
    pub z_mode: i32,
    pub alpha_ref: i32,
    pub skin_bone_count: i32,
    pub reflections: i32,
    pub shadows: i32,
    pub alpha_sort: i32,
    pub version: i32,
    pub invisible: i32,
    pub z_fail: i32,
    pub parameter_set_count: i32,
    /// `ParameterSet0…3`; `None` where the member is absent or null.
    pub sets: [Option<FgxParamSet>; MAX_PARAMETER_SETS],
}

/// The material's `ExtendedData` object, if it has one.
pub fn extended<'f>(material: &Object<'f>) -> Result<Option<Object<'f>>, GrannyError> {
    if material.type_def().member("ExtendedData").is_none() {
        return Ok(None);
    }
    material.reference("ExtendedData")
}

/// Whether `data` is a `FireGrafixMaterial` block (by its `typeName`, else by having `ShaderSet`).
pub fn is_fgx(data: &Object<'_>) -> bool {
    if data.type_def().member("typeName").is_some()
        && let Ok((text, id)) = data.name_and_id("typeName")
    {
        return text.as_deref() == Some(FGX_TYPE_NAME) || id == Some(name_hash(FGX_TYPE_NAME));
    }
    data.type_def().member("ShaderSet").is_some()
}

fn name_at(elem: &Object<'_>) -> Result<NameRef, GrannyError> {
    let Some(Value::String(s)) = elem.value(0) else {
        return Err(GrannyError::TypeMismatch {
            field: "string element".into(),
            expected: "string",
            found: elem.value(0).map_or("missing", |v| v.kind_name()),
        });
    };
    let text = s.to_string_lossy()?;
    let id = if text.is_none() {
        Some(le_u32(elem.bytes(), 0)).filter(|&v| v != 0)
    } else {
        None
    };
    Ok(NameRef { text, id })
}

fn opt_int(o: &Object<'_>, name: &str) -> Result<i32, GrannyError> {
    if o.type_def().member(name).is_none() {
        return Ok(0);
    }
    o.i32(name)
}

fn name(o: &Object<'_>, field: &str) -> Result<NameRef, GrannyError> {
    if o.type_def().member(field).is_none() {
        return Ok(NameRef::default());
    }
    let (text, id) = o.name_and_id(field)?;
    Ok(NameRef { text, id })
}

/// Array member whose elements are single-member structs (`Int32`, `Real32`, `String`); an absent
/// member reads as empty.
fn elements<'f>(o: &Object<'f>, field: &str) -> Result<Vec<Object<'f>>, GrannyError> {
    if o.type_def().member(field).is_none() {
        return Ok(Vec::new());
    }
    let array = o.array(field)?;
    // A zero-size element type passes the bounds check for any count; refuse it rather than
    // materialising billions of empty elements.
    if array.stride() == 0 && !array.is_empty() {
        return Err(GrannyError::Invalid(format!(
            "array `{field}` has zero-size elements"
        )));
    }
    Ok(array.iter().collect())
}

fn ints(o: &Object<'_>, field: &str) -> Result<Vec<i32>, GrannyError> {
    elements(o, field)?
        .iter()
        .map(|e| match e.value(0) {
            Some(v) => v
                .as_i64()
                .and_then(|x| i32::try_from(x).ok())
                .ok_or_else(|| GrannyError::TypeMismatch {
                    field: field.to_owned(),
                    expected: "int32",
                    found: v.kind_name(),
                }),
            None => Err(GrannyError::MissingField(field.to_owned())),
        })
        .collect()
}

fn floats(o: &Object<'_>, field: &str) -> Result<Vec<f32>, GrannyError> {
    elements(o, field)?
        .iter()
        .map(|e| match e.value(0) {
            Some(Value::Real32(x)) => Ok(x),
            Some(v) => Err(GrannyError::TypeMismatch {
                field: field.to_owned(),
                expected: "real32",
                found: v.kind_name(),
            }),
            None => Err(GrannyError::MissingField(field.to_owned())),
        })
        .collect()
}

fn names(o: &Object<'_>, field: &str) -> Result<Vec<NameRef>, GrannyError> {
    elements(o, field)?.iter().map(name_at).collect()
}

impl FgxParamSet {
    pub fn read(o: &Object<'_>) -> Result<FgxParamSet, GrannyError> {
        Ok(FgxParamSet {
            block: name(o, "ParamBlock")?,
            count: opt_int(o, "ParamCount")?,
            names: names(o, "ParamNames")?,
            types: ints(o, "ParamTypes")?,
            usage: ints(o, "ParamUsage")?,
            regs: ints(o, "ParamRegs")?,
            size: ints(o, "ParamSize")?,
            semantics: ints(o, "ParamSemantics")?,
            store_idx: ints(o, "ParamStoreIdx")?,
            data_f: floats(o, "ParamDataF")?,
            data_i: ints(o, "ParamDataI")?,
            data_t: names(o, "ParamDataT")?,
            active: opt_int(o, "ParamActive")?,
            flags: ints(o, "ParamFlags")?,
        })
    }

    /// Every string of the set (block, parameter names, texture paths) with its role.
    pub fn strings(&self) -> impl Iterator<Item = &NameRef> {
        std::iter::once(&self.block)
            .chain(&self.names)
            .chain(&self.data_t)
    }
}

impl FgxMaterial {
    /// Reads the block. A version outside [`KNOWN_VERSIONS`] is an error.
    pub fn read(data: &Object<'_>) -> Result<FgxMaterial, GrannyError> {
        let version = opt_int(data, "Version")?;
        if !KNOWN_VERSIONS.contains(&version) {
            return Err(GrannyError::Invalid(format!(
                "unsupported FireGrafixMaterial version {version}"
            )));
        }
        let mut sets: [Option<FgxParamSet>; MAX_PARAMETER_SETS] = Default::default();
        for (i, slot) in sets.iter_mut().enumerate() {
            let field = format!("ParameterSet{i}");
            if data.type_def().member(&field).is_none() {
                continue;
            }
            if let Some(set) = data.reference(&field)? {
                *slot = Some(FgxParamSet::read(&set)?);
            }
        }
        Ok(FgxMaterial {
            shader_set: name(data, "ShaderSet")?,
            fallback_mtl: names(data, "FallbackMtl")?,
            use_fallback_mtl: opt_int(data, "UseFallbackMtl")?,
            alpha_mode: opt_int(data, "AlphaMode")?,
            z_mode: opt_int(data, "ZMode")?,
            alpha_ref: opt_int(data, "AlphaRef")?,
            skin_bone_count: opt_int(data, "SkinBoneCount")?,
            reflections: opt_int(data, "Reflections")?,
            shadows: opt_int(data, "Shadows")?,
            alpha_sort: opt_int(data, "AlphaSort")?,
            version,
            invisible: opt_int(data, "Invisible")?,
            z_fail: opt_int(data, "Z-Fail")?,
            parameter_set_count: opt_int(data, "ParameterSetCount")?,
            sets,
        })
    }

    /// The sets that exist, in order.
    pub fn present_sets(&self) -> impl Iterator<Item = &FgxParamSet> {
        self.sets.iter().flatten()
    }

    /// Every string of the block: shader set, fallback names and the strings of each set.
    pub fn strings(&self) -> impl Iterator<Item = &NameRef> {
        std::iter::once(&self.shader_set)
            .chain(&self.fallback_mtl)
            .chain(self.present_sets().flat_map(FgxParamSet::strings))
    }
}

/// The FGX block of each root `Materials` entry, in order (the indices `MaterialBindings` use);
/// `None` for a material without one.
pub fn read_fgx_materials(file: &GrannyFile<'_>) -> Result<Vec<Option<FgxMaterial>>, GrannyError> {
    let root = file.root()?;
    let arr = match root.references("Materials") {
        Ok(a) => a,
        Err(GrannyError::MissingField(_)) => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for m in arr.iter()? {
        let fgx = match m? {
            Some(m) => match extended(&m)? {
                Some(d) if is_fgx(&d) => Some(FgxMaterial::read(&d)?),
                _ => None,
            },
            None => None,
        };
        out.push(fgx);
    }
    Ok(out)
}
