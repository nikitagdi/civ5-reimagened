//! Typed contents of a gr2 file: skeletons, meshes, models, animations and materials.

use crate::animation::Animation;
use crate::material::Material;
use crate::mesh::Mesh;
use crate::skeleton::Skeleton;
use crate::{GrannyError, GrannyFile, Object, Ptr, Transform};

/// A model: a skeleton plus the meshes drawn with it.
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub name: Option<String>,
    /// Index into `Scene::skeletons`.
    pub skeleton: Option<usize>,
    pub initial_placement: Transform,
    /// Indices into `Scene::meshes`.
    pub meshes: Vec<usize>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scene {
    pub skeletons: Vec<Skeleton>,
    pub meshes: Vec<Mesh>,
    pub models: Vec<Model>,
    pub animations: Vec<Animation>,
    /// The file's `Materials`, indexed by [`Mesh::material_bindings`].
    pub materials: Vec<Material>,
}

fn refs<'f>(root: &Object<'f>, field: &str) -> Result<Vec<Object<'f>>, GrannyError> {
    let mut out = Vec::new();
    let arr = match root.references(field) {
        Ok(a) => a,
        Err(GrannyError::MissingField(_)) => return Ok(out),
        Err(e) => return Err(e),
    };
    for o in arr.iter()? {
        if let Some(o) = o? {
            out.push(o);
        }
    }
    Ok(out)
}

fn index_of(objs: &[Object<'_>], target: Option<Ptr>) -> Option<usize> {
    let target = target?;
    objs.iter().position(|o| o.at() == target)
}

impl Scene {
    /// Decodes every skeleton, mesh, model and animation of `file`.
    pub fn read(file: &GrannyFile<'_>) -> Result<Scene, GrannyError> {
        let root = file.root()?;
        let skel_objs = refs(&root, "Skeletons")?;
        let mesh_objs = refs(&root, "Meshes")?;
        let model_objs = refs(&root, "Models")?;
        let material_objs = refs(&root, "Materials")?;

        let skeletons = skel_objs
            .iter()
            .map(Skeleton::read)
            .collect::<Result<Vec<_>, _>>()?;

        let mut models = Vec::new();
        for m in &model_objs {
            let skel = m.reference("Skeleton")?;
            let mut meshes = Vec::new();
            for b in m.array("MeshBindings")?.iter() {
                if let Some(i) = index_of(&mesh_objs, b.reference("Mesh")?.map(|o| o.at())) {
                    meshes.push(i);
                }
            }
            models.push(Model {
                name: m.string("Name")?,
                skeleton: index_of(&skel_objs, skel.map(|o| o.at())),
                initial_placement: m.transform("InitialPlacement")?,
                meshes,
            });
        }

        // Bone-binding names resolve against the skeleton of the model that uses the mesh.
        let mut meshes = Vec::new();
        for (mi, mo) in mesh_objs.iter().enumerate() {
            let skel = models
                .iter()
                .find(|m| m.meshes.contains(&mi))
                .and_then(|m| m.skeleton)
                .and_then(|s| skeletons.get(s));
            let lookup = |name: Option<&str>, id: Option<u32>| {
                let s = skel?;
                name.and_then(|n| s.find_bone(n))
                    .or_else(|| id.and_then(|i| s.find_bone_by_id(i)))
            };
            meshes.push(Mesh::read(mo, &material_objs, &lookup)?);
        }

        let mut animations = Vec::new();
        for a in refs(&root, "Animations")? {
            animations.push(Animation::read(&a)?);
        }
        let materials = material_objs
            .iter()
            .map(Material::read)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Scene {
            skeletons,
            meshes,
            models,
            animations,
            materials,
        })
    }
}
