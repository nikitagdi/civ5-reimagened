//! Images: DDS in (`world.dds`, `final.dds`, targets of a capture), raw RGBA in, PNG out
//! Only 8-bit colour formats are read; anything else is an error that says so.

use std::path::Path;

use anyhow::{Context, Result, bail};
use ddsfile::{Dds, DxgiFormat};

use crate::metrics::Image;

/// 8-bit RGBA image, row major, 4 bytes per pixel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba8 {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

impl Rgba8 {
    /// The image the metrics take (alpha dropped).
    pub fn to_image(&self) -> Image {
        Image::from_rgba(self.width, self.height, &self.rgba)
    }
}

/// IEEE half to f32 (no dependency).
fn f16_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = i32::from((h >> 10) & 0x1F);
    let man = f32::from(h & 0x3FF);
    match exp {
        0 => sign * man * 2f32.powi(-24),
        31 if man == 0.0 => sign * f32::INFINITY,
        31 => f32::NAN,
        e => sign * (1.0 + man / 1024.0) * 2f32.powi(e - 15),
    }
}

/// Float to 8-bit UNORM as the GPU writes it: clamp to 0..1 (NaN is 0), scale, round to nearest.
fn unorm8(v: f32) -> u8 {
    if v.is_nan() {
        return 0;
    }
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// Mip 0 (first layer) of a DDS in `R8G8B8A8` or `B8G8R8A8` / `B8G8R8X8` (UNORM, with or without the
/// sRGB view), or `R16G16B16A16_FLOAT`. 8-bit bytes are kept as they are: the sRGB flag only says how the
/// GPU reads them. Half floats become 8-bit UNORM the way the original's `cvResolvePass` (a plain copy into
/// the `R8G8B8A8_UNORM` back buffer) writes them: clamp, scale, round; this is `world.dds`, the frame before
/// the UI boundary.
pub fn read_dds_rgba8(bytes: &[u8]) -> Result<Rgba8> {
    let dds = Dds::read(bytes).context("not a DDS file")?;
    let (w, h) = (dds.get_width() as usize, dds.get_height() as usize);
    if dds.get_dxgi_format() == Some(DxgiFormat::R16G16B16A16_Float) {
        let layer = dds.get_data(0).context("DDS has no pixel data")?;
        let need = w * h * 8;
        if layer.len() < need {
            bail!(
                "DDS pixel data is {} bytes, {w}x{h} half RGBA needs {need}",
                layer.len()
            );
        }
        let rgba = layer[..need]
            .chunks_exact(2)
            .map(|c| unorm8(f16_to_f32(u16::from_le_bytes([c[0], c[1]]))))
            .collect();
        return Ok(Rgba8 {
            width: w,
            height: h,
            rgba,
        });
    }
    let swap_rb = match dds.get_dxgi_format() {
        Some(DxgiFormat::R8G8B8A8_UNorm | DxgiFormat::R8G8B8A8_UNorm_sRGB) => false,
        Some(
            DxgiFormat::B8G8R8A8_UNorm
            | DxgiFormat::B8G8R8A8_UNorm_sRGB
            | DxgiFormat::B8G8R8X8_UNorm
            | DxgiFormat::B8G8R8X8_UNorm_sRGB,
        ) => true,
        other => bail!("DDS format {other:?} is not an 8-bit colour format (R8G8B8A8, B8G8R8A8)"),
    };
    let layer = dds.get_data(0).context("DDS has no pixel data")?;
    let need = w * h * 4;
    if layer.len() < need {
        bail!(
            "DDS pixel data is {} bytes, {w}x{h} needs {need}",
            layer.len()
        );
    }
    let mut rgba = layer[..need].to_vec();
    if swap_rb {
        for p in rgba.chunks_exact_mut(4) {
            p.swap(0, 2);
        }
    }
    Ok(Rgba8 {
        width: w,
        height: h,
        rgba,
    })
}

pub fn read_dds_file(path: &Path) -> Result<Rgba8> {
    let bytes =
        std::fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    read_dds_rgba8(&bytes).with_context(|| path.display().to_string())
}

/// Raw `RGBA8` frame (`ours-native.rgba`): size comes from the caller.
pub fn read_raw_rgba(path: &Path, width: usize, height: usize) -> Result<Rgba8> {
    let rgba = std::fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    if rgba.len() != width * height * 4 {
        bail!(
            "{}: {} bytes, {width}x{height} RGBA needs {}",
            path.display(),
            rgba.len(),
            width * height * 4
        );
    }
    Ok(Rgba8 {
        width,
        height,
        rgba,
    })
}

fn crc32(data: &[u8]) -> u32 {
    // CRC-32/ISO-HDLC (PNG, zlib): polynomial 0xEDB88320, reflected.
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    const MOD: u32 = 65521;
    let (mut a, mut b) = (1u32, 0u32);
    for &x in data {
        a = (a + u32::from(x)) % MOD;
        b = (b + a) % MOD;
    }
    (b << 16) | a
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], body: &[u8]) {
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    let mut c = kind.to_vec();
    c.extend_from_slice(body);
    out.extend_from_slice(&c);
    out.extend_from_slice(&crc32(&c).to_be_bytes());
}

/// PNG (8-bit RGBA, filter 0, zlib stored blocks: no compression, no dependency).
pub fn png_bytes(img: &Rgba8) -> Vec<u8> {
    let stride = img.width * 4;
    let mut raw = Vec::with_capacity((stride + 1) * img.height);
    for row in img.rgba.chunks_exact(stride.max(1)) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut z = vec![0x78, 0x01];
    let mut blocks = raw.chunks(65535).peekable();
    if blocks.peek().is_none() {
        z.extend_from_slice(&[1, 0, 0, 0xFF, 0xFF]);
    }
    while let Some(b) = blocks.next() {
        z.push(u8::from(blocks.peek().is_none()));
        z.extend_from_slice(&(b.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(b.len() as u16)).to_le_bytes());
        z.extend_from_slice(b);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(img.width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(img.height as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

pub fn write_png(path: &Path, img: &Rgba8) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, png_bytes(img))
        .with_context(|| format!("failed to write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_and_adler_known_values() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }

    #[test]
    fn half_floats_become_unorm8_like_the_resolve() {
        assert_eq!(f16_to_f32(0x3C00), 1.0);
        assert_eq!(f16_to_f32(0x3800), 0.5);
        assert_eq!(f16_to_f32(0xC000), -2.0);
        assert_eq!(f16_to_f32(0x0001), 2f32.powi(-24));
        assert_eq!(unorm8(0.5), 128);
        assert_eq!(unorm8(-1.0), 0);
        assert_eq!(unorm8(3.0), 255);
        assert_eq!(unorm8(f32::NAN), 0);
    }

    #[test]
    fn png_has_signature_and_chunks() {
        let img = Rgba8 {
            width: 2,
            height: 1,
            rgba: vec![255, 0, 0, 255, 0, 255, 0, 255],
        };
        let p = png_bytes(&img);
        assert_eq!(&p[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(&p[12..16], b"IHDR");
        assert_eq!(&p[p.len() - 8..p.len() - 4], b"IEND");
        // IEND crc is the well-known constant.
        assert_eq!(&p[p.len() - 4..], &[0xAE, 0x42, 0x60, 0x82]);
    }
}
