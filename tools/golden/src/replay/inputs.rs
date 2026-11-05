//! Replay inputs: the draw call as `events.json` stores it, the buffers and
//! textures of the capture, and the decoding of vertices, DDS files and pixels.
//!
//! Everything here is plain data and CPU code; the GPU side is in `mod.rs` and `prefill.rs`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

// ------------------------------------------------------------------ the call

/// An element of the input layout.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Element {
    pub semantic_name: String,
    #[serde(default)]
    pub semantic_index: u32,
    pub format: String,
    #[serde(default)]
    pub input_slot: u32,
    #[serde(default)]
    pub byte_offset: u32,
    #[serde(default)]
    pub per_instance: bool,
    #[serde(default)]
    pub instance_data_step_rate: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct VertexBuf {
    pub slot: u32,
    pub resource: String,
    #[serde(default)]
    pub offset: u64,
    #[serde(default)]
    pub stride: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IndexBuf {
    pub resource: String,
    #[serde(default)]
    pub offset: u64,
    /// 2 or 4.
    pub stride: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlendEq {
    pub source: String,
    pub destination: String,
    pub operation: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlendTarget {
    pub enabled: bool,
    pub color_blend: BlendEq,
    pub alpha_blend: BlendEq,
    pub write_mask: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Blend {
    #[serde(default)]
    pub blend_factor: [f32; 4],
    #[serde(default)]
    pub blends: Vec<BlendTarget>,
    #[serde(default)]
    pub independent_blend: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StencilFace {
    pub function: String,
    pub pass_operation: String,
    pub fail_operation: String,
    pub depth_fail_operation: String,
    #[serde(default)]
    pub reference: u32,
    #[serde(default = "ff")]
    pub compare_mask: u32,
    #[serde(default = "ff")]
    pub write_mask: u32,
}

fn big_lod() -> f32 {
    f32::MAX
}

fn ff() -> u32 {
    0xff
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DepthStencil {
    pub depth_enable: bool,
    pub depth_function: String,
    pub depth_writes: bool,
    pub stencil_enable: bool,
    pub front_face: StencilFace,
    pub back_face: StencilFace,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Raster {
    pub cull_mode: String,
    #[serde(default = "yes")]
    pub front_ccw: bool,
    #[serde(default)]
    pub depth_bias: i32,
    #[serde(default)]
    pub depth_bias_clamp: f32,
    #[serde(default)]
    pub slope_scaled_depth_bias: f32,
    #[serde(default)]
    pub scissor_enable: bool,
    #[serde(default = "solid")]
    pub fill_mode: String,
}

fn yes() -> bool {
    true
}
fn solid() -> String {
    "Solid".into()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Viewport {
    pub enabled: bool,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub min_depth: f32,
    pub max_depth: f32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Scissor {
    pub enabled: bool,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// A bound target or shader resource (`_view` of `extract.py`).
#[derive(Debug, Clone, Deserialize)]
pub struct View {
    pub resource: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub view_format: String,
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    #[serde(default = "one")]
    pub samples: u32,
    #[serde(default)]
    pub num_mips: u32,
    #[serde(default)]
    pub first_mip: u32,
    #[serde(default)]
    pub dimension: String,
    /// Register of a shader resource.
    #[serde(default)]
    pub slot: u32,
    #[serde(default)]
    pub srgb: bool,
    #[serde(default)]
    pub buffer: bool,
}

fn one() -> u32 {
    1
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SamplerState {
    pub slot: u32,
    #[serde(default)]
    pub address_u: String,
    #[serde(default)]
    pub address_v: String,
    #[serde(default)]
    pub address_w: String,
    #[serde(default)]
    pub filter: SamplerFilter,
    #[serde(default)]
    pub max_anisotropy: f32,
    #[serde(default, rename = "minLOD")]
    pub min_lod: f32,
    #[serde(default = "big_lod", rename = "maxLOD")]
    pub max_lod: f32,
    #[serde(default)]
    pub mip_bias: f32,
    #[serde(default)]
    pub compare_function: String,
    #[serde(default)]
    pub border_color_value: BorderColor,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct SamplerFilter {
    #[serde(default)]
    pub filter: String,
    #[serde(default)]
    pub magnify: String,
    #[serde(default)]
    pub minify: String,
    #[serde(default)]
    pub mip: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BorderColor {
    #[serde(default)]
    pub float_value: [f32; 4],
}

#[derive(Debug, Clone, Deserialize)]
pub struct CBuffer {
    pub slot: u32,
    #[serde(default)]
    pub floats: Vec<f32>,
}

/// Something per shader stage (`vs`, `ps`, ...), as `events.json` keys it.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Stages<T: Default> {
    #[serde(default)]
    pub vs: T,
    #[serde(default)]
    pub ps: T,
}

/// A draw call of `events.json` with the state the replay needs. Fields a draw does not have
/// (events of other kinds) take their defaults.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Call {
    pub eid: u32,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub topology: String,
    #[serde(default)]
    pub index_count: u32,
    #[serde(default)]
    pub vertex_count: u32,
    #[serde(default)]
    pub instances: u32,
    /// Offsets of the draw (`extract.py` records them from the second parse on; absent in an older
    /// `events.json`, see [`Call::offsets_known`]).
    #[serde(default)]
    pub start_index: Option<u32>,
    #[serde(default)]
    pub base_vertex: i32,
    #[serde(default)]
    pub start_vertex: u32,
    #[serde(default)]
    pub start_instance: u32,
    #[serde(default)]
    pub input_layout: Vec<Element>,
    #[serde(default)]
    pub vertex_buffers: Vec<VertexBuf>,
    #[serde(default)]
    pub index_buffer: Option<IndexBuf>,
    #[serde(default)]
    pub blend: Option<Blend>,
    #[serde(default)]
    pub depth_stencil: Option<DepthStencil>,
    #[serde(default)]
    pub rasterizer: Option<Raster>,
    #[serde(default)]
    pub viewports: Vec<Viewport>,
    #[serde(default)]
    pub scissors: Vec<Scissor>,
    #[serde(default)]
    pub targets: Vec<View>,
    #[serde(default)]
    pub depth_target: Option<View>,
    #[serde(default)]
    pub textures: Stages<Vec<View>>,
    #[serde(default)]
    pub samplers: Stages<Vec<SamplerState>>,
    #[serde(default)]
    pub cbuffers: Stages<Vec<CBuffer>>,
}

impl Call {
    /// Whether the draw's offsets were recorded. An `events.json` from before `extract.py` wrote them
    /// has every draw at 0, which is wrong for a draw that does not start at the head of its buffers
    /// (the later draws of the FOW mask): the replay then compares other vertices than the capture's.
    pub fn offsets_known(&self) -> bool {
        self.start_index.is_some() || self.index_count == 0
    }
}

#[derive(Debug, Deserialize)]
struct EventsFile {
    calls: Vec<Call>,
}

/// A parsed capture on disk (`cache/golden/<scene>/<capture>/`).
pub struct CaptureData {
    pub dir: PathBuf,
    pub calls: BTreeMap<u32, Call>,
}

impl CaptureData {
    pub fn load(dir: &Path) -> Result<Self> {
        let p = dir.join("events.json");
        let text = std::fs::read_to_string(&p).with_context(|| p.display().to_string())?;
        let ev: EventsFile =
            serde_json::from_str(&text).with_context(|| p.display().to_string())?;
        let calls = ev
            .calls
            .into_iter()
            .filter(|c| !c.topology.is_empty())
            .map(|c| (c.eid, c))
            .collect();
        Ok(Self {
            dir: dir.to_path_buf(),
            calls,
        })
    }

    pub fn buffer(&self, resource: &str) -> Result<Vec<u8>> {
        let p = self.dir.join("buffers").join(format!("{resource}.bin"));
        std::fs::read(&p).with_context(|| format!("buffer {resource}: {}", p.display()))
    }

    pub fn target_file(&self, eid: u32, name: &str) -> PathBuf {
        self.dir.join("targets").join(format!("{eid}-{name}.dds"))
    }

    pub fn texture_file(&self, resource: &str) -> PathBuf {
        self.dir.join("textures").join(format!("{resource}.dds"))
    }
}

// ------------------------------------------------------------------ formats

/// How the replay treats a pixel or texel format.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fmt {
    pub wgpu: wgpu::TextureFormat,
    /// Block size in texels (4x4 for BC, else 1x1) and bytes per block.
    pub block: (u32, u32),
    pub bytes: u32,
    /// Channels, 1 to 4.
    pub channels: u32,
    pub kind: Kind,
    /// Normalised 8-bit channels: the tolerance is an absolute 2/255; floats scale with the value.
    pub unorm: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Unorm8,
    Unorm16,
    Half,
    Float,
    /// Block compressed: only for sampling.
    Bc,
    /// Packed depth 24 + stencil 8.
    D24S8,
}

/// The format of a RenderDoc name (`R8G8B8A8_UNORM`, `BC1_UNORM_SRGB`, ...). TYPELESS names are
/// read as UNORM.
pub fn fmt_by_name(name: &str) -> Option<Fmt> {
    use wgpu::TextureFormat as T;
    let srgb = name.contains("SRGB");
    let n = name.replace("_SRGB", "").replace("_TYPELESS", "_UNORM");
    let f = |w, bytes, channels, kind, unorm| Fmt {
        wgpu: w,
        block: (1, 1),
        bytes,
        channels,
        kind,
        unorm,
    };
    let bc = |w| Fmt {
        wgpu: w,
        block: (4, 4),
        bytes: 0,
        channels: 4,
        kind: Kind::Bc,
        unorm: true,
    };
    Some(match n.as_str() {
        "R8G8B8A8_UNORM" => f(
            if srgb {
                T::Rgba8UnormSrgb
            } else {
                T::Rgba8Unorm
            },
            4,
            4,
            Kind::Unorm8,
            true,
        ),
        "B8G8R8A8_UNORM" => f(
            if srgb {
                T::Bgra8UnormSrgb
            } else {
                T::Bgra8Unorm
            },
            4,
            4,
            Kind::Unorm8,
            true,
        ),
        "R8_UNORM" => f(T::R8Unorm, 1, 1, Kind::Unorm8, true),
        "R8G8_UNORM" => f(T::Rg8Unorm, 2, 2, Kind::Unorm8, true),
        "R16_UNORM" => f(T::R16Unorm, 2, 1, Kind::Unorm16, true),
        "R16G16_UNORM" => f(T::Rg16Unorm, 4, 2, Kind::Unorm16, true),
        "R16G16B16A16_UNORM" => f(T::Rgba16Unorm, 8, 4, Kind::Unorm16, true),
        "R16_FLOAT" => f(T::R16Float, 2, 1, Kind::Half, false),
        "R16G16_FLOAT" => f(T::Rg16Float, 4, 2, Kind::Half, false),
        "R16G16B16A16_FLOAT" => f(T::Rgba16Float, 8, 4, Kind::Half, false),
        "R32_FLOAT" => f(T::R32Float, 4, 1, Kind::Float, false),
        "R32G32_FLOAT" => f(T::Rg32Float, 8, 2, Kind::Float, false),
        "R32G32B32A32_FLOAT" => f(T::Rgba32Float, 16, 4, Kind::Float, false),
        "R11G11B10_FLOAT" => f(T::Rg11b10Ufloat, 4, 3, Kind::Float, false),
        "D24S8" | "D24_UNORM_S8_UINT" | "R24G8_UNORM" => Fmt {
            wgpu: T::Depth24PlusStencil8,
            block: (1, 1),
            bytes: 4,
            channels: 1,
            kind: Kind::D24S8,
            unorm: true,
        },
        "BC1_UNORM" => Fmt {
            bytes: 8,
            ..bc(if srgb {
                T::Bc1RgbaUnormSrgb
            } else {
                T::Bc1RgbaUnorm
            })
        },
        "BC2_UNORM" => Fmt {
            bytes: 16,
            ..bc(if srgb {
                T::Bc2RgbaUnormSrgb
            } else {
                T::Bc2RgbaUnorm
            })
        },
        "BC3_UNORM" => Fmt {
            bytes: 16,
            ..bc(if srgb {
                T::Bc3RgbaUnormSrgb
            } else {
                T::Bc3RgbaUnorm
            })
        },
        "BC4_UNORM" => Fmt {
            bytes: 8,
            channels: 1,
            ..bc(T::Bc4RUnorm)
        },
        "BC5_UNORM" => Fmt {
            bytes: 16,
            channels: 2,
            ..bc(T::Bc5RgUnorm)
        },
        "BC7_UNORM" => Fmt {
            bytes: 16,
            ..bc(if srgb {
                T::Bc7RgbaUnormSrgb
            } else {
                T::Bc7RgbaUnorm
            })
        },
        _ => return None,
    })
}

/// DXGI numbers of the DDS header to RenderDoc names.
fn dxgi_name(id: u32) -> Option<&'static str> {
    Some(match id {
        2 => "R32G32B32A32_FLOAT",
        10 => "R16G16B16A16_FLOAT",
        11 => "R16G16B16A16_UNORM",
        16 => "R32G32_FLOAT",
        26 => "R11G11B10_FLOAT",
        27 | 28 => "R8G8B8A8_UNORM",
        29 => "R8G8B8A8_UNORM_SRGB",
        34 => "R16G16_FLOAT",
        35 => "R16G16_UNORM",
        40..=41 => "R32_FLOAT",
        45 => "D24_UNORM_S8_UINT",
        49 => "R8G8_UNORM",
        54 => "R16_FLOAT",
        56 => "R16_UNORM",
        61 => "R8_UNORM",
        70 | 71 => "BC1_UNORM",
        72 => "BC1_UNORM_SRGB",
        73 | 74 => "BC2_UNORM",
        75 => "BC2_UNORM_SRGB",
        76 | 77 => "BC3_UNORM",
        78 => "BC3_UNORM_SRGB",
        79 | 80 => "BC4_UNORM",
        82 | 83 => "BC5_UNORM",
        87 => "B8G8R8A8_UNORM",
        91 => "B8G8R8A8_UNORM_SRGB",
        98 | 99 => "BC7_UNORM",
        100 => "BC7_UNORM_SRGB",
        _ => return None,
    })
}

/// A DDS file of the capture, split into its images.
pub struct Dds {
    pub name: &'static str,
    pub fmt: Fmt,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub mips: u32,
    pub layers: u32,
    /// `images[layer][mip]`.
    pub images: Vec<Vec<Vec<u8>>>,
}

fn rd(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("4 bytes"))
}

/// Size of one mip in bytes: `(row bytes, rows, depth slices)`.
pub fn mip_layout(f: &Fmt, w: u32, h: u32, d: u32, mip: u32) -> (u32, u32, u32) {
    let (mw, mh, md) = ((w >> mip).max(1), (h >> mip).max(1), (d >> mip).max(1));
    let (bw, bh) = f.block;
    (mw.div_ceil(bw) * f.bytes, mh.div_ceil(bh), md)
}

impl Dds {
    pub fn read(bytes: &[u8]) -> Result<Dds> {
        if bytes.len() < 128 || &bytes[..4] != b"DDS " {
            bail!("not a DDS file");
        }
        let height = rd(bytes, 12);
        let width = rd(bytes, 16);
        let depth = rd(bytes, 24).max(1);
        let mips = rd(bytes, 28).max(1);
        if &bytes[84..88] != b"DX10" {
            bail!("only DDS files with a DX10 header are read");
        }
        let id = rd(bytes, 128);
        let layers = rd(bytes, 140).max(1);
        let name = dxgi_name(id).with_context(|| format!("DXGI format {id} is not supported"))?;
        let fmt = fmt_by_name(name).with_context(|| format!("format {name}"))?;
        let mut at = 148;
        let mut images = Vec::new();
        for _ in 0..layers {
            let mut per_mip = Vec::new();
            for m in 0..mips {
                let (row, rows, slices) = mip_layout(&fmt, width, height, depth, m);
                let n = (row * rows * slices) as usize;
                let img = bytes
                    .get(at..at + n)
                    .with_context(|| format!("DDS ends inside mip {m}"))?;
                per_mip.push(img.to_vec());
                at += n;
            }
            images.push(per_mip);
        }
        Ok(Dds {
            name,
            fmt,
            width,
            height,
            depth,
            mips,
            layers,
            images,
        })
    }

    pub fn load(path: &Path) -> Result<Dds> {
        let b = std::fs::read(path).with_context(|| path.display().to_string())?;
        Dds::read(&b).with_context(|| path.display().to_string())
    }
}

/// Pixels of a mip-0 image as four floats each (unused channels 0, alpha 1 where there is none).
pub fn decode_pixels(f: &Fmt, bytes: &[u8]) -> Result<Vec<[f32; 4]>> {
    let n = f.bytes as usize;
    if n == 0 || !bytes.len().is_multiple_of(n) {
        bail!("cannot decode {:?} pixels", f.wgpu);
    }
    let mut out = Vec::with_capacity(bytes.len() / n);
    for px in bytes.chunks_exact(n) {
        let mut q = [0.0f32, 0.0, 0.0, 1.0];
        let ch = f.channels as usize;
        match f.kind {
            Kind::Unorm8 => {
                for c in 0..ch {
                    q[c] = f32::from(px[c]) / 255.0;
                }
            }
            Kind::Unorm16 => {
                for c in 0..ch {
                    q[c] = f32::from(u16::from_le_bytes([px[2 * c], px[2 * c + 1]])) / 65535.0;
                }
            }
            Kind::Half => {
                for c in 0..ch {
                    q[c] = half_to_f32(u16::from_le_bytes([px[2 * c], px[2 * c + 1]]));
                }
            }
            Kind::Float => {
                for c in 0..ch {
                    q[c] = f32::from_le_bytes(px[4 * c..4 * c + 4].try_into().expect("4"));
                }
            }
            Kind::Bc | Kind::D24S8 => bail!("{:?} is not a colour pixel format", f.wgpu),
        }
        out.push(q);
    }
    Ok(out)
}

pub fn half_to_f32(h: u16) -> f32 {
    let s = u32::from(h >> 15) << 31;
    let e = u32::from((h >> 10) & 0x1f);
    let m = u32::from(h & 0x3ff);
    let bits = match (e, m) {
        (0, 0) => s,
        (0, m) => {
            // Subnormal: renormalise.
            let mut e = 127 - 15 + 1;
            let mut m = m;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            s | (e << 23) | ((m & 0x3ff) << 13)
        }
        (0x1f, m) => s | 0x7f80_0000 | (m << 13),
        (e, m) => s | ((e + 127 - 15) << 23) | (m << 13),
    };
    f32::from_bits(bits)
}

// ------------------------------------------------------------------ vertices

/// The components of a vertex element as four floats (D3D rules: missing components 0, w = 1;
/// UINT / SINT converted to their numeric value, as the D3D9 shaders read them).
pub fn decode_element(format: &str, b: &[u8]) -> Result<[f32; 4]> {
    // `R32G32B32_FLOAT`: letters R G B A with a width each, then the kind.
    let (comps, kind) = format
        .split_once('_')
        .with_context(|| format!("vertex format {format}"))?;
    let mut widths = Vec::new();
    let mut chars = comps.chars().peekable();
    while let Some(c) = chars.next() {
        if !"RGBA".contains(c) {
            bail!("vertex format {format}");
        }
        let mut w = 0u32;
        while let Some(d) = chars.peek().and_then(|d| d.to_digit(10)) {
            w = w * 10 + d;
            chars.next();
        }
        widths.push(w);
    }
    let mut q = [0.0f32, 0.0, 0.0, 1.0];
    let mut at = 0usize;
    for (c, &w) in widths.iter().enumerate().take(4) {
        let n = (w / 8) as usize;
        let raw = b
            .get(at..at + n)
            .with_context(|| format!("vertex data ends inside {format}"))?;
        at += n;
        q[c] = match (kind, w) {
            ("FLOAT", 32) => f32::from_le_bytes(raw.try_into().expect("4")),
            ("FLOAT", 16) => half_to_f32(u16::from_le_bytes([raw[0], raw[1]])),
            ("UNORM", 8) => f32::from(raw[0]) / 255.0,
            ("UNORM", 16) => f32::from(u16::from_le_bytes([raw[0], raw[1]])) / 65535.0,
            ("SNORM", 8) => (f32::from(raw[0] as i8) / 127.0).max(-1.0),
            ("SNORM", 16) => (f32::from(i16::from_le_bytes([raw[0], raw[1]])) / 32767.0).max(-1.0),
            ("UINT", 8) => f32::from(raw[0]),
            ("UINT", 16) => f32::from(u16::from_le_bytes([raw[0], raw[1]])),
            ("UINT", 32) => u32::from_le_bytes(raw.try_into().expect("4")) as f32,
            ("SINT", 8) => f32::from(raw[0] as i8),
            ("SINT", 16) => f32::from(i16::from_le_bytes([raw[0], raw[1]])),
            ("SINT", 32) => i32::from_le_bytes(raw.try_into().expect("4")) as f32,
            _ => bail!("vertex format {format} is not supported"),
        };
    }
    Ok(q)
}

/// The vertices of a draw, one entry per drawn vertex (the instances and the index buffer already
/// applied), for the elements named `want` as `(semantic lower case, index)`. Returns
/// `want.len()` arrays of `[f32; 4]`, vertex-major.
pub struct DrawVertices {
    /// `data[vertex][k]` is the value for `want[k]`.
    pub data: Vec<Vec<[f32; 4]>>,
    pub missing: Vec<(String, u32)>,
}

pub fn fetch_vertices(
    cap: &dyn BufferSource,
    call: &Call,
    want: &[(String, u32)],
    limit: usize,
) -> Result<DrawVertices> {
    let indexed = call.index_count > 0;
    let count = if indexed {
        call.index_count
    } else {
        call.vertex_count
    } as usize;
    let instances = call.instances.max(1) as usize;
    if count * instances > limit {
        bail!(
            "{} vertices ({} x {} instances) are over the replay limit {limit}",
            count * instances,
            count,
            instances
        );
    }
    // Vertex numbers of one instance.
    let numbers: Vec<u32> = if indexed {
        let ib = call
            .index_buffer
            .as_ref()
            .context("an indexed draw without an index buffer")?;
        let data = cap.buffer(&ib.resource)?;
        let stride = ib.stride.unwrap_or(2) as usize;
        let first = ib.offset as usize + call.start_index.unwrap_or(0) as usize * stride;
        (0..count)
            .map(|i| {
                let at = first + i * stride;
                let raw = data
                    .get(at..at + stride)
                    .with_context(|| format!("index {i} is past the end of the index buffer"))?;
                Ok(match stride {
                    2 => u32::from(u16::from_le_bytes([raw[0], raw[1]])),
                    4 => u32::from_le_bytes(raw.try_into().expect("4")),
                    s => bail!("index stride {s}"),
                })
            })
            .collect::<Result<_>>()?
    } else {
        (call.start_vertex..call.start_vertex + count as u32).collect()
    };
    let mut missing = Vec::new();
    // Per wanted element: its layout entry and buffer.
    let mut plan = Vec::new();
    let mut buffers: BTreeMap<u32, Vec<u8>> = BTreeMap::new();
    for (name, idx) in want {
        let el = call
            .input_layout
            .iter()
            .find(|e| e.semantic_name.eq_ignore_ascii_case(name) && e.semantic_index == *idx);
        match el {
            None => {
                missing.push((name.clone(), *idx));
                plan.push(None);
            }
            Some(e) => {
                let vb = call
                    .vertex_buffers
                    .iter()
                    .find(|v| v.slot == e.input_slot)
                    .with_context(|| format!("no vertex buffer in slot {}", e.input_slot))?;
                if let std::collections::btree_map::Entry::Vacant(slot) = buffers.entry(vb.slot) {
                    slot.insert(cap.buffer(&vb.resource)?);
                }
                plan.push(Some((e.clone(), vb.clone())));
            }
        }
    }
    let mut data = Vec::with_capacity(count * instances);
    for inst in 0..instances {
        let inst_no = inst as u32 + call.start_instance;
        for &n in &numbers {
            let vertex = i64::from(n) + i64::from(call.base_vertex);
            let mut row = Vec::with_capacity(plan.len());
            for p in &plan {
                let Some((e, vb)) = p else {
                    row.push([0.0, 0.0, 0.0, 1.0]);
                    continue;
                };
                let step = if e.per_instance {
                    i64::from(inst_no / e.instance_data_step_rate.max(1))
                } else {
                    vertex
                };
                let at = vb.offset as i64 + step * vb.stride as i64 + i64::from(e.byte_offset);
                let buf = &buffers[&vb.slot];
                let at = usize::try_from(at).context("negative vertex address")?;
                let raw = buf.get(at..).unwrap_or(&[]);
                row.push(
                    decode_element(&e.format, raw)
                        .with_context(|| format!("vertex {vertex}, {}", e.semantic_name))?,
                );
            }
            data.push(row);
        }
    }
    Ok(DrawVertices { data, missing })
}

/// Where buffer contents come from: the capture folder, or a test.
pub trait BufferSource {
    fn buffer(&self, resource: &str) -> Result<Vec<u8>>;
}

impl BufferSource for CaptureData {
    fn buffer(&self, resource: &str) -> Result<Vec<u8>> {
        CaptureData::buffer(self, resource)
    }
}

/// Buffers held in memory (the self-test's "capture").
#[derive(Default)]
pub struct MemBuffers(pub BTreeMap<String, Vec<u8>>);

impl BufferSource for MemBuffers {
    fn buffer(&self, resource: &str) -> Result<Vec<u8>> {
        self.0
            .get(resource)
            .cloned()
            .with_context(|| format!("no buffer {resource}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halves_decode() {
        assert_eq!(half_to_f32(0x3c00), 1.0);
        assert_eq!(half_to_f32(0xc000), -2.0);
        assert_eq!(half_to_f32(0), 0.0);
        assert!((half_to_f32(0x3555) - 0.333_251_95).abs() < 1e-6);
    }

    #[test]
    fn elements_decode() {
        let mut b = Vec::new();
        for v in [1.0f32, 2.0, 3.0] {
            b.extend(v.to_le_bytes());
        }
        assert_eq!(
            decode_element("R32G32B32_FLOAT", &b).unwrap(),
            [1.0, 2.0, 3.0, 1.0]
        );
        assert_eq!(
            decode_element("R8G8B8A8_UINT", &[1, 2, 3, 250]).unwrap(),
            [1.0, 2.0, 3.0, 250.0]
        );
        assert_eq!(
            decode_element("R8G8B8A8_UNORM", &[255, 0, 0, 255]).unwrap(),
            [1.0, 0.0, 0.0, 1.0]
        );
    }

    #[test]
    fn formats_by_name() {
        assert_eq!(
            fmt_by_name("BC1_UNORM_SRGB").unwrap().wgpu,
            wgpu::TextureFormat::Bc1RgbaUnormSrgb
        );
        assert_eq!(
            fmt_by_name("R8G8B8A8_TYPELESS").unwrap().wgpu,
            wgpu::TextureFormat::Rgba8Unorm
        );
        assert!(fmt_by_name("NOT_A_FORMAT").is_none());
    }
}
