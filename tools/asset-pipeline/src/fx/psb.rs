//! Reader of Fork particle files (`.psb`), by the table of
//! `civ5-re` answers-b updates 6, 7 and 10 (RW-29).
//!
//! Layout (little-endian; offsets relative to the start the comment names):
//! * file: u32 at 0x0C is the offset of the first chunk; nothing else of the header is read.
//! * chunk (0x10C header): `name[0x100]`, u32 size at 0x100 (the next chunk is at chunk + size),
//!   u32 data offset at 0x104, u8 type at 0x108 (0 emitter, 1 track), 0x109 bit 0 last chunk.
//!   The last chunk ends at the file's end: no byte is left over.
//! * emitter data: the property (0x1FC bytes) then blocks named by `(count, offset)` pairs of the
//!   property, offsets relative to the data start.
//!
//! The reader rejects any block outside its chunk, a custom-data list that does not sum to its
//! size, unsorted track keys and a chunk list that does not end at the file's end.

use asset_format::{
    FxEffect, FxEmitter, FxForce, FxFrame, FxPath, FxProps, FxShader, FxTrack, LogicalId,
};

/// Bytes of the emitter property.
pub const PROPERTY_LEN: usize = 0x1FC;
/// Bytes of a chunk header.
pub const CHUNK_HEADER_LEN: usize = 0x10C;
/// Bytes of an asset record (texture, shader, ref and emission assets).
pub const ASSET_LEN: usize = 0x120;
/// Bytes of a force record.
pub const FORCE_LEN: usize = 0x20;
/// Bytes of a custom-data sub-chunk header.
pub const CUSTOM_HEADER_LEN: usize = 0x2C;
/// Bytes of a track key the reader needs: frame and property.
pub const KEY_LEN: usize = 4 + PROPERTY_LEN;
/// Custom-data type of the shader parameters (`GetCustomDataChunkOfType(10)`).
pub const SHADER_DATA_TYPE: u32 = 10;

/// What a file holds besides the effect, for the report.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct PsbNotes {
    /// Texture file names the resolver did not know.
    pub missing_textures: Vec<String>,
    /// Standalone track chunks (type 1); none in the shipped files.
    pub track_chunks: usize,
    /// Emitters with emission assets (no reader in the SDK; not kept).
    pub emission_assets: usize,
    /// A ref asset that names no emitter of the file.
    pub unknown_refs: Vec<String>,
}

struct Reader<'a> {
    b: &'a [u8],
}

impl<'a> Reader<'a> {
    fn u32(&self, at: usize) -> Result<u32, String> {
        self.b
            .get(at..at + 4)
            .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
            .ok_or_else(|| format!("u32 at {at:#x} past the end"))
    }

    fn f32(&self, at: usize) -> Result<f32, String> {
        self.u32(at).map(f32::from_bits)
    }

    fn fs<const N: usize>(&self, at: usize) -> Result<[f32; N], String> {
        let mut out = [0.0; N];
        for (i, v) in out.iter_mut().enumerate() {
            *v = self.f32(at + 4 * i)?;
        }
        Ok(out)
    }

    fn u8(&self, at: usize) -> Result<u8, String> {
        self.b
            .get(at)
            .copied()
            .ok_or_else(|| format!("byte at {at:#x} past the end"))
    }

    /// A NUL-terminated string in `len` bytes.
    fn cstr(&self, at: usize, len: usize) -> Result<String, String> {
        let s = self
            .b
            .get(at..at + len)
            .ok_or_else(|| format!("string at {at:#x} past the end"))?;
        let end = s.iter().position(|c| *c == 0).unwrap_or(len);
        Ok(s[..end].iter().map(|c| char::from(*c)).collect())
    }
}

/// The property at `at`.
pub fn read_props(b: &[u8], at: usize) -> Result<FxProps, String> {
    let r = Reader { b };
    if at + PROPERTY_LEN > b.len() {
        return Err(format!("property at {at:#x} past the end"));
    }
    let colors = [
        r.fs::<4>(at + 0x10)?,
        r.fs::<4>(at + 0x20)?,
        r.fs::<4>(at + 0x30)?,
        r.fs::<4>(at + 0x40)?,
    ];
    let alpha = r.u32(at + 0x10C)?;
    Ok(FxProps {
        colors,
        times: r.fs(at + 0x50)?,
        life: r.fs(at + 0x58)?,
        speed: r.fs(at + 0x60)?,
        flags: r.u32(at + 0x68)?,
        size_max: r.fs(at + 0x6C)?,
        mid_time: r.f32(at + 0x78)?,
        spin: r.fs(at + 0x7C)?,
        drag: r.f32(at + 0x84)?,
        size_scale: r.fs(at + 0x88)?,
        rotation: r.fs(at + 0x98)?,
        tint: r.fs(at + 0xA8)?,
        size_min: r.fs(at + 0xB8)?,
        prewarm: r.f32(at + 0xC4)?,
        rate: r.f32(at + 0xCC)?,
        gravity: r.f32(at + 0xD0)?,
        volume: r.fs(at + 0xD4)?,
        max_live: r.f32(at + 0xE4)?,
        volume_kind: r.u8(at + 0xE8)?,
        shape: r.u8(at + 0xE9)?,
        burst: r.u32(at + 0x104)? as i32,
        flipbook_fps: r.f32(at + 0x108)?,
        alpha: u8::try_from(alpha).map_err(|_| format!("alpha state {alpha}"))?,
        time_scale: r.f32(at + 0x110)?,
        tube_radius: r.fs(at + 0x188)?,
        path_speed: r.f32(at + 0x190)?,
        start_delay: r.fs(at + 0x19C)?,
        matrix: r.fs(at + 0x1A8)?,
        burst_interval: r.fs(at + 0x1E8)?,
        extra: r.fs(at + 0x1F0)?,
        flags2: r.u32(at + 0x1F8)?,
    })
}

/// The file name of a path (after the last `\` or `/`).
pub fn file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// One emitter chunk before the ref assets are resolved.
struct RawEmitter {
    emitter: FxEmitter,
    ref_name: Option<String>,
}

/// Reads one `.psb`. `name` is the file name without extension (lower case); `texture` maps a
/// texture file name to its `tx:` id (`None`: not in the install, the name goes to the notes).
pub fn read_psb(
    name: &str,
    b: &[u8],
    texture: &dyn Fn(&str) -> Option<LogicalId>,
) -> Result<(FxEffect, PsbNotes), String> {
    let r = Reader { b };
    let mut notes = PsbNotes::default();
    let mut raw: Vec<RawEmitter> = Vec::new();
    let mut p = r.u32(0x0C)? as usize;
    loop {
        if p + CHUNK_HEADER_LEN > b.len() {
            return Err(format!("chunk header at {p:#x} past the end"));
        }
        let size = r.u32(p + 0x100)? as usize;
        let doff = r.u32(p + 0x104)? as usize;
        let kind = r.u8(p + 0x108)?;
        let last = r.u8(p + 0x109)? & 1 != 0;
        let end = p + size;
        if size < CHUNK_HEADER_LEN || doff > size || end > b.len() {
            return Err(format!(
                "chunk at {p:#x}: size {size:#x}, data at {doff:#x}"
            ));
        }
        let chunk_name = r.cstr(p, 0x100)?;
        match kind {
            0 => {
                let wrap = |e: String| format!("emitter {chunk_name}: {e}");
                raw.push(
                    read_emitter(&r, &chunk_name, p + doff, end, texture, &mut notes)
                        .map_err(wrap)?,
                );
            }
            1 => notes.track_chunks += 1,
            k => return Err(format!("chunk {chunk_name} of unknown type {k}")),
        }
        if last {
            if end != b.len() {
                return Err(format!("{} byte(s) after the last chunk", b.len() - end));
            }
            break;
        }
        p = end;
        if p >= b.len() {
            return Err("no chunk is marked last".into());
        }
    }
    let names: Vec<String> = raw.iter().map(|e| e.emitter.name.clone()).collect();
    let emitters = raw
        .into_iter()
        .map(|mut e| {
            if let Some(n) = e.ref_name {
                match names.iter().position(|x| x.eq_ignore_ascii_case(&n)) {
                    Some(i) => e.emitter.sub_emitter = Some(i as u16),
                    None => notes.unknown_refs.push(n),
                }
            }
            e.emitter
        })
        .collect();
    let fx = FxEffect {
        name: name.to_owned(),
        emitters,
    };
    fx.validate().map_err(|e| e.to_string())?;
    Ok((fx, notes))
}

/// A block of `count` records of `len` bytes at the data offset in the pair; must lie in the
/// chunk. Returns the absolute start.
fn block(
    r: &Reader<'_>,
    d: usize,
    end: usize,
    count_at: usize,
    ptr_at: usize,
    len: usize,
) -> Result<Option<(usize, usize)>, String> {
    let n = r.u32(d + count_at)? as usize;
    if n == 0 {
        return Ok(None);
    }
    let s = d + r.u32(d + ptr_at)? as usize;
    if s + n * len > end {
        return Err(format!(
            "block {count_at:#x} ({n} x {len:#x} at {s:#x}) outside the chunk"
        ));
    }
    Ok(Some((s, n)))
}

fn read_emitter(
    r: &Reader<'_>,
    name: &str,
    d: usize,
    end: usize,
    texture: &dyn Fn(&str) -> Option<LogicalId>,
    notes: &mut PsbNotes,
) -> Result<RawEmitter, String> {
    if d + PROPERTY_LEN > end {
        return Err("data shorter than the property".into());
    }
    let props = read_props(r.b, d)?;

    // Texture assets: the flipbook frames.
    let mut frames = Vec::new();
    if let Some((s, n)) = block(r, d, end, 0x124, 0x12C, ASSET_LEN)? {
        for i in 0..n {
            let a = s + i * ASSET_LEN;
            let kind = r.u32(a)?;
            let key = r.cstr(a + 4, 0x100)?;
            let tex = if kind == 1 {
                let f = file_name(&key);
                let t = texture(f);
                if t.is_none() {
                    notes.missing_textures.push(f.to_owned());
                }
                t
            } else {
                None
            };
            frames.push(FxFrame {
                texture: tex,
                uv: r.fs(a + 0x108)?,
            });
        }
    }

    let mut forces = Vec::new();
    if let Some((s, n)) = block(r, d, end, 0x114, 0x118, FORCE_LEN)? {
        for i in 0..n {
            let a = s + i * FORCE_LEN;
            let kind = r.f32(a + 0x14)?;
            forces.push(FxForce {
                position: r.fs(a)?,
                strength: r.f32(a + 0xC)?,
                radius: r.f32(a + 0x10)?,
                kind: if (0.0..=8.0).contains(&kind) {
                    kind as u8
                } else {
                    return Err(format!("force type {kind}"));
                },
            });
        }
    }

    if block(r, d, end, 0x130, 0x134, ASSET_LEN)?.is_some() {
        notes.emission_assets += 1;
    }

    // Custom data: the sub-chunks must sum to the size; type 10 holds the shader parameters.
    let mut shader_data: Option<Vec<u8>> = None;
    if let Some((s, n)) = block(r, d, end, 0x11C, 0x120, 1)? {
        let lim = s + n;
        let mut q = s;
        while q < lim {
            if q + CUSTOM_HEADER_LEN > lim {
                return Err("custom data header past its block".into());
            }
            let kind = r.u32(q + 0x20)?;
            let size = r.u32(q + 0x24)? as usize;
            let body = q + CUSTOM_HEADER_LEN;
            if body + size > lim {
                return Err("custom data past its block".into());
            }
            if kind == SHADER_DATA_TYPE {
                shader_data = Some(r.b[body..body + size].to_vec());
            }
            q = body + size;
        }
        if q != lim {
            return Err("custom data does not sum to its size".into());
        }
    }

    let shader_key = match block(r, d, end, 0x180, 0x184, ASSET_LEN)? {
        Some((s, _)) => Some(r.cstr(s + 4, 0x100)?),
        None => None,
    };
    let shader = shader_of(
        shader_key.as_deref(),
        shader_data.as_deref(),
        texture,
        notes,
    );

    let ref_name = match block(r, d, end, 0x194, 0x198, ASSET_LEN)? {
        Some((s, _)) => Some(r.cstr(s + 4, 0x100)?),
        None => None,
    };

    let track = if r.u32(d + 0x138)? != 0 {
        let t = d + r.u32(d + 0x13C)? as usize;
        Some(read_track(r, t, end)?)
    } else {
        None
    };

    let path = if r.u32(d + 0x170)? != 0 {
        let ps = d + r.u32(d + 0x17C)? as usize;
        if ps + 0x1C > end {
            return Err("path struct outside the chunk".into());
        }
        let n = r.u32(ps)? as usize;
        let pts = d + r.u32(ps + 8)? as usize;
        let dts = d + r.u32(ps + 0xC)? as usize;
        if n < 2 || pts + n * 12 > end || dts + n * 12 > end {
            return Err(format!("path of {n} points outside the chunk"));
        }
        let mut points = Vec::with_capacity(n);
        let mut deltas = Vec::with_capacity(n);
        for i in 0..n {
            points.push(r.fs(pts + 12 * i)?);
            deltas.push(r.fs(dts + 12 * i)?);
        }
        Some(FxPath {
            mode: r.u32(d + 0x170)?,
            length: r.f32(ps + 4)?,
            points,
            deltas,
            origin: r.fs(ps + 0x10)?,
        })
    } else {
        None
    };

    Ok(RawEmitter {
        emitter: FxEmitter {
            name: name.to_owned(),
            props,
            frames,
            forces,
            shader,
            track,
            path,
            sub_emitter: None,
        },
        ref_name,
    })
}

/// The shader by the key of shader asset 0 and the custom data of type 10 (vf_06 0x088d0af4).
fn shader_of(
    key: Option<&str>,
    data: Option<&[u8]>,
    texture: &dyn Fn(&str) -> Option<LogicalId>,
    notes: &mut PsbNotes,
) -> FxShader {
    let (Some(key), Some(data)) = (key, data) else {
        return FxShader::Default;
    };
    let f = |i: usize| {
        data.get(4 * i..4 * i + 4)
            .map(|s| f32::from_le_bytes([s[0], s[1], s[2], s[3]]))
            .unwrap_or(0.0)
    };
    match key.to_ascii_lowercase().as_str() {
        // `Glow.txt` builds the Brightness shader too (the Glow constructor is never called).
        "glow.txt" | "brightness.txt" => FxShader::Brightness(f(0)),
        "mask.txt" => FxShader::Mask([f(0), f(1), f(2), f(3)]),
        "fakevolume.txt" => {
            let path: String = data
                .get(4..)
                .unwrap_or_default()
                .iter()
                .take_while(|c| **c != 0)
                .map(|c| char::from(*c))
                .collect();
            let file = file_name(&path);
            let gradient = texture(file);
            if gradient.is_none() && !file.is_empty() {
                notes.missing_textures.push(file.to_owned());
            }
            FxShader::FakeVolume { gradient }
        }
        _ => FxShader::Default,
    }
}

/// A track at `t` (frkTRKPrepare 0x08ff9d78): flags +0x20 (bit 0 loop), fps +0x24, key count
/// +0x28, key pointer array +0x2C (offsets relative to the track).
fn read_track(r: &Reader<'_>, t: usize, end: usize) -> Result<FxTrack, String> {
    if t + 0x40 > end {
        return Err("track header outside the chunk".into());
    }
    let flags = r.u32(t + 0x20)?;
    let fps = r.f32(t + 0x24)?;
    let n = r.u32(t + 0x28)? as usize;
    let arr = t + r.u32(t + 0x2C)? as usize;
    if n == 0 || arr + 4 * n > end {
        return Err(format!("track of {n} keys outside the chunk"));
    }
    let mut keys = Vec::with_capacity(n);
    for i in 0..n {
        let k = t + r.u32(arr + 4 * i)? as usize;
        if k + KEY_LEN > end {
            return Err(format!("track key {i} outside the chunk"));
        }
        keys.push((r.u32(k)?, read_props(r.b, k + 4)?));
    }
    if keys.windows(2).any(|w| w[1].0 < w[0].0) {
        return Err("track frames not ascending".into());
    }
    Ok(FxTrack {
        fps,
        looped: flags & 1 != 0,
        keys,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A file of one emitter chunk with a texture, a force and FakeVolume custom data, built the
    /// way the table says.
    pub fn sample() -> Vec<u8> {
        let mut prop = vec![0u8; PROPERTY_LEN];
        let put =
            |p: &mut Vec<u8>, at: usize, v: u32| p[at..at + 4].copy_from_slice(&v.to_le_bytes());
        let putf = |p: &mut Vec<u8>, at: usize, v: f32| {
            p[at..at + 4].copy_from_slice(&v.to_bits().to_le_bytes())
        };
        putf(&mut prop, 0x10 + 12, 0.4);
        putf(&mut prop, 0x58, 1.0);
        putf(&mut prop, 0x5C, 2.0);
        putf(&mut prop, 0xCC, 5.0);
        prop[0xE8] = 8;
        prop[0xE9] = 1;
        put(&mut prop, 0x10C, 1);
        for i in 0..4 {
            putf(&mut prop, 0x1A8 + 20 * i, 1.0);
        }
        // Blocks after the property: texture asset, force, shader asset, custom data.
        let mut blocks = Vec::new();
        let base = PROPERTY_LEN;
        let mut asset = vec![0u8; ASSET_LEN];
        asset[0] = 1;
        asset[4..4 + 15].copy_from_slice(b"FX_Flash_01.dds");
        for (i, v) in [0.0f32, 0.0, 1.0, 1.0].iter().enumerate() {
            asset[0x108 + 4 * i..0x10C + 4 * i].copy_from_slice(&v.to_le_bytes());
        }
        put(&mut prop, 0x124, 1);
        put(&mut prop, 0x12C, (base + blocks.len()) as u32);
        blocks.extend_from_slice(&asset);
        let mut force = vec![0u8; FORCE_LEN];
        force[0x14..0x18].copy_from_slice(&4.0f32.to_le_bytes());
        put(&mut prop, 0x114, 1);
        put(&mut prop, 0x118, (base + blocks.len()) as u32);
        blocks.extend_from_slice(&force);
        let mut shader = vec![0u8; ASSET_LEN];
        shader[0] = 1;
        shader[4..4 + 14].copy_from_slice(b"FakeVolume.txt");
        put(&mut prop, 0x180, 1);
        put(&mut prop, 0x184, (base + blocks.len()) as u32);
        blocks.extend_from_slice(&shader);
        let path = b"D:\\lib\\FX_gradient_WtOrYl.dds\0";
        let mut custom = vec![0u8; CUSTOM_HEADER_LEN];
        custom[0x20..0x24].copy_from_slice(&10u32.to_le_bytes());
        custom[0x24..0x28].copy_from_slice(&((4 + path.len()) as u32).to_le_bytes());
        custom.extend_from_slice(&[0; 4]);
        custom.extend_from_slice(path);
        put(&mut prop, 0x11C, custom.len() as u32);
        put(&mut prop, 0x120, (base + blocks.len()) as u32);
        blocks.extend_from_slice(&custom);

        let mut chunk = vec![0u8; CHUNK_HEADER_LEN];
        chunk[..6].copy_from_slice(b"Glow01");
        let size = CHUNK_HEADER_LEN + PROPERTY_LEN + blocks.len();
        chunk[0x100..0x104].copy_from_slice(&(size as u32).to_le_bytes());
        chunk[0x104..0x108].copy_from_slice(&(CHUNK_HEADER_LEN as u32).to_le_bytes());
        chunk[0x109] = 1;
        let mut file = vec![0u8; 0x10];
        file[0x0C] = 0x10;
        file.extend_from_slice(&chunk);
        file.extend_from_slice(&prop);
        file.extend_from_slice(&blocks);
        file
    }

    fn tex(f: &str) -> Option<LogicalId> {
        (f != "missing.dds").then(|| LogicalId::new(format!("tx:{}", f.to_ascii_lowercase())))
    }

    #[test]
    fn a_built_file_reads_to_the_end() {
        let (fx, notes) = read_psb("t", &sample(), &tex).unwrap();
        assert_eq!(fx.emitters.len(), 1);
        let e = &fx.emitters[0];
        assert_eq!(e.name, "Glow01");
        assert_eq!(e.props.life, [1.0, 2.0]);
        assert_eq!(e.props.volume_kind, 8);
        assert_eq!(e.props.alpha, 1);
        assert_eq!(
            e.frames[0].texture.as_ref().unwrap().as_str(),
            "tx:fx_flash_01.dds"
        );
        assert_eq!(e.forces[0].kind, 4);
        assert_eq!(
            e.shader,
            FxShader::FakeVolume {
                gradient: Some(LogicalId::new("tx:fx_gradient_wtoryl.dds"))
            }
        );
        assert_eq!(notes, PsbNotes::default());
    }

    #[test]
    fn leftovers_and_bad_blocks_are_errors() {
        let mut long = sample();
        long.push(0);
        assert!(
            read_psb("t", &long, &tex)
                .unwrap_err()
                .contains("after the last chunk")
        );
        let good = sample();
        for cut in [0x0C, 0x20, 0x150, good.len() - 1] {
            assert!(read_psb("t", &good[..cut], &tex).is_err(), "cut {cut}");
        }
        let mut far = sample();
        let d = 0x10 + CHUNK_HEADER_LEN;
        far[d + 0x12C..d + 0x130].copy_from_slice(&0x10_000u32.to_le_bytes());
        assert!(
            read_psb("t", &far, &tex)
                .unwrap_err()
                .contains("outside the chunk")
        );
        let mut sum = sample();
        let n = u32::from_le_bytes(sum[d + 0x11C..d + 0x120].try_into().unwrap());
        sum[d + 0x11C..d + 0x120].copy_from_slice(&(n - 1).to_le_bytes());
        assert!(read_psb("t", &sum, &tex).is_err());
    }

    /// The 360 files of the active set (`assets-b2` 4.12): every one reads to the end. Runs when
    /// `CIV5_PSB_DIR` names a folder of `.psb` files (e.g. one filled by `civ5-assets extract --name "*.psb" --out <dir>`).
    #[test]
    fn every_sample_reads_without_leftovers() {
        let Ok(dir) = std::env::var("CIV5_PSB_DIR") else {
            eprintln!("SKIP: CIV5_PSB_DIR is not set");
            return;
        };
        let any = |_: &str| Some(LogicalId::new("tx:any"));
        let mut n = 0;
        let mut emitters = 0;
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("psb")) {
                continue;
            }
            let b = std::fs::read(&p).unwrap();
            let (fx, _) =
                read_psb("s", &b, &any).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            let back = FxEffect::decode(&fx.encode()).unwrap();
            assert_eq!(back, fx);
            emitters += fx.emitters.len();
            n += 1;
        }
        eprintln!("{n} files, {emitters} emitters");
        assert!(n > 0);
    }
}
