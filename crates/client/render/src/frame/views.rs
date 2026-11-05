//! Views: the constants of group 0 per camera and map copy. A copy
//! of the wrapped map is a view with its own `ViewGlobals`, whose matrix is shifted by the map
//! width; subsystems draw as if there were one map and never see the shift.

use crate::perf::{Buf, Cluster, DeviceTrack};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};

use crate::wrap;

/// Left, middle and right copy: the most a screen can reach.
pub const MAX_MAP_COPIES: usize = 3;

/// The rotation `B` of `ours_to_orig`: our (x, y, z) (X east, Y up, Z south) becomes
/// the original's (x, -z, y) (x east, y north, z up). It is `ours_to_orig` of a map without a
/// frame yet.
pub fn basis_rotation() -> Mat4 {
    use glam::Vec4;
    Mat4::from_cols(
        Vec4::new(1.0, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0, 0.0),
        Vec4::new(0.0, -1.0, 0.0, 0.0),
        Vec4::W,
    )
}

/// Group 0, binding 0 for one view. Our basis (X east, Y up, Z south); the move to the original's
/// basis comes with `FrameGlobals` and `ours_to_orig`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct ViewGlobals {
    pub view_proj: [[f32; 4]; 4],
    /// Eye in the object space of this copy: the real eye minus the shift.
    pub eye: [f32; 4],
    /// `(shift, 0, 0, 0)`: where this copy lies relative to the middle one.
    pub copy_offset: [f32; 4],
    /// `(height, on, 0, 0)`: the plane the reflection pass cuts at, in the original's basis (z up).
    /// With `on` 0 (every other view) nothing is cut. Read by `below_water_clip` of `frame.wgsl`.
    pub water_clip: [f32; 4],
}

impl ViewGlobals {
    pub fn new(view_proj: Mat4, eye: Vec3) -> Self {
        Self {
            view_proj: view_proj.to_cols_array_2d(),
            eye: eye.extend(1.0).to_array(),
            copy_offset: [0.0; 4],
            water_clip: [0.0; 4],
        }
    }

    /// The same view with the clip plane at `height` switched on (the reflection pass).
    pub fn with_water_clip(mut self, height: f32) -> Self {
        self.water_clip = [height, 1.0, 0.0, 0.0];
        self
    }

    /// The same view for the copy of the map lying `shift` along X. A point `p` of the map,
    /// drawn unshifted, lands where `p + shift` lands in `self`.
    pub fn for_copy(&self, shift: f32) -> Self {
        let vp = Mat4::from_cols_array_2d(&self.view_proj)
            * Mat4::from_translation(Vec3::new(shift, 0.0, 0.0));
        Self {
            view_proj: vp.to_cols_array_2d(),
            eye: [self.eye[0] - shift, self.eye[1], self.eye[2], self.eye[3]],
            copy_offset: [self.copy_offset[0] + shift, 0.0, 0.0, 0.0],
            water_clip: self.water_clip,
        }
    }
}

/// X shifts of the map copies that reach the screen, middle copy first. A map of zero width does
/// not wrap.
pub fn copy_shifts(map_width: f32, view_proj: &Mat4) -> Vec<f32> {
    if map_width <= 0.0 {
        return vec![0.0];
    }
    let range = wrap::visible_x_range(view_proj);
    [0.0, -map_width, map_width]
        .into_iter()
        .filter(|&s| wrap::copy_visible(s, map_width, range))
        .collect()
}

/// Uniform buffers and group 0 bind groups for up to [`MAX_MAP_COPIES`] copies of one view kind
/// (main camera, shadow, reflection).
pub struct ViewBuffers {
    layout: wgpu::BindGroupLayout,
    slots: Vec<(Buf, wgpu::BindGroup)>,
}

impl ViewBuffers {
    pub fn layout_desc() -> wgpu::BindGroupLayoutDescriptor<'static> {
        wgpu::BindGroupLayoutDescriptor {
            label: Some("view-globals-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        }
    }

    pub fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&Self::layout_desc());
        let slots = (0..MAX_MAP_COPIES)
            .map(|_| {
                let buffer = device.buf(
                    &wgpu::BufferDescriptor {
                        label: Some("view-globals"),
                        size: std::mem::size_of::<ViewGlobals>() as u64,
                        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    },
                    Cluster::Other,
                );
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("view-globals"),
                    layout: &layout,
                    entries: &[wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffer.as_entire_binding(),
                    }],
                });
                (buffer, group)
            })
            .collect();
        Self { layout, slots }
    }

    pub fn layout(&self) -> &wgpu::BindGroupLayout {
        &self.layout
    }

    /// Writes the views of the copies, `shifts[i]` into slot `i`. Extra shifts are dropped.
    pub fn write(&self, queue: &wgpu::Queue, base: &ViewGlobals, shifts: &[f32]) {
        for ((buffer, _), &shift) in self.slots.iter().zip(shifts) {
            queue.write_buffer(buffer, 0, bytemuck::bytes_of(&base.for_copy(shift)));
        }
    }

    pub fn bind_group(&self, copy: usize) -> &wgpu::BindGroup {
        &self.slots[copy].1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_camera::GameCamera;
    use glam::{Vec2, Vec4};

    const W: f32 = 64.0 * 40.0;

    fn project(vg: &ViewGlobals, p: Vec3) -> Vec4 {
        Mat4::from_cols_array_2d(&vg.view_proj) * p.extend(1.0)
    }

    #[test]
    fn copy_offset_shifts_the_matrix_by_the_map_width() {
        let cam = GameCamera::new(Vec2::new(300.0, -200.0));
        let base = ViewGlobals::new(cam.view_proj(1.5), Vec3::new(300.0, 900.0, -200.0));
        let p = Vec3::new(120.0, 4.0, -50.0);
        for shift in [W, -W] {
            let copy = base.for_copy(shift);
            let want = project(&base, p + Vec3::new(shift, 0.0, 0.0));
            let got = project(&copy, p);
            assert!((want - got).abs().max_element() < 1e-2, "{want} {got}");
            assert_eq!(copy.copy_offset[0], shift);
            assert_eq!(copy.eye[0], base.eye[0] - shift);
            assert_eq!(copy.eye[1..], base.eye[1..]);
        }
        assert_eq!(base.for_copy(0.0), base);
    }

    #[test]
    fn the_basis_rotation_is_x_minus_z_y_with_determinant_one() {
        let b = basis_rotation();
        assert!((b.determinant() - 1.0).abs() < 1e-6);
        assert_eq!(
            b.transform_vector3(Vec3::new(1.0, 2.0, 3.0)),
            Vec3::new(1.0, -3.0, 2.0)
        );
        // Up in our basis is up in the original's; south is minus north.
        assert_eq!(b.transform_vector3(Vec3::Y), Vec3::Z);
        assert_eq!(b.transform_vector3(Vec3::Z), -Vec3::Y);
    }

    #[test]
    fn view_globals_size_is_whole_vec4s() {
        assert_eq!(std::mem::size_of::<ViewGlobals>(), 112);
    }

    #[test]
    fn copies_follow_the_camera_across_the_seam() {
        let shifts = |fx: f32| {
            let mut c = GameCamera::new(Vec2::new(fx, -400.0));
            c.set_level(20.0);
            copy_shifts(W, &c.view_proj(16.0 / 9.0))
        };
        assert_eq!(shifts(W * 0.5), [0.0]);
        assert_eq!(shifts(40.0), [0.0, -W]);
        assert_eq!(shifts(W - 40.0), [0.0, W]);
        assert_eq!(copy_shifts(0.0, &Mat4::IDENTITY), [0.0]);
    }
}
