//! T4 dumps (re-b16): ids, heights and depth bytes across a coast hex next to grassland in S2.

use std::path::PathBuf;

use golden::terrain_check::{build_terrain, terrain_basis};
use golden::wv3;
use render::terrain::{Relief, TerrainKind};

fn bc3(data: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = vec![0u8; w * h * 4];
    let c565 = |c: u16| {
        [
            f32::from((c >> 11) as u8 & 31) * 255.0 / 31.0,
            f32::from((c >> 5) as u8 & 63) * 255.0 / 63.0,
            f32::from(c as u8 & 31) * 255.0 / 31.0,
        ]
    };
    for (bi, blk) in data.chunks(16).enumerate() {
        let (bx, by) = ((bi % (w / 4)) * 4, (bi / (w / 4)) * 4);
        let (a0, a1) = (f32::from(blk[0]), f32::from(blk[1]));
        let mut abits = 0u64;
        for k in 0..6 {
            abits |= u64::from(blk[2 + k]) << (8 * k);
        }
        let alpha = |i: u64| -> f32 {
            match i {
                0 => a0,
                1 => a1,
                _ if a0 > a1 => ((8 - i) as f32 * a0 + (i - 1) as f32 * a1) / 7.0,
                6 => 0.0,
                7 => 255.0,
                _ => ((6 - i) as f32 * a0 + (i - 1) as f32 * a1) / 5.0,
            }
        };
        let e0 = c565(u16::from_le_bytes([blk[8], blk[9]]));
        let e1 = c565(u16::from_le_bytes([blk[10], blk[11]]));
        let cbits = u32::from_le_bytes([blk[12], blk[13], blk[14], blk[15]]);
        for p in 0..16 {
            let ci = (cbits >> (2 * p)) & 3;
            let t = [0.0, 1.0, 1.0 / 3.0, 2.0 / 3.0][ci as usize];
            let o = ((by + p / 4) * w + bx + p % 4) * 4;
            for k in 0..3 {
                out[o + k] = (e0[k] * (1.0 - t) + e1[k] * t).round() as u8;
            }
            out[o + 3] = alpha((abits >> (3 * p)) & 7).round() as u8;
        }
    }
    out
}

#[test]
#[ignore]
fn dds_to_png() {
    let src = std::env::var("DDS_IN").unwrap();
    let img = golden::image::read_dds_file(std::path::Path::new(&src)).unwrap();
    golden::image::write_png(
        std::path::Path::new(&std::env::var("PNG_OUT").unwrap()),
        &img,
    )
    .unwrap();
}

fn workspace() -> PathBuf {
    golden::captures::default_cache()
        .map(|c| c.join(".."))
        .unwrap_or_default()
}

#[test]
#[ignore]
fn dump_the_seabed_of_a_coast_hex_next_to_grassland() {
    let ws = workspace();
    let scene = std::env::var("T4_SCENE").unwrap_or_else(|_| "S2".into());
    let pack = std::env::var("T4_PACK").unwrap_or_else(|_| "b13".into());
    let basis_file = ws.join(format!("cache/golden/{scene}/mapbasis.wv3"));
    let assets = ws.join(format!("assets-out/{pack}"));
    let mb = wv3::decode_basis(&std::fs::read(&basis_file).unwrap()).unwrap();
    let basis = terrain_basis(&mb).unwrap();
    let terrain = build_terrain(&assets, basis.clone()).unwrap();
    let s = terrain.sampler().unwrap();
    let defs = terrain.library().defs().rows().to_vec();
    let name = |id: u8| {
        defs.get(id as usize)
            .and_then(|r| r.diffuse.as_ref())
            .map_or("?".to_string(), |d| format!("{d:?}"))
    };
    for (i, r) in defs.iter().enumerate() {
        eprintln!(
            "row {i:2}: cont {} id {:x} scale {} {:?}",
            r.continent,
            r.id,
            r.texture_scale,
            r.diffuse.as_ref().map(|d| format!("{d:?}"))
        );
    }
    // A coast plot whose west neighbour is flat grassland.
    let mut found = 0;
    for y in 0..i32::from(basis.height) {
        for x in 1..i32::from(basis.width) {
            let (Some(p), Some(w)) = (basis.plot(x, y), basis.plot(x - 1, y)) else {
                continue;
            };
            if !(p.relief == Relief::Water
                && p.terrain == TerrainKind::Coast
                && w.relief == Relief::Flat
                && w.terrain == TerrainKind::Grass)
            {
                continue;
            }
            found += 1;
            if found > 2 {
                break;
            }
            let c = s.frame().hex_to_tex(x, y);
            let cw = s.frame().hex_to_tex(x - 1, y);
            eprintln!("coast hex ({x},{y}) centre {c:?}; grass west centre {cw:?}");
            let v = c.y as i32;
            let mut hist = std::collections::BTreeMap::<u8, u32>::new();
            let r = s.frame().hex_texel_rect(x, y);
            for vv in r.v0..r.v1 {
                for uu in r.u0..r.u1 {
                    if s.frame()
                        .tex_to_hex(glam::Vec2::new(uu as f32 + 0.5, vv as f32 + 0.5))
                        == (x, y)
                    {
                        *hist.entry(s.field().get_id(uu, vv)).or_default() += 1;
                    }
                }
            }
            eprintln!(
                "  id histogram of the hex: {:?}",
                hist.iter()
                    .map(|(k, n)| format!("{k}:{}={n}", name(*k)))
                    .collect::<Vec<_>>()
            );
            for dv in [-8, 0, 8] {
                let vv = v + dv;
                let mut line = String::new();
                for u in (cw.x as i32)..=(c.x as i32 + 32) {
                    let h = s.raw_height(u, vv);
                    let id = s.field().get_id(u, vv);
                    line += &format!("{}:{}/{}/{} ", u - c.x as i32, id, h, h >> 8);
                }
                eprintln!("  row v={vv} (u-centre:id/raw/raw>>8): {line}");
            }
        }
    }
    assert!(found > 0, "no coast hex next to grassland");
    // 4: the seabed texture of the pack.
    use asset_format::{ContentHash, LogicalId, Manifest, PlatformProfile, TextureBlob};
    let text = std::fs::read_to_string(assets.join("manifest")).unwrap();
    let hash: ContentHash = text.trim().parse().unwrap();
    let manifest =
        Manifest::decode(&std::fs::read(assets.join(hash.to_string())).unwrap()).unwrap();
    for name in [
        "tx:euro_shallow_seas_d.dds",
        "tx:euro_coast_d.dds",
        "tx:asia_grassland_d.dds",
        "tx:wave_coastline_all.dds",
        "tx:ice_environment_diffuse.dds",
        "tx:ice_d.dds",
        "tx:ice_d_sides.dds",
    ] {
        let id = LogicalId::new(name);
        let v = manifest
            .resolve(&id, &PlatformProfile::uncompressed_only())
            .or_else(|| manifest.variants(&id).first())
            .unwrap();
        let bytes = std::fs::read(assets.join(v.hash.to_string())).unwrap();
        let b = TextureBlob::decode(&bytes).unwrap();
        let m0 = &if format!("{:?}", b.format) == "Bc3" {
            bc3(&b.mips[0], b.width as usize, b.height as usize)
        } else {
            b.mips[0].clone()
        };
        eprintln!(
            "{name}: {:?} srgb {} {}x{} mips {} bytes0 {}",
            b.format,
            b.srgb,
            b.width,
            b.height,
            b.mips.len(),
            m0.len()
        );
        if m0.len() == (b.width * b.height * 4) as usize {
            let n = (b.width * b.height) as f64;
            let mut s = [0f64; 4];
            for p in m0.chunks(4) {
                for k in 0..4 {
                    s[k] += f64::from(p[k]);
                }
            }
            eprintln!("  mean rgba {:?}", s.map(|x| (x / n).round()));
            let out = std::env::var("T4_OUT").unwrap_or_else(|_| ".".into());
            golden::image::write_png(
                &std::path::Path::new(&out).join(format!("{}.png", &name[3..name.len() - 4])),
                &golden::image::Rgba8 {
                    width: b.width as usize,
                    height: b.height as usize,
                    rgba: m0.clone(),
                },
            )
            .unwrap();
        }
    }
}
