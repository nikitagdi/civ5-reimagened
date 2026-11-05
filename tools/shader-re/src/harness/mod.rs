//! Native GPU runs of the oracle and of hand-written WGSL.
//!
//! A [`Subject`] (the oracle or a hand-written module, see `subject` for the body contract) is
//! wrapped in a generated entry point and drawn on a 64x64 grid with random inputs:
//!
//! - **pixel shaders** (`ps`): a full-screen triangle; every pixel reads its inputs from a storage
//!   buffer, calls the body and writes the outputs to another one;
//! - **vertex shaders** (`vs`): one point per pixel; the vertex reads its attributes the same way,
//!   the outputs travel as `flat` varyings and the fragment stage stores them.
//!
//! Resources are matched by CTAB name: textures of the same kind and the same random contents
//! for both sides, one sampler (a linear-with-mips run and a nearest run), constants from the
//! same registers. `compare` runs both sides over several seeds and applies the metric SR-D10.

pub mod compare;
pub mod formats;
pub mod ps;
pub mod random;
pub mod subject;
pub mod vs;

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use naga::valid::{Capabilities, ValidationFlags, Validator};
use naga::{AddressSpace, ImageClass, ImageDimension, StorageAccess, TypeInner};

pub use random::Registers;
pub use subject::{
    ConstSource, Field, Shape, Subject, UboFill, fill_uniforms_by_ctab, oracle_uniform_bytes,
};

use crate::oracle::SamplerRes;
use crate::scan::Stage;
use crate::sm3::ctab::{Ctab, CtabConstant};
use crate::status::DataMove;
use formats::TexData;

/// Edge of the grid (4.8: 64x64).
pub const GRID: u32 = 64;
/// Pixels (or vertices) per run.
pub const PIXELS: usize = (GRID * GRID) as usize;
/// The seeds of a comparison (4.8: three).
pub const SEEDS: [u64; 3] = [1, 2, 3];
/// Width of the bone texture in texels: 512 bones of 3 texels (render-core 4.12, world-anim).
pub const BONE_TEXTURE_WIDTH: u32 = 1536;
/// Name of the bone texture in a hand-written module.
pub const BONE_TEXTURE: &str = "bone_texture";
/// The name `skin.wgsl` of render-core gives the same texture in the products.
pub const BONE_TEXTURE_ALIAS: &str = "bone_tex";

/// A data move for a palette of `row_major float4x4` bones (four registers a bone, the columns of
/// the 3x4 matrix, the fourth the translation): the registers of a bone become its three rows.
pub const BONE_TEXTURE_COLS4: &str = "bone_texture_cols4";

fn is_bone_texture(name: &str) -> bool {
    name == BONE_TEXTURE || name == BONE_TEXTURE_ALIAS
}

#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error("{0}")]
    Wgsl(String),
    /// The module does not follow the body contract (see `subject`).
    #[error("{0}")]
    Contract(String),
    #[error("gpu: {0}")]
    Gpu(String),
    #[error("{0}")]
    Fill(String),
}

/// A native device with the adapter's full limits (4.8: "native wgpu, full device limits").
pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub info: wgpu::AdapterInfo,
}

struct ThreadWaker(std::thread::Thread);

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Runs a future to the end on this thread (wgpu's adapter and device requests are futures).
pub fn block_on<F: Future>(fut: F) -> F::Output {
    let mut fut = std::pin::pin!(fut);
    let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::park();
    }
}

impl Gpu {
    /// The first adapter wgpu offers (a software one if no hardware adapter exists).
    /// `Err` with the reason when there is none: callers skip with it, they do not fail.
    pub fn new() -> Result<Gpu, String> {
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
            match block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("shader-re device"),
                required_features: wgpu::Features::empty(),
                required_limits: adapter.limits(),
                ..Default::default()
            })) {
                Ok((device, queue)) => {
                    return Ok(Gpu {
                        device,
                        queue,
                        info: adapter.get_info(),
                    });
                }
                Err(e) => last = e.to_string(),
            }
        }
        Err(last)
    }

    /// Bytes of a buffer that has `COPY_SRC`.
    pub fn read_buffer(&self, buffer: &wgpu::Buffer, size: u64) -> Vec<u8> {
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        enc.copy_buffer_to_buffer(buffer, 0, &staging, 0, size);
        self.queue.submit([enc.finish()]);
        staging.map_async(wgpu::MapMode::Read, .., |r| r.expect("map"));
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        staging.get_mapped_range(..).expect("range").to_vec()
    }
}

/// What a run needs to know about the original shader.
pub struct EntryCtx<'a> {
    /// `Package::Entry:STAGE`.
    pub name: &'a str,
    /// The block's constant table: names, registers, classes.
    pub ctab: Option<&'a Ctab>,
    /// Samplers: CTAB name and kind.
    pub samplers: &'a [SamplerRes],
}

/// One run's data, shared by the two sides.
pub struct Scene {
    pub seed: u64,
    pub regs: Registers,
    /// Textures by CTAB sampler name, in our formats.
    pub textures: BTreeMap<String, TexData>,
    /// Linear filter with mips (`true`) or point sampling.
    pub linear: bool,
    /// `data_moves` to the instance: `In` member name (`instance_<attr>`) to the CTAB constant
    /// whose registers it takes (cut to the member's type by `subject::move_slots`).
    pub moved: BTreeMap<String, CtabConstant>,
    /// `data_moves` to the bone texture: the palette's texels (row 0), `bone_base = 0`.
    pub bone_texels: Option<Vec<[f32; 4]>>,
    /// `input_ranges`: input name (`v4`) to its `(min, max)`.
    pub input_ranges: BTreeMap<String, (i32, i32)>,
}

/// The values of input `name` for `seed`, in its range if the entry gives one.
pub fn input_values(seed: u64, name: &str, ranges: &BTreeMap<String, (i32, i32)>) -> Vec<[f32; 4]> {
    match ranges.get(name) {
        Some(&(lo, hi)) => random::input_slot_in(seed, name, PIXELS, lo, hi),
        None => random::input_slot(seed, name, PIXELS),
    }
}

/// What a run produced: per output member, the value in every pixel (as `vec4`).
#[derive(Clone, Debug, Default)]
pub struct RunOutput {
    pub fields: Vec<(String, Vec<[f32; 4]>)>,
    /// Number of components compared per field.
    pub comps: BTreeMap<String, usize>,
    /// Uniform members that got values / that no CTAB constant claimed (all uniform buffers).
    pub filled: Vec<String>,
    pub unfilled: Vec<String>,
}

impl RunOutput {
    pub fn get(&self, name: &str) -> Option<&Vec<[f32; 4]>> {
        self.fields.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }
}

/// How an `In` member is fed.
enum Feed {
    /// `vec4`s from the input buffer starting at this slot.
    Slot(usize),
    /// `vec4(position.xy - 0.5, 0, 0)` of the fragment (D3D9 `vPos`).
    Vpos,
}

enum Kind {
    Uniform,
    StorageRead,
    StorageWrite,
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

fn parse_v(name: &str) -> bool {
    name.strip_prefix('v')
        .is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit()))
}

/// Slots the inputs take, in order, and the data of each.
struct InputPlan<'a> {
    feeds: Vec<(&'a Field, Feed)>,
    slots: Vec<Vec<[f32; 4]>>,
}

fn plan_inputs<'a>(s: &'a Subject, scene: &Scene) -> Result<InputPlan<'a>, HarnessError> {
    let mut plan = InputPlan {
        feeds: Vec::new(),
        slots: Vec::new(),
    };
    for f in &s.input {
        let n = f.name.as_str();
        if n == "pad" {
            // The oracle's placeholder for a body without inputs (WGSL has no empty structs).
            plan.feeds.push((f, Feed::Slot(plan.slots.len())));
            plan.slots.push(vec![[0.0; 4]; PIXELS]);
        } else if n == "vpos" {
            if s.stage != Stage::Ps {
                return Err(HarnessError::Contract(format!(
                    "{}: `vpos` is a pixel shader input",
                    s.label
                )));
            }
            plan.feeds.push((f, Feed::Vpos));
        } else if n == "vface" {
            plan.feeds.push((f, Feed::Slot(plan.slots.len())));
            plan.slots.push(random::face_slot(scene.seed, PIXELS));
        } else if parse_v(n) {
            if f.slots() != 1 {
                return Err(HarnessError::Contract(format!(
                    "{}: input `{n}` is a matrix",
                    s.label
                )));
            }
            plan.feeds.push((f, Feed::Slot(plan.slots.len())));
            plan.slots
                .push(input_values(scene.seed, n, &scene.input_ranges));
        } else if matches!(n, "bone_base" | "bone_base_next" | "bone_blend") {
            // `blend = 0`, `next = base` (4.8): the pose is the first one, nothing is blended.
            plan.feeds.push((f, Feed::Slot(plan.slots.len())));
            plan.slots.push(vec![[0.0; 4]; PIXELS]);
        } else if n.starts_with("instance_") {
            let c = scene.moved.get(n).ok_or_else(|| {
                HarnessError::Contract(format!(
                    "{}: input `{n}` has no data move (status.toml: data_moves, to = \"instance:{}\")",
                    s.label,
                    &n["instance_".len()..]
                ))
            })?;
            plan.feeds.push((f, Feed::Slot(plan.slots.len())));
            for slot in subject::move_slots(f, c, &scene.regs)? {
                plan.slots.push(vec![slot; PIXELS]);
            }
        } else {
            return Err(HarnessError::Contract(format!(
                "{}: input member `{n}` is not v<N>, vpos, vface, instance_<attr> or a bone member",
                s.label
            )));
        }
    }
    Ok(plan)
}

/// The text the wrapper uses for one input member; `idx` is the pixel (or vertex) index.
fn feed_expr(f: &Field, feed: &Feed, idx: &str) -> String {
    match feed {
        Feed::Vpos => {
            // `vpos` may be a vec2 or a vec4.
            f.from_slots(&|_| "vec4<f32>(pos.xy - vec2<f32>(0.5), 0.0, 0.0)".to_string())
        }
        Feed::Slot(start) => {
            f.from_slots(&|k| format!("harness_in[{}u * {PIXELS}u + {idx}]", start + k))
        }
    }
}

/// `harness_in` / `harness_out` declarations for the group `g`.
fn declarations(g: u32) -> String {
    format!(
        "\n// ---- harness wrapper (generated) ----\n\
         @group({g}) @binding(0) var<storage, read> harness_in: array<vec4<f32>>;\n\
         @group({g}) @binding(1) var<storage, read_write> harness_out: array<vec4<f32>>;\n"
    )
}

fn texture_view_dim(
    d: ImageDimension,
    arrayed: bool,
) -> Result<wgpu::TextureViewDimension, String> {
    Ok(match (d, arrayed) {
        (ImageDimension::D2, false) => wgpu::TextureViewDimension::D2,
        (ImageDimension::D3, false) => wgpu::TextureViewDimension::D3,
        (ImageDimension::Cube, false) => wgpu::TextureViewDimension::Cube,
        _ => return Err("only 2D, cube and 3D textures are supported".into()),
    })
}

fn describe(
    module: &naga::Module,
    info: &naga::valid::ModuleInfo,
    entry: &[&str],
) -> Result<Vec<Used>, HarnessError> {
    let mut used = Vec::new();
    for (handle, var) in module.global_variables.iter() {
        let Some(b) = &var.binding else { continue };
        let is_used = entry.iter().any(|e| {
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
                    Kind::StorageWrite
                } else {
                    Kind::StorageRead
                }
            }
            (AddressSpace::Handle, TypeInner::Sampler { .. }) => Kind::Sampler,
            (
                AddressSpace::Handle,
                TypeInner::Image {
                    dim,
                    arrayed,
                    class: ImageClass::Sampled { .. },
                },
            ) => Kind::Texture(
                texture_view_dim(*dim, *arrayed)
                    .map_err(|e| HarnessError::Contract(format!("texture `{name}`: {e}")))?,
            ),
            _ => {
                return Err(HarnessError::Contract(format!(
                    "resource `{name}` is of a kind the harness cannot feed"
                )));
            }
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

fn texture_of(gpu: &Gpu, t: &TexData) -> (wgpu::Texture, wgpu::TextureView) {
    use crate::sm3::ir::SamplerKind;
    let layers = t.layers();
    let (dimension, depth, view_dim) = match t.kind {
        SamplerKind::Cube => (
            wgpu::TextureDimension::D2,
            6,
            wgpu::TextureViewDimension::Cube,
        ),
        SamplerKind::Volume => (
            wgpu::TextureDimension::D3,
            t.size,
            wgpu::TextureViewDimension::D3,
        ),
        _ => (
            wgpu::TextureDimension::D2,
            1,
            wgpu::TextureViewDimension::D2,
        ),
    };
    let tex = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("harness texture"),
        size: wgpu::Extent3d {
            width: t.size,
            height: t.size,
            depth_or_array_layers: depth,
        },
        mip_level_count: t.mips.len() as u32,
        sample_count: 1,
        dimension,
        format: t.format.wgpu(),
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let bpt = t.format.bytes_per_texel() as u32;
    for (level, data) in t.mips.iter().enumerate() {
        let e = t.edge(level);
        let d = match t.kind {
            SamplerKind::Cube => layers,
            SamplerKind::Volume => e,
            _ => 1,
        };
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &tex,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(e * bpt),
                rows_per_image: Some(e),
            },
            wgpu::Extent3d {
                width: e,
                height: e,
                depth_or_array_layers: d,
            },
        );
    }
    let view = tex.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(view_dim),
        ..Default::default()
    });
    (tex, view)
}

fn bone_texture_of(gpu: &Gpu, texels: &[[f32; 4]]) -> (wgpu::Texture, wgpu::TextureView) {
    let tex = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("harness bone texture"),
        size: wgpu::Extent3d {
            width: BONE_TEXTURE_WIDTH,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba32Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let mut bytes = vec![0u8; BONE_TEXTURE_WIDTH as usize * 16];
    for (k, q) in texels.iter().take(BONE_TEXTURE_WIDTH as usize).enumerate() {
        for (j, x) in q.iter().enumerate() {
            bytes[k * 16 + j * 4..k * 16 + j * 4 + 4].copy_from_slice(&x.to_le_bytes());
        }
    }
    gpu.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(BONE_TEXTURE_WIDTH * 16),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: BONE_TEXTURE_WIDTH,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    (tex, view)
}

fn f32_bytes(v: &[[f32; 4]]) -> Vec<u8> {
    v.iter()
        .flat_map(|q| q.iter().flat_map(|x| x.to_le_bytes()))
        .collect()
}

/// The sampler's texture: `S_smp` / `S_sampler` / `S_samp` belongs to texture `S`.
fn texture_of_sampler(name: &str) -> &str {
    for suffix in ["_smp", "_sampler", "_samp"] {
        if let Some(t) = name.strip_suffix(suffix) {
            return t;
        }
    }
    name
}

/// The sampler a texture variable stands for: the variable is named like the CTAB constant, or,
/// when that is not a WGSL name (`FOWMask+FOWMask_texture`), as the oracle names it
/// (`tex<reg>_<name>`, see `oracle::regs`).
fn sampler_of<'a>(ctx: &EntryCtx<'a>, var: &str) -> Option<&'a SamplerRes> {
    ctx.samplers
        .iter()
        .find(|s| s.texture_var == var || s.ctab_name == var)
}

/// Runs `s` on the grid with the data of `scene`.
pub fn run_subject(
    gpu: &Gpu,
    s: &Subject,
    ctx: &EntryCtx<'_>,
    scene: &Scene,
) -> Result<RunOutput, HarnessError> {
    let plan = plan_inputs(s, scene)?;
    // Outputs: one slot each.
    let outs: Vec<(&Field, usize)> = s.output.iter().enumerate().map(|(k, f)| (f, k)).collect();
    if outs.is_empty() {
        return Err(HarnessError::Contract(format!(
            "{}: the body returns no members",
            s.label
        )));
    }
    // Group of the wrapper's own buffers: the one after the module's last.
    let base = naga::front::wgsl::parse_str(&s.wgsl)
        .map_err(|e| HarnessError::Wgsl(format!("{}: {}", s.label, e.emit_to_string(&s.wgsl))))?;
    let group = base
        .global_variables
        .iter()
        .filter_map(|(_, v)| v.binding.as_ref().map(|b| b.group + 1))
        .max()
        .unwrap_or(0);
    if group >= gpu.device.limits().max_bind_groups {
        return Err(HarnessError::Gpu(format!(
            "{}: the wrapper needs bind group {group}, the device has {}",
            s.label,
            gpu.device.limits().max_bind_groups
        )));
    }
    let mut src = s.wgsl.clone();
    src.push_str(&declarations(group));
    let (vs_entry, fs_entry) = ("harness_vs", "harness_fs");
    match s.stage {
        Stage::Ps => src.push_str(&ps::wrapper(s, &plan, &outs)?),
        Stage::Vs => {
            let limit = gpu.device.limits().max_inter_stage_shader_variables as usize;
            if outs.len() > limit {
                return Err(HarnessError::Contract(format!(
                    "{}: {} outputs, the device passes {limit} between the stages",
                    s.label,
                    outs.len()
                )));
            }
            src.push_str(&vs::wrapper(s, &plan, &outs)?);
        }
        other => {
            return Err(HarnessError::Contract(format!(
                "stage {other:?} cannot be compared"
            )));
        }
    }

    let module = naga::front::wgsl::parse_str(&src)
        .map_err(|e| HarnessError::Wgsl(format!("{}: {}", s.label, e.emit_to_string(&src))))?;
    let info = Validator::new(ValidationFlags::all(), Capabilities::empty())
        .validate(&module)
        .map_err(|e| HarnessError::Wgsl(format!("{}: {}", s.label, e.emit_to_string(&src))))?;
    let used = describe(&module, &info, &[vs_entry, fs_entry])?;

    let scope = gpu.device.push_error_scope(wgpu::ErrorFilter::Validation);

    // ---- resources ----
    enum Res {
        Buffer(wgpu::Buffer),
        View(wgpu::TextureView),
        Sampler(wgpu::Sampler),
    }
    let mut res: Vec<Res> = Vec::new();
    let mut keep: Vec<wgpu::Texture> = Vec::new();
    let mut output = RunOutput::default();
    let in_buf = {
        let mut flat: Vec<[f32; 4]> = plan.slots.iter().flatten().copied().collect();
        if flat.is_empty() {
            flat.push([0.0; 4]);
        }
        let b = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("harness_in"),
            size: (flat.len() * 16) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        gpu.queue.write_buffer(&b, 0, &f32_bytes(&flat));
        b
    };
    let out_size = (outs.len() * PIXELS * 16) as u64;
    let out_buf = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("harness_out"),
        size: out_size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let sampler = gpu.device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("harness sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::Repeat,
        mag_filter: if scene.linear {
            wgpu::FilterMode::Linear
        } else {
            wgpu::FilterMode::Nearest
        },
        min_filter: if scene.linear {
            wgpu::FilterMode::Linear
        } else {
            wgpu::FilterMode::Nearest
        },
        mipmap_filter: if scene.linear {
            wgpu::MipmapFilterMode::Linear
        } else {
            wgpu::MipmapFilterMode::Nearest
        },
        ..Default::default()
    });
    for u in &used {
        let var = &module.global_variables[u.var];
        res.push(match u.kind {
            Kind::StorageRead => Res::Buffer(in_buf.clone()),
            Kind::StorageWrite => Res::Buffer(out_buf.clone()),
            Kind::Uniform => {
                let bytes = match s.consts {
                    ConstSource::Oracle(cb) => {
                        if var.name.as_deref() != Some("ora") {
                            return Err(HarnessError::Contract(format!(
                                "{}: the oracle has no uniform `{}`",
                                s.label, u.name
                            )));
                        }
                        oracle_uniform_bytes(&cb, &scene.regs)
                    }
                    ConstSource::ByCtab => {
                        let ct = ctx.ctab.ok_or_else(|| {
                            HarnessError::Fill(format!(
                                "{}: uniform `{}` needs the CTAB of the original block",
                                s.label, u.name
                            ))
                        })?;
                        let f = fill_uniforms_by_ctab(&module, var, ct, &scene.regs)?;
                        output.filled.extend(f.filled);
                        output.unfilled.extend(f.unfilled);
                        f.bytes
                    }
                };
                let b = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("harness uniform"),
                    size: bytes.len().max(16) as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                gpu.queue.write_buffer(&b, 0, &bytes);
                Res::Buffer(b)
            }
            Kind::Texture(dim) => {
                if is_bone_texture(&u.name) {
                    let texels = scene.bone_texels.as_ref().ok_or_else(|| {
                        HarnessError::Contract(format!(
                            "{}: `{BONE_TEXTURE}` is read but no data move fills it",
                            s.label
                        ))
                    })?;
                    let (t, v) = bone_texture_of(gpu, texels);
                    keep.push(t);
                    Res::View(v)
                } else {
                    let t = sampler_of(ctx, &u.name)
                        .and_then(|sr| scene.textures.get(&sr.ctab_name))
                        .ok_or_else(|| {
                            HarnessError::Contract(format!(
                                "{}: texture `{}` is not a sampler of {}",
                                s.label, u.name, ctx.name
                            ))
                        })?;
                    let want = match t.kind {
                        crate::sm3::ir::SamplerKind::Cube => wgpu::TextureViewDimension::Cube,
                        crate::sm3::ir::SamplerKind::Volume => wgpu::TextureViewDimension::D3,
                        _ => wgpu::TextureViewDimension::D2,
                    };
                    if want != dim {
                        return Err(HarnessError::Contract(format!(
                            "{}: texture `{}` is {dim:?} here, the original samples a {want:?}",
                            s.label, u.name
                        )));
                    }
                    let (tex, v) = texture_of(gpu, t);
                    keep.push(tex);
                    Res::View(v)
                }
            }
            Kind::Sampler => {
                let tex_name = texture_of_sampler(&u.name);
                if sampler_of(ctx, tex_name).is_none() && !is_bone_texture(tex_name) {
                    return Err(HarnessError::Contract(format!(
                        "{}: sampler `{}` does not belong to a texture named like a CTAB sampler",
                        s.label, u.name
                    )));
                }
                Res::Sampler(sampler.clone())
            }
        });
    }

    // ---- layout, pipeline ----
    let groups = used.iter().map(|u| u.group + 1).max().unwrap_or(0);
    let mut layouts: Vec<wgpu::BindGroupLayout> = Vec::new();
    for g in 0..groups {
        let entries: Vec<wgpu::BindGroupLayoutEntry> = used
            .iter()
            .filter(|u| u.group == g)
            .map(|u| {
                let vis = if matches!(u.kind, Kind::StorageWrite) {
                    wgpu::ShaderStages::FRAGMENT
                } else {
                    wgpu::ShaderStages::VERTEX_FRAGMENT
                };
                wgpu::BindGroupLayoutEntry {
                    binding: u.binding,
                    visibility: vis,
                    ty: match &u.kind {
                        Kind::Uniform => wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        Kind::StorageRead => wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: true },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        Kind::StorageWrite => wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: false },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        Kind::Texture(dim) => wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float {
                                filterable: !is_bone_texture(&u.name),
                            },
                            view_dimension: *dim,
                            multisampled: false,
                        },
                        Kind::Sampler => {
                            wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)
                        }
                    },
                    count: None,
                }
            })
            .collect();
        layouts.push(
            gpu.device
                .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: Some("harness"),
                    entries: &entries,
                }),
        );
    }
    let layout_refs: Vec<Option<&wgpu::BindGroupLayout>> = layouts.iter().map(Some).collect();
    let pipeline_layout = gpu
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("harness"),
            bind_group_layouts: &layout_refs,
            immediate_size: 0,
        });
    let shader = gpu
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("harness"),
            source: wgpu::ShaderSource::Wgsl(src.clone().into()),
        });
    let pipeline = gpu
        .device
        .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("harness"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some(vs_entry),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState {
                topology: if s.stage == Stage::Vs {
                    wgpu::PrimitiveTopology::PointList
                } else {
                    wgpu::PrimitiveTopology::TriangleList
                },
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some(fs_entry),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    // Nothing is drawn to the target: the outputs go to `harness_out`.
                    write_mask: wgpu::ColorWrites::empty(),
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
    let mut bind_groups: Vec<wgpu::BindGroup> = Vec::new();
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
                    Res::Sampler(sm) => wgpu::BindingResource::Sampler(sm),
                },
            });
        }
        bind_groups.push(gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("harness"),
            layout,
            entries: &entries,
        }));
    }
    let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("harness target"),
        size: wgpu::Extent3d {
            width: GRID,
            height: GRID,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let target_view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let mut enc = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("harness"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target_view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Discard,
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
        if s.stage == Stage::Vs {
            pass.draw(0..PIXELS as u32, 0..1);
        } else {
            pass.draw(0..3, 0..1);
        }
    }
    gpu.queue.submit([enc.finish()]);
    let bytes = gpu.read_buffer(&out_buf, out_size);
    if let Some(e) = block_on(scope.pop()) {
        return Err(HarnessError::Gpu(format!("{}: {e}", s.label)));
    }
    drop(keep);

    for (f, k) in &outs {
        let at = k * PIXELS * 16;
        let vals: Vec<[f32; 4]> = (0..PIXELS)
            .map(|p| {
                let mut q = [0f32; 4];
                for (c, v) in q.iter_mut().enumerate() {
                    let o = at + p * 16 + c * 4;
                    *v = f32::from_le_bytes(bytes[o..o + 4].try_into().expect("4 bytes"));
                }
                q
            })
            .collect();
        output.comps.insert(f.name.clone(), f.comps().min(4));
        output.fields.push((f.name.clone(), vals));
    }
    output.filled.sort();
    output.filled.dedup();
    output.unfilled.sort();
    output.unfilled.dedup();
    Ok(output)
}

/// Builds the scene of one run for the original shader `ctx`: constants, textures in the formats
/// of the map, data moves.
pub fn make_scene(
    ctx: &EntryCtx<'_>,
    float_regs: usize,
    seed: u64,
    linear: bool,
    formats: &formats::Formats,
    zero_in_compare: &[String],
    moves: &[DataMove],
) -> Result<Scene, HarnessError> {
    let mut regs = Registers::random(seed, float_regs);
    let constant = |name: &str, what: &str| -> Result<CtabConstant, HarnessError> {
        ctx.ctab
            .and_then(|ct| ct.constants.iter().find(|c| c.name == name))
            .cloned()
            .ok_or_else(|| {
                HarnessError::Fill(format!(
                    "{what} names `{name}`, which {} has no constant of",
                    ctx.name
                ))
            })
    };
    for name in zero_in_compare {
        let c = constant(name, "zero_in_compare")?;
        regs.zero(c.reg as usize, c.count as usize);
    }
    let mut textures = BTreeMap::new();
    for s in ctx.samplers {
        let def = formats.for_sampler(ctx.name, &s.ctab_name);
        textures.insert(
            s.ctab_name.clone(),
            TexData::random(seed, &s.ctab_name, s.kind, def),
        );
    }
    let mut moved = BTreeMap::new();
    let mut bone_texels = None;
    for m in moves {
        let c = constant(&m.ctab, "a data move")?;
        if m.to == BONE_TEXTURE || m.to == BONE_TEXTURE_COLS4 {
            // The palette's registers become the texels of row 0, in register order (3 a bone).
            let palette: Vec<[f32; 4]> = (0..c.count as usize)
                .map(|k| regs.c.get(c.reg as usize + k).copied().unwrap_or([0.0; 4]))
                .collect();
            bone_texels = Some(if m.to == BONE_TEXTURE_COLS4 {
                // Registers 4b..4b+3 are the columns of bone b: texel 3b + j is row j of them.
                palette
                    .chunks_exact(4)
                    .flat_map(|cols| {
                        (0..3).map(move |j| [cols[0][j], cols[1][j], cols[2][j], cols[3][j]])
                    })
                    .collect()
            } else {
                palette
            });
        } else if let Some(attr) = m.to.strip_prefix("instance:") {
            moved.insert(format!("instance_{attr}"), c);
        } else {
            return Err(HarnessError::Fill(format!(
                "data move to `{}`: only `instance:<attr>`, `{BONE_TEXTURE}` and `{BONE_TEXTURE_COLS4}` exist",
                m.to
            )));
        }
    }
    Ok(Scene {
        seed,
        regs,
        textures,
        linear,
        moved,
        bone_texels,
        input_ranges: BTreeMap::new(),
    })
}
