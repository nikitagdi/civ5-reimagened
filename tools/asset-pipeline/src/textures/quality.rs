//! Blob decoding for verification: PSNR of encoded mips against the RGBA8 source.

use std::ffi::c_void;

use asset_format::{TextureBlob, TextureFormat};
use ctt::encoders::astcenc::astc;

use super::dds::decode_blocks;
use super::{Rgba8Image, TextureError};

/// PSNR in dB over all four channels; `INFINITY` for identical images.
///
/// # Panics
/// If the slices differ in length.
pub fn psnr(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len(), "PSNR needs equally sized images");
    if a.is_empty() {
        return f64::INFINITY;
    }
    let sq: u64 = a
        .iter()
        .zip(b)
        .map(|(&x, &y)| {
            let d = i64::from(x) - i64::from(y);
            (d * d) as u64
        })
        .sum();
    if sq == 0 {
        return f64::INFINITY;
    }
    let mse = sq as f64 / a.len() as f64;
    10.0 * (255.0f64 * 255.0 / mse).log10()
}

/// Decodes one mip of a 2D blob back to RGBA8 (sRGB-encoded values for sRGB blobs).
pub fn decode_mip(blob: &TextureBlob, level: usize) -> Result<Rgba8Image, TextureError> {
    let data = blob
        .mips
        .get(level)
        .ok_or_else(|| TextureError::Encode(format!("no mip {level}")))?;
    let (w, h) = ((blob.width >> level).max(1), (blob.height >> level).max(1));
    if data.len() != blob.format.mip_size(w, h) {
        return Err(TextureError::Encode(format!(
            "mip {level} has a wrong size"
        )));
    }
    decode_slice(blob.format, blob.srgb, w, h, data)
}

/// Decodes one 2D slice (`w` x `h`, a cube face or a whole 2D level) of `format` to RGBA8. `R8`
/// is grey with alpha 1 (the L8 of the original).
pub fn decode_slice(
    format: TextureFormat,
    srgb: bool,
    w: u32,
    h: u32,
    data: &[u8],
) -> Result<Rgba8Image, TextureError> {
    if data.len() != format.mip_size(w, h) {
        return Err(TextureError::Encode(format!(
            "a {w}x{h} {format:?} slice of {} bytes",
            data.len()
        )));
    }
    let mut out = vec![0u8; w as usize * h as usize * 4];
    match format {
        TextureFormat::Rgba8 => out.copy_from_slice(data),
        TextureFormat::Bc7 => {
            decode_blocks(data, 16, w as usize, h as usize, &mut out, bcdec_rs::bc7)
        }
        TextureFormat::Astc4x4 => decode_astc(data, srgb, w, h, &mut out)?,
        TextureFormat::R8 => {
            for (o, &l) in out.chunks_exact_mut(4).zip(data) {
                o.copy_from_slice(&[l, l, l, 255]);
            }
        }
        TextureFormat::Bc1 => {
            decode_blocks(data, 8, w as usize, h as usize, &mut out, bcdec_rs::bc1)
        }
        TextureFormat::Bc2 => {
            decode_blocks(data, 16, w as usize, h as usize, &mut out, bcdec_rs::bc2)
        }
        TextureFormat::Bc3 => {
            decode_blocks(data, 16, w as usize, h as usize, &mut out, bcdec_rs::bc3)
        }
        other => {
            return Err(TextureError::Encode(format!(
                "{other:?} cannot be decoded to RGBA8"
            )));
        }
    }
    Ok(Rgba8Image::new(w, h, out))
}

fn decode_astc(
    data: &[u8],
    srgb: bool,
    w: u32,
    h: u32,
    out: &mut [u8],
) -> Result<(), TextureError> {
    let err = |e: astc::Error| TextureError::Encode(format!("astcenc decode: {e}"));
    let profile = if srgb {
        astc::Profile::LdrSrgb
    } else {
        astc::Profile::Ldr
    };
    let config = astc::config_init(
        profile,
        4,
        4,
        1,
        astc::Preset::Medium,
        astc::Flags::DECOMPRESS_ONLY,
    )
    .map_err(err)?;
    let mut ctx = astc::Context::new(&config).map_err(err)?;
    // astcenc_image takes an array of per-slice plane pointers; a 2D image has one.
    let mut plane = out.as_mut_ptr().cast::<c_void>();
    let mut image = astc::bindings::astcenc_image {
        dim_x: w,
        dim_y: h,
        dim_z: 1,
        data_type: astc::bindings::astcenc_type_ASTCENC_TYPE_U8,
        data: &mut plane,
    };
    ctx.decompress(data, &mut image, astc::Swizzle::IDENTITY)
        .map_err(err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn psnr_values() {
        assert_eq!(psnr(&[1, 2, 3], &[1, 2, 3]), f64::INFINITY);
        // Every byte off by one: MSE 1 -> 10*log10(255^2) = 48.13 dB.
        let p = psnr(&[10; 8], &[11; 8]);
        assert!((p - 48.130).abs() < 0.01, "{p}");
        assert!(psnr(&[0; 4], &[255; 4]) < 1e-9);
    }
}
