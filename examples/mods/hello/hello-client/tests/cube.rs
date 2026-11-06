//! The cube of `hello` on a real device (native; a test that finds no adapter says so and returns, as the renderer's
//! own GPU tests do). The system is built from a `ModGpu` made here with the formats of the main pass, prepared with
//! the terrain sampler and recorded into a pass the way `Renderer` records `MainSlot::Mod`.

use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use hello_client::{HEX, HelloCube, SHADER};
use render::frame::{
    DrawContext, MainSlot, ModGpu, PassSlot, PrepareContext, RenderSystem, SystemId, ViewGlobals,
    WorldServices,
};
use render::glam::{Mat4, Vec3};
use render::wgpu;
use render::world::terrain::{TerrainFrame, TerrainSampler};
use render::{CameraView, FirstPerson, FrameStats};

const SIZE: u32 = 64;
const COLOR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

struct Park(std::thread::Thread);
impl Wake for Park {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

fn block_on<F: Future>(fut: F) -> F::Output {
    let mut fut = std::pin::pin!(fut);
    let waker = Waker::from(Arc::new(Park(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::park();
    }
}

fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = match block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
    {
        Ok(a) => a,
        Err(e) => {
            eprintln!("SKIP hello cube: no adapter: {e}");
            return None;
        }
    };
    let limits = wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits());
    match block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("hello cube test"),
        required_limits: limits,
        ..Default::default()
    })) {
        Ok(d) => Some(d),
        Err(e) => {
            eprintln!("SKIP hello cube: no device: {e}");
            None
        }
    }
}

fn uniform_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("test"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    })
}

fn sampler() -> TerrainSampler {
    TerrainSampler::new(TerrainFrame::new(16, 10, true))
}

/// A camera above and in front of the cube, looking at its middle.
fn camera_for(ground: Vec3) -> FirstPerson {
    let eye = ground + Vec3::new(0.0, 50.0, 80.0);
    let dir = (ground + Vec3::new(0.0, 12.0, 0.0) - eye).normalize();
    let mut cam = FirstPerson::new(eye);
    cam.yaw = dir.x.atan2(-dir.z);
    cam.pitch = dir.y.asin();
    cam.near = 1.0;
    cam.far = 1000.0;
    cam
}

/// Draws the cube system into a 64x64 target after preparing it with `terrain`; returns the system and the pixels.
fn render(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    terrain: Option<&TerrainSampler>,
    view_proj: Mat4,
    eye: Vec3,
) -> (HelloCube, Vec<[u8; 4]>) {
    let view_layout = uniform_layout(device);
    let gpu = ModGpu {
        device: device.clone(),
        queue: queue.clone(),
        view_layout: view_layout.clone(),
        frame_layout: uniform_layout(device),
        color_format: COLOR,
        depth_format: DEPTH,
        samples: 1,
    };
    let mut cube = HelloCube::new(&gpu);
    assert_eq!(cube.id(), SystemId::Mod(0));
    assert_eq!(cube.passes(), &[PassSlot::Main(MainSlot::Mod)]);

    let services = WorldServices::default();
    let mut frame = None;
    cube.prepare(
        &mut PrepareContext::new(device, queue, &services, &mut frame).with_terrain(terrain),
    );

    // The view of the main pass: the engine's ViewGlobals.
    let g = ViewGlobals::new(view_proj, eye);
    let mut bytes = Vec::new();
    for v in g
        .view_proj
        .iter()
        .flatten()
        .chain(&g.eye)
        .chain(&g.copy_offset)
        .chain(&g.water_clip)
    {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    let view_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("view"),
        size: bytes.len() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&view_buffer, 0, &bytes);
    let view_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("view"),
        layout: &view_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: view_buffer.as_entire_binding(),
        }],
    });

    let extent = wgpu::Extent3d {
        width: SIZE,
        height: SIZE,
        depth_or_array_layers: 1,
    };
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("target"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COLOR,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("depth"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let target_view = target.create_view(&Default::default());
        let depth_view = depth.create_view(&Default::default());
        let mut pass = encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("main"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            })
            .forget_lifetime();
        let mut stats = FrameStats::default();
        let mut cx = DrawContext::new(&mut pass, &view_group, None, 0.0, &mut stats, &services)
            .with_terrain(terrain);
        cube.record(PassSlot::Main(MainSlot::Mod), &mut cx);
        // A slot that is not the cube's draws nothing.
        cube.record(PassSlot::Main(MainSlot::Unit), &mut cx);
    }
    let padded = (SIZE * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(padded * SIZE),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(SIZE),
            },
        },
        extent,
    );
    queue.submit([encoder.finish()]);
    readback.map_async(wgpu::MapMode::Read, .., |r| r.expect("map"));
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let data = readback.get_mapped_range(..).expect("range");
    let mut pixels = Vec::new();
    for y in 0..SIZE as usize {
        for x in 0..SIZE as usize {
            let at = y * padded as usize + x * 4;
            pixels.push([data[at], data[at + 1], data[at + 2], data[at + 3]]);
        }
    }
    (cube, pixels)
}

#[test]
fn the_shader_is_valid_wgsl() {
    // The parser and validator of the renderer's shader catalog (naga of wgpu).
    let module = render::wgpu::naga::front::wgsl::parse_str(SHADER).expect("parses");
    render::wgpu::naga::valid::Validator::new(
        render::wgpu::naga::valid::ValidationFlags::all(),
        render::wgpu::naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .expect("validates");
}

#[test]
fn the_cube_is_drawn_on_the_ground_and_only_there() {
    let Some((device, queue)) = device() else {
        return;
    };
    let terrain = sampler();
    let ground = terrain.hex_location(HEX.0, HEX.1);
    let cam = camera_for(ground);
    let (cube, pixels) = render(
        &device,
        &queue,
        Some(&terrain),
        cam.view_proj(1.0),
        cam.eye(),
    );
    assert!(cube.visible());
    let at = |x: usize, y: usize| pixels[y * SIZE as usize + x];
    let clear = [0, 0, 0, 255];
    assert_eq!(at(0, 0), clear, "the corner is the background");
    assert_eq!(at(SIZE as usize - 1, SIZE as usize - 1), clear);
    let lit = pixels.iter().filter(|p| **p != clear).count();
    assert!(
        lit > 150,
        "the cube covers a good part of the picture: {lit} pixels"
    );
    // The middle of the picture is the cube: orange (red over blue), shaded, opaque.
    let mid = at(SIZE as usize / 2, SIZE as usize / 2);
    assert!(mid[0] > mid[2] + 40 && mid[3] == 255, "{mid:?}");
}

#[test]
fn without_a_map_nothing_is_drawn() {
    let Some((device, queue)) = device() else {
        return;
    };
    let terrain = sampler();
    let cam = camera_for(terrain.hex_location(HEX.0, HEX.1));
    let (cube, pixels) = render(&device, &queue, None, cam.view_proj(1.0), cam.eye());
    assert!(!cube.visible());
    assert!(pixels.iter().all(|p| *p == [0, 0, 0, 255]));
}
