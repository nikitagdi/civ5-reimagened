//! Preparing the target of a replay and reading it back: the colour target is
//! filled with the captured `before`, the depth and stencil planes by full-screen passes, textures
//! are uploaded in their captured formats.

use std::collections::BTreeSet;

use anyhow::{Context, Result, bail};

use super::ReplayGpu;
use super::inputs::{Dds, Fmt, Kind};

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth24PlusStencil8;

/// Uploads every image of `dds` (all layers and mips) as a sampled texture.
pub fn upload_texture(
    gpu: &ReplayGpu,
    dds: &Dds,
    format: wgpu::TextureFormat,
    label: &str,
) -> Result<wgpu::Texture> {
    let f = &dds.fmt;
    let is_3d = dds.depth > 1;
    let (dimension, depth_or_layers) = if is_3d {
        (wgpu::TextureDimension::D3, dds.depth)
    } else {
        (wgpu::TextureDimension::D2, dds.layers)
    };
    let (bw, bh) = f.block;
    let tex = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: dds.width.next_multiple_of(bw),
            height: dds.height.next_multiple_of(bh),
            depth_or_array_layers: depth_or_layers,
        },
        mip_level_count: dds.mips,
        sample_count: 1,
        dimension,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (layer, mips) in dds.images.iter().enumerate() {
        for (m, data) in mips.iter().enumerate() {
            let (row, rows, slices) =
                super::inputs::mip_layout(f, dds.width, dds.height, dds.depth, m as u32);
            let (mw, mh) = ((dds.width >> m).max(1), (dds.height >> m).max(1));
            gpu.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &tex,
                    mip_level: m as u32,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: if is_3d { 0 } else { layer as u32 },
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(rows),
                },
                wgpu::Extent3d {
                    width: mw.next_multiple_of(bw),
                    height: mh.next_multiple_of(bh),
                    depth_or_array_layers: slices,
                },
            );
        }
    }
    Ok(tex)
}

/// Writes tightly packed `bytes` of format `f` into mip 0 of `tex`.
pub fn write_pixels(
    gpu: &ReplayGpu,
    tex: &wgpu::Texture,
    f: &Fmt,
    w: u32,
    h: u32,
    bytes: &[u8],
) -> Result<()> {
    let need = (w * h * f.bytes) as usize;
    if bytes.len() < need {
        bail!(
            "{} bytes of pixels, {w}x{h} of {:?} needs {need}",
            bytes.len(),
            f.wgpu
        );
    }
    gpu.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &bytes[..need],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * f.bytes),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    Ok(())
}

/// The bytes of mip 0 of a colour texture, tightly packed.
pub fn read_pixels(
    gpu: &ReplayGpu,
    tex: &wgpu::Texture,
    f: &Fmt,
    w: u32,
    h: u32,
) -> Result<Vec<u8>> {
    let row = w * f.bytes;
    let padded = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let size = u64::from(padded) * u64::from(h);
    let buf = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("replay readback"),
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit([enc.finish()]);
    buf.map_async(wgpu::MapMode::Read, .., |r| r.expect("map"));
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| anyhow::anyhow!("poll: {e:?}"))?;
    let mapped = buf.get_mapped_range(..).context("mapped range")?;
    let mut out = Vec::with_capacity((row * h) as usize);
    for y in 0..h as usize {
        out.extend_from_slice(&mapped[y * padded as usize..y * padded as usize + row as usize]);
    }
    Ok(out)
}

const PREFILL_WGSL: &str = r"
@group(0) @binding(0) var packed: texture_2d<u32>;
struct Stencil { value: u32, }
@group(0) @binding(1) var<uniform> stencil: Stencil;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

struct DepthOut { @builtin(frag_depth) d: f32, }

// D24S8: depth in the low 24 bits, stencil in the high 8.
@fragment
fn fs_depth(@builtin(position) p: vec4<f32>) -> DepthOut {
    let w = textureLoad(packed, vec2<i32>(p.xy), 0).x;
    return DepthOut(f32(w & 0xffffffu) / 16777215.0);
}

@fragment
fn fs_stencil(@builtin(position) p: vec4<f32>) {
    let w = textureLoad(packed, vec2<i32>(p.xy), 0).x;
    if (w >> 24u) != stencil.value {
        discard;
    }
}
";

/// Fills the depth plane of `depth` (`samples` samples a pixel, all get the value) from the
/// captured D24S8 words (`packed`, one `u32` per pixel) with a full-screen pass writing
/// `frag_depth`; with `stencil` the stencil plane is filled with one pass per value that occurs
/// (stencil reference, the pixels of other values are discarded).
pub fn prefill_depth(
    gpu: &ReplayGpu,
    depth: &wgpu::Texture,
    packed: &[u8],
    w: u32,
    h: u32,
    stencil: bool,
    samples: u32,
) -> Result<()> {
    let need = (w * h * 4) as usize;
    if packed.len() < need {
        bail!("depth data is {} bytes, {w}x{h} needs {need}", packed.len());
    }
    let words: Vec<u32> = packed[..need]
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes(c.try_into().expect("4")))
        .collect();
    let tex = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("replay depth words"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R32Uint,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    gpu.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &packed[..need],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * 4),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    let module = gpu
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("replay prefill"),
            source: wgpu::ShaderSource::Wgsl(PREFILL_WGSL.into()),
        });
    let bgl = gpu
        .device
        .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("replay prefill"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
    let layout = gpu
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("replay prefill"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
    let always = wgpu::StencilFaceState {
        compare: wgpu::CompareFunction::Always,
        fail_op: wgpu::StencilOperation::Replace,
        depth_fail_op: wgpu::StencilOperation::Replace,
        pass_op: wgpu::StencilOperation::Replace,
    };
    let make = |entry: &str, depth_write: bool, st: Option<wgpu::StencilFaceState>| {
        gpu.device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("replay prefill"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(depth_write),
                    depth_compare: Some(wgpu::CompareFunction::Always),
                    stencil: match st {
                        Some(f) => wgpu::StencilState {
                            front: f,
                            back: f,
                            read_mask: 0xff,
                            write_mask: 0xff,
                        },
                        None => wgpu::StencilState::default(),
                    },
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: samples,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[],
                }),
                multiview_mask: None,
                cache: None,
            })
    };
    let depth_pipeline = make("fs_depth", true, None);
    let stencil_pipeline = make("fs_stencil", false, Some(always));
    let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
    let values: BTreeSet<u32> = if stencil {
        words.iter().map(|w| w >> 24).collect()
    } else {
        BTreeSet::new()
    };
    let bind = |value: u32| {
        let buf = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("replay stencil value"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        gpu.queue.write_buffer(&buf, 0, &value.to_le_bytes());
        let bg = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("replay prefill"),
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: buf.as_entire_binding(),
                },
            ],
        });
        (buf, bg)
    };
    let mut enc = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    let mut keep = Vec::new();
    // Depth, and a stencil of zero everywhere first.
    let passes: Vec<Option<u32>> = std::iter::once(None)
        .chain(values.into_iter().map(Some))
        .collect();
    for (k, value) in passes.iter().enumerate() {
        let (buf, bg) = bind(value.unwrap_or(0));
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("replay prefill"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: if k == 0 {
                        wgpu::LoadOp::Clear(1.0)
                    } else {
                        wgpu::LoadOp::Load
                    },
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: Some(wgpu::Operations {
                    load: if k == 0 {
                        wgpu::LoadOp::Clear(0)
                    } else {
                        wgpu::LoadOp::Load
                    },
                    store: wgpu::StoreOp::Store,
                }),
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        match value {
            None => pass.set_pipeline(&depth_pipeline),
            Some(v) => {
                pass.set_pipeline(&stencil_pipeline);
                pass.set_stencil_reference(*v);
            }
        }
        pass.set_bind_group(0, &bg, &[]);
        pass.draw(0..3, 0..1);
        drop(pass);
        keep.push(buf);
    }
    gpu.queue.submit([enc.finish()]);
    drop(keep);
    Ok(())
}

/// Whether the replay can hold this colour format as a render target.
pub fn renderable(f: &Fmt) -> bool {
    matches!(
        f.kind,
        Kind::Unorm8 | Kind::Unorm16 | Kind::Half | Kind::Float
    )
}

/// The format the replay renders to: 16-bit UNORM targets cannot be render attachments in wgpu,
/// they are held as half floats (11 bits of mantissa: far inside the 2/255 of the comparison).
pub fn render_fmt(f: &Fmt) -> Fmt {
    match f.kind {
        Kind::Unorm16 => Fmt {
            wgpu: match f.channels {
                1 => wgpu::TextureFormat::R16Float,
                2 => wgpu::TextureFormat::Rg16Float,
                _ => wgpu::TextureFormat::Rgba16Float,
            },
            kind: Kind::Half,
            ..*f
        },
        _ => *f,
    }
}

/// `bytes` of format `f` in the layout of `render_fmt(f)`.
pub fn to_render_bytes(f: &Fmt, bytes: &[u8]) -> Vec<u8> {
    if f.kind != Kind::Unorm16 {
        return bytes.to_vec();
    }
    bytes
        .chunks_exact(2)
        .flat_map(|c| {
            let v = f32::from(u16::from_le_bytes([c[0], c[1]])) / 65535.0;
            shader_re::harness::random::f32_to_f16(v).to_le_bytes()
        })
        .collect()
}
