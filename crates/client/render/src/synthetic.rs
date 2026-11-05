//! Dev-only synthetic content: a skinned two-bone cylinder with a bending animation and a checker
//! texture. Lets the skinning pipeline run end to end before real converted assets exist.

use std::collections::HashMap;
use std::f32::consts::TAU;

use asset_format::{
    AnimEvent, AnimRef, AnimationBlob, Bone, Indices, LogicalId, Material, Mesh, ModelBlob,
    Skeleton, TextureBlob, TextureFormat, Track, Transform, f32_to_f16_bits, oct_encode, quat_pack,
};
use glam::{Mat4, Quat, Vec3};

pub const DEMO_TEXTURE_ID: &str = "tex:demo_checker";
pub const DEMO_ANIM_BEND: &str = "anim:demo_bend";
pub const DEMO_ANIM_TWIST: &str = "anim:demo_twist";

const RINGS: usize = 9;
const SEGMENTS: usize = 16;
const RADIUS: f32 = 0.3;
/// Height of the tube; the second bone sits halfway up.
const HEIGHT: f32 = 2.0;
const CLIP_SECONDS: f32 = 2.0;
const CLIP_RATE: f32 = 30.0;
const CHECKER_SIZE: u32 = 64;

/// Model, one clip per `model.animations` entry (same order) and the textures it references.
#[derive(Debug, Clone)]
pub struct SyntheticModel {
    pub model: ModelBlob,
    pub animations: Vec<AnimationBlob>,
    pub textures: HashMap<LogicalId, TextureBlob>,
}

pub fn skinned_cylinder() -> SyntheticModel {
    let skeleton = Skeleton {
        bones: vec![
            Bone {
                name: "root".into(),
                parent: None,
                rest: Transform::IDENTITY,
                inverse_bind: Mat4::IDENTITY.to_cols_array(),
            },
            Bone {
                name: "upper".into(),
                parent: Some(0),
                rest: Transform {
                    translation: [0.0, HEIGHT * 0.5, 0.0],
                    ..Transform::IDENTITY
                },
                inverse_bind: Mat4::from_translation(Vec3::new(0.0, -HEIGHT * 0.5, 0.0))
                    .to_cols_array(),
            },
        ],
    };
    let model = ModelBlob {
        skeleton,
        meshes: vec![tube(), pad()],
        materials: vec![
            Material {
                diffuse: Some(LogicalId::new(DEMO_TEXTURE_ID)),
                specular: None,
            },
            // No texture: exercises the white fallback.
            Material::default(),
        ],
        animations: vec![
            AnimRef {
                name: "demo_idle_bend".into(),
                blob: LogicalId::new(DEMO_ANIM_BEND),
                events: vec![AnimEvent { time: 0.0, code: 1 }],
            },
            AnimRef {
                name: "demo_twist".into(),
                blob: LogicalId::new(DEMO_ANIM_TWIST),
                events: vec![],
            },
        ],
    };
    let animations = vec![
        clip(|phase| {
            Quat::from_rotation_z(0.9 * (phase * TAU).sin())
                * Quat::from_rotation_x(0.35 * (phase * TAU).cos())
        }),
        clip(|phase| Quat::from_rotation_y(phase * TAU) * Quat::from_rotation_z(0.5)),
    ];
    let textures = HashMap::from([(LogicalId::new(DEMO_TEXTURE_ID), checker())]);
    SyntheticModel {
        model,
        animations,
        textures,
    }
}

/// Open tube along +Y, skinned: weight moves from the root to the upper bone over the middle half.
fn tube() -> Mesh {
    let mut m = Mesh {
        positions: vec![],
        normals: vec![],
        uvs: vec![],
        joints: vec![],
        weights: vec![],
        indices: Indices::U16(vec![]),
        material: 0,
    };
    for ring in 0..RINGS {
        let t = ring as f32 / (RINGS - 1) as f32;
        let y = t * HEIGHT;
        let blend = ((t - 0.25) * 2.0).clamp(0.0, 1.0);
        let w1 = (blend * blend * (3.0 - 2.0 * blend) * 255.0).round() as u8;
        for seg in 0..SEGMENTS {
            let a = seg as f32 / SEGMENTS as f32 * TAU;
            let (s, c) = a.sin_cos();
            m.positions.push([RADIUS * c, y, RADIUS * s]);
            m.normals.push(oct_encode([c, 0.0, s]));
            m.uvs.push([
                f32_to_f16_bits(seg as f32 / SEGMENTS as f32 * 4.0),
                f32_to_f16_bits(t * 4.0),
            ]);
            m.joints.push([0, 1, 0, 0]);
            m.weights.push([255 - w1, w1, 0, 0]);
        }
    }
    let mut idx = Vec::new();
    for ring in 0..RINGS - 1 {
        for seg in 0..SEGMENTS {
            let next = (seg + 1) % SEGMENTS;
            let at = |r: usize, s: usize| (r * SEGMENTS + s) as u16;
            let (a, b) = (at(ring, seg), at(ring, next));
            let (c, d) = (at(ring + 1, seg), at(ring + 1, next));
            // Counter-clockwise seen from outside.
            idx.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    m.indices = Indices::U16(idx);
    m
}

/// Rigid ground pad under the tube (no joints).
fn pad() -> Mesh {
    let s = 0.8;
    let up = oct_encode([0.0, 1.0, 0.0]);
    let uv = |u: f32, v: f32| [f32_to_f16_bits(u), f32_to_f16_bits(v)];
    Mesh {
        positions: vec![[-s, 0.02, -s], [-s, 0.02, s], [s, 0.02, s], [s, 0.02, -s]],
        normals: vec![up; 4],
        uvs: vec![uv(0.0, 0.0), uv(0.0, 1.0), uv(1.0, 1.0), uv(1.0, 0.0)],
        joints: vec![],
        weights: vec![],
        indices: Indices::U32(vec![0, 1, 2, 0, 2, 3]),
        material: 1,
    }
}

/// Clip with a rotation for the upper bone driven by `rotation(phase)`, phase in `[0, 1)`; the last
/// key repeats the first so the loop is seamless.
fn clip(rotation: impl Fn(f32) -> Quat) -> AnimationBlob {
    let frames = (CLIP_SECONDS * CLIP_RATE) as usize;
    let rotations = (0..=frames)
        .map(|k| quat_pack(rotation((k % frames) as f32 / frames as f32).to_array()))
        .collect();
    AnimationBlob {
        duration: CLIP_SECONDS,
        sample_rate: CLIP_RATE,
        tracks: vec![Track {
            bone: 1,
            rotations,
            translations: vec![],
            scales: vec![],
        }],
    }
}

/// 64×64 sRGB checker with a full mip chain (box-filtered).
fn checker() -> TextureBlob {
    let n = CHECKER_SIZE as usize;
    let mut level: Vec<u8> = Vec::with_capacity(n * n * 4);
    for y in 0..n {
        for x in 0..n {
            let on = (x / 8 + y / 8) % 2 == 0;
            level.extend_from_slice(if on {
                &[235, 190, 60, 255]
            } else {
                &[60, 90, 200, 255]
            });
        }
    }
    let mut mips = vec![level];
    let mut size = n;
    while size > 1 {
        let src = mips.last().expect("level 0 exists");
        let half = size / 2;
        let mut next = Vec::with_capacity(half * half * 4);
        for y in 0..half {
            for x in 0..half {
                for ch in 0..4 {
                    let at = |dx: usize, dy: usize| {
                        u32::from(src[((2 * y + dy) * size + 2 * x + dx) * 4 + ch])
                    };
                    next.push(((at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1) + 2) / 4) as u8);
                }
            }
        }
        mips.push(next);
        size = half;
    }
    TextureBlob {
        format: TextureFormat::Rgba8,
        dim: asset_format::TexDim::D2,
        srgb: true,
        width: CHECKER_SIZE,
        height: CHECKER_SIZE,
        depth: 1,
        mips,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blobs_roundtrip_and_validate() {
        let s = skinned_cylinder();
        assert_eq!(ModelBlob::decode(&s.model.encode()).unwrap(), s.model);
        for a in &s.animations {
            assert_eq!(AnimationBlob::decode(&a.encode()).unwrap(), *a);
        }
        for t in s.textures.values() {
            assert_eq!(TextureBlob::decode(&t.encode()).unwrap(), *t);
        }
        assert_eq!(s.model.animations.len(), s.animations.len());
    }

    #[test]
    fn tube_weights_sum_to_255_and_faces_point_outward() {
        let s = skinned_cylinder();
        let tube = &s.model.meshes[0];
        assert!(
            tube.weights
                .iter()
                .all(|w| w.iter().map(|&x| u32::from(x)).sum::<u32>() == 255)
        );
        let Indices::U16(idx) = &tube.indices else {
            panic!("tube uses u16 indices");
        };
        for tri in idx.chunks(3) {
            let p = |i: u16| Vec3::from(tube.positions[usize::from(i)]);
            let n = (p(tri[1]) - p(tri[0])).cross(p(tri[2]) - p(tri[0]));
            let radial = Vec3::new(p(tri[0]).x, 0.0, p(tri[0]).z);
            assert!(n.dot(radial) > 0.0, "triangle {tri:?} faces inward");
        }
    }
}
