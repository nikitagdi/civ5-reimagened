//! What the overlay subsystems need from B1's pipelines while they still draw with them (WOV0:
//! the code of B1 moved into `FluidFow`, `Overlay` and `MovementPath` without a change of the
//! picture). `Renderer` makes it once, after the device exists, and hands a clone to each of them
//! (`bind`); the handles are cheap to clone. The `world-overlays` passes of the original replace
//! the B1 pipelines one by one (WOV1, WOV2), and this file shrinks with them.

use crate::perf::{Cluster, DeviceTrack};
use glam::{Mat4, Vec3};

use crate::blend::BlendPipeline;
use crate::renderer::ObjectUniform;
use crate::terrain::TerrainSnapshot;

#[derive(Clone)]
pub(crate) struct B1Env {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    /// Group 1 of the B1 pipelines: the model matrix of a map copy.
    pub object_layout: wgpu::BindGroupLayout,
    /// The blend pipeline (rivers, borders, grid, highlights, path) and the layout of the fog-state
    /// texture (group 2 of every B1 pipeline).
    pub blend: BlendPipeline,
    /// Samples of the main pass.
    pub samples: u32,
}

/// X shifts of the copies of a map: one on a flat map, three (the map and one to each side) on a
/// wrapping one. The same as the terrain's, so a copy of the map and a copy of what lies on it
/// match.
pub(crate) fn map_shifts(snapshot: &TerrainSnapshot) -> Vec<f32> {
    if snapshot.wrap_x() {
        let w = snapshot.shape().world_width();
        vec![0.0, -w, w]
    } else {
        vec![0.0]
    }
}

impl B1Env {
    /// One object group per shift (`map_shifts`).
    pub fn copy_groups(&self, shifts: &[f32]) -> Vec<(f32, wgpu::BindGroup)> {
        shifts
            .iter()
            .map(|&dx| {
                let uniform = ObjectUniform {
                    model: Mat4::from_translation(Vec3::new(dx, 0.0, 0.0)).to_cols_array_2d(),
                    tint: [1.0, 1.0, 1.0, 0.0],
                };
                let buffer = self.device.buf_init(
                    &wgpu::util::BufferInitDescriptor {
                        label: Some("overlay object"),
                        contents: bytemuck::bytes_of(&uniform),
                        usage: wgpu::BufferUsages::UNIFORM,
                    },
                    Cluster::Other,
                );
                let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("overlay object"),
                    layout: &self.object_layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffer.as_entire_binding(),
                    }],
                });
                (dx, group)
            })
            .collect()
    }
}

/// The group of the copy shifted by `shift`.
pub(crate) fn copy_group(
    copies: &[(f32, wgpu::BindGroup)],
    shift: f32,
) -> Option<&wgpu::BindGroup> {
    copies.iter().find(|(s, _)| *s == shift).map(|(_, g)| g)
}
