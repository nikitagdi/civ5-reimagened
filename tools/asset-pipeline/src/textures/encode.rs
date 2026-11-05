//! RGBA8 mip chain to a texture blob, via ctt for the block-compressed formats.

use asset_format::{TextureBlob, TextureFormat};
use ctt::encoders::Encoder;
use ctt::encoders::bc7enc::Bc7encSettings;
use ctt::{
    AlphaMode, Container, ConvertSettings, Format, FormatDesc, ImageRef, PipelineOutput, Quality,
    SurfaceRef, TargetFormat, TextureKind,
};

use super::{ColorSpace, MAX_SIDE, Rgba8Image, TextureError};

/// Encodes a full or partial chain; every level is encoded, so callers complete the chain
/// first if they want one.
pub fn encode_chain(
    mips: &[Rgba8Image],
    color_space: ColorSpace,
    format: TextureFormat,
) -> Result<TextureBlob, TextureError> {
    encode_chain_with(mips, color_space, format, AlphaMode::Straight)
}

/// [`encode_chain`] for the repack pass: the colour of a texel with alpha 0 is kept. ctt
/// premultiplies a straight-alpha image on load and divides again on store, which turns the RGB
/// of every alpha-0 texel (and, a little, of the translucent ones) into 0; `Opaque` skips both
/// steps and hands the texels to the encoder as they are. Alpha is still encoded: bc7enc-rdo
/// does not read the mode.
pub fn encode_chain_keep_rgb(
    mips: &[Rgba8Image],
    color_space: ColorSpace,
    format: TextureFormat,
) -> Result<TextureBlob, TextureError> {
    encode_chain_with(mips, color_space, format, AlphaMode::Opaque)
}

fn encode_chain_with(
    mips: &[Rgba8Image],
    color_space: ColorSpace,
    format: TextureFormat,
    alpha: AlphaMode,
) -> Result<TextureBlob, TextureError> {
    let base = mips
        .first()
        .ok_or_else(|| TextureError::Encode("no mip levels".into()))?;
    let (width, height) = (base.width, base.height);
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return Err(TextureError::BadSize { width, height });
    }
    for (level, m) in mips.iter().enumerate() {
        let want = ((width >> level).max(1), (height >> level).max(1));
        if (m.width, m.height) != want || m.pixels.len() != m.width as usize * m.height as usize * 4
        {
            return Err(TextureError::Encode(format!(
                "mip {level} is {}x{}, expected {}x{}",
                m.width, m.height, want.0, want.1
            )));
        }
    }
    let data = match format {
        TextureFormat::Rgba8 => mips.iter().map(|m| m.pixels.clone()).collect(),
        TextureFormat::Bc1 | TextureFormat::Bc7 | TextureFormat::Astc4x4 => {
            compress(mips, color_space, format, alpha)?
        }
        // The world formats keep the bytes of the source file (AB-D2); they are not encoded here.
        other => {
            return Err(TextureError::Encode(format!(
                "{other:?} is not produced by the RGBA8 encoder"
            )));
        }
    };
    Ok(TextureBlob::new_2d(
        format,
        color_space == ColorSpace::Srgb,
        width,
        height,
        data,
    ))
}

fn compress(
    mips: &[Rgba8Image],
    color_space: ColorSpace,
    format: TextureFormat,
    alpha: AlphaMode,
) -> Result<Vec<Vec<u8>>, TextureError> {
    let srgb = color_space == ColorSpace::Srgb;
    // The container is Raw, so these formats only steer the encoders and the sRGB handling.
    let (input, ctt_color_space) = if srgb {
        (Format::R8G8B8A8_SRGB, ctt::ColorSpace::Srgb)
    } else {
        (Format::R8G8B8A8_UNORM, ctt::ColorSpace::Linear)
    };
    let (target, encoder, quality) = match (format, srgb) {
        (TextureFormat::Bc7, true) => (Format::BC7_SRGB_BLOCK, bc7_encoder(), Quality::Slow),
        (TextureFormat::Bc7, false) => (Format::BC7_UNORM_BLOCK, bc7_encoder(), Quality::Slow),
        // Opaque input only (the repack pass checks); the encoder is the auto pick.
        (TextureFormat::Bc1, true) => (Format::BC1_RGBA_SRGB_BLOCK, Encoder::Auto, Quality::Slow),
        (TextureFormat::Bc1, false) => (Format::BC1_RGBA_UNORM_BLOCK, Encoder::Auto, Quality::Slow),
        (TextureFormat::Astc4x4, true) => {
            (Format::ASTC_4x4_SRGB_BLOCK, Encoder::Auto, Quality::Basic)
        }
        (TextureFormat::Astc4x4, false) => {
            (Format::ASTC_4x4_UNORM_BLOCK, Encoder::Auto, Quality::Basic)
        }
        (other, _) => {
            return Err(TextureError::Encode(format!(
                "{other:?} is not an encoder target"
            )));
        }
    };

    let surfaces = mips
        .iter()
        .map(|m| SurfaceRef {
            data: &m.pixels,
            width: m.width,
            height: m.height,
            depth: 1,
            stride: m.width * 4,
            slice_stride: 0,
        })
        .collect();
    let image = ImageRef {
        surfaces: vec![surfaces],
        kind: TextureKind::Texture2D,
        desc: FormatDesc {
            format: input,
            color_space: ctt_color_space,
            alpha,
        },
    };
    let settings = ConvertSettings {
        format: Some(TargetFormat::Compressed {
            format: target,
            encoder,
        }),
        container: Container::Raw,
        quality,
        ..Default::default()
    };
    let out = if format == TextureFormat::Astc4x4 {
        // ctt's astcenc sizes its context for every worker of the current rayon pool and runs one
        // image on all of them; called from the sets' and the variants pass's own `par_iter`, that
        // is a context of every worker per worker (tens of GB of scratch). A pool of one makes it
        // a one-thread context; the callers already run one texture per worker.
        SERIAL.with(|pool| pool.install(|| ctt::convert(image, settings)))
    } else {
        ctt::convert(image, settings)
    }
    .map_err(|e| TextureError::Encode(e.to_string()))?;
    let PipelineOutput::Raw(encoded) = out else {
        return Err(TextureError::Encode("encoder returned a file".into()));
    };
    let [layer] = <[_; 1]>::try_from(encoded.surfaces)
        .map_err(|_| TextureError::Encode("encoder returned several layers".into()))?;
    if layer.len() != mips.len() {
        return Err(TextureError::Encode("encoder dropped mip levels".into()));
    }
    // Tightly packed blocks are what wgpu's write_texture expects; verify rather than trust.
    layer
        .into_iter()
        .zip(mips)
        .map(|(s, m)| {
            let want = format.mip_size(m.width, m.height);
            if s.data.len() == want {
                Ok(s.data)
            } else {
                Err(TextureError::Encode(format!(
                    "{}x{} encoded to {} bytes, expected {want}",
                    m.width,
                    m.height,
                    s.data.len()
                )))
            }
        })
        .collect()
}

thread_local! {
    /// A one-thread rayon pool per calling thread, for the ASTC encoder (see `compress`). Its
    /// stack is the one the encoders need (`RUST_MIN_STACK` of .cargo/config.toml).
    static SERIAL: rayon::ThreadPool = rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .stack_size(16 << 20)
        .build()
        .expect("a one-thread rayon pool");
}

/// bc7enc-rdo with plain RGB error: albedo, masks and data share one setting.
fn bc7_encoder() -> Encoder {
    Encoder::Bc7enc(Bc7encSettings {
        perceptual: false,
        ..Default::default()
    })
}
