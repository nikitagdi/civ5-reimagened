//! Typed materials (`granny_material`): name, texture maps and the source file of each map.
//!
//! Civ5 packed files strip every string: `Name`, `Usage` and `FromFileName` keep only a hash of
//! the original text ([`crate::name_hash`]) in the pointer slot, exposed as the `*_id` fields.
//! `FromFileName` is the exporter's absolute source path (directory unknown), so a file name can
//! only be checked against it with [`crate::name_hash_prefix`].

use crate::{GrannyError, GrannyFile, Object};

/// A string member that may be stripped: the text if present, else its hash.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NameRef {
    pub text: Option<String>,
    /// Hash stored in place of the string (non-zero), when `text` is `None`.
    pub id: Option<u32>,
}

impl NameRef {
    fn read(o: &Object<'_>, field: &str) -> Result<NameRef, GrannyError> {
        if o.type_def().member(field).is_none() {
            return Ok(NameRef::default());
        }
        let (text, id) = o.name_and_id(field)?;
        Ok(NameRef { text, id })
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_none() && self.id.is_none()
    }
}

/// One `Maps` entry: a usage label and the texture of the referenced sub-material.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MaterialMap {
    /// 3ds Max slot label, e.g. `"Diffuse"`.
    pub usage: NameRef,
    /// `Map -> Texture -> FromFileName`, empty if the map has no texture.
    pub texture_file: NameRef,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Material {
    pub name: NameRef,
    /// `Texture -> FromFileName` of the material itself (Civ5 materials use `maps` instead).
    pub texture_file: NameRef,
    pub maps: Vec<MaterialMap>,
}

fn texture_file(o: &Object<'_>) -> Result<NameRef, GrannyError> {
    if o.type_def().member("Texture").is_none() {
        return Ok(NameRef::default());
    }
    match o.reference("Texture")? {
        Some(t) => NameRef::read(&t, "FromFileName"),
        None => Ok(NameRef::default()),
    }
}

/// The file's `Materials` in order (the indices `MaterialBindings` use), without decoding meshes.
pub fn read_materials(file: &GrannyFile<'_>) -> Result<Vec<Material>, GrannyError> {
    let root = file.root()?;
    let arr = match root.references("Materials") {
        Ok(a) => a,
        Err(GrannyError::MissingField(_)) => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut out = Vec::new();
    for o in arr.iter()? {
        if let Some(o) = o? {
            out.push(Material::read(&o)?);
        }
    }
    Ok(out)
}

impl Material {
    pub fn read(o: &Object<'_>) -> Result<Material, GrannyError> {
        let mut maps = Vec::new();
        if o.type_def().member("Maps").is_some() {
            for m in o.array("Maps")?.iter() {
                let texture_file = match m.reference("Map")? {
                    Some(sub) => texture_file(&sub)?,
                    None => NameRef::default(),
                };
                maps.push(MaterialMap {
                    usage: NameRef::read(&m, "Usage")?,
                    texture_file,
                });
            }
        }
        Ok(Material {
            name: NameRef::read(o, "Name")?,
            texture_file: texture_file(o)?,
            maps,
        })
    }
}
