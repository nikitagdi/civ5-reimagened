//! UI textures keep the bytes of the file.

use asset_format::{TextureBlob, TextureFormat};

use crate::textures::{RawDds, RawFormat, TextureError, read_dds};

/// A UI DDS as a blob of the same pixels. `L16` index textures become `Rg8` with the same bytes:
/// R is the low byte of the index, G the high byte (`index = g * 256 + r`).
///
/// A block-dictionary texture (see [`block_size`]) given `index`, the bytes of its `x-index.dds`
/// partner, is expanded to the image the game's shaders sample: font atlases, icon atlases and palettes
/// alike. Without `index` the pool is kept as stored.
pub fn ui_texture(bytes: &[u8], index: Option<&[u8]>) -> Result<TextureBlob, TextureError> {
    let mut raw = read_dds(bytes)?;
    if raw.format == RawFormat::R16 {
        return Ok(TextureBlob {
            format: TextureFormat::Rg8,
            dim: raw.dim,
            srgb: false,
            width: raw.width,
            height: raw.height,
            depth: raw.depth,
            mips: raw.mips,
        });
    }
    // A pool of one block is a stub (EUI blanks a texture with a 1x1 file): nothing to expand.
    let stub = |b: u32| raw.width <= b && raw.height <= b;
    if let (Some(block), Some(index)) = (block_size(bytes), index)
        && !stub(block)
    {
        raw = expand_blocks(&raw, block, &read_dds(index)?)?;
    }
    raw.into_blob()
}

/// Edge in pixels of the blocks of a font atlas stored as a dictionary, from the `FTXT` header the
/// engine reads in `FGXFont::LoadTextureFromXML` (`sscanf(type, "BC%3x")`): `BC004` is 4x4, `BC002` is 2x2.
/// The DDS size of such a file is the size of the block pool, not of the atlas; the `-index` partner
/// holds one u16 (or u8) per block of the atlas: the block's position in the pool (0 is pool block 0,
/// which a font keeps empty and a colour texture such as TopPanelBar keeps opaque).
pub fn block_size(bytes: &[u8]) -> Option<u32> {
    if bytes.get(32..36)? != b"FTXT" {
        return None;
    }
    let tag = bytes.get(116..121)?;
    let digits = tag.strip_prefix(b"BC")?;
    let n = std::str::from_utf8(digits)
        .ok()
        .and_then(|d| u32::from_str_radix(d, 16).ok())?;
    (n > 0).then_some(n)
}

/// The atlas of a block pool and its index (`index.width * block` by `index.height * block`).
fn expand_blocks(pool: &RawDds, block: u32, index: &RawDds) -> Result<RawDds, TextureError> {
    let bad = |m: String| TextureError::Unsupported(m);
    let bpp: usize = match pool.format {
        RawFormat::R8 => 1,
        RawFormat::Rg8 => 2,
        RawFormat::Rgba8 => 4,
        f => return Err(bad(format!("block dictionary of {f:?}"))),
    };
    // u8 or u16 per block (the file is L8 or L16).
    let wide = match index.format {
        RawFormat::R8 => false,
        RawFormat::R16 => true,
        f => return Err(bad(format!("block index of {f:?}"))),
    };
    let b = block as usize;
    let (pw, ph) = (pool.width as usize, pool.height as usize);
    if pw % b != 0 || ph % b != 0 {
        return Err(bad(format!("pool {pw}x{ph} is not a multiple of {b}")));
    }
    let per_row = pw / b;
    let pool_blocks = per_row * (ph / b);
    let pool_px = pool
        .mips
        .first()
        .ok_or_else(|| bad("pool without pixels".into()))?;
    let idx = index
        .mips
        .first()
        .ok_or_else(|| bad("index without pixels".into()))?;
    let (iw, ih) = (index.width as usize, index.height as usize);
    let (w, h) = (iw * b, ih * b);
    let mut out = vec![0u8; w * h * bpp];
    for by in 0..ih {
        for bx in 0..iw {
            let i = by * iw + bx;
            let n = if wide {
                usize::from(u16::from_le_bytes([idx[i * 2], idx[i * 2 + 1]]))
            } else {
                usize::from(idx[i])
            };
            // Index 0 is pool block 0, not "nothing": fonts keep an empty block there, but a colour
            // texture (TopPanelBar) keeps its opaque black edge there, and leaving it zero made the
            // bar translucent.
            if n >= pool_blocks {
                return Err(bad(format!(
                    "block {n} is outside the pool of {pool_blocks}"
                )));
            }
            let (sx, sy) = (n % per_row * b, n / per_row * b);
            for y in 0..b {
                let src = ((sy + y) * pw + sx) * bpp;
                let dst = ((by * b + y) * w + bx * b) * bpp;
                out[dst..dst + b * bpp].copy_from_slice(&pool_px[src..src + b * bpp]);
            }
        }
    }
    Ok(RawDds {
        format: pool.format,
        dim: pool.dim,
        width: w as u32,
        height: h as u32,
        depth: 1,
        mips: vec![out],
    })
}

/// `x-index.dds` is the index of `x.dds`.
pub fn index_of(name: &str) -> Option<String> {
    let stem = name.strip_suffix("-index.dds")?;
    Some(format!("{stem}.dds"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ddsfile::{D3DFormat, Dds, NewD3dParams};

    fn dds(format: D3DFormat, w: u32, h: u32) -> Vec<u8> {
        let mut d = Dds::new_d3d(NewD3dParams {
            height: h,
            width: w,
            depth: None,
            format,
            mipmap_levels: None,
            caps2: None,
        })
        .unwrap();
        for (i, b) in d.data.iter_mut().enumerate() {
            *b = (i * 7 + 1) as u8;
        }
        let mut out = Vec::new();
        d.write(&mut out).unwrap();
        if format == D3DFormat::L16 {
            // ddsfile writes L16 as an RGB mask; the game's files are luminance (0x20000).
            out[80..84].copy_from_slice(&0x2_0000u32.to_le_bytes());
            out[84..88].fill(0);
            out[88..92].copy_from_slice(&16u32.to_le_bytes());
            out[92..96].copy_from_slice(&0xffffu32.to_le_bytes());
            out[96..108].fill(0);
        }
        out
    }

    #[test]
    fn l16_becomes_rg8_with_the_same_bytes() {
        let f = dds(D3DFormat::L16, 4, 2);
        let src = Dds::read(&f[..]).unwrap();
        let t = ui_texture(&f, None).unwrap();
        assert_eq!(t.format, TextureFormat::Rg8);
        assert_eq!((t.width, t.height), (4, 2));
        assert_eq!(t.mips[0], src.data);
        assert!(!t.srgb);
    }

    #[test]
    fn argb_becomes_rgba_and_dxt_is_kept() {
        let f = dds(D3DFormat::A8R8G8B8, 2, 1);
        let t = ui_texture(&f, None).unwrap();
        assert_eq!(t.format, TextureFormat::Rgba8);
        // A8R8G8B8 bytes B,G,R,A become R,G,B,A.
        let src = Dds::read(&f[..]).unwrap().data;
        assert_eq!(&t.mips[0][..4], &[src[2], src[1], src[0], src[3]]);
        let f = dds(D3DFormat::DXT5, 4, 4);
        assert_eq!(ui_texture(&f, None).unwrap().format, TextureFormat::Bc3);
    }

    #[test]
    fn index_partner_names() {
        assert_eq!(
            index_of("marcpips-index.dds").as_deref(),
            Some("marcpips.dds")
        );
        assert_eq!(index_of("marcpips.dds"), None);
    }

    /// An A8 file as the font atlases are stored: alpha-only pixel format, 8 bits.
    fn alpha8(w: u32, h: u32) -> Vec<u8> {
        let mut out = dds(D3DFormat::A8R8G8B8, w, h);
        out.truncate(128 + (w * h) as usize);
        out[80..84].copy_from_slice(&2u32.to_le_bytes());
        out[84..88].fill(0);
        out[88..92].copy_from_slice(&8u32.to_le_bytes());
        out[92..108].fill(0);
        out[104..108].copy_from_slice(&0xffu32.to_le_bytes());
        out
    }

    /// A dictionary file as the game's tools write it: `FTXT` at 32, `BCxxx` at 116.
    fn block_file(mut f: Vec<u8>, tag: &[u8; 5]) -> Vec<u8> {
        f[32..36].copy_from_slice(b"FTXT");
        f[116..121].copy_from_slice(tag);
        f
    }

    #[test]
    fn block_dictionary_expands_through_its_index() {
        // Pool 4x2 of 2x2 blocks: block 0 is the empty one, block 1 holds 1..=4.
        let mut pool = alpha8(4, 2);
        pool[128..136].copy_from_slice(&[0, 0, 1, 2, 0, 0, 3, 4]);
        let pool = block_file(pool, b"BC002");
        assert_eq!(block_size(&pool), Some(2));
        // Index 2x2: block 1 at the top right and the bottom left.
        let mut index = dds(D3DFormat::L16, 2, 2);
        index[128..136].copy_from_slice(&[0, 0, 1, 0, 1, 0, 0, 0]);
        let blob = ui_texture(&pool, Some(&index)).unwrap();
        assert_eq!(blob.format, TextureFormat::R8);
        assert_eq!((blob.width, blob.height), (4, 4));
        #[rustfmt::skip]
        assert_eq!(blob.mips[0], [
            0, 0, 1, 2,
            0, 0, 3, 4,
            1, 2, 0, 0,
            3, 4, 0, 0,
        ]);
        assert_eq!(ui_texture(&pool, None).unwrap().width, 4);
        // An ordinary texture is not a dictionary.
        assert_eq!(block_size(&alpha8(4, 2)), None);
    }

    #[test]
    fn block_index_outside_the_pool_is_an_error() {
        let pool = block_file(alpha8(4, 2), b"BC002");
        let mut index = dds(D3DFormat::L16, 1, 1);
        index[128..130].copy_from_slice(&2u16.to_le_bytes());
        assert!(ui_texture(&pool, Some(&index)).is_err());
    }

    /// The real font atlases of `UITextures.fpk` come out at the size their `.ggxml` names, with ink
    /// inside the glyph rectangles and none in the gaps between them.
    #[test]
    fn game_font_atlases_match_their_ggxml() {
        use crate::game::fpk;
        use std::fs::File;
        use std::path::PathBuf;

        let Some(game) = std::env::var_os("CIV5_GAME_DIR").map(PathBuf::from) else {
            eprintln!("SKIPPED: game files not found (set CIV5_GAME_DIR)");
            return;
        };
        let pack = game.join("Resource/DX9/UITextures.fpk");
        let Ok(entries) = fpk::open_directory(&pack) else {
            eprintln!("SKIPPED: {} not found", pack.display());
            return;
        };
        let mut f = File::open(&pack).unwrap();
        let mut read = |name: &str| {
            let e = entries
                .iter()
                .find(|e| e.name == name)
                .unwrap_or_else(|| panic!("{name}"));
            fpk::read_payload(&mut f, e).unwrap()
        };
        // (atlas, its .ggxml, bytes per pixel of the stored atlas)
        let cases = [
            (
                "twcenmt16base",
                "Assets/UI/Fonts/Tw Cent MT/TwCenMT16.ggxml",
                1usize,
            ),
            ("fonticons", "Assets/UI/Fonts/Font Icons/FontIcons.ggxml", 4),
        ];
        for (stem, ggxml, bpp) in cases {
            let base = read(&format!("{stem}.dds"));
            let index = read(&format!("{stem}-index.dds"));
            let blob = ui_texture(&base, Some(&index)).unwrap();
            let xml = std::fs::read_to_string(game.join(ggxml)).unwrap();
            let size = |tag: &str| -> u32 {
                let head = &xml[xml.find("<textures").unwrap()..];
                let head = &head[..head.find('>').unwrap()];
                let v = &head[head.find(&format!("{tag}=\"")).unwrap() + tag.len() + 2..];
                v[..v.find('"').unwrap()].parse().unwrap()
            };
            assert_eq!(
                (blob.width, blob.height),
                (size("width"), size("height")),
                "{stem}"
            );
            let (w, h) = (blob.width as usize, blob.height as usize);
            let px = &blob.mips[0];
            assert_eq!(px.len(), w * h * bpp);
            // Coverage: the byte of A8, the alpha of RGBA.
            let ink = |x: usize, y: usize| -> bool {
                let p = (y * w + x) * bpp;
                if bpp == 1 { px[p] != 0 } else { px[p + 3] != 0 }
            };
            let mut inside = vec![false; w * h];
            let (mut rects, mut inked) = (0, 0);
            for g in xml.split("<glyph ").skip(1) {
                let attr = |k: &str| -> usize {
                    let v = &g[g.find(&format!("{k}=\"")).unwrap() + k.len() + 2..];
                    v[..v.find('"').unwrap()].parse().unwrap()
                };
                let (u, v, gw, gh) = (attr("u"), attr("v"), attr("width"), attr("height"));
                if gw == 0 || gh == 0 {
                    continue;
                }
                rects += 1;
                let mut any = false;
                for y in v..v + gh {
                    for x in u..u + gw {
                        inside[y * w + x] = true;
                        any |= ink(x, y);
                    }
                }
                inked += usize::from(any);
            }
            assert!(rects > 50, "{stem}: {rects} rects");
            assert_eq!(inked, rects, "{stem}: every glyph rectangle holds ink");
            let stray = (0..h)
                .flat_map(|y| (0..w).map(move |x| (x, y)))
                .filter(|&(x, y)| !inside[y * w + x] && ink(x, y))
                .count();
            assert_eq!(stray, 0, "{stem}: ink between the glyph rectangles");
        }
    }

    #[test]
    fn block_index_of_one_byte_per_block() {
        // Pool 4x2 of 2x2 blocks as above, but the index is an L8 file.
        let mut pool = alpha8(4, 2);
        pool[128..136].copy_from_slice(&[0, 0, 1, 2, 0, 0, 3, 4]);
        let pool = block_file(pool, b"BC002");
        let mut index = alpha8(2, 1);
        index[128..130].copy_from_slice(&[1, 0]);
        let blob = ui_texture(&pool, Some(&index)).unwrap();
        assert_eq!((blob.width, blob.height), (4, 2));
        assert_eq!(blob.mips[0], [1, 2, 0, 0, 3, 4, 0, 0]);
    }

    /// Every dictionary of `UITextures.fpk` expands to the size of its index times its block.
    #[test]
    fn game_dictionaries_all_expand() {
        use crate::game::fpk;
        use std::fs::File;
        use std::path::PathBuf;

        let Some(game) = std::env::var_os("CIV5_GAME_DIR").map(PathBuf::from) else {
            eprintln!("SKIPPED: game files not found (set CIV5_GAME_DIR)");
            return;
        };
        let pack = game.join("Resource/DX9/UITextures.fpk");
        let Ok(entries) = fpk::open_directory(&pack) else {
            eprintln!("SKIPPED: {} not found", pack.display());
            return;
        };
        let mut f = File::open(&pack).unwrap();
        let (mut n, mut stubs, mut sizes) = (0, 0, std::collections::BTreeSet::new());
        for e in entries
            .iter()
            .filter(|e| e.name.ends_with(".dds") && !e.name.ends_with("-index.dds"))
        {
            let base = fpk::read_payload(&mut f, e).unwrap();
            let Some(block) = block_size(&base) else {
                continue;
            };
            let stem = e.name.strip_suffix(".dds").unwrap();
            let ie = entries
                .iter()
                .find(|x| x.name == format!("{stem}-index.dds"))
                .unwrap_or_else(|| panic!("{stem} has no index"));
            let index = fpk::read_payload(&mut f, ie).unwrap();
            let want = read_dds(&index).unwrap();
            let blob = ui_texture(&base, Some(&index)).unwrap_or_else(|m| panic!("{stem}: {m}"));
            let pool = read_dds(&base).unwrap();
            if pool.width <= block && pool.height <= block {
                // A blank stub stays as stored.
                assert_eq!(
                    (blob.width, blob.height),
                    (pool.width, pool.height),
                    "{stem}"
                );
                stubs += 1;
                continue;
            }
            assert_eq!(
                (blob.width, blob.height),
                (want.width * block, want.height * block),
                "{stem}"
            );
            sizes.insert(block);
            n += 1;
        }
        assert!(n > 400, "{n} dictionaries, {stubs} stubs");
        assert!(
            [2, 4, 8, 256].iter().all(|b| sizes.contains(b)),
            "{sizes:?}"
        );
    }

    /// TopPanelBar: an RGBA dictionary whose index points at pool block 0, so index 0 is not empty.
    #[test]
    fn game_top_panel_bar_is_opaque() {
        use crate::game::fpk;
        use std::fs::File;
        use std::path::PathBuf;

        let Some(game) = std::env::var_os("CIV5_GAME_DIR").map(PathBuf::from) else {
            eprintln!("SKIPPED: game files not found (set CIV5_GAME_DIR)");
            return;
        };
        let pack = game.join("Resource/DX9/UITextures.fpk");
        let Ok(entries) = fpk::open_directory(&pack) else {
            eprintln!("SKIPPED: {} not found", pack.display());
            return;
        };
        let mut f = File::open(&pack).unwrap();
        let mut read = |name: &str| {
            let e = entries.iter().find(|e| e.name == name).unwrap();
            fpk::read_payload(&mut f, e).unwrap()
        };
        let blob = ui_texture(
            &read("toppanelbar.dds"),
            Some(&read("toppanelbar-index.dds")),
        )
        .unwrap();
        assert_eq!(
            (blob.format, blob.width, blob.height),
            (TextureFormat::Rgba8, 32, 32)
        );
        let px = &blob.mips[0];
        let clear = px.chunks(4).filter(|p| p[3] == 0).count();
        eprintln!("TopPanelBar: {clear} of 1024 texels fully transparent");
        assert_eq!(clear, 0, "the bar has no transparent texel");
    }

    /// Index 0 means pool block 0 (an RGBA pool keeps real pixels there), not an empty block.
    #[test]
    fn block_index_zero_is_pool_block_zero() {
        // Pool 4x1 RGBA of 2x2 blocks would need 2 rows; use 2x2 blocks: pool 4x2, blocks 0 and 1.
        let mut pool = dds(D3DFormat::A8R8G8B8, 4, 2);
        for (i, b) in pool[128..].iter_mut().enumerate() {
            *b = if i % 4 == 3 { 0xff } else { 0x10 };
        }
        // Block 1 is fully transparent.
        for y in 0..2 {
            for x in 2..4 {
                let p = 128 + (y * 4 + x) * 4;
                pool[p..p + 4].fill(0);
            }
        }
        let pool = block_file(pool, b"BC002");
        // Index 2x1: block 0 then block 1.
        let mut index = dds(D3DFormat::L16, 2, 1);
        index[128..132].copy_from_slice(&[0, 0, 1, 0]);
        let blob = ui_texture(&pool, Some(&index)).unwrap();
        assert_eq!((blob.width, blob.height), (4, 2));
        let a = |x: usize, y: usize| blob.mips[0][(y * 4 + x) * 4 + 3];
        assert_eq!([a(0, 0), a(1, 0), a(0, 1), a(1, 1)], [255; 4]);
        assert_eq!([a(2, 0), a(3, 0), a(2, 1), a(3, 1)], [0; 4]);
    }

    #[test]
    fn one_block_stub_is_kept_as_stored() {
        let pool = block_file(alpha8(1, 1), b"BC001");
        let mut index = dds(D3DFormat::L16, 2, 2);
        index[128..136].copy_from_slice(&[0, 0, 5, 0, 0, 0, 9, 0]);
        let blob = ui_texture(&pool, Some(&index)).unwrap();
        assert_eq!((blob.width, blob.height), (1, 1));
    }
}
