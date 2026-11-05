//! Replay tests of the shaders (AC-R9.6 first half).
//!
//! A draw call of a capture is run again on a native device: the vertex and the pixel shader are
//! the `*_body` functions of our hand-written WGSL (the contract of `shader-re` 4.8.1), wrapped by
//! a generated vertex and fragment entry point; the vertices come from the captured buffers, the
//! constants from the captured constant buffers through the constant map of `shader-re`, the
//! textures and sampler states from the capture. The colour target starts with the captured
//! `before`, depth and stencil are filled by full-screen passes, and the result is compared with
//! `after` on the pixels the draw covered (`compare`).
//!
//! `run_draw` takes the call and everything it needs in memory (`DrawSpec`), so that the
//! self-test can replay a "capture" made by our own renderer. `driver` reads the real captures.

pub mod compare;
pub mod driver;
pub mod inputs;
pub mod msaa;
pub mod prefill;

use std::collections::BTreeMap;

use anyhow::{Context, Result, anyhow, bail};
use naga::valid::{Capabilities, ValidationFlags, Validator};
use naga::{AddressSpace, ImageClass, ImageDimension, StorageAccess, TypeInner};
use shader_re::harness::subject::move_slots;
use shader_re::harness::{Registers, Subject, block_on, fill_uniforms_by_ctab};
use shader_re::sm3::Ctab;
use shader_re::status::DataMove;

use inputs::{BufferSource, Call, Dds, Fmt, fetch_vertices, fmt_by_name};

/// Vertices a replay draws at most (instances times indices).
pub const VERTEX_LIMIT: usize = 4_000_000;

/// A native device with the features the captured formats need.
pub struct ReplayGpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub info: wgpu::AdapterInfo,
    pub features: wgpu::Features,
}

impl ReplayGpu {
    /// `Err` with the reason when there is no adapter: callers skip with it, they do not fail.
    pub fn new() -> Result<ReplayGpu, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut last = String::from("no adapter");
        for fallback in [false, true] {
            let adapter = match block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: fallback,
                ..Default::default()
            })) {
                Ok(a) => a,
                Err(e) => {
                    last = e.to_string();
                    continue;
                }
            };
            let wanted = wgpu::Features::TEXTURE_COMPRESSION_BC
                | wgpu::Features::TEXTURE_FORMAT_16BIT_NORM
                | wgpu::Features::FLOAT32_FILTERABLE
                | wgpu::Features::ADDRESS_MODE_CLAMP_TO_BORDER;
            let features = adapter.features() & wanted;
            match block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("golden replay"),
                required_features: features,
                required_limits: adapter.limits(),
                ..Default::default()
            })) {
                Ok((device, queue)) => {
                    return Ok(ReplayGpu {
                        device,
                        queue,
                        info: adapter.get_info(),
                        features,
                    });
                }
                Err(e) => last = e.to_string(),
            }
        }
        Err(last)
    }
}

// ------------------------------------------------------------------ the shaders of a draw

/// An input or output register with its semantic (a row of the catalog's `inputs` / `outputs`).
#[derive(Debug, Clone)]
pub struct IoReg {
    /// `v0`, `o1`, ...
    pub reg: String,
    /// Lower case: `position`, `texcoord`, `color`, `blendindices`, ...
    pub semantic: String,
    pub index: u8,
}

/// A sampler of the original shader: CTAB name and register.
#[derive(Debug, Clone)]
pub struct SamplerDecl {
    pub name: String,
    pub register: u32,
}

/// One stage of a draw: the module's body function, the original block's tables and the constant
/// registers read out of the captured constant buffers.
pub struct ShaderSide {
    /// `Package::Entry:STAGE`.
    pub entry: String,
    pub subject: Subject,
    pub inputs: Vec<IoReg>,
    pub outputs: Vec<IoReg>,
    pub ctab: Ctab,
    pub samplers: Vec<SamplerDecl>,
    pub regs: Registers,
    /// `status.toml` `data_moves`.
    pub moves: Vec<DataMove>,
}

/// A texture of the draw, by stage and register, in the captured format.
pub struct BoundTexture {
    pub dds: Dds,
    /// The format of the view (the DDS holds a typeless or UNORM one): sRGB-ness is the view's.
    pub format: wgpu::TextureFormat,
}

/// Everything `run_draw` needs.
pub struct DrawSpec<'a> {
    pub label: String,
    /// Text of the WGSL module both stages come from.
    pub wgsl: String,
    pub vs: ShaderSide,
    pub ps: ShaderSide,
    pub call: &'a Call,
    pub buffers: &'a dyn BufferSource,
    /// Key: (pixel stage, register).
    pub textures: BTreeMap<(bool, u32), BoundTexture>,
    /// Bytes of mip 0 of the first target before the draw, in the format of the target.
    pub before: Vec<u8>,
    /// `D24S8` words before the draw.
    pub before_depth: Option<Vec<u8>>,
}

/// What a replayed draw produced.
pub struct DrawResult {
    pub width: u32,
    pub height: u32,
    pub fmt: Fmt,
    /// First target after the draw.
    pub ours: Vec<[f32; 4]>,
    /// Pixels the draw covered.
    pub cover: Vec<bool>,
    pub notes: Vec<String>,
}

// ------------------------------------------------------------------ the wrapper

struct Wrapper {
    text: String,
    vertex_slots: usize,
    /// The words the vertex wrapper reads: `want[k]` is `(semantic, index)` of slot `k`.
    want: Vec<(String, u32)>,
    moves: Vec<[f32; 4]>,
}

fn is_v(name: &str) -> bool {
    name.strip_prefix('v')
        .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
}

fn find_io<'a>(list: &'a [IoReg], reg: &str) -> Option<&'a IoReg> {
    list.iter().find(|i| i.reg == reg)
}

/// The `instance_*` members of a side: their slots in the move buffer.
fn side_moves(
    side: &ShaderSide,
    tag: &str,
    moves: &mut Vec<[f32; 4]>,
    at: &mut BTreeMap<String, usize>,
) -> Result<()> {
    for f in &side.subject.input {
        let Some(attr) = f.name.strip_prefix("instance_") else {
            continue;
        };
        let mv = side
            .moves
            .iter()
            .find(|m| m.to == format!("instance:{attr}"))
            .with_context(|| {
                format!(
                    "{}: member `{}` has no data move in status.toml",
                    side.entry, f.name
                )
            })?;
        let c = side
            .ctab
            .constants
            .iter()
            .find(|c| c.name == mv.ctab)
            .with_context(|| {
                format!(
                    "{}: data move of `{}`: no such constant",
                    side.entry, mv.ctab
                )
            })?;
        let slots = move_slots(f, c, &side.regs).map_err(|e| anyhow!("{e}"))?;
        at.insert(format!("{tag}:{}", f.name), moves.len());
        moves.extend(slots);
    }
    Ok(())
}

fn build_wrapper(spec: &DrawSpec, group: u32, targets: usize, has_depth: bool) -> Result<Wrapper> {
    let (vs, ps) = (&spec.vs, &spec.ps);
    if vs.subject.input.iter().any(|f| f.name.starts_with("bone_")) {
        bail!(
            "{}: the bone palette (data move to the bone texture) is not replayed",
            vs.entry
        );
    }
    let mut moves = Vec::new();
    let mut move_at = BTreeMap::new();
    side_moves(vs, "vs", &mut moves, &mut move_at)?;
    side_moves(ps, "ps", &mut moves, &mut move_at)?;
    if moves.is_empty() {
        moves.push([0.0; 4]);
    }

    // ---- vertex inputs
    let mut want: Vec<(String, u32)> = Vec::new();
    let mut slot_of: BTreeMap<String, usize> = BTreeMap::new();
    for f in &vs.subject.input {
        if !is_v(&f.name) {
            continue;
        }
        let io = find_io(&vs.inputs, &f.name).with_context(|| {
            format!(
                "{}: input `{}` is not in the catalog's inputs",
                vs.entry, f.name
            )
        })?;
        slot_of.insert(f.name.clone(), want.len());
        want.push((io.semantic.clone(), u32::from(io.index)));
    }
    let ns = want.len().max(1);

    // ---- varyings
    let mut pos_expr = None;
    let mut vary: Vec<(String, String)> = Vec::new(); // (semantic key, expression)
    let mut vary_loc: BTreeMap<(String, u8), usize> = BTreeMap::new();
    for f in &vs.subject.output {
        let io = find_io(&vs.outputs, &f.name).with_context(|| {
            format!(
                "{}: output `{}` is not in the catalog's outputs",
                vs.entry, f.name
            )
        })?;
        let value = f
            .to_vec4(&format!("r.{}", f.name))
            .map_err(|e| anyhow!("{e}"))?;
        if io.semantic == "position" && io.index == 0 {
            pos_expr = Some(value);
        } else {
            vary_loc.insert((io.semantic.clone(), io.index), vary.len());
            vary.push((io.reg.clone(), value));
        }
    }
    let pos_expr = pos_expr.with_context(|| format!("{}: no position output", vs.entry))?;

    let mut t = String::new();
    t.push_str(&format!(
        "\n// ---- replay wrapper (generated) ----\n\
         @group({group}) @binding(0) var<storage, read> replay_vertices: array<vec4<f32>>;\n\
         @group({group}) @binding(1) var<storage, read> replay_moves: array<vec4<f32>>;\n\n\
         struct ReplayVary {{\n    @builtin(position) pos: vec4<f32>,\n"
    ));
    for k in 0..vary.len() {
        t.push_str(&format!("    @location({k}) a{k}: vec4<f32>,\n"));
    }
    t.push_str("}\n\n@vertex\nfn replay_vs(@builtin(vertex_index) vi: u32) -> ReplayVary {\n");
    t.push_str(&format!("    var inp: {};\n", vs.subject.in_type));
    for f in &vs.subject.input {
        if let Some(k) = slot_of.get(&f.name) {
            if f.slots() != 1 {
                bail!("{}: input `{}` is a matrix", vs.entry, f.name);
            }
            t.push_str(&format!(
                "    inp.{} = {};\n",
                f.name,
                f.from_slots(&|_| format!("replay_vertices[vi * {ns}u + {k}u]"))
            ));
        } else if let Some(at) = move_at.get(&format!("vs:{}", f.name)) {
            t.push_str(&format!(
                "    inp.{} = {};\n",
                f.name,
                f.from_slots(&|j| format!("replay_moves[{}u]", at + j))
            ));
        } else if f.name != "pad" {
            bail!(
                "{}: input `{}` is of a kind the replay cannot feed",
                vs.entry,
                f.name
            );
        }
    }
    t.push_str(&format!("    let r = {}(inp);\n", vs.subject.body));
    t.push_str("    var o: ReplayVary;\n");
    t.push_str(&format!("    o.pos = {pos_expr};\n"));
    for (k, (_, e)) in vary.iter().enumerate() {
        t.push_str(&format!("    o.a{k} = {e};\n"));
    }
    t.push_str("    return o;\n}\n\n");

    // ---- pixel stage
    let colour: Vec<&shader_re::harness::Field> = ps
        .subject
        .output
        .iter()
        .filter(|f| f.name.starts_with("oc"))
        .collect();
    let kill = ps.subject.output.iter().any(|f| f.name == "kill");
    let odepth = ps.subject.output.iter().any(|f| f.name == "odepth") && has_depth;
    t.push_str("struct ReplayFsOut {\n");
    for k in 0..targets {
        t.push_str(&format!("    @location({k}) c{k}: vec4<f32>,\n"));
    }
    t.push_str(&format!("    @location({targets}) cover: vec4<f32>,\n"));
    if odepth {
        t.push_str("    @builtin(frag_depth) depth: f32,\n");
    }
    t.push_str("}\n\n@fragment\nfn replay_fs(in: ReplayVary, @builtin(front_facing) ff: bool) -> ReplayFsOut {\n");
    t.push_str(&format!("    var inp: {};\n", ps.subject.in_type));
    for f in &ps.subject.input {
        let n = f.name.as_str();
        if is_v(n) {
            let io = find_io(&ps.inputs, n).with_context(|| {
                format!("{}: input `{n}` is not in the catalog's inputs", ps.entry)
            })?;
            let expr = match vary_loc.get(&(io.semantic.clone(), io.index)) {
                Some(k) => f.from_slots(&|_| format!("in.a{k}")),
                None => f.from_slots(&|_| "vec4<f32>(0.0)".to_string()),
            };
            t.push_str(&format!("    inp.{n} = {expr};\n"));
        } else if n == "vpos" {
            t.push_str(&format!(
                "    inp.vpos = {};\n",
                f.from_slots(&|_| {
                    // D3D9 `vPos` counts whole pixels (the convention of the harness);
                    // GOLDEN_REPLAY_DEBUG=vposc gives the DX11 pixel centre.
                    if std::env::var("GOLDEN_REPLAY_DEBUG").is_ok_and(|d| d.contains("vposc")) {
                        "vec4<f32>(in.pos.xy, 0.0, 0.0)".to_string()
                    } else {
                        "vec4<f32>(in.pos.xy - vec2<f32>(0.5), 0.0, 0.0)".to_string()
                    }
                })
            ));
        } else if n == "vface" {
            t.push_str(&format!(
                "    inp.vface = {};\n",
                f.from_slots(&|_| "vec4<f32>(select(-1.0, 1.0, ff), 0.0, 0.0, 0.0)".to_string())
            ));
        } else if let Some(at) = move_at.get(&format!("ps:{n}")) {
            t.push_str(&format!(
                "    inp.{n} = {};\n",
                f.from_slots(&|j| format!("replay_moves[{}u]", at + j))
            ));
        } else if n != "pad" {
            bail!(
                "{}: input `{n}` is of a kind the replay cannot feed",
                ps.entry
            );
        }
    }
    t.push_str(&format!("    let r = {}(inp);\n", ps.subject.body));
    if kill && !std::env::var("GOLDEN_REPLAY_DEBUG").is_ok_and(|d| d.contains("nokill")) {
        t.push_str("    if r.kill { discard; }\n");
    }
    t.push_str("    var o: ReplayFsOut;\n");
    for k in 0..targets {
        match colour.iter().find(|f| f.name == format!("oc{k}")) {
            Some(f) => {
                let v = f
                    .to_vec4(&format!("r.{}", f.name))
                    .map_err(|e| anyhow!("{e}"))?;
                t.push_str(&format!("    o.c{k} = {v};\n"));
            }
            None => t.push_str(&format!("    o.c{k} = vec4<f32>(0.0);\n")),
        }
    }
    t.push_str("    o.cover = vec4<f32>(1.0);\n");
    if odepth {
        t.push_str("    o.depth = r.odepth;\n");
    }
    t.push_str("    return o;\n}\n");
    Ok(Wrapper {
        text: t,
        vertex_slots: ns,
        want,
        moves,
    })
}

// ------------------------------------------------------------------ state mapping

fn compare_fn(s: &str) -> wgpu::CompareFunction {
    match s {
        "Never" | "NeverTrue" => wgpu::CompareFunction::Never,
        "Less" => wgpu::CompareFunction::Less,
        "Equal" => wgpu::CompareFunction::Equal,
        "LessEqual" => wgpu::CompareFunction::LessEqual,
        "Greater" => wgpu::CompareFunction::Greater,
        "NotEqual" => wgpu::CompareFunction::NotEqual,
        "GreaterEqual" => wgpu::CompareFunction::GreaterEqual,
        _ => wgpu::CompareFunction::Always,
    }
}

fn stencil_op(s: &str) -> wgpu::StencilOperation {
    match s {
        "Zero" => wgpu::StencilOperation::Zero,
        "Replace" => wgpu::StencilOperation::Replace,
        "IncSat" => wgpu::StencilOperation::IncrementClamp,
        "DecSat" => wgpu::StencilOperation::DecrementClamp,
        "Invert" => wgpu::StencilOperation::Invert,
        "IncWrap" => wgpu::StencilOperation::IncrementWrap,
        "DecWrap" => wgpu::StencilOperation::DecrementWrap,
        _ => wgpu::StencilOperation::Keep,
    }
}

fn stencil_face(f: &inputs::StencilFace) -> wgpu::StencilFaceState {
    wgpu::StencilFaceState {
        compare: compare_fn(&f.function),
        fail_op: stencil_op(&f.fail_operation),
        depth_fail_op: stencil_op(&f.depth_fail_operation),
        pass_op: stencil_op(&f.pass_operation),
    }
}

fn blend_factor(s: &str) -> wgpu::BlendFactor {
    use wgpu::BlendFactor as B;
    match s {
        "Zero" => B::Zero,
        "One" => B::One,
        "SrcColor" => B::Src,
        "InvSrcColor" => B::OneMinusSrc,
        "SrcAlpha" => B::SrcAlpha,
        "InvSrcAlpha" => B::OneMinusSrcAlpha,
        "DestAlpha" => B::DstAlpha,
        "InvDestAlpha" => B::OneMinusDstAlpha,
        "DestColor" => B::Dst,
        "InvDestColor" => B::OneMinusDst,
        "SrcAlphaSat" => B::SrcAlphaSaturated,
        "BlendFactor" => B::Constant,
        "InvBlendFactor" => B::OneMinusConstant,
        _ => B::One,
    }
}

fn blend_op(s: &str) -> wgpu::BlendOperation {
    match s {
        "Subtract" => wgpu::BlendOperation::Subtract,
        "ReversedSubtract" | "ReverseSubtract" | "RevSubtract" => {
            wgpu::BlendOperation::ReverseSubtract
        }
        "Min" | "Minimum" => wgpu::BlendOperation::Min,
        "Max" | "Maximum" => wgpu::BlendOperation::Max,
        _ => wgpu::BlendOperation::Add,
    }
}

fn blend_state(t: &inputs::BlendTarget) -> Option<wgpu::BlendState> {
    t.enabled.then(|| wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: blend_factor(&t.color_blend.source),
            dst_factor: blend_factor(&t.color_blend.destination),
            operation: blend_op(&t.color_blend.operation),
        },
        alpha: wgpu::BlendComponent {
            src_factor: blend_factor(&t.alpha_blend.source),
            dst_factor: blend_factor(&t.alpha_blend.destination),
            operation: blend_op(&t.alpha_blend.operation),
        },
    })
}

fn address(s: &str, gpu: &ReplayGpu) -> wgpu::AddressMode {
    match s {
        "Mirror" | "MirrorOnce" => wgpu::AddressMode::MirrorRepeat,
        "ClampEdge" | "Clamp" => wgpu::AddressMode::ClampToEdge,
        "ClampBorder" | "Border"
            if gpu
                .features
                .contains(wgpu::Features::ADDRESS_MODE_CLAMP_TO_BORDER) =>
        {
            wgpu::AddressMode::ClampToBorder
        }
        "ClampBorder" | "Border" => wgpu::AddressMode::ClampToEdge,
        // "Wrap" and anything unknown.
        _ => wgpu::AddressMode::Repeat,
    }
}

fn filter_mode(s: &str) -> wgpu::FilterMode {
    if s == "Point" {
        wgpu::FilterMode::Nearest
    } else {
        wgpu::FilterMode::Linear
    }
}

fn sampler_of(gpu: &ReplayGpu, st: Option<&inputs::SamplerState>, nearest: bool) -> wgpu::Sampler {
    let mut d = wgpu::SamplerDescriptor {
        label: Some("replay sampler"),
        ..Default::default()
    };
    if let Some(s) = st {
        d.address_mode_u = address(&s.address_u, gpu);
        d.address_mode_v = address(&s.address_v, gpu);
        d.address_mode_w = address(&s.address_w, gpu);
        d.mag_filter = filter_mode(&s.filter.magnify);
        d.min_filter = filter_mode(&s.filter.minify);
        d.mipmap_filter = if s.filter.mip == "Point" {
            wgpu::MipmapFilterMode::Nearest
        } else {
            wgpu::MipmapFilterMode::Linear
        };
        d.lod_min_clamp = s.min_lod.max(0.0);
        d.lod_max_clamp = s.max_lod.clamp(d.lod_min_clamp, 32.0);
        if s.filter.filter == "Anisotropic" && s.max_anisotropy > 1.0 {
            d.anisotropy_clamp = (s.max_anisotropy as u16).clamp(1, 16);
            d.mag_filter = wgpu::FilterMode::Linear;
            d.min_filter = wgpu::FilterMode::Linear;
            d.mipmap_filter = wgpu::MipmapFilterMode::Linear;
        }
        let b = s.border_color_value.float_value;
        d.border_color = Some(if b[3] == 0.0 {
            wgpu::SamplerBorderColor::TransparentBlack
        } else if b[0] == 0.0 {
            wgpu::SamplerBorderColor::OpaqueBlack
        } else {
            wgpu::SamplerBorderColor::OpaqueWhite
        });
    }
    if nearest {
        d.mag_filter = wgpu::FilterMode::Nearest;
        d.min_filter = wgpu::FilterMode::Nearest;
        d.mipmap_filter = wgpu::MipmapFilterMode::Nearest;
        d.anisotropy_clamp = 1;
    }
    gpu.device.create_sampler(&d)
}

fn topology(s: &str) -> Result<wgpu::PrimitiveTopology> {
    Ok(match s {
        "TriangleList" => wgpu::PrimitiveTopology::TriangleList,
        "TriangleStrip" => wgpu::PrimitiveTopology::TriangleStrip,
        "LineList" => wgpu::PrimitiveTopology::LineList,
        "LineStrip" => wgpu::PrimitiveTopology::LineStrip,
        "PointList" => wgpu::PrimitiveTopology::PointList,
        other => {
            bail!("topology {other} is not replayed (tessellated draws have DX11-only stages)")
        }
    })
}

// ------------------------------------------------------------------ resources of a module

enum Kind {
    Uniform,
    Storage,
    Texture(wgpu::TextureViewDimension),
    Sampler,
}

struct Used {
    name: String,
    group: u32,
    binding: u32,
    kind: Kind,
    var: naga::Handle<naga::GlobalVariable>,
}

fn describe(
    module: &naga::Module,
    info: &naga::valid::ModuleInfo,
    entries: &[&str],
) -> Result<Vec<Used>> {
    let mut used = Vec::new();
    for (handle, var) in module.global_variables.iter() {
        let Some(b) = &var.binding else { continue };
        let is_used = entries.iter().any(|e| {
            module
                .entry_points
                .iter()
                .position(|ep| ep.name == *e)
                .is_some_and(|i| !info.get_entry_point(i)[handle].is_empty())
        });
        if !is_used {
            continue;
        }
        let name = var.name.clone().unwrap_or_default();
        let kind = match (&var.space, &module.types[var.ty].inner) {
            (AddressSpace::Uniform, _) => Kind::Uniform,
            (AddressSpace::Storage { access }, _) => {
                if access.contains(StorageAccess::STORE) {
                    bail!("storage buffer `{name}` is written");
                }
                Kind::Storage
            }
            (AddressSpace::Handle, TypeInner::Sampler { .. }) => Kind::Sampler,
            (
                AddressSpace::Handle,
                TypeInner::Image {
                    dim,
                    arrayed: false,
                    class: ImageClass::Sampled { .. },
                },
            ) => Kind::Texture(match dim {
                ImageDimension::D2 => wgpu::TextureViewDimension::D2,
                ImageDimension::D3 => wgpu::TextureViewDimension::D3,
                ImageDimension::Cube => wgpu::TextureViewDimension::Cube,
                ImageDimension::D1 => bail!("texture `{name}` is 1D"),
            }),
            _ => bail!("resource `{name}` is of a kind the replay cannot feed"),
        };
        used.push(Used {
            name,
            group: b.group,
            binding: b.binding,
            kind,
            var: handle,
        });
    }
    Ok(used)
}

/// Bytes of a uniform variable: the members the vertex stage's CTAB knows from its registers, the
/// others from the pixel stage's.
fn uniform_bytes(
    module: &naga::Module,
    var: &naga::GlobalVariable,
    spec: &DrawSpec,
) -> Result<(Vec<u8>, Vec<String>)> {
    let a = fill_uniforms_by_ctab(module, var, &spec.vs.ctab, &spec.vs.regs)
        .map_err(|e| anyhow!("{e}"))?;
    let b = fill_uniforms_by_ctab(module, var, &spec.ps.ctab, &spec.ps.regs)
        .map_err(|e| anyhow!("{e}"))?;
    let mut bytes = a.bytes.clone();
    let mut unfilled = Vec::new();
    match &module.types[var.ty].inner {
        TypeInner::Struct { members, .. } => {
            for m in members {
                let name = m.name.clone().unwrap_or_default();
                if a.filled.contains(&name) {
                    continue;
                }
                if b.filled.contains(&name) {
                    let size = module.types[m.ty].inner.size(module.to_ctx()) as usize;
                    let at = m.offset as usize;
                    bytes[at..at + size].copy_from_slice(&b.bytes[at..at + size]);
                } else {
                    unfilled.push(name);
                }
            }
        }
        _ => {
            let name = var.name.clone().unwrap_or_default();
            if !a.filled.contains(&name) {
                if b.filled.contains(&name) {
                    bytes = b.bytes.clone();
                } else {
                    unfilled.push(name);
                }
            }
        }
    }
    Ok((bytes, unfilled))
}

/// The sampler of the original a WGSL texture variable stands for: named like the CTAB constant,
/// or, when that is not a WGSL name (`A+B`), as the oracle names it: `tex<reg>_<name with `_`>`.
fn sampler_decl<'a>(spec: &'a DrawSpec, var: &str) -> Option<(bool, &'a SamplerDecl)> {
    let hit = |s: &&SamplerDecl| {
        let clean: String = s
            .name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        s.name == var || var == format!("tex{}_{clean}", s.register)
    };
    spec.ps
        .samplers
        .iter()
        .find(hit)
        .map(|s| (true, s))
        .or_else(|| spec.vs.samplers.iter().find(hit).map(|s| (false, s)))
}

fn texture_of_sampler(name: &str) -> &str {
    for suffix in ["_smp", "_sampler", "_samp"] {
        if let Some(t) = name.strip_suffix(suffix) {
            return t;
        }
    }
    name
}

fn f32_bytes(v: &[[f32; 4]]) -> Vec<u8> {
    v.iter()
        .flat_map(|q| q.iter().flat_map(|x| x.to_le_bytes()))
        .collect()
}

// ------------------------------------------------------------------ the draw

/// Replays one draw. Errors name what the replay cannot do; a mismatch of the pixels is not an
/// error (see [`compare`]).
pub fn run_draw(gpu: &ReplayGpu, spec: &DrawSpec) -> Result<DrawResult> {
    let call = spec.call;
    let mut notes = Vec::new();
    if call.targets.is_empty() {
        bail!("the draw has no colour target");
    }
    let t0 = &call.targets[0];
    let fmt = fmt_by_name(&t0.view_format)
        .filter(prefill::renderable)
        .with_context(|| format!("target format {} is not replayed", t0.view_format))?;
    let (w, h) = (t0.width, t0.height);
    let topo = topology(&call.topology)?;
    let has_depth = call.depth_target.is_some();
    // The captured targets are multisampled (4); sample 0 is what the capture saved.
    let samples = t0.samples.max(1);
    if call.targets.iter().any(|t| t.samples.max(1) != samples)
        || call
            .depth_target
            .as_ref()
            .is_some_and(|d| d.samples.max(1) != samples)
    {
        bail!("the targets of the draw have different sample counts");
    }

    // The wrapper's group is the one after the module's last.
    let base = naga::front::wgsl::parse_str(&spec.wgsl)
        .map_err(|e| anyhow!("{}: {}", spec.label, e.emit_to_string(&spec.wgsl)))?;
    let group = base
        .global_variables
        .iter()
        .filter_map(|(_, v)| v.binding.as_ref().map(|b| b.group + 1))
        .max()
        .unwrap_or(0);
    if group >= gpu.device.limits().max_bind_groups {
        bail!(
            "the wrapper needs bind group {group}, the device has {}",
            gpu.device.limits().max_bind_groups
        );
    }
    let wrapper = build_wrapper(spec, group, call.targets.len(), has_depth)?;
    let src = format!("{}{}", spec.wgsl, wrapper.text);
    let module = naga::front::wgsl::parse_str(&src)
        .map_err(|e| anyhow!("{}: {}", spec.label, e.emit_to_string(&src)))?;
    let info = Validator::new(ValidationFlags::all(), Capabilities::all())
        .validate(&module)
        .map_err(|e| anyhow!("{}: {}", spec.label, e.emit_to_string(&src)))?;
    let used = describe(&module, &info, &["replay_vs", "replay_fs"])?;

    // ---- vertices
    let verts = fetch_vertices(spec.buffers, call, &wrapper.want, VERTEX_LIMIT)?;
    for (n, i) in &verts.missing {
        notes.push(format!(
            "vertex element {n}{i} is not in the input layout: zero"
        ));
    }
    if std::env::var("GOLDEN_REPLAY_DEBUG").is_ok_and(|d| d.contains("dump")) {
        eprintln!("vs regs c0..12: {:?}", &spec.vs.regs.c[..12]);
        eprintln!(
            "want {:?}; first vertices {:?}",
            wrapper.want,
            &verts.data[..verts.data.len().min(3)]
        );
    }
    let vcount = verts.data.len();
    if vcount == 0 {
        bail!("the draw has no vertices");
    }
    let mut flat: Vec<[f32; 4]> = Vec::with_capacity(vcount * wrapper.vertex_slots);
    for row in &verts.data {
        for k in 0..wrapper.vertex_slots {
            flat.push(row.get(k).copied().unwrap_or([0.0; 4]));
        }
    }

    let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);

    // ---- bound resources
    enum Res {
        Buffer(wgpu::Buffer),
        View(wgpu::TextureView),
        Sampler(wgpu::Sampler),
    }
    let mut res = Vec::new();
    let mut keep: Vec<wgpu::Texture> = Vec::new();
    let mut unfilled: Vec<String> = Vec::new();
    let mut filterable: BTreeMap<String, bool> = BTreeMap::new();
    for u in &used {
        let var = &module.global_variables[u.var];
        res.push(match &u.kind {
            Kind::Storage => {
                let (data, label) = match u.name.as_str() {
                    "replay_vertices" => (f32_bytes(&flat), "replay vertices"),
                    "replay_moves" => (f32_bytes(&wrapper.moves), "replay moves"),
                    other => bail!("storage buffer `{other}` is not the wrapper's"),
                };
                let b = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size: data.len().max(16) as u64,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                gpu.queue.write_buffer(&b, 0, &data);
                Res::Buffer(b)
            }
            Kind::Uniform => {
                let (bytes, un) = uniform_bytes(&module, var, spec)?;
                unfilled.extend(un);
                let b = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("replay uniform"),
                    size: bytes.len().max(16) as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                gpu.queue.write_buffer(&b, 0, &bytes);
                Res::Buffer(b)
            }
            Kind::Texture(dim) => {
                let (is_ps, decl) = sampler_decl(spec, &u.name)
                    .with_context(|| format!("texture `{}` is not a sampler of the original shaders", u.name))?;
                let tex = spec.textures.get(&(is_ps, decl.register)).with_context(|| {
                    format!(
                        "the capture has no texture in register {} ({}) of the {} stage",
                        decl.register,
                        u.name,
                        if is_ps { "pixel" } else { "vertex" }
                    )
                })?;
                let d = &tex.dds;
                let ok = match dim {
                    wgpu::TextureViewDimension::D2 => d.depth == 1 && d.layers == 1,
                    wgpu::TextureViewDimension::D3 => d.depth > 1,
                    wgpu::TextureViewDimension::Cube => d.layers == 6,
                    _ => false,
                };
                if !ok {
                    bail!(
                        "texture `{}` is {dim:?} in the module, the capture has {}x{}x{} with {} layers",
                        u.name,
                        d.width,
                        d.height,
                        d.depth,
                        d.layers
                    );
                }
                let t = prefill::upload_texture(gpu, d, tex.format, &u.name)?;
                let v = t.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(*dim),
                    ..Default::default()
                });
                let f32_fmt = matches!(
                    tex.format,
                    wgpu::TextureFormat::R32Float
                        | wgpu::TextureFormat::Rg32Float
                        | wgpu::TextureFormat::Rgba32Float
                );
                filterable.insert(
                    u.name.clone(),
                    !f32_fmt || gpu.features.contains(wgpu::Features::FLOAT32_FILTERABLE),
                );
                keep.push(t);
                Res::View(v)
            }
            Kind::Sampler => {
                let tex_name = texture_of_sampler(&u.name);
                let (is_ps, decl) = sampler_decl(spec, tex_name)
                    .with_context(|| format!("sampler `{}` does not belong to a texture of the original", u.name))?;
                let states = if is_ps { &call.samplers.ps } else { &call.samplers.vs };
                let st = states.iter().find(|s| s.slot == decl.register);
                if st.is_none() {
                    notes.push(format!("no sampler state in register {}: linear, wrap", decl.register));
                }
                if st.is_some_and(|s| s.mip_bias != 0.0) {
                    notes.push(format!("sampler {} has a LOD bias, which is not replayed", u.name));
                }
                Res::Sampler(sampler_of(gpu, st, false))
            }
        });
    }

    // ---- layouts, pipeline
    let groups = used.iter().map(|u| u.group + 1).max().unwrap_or(0);
    let mut layouts = Vec::new();
    for g in 0..groups {
        let entries: Vec<wgpu::BindGroupLayoutEntry> = used
            .iter()
            .filter(|u| u.group == g)
            .map(|u| wgpu::BindGroupLayoutEntry {
                binding: u.binding,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: match &u.kind {
                    Kind::Uniform => wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    Kind::Storage => wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    Kind::Texture(dim) => wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float {
                            filterable: filterable.get(&u.name).copied().unwrap_or(true),
                        },
                        view_dimension: *dim,
                        multisampled: false,
                    },
                    Kind::Sampler => {
                        let tex = texture_of_sampler(&u.name);
                        wgpu::BindingType::Sampler(
                            if filterable.get(tex).copied().unwrap_or(true) {
                                wgpu::SamplerBindingType::Filtering
                            } else {
                                wgpu::SamplerBindingType::NonFiltering
                            },
                        )
                    }
                },
                count: None,
            })
            .collect();
        layouts.push(
            gpu.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("replay"),
                    entries: &entries,
                }),
        );
    }
    let layout_refs: Vec<Option<&wgpu::BindGroupLayout>> = layouts.iter().map(Some).collect();
    let pipeline_layout = gpu
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("replay"),
            bind_group_layouts: &layout_refs,
            immediate_size: 0,
        });
    let shader = gpu
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("replay"),
            source: wgpu::ShaderSource::Wgsl(src.clone().into()),
        });

    // Targets: the captured ones, then the coverage plane.
    let blend = call.blend.as_ref();
    let mut color_states: Vec<Option<wgpu::ColorTargetState>> = Vec::new();
    for (k, t) in call.targets.iter().enumerate() {
        let f = fmt_by_name(&t.view_format)
            .filter(prefill::renderable)
            .map(|f| prefill::render_fmt(&f))
            .with_context(|| format!("target {k}: format {} is not replayed", t.view_format))?;
        let bt = blend.and_then(|b| {
            if b.independent_blend {
                b.blends.get(k)
            } else {
                b.blends.first()
            }
        });
        color_states.push(Some(wgpu::ColorTargetState {
            format: f.wgpu,
            blend: bt.and_then(blend_state),
            write_mask: bt.map_or(wgpu::ColorWrites::ALL, |b| {
                wgpu::ColorWrites::from_bits_truncate(b.write_mask)
            }),
        }));
    }
    color_states.push(Some(wgpu::ColorTargetState {
        format: wgpu::TextureFormat::R8Unorm,
        blend: None,
        write_mask: wgpu::ColorWrites::ALL,
    }));
    // Debugging knobs (not part of the check): GOLDEN_REPLAY_DEBUG=nocull,nodepth.
    let dbg = std::env::var("GOLDEN_REPLAY_DEBUG").unwrap_or_default();
    let mut raster = call.rasterizer.clone();
    let mut ds = call.depth_stencil.clone();
    if dbg.contains("nocull")
        && let Some(r) = raster.as_mut()
    {
        r.cull_mode = "NoCull".into();
    }
    if dbg.contains("nodepth")
        && let Some(d) = ds.as_mut()
    {
        d.depth_enable = false;
    }
    let depth_state = has_depth.then(|| {
        let enabled = ds.as_ref().is_some_and(|d| d.depth_enable);
        let stencil = ds.as_ref().filter(|d| d.stencil_enable);
        wgpu::DepthStencilState {
            format: prefill::DEPTH_FORMAT,
            depth_write_enabled: Some(enabled && ds.as_ref().is_some_and(|d| d.depth_writes)),
            depth_compare: Some(if enabled {
                compare_fn(ds.as_ref().map_or("Always", |d| d.depth_function.as_str()))
            } else {
                wgpu::CompareFunction::Always
            }),
            stencil: match stencil {
                Some(s) => wgpu::StencilState {
                    front: stencil_face(&s.front_face),
                    back: stencil_face(&s.back_face),
                    read_mask: s.front_face.compare_mask,
                    write_mask: s.front_face.write_mask,
                },
                None => wgpu::StencilState::default(),
            },
            bias: raster
                .as_ref()
                .map_or(wgpu::DepthBiasState::default(), |r| wgpu::DepthBiasState {
                    constant: r.depth_bias,
                    slope_scale: r.slope_scaled_depth_bias,
                    clamp: r.depth_bias_clamp,
                }),
        }
    });
    let make_pipeline = |depth_state: Option<wgpu::DepthStencilState>,
                         targets: &[Option<wgpu::ColorTargetState>]| {
        gpu.device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("replay"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("replay_vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    topology: topo,
                    front_face: if raster.as_ref().is_none_or(|r| r.front_ccw) {
                        wgpu::FrontFace::Ccw
                    } else {
                        wgpu::FrontFace::Cw
                    },
                    cull_mode: match raster.as_ref().map_or("NoCull", |r| r.cull_mode.as_str()) {
                        "Front" => Some(wgpu::Face::Front),
                        "Back" => Some(wgpu::Face::Back),
                        _ => None,
                    },
                    ..Default::default()
                },
                depth_stencil: depth_state,
                multisample: wgpu::MultisampleState {
                    count: samples,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("replay_fs"),
                    compilation_options: Default::default(),
                    targets,
                }),
                multiview_mask: None,
                cache: None,
            })
    };
    let pipeline = make_pipeline(depth_state, &color_states);
    let mut bind_groups = Vec::new();
    for (g, layout) in layouts.iter().enumerate() {
        let mut entries = Vec::new();
        for (u, r) in used.iter().zip(&res) {
            if u.group as usize != g {
                continue;
            }
            entries.push(wgpu::BindGroupEntry {
                binding: u.binding,
                resource: match r {
                    Res::Buffer(b) => b.as_entire_binding(),
                    Res::View(v) => wgpu::BindingResource::TextureView(v),
                    Res::Sampler(s) => wgpu::BindingResource::Sampler(s),
                },
            });
        }
        bind_groups.push(gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("replay"),
            layout,
            entries: &entries,
        }));
    }

    // ---- target textures
    let mk_tex = |label: &str, format: wgpu::TextureFormat, (tw, th): (u32, u32), sampled: bool| {
        gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: tw,
                height: th,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format,
            // A multisampled texture cannot be copied: it is read by a pass (`sampled`).
            usage: if samples > 1 {
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING
            } else if sampled {
                wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::COPY_DST
            } else {
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC
            },
            view_formats: &[],
        })
    };
    let mut target_tex = Vec::new();
    for t in &call.targets {
        let f = prefill::render_fmt(&fmt_by_name(&t.view_format).expect("checked above"));
        target_tex.push(mk_tex("replay target", f.wgpu, (t.width, t.height), true));
    }
    let act = prefill::render_fmt(&fmt);
    let before_bytes = prefill::to_render_bytes(&fmt, &spec.before);
    if samples > 1 {
        // `before` in a single-sample texture, then into every sample of the target.
        let single = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("replay before"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: act.wgpu,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        prefill::write_pixels(gpu, &single, &act, w, h, &before_bytes)?;
        msaa::copy_to_all_samples(gpu, &single, &target_tex[0], act.wgpu, samples);
        keep.push(single);
    } else {
        prefill::write_pixels(gpu, &target_tex[0], &act, w, h, &before_bytes)?;
    }
    let cover_tex = mk_tex(
        "replay coverage",
        wgpu::TextureFormat::R8Unorm,
        (w, h),
        true,
    );
    let depth_tex = call.depth_target.as_ref().map(|d| {
        mk_tex(
            "replay depth",
            prefill::DEPTH_FORMAT,
            (d.width, d.height),
            false,
        )
    });
    let mut depth_loaded = false;
    if let (Some(dt), Some(dv)) = (&depth_tex, &call.depth_target) {
        match &spec.before_depth {
            Some(words) => {
                let stencil = ds.as_ref().is_some_and(|d| d.stencil_enable);
                prefill::prefill_depth(gpu, dt, words, dv.width, dv.height, stencil, samples)?;
                depth_loaded = true;
            }
            None => notes.push("no depth before the draw was saved: depth is cleared to 1".into()),
        }
    }

    // ---- draw
    let views: Vec<wgpu::TextureView> = target_tex
        .iter()
        .map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()))
        .collect();
    let cover_view = cover_tex.create_view(&wgpu::TextureViewDescriptor::default());
    let depth_view = depth_tex
        .as_ref()
        .map(|t| t.create_view(&wgpu::TextureViewDescriptor::default()));
    let mut attachments: Vec<Option<wgpu::RenderPassColorAttachment>> = Vec::new();
    for (k, v) in views.iter().enumerate() {
        attachments.push(Some(wgpu::RenderPassColorAttachment {
            view: v,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: if k == 0 {
                    wgpu::LoadOp::Load
                } else {
                    wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                },
                store: wgpu::StoreOp::Store,
            },
        }));
    }
    attachments.push(Some(wgpu::RenderPassColorAttachment {
        view: &cover_view,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations {
            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
            store: wgpu::StoreOp::Store,
        },
    }));
    let mut enc = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("replay"),
            color_attachments: &attachments,
            depth_stencil_attachment: depth_view.as_ref().map(|v| {
                wgpu::RenderPassDepthStencilAttachment {
                    view: v,
                    depth_ops: Some(wgpu::Operations {
                        load: if depth_loaded {
                            wgpu::LoadOp::Load
                        } else {
                            wgpu::LoadOp::Clear(1.0)
                        },
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: Some(wgpu::Operations {
                        load: if depth_loaded {
                            wgpu::LoadOp::Load
                        } else {
                            wgpu::LoadOp::Clear(0)
                        },
                        store: wgpu::StoreOp::Store,
                    }),
                }
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipeline);
        for (g, bg) in bind_groups.iter().enumerate() {
            pass.set_bind_group(g as u32, bg, &[]);
        }
        if let Some(vp) = call.viewports.first().filter(|v| v.enabled) {
            pass.set_viewport(vp.x, vp.y, vp.width, vp.height, vp.min_depth, vp.max_depth);
        }
        if raster.as_ref().is_some_and(|r| r.scissor_enable)
            && let Some(sc) = call.scissors.first().filter(|s| s.enabled)
        {
            pass.set_scissor_rect(
                sc.x.max(0) as u32,
                sc.y.max(0) as u32,
                sc.width.max(0) as u32,
                sc.height.max(0) as u32,
            );
        }
        if let Some(b) = blend {
            let f = b.blend_factor;
            pass.set_blend_constant(wgpu::Color {
                r: f64::from(f[0]),
                g: f64::from(f[1]),
                b: f64::from(f[2]),
                a: f64::from(f[3]),
            });
        }
        if let Some(d) = &ds {
            pass.set_stencil_reference(d.front_face.reference);
        }
        pass.draw(0..vcount as u32, 0..1);
    }
    gpu.queue.submit([enc.finish()]);

    // Sample 0 of the target and of the coverage plane.
    let r8 = fmt_by_name("R8_UNORM").expect("R8");
    let (bytes, cover_bytes) = if samples > 1 {
        let t = msaa::resolve_sample0(gpu, &target_tex[0], act.wgpu, w, h);
        let c = msaa::resolve_sample0(gpu, &cover_tex, wgpu::TextureFormat::R8Unorm, w, h);
        (
            prefill::read_pixels(gpu, &t, &act, w, h)?,
            prefill::read_pixels(gpu, &c, &r8, w, h)?,
        )
    } else {
        (
            prefill::read_pixels(gpu, &target_tex[0], &act, w, h)?,
            prefill::read_pixels(gpu, &cover_tex, &r8, w, h)?,
        )
    };
    if let Some(e) = block_on(scope.pop()) {
        bail!("gpu: {e}");
    }
    drop(keep);
    if !unfilled.is_empty() {
        unfilled.sort();
        unfilled.dedup();
        notes.push(format!(
            "uniform members with no constant in the capture (zero): {}",
            unfilled.join(", ")
        ));
    }
    let mut ours = inputs::decode_pixels(&act, &bytes)?;
    if fmt.kind == inputs::Kind::Unorm16 {
        // The target was held as half floats: a UNORM target saturates what is written.
        for q in &mut ours {
            for c in q.iter_mut() {
                *c = c.clamp(0.0, 1.0);
            }
        }
    }
    Ok(DrawResult {
        width: w,
        height: h,
        fmt,
        ours,
        cover: cover_bytes.iter().map(|&c| c > 127).collect(),
        notes,
    })
}
