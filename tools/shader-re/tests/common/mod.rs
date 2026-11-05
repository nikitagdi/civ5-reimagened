//! Test support: a tiny SM3 token assembler (the tests build their own blocks, nothing comes
//! from the game) and a native GPU runner for oracle modules.
#![allow(dead_code)]

use std::collections::BTreeMap;

use shader_re::harness::Gpu;
use shader_re::oracle::{self, Oracle};
use shader_re::scan::Stage;
use shader_re::sm3::ctab::{Ctab, CtabConstant, CtabType, ParamClass, ParamType, RegisterSet};
use shader_re::sm3::{self, ir::*};

// ---- assembler ----------------------------------------------------------------------------

pub const PS_3_0: u32 = 0xFFFF_0300;
pub const VS_3_0: u32 = 0xFFFE_0300;
pub const XYZW: [u8; 4] = [0, 1, 2, 3];
pub const X: [u8; 4] = [0, 0, 0, 0];
pub const Y: [u8; 4] = [1, 1, 1, 1];
pub const Z: [u8; 4] = [2, 2, 2, 2];
pub const W: [u8; 4] = [3, 3, 3, 3];

fn reg_bits(ty: RegType, num: u32) -> u32 {
    let c = ty.code();
    0x8000_0000 | num | ((c & 7) << 28) | ((c & 0x18) << 8)
}

/// Destination token: all components.
pub fn d(ty: RegType, num: u32) -> u32 {
    dm(ty, num, 0xF)
}

/// Destination token with a write mask (bit 0 = x).
pub fn dm(ty: RegType, num: u32, mask: u8) -> u32 {
    reg_bits(ty, num) | ((mask as u32) << 16)
}

/// Destination with `_sat` (bit 0 of the modifier field) and a result shift.
pub fn dmod(ty: RegType, num: u32, mask: u8, sat: bool, shift: i8) -> u32 {
    dm(ty, num, mask) | (u32::from(sat) << 20) | (((shift as u32) & 0xF) << 24)
}

fn sw_bits(sw: [u8; 4]) -> u32 {
    (sw[0] as u32 | (sw[1] as u32) << 2 | (sw[2] as u32) << 4 | (sw[3] as u32) << 6) << 16
}

/// Source token.
pub fn s(ty: RegType, num: u32, sw: [u8; 4]) -> u32 {
    reg_bits(ty, num) | sw_bits(sw)
}

/// Source token with a modifier.
pub fn sm(ty: RegType, num: u32, sw: [u8; 4], m: SrcMod) -> u32 {
    s(ty, num, sw) | (m.code() << 24)
}

/// Relative source `c[a0.<comp> + num]`: two tokens.
pub fn srel(num: u32, sw: [u8; 4], comp: u8) -> [u32; 2] {
    [
        reg_bits(RegType::Const, num) | sw_bits(sw) | (1 << 13),
        reg_bits(RegType::Addr, 0) | ((comp as u32) << 16),
    ]
}

pub struct Prog {
    w: Vec<u32>,
}

impl Prog {
    pub fn new(version: u32) -> Prog {
        Prog { w: vec![version] }
    }

    pub fn inst(&mut self, op: Opcode, control: u8, ops: &[u32]) -> &mut Prog {
        let len = ops.len() as u32;
        self.w
            .push(op.code() as u32 | ((control as u32) << 16) | (len << 24));
        self.w.extend_from_slice(ops);
        self
    }

    /// `dcl_<usage><index> reg` for an input or output.
    pub fn dcl(&mut self, usage: Usage, index: u8, dst: u32) -> &mut Prog {
        self.inst(
            Opcode::Dcl,
            0,
            &[usage.code() | ((index as u32) << 16) | 0x8000_0000, dst],
        )
    }

    /// `dcl_2d` / `dcl_cube` / `dcl_volume` for `s<num>`.
    pub fn dcl_sampler(&mut self, kind: SamplerKind, num: u32) -> &mut Prog {
        self.inst(
            Opcode::Dcl,
            0,
            &[(kind.code() << 27) | 0x8000_0000, d(RegType::Sampler, num)],
        )
    }

    /// A constant table comment (`CTAB`) with these constants; the target is the block's profile.
    pub fn ctab(&mut self, constants: Vec<CtabConstant>) -> &mut Prog {
        let ct = Ctab {
            creator: "shader-re test".to_string(),
            version: self.w[0],
            flags: 0,
            target: if self.w[0] == PS_3_0 {
                "ps_3_0"
            } else {
                "vs_3_0"
            }
            .to_string(),
            constants,
        };
        let payload = sm3::ctab::build(&ct);
        assert_eq!(payload.len() % 4, 0);
        let dwords = 1 + payload.len() / 4;
        self.w.push(0xFFFE | ((dwords as u32) << 16));
        self.w.push(sm3::ctab::CTAB_TAG);
        self.w.extend(
            payload
                .chunks_exact(4)
                .map(|c| u32::from_le_bytes(c.try_into().unwrap())),
        );
        self
    }

    pub fn def(&mut self, num: u32, v: [f32; 4]) -> &mut Prog {
        let mut ops = vec![reg_bits(RegType::Const, num)];
        ops.extend(v.iter().map(|x| x.to_bits()));
        self.inst(Opcode::Def, 0, &ops)
    }

    pub fn defi(&mut self, num: u32, v: [i32; 4]) -> &mut Prog {
        let mut ops = vec![reg_bits(RegType::ConstInt, num)];
        ops.extend(v.iter().map(|x| *x as u32));
        self.inst(Opcode::DefI, 0, &ops)
    }

    /// Finished block (END token added).
    pub fn bytes(&self) -> Vec<u8> {
        let mut w = self.w.clone();
        w.push(0x0000_FFFF);
        w.iter().flat_map(|x| x.to_le_bytes()).collect()
    }

    pub fn shader(&self) -> sm3::Shader {
        sm3::parse(&self.bytes()).expect("the assembler writes valid tokens")
    }
}

/// A CTAB entry: `float4 name` at `c<reg>` (a vector), an array or a matrix by the arguments.
#[allow(clippy::too_many_arguments)]
pub fn konst(
    name: &str,
    set: RegisterSet,
    reg: u16,
    count: u16,
    class: ParamClass,
    ty: ParamType,
    rows: u16,
    columns: u16,
    elements: u16,
) -> CtabConstant {
    CtabConstant {
        name: name.to_string(),
        set,
        reg,
        count,
        ty: CtabType {
            class,
            ty,
            rows,
            columns,
            elements,
            members: Vec::new(),
        },
        default: None,
    }
}

/// `float4 name` at `c<reg>`.
pub fn float4(name: &str, reg: u16) -> CtabConstant {
    konst(
        name,
        RegisterSet::Float4,
        reg,
        1,
        ParamClass::Vector,
        ParamType::Float,
        1,
        4,
        1,
    )
}

/// `float name` at `c<reg>.x`.
pub fn float1(name: &str, reg: u16) -> CtabConstant {
    konst(
        name,
        RegisterSet::Float4,
        reg,
        1,
        ParamClass::Scalar,
        ParamType::Float,
        1,
        1,
        1,
    )
}

/// `float4x4 name` at `c<reg>..c<reg+3>`: registers are columns (`matrix_columns`) or rows
/// (`matrix_rows`).
pub fn float4x4(name: &str, reg: u16, class: ParamClass) -> CtabConstant {
    konst(
        name,
        RegisterSet::Float4,
        reg,
        4,
        class,
        ParamType::Float,
        4,
        4,
        1,
    )
}

/// `sampler2D` / `samplerCUBE` named `name` at `s<reg>`.
pub fn sampler(name: &str, reg: u16, ty: ParamType) -> CtabConstant {
    konst(
        name,
        RegisterSet::Sampler,
        reg,
        1,
        ParamClass::Object,
        ty,
        0,
        0,
        1,
    )
}

/// A pixel shader with an `oC0` write built from `f`.
pub fn ps(f: impl FnOnce(&mut Prog)) -> sm3::Shader {
    let mut p = Prog::new(PS_3_0);
    f(&mut p);
    p.shader()
}

/// A vertex shader: `dcl_position o0`, `dcl_texcoord1 o1`, body from `f`; `o0` is set to the
/// clip-space origin by the test body itself.
pub fn vs(f: impl FnOnce(&mut Prog)) -> sm3::Shader {
    let mut p = Prog::new(VS_3_0);
    p.dcl(Usage::Position, 0, d(RegType::Output, 0));
    p.dcl(Usage::TexCoord, 1, d(RegType::Output, 1));
    f(&mut p);
    p.shader()
}

pub const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

pub fn translate(sh: &sm3::Shader) -> Oracle {
    oracle::translate(sh, "Test::Entry:XS", SHA, &oracle::Options::default())
        .unwrap_or_else(|e| panic!("{e}"))
}

// ---- values -------------------------------------------------------------------------------

pub const NAN: f32 = f32::NAN;
pub const INF: f32 = f32::INFINITY;

/// Same value: NaN is NaN, infinities have their sign, the rest agree to `1e-5` relative.
pub fn same(a: f32, b: f32) -> bool {
    if a.is_nan() || b.is_nan() {
        return a.is_nan() && b.is_nan();
    }
    if a.is_infinite() || b.is_infinite() {
        return a == b;
    }
    (a - b).abs() <= 1e-5 * 1f32.max(a.abs()).max(b.abs())
}

pub fn assert_vec(got: [f32; 4], want: [f32; 4], what: &str) {
    for c in 0..4 {
        assert!(
            same(got[c], want[c]),
            "{what}: component {c}: got {:?}, want {:?}",
            got,
            want
        );
    }
}

// ---- GPU runner ---------------------------------------------------------------------------

/// A test's GPU, held under a process-wide lock: the test harness runs tests on parallel
/// threads, and several devices created and polled at once on Windows hung the run for hours
/// (2026-11-01, `oracle_semantics` stuck at 100% CPU). One test owns the GPU at a time.
pub struct GpuLock {
    gpu: Gpu,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl std::ops::Deref for GpuLock {
    type Target = Gpu;
    fn deref(&self) -> &Gpu {
        &self.gpu
    }
}

static GPU_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn gpu(test: &str) -> Option<GpuLock> {
    // A test that panicked while it held the lock only poisons it; the GPU is still usable.
    let guard = GPU_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    match Gpu::new() {
        Ok(g) => {
            eprintln!("{test}: GPU {} ({:?})", g.info.name, g.info.backend);
            Some(GpuLock {
                gpu: g,
                _guard: guard,
            })
        }
        Err(e) => {
            eprintln!("SKIP {test}: no GPU adapter: {e}");
            None
        }
    }
}

/// A 2D texture for a test: RGBA8 texels per mip level, base size `size` x `size`.
#[derive(Clone)]
pub struct TestTex {
    pub size: u32,
    /// `mips[level]` holds `(size >> level)^2` texels.
    pub mips: Vec<Vec<[u8; 4]>>,
}

impl TestTex {
    pub fn solid(color: [u8; 4]) -> TestTex {
        TestTex {
            size: 1,
            mips: vec![vec![color]],
        }
    }
}

#[derive(Default)]
pub struct Run {
    pub consts: Vec<[f32; 4]>,
    pub ints: Vec<[i32; 4]>,
    pub bools: u32,
    /// Input registers `v#`: for a pixel shader the value the vertex stage passes on, for a
    /// vertex shader the attribute.
    pub v: BTreeMap<u32, [f32; 4]>,
    pub textures: BTreeMap<u32, TestTex>,
    pub size: (u32, u32),
}

fn f32_bytes(v: &[[f32; 4]], total: usize) -> Vec<u8> {
    let mut out: Vec<u8> = v
        .iter()
        .flat_map(|q| q.iter().flat_map(|x| x.to_le_bytes()))
        .collect();
    out.resize(total, 0);
    out
}

fn uniform_bytes(o: &Oracle, run: &Run) -> Option<Vec<u8>> {
    let cb = o.consts?;
    let mut bytes = vec![0u8; cb.size()];
    let c = f32_bytes(&run.consts, cb.float_regs as usize * 16);
    bytes[..c.len()].copy_from_slice(&c[..cb.float_regs as usize * 16]);
    for (k, q) in run.ints.iter().take(cb.int_regs as usize).enumerate() {
        for (j, x) in q.iter().enumerate() {
            let at = cb.i_offset() + k * 16 + j * 4;
            bytes[at..at + 4].copy_from_slice(&x.to_le_bytes());
        }
    }
    if cb.bool_mask {
        bytes[cb.b_offset()..cb.b_offset() + 4].copy_from_slice(&run.bools.to_le_bytes());
    }
    Some(bytes)
}

fn buffer_with(gpu: &Gpu, usage: wgpu::BufferUsages, bytes: &[u8]) -> wgpu::Buffer {
    let b = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: bytes.len() as u64,
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    gpu.queue.write_buffer(&b, 0, bytes);
    b
}

fn upload(gpu: &Gpu, t: &TestTex) -> wgpu::TextureView {
    let tex = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: t.size,
            height: t.size,
            depth_or_array_layers: 1,
        },
        mip_level_count: t.mips.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (level, texels) in t.mips.iter().enumerate() {
        let n = (t.size >> level).max(1);
        let bytes: Vec<u8> = texels.iter().flatten().copied().collect();
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(n * 4),
                rows_per_image: Some(n),
            },
            wgpu::Extent3d {
                width: n,
                height: n,
                depth_or_array_layers: 1,
            },
        );
    }
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}

/// Draws with the oracle module (plus a small test stage) into an `Rgba32Float` target and
/// returns the texels, row by row. A pixel shader gets a full-screen triangle; a vertex shader
/// one point in the middle of a 1x1 target, and `out_reg` (an `o#` register) is what is read back.
pub fn run(gpu: &Gpu, o: &Oracle, r: &Run, out_reg: u32) -> Vec<[f32; 4]> {
    let is_ps = o.stage == Stage::Ps;
    let size = if is_ps { r.size } else { (1, 1) };
    let mut src = o.wgsl.clone();
    let input_regs: Vec<u32> = o
        .inputs
        .iter()
        .filter(|i| i.usage.is_some())
        .map(|i| i.reg)
        .collect();
    if is_ps {
        if !input_regs.is_empty() {
            src.push_str("\nstruct TestIn { v: array<vec4<f32>, 16> }\n");
            src.push_str("@group(2) @binding(0) var<uniform> test_in: TestIn;\n");
        }
        let outs: Vec<String> = input_regs
            .iter()
            .map(|n| format!("@location({n}) v{n}: vec4<f32>"))
            .collect();
        let mut vs_out = vec!["@builtin(position) pos: vec4<f32>".to_string()];
        vs_out.extend(outs);
        src.push_str(&format!("\nstruct TestVsOut {{ {} }}\n", vs_out.join(", ")));
        src.push_str(
            "@vertex fn test_vs(@builtin(vertex_index) i: u32) -> TestVsOut {\n    var o: TestVsOut;\n    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));\n    o.pos = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);\n",
        );
        for n in &input_regs {
            src.push_str(&format!("    o.v{n} = test_in.v[{n}];\n"));
        }
        src.push_str("    return o;\n}\n");
    } else {
        let loc = o
            .outputs
            .iter()
            .find(|x| x.reg == out_reg)
            .and_then(|x| x.location)
            .expect("an output with a location");
        src.push_str(&format!(
            "\n@fragment fn test_fs(@location({loc}) v: vec4<f32>) -> @location(0) vec4<f32> {{ return v; }}\n"
        ));
    }
    let module = gpu
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("oracle test"),
            source: wgpu::ShaderSource::Wgsl(src.into()),
        });
    let target_format = wgpu::TextureFormat::Rgba32Float;
    let attrs: Vec<wgpu::VertexAttribute> = input_regs
        .iter()
        .enumerate()
        .map(|(k, n)| wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x4,
            offset: 16 * k as u64,
            shader_location: *n,
        })
        .collect();
    let vbuf_layout = [Some(wgpu::VertexBufferLayout {
        array_stride: 16 * input_regs.len().max(1) as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &attrs,
    })];
    let pipeline = gpu
        .device
        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("oracle test"),
            layout: None,
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some(if is_ps { "test_vs" } else { "oracle_vs" }),
                compilation_options: Default::default(),
                buffers: if is_ps || attrs.is_empty() {
                    &[]
                } else {
                    &vbuf_layout
                },
            },
            primitive: wgpu::PrimitiveState {
                topology: if is_ps {
                    wgpu::PrimitiveTopology::TriangleList
                } else {
                    wgpu::PrimitiveTopology::PointList
                },
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some(if is_ps { "oracle_fs" } else { "test_fs" }),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

    // Resources by group.
    let ubo = uniform_bytes(o, r).map(|b| buffer_with(gpu, wgpu::BufferUsages::UNIFORM, &b));
    let mut test_in = [[0f32; 4]; 16];
    for (n, v) in &r.v {
        test_in[*n as usize] = *v;
    }
    let test_in_buf = buffer_with(
        gpu,
        wgpu::BufferUsages::UNIFORM,
        &f32_bytes(&test_in, 16 * 16),
    );
    let views: Vec<(u32, wgpu::TextureView)> = o
        .samplers
        .iter()
        .map(|s| {
            let t = r
                .textures
                .get(&s.reg)
                .cloned()
                .unwrap_or_else(|| TestTex::solid([255; 4]));
            (s.reg, upload(gpu, &t))
        })
        .collect();
    let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        ..Default::default()
    });
    let mut groups: Vec<Vec<wgpu::BindGroupEntry>> = vec![vec![], vec![], vec![]];
    if let (Some(cb), Some(ubo)) = (o.consts, &ubo) {
        groups[cb.group as usize].push(wgpu::BindGroupEntry {
            binding: cb.binding,
            resource: ubo.as_entire_binding(),
        });
    }
    for s in &o.samplers {
        let view = &views.iter().find(|(r, _)| *r == s.reg).unwrap().1;
        groups[s.group as usize].push(wgpu::BindGroupEntry {
            binding: s.texture_binding,
            resource: wgpu::BindingResource::TextureView(view),
        });
        groups[s.group as usize].push(wgpu::BindGroupEntry {
            binding: s.sampler_binding,
            resource: wgpu::BindingResource::Sampler(&sampler),
        });
    }
    if is_ps && !input_regs.is_empty() {
        groups[2].push(wgpu::BindGroupEntry {
            binding: 0,
            resource: test_in_buf.as_entire_binding(),
        });
    }
    let bind_groups: Vec<wgpu::BindGroup> = groups
        .iter()
        .enumerate()
        .map(|(g, entries)| {
            gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &pipeline.get_bind_group_layout(g as u32),
                entries,
            })
        })
        .collect();
    // Vertex data of the vertex-shader runs.
    let vdata: Vec<u8> = f32_bytes(
        &input_regs
            .iter()
            .map(|n| r.v.get(n).copied().unwrap_or([0.0; 4]))
            .collect::<Vec<_>>(),
        16 * input_regs.len().max(1),
    );
    let vbuf = buffer_with(gpu, wgpu::BufferUsages::VERTEX, &vdata);

    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: target_format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let mut enc = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    // A pixel that is not written (discard) reads back as 7.
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 7.0,
                        g: 7.0,
                        b: 7.0,
                        a: 7.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipeline);
        for (g, bg) in bind_groups.iter().enumerate() {
            pass.set_bind_group(g as u32, bg, &[]);
        }
        if is_ps {
            pass.draw(0..3, 0..1);
        } else {
            if !attrs.is_empty() {
                pass.set_vertex_buffer(0, vbuf.slice(..));
            }
            pass.draw(0..1, 0..1);
        }
    }
    let row = size.0 * 16;
    let padded = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let staging = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(padded) * u64::from(size.1),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(size.1),
            },
        },
        wgpu::Extent3d {
            width: size.0,
            height: size.1,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit([enc.finish()]);
    staging.map_async(wgpu::MapMode::Read, .., |r| r.expect("map"));
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let data = staging.get_mapped_range(..).expect("range");
    let mut out = Vec::new();
    for y in 0..size.1 as usize {
        for x in 0..size.0 as usize {
            let at = y * padded as usize + x * 16;
            let mut q = [0f32; 4];
            for (c, v) in q.iter_mut().enumerate() {
                *v = f32::from_le_bytes(data[at + c * 4..at + c * 4 + 4].try_into().unwrap());
            }
            out.push(q);
        }
    }
    out
}
