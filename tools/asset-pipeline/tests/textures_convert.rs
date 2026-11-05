//! Texture conversion: synthetic DDS files, plus Warrior textures when `CIV5_ROOT` is set.

use asset_format::{TextureBlob, TextureFormat};
use asset_pipeline::game::GameSource;
use asset_pipeline::textures::{
    ColorSpace, TextureError, color_space_for, convert_dds, decode_dds, decode_mip, extend_mips,
    full_chain_len, psnr,
};
use ddsfile::{D3DFormat, Dds, NewD3dParams};

const ALL: [TextureFormat; 3] = [
    TextureFormat::Rgba8,
    TextureFormat::Bc7,
    TextureFormat::Astc4x4,
];

/// Minimum PSNR for the lossy formats.
const MIN_PSNR: f64 = 35.0;

fn dds_bytes(width: u32, height: u32, format: D3DFormat, mips: u32, data: &[u8]) -> Vec<u8> {
    let mut dds = Dds::new_d3d(NewD3dParams {
        height,
        width,
        depth: None,
        format,
        mipmap_levels: Some(mips),
        caps2: None,
    })
    .unwrap();
    assert_eq!(dds.data.len(), data.len(), "test data size");
    dds.data.copy_from_slice(data);
    let mut out = Vec::new();
    dds.write(&mut out).unwrap();
    out
}

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1);
        (self.0 >> 33) as u32
    }
}

/// Smooth gradients plus mild noise, as BGRA bytes (A8R8G8B8 layout).
fn gradient_noise_bgra(w: u32, h: u32, noise: u32) -> Vec<u8> {
    let mut rng = Lcg(7);
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let n = |rng: &mut Lcg| (rng.next() % (2 * noise + 1)) as i32 - noise as i32;
            let r = (x * 255 / (w - 1)) as i32 + n(&mut rng);
            let g = (y * 255 / (h - 1)) as i32 + n(&mut rng);
            let b = ((x + y) * 255 / (w + h - 2)) as i32 + n(&mut rng);
            let a = 255 - (x * 90 / w) as i32;
            out.extend([b, g, r, a].map(|v| v.clamp(0, 255) as u8));
        }
    }
    out
}

fn check_blob(blob: &TextureBlob, format: TextureFormat, w: u32, h: u32, mips: usize) {
    assert_eq!(blob.format, format);
    assert_eq!((blob.width, blob.height), (w, h));
    assert_eq!(blob.mips.len(), mips);
    // `decode` validates every mip size against `mip_size`, i.e. tight wgpu layout.
    assert_eq!(&TextureBlob::decode(&blob.encode()).unwrap(), blob);
    for (level, data) in blob.mips.iter().enumerate() {
        let (lw, lh) = ((w >> level).max(1), (h >> level).max(1));
        assert_eq!(data.len(), format.mip_size(lw, lh), "mip {level}");
    }
}

#[test]
fn dxt5_to_rgba8_is_exact_and_keeps_source_mips() {
    // 8x8 DXT5, 4 mips (8, 4, 2, 1 px): one solid block per 4x4 tile, picked so every
    // channel is 0 or 255 and alpha is the block's alpha0 (index 0 selects alpha0).
    let colors: [(u16, [u8; 3]); 4] = [
        (0xF800, [255, 0, 0]),
        (0x07E0, [0, 255, 0]),
        (0x001F, [0, 0, 255]),
        (0xFFFF, [255, 255, 255]),
    ];
    let alphas = [200u8, 100, 50, 10];
    let block = |i: usize| {
        let mut b = [0u8; 16];
        b[0] = alphas[i];
        b[1] = 0;
        b[8..10].copy_from_slice(&colors[i].0.to_le_bytes());
        b
    };
    let mut data = Vec::new();
    (0..4).for_each(|i| data.extend(block(i)));
    (0..3).for_each(|_| data.extend(block(3))); // mips 4x4, 2x2, 1x1: one block each
    let bytes = dds_bytes(8, 8, D3DFormat::DXT5, 4, &data);

    let blobs = convert_dds(&bytes, ColorSpace::Srgb, &[TextureFormat::Rgba8]).unwrap();
    let blob = &blobs[0];
    check_blob(blob, TextureFormat::Rgba8, 8, 8, 4);
    assert!(blob.srgb);
    // Mip 0: tiles row-major; pixel (x, y) is in block (x/4, y/4).
    for y in 0..8usize {
        for x in 0..8usize {
            let i = (y / 4) * 2 + x / 4;
            let rgb = colors[i].1;
            let want = [rgb[0], rgb[1], rgb[2], alphas[i]];
            let at = (y * 8 + x) * 4;
            assert_eq!(blob.mips[0][at..at + 4], want, "pixel {x},{y}");
        }
    }
    // Source mips are kept verbatim: mip 1..3 are the white block.
    for level in 1..4 {
        assert!(
            blob.mips[level]
                .chunks_exact(4)
                .all(|p| p == [255, 255, 255, 10]),
            "mip {level}"
        );
    }
}

#[test]
fn missing_mips_are_generated_and_source_mips_are_not_regenerated() {
    let (w, h) = (16, 8);
    let base = gradient_noise_bgra(w, h, 0);
    // One stored level: the chain is generated down to 1x1.
    let bytes = dds_bytes(w, h, D3DFormat::A8R8G8B8, 1, &base);
    let blob = &convert_dds(&bytes, ColorSpace::Linear, &[TextureFormat::Rgba8]).unwrap()[0];
    check_blob(blob, TextureFormat::Rgba8, w, h, full_chain_len(w, h));
    assert_eq!(blob.mips.len(), 5);
    assert!(!blob.srgb);

    // Two stored levels (second is flat grey): kept as is, tail generated from it.
    let mut data = base.clone();
    data.extend(std::iter::repeat_n(0x80u8, 8 * 4 * 4));
    let bytes = dds_bytes(w, h, D3DFormat::A8R8G8B8, 2, &data);
    let blob = &convert_dds(&bytes, ColorSpace::Linear, &[TextureFormat::Rgba8]).unwrap()[0];
    assert_eq!(blob.mips.len(), 5);
    assert!(blob.mips[1].iter().all(|&b| b == 0x80));
    assert!(blob.mips[2].iter().all(|&b| b == 0x80));
}

fn assert_psnr_all_mips(bgra: &[u8], w: u32, h: u32, space: ColorSpace) {
    let bytes = dds_bytes(w, h, D3DFormat::A8R8G8B8, 1, bgra);
    // Reference chain: what the pipeline feeds the encoders.
    let mut reference = decode_dds(&bytes).unwrap().mips;
    extend_mips(&mut reference, space);

    let blobs = convert_dds(&bytes, space, &ALL).unwrap();
    for (blob, format) in blobs.iter().zip(ALL) {
        check_blob(blob, format, w, h, reference.len());
        assert_eq!(blob.srgb, space == ColorSpace::Srgb);
        for (level, want) in reference.iter().enumerate() {
            let got = decode_mip(blob, level).unwrap();
            assert_eq!((got.width, got.height), (want.width, want.height));
            let p = psnr(&want.pixels, &got.pixels);
            println!("{format:?} {space:?} {w}x{h} mip {level}: {p:.1} dB");
            if format == TextureFormat::Rgba8 {
                assert_eq!(got.pixels, want.pixels);
            } else if level == 0 {
                assert!(p >= MIN_PSNR, "{format:?} {space:?} mip 0: {p:.1} dB");
            } else if want.width.min(want.height) >= 16 {
                // Small mips of a full-range gradient are steep inside a block, so only
                // sanity-check them; mip 0 is the contractual one.
                assert!(p >= 30.0, "{format:?} {space:?} mip {level}: {p:.1} dB");
            }
        }
    }
}

#[test]
fn bc7_and_astc_psnr_on_gradient_with_noise() {
    for space in [ColorSpace::Srgb, ColorSpace::Linear] {
        assert_psnr_all_mips(&gradient_noise_bgra(64, 64, 4), 64, 64, space);
    }
}

#[test]
fn non_multiple_of_four_sizes_encode_with_padded_edge_blocks() {
    // 37x21 -> 10x6 blocks at level 0; down to 1x1 where blocks are padded to 4x4.
    let (w, h) = (37, 21);
    assert_psnr_all_mips(&gradient_noise_bgra(w, h, 2), w, h, ColorSpace::Srgb);
    let bytes = dds_bytes(w, h, D3DFormat::A8R8G8B8, 1, &gradient_noise_bgra(w, h, 2));
    let blobs = convert_dds(&bytes, ColorSpace::Srgb, &ALL).unwrap();
    for b in &blobs[1..] {
        assert_eq!(b.mips[0].len(), 10 * 6 * 16);
        assert_eq!(b.mips.last().unwrap().len(), 16, "1x1 mip is a full block");
    }
}

#[test]
fn flat_and_tiny_textures_survive() {
    for (w, h) in [(1, 1), (2, 1), (4, 4), (3, 7)] {
        let data = vec![0x40u8; (w * h * 4) as usize];
        let bytes = dds_bytes(w, h, D3DFormat::A8R8G8B8, 1, &data);
        let blobs = convert_dds(&bytes, ColorSpace::Srgb, &ALL).unwrap();
        for (blob, format) in blobs.iter().zip(ALL) {
            check_blob(blob, format, w, h, full_chain_len(w, h));
            let got = decode_mip(blob, 0).unwrap();
            assert!(psnr(&decode_dds(&bytes).unwrap().mips[0].pixels, &got.pixels) > 40.0);
        }
    }
}

#[test]
fn error_cases() {
    assert!(matches!(
        convert_dds(b"not a dds", ColorSpace::Srgb, &ALL),
        Err(TextureError::Dds(_))
    ));
    let good = dds_bytes(8, 8, D3DFormat::A8R8G8B8, 1, &vec![0; 8 * 8 * 4]);
    assert!(matches!(
        convert_dds(&good[..good.len() - 1], ColorSpace::Srgb, &ALL),
        Err(TextureError::Truncated { .. })
    ));
    let r32f = dds_bytes(4, 4, D3DFormat::R32F, 1, &[0; 64]);
    // Whatever the container says, unsupported formats never panic.
    assert!(convert_dds(&r32f, ColorSpace::Linear, &ALL).is_err());
}

#[test]
fn warrior_textures_convert_with_psnr_at_least_35db() {
    let Some(root) = std::env::var_os("CIV5_ROOT") else {
        println!("skipped: CIV5_ROOT not set");
        return;
    };
    let src = GameSource::open(&root).expect("scan failed");
    let names: Vec<String> = src
        .iter_ext("dds")
        .filter(|e| e.name.starts_with("warrior"))
        .map(|e| e.name.clone())
        .collect();
    assert!(names.contains(&"warrior_diff.dds".to_string()), "{names:?}");
    assert!(names.contains(&"warrior_sref.dds".to_string()), "{names:?}");

    let started = std::time::Instant::now();
    for name in &names {
        let bytes = src.read(src.lookup(name).unwrap()).unwrap();
        let space = color_space_for(name);
        let source = decode_dds(&bytes).expect(name);
        let source_mips = source.mips.len();
        let blobs = convert_dds(&bytes, space, &ALL).expect(name);
        for blob in &blobs {
            let format = blob.format;
            check_blob(
                blob,
                format,
                source.mips[0].width,
                source.mips[0].height,
                source_mips,
            );
            assert_eq!(blob.mips.len(), source_mips, "{name} mip count preserved");
            for (level, want) in source.mips.iter().enumerate() {
                let got = decode_mip(blob, level).unwrap();
                let p = psnr(&want.pixels, &got.pixels);
                if level == 0 {
                    println!("{name} {space:?} {format:?} mip0: {p:.2} dB");
                    if format == TextureFormat::Rgba8 {
                        assert_eq!(got.pixels, want.pixels, "{name} RGBA8 must be exact");
                    } else {
                        assert!(p >= MIN_PSNR, "{name} {format:?} mip0: {p:.2} dB");
                    }
                }
            }
        }
    }
    println!(
        "{} warrior textures x 3 formats in {:.2}s",
        names.len(),
        started.elapsed().as_secs_f64()
    );
}
