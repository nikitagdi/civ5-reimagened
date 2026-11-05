//! glTF 2.0 binary (GLB) export of a decoded model, for inspection in Blender and friends.
//!
//! Works from the same blobs the client loads, so what you see is what the client gets. The
//! blobs are already Y up, right-handed; glTF needs no axis change. Skinned meshes ignore their
//! own node transform in glTF, so the model scale lives on the armature node that parents the
//! skeleton (joint world matrices include it).

use std::collections::BTreeMap;

use asset_format::{
    AnimationBlob, Indices, LogicalId, Mesh, ModelBlob, TextureBlob, TextureFormat,
    f16_bits_to_f32, oct_decode, quat_unpack,
};
use gltf_json as json;
use json::accessor::{ComponentType, GenericComponentType, Type};
use json::animation::{Channel, Interpolation, Property, Sampler, Target};
use json::buffer::Target as BufferTarget;
use json::mesh::{Mode, Primitive, Semantic};
use json::validation::Checked::Valid;
use json::{Index, Value};

use super::ModelError;
use super::png;

struct Builder {
    root: json::Root,
    bin: Vec<u8>,
}

fn f32_bytes(values: impl IntoIterator<Item = f32>) -> Vec<u8> {
    values.into_iter().flat_map(f32::to_le_bytes).collect()
}

fn floats(values: &[f32]) -> Value {
    Value::from(values.to_vec())
}

impl Builder {
    fn view(&mut self, bytes: &[u8], target: Option<BufferTarget>) -> Index<json::buffer::View> {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let offset = self.bin.len();
        self.bin.extend_from_slice(bytes);
        self.root.push(json::buffer::View {
            buffer: Index::new(0),
            byte_length: bytes.len().into(),
            byte_offset: Some(offset.into()),
            byte_stride: None,
            target: target.map(Valid),
            extensions: None,
            extras: Default::default(),
            name: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn accessor(
        &mut self,
        bytes: &[u8],
        target: Option<BufferTarget>,
        count: usize,
        component: ComponentType,
        kind: Type,
        min_max: Option<(Value, Value)>,
        normalized: bool,
    ) -> Index<json::Accessor> {
        let view = self.view(bytes, target);
        let (min, max) = min_max.map_or((None, None), |(a, b)| (Some(a), Some(b)));
        self.root.push(json::Accessor {
            buffer_view: Some(view),
            byte_offset: None,
            count: count.into(),
            component_type: Valid(GenericComponentType(component)),
            extensions: None,
            extras: Default::default(),
            type_: Valid(kind),
            min,
            max,
            name: None,
            normalized,
            sparse: None,
        })
    }

    fn vec3(
        &mut self,
        data: &[[f32; 3]],
        target: Option<BufferTarget>,
        bounds: bool,
    ) -> Index<json::Accessor> {
        let bytes = f32_bytes(data.iter().flatten().copied());
        let min_max = bounds.then(|| {
            let mut lo = [f32::INFINITY; 3];
            let mut hi = [f32::NEG_INFINITY; 3];
            for p in data {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
            (floats(&lo), floats(&hi))
        });
        self.accessor(
            &bytes,
            target,
            data.len(),
            ComponentType::F32,
            Type::Vec3,
            min_max,
            false,
        )
    }
}

fn node(name: String) -> json::Node {
    json::Node {
        name: Some(name),
        ..Default::default()
    }
}

fn add_skeleton(
    b: &mut Builder,
    model: &ModelBlob,
    root_scale: f32,
) -> (Index<json::Node>, Option<Index<json::Skin>>) {
    let bones = &model.skeleton.bones;
    let mut children: Vec<Vec<Index<json::Node>>> = vec![Vec::new(); bones.len()];
    let mut roots = Vec::new();
    for (i, bone) in bones.iter().enumerate() {
        match bone.parent {
            Some(p) => children[usize::from(p)].push(Index::new(i as u32)),
            None => roots.push(Index::new(i as u32)),
        }
    }
    let mut joints = Vec::with_capacity(bones.len());
    for (i, bone) in bones.iter().enumerate() {
        let r = &bone.rest;
        let kids = std::mem::take(&mut children[i]);
        let idx = b.root.push(json::Node {
            children: (!kids.is_empty()).then_some(kids),
            translation: Some(r.translation),
            rotation: Some(json::scene::UnitQuaternion(r.rotation)),
            scale: Some(r.scale),
            ..node(bone.name.clone())
        });
        joints.push(idx);
    }
    let armature = b.root.push(json::Node {
        children: Some(roots),
        scale: Some([root_scale; 3]),
        ..node("armature".into())
    });
    if bones.is_empty() {
        // Static models (trees, cities) have no skeleton; glTF forbids a skin without joints.
        return (armature, None);
    }
    let ibm: Vec<f32> = bones.iter().flat_map(|bone| bone.inverse_bind).collect();
    let ibm = b.accessor(
        &f32_bytes(ibm),
        None,
        bones.len(),
        ComponentType::F32,
        Type::Mat4,
        None,
        false,
    );
    let skin = b.root.push(json::Skin {
        extensions: None,
        extras: Default::default(),
        inverse_bind_matrices: Some(ibm),
        joints,
        name: Some("skin".into()),
        skeleton: Some(armature),
    });
    (armature, Some(skin))
}

fn texture_png(t: &TextureBlob) -> Result<Vec<u8>, ModelError> {
    if t.format != TextureFormat::Rgba8 {
        return Err(ModelError::Gltf("diffuse texture must be RGBA8".into()));
    }
    let mip0 = t
        .mips
        .first()
        .ok_or_else(|| ModelError::Gltf("texture has no mips".into()))?;
    png::encode_rgba8(t.width, t.height, mip0).map_err(ModelError::Gltf)
}

fn add_mesh(
    b: &mut Builder,
    meshes: &[&Mesh],
    materials: &[Option<Index<json::Material>>],
    name: &str,
) -> Index<json::Mesh> {
    let mut primitives = Vec::new();
    for mesh in meshes {
        let mut attributes = BTreeMap::new();
        attributes.insert(
            Valid(Semantic::Positions),
            b.vec3(&mesh.positions, Some(BufferTarget::ArrayBuffer), true),
        );
        let normals: Vec<[f32; 3]> = mesh.normals.iter().map(|&n| oct_decode(n)).collect();
        attributes.insert(
            Valid(Semantic::Normals),
            b.vec3(&normals, Some(BufferTarget::ArrayBuffer), false),
        );
        let uvs = f32_bytes(mesh.uvs.iter().flatten().map(|&h| f16_bits_to_f32(h)));
        attributes.insert(
            Valid(Semantic::TexCoords(0)),
            b.accessor(
                &uvs,
                Some(BufferTarget::ArrayBuffer),
                mesh.uvs.len(),
                ComponentType::F32,
                Type::Vec2,
                None,
                false,
            ),
        );
        if !mesh.joints.is_empty() {
            let joints: Vec<u8> = mesh.joints.iter().flatten().copied().collect();
            attributes.insert(
                Valid(Semantic::Joints(0)),
                b.accessor(
                    &joints,
                    Some(BufferTarget::ArrayBuffer),
                    mesh.joints.len(),
                    ComponentType::U8,
                    Type::Vec4,
                    None,
                    false,
                ),
            );
            let weights: Vec<u8> = mesh.weights.iter().flatten().copied().collect();
            attributes.insert(
                Valid(Semantic::Weights(0)),
                b.accessor(
                    &weights,
                    Some(BufferTarget::ArrayBuffer),
                    mesh.weights.len(),
                    ComponentType::U8,
                    Type::Vec4,
                    None,
                    true,
                ),
            );
        }
        let (bytes, component) = match &mesh.indices {
            Indices::U16(v) => (
                v.iter().flat_map(|i| i.to_le_bytes()).collect::<Vec<u8>>(),
                ComponentType::U16,
            ),
            Indices::U32(v) => (
                v.iter().flat_map(|i| i.to_le_bytes()).collect(),
                ComponentType::U32,
            ),
        };
        let indices = b.accessor(
            &bytes,
            Some(BufferTarget::ElementArrayBuffer),
            mesh.indices.len(),
            component,
            Type::Scalar,
            None,
            false,
        );
        primitives.push(Primitive {
            attributes,
            extensions: None,
            extras: Default::default(),
            indices: Some(indices),
            material: materials.get(usize::from(mesh.material)).copied().flatten(),
            mode: Valid(Mode::Triangles),
            targets: None,
        });
    }
    b.root.push(json::Mesh {
        extensions: None,
        extras: Default::default(),
        name: Some(name.into()),
        primitives,
        weights: None,
    })
}

/// Input accessor holding `count` key times at `rate`.
fn times_accessor(b: &mut Builder, count: usize, rate: f32) -> Index<json::Accessor> {
    let times: Vec<f32> = (0..count).map(|i| i as f32 / rate).collect();
    let last = times.last().copied().unwrap_or(0.0);
    b.accessor(
        &f32_bytes(times),
        None,
        count,
        ComponentType::F32,
        Type::Scalar,
        Some((floats(&[0.0]), floats(&[last]))),
        false,
    )
}

fn add_animation(b: &mut Builder, name: &str, clip: &AnimationBlob) -> json::Animation {
    let mut samplers = Vec::new();
    let mut channels = Vec::new();
    let mut times: BTreeMap<usize, Index<json::Accessor>> = BTreeMap::new();
    let mut channel =
        |b: &mut Builder, node: u16, path: Property, count: usize, kind: Type, out: Vec<f32>| {
            let input = *times
                .entry(count)
                .or_insert_with(|| times_accessor(b, count, clip.sample_rate));
            let output = b.accessor(
                &f32_bytes(out),
                None,
                count,
                ComponentType::F32,
                kind,
                None,
                false,
            );
            let sampler = Index::new(samplers.len() as u32);
            samplers.push(Sampler {
                extensions: None,
                extras: Default::default(),
                input,
                interpolation: Valid(Interpolation::Linear),
                output,
            });
            channels.push(Channel {
                sampler,
                target: Target {
                    extensions: None,
                    extras: Default::default(),
                    node: Index::new(u32::from(node)),
                    path: Valid(path),
                },
                extensions: None,
                extras: Default::default(),
            });
        };
    for track in &clip.tracks {
        if !track.translations.is_empty() {
            let out = track.translations.iter().flatten().copied().collect();
            channel(
                b,
                track.bone,
                Property::Translation,
                track.translations.len(),
                Type::Vec3,
                out,
            );
        }
        if !track.rotations.is_empty() {
            let out = track
                .rotations
                .iter()
                .flat_map(|&q| quat_unpack(q))
                .collect();
            channel(
                b,
                track.bone,
                Property::Rotation,
                track.rotations.len(),
                Type::Vec4,
                out,
            );
        }
        if !track.scales.is_empty() {
            let out = track.scales.iter().flatten().copied().collect();
            channel(
                b,
                track.bone,
                Property::Scale,
                track.scales.len(),
                Type::Vec3,
                out,
            );
        }
    }
    json::Animation {
        extensions: None,
        extras: Default::default(),
        channels,
        name: Some(name.to_owned()),
        samplers,
    }
}

fn add_texture(
    b: &mut Builder,
    name: &str,
    t: &TextureBlob,
) -> Result<Index<json::Texture>, ModelError> {
    let png = texture_png(t)?;
    let view = b.view(&png, None);
    let image = b.root.push(json::Image {
        buffer_view: Some(view),
        mime_type: Some(json::image::MimeType("image/png".into())),
        uri: None,
        name: Some(name.to_owned()),
        extensions: None,
        extras: Default::default(),
    });
    let sampler = b.root.push(json::texture::Sampler {
        mag_filter: Some(Valid(json::texture::MagFilter::Linear)),
        min_filter: Some(Valid(json::texture::MinFilter::LinearMipmapLinear)),
        ..Default::default()
    });
    Ok(b.root.push(json::Texture {
        name: Some(name.to_owned()),
        sampler: Some(sampler),
        source: image,
        extensions: None,
        extras: Default::default(),
    }))
}

/// Builds a GLB. `clips` pairs each name with its decoded blob; `textures` are RGBA8 diffuse
/// textures by logical id, applied to the materials whose diffuse map has that id;
/// `root_scale` scales the armature. Skinned and rigid meshes become two glTF meshes (a skinned
/// node needs joints on every primitive); the rigid one hangs under the armature node so it
/// shares the scale.
pub fn write_glb(
    model: &ModelBlob,
    clips: &[(String, AnimationBlob)],
    textures: &[(LogicalId, TextureBlob)],
    root_scale: f32,
) -> Result<Vec<u8>, ModelError> {
    let mut b = Builder {
        root: json::Root {
            asset: json::Asset {
                generator: Some("civ5-assets".into()),
                version: "2.0".into(),
                ..Default::default()
            },
            ..Default::default()
        },
        bin: Vec::new(),
    };

    let (armature, skin) = add_skeleton(&mut b, model, root_scale);

    let mut by_id: Vec<(&LogicalId, Index<json::Texture>)> = Vec::new();
    for (id, t) in textures {
        if !by_id.iter().any(|(i, _)| *i == id) {
            let index = add_texture(&mut b, id.as_str(), t)?;
            by_id.push((id, index));
        }
    }
    let materials: Vec<Option<Index<json::Material>>> = model
        .materials
        .iter()
        .map(|m| {
            let tex = m
                .diffuse
                .as_ref()
                .and_then(|d| by_id.iter().find(|(i, _)| *i == d).map(|e| e.1));
            Some(b.root.push(json::Material {
                name: m.diffuse.as_ref().map(|d| d.as_str().to_owned()),
                pbr_metallic_roughness: json::material::PbrMetallicRoughness {
                    base_color_texture: tex.map(|index| json::texture::Info {
                        index,
                        tex_coord: 0,
                        extensions: None,
                        extras: Default::default(),
                    }),
                    metallic_factor: json::material::StrengthFactor(0.0),
                    roughness_factor: json::material::StrengthFactor(1.0),
                    ..Default::default()
                },
                alpha_mode: Valid(json::material::AlphaMode::Opaque),
                ..Default::default()
            }))
        })
        .collect();

    let mut scene_nodes = vec![armature];
    let skinned: Vec<&Mesh> = model
        .meshes
        .iter()
        .filter(|m| !m.joints.is_empty())
        .collect();
    let rigid: Vec<&Mesh> = model
        .meshes
        .iter()
        .filter(|m| m.joints.is_empty())
        .collect();
    if !skinned.is_empty() {
        let mesh = add_mesh(&mut b, &skinned, &materials, "mesh");
        scene_nodes.push(b.root.push(json::Node {
            mesh: Some(mesh),
            skin,
            ..node("mesh".into())
        }));
    }
    if !rigid.is_empty() {
        let mesh = add_mesh(&mut b, &rigid, &materials, "rigid");
        let n = b.root.push(json::Node {
            mesh: Some(mesh),
            ..node("rigid".into())
        });
        let arm = &mut b.root.nodes[armature.value()];
        arm.children.get_or_insert_with(Vec::new).push(n);
    }
    for (name, clip) in clips {
        let anim = add_animation(&mut b, name, clip);
        b.root.push(anim);
    }
    let scene = b.root.push(json::Scene {
        extensions: None,
        extras: Default::default(),
        name: Some("scene".into()),
        nodes: scene_nodes,
    });
    b.root.scene = Some(scene);

    while !b.bin.len().is_multiple_of(4) {
        b.bin.push(0);
    }
    b.root.push(json::Buffer {
        byte_length: b.bin.len().into(),
        uri: None,
        name: None,
        extensions: None,
        extras: Default::default(),
    });

    let mut json_bytes = b
        .root
        .to_vec()
        .map_err(|e| ModelError::Gltf(e.to_string()))?;
    while !json_bytes.len().is_multiple_of(4) {
        json_bytes.push(b' ');
    }
    let total = 12 + 8 + json_bytes.len() + 8 + b.bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&json_bytes);
    out.extend_from_slice(&(b.bin.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&b.bin);
    Ok(out)
}
