//! `TextureBlob` → wgpu texture: format mapping, per-mip copy layout and upload.

pub mod bc_decode;

use crate::perf::{Cluster, DeviceTrack, Tex};
use std::sync::atomic::{AtomicU64, Ordering};

use asset_format::{PlatformProfile, TexDim, TextureBlob, TextureFormat, VariantFormat};

/// Block edge of BC7 and ASTC 4×4.
const BLOCK: u32 = 4;

/// Device format for a blob format. `srgb` picks the `*Srgb` flavour, so the sampler decodes to linear.
pub(crate) fn wgpu_format(format: TextureFormat, srgb: bool) -> wgpu::TextureFormat {
    match (format, srgb) {
        (TextureFormat::Rgba8, true) => wgpu::TextureFormat::Rgba8UnormSrgb,
        (TextureFormat::Rgba8, false) => wgpu::TextureFormat::Rgba8Unorm,
        // The world formats carry no sRGB variant: shaders do their own gamma (AB-D2).
        (TextureFormat::Bc1, true) => wgpu::TextureFormat::Bc1RgbaUnormSrgb,
        (TextureFormat::Bc1, false) => wgpu::TextureFormat::Bc1RgbaUnorm,
        (TextureFormat::Bc2, true) => wgpu::TextureFormat::Bc2RgbaUnormSrgb,
        (TextureFormat::Bc2, false) => wgpu::TextureFormat::Bc2RgbaUnorm,
        (TextureFormat::Bc3, true) => wgpu::TextureFormat::Bc3RgbaUnormSrgb,
        (TextureFormat::Bc3, false) => wgpu::TextureFormat::Bc3RgbaUnorm,
        (TextureFormat::Bc4, _) => wgpu::TextureFormat::Bc4RUnorm,
        (TextureFormat::Bc5, _) => wgpu::TextureFormat::Bc5RgUnorm,
        (TextureFormat::R8, _) => wgpu::TextureFormat::R8Unorm,
        (TextureFormat::Rg8, _) => wgpu::TextureFormat::Rg8Unorm,
        (TextureFormat::R16F, _) => wgpu::TextureFormat::R16Float,
        (TextureFormat::Rgba16F, _) => wgpu::TextureFormat::Rgba16Float,
        (TextureFormat::Bc7, true) => wgpu::TextureFormat::Bc7RgbaUnormSrgb,
        (TextureFormat::Bc7, false) => wgpu::TextureFormat::Bc7RgbaUnorm,
        (TextureFormat::Astc4x4, srgb) => wgpu::TextureFormat::Astc {
            block: wgpu::AstcBlock::B4x4,
            channel: if srgb {
                wgpu::AstcChannel::UnormSrgb
            } else {
                wgpu::AstcChannel::Unorm
            },
        },
    }
}

/// Device feature a blob format needs.
pub(crate) fn required_features(format: TextureFormat) -> wgpu::Features {
    match format {
        TextureFormat::Rgba8
        | TextureFormat::R8
        | TextureFormat::Rg8
        | TextureFormat::R16F
        | TextureFormat::Rgba16F => wgpu::Features::empty(),
        TextureFormat::Bc1
        | TextureFormat::Bc2
        | TextureFormat::Bc3
        | TextureFormat::Bc4
        | TextureFormat::Bc5
        | TextureFormat::Bc7 => wgpu::Features::TEXTURE_COMPRESSION_BC,
        TextureFormat::Astc4x4 => wgpu::Features::TEXTURE_COMPRESSION_ASTC,
    }
}

/// Variants the device takes, most preferred first: the compressed ones it samples, then
/// `Uncompressed`. A device without BC lists `Bc` last: [`upload_blob`] decodes BC1-BC5 on the CPU,
/// which is the way of a texture that has neither an `Astc` nor an `Uncompressed` variant (a
/// phone gets ASTC for every texture but the written exceptions).
pub(crate) fn profile_for_features(features: wgpu::Features) -> PlatformProfile {
    let bc = features.contains(wgpu::Features::TEXTURE_COMPRESSION_BC);
    let mut formats = Vec::new();
    if bc {
        formats.push(VariantFormat::Bc);
    }
    if features.contains(wgpu::Features::TEXTURE_COMPRESSION_ASTC) {
        formats.push(VariantFormat::Astc);
    }
    formats.push(VariantFormat::Uncompressed);
    if !bc {
        formats.push(VariantFormat::Bc);
    }
    PlatformProfile::new(formats)
}

/// Where a mip goes: copy extent and row stride.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MipLayout {
    /// Copy extent in texels; rounded up to whole blocks for compressed formats.
    pub width: u32,
    pub height: u32,
    pub bytes_per_row: u32,
    /// Rows of texels (uncompressed) or of blocks (compressed).
    pub rows: u32,
}

pub(crate) fn mip_layout(format: TextureFormat, base: (u32, u32), level: usize) -> MipLayout {
    let w = (base.0 >> level).max(1);
    let h = (base.1 >> level).max(1);
    let dim = format.block_dim();
    let (bw, bh) = (w.div_ceil(dim), h.div_ceil(dim));
    MipLayout {
        width: bw * dim,
        height: bh * dim,
        bytes_per_row: bw * format.block_bytes(),
        rows: bh,
    }
}

/// How a blob is turned into a texture.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UploadOptions {
    /// Ignore the blob's `srgb` flag and upload raw `*Unorm` values (game textures: the ported
    /// shaders do their own gamma).
    pub linear: bool,
    /// Decode BC1-BC5 on the CPU even when the device could sample them (tests, and the path of
    /// a device without `TEXTURE_COMPRESSION_BC`).
    pub force_cpu_bc: bool,
}

impl UploadOptions {
    /// Game textures: no sRGB.
    pub const WORLD: Self = Self {
        linear: true,
        force_cpu_bc: false,
    };
}

/// An uploaded texture and what the bind group layout needs to know about it.
#[derive(Debug)]
pub struct Uploaded {
    pub texture: Tex,
    pub view_dimension: wgpu::TextureViewDimension,
    /// Format the texture has on the device (after a CPU decode: the decoded one).
    pub format: wgpu::TextureFormat,
    /// BC1-BC5 were decoded on the CPU (memory use 4-8x, reported in the logs and `?stats=1`).
    pub cpu_decoded: bool,
}

/// Bytes of BC textures decoded on the CPU so far (output size). For `?stats=1`.
static CPU_DECODED_BYTES: AtomicU64 = AtomicU64::new(0);

pub fn cpu_decoded_bytes() -> u64 {
    CPU_DECODED_BYTES.load(Ordering::Relaxed)
}

/// Number of 2D slices in a level of `blob`.
fn level_slices(blob: &TextureBlob, level: usize) -> usize {
    match blob.dim {
        TexDim::D2 => 1,
        TexDim::Cube => 6,
        TexDim::D3 => (blob.depth >> level).max(1) as usize,
    }
}

/// `blob` with BC1-BC5 decoded to RGBA8, R8 or RG8, level by level and slice by slice.
/// `None` when the format is not BC1-BC5 or the data is short.
pub fn decode_blob(blob: &TextureBlob) -> Option<TextureBlob> {
    let format = bc_decode::decoded_format(blob.format)?;
    let mut mips = Vec::with_capacity(blob.mips.len());
    for (level, data) in blob.mips.iter().enumerate() {
        let (w, h) = ((blob.width >> level).max(1), (blob.height >> level).max(1));
        let slices = level_slices(blob, level);
        let slice_bytes = blob.format.mip_size(w, h);
        let mut out = Vec::with_capacity(slices * format.mip_size(w, h));
        for s in 0..slices {
            let src = data.get(s * slice_bytes..(s + 1) * slice_bytes)?;
            out.extend(bc_decode::decode_slice(blob.format, src, w, h)?);
        }
        mips.push(out);
    }
    Some(TextureBlob {
        format,
        mips,
        ..blob.clone()
    })
}

pub(crate) fn upload(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    blob: &TextureBlob,
    label: &str,
) -> Result<Tex, String> {
    upload_blob(device, queue, blob, label, UploadOptions::default()).map(|u| u.texture)
}

/// Uploads any `C5TX` v2 texture: `D2`, `Cube` (six array layers) or `D3`, every format.
/// BC1-BC5 are decoded on the CPU when the device lacks
/// `TEXTURE_COMPRESSION_BC` (and, for `D3`, `TEXTURE_COMPRESSION_BC_SLICED_3D`).
pub fn upload_blob(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    blob: &TextureBlob,
    label: &str,
    opts: UploadOptions,
) -> Result<Uploaded, String> {
    let features = device.features();
    let bc15 = bc_decode::decoded_format(blob.format).is_some();
    // WebGPU makes a block-compressed texture only of whole blocks; a few DXT originals of the UI
    // are 45x45 or 214x214: those go up decoded.
    let whole_blocks = blob.width.is_multiple_of(BLOCK) && blob.height.is_multiple_of(BLOCK);
    let sampleable = features.contains(wgpu::Features::TEXTURE_COMPRESSION_BC)
        && (blob.dim != TexDim::D3
            || features.contains(wgpu::Features::TEXTURE_COMPRESSION_BC_SLICED_3D))
        && whole_blocks;
    if bc15 && (opts.force_cpu_bc || !sampleable) {
        let decoded = decode_blob(blob).ok_or_else(|| format!("{label}: corrupt BC data"))?;
        let bytes: usize = decoded.mips.iter().map(Vec::len).sum();
        CPU_DECODED_BYTES.fetch_add(bytes as u64, Ordering::Relaxed);
        tracing::warn!(
            "texture {label}: {:?} {}x{} decoded on the CPU ({bytes} bytes): no BC support on the device, or not whole blocks",
            blob.format,
            blob.width,
            blob.height
        );
        let mut up = upload_blob(device, queue, &decoded, label, opts)?;
        up.cpu_decoded = true;
        return Ok(up);
    }
    let needed = required_features(blob.format);
    if !features.contains(needed) {
        return Err(format!("{:?} textures need {needed:?}", blob.format));
    }
    if blob.dim == TexDim::D3 && blob.format.is_compressed() {
        let sliced = match blob.format {
            TextureFormat::Astc4x4 => wgpu::Features::TEXTURE_COMPRESSION_ASTC_SLICED_3D,
            _ => wgpu::Features::TEXTURE_COMPRESSION_BC_SLICED_3D,
        };
        if !features.contains(sliced) {
            return Err(format!("{:?} volumes need {sliced:?}", blob.format));
        }
    }
    if blob.format.is_compressed()
        && (!blob.width.is_multiple_of(BLOCK) || !blob.height.is_multiple_of(BLOCK))
    {
        return Err(format!(
            "{:?} texture {}×{} is not a multiple of the 4×4 block",
            blob.format, blob.width, blob.height
        ));
    }
    let limits = device.limits();
    let (dimension, view_dimension, side_max, depth_or_layers) = match blob.dim {
        TexDim::D2 => (
            wgpu::TextureDimension::D2,
            wgpu::TextureViewDimension::D2,
            limits.max_texture_dimension_2d,
            1,
        ),
        TexDim::Cube => (
            wgpu::TextureDimension::D2,
            wgpu::TextureViewDimension::Cube,
            limits.max_texture_dimension_2d,
            6,
        ),
        TexDim::D3 => (
            wgpu::TextureDimension::D3,
            wgpu::TextureViewDimension::D3,
            limits.max_texture_dimension_3d,
            blob.depth,
        ),
    };
    if blob.width.max(blob.height) > side_max || (blob.dim == TexDim::D3 && blob.depth > side_max) {
        return Err(format!(
            "texture {}×{}×{depth_or_layers} exceeds the device limit {side_max}",
            blob.width, blob.height
        ));
    }
    let format = wgpu_format(blob.format, blob.srgb && !opts.linear);
    let texture = device.tex(
        &wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: blob.width,
                height: blob.height,
                depth_or_array_layers: depth_or_layers,
            },
            mip_level_count: blob.mips.len() as u32,
            sample_count: 1,
            dimension,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        Cluster::Models,
    );
    for (level, data) in blob.mips.iter().enumerate() {
        let l = mip_layout(blob.format, (blob.width, blob.height), level);
        let copy = |origin_z: u32, depth: u32, bytes: &[u8]| {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: origin_z,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                bytes,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(l.bytes_per_row),
                    rows_per_image: Some(l.rows),
                },
                wgpu::Extent3d {
                    width: l.width,
                    height: l.height,
                    depth_or_array_layers: depth,
                },
            );
        };
        match blob.dim {
            TexDim::D2 => copy(0, 1, data),
            // The faces of a level sit back to back (+X, -X, +Y, -Y, +Z, -Z).
            TexDim::Cube => {
                let face = data.len() / 6;
                for f in 0..6u32 {
                    copy(f, 1, &data[f as usize * face..(f as usize + 1) * face]);
                }
            }
            TexDim::D3 => copy(0, (blob.depth >> level).max(1), data),
        }
    }
    Ok(Uploaded {
        texture,
        view_dimension,
        format,
        cpu_decoded: false,
    })
}

/// Bytes of one cell of a mip: a texel (RGBA8) or a 4x4 block.
fn cell_bytes(format: TextureFormat) -> usize {
    format.block_bytes() as usize
}

/// Size of a mip level in cells (texels or 4x4 blocks).
fn level_cells(format: TextureFormat, base: (u32, u32), level: usize) -> (u32, u32) {
    let (w, h) = ((base.0 >> level).max(1), (base.1 >> level).max(1));
    let dim = format.block_dim();
    (w.div_ceil(dim), h.div_ceil(dim))
}

/// Mip `level` of a `target` sized layer made by repeating `blob` (whose sides divide the target's
/// by the same power of two). Level L of the layer repeats level L of the source, which is exact
/// while the source level still has whole cells; below that (a compressed source under 4x4, or no
/// levels left) the smallest such level is repeated instead, which is close enough for the last
/// few mips.
pub(crate) fn tiled_level(blob: &TextureBlob, target: (u32, u32), level: usize) -> Vec<u8> {
    let format = blob.format;
    let unit = cell_bytes(format);
    let base = (blob.width, blob.height);
    let min_side = format.block_dim();
    let usable = (0..blob.mips.len())
        .rev()
        .find(|&l| (base.0 >> l).max(1).min((base.1 >> l).max(1)) >= min_side)
        .unwrap_or(0);
    let src_level = level.min(usable);
    let (sw, sh) = level_cells(format, base, src_level);
    let (ow, oh) = level_cells(format, target, level);
    let src = &blob.mips[src_level];
    let mut out = Vec::with_capacity(ow as usize * oh as usize * unit);
    for y in 0..oh {
        let row = (y % sh) as usize * sw as usize;
        for x in 0..ow {
            let at = (row + (x % sw) as usize) * unit;
            out.extend_from_slice(&src[at..at + unit]);
        }
    }
    out
}

/// Layers to make an array texture of `layers` with, so that the GL backend (WebGL2) makes it a
/// `TEXTURE_2D_ARRAY`: wgpu-hal picks the GL target from the size alone, one layer is a `TEXTURE_2D`
/// and a square texture of 6k layers a cube map, and either fails a `D2Array` binding. Such a count
/// gets one spare layer there (left empty); other backends take the count as it is.
pub(crate) fn gl_array_layers(device: &wgpu::Device, layers: u32, size: (u32, u32)) -> u32 {
    let odd_target = layers == 1 || (size.0 == size.1 && layers.is_multiple_of(6));
    if odd_target
        && device.adapter_info().backend == wgpu::Backend::Gl
        && layers < device.limits().max_texture_array_layers
    {
        layers + 1
    } else {
        layers
    }
}

/// The blobs as the layers of one 2D array texture of `size`. All blobs share format and colour
/// space; a blob smaller than `size` is repeated to fill its layer (`LayerPlan` checks that it
/// tiles). `mip_count` levels are uploaded, at most what the layers can fill.
pub(crate) fn upload_array(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layers: &[&TextureBlob],
    size: (u32, u32),
    mip_count: u32,
    label: &str,
) -> Result<Tex, String> {
    let first = layers.first().ok_or("no layers")?;
    let needed = required_features(first.format);
    if !device.features().contains(needed) {
        return Err(format!("{:?} textures need {needed:?}", first.format));
    }
    let limits = device.limits();
    if layers.len() as u32 > limits.max_texture_array_layers {
        return Err(format!(
            "{} layers exceed the device limit {}",
            layers.len(),
            limits.max_texture_array_layers
        ));
    }
    if size.0.max(size.1) > limits.max_texture_dimension_2d {
        return Err(format!(
            "texture {}x{} exceeds the device limit {}",
            size.0, size.1, limits.max_texture_dimension_2d
        ));
    }
    if first.format != TextureFormat::Rgba8
        && (!size.0.is_multiple_of(BLOCK) || !size.1.is_multiple_of(BLOCK))
    {
        return Err(format!(
            "{:?} layers of {}x{} are not a multiple of the 4x4 block",
            first.format, size.0, size.1
        ));
    }
    if layers
        .iter()
        .any(|b| (b.format, b.srgb) != (first.format, first.srgb))
    {
        return Err("layers differ in format".to_owned());
    }
    let full = size.0.max(size.1).ilog2() + 1;
    let mip_count = mip_count.clamp(1, full);
    let texture = device.tex(
        &wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: gl_array_layers(device, layers.len() as u32, size),
            },
            mip_level_count: mip_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu_format(first.format, first.srgb),
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
        Cluster::TerrainTex,
    );
    for (layer, blob) in layers.iter().enumerate() {
        let tiled = (blob.width, blob.height) != size;
        for level in 0..mip_count as usize {
            let l = mip_layout(first.format, size, level);
            let data = if tiled {
                std::borrow::Cow::Owned(tiled_level(blob, size, level))
            } else {
                let Some(m) = blob.mips.get(level) else {
                    return Err(format!("a layer has only {} mips", blob.mips.len()));
                };
                std::borrow::Cow::Borrowed(m.as_slice())
            };
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: level as u32,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: layer as u32,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(l.bytes_per_row),
                    rows_per_image: Some(l.rows),
                },
                wgpu::Extent3d {
                    width: l.width,
                    height: l.height,
                    depth_or_array_layers: 1,
                },
            );
        }
    }
    Ok(texture)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_mapping() {
        assert_eq!(
            wgpu_format(TextureFormat::Rgba8, true),
            wgpu::TextureFormat::Rgba8UnormSrgb
        );
        assert_eq!(
            wgpu_format(TextureFormat::Rgba8, false),
            wgpu::TextureFormat::Rgba8Unorm
        );
        assert_eq!(
            wgpu_format(TextureFormat::Bc7, true),
            wgpu::TextureFormat::Bc7RgbaUnormSrgb
        );
        assert_eq!(
            wgpu_format(TextureFormat::Bc7, false),
            wgpu::TextureFormat::Bc7RgbaUnorm
        );
        assert!(matches!(
            wgpu_format(TextureFormat::Astc4x4, true),
            wgpu::TextureFormat::Astc {
                block: wgpu::AstcBlock::B4x4,
                channel: wgpu::AstcChannel::UnormSrgb
            }
        ));
        assert!(matches!(
            wgpu_format(TextureFormat::Astc4x4, false),
            wgpu::TextureFormat::Astc {
                channel: wgpu::AstcChannel::Unorm,
                ..
            }
        ));
    }

    #[test]
    fn mip_layouts_match_blob_sizes() {
        for format in [
            TextureFormat::Rgba8,
            TextureFormat::Bc7,
            TextureFormat::Astc4x4,
        ] {
            for level in 0..5 {
                let l = mip_layout(format, (16, 8), level);
                let (w, h) = ((16u32 >> level).max(1), (8u32 >> level).max(1));
                assert_eq!(
                    (l.bytes_per_row * l.rows) as usize,
                    format.mip_size(w, h),
                    "{format:?} level {level}"
                );
            }
        }
        // 1×1 compressed mip still copies a whole block.
        let l = mip_layout(TextureFormat::Bc7, (4, 4), 2);
        assert_eq!((l.width, l.height, l.bytes_per_row, l.rows), (4, 4, 16, 1));
        let l = mip_layout(TextureFormat::Rgba8, (4, 2), 2);
        assert_eq!((l.width, l.height, l.bytes_per_row), (1, 1, 4));
    }

    #[test]
    fn profile_follows_features() {
        let p = |f| profile_for_features(f).texture_formats;
        assert_eq!(
            p(wgpu::Features::empty()),
            PlatformProfile::no_compression().texture_formats
        );
        assert_eq!(
            p(wgpu::Features::TEXTURE_COMPRESSION_BC),
            PlatformProfile::bc_device().texture_formats
        );
        // A phone: ASTC first, BC only as the CPU-decoded last resort.
        assert_eq!(
            p(wgpu::Features::TEXTURE_COMPRESSION_ASTC),
            PlatformProfile::astc_device().texture_formats
        );
        assert_eq!(
            p(wgpu::Features::TEXTURE_COMPRESSION_BC | wgpu::Features::TEXTURE_COMPRESSION_ASTC),
            [
                VariantFormat::Bc,
                VariantFormat::Astc,
                VariantFormat::Uncompressed
            ]
        );
    }

    #[test]
    fn features_per_format() {
        assert!(required_features(TextureFormat::Rgba8).is_empty());
        assert_eq!(
            required_features(TextureFormat::Bc7),
            wgpu::Features::TEXTURE_COMPRESSION_BC
        );
    }

    fn blob(
        format: TextureFormat,
        w: u32,
        h: u32,
        fill: impl Fn(usize, usize) -> u8,
    ) -> TextureBlob {
        let levels = w.max(h).ilog2() as usize + 1;
        TextureBlob {
            format,
            dim: asset_format::TexDim::D2,
            srgb: true,
            width: w,
            height: h,
            depth: 1,
            mips: (0..levels)
                .map(|l| {
                    let (cw, ch) = level_cells(format, (w, h), l);
                    let unit = cell_bytes(format);
                    (0..cw as usize * ch as usize)
                        .flat_map(|c| vec![fill(l, c); unit])
                        .collect()
                })
                .collect(),
        }
    }

    #[test]
    fn tiling_repeats_rgba_texels() {
        // 2x2 source, texel value = index; tiled into 4x4.
        let b = TextureBlob {
            format: TextureFormat::Rgba8,
            dim: asset_format::TexDim::D2,
            srgb: true,
            width: 2,
            height: 2,
            depth: 1,
            mips: vec![(0u8..4).flat_map(|v| [v; 4]).collect(), vec![9; 4]],
        };
        let l0 = tiled_level(&b, (4, 4), 0);
        let texel = |x: usize, y: usize| l0[(y * 4 + x) * 4];
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(texel(x, y), ((y % 2) * 2 + x % 2) as u8, "{x},{y}");
            }
        }
        assert_eq!(l0.len(), TextureFormat::Rgba8.mip_size(4, 4));
        // Level 1 of the layer is 2x2: the source's own level 1 is 1x1, repeated.
        assert_eq!(tiled_level(&b, (4, 4), 1), vec![9; 16]);
        assert_eq!(tiled_level(&b, (4, 4), 2), vec![9; 4]);
    }

    #[test]
    fn tiling_repeats_compressed_blocks_and_sizes_match_the_layout() {
        // 8x8 BC7 source (2x2 blocks, block value = index), tiled into 32x32.
        let b = blob(TextureFormat::Bc7, 8, 8, |l, c| (l * 10 + c) as u8);
        for level in 0..6 {
            let data = tiled_level(&b, (32, 32), level);
            let l = mip_layout(TextureFormat::Bc7, (32, 32), level);
            assert_eq!(
                data.len(),
                (l.bytes_per_row * l.rows) as usize,
                "level {level}"
            );
        }
        let l0 = tiled_level(&b, (32, 32), 0);
        // 8x8 blocks of 16 bytes; block (x, y) holds the value of source block (x%2, y%2).
        for y in 0..8usize {
            for x in 0..8usize {
                assert_eq!(l0[(y * 8 + x) * 16], ((y % 2) * 2 + x % 2) as u8);
            }
        }
        // Level 1 of the layer is 16x16 (4x4 blocks); the source's level 1 is 4x4 (one block,
        // value 10), repeated.
        let l1 = tiled_level(&b, (32, 32), 1);
        assert!(l1.chunks(16).all(|c| c[0] == 10));
        // Below the source's last 4x4 level that level's block is repeated.
        let l5 = tiled_level(&b, (32, 32), 5);
        assert_eq!(l5.len(), 16);
        assert_eq!(l5[0], 10);
    }
}

#[cfg(test)]
mod gpu_tests {
    use super::*;
    use crate::gpu_test::{Gpu, gpu, read_texture};

    /// Largest difference accepted on interpolated palette entries (see the test).
    const INTERPOLATION_TOLERANCE: u8 = 8;

    const ALL_FORMATS: [TextureFormat; 12] = [
        TextureFormat::Rgba8,
        TextureFormat::Bc7,
        TextureFormat::Astc4x4,
        TextureFormat::Bc1,
        TextureFormat::Bc2,
        TextureFormat::Bc3,
        TextureFormat::Bc4,
        TextureFormat::Bc5,
        TextureFormat::R8,
        TextureFormat::Rg8,
        TextureFormat::R16F,
        TextureFormat::Rgba16F,
    ];

    /// Deterministic bytes (xorshift), so block data covers every mode.
    fn noise(seed: u32, len: usize) -> Vec<u8> {
        let mut s = seed | 1;
        (0..len)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                (s >> 8) as u8
            })
            .collect()
    }

    fn blob(
        format: TextureFormat,
        dim: TexDim,
        side: u32,
        depth: u32,
        levels: usize,
    ) -> TextureBlob {
        let mips = (0..levels)
            .map(|l| {
                let s = (side >> l).max(1);
                let slices = match dim {
                    TexDim::D2 => 1,
                    TexDim::Cube => 6,
                    TexDim::D3 => (depth >> l).max(1) as usize,
                };
                noise(l as u32 + 7, format.mip_size(s, s) * slices)
            })
            .collect();
        TextureBlob {
            format,
            dim,
            srgb: false,
            width: side,
            height: side,
            depth,
            mips,
        }
    }

    /// Reads texel colours of level 0 through the sampling hardware: `textureLoad` into an
    /// `Rgba8Unorm` target (missing channels read as 0, alpha 1).
    fn sample_level0(gpu: &Gpu, texture: &wgpu::Texture, size: (u32, u32)) -> Vec<u8> {
        const SHADER: &str = "
@group(0) @binding(0) var src: texture_2d<f32>;
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}
@fragment fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(src, vec2<i32>(pos.xy), 0);
}";
        let d = &gpu.device;
        let layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let module = d.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pl = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::TextureFormat::Rgba8Unorm.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let target = d.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let src = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let dst = target.create_view(&wgpu::TextureViewDescriptor::default());
        let bind = d.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&src),
            }],
        });
        let mut enc = d.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &dst,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            use crate::perf::PassCount as _;
            pass.set_pipeline_c(&pipeline);
            pass.set_bind_group_c(0, &bind, &[]);
            pass.draw_c(0..3, 0..1);
        }
        gpu.queue.submit([enc.finish()]);
        read_texture(gpu, &target, 0, 0, size, 4)
    }

    /// Every format and kind uploads, with and without a BC device.
    #[test]
    fn every_format_and_dim_uploads() {
        for (label, want) in [
            ("no BC", wgpu::Features::empty()),
            ("BC", wgpu::Features::TEXTURE_COMPRESSION_BC),
        ] {
            let Some(g) = gpu(
                "every_format_and_dim_uploads",
                wgpu::Backends::PRIMARY,
                want,
            ) else {
                continue;
            };
            let has_bc = want.contains(wgpu::Features::TEXTURE_COMPRESSION_BC);
            for format in ALL_FORMATS {
                for (dim, depth) in [(TexDim::D2, 1), (TexDim::Cube, 1), (TexDim::D3, 8)] {
                    let b = blob(format, dim, 8, depth, 4);
                    let r = upload_blob(&g.device, &g.queue, &b, "t", UploadOptions::WORLD);
                    let what = format!("{label} {format:?} {dim:?}");
                    match format {
                        // Not decodable here: need their own feature.
                        TextureFormat::Bc7 | TextureFormat::Astc4x4 => {
                            // Bc7 needs the BC feature, Astc its own (never requested here).
                            let sliced = g
                                .device
                                .features()
                                .contains(wgpu::Features::TEXTURE_COMPRESSION_BC_SLICED_3D);
                            let needs = format == TextureFormat::Astc4x4
                                || !has_bc
                                || (dim == TexDim::D3 && !sliced);
                            assert_eq!(r.is_err(), needs, "{what}");
                        }
                        _ => {
                            let u = r.unwrap_or_else(|e| panic!("{what}: {e}"));
                            let bc15 = bc_decode::decoded_format(format).is_some();
                            let sliced = g
                                .device
                                .features()
                                .contains(wgpu::Features::TEXTURE_COMPRESSION_BC_SLICED_3D);
                            let expect_cpu = bc15 && (!has_bc || (dim == TexDim::D3 && !sliced));
                            assert_eq!(u.cpu_decoded, expect_cpu, "{what}");
                            let size = u.texture.size();
                            assert_eq!(size.width, 8, "{what}");
                            assert_eq!(
                                size.depth_or_array_layers,
                                match dim {
                                    TexDim::D2 => 1,
                                    TexDim::Cube => 6,
                                    TexDim::D3 => 8,
                                },
                                "{what}"
                            );
                            assert_eq!(u.texture.mip_level_count(), 4, "{what}");
                            // No sRGB on the world path, whatever the blob says.
                            assert!(!u.format.is_srgb(), "{what}");
                        }
                    }
                }
            }
        }
    }

    /// A DXT original that is not whole blocks (the UI has 45x45 ones) cannot be a BC texture in
    /// WebGPU: it goes up decoded, also on a BC device.
    #[test]
    fn a_bc_texture_that_is_not_whole_blocks_goes_up_decoded() {
        let Some(g) = gpu(
            "a_bc_texture_that_is_not_whole_blocks_goes_up_decoded",
            wgpu::Backends::PRIMARY,
            wgpu::Features::TEXTURE_COMPRESSION_BC,
        ) else {
            return;
        };
        let b = TextureBlob::new_2d(
            TextureFormat::Bc1,
            false,
            6,
            6,
            vec![noise(3, TextureFormat::Bc1.mip_size(6, 6))],
        );
        let u = upload_blob(&g.device, &g.queue, &b, "t", UploadOptions::WORLD).unwrap();
        assert!(u.cpu_decoded);
        assert_eq!(u.format, wgpu::TextureFormat::Rgba8Unorm);
        let size = u.texture.size();
        assert_eq!((size.width, size.height), (6, 6));
    }

    #[test]
    fn world_upload_ignores_the_srgb_flag() {
        let Some(g) = gpu(
            "world_upload_ignores_the_srgb_flag",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        let mut b = blob(TextureFormat::Rgba8, TexDim::D2, 4, 1, 1);
        b.srgb = true;
        let linear = upload_blob(&g.device, &g.queue, &b, "t", UploadOptions::WORLD).unwrap();
        assert_eq!(linear.format, wgpu::TextureFormat::Rgba8Unorm);
        let b1 = upload_blob(&g.device, &g.queue, &b, "t", UploadOptions::default()).unwrap();
        assert_eq!(b1.format, wgpu::TextureFormat::Rgba8UnormSrgb);
    }

    #[test]
    fn cpu_decoded_formats_and_sizes() {
        let b = blob(TextureFormat::Bc5, TexDim::Cube, 8, 1, 4);
        let d = decode_blob(&b).unwrap();
        assert_eq!(d.format, TextureFormat::Rg8);
        assert_eq!(d.mips.len(), 4);
        // Six faces per level, tightly packed, 1x1 at the last level.
        assert_eq!(d.mips[0].len(), 6 * 8 * 8 * 2);
        assert_eq!(d.mips[3].len(), 6 * 2);
        let v = blob(TextureFormat::Bc1, TexDim::D3, 8, 8, 4);
        let d = decode_blob(&v).unwrap();
        assert_eq!(d.format, TextureFormat::Rgba8);
        assert_eq!(d.mips[1].len(), 4 * 4 * 4 * 4);
        assert!(decode_blob(&blob(TextureFormat::Bc7, TexDim::D2, 8, 1, 1)).is_none());
    }

    /// Keeps only the endpoint indices (0 and 1) of every BC block, so the palette
    /// interpolation, which hardware does its own way, plays no part.
    fn endpoints_only(format: TextureFormat, data: &mut [u8]) {
        let block = format.block_bytes() as usize;
        for b in data.chunks_exact_mut(block) {
            // Colour indices: 4 bytes after the two endpoints. Two bits per texel, keep bit 0.
            // BC4 / alpha indices: 6 bytes after two endpoints. Three bits per texel, keep bit 0.
            let colour = |c: &mut [u8]| c[4..8].iter_mut().for_each(|v| *v &= 0x55);
            let alpha = |a: &mut [u8]| {
                let mut bits = 0u64;
                for (i, v) in a[2..8].iter().enumerate() {
                    bits |= u64::from(*v) << (8 * i);
                }
                bits &= 0x2492_4924_9249;
                for (i, v) in a[2..8].iter_mut().enumerate() {
                    *v = (bits >> (8 * i)) as u8;
                }
            };
            match format {
                TextureFormat::Bc1 => colour(b),
                TextureFormat::Bc2 => colour(&mut b[8..]),
                TextureFormat::Bc3 => {
                    alpha(&mut b[..8]);
                    colour(&mut b[8..]);
                }
                TextureFormat::Bc4 => alpha(b),
                TextureFormat::Bc5 => {
                    alpha(&mut b[..8]);
                    alpha(&mut b[8..]);
                }
                _ => unreachable!(),
            }
        }
    }

    /// BC1-BC5 sampled from a real BC texture against the CPU decode. Endpoint texels agree to
    /// 1/255. Interpolated ones are looser: the capture GPU blends palette entries with weights
    /// in 1/16 steps (11/16 where the formula says 2/3), up to 5/255 off on a wide palette.
    #[test]
    fn bc_decode_matches_the_gpu() {
        let Some(g) = gpu(
            "bc_decode_matches_the_gpu",
            wgpu::Backends::PRIMARY,
            wgpu::Features::TEXTURE_COMPRESSION_BC,
        ) else {
            return;
        };
        for (exact, limit) in [(true, 1u8), (false, INTERPOLATION_TOLERANCE)] {
            for format in [
                TextureFormat::Bc1,
                TextureFormat::Bc2,
                TextureFormat::Bc3,
                TextureFormat::Bc4,
                TextureFormat::Bc5,
            ] {
                // 32x32: 64 blocks of random data, enough to hit both BC1 and both BC4 modes.
                let mut b = blob(format, TexDim::D2, 32, 1, 1);
                if exact {
                    endpoints_only(format, &mut b.mips[0]);
                }
                let native =
                    upload_blob(&g.device, &g.queue, &b, "t", UploadOptions::WORLD).unwrap();
                assert!(!native.cpu_decoded);
                let cpu = upload_blob(
                    &g.device,
                    &g.queue,
                    &b,
                    "t",
                    UploadOptions {
                        force_cpu_bc: true,
                        ..UploadOptions::WORLD
                    },
                )
                .unwrap();
                assert!(cpu.cpu_decoded);
                let from_gpu = sample_level0(&g, &native.texture, (32, 32));
                let from_cpu = sample_level0(&g, &cpu.texture, (32, 32));
                let channels = match format {
                    TextureFormat::Bc4 => 1,
                    TextureFormat::Bc5 => 2,
                    _ => 4,
                };
                let mut worst = [0u8; 4];
                for (i, (a, c)) in from_gpu.iter().zip(&from_cpu).enumerate() {
                    worst[i % 4] = worst[i % 4].max(a.abs_diff(*c));
                }
                eprintln!(
                    "{format:?} endpoints only: {exact}, worst difference per channel on {}: {worst:?}",
                    g.info.name
                );
                for (ch, w) in worst.iter().enumerate() {
                    // Channels the format lacks read as 0 (alpha 1) on both sides.
                    if ch >= channels && ch != 3 {
                        continue;
                    }
                    assert!(
                        *w <= limit,
                        "{format:?} (endpoints only: {exact}) channel {ch}: CPU decode differs \
                         from the GPU by {w}/255"
                    );
                }
            }
        }
    }

    #[test]
    fn no_bc_device_reads_back_the_formula_values() {
        let Some(g) = gpu(
            "no_bc_device_reads_back_the_formula_values",
            wgpu::Backends::PRIMARY,
            wgpu::Features::empty(),
        ) else {
            return;
        };
        // One block: c0 = red, c1 = blue, texel 0 index 0, texel 1 index 1, texel 2 index 2.
        let block: Vec<u8> = [
            &0xf800u16.to_le_bytes()[..],
            &0x001fu16.to_le_bytes()[..],
            &[0b10_01_00, 0, 0, 0][..],
        ]
        .concat();
        let b = TextureBlob::new_2d(TextureFormat::Bc1, false, 4, 4, vec![block]);
        let u = upload_blob(&g.device, &g.queue, &b, "t", UploadOptions::WORLD).unwrap();
        assert!(u.cpu_decoded);
        let px = sample_level0(&g, &u.texture, (4, 4));
        assert_eq!(px[..4], [255, 0, 0, 255]);
        assert_eq!(px[4..8], [0, 0, 255, 255]);
        assert_eq!(px[8..12], [170, 0, 85, 255]);
    }
}
