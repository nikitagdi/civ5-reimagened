//! Typed skeletons (`granny_skeleton`).

use crate::{GrannyError, Object, Transform, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct Bone {
    /// `None` in packed game files, where names are stripped.
    pub name: Option<String>,
    /// Raw ID stored in place of the name in packed game files (a name hash).
    pub name_id: Option<u32>,
    pub parent: Option<usize>,
    /// Rest transform relative to the parent.
    pub local: Transform,
    /// Column-major 4x4: model space to bone space.
    pub inverse_world: [f32; 16],
    pub lod_error: f32,
}

/// Decomposed local transform. `shear_free` is false if the 3x3 has off-diagonal terms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Trs {
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    pub shear_free: bool,
}

/// Scale/shear 3x3 off-diagonal tolerance for `Trs::shear_free`.
const SHEAR_EPS: f32 = 1e-4;

impl Bone {
    /// Translation, rotation and the diagonal of the scale/shear matrix.
    pub fn local_trs(&self) -> Trs {
        trs(&self.local)
    }
}

pub fn trs(t: &Transform) -> Trs {
    let m = &t.scale_shear;
    let off = [m[0][1], m[0][2], m[1][0], m[1][2], m[2][0], m[2][1]];
    Trs {
        translation: t.position,
        rotation: t.orientation,
        scale: [m[0][0], m[1][1], m[2][2]],
        shear_free: off.iter().all(|x| x.abs() <= SHEAR_EPS),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Skeleton {
    pub name: Option<String>,
    pub bones: Vec<Bone>,
}

impl Skeleton {
    pub fn read(o: &Object<'_>) -> Result<Skeleton, GrannyError> {
        let arr = o.array("Bones")?;
        let mut bones = Vec::with_capacity(arr.len());
        for (i, b) in arr.iter().enumerate() {
            let parent = b.i32("ParentIndex")?;
            let parent = match usize::try_from(parent) {
                Ok(p) if p < arr.len() => Some(p),
                Err(_) if parent == -1 => None,
                _ => {
                    return Err(GrannyError::Invalid(format!(
                        "bone {i}: parent index {parent} out of range"
                    )));
                }
            };
            let inverse_world = match b.get("InverseWorldTransform")? {
                Value::Fixed(f) if f.len() == 16 => {
                    let mut m = [0.0f32; 16];
                    for (dst, v) in m.iter_mut().zip(f.iter()) {
                        if let Value::Real32(x) = v {
                            *dst = x;
                        }
                    }
                    m
                }
                v => {
                    return Err(GrannyError::TypeMismatch {
                        field: "InverseWorldTransform".into(),
                        expected: "real32[16]",
                        found: v.kind_name(),
                    });
                }
            };
            let (name, name_id) = b.name_and_id("Name")?;
            bones.push(Bone {
                name,
                name_id,
                parent,
                local: b.transform("Transform")?,
                inverse_world,
                lod_error: b.f32("LODError").unwrap_or(0.0),
            });
        }
        Ok(Skeleton {
            name: o.string("Name")?,
            bones,
        })
    }

    /// Bone index for a name ID (unique match only). A bone that keeps its name (a loose mod file,
    /// e.g. Lekmod's `Baghlah.gr2`) matches by the hash of the name: such meshes reuse the
    /// stripped clips of the base game (`Caravel_IdleA.gr2`), whose tracks carry only the ID.
    pub fn find_bone_by_id(&self, id: u32) -> Option<usize> {
        let mut it = self.bones.iter().enumerate().filter(|(_, b)| {
            b.name_id == Some(id)
                || b.name
                    .as_deref()
                    .is_some_and(|n| crate::names::name_hash(n) == id)
        });
        let first = it.next()?;
        it.next().is_none().then_some(first.0)
    }

    /// Bone index for a name (exact match), if names exist.
    pub fn find_bone(&self, name: &str) -> Option<usize> {
        self.bones
            .iter()
            .position(|b| b.name.as_deref() == Some(name))
    }

    pub fn has_names(&self) -> bool {
        self.bones.iter().all(|b| b.name.is_some())
    }

    /// World-space rest matrices (column-major 4x4) built from the local transforms.
    pub fn rest_world(&self) -> Vec<[f32; 16]> {
        let mut world: Vec<[f32; 16]> = Vec::with_capacity(self.bones.len());
        for b in &self.bones {
            let local = transform_matrix(&b.local);
            let m = match b.parent.and_then(|p| world.get(p)) {
                Some(p) => mat_mul(p, &local),
                None => local,
            };
            world.push(m);
        }
        world
    }
}

/// Column-major 4x4 of a Granny transform: `T * R * S` (column vectors).
pub fn transform_matrix(t: &Transform) -> [f32; 16] {
    let [x, y, z, w] = t.orientation;
    let r = [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - z * w),
            2.0 * (x * z + y * w),
        ],
        [
            2.0 * (x * y + z * w),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - x * w),
        ],
        [
            2.0 * (x * z - y * w),
            2.0 * (y * z + x * w),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ];
    let s = &t.scale_shear;
    // linear = R * S, rows r, columns c
    let mut m = [0.0f32; 16];
    for row in 0..3 {
        for col in 0..3 {
            m[col * 4 + row] = (0..3).map(|k| r[row][k] * s[k][col]).sum();
        }
        m[12 + row] = t.position[row];
    }
    m[15] = 1.0;
    m
}

/// Column-major `a * b`.
pub fn mat_mul(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut m = [0.0f32; 16];
    for col in 0..4 {
        for row in 0..4 {
            m[col * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[col * 4 + k]).sum();
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bone(name: Option<&str>, id: Option<u32>) -> Bone {
        Bone {
            name: name.map(str::to_owned),
            name_id: id,
            parent: None,
            local: Transform {
                flags: 0,
                position: [0.0; 3],
                orientation: [0.0, 0.0, 0.0, 1.0],
                scale_shear: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            },
            inverse_world: [0.0; 16],
            lod_error: 0.0,
        }
    }

    #[test]
    fn an_id_finds_a_stripped_bone_and_a_named_one_by_its_hash() {
        let h = crate::names::name_hash("Bone_Root");
        let named = Skeleton {
            name: None,
            bones: vec![bone(Some("World"), None), bone(Some("Bone_Root"), None)],
        };
        assert_eq!(named.find_bone_by_id(h), Some(1));
        let stripped = Skeleton {
            name: None,
            bones: vec![bone(None, Some(7)), bone(None, Some(h))],
        };
        assert_eq!(stripped.find_bone_by_id(h), Some(1));
        assert_eq!(stripped.find_bone_by_id(9), None);
    }
}
