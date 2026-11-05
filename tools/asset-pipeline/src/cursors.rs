//! The animated mouse cursors of the install (`.ani`, RIFF `ACON`, in `Assets/UI/Cursors`) as one
//! [`CursorSet`]: per cursor the 32x32 RGBA frames in the order of the animation's sequence (a step
//! that lasts longer repeats its picture, so every frame shows for the same time), the hot spot of
//! the first frame, and the frame time.
//!
//! The cursors are the rows of the game's `Cursors` table (`CIV5Cursors.xml`) that the client maps
//! interface modes to (`client-app/src/cursor.rs`); `NAMES` pairs the client's name with the file.

use asset_format::{CursorArt, CursorSet};

/// Side of every cursor frame of the game.
pub const SIZE: u16 = 32;

/// The client's name of each cursor and the stem of its `.ani` file.
pub const NAMES: &[(&str, &str)] = &[
    ("pointer", "Pointer"),
    ("waiting", "Waiting"),
    ("ping", "Ping"),
    ("go_to", "GoTo"),
    ("route_to", "RouteTo"),
    ("airlift", "Airlift"),
    ("nuke", "Nuke"),
    ("airstrike", "Airstrike"),
    ("recon", "Recon"),
    ("paradrop", "Paradrop"),
    ("airbomb", "Airbomb"),
    ("rebase", "Rebase"),
    ("move", "Move"),
    ("link", "Link"),
    ("grip", "Grip"),
    ("edit", "Edit"),
    ("claim", "Claim"),
    ("gift", "Gift"),
];

type R<T> = Result<T, String>;

fn u16_at(d: &[u8], p: usize) -> R<u16> {
    d.get(p..p + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
        .ok_or_else(|| format!("truncated at {p}"))
}

fn u32_at(d: &[u8], p: usize) -> R<u32> {
    d.get(p..p + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| format!("truncated at {p}"))
}

fn i32_at(d: &[u8], p: usize) -> R<i32> {
    u32_at(d, p).map(|v| v as i32)
}

#[derive(Default)]
struct Ani {
    /// `anih`: `(steps, jiffies)` (display rate in 1/60 s).
    header: Option<(u32, u32)>,
    rate: Option<Vec<u32>>,
    seq: Option<Vec<u32>>,
    frames: Vec<Vec<u8>>,
}

fn u32_list(d: &[u8], body: usize, size: usize) -> R<Vec<u32>> {
    (0..size / 4).map(|i| u32_at(d, body + i * 4)).collect()
}

fn walk(d: &[u8], mut pos: usize, end: usize, a: &mut Ani) -> R<()> {
    while pos + 8 <= end {
        let id = &d[pos..pos + 4];
        let size = u32_at(d, pos + 4)? as usize;
        let body = pos + 8;
        if body + size > d.len() {
            return Err(format!("chunk at {pos} runs past the end"));
        }
        match id {
            b"LIST" => {
                if &d[body..body + 4] == b"fram" {
                    let mut p = body + 4;
                    while p + 8 <= body + size {
                        let s2 = u32_at(d, p + 4)? as usize;
                        if &d[p..p + 4] == b"icon" {
                            a.frames.push(
                                d.get(p + 8..p + 8 + s2)
                                    .ok_or("icon runs past the end")?
                                    .to_vec(),
                            );
                        }
                        p += 8 + s2 + (s2 & 1);
                    }
                } else {
                    walk(d, body + 4, body + size, a)?;
                }
            }
            b"anih" => a.header = Some((u32_at(d, body + 8)?, u32_at(d, body + 28)?)),
            b"rate" => a.rate = Some(u32_list(d, body, size)?),
            b"seq " => a.seq = Some(u32_list(d, body, size)?),
            _ => {}
        }
        pos = body + size + (size & 1);
    }
    Ok(())
}

/// One ICO/CUR image: `(width, height, hot x, hot y, RGBA top row first)`.
fn frame_rgba(blob: &[u8]) -> R<(u32, u32, u16, u16, Vec<u8>)> {
    let hx = u16_at(blob, 10)?;
    let hy = u16_at(blob, 12)?;
    let size = u32_at(blob, 14)? as usize;
    let off = u32_at(blob, 18)? as usize;
    let dib = blob
        .get(off..off + size)
        .ok_or("image data runs past the end")?;
    let hdr = u32_at(dib, 0)? as usize;
    let bw = i32_at(dib, 4)?;
    let bh2 = i32_at(dib, 8)?;
    let bpp = u16_at(dib, 14)? as usize;
    if bw <= 0 || bh2 <= 0 || ![32, 24, 8, 4].contains(&bpp) {
        return Err(format!("unsupported image {bw}x{bh2} at {bpp} bpp"));
    }
    let (bw, bh) = (bw as usize, bh2 as usize / 2);
    let clr_used = u32_at(dib, 32)? as usize;
    let pal_n = if bpp <= 8 {
        if clr_used > 0 { clr_used } else { 1 << bpp }
    } else {
        0
    };
    let mut pal = Vec::with_capacity(pal_n);
    let mut p = hdr;
    for _ in 0..pal_n {
        let c = dib.get(p..p + 4).ok_or("palette runs past the end")?;
        pal.push([c[2], c[1], c[0]]);
        p += 4;
    }
    let row = (bw * bpp).div_ceil(32) * 4;
    let xor = dib
        .get(p..p + row * bh)
        .ok_or("colour plane runs past the end")?;
    let mrow = bw.div_ceil(32) * 4;
    let mask = dib
        .get(p + row * bh..p + row * bh + mrow * bh)
        .unwrap_or(&[]);
    let has_alpha = bpp == 32 && (0..bh).any(|y| (0..bw).any(|x| xor[y * row + x * 4 + 3] != 0));
    let px = |i: usize| pal.get(i).copied().ok_or("palette index out of range");
    let mut out = vec![0u8; bw * bh * 4];
    for y in 0..bh {
        let sy = bh - 1 - y;
        for x in 0..bw {
            let (rgb, mut a) = match bpp {
                32 => {
                    let o = sy * row + x * 4;
                    ([xor[o + 2], xor[o + 1], xor[o]], xor[o + 3])
                }
                24 => {
                    let o = sy * row + x * 3;
                    ([xor[o + 2], xor[o + 1], xor[o]], 255)
                }
                8 => (px(usize::from(xor[sy * row + x]))?, 255),
                _ => {
                    let b = xor[sy * row + x / 2];
                    let i = if x % 2 == 0 { b >> 4 } else { b & 15 };
                    (px(usize::from(i))?, 255)
                }
            };
            if !has_alpha {
                // The AND mask decides (palette images, and 32 bit images without alpha).
                let m = mask
                    .get(sy * mrow + x / 8)
                    .map_or(0, |b| (b >> (7 - x % 8)) & 1);
                a = if m == 1 { 0 } else { 255 };
            }
            let o = (y * bw + x) * 4;
            out[o..o + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], a]);
        }
    }
    Ok((bw as u32, bh as u32, hx, hy, out))
}

fn gcd(a: u32, b: u32) -> u32 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// One `.ani` file as a cursor named `name`.
pub fn convert_ani(name: &str, bytes: &[u8]) -> R<CursorArt> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"ACON" {
        return Err("not a RIFF ACON file".into());
    }
    let mut a = Ani::default();
    walk(bytes, 12, bytes.len(), &mut a)?;
    let (_, jiffies) = a.header.ok_or("no anih chunk")?;
    let frames = a
        .frames
        .iter()
        .map(|f| frame_rgba(f))
        .collect::<R<Vec<_>>>()?;
    if frames.is_empty() {
        return Err("no frames".into());
    }
    if let Some(f) = frames
        .iter()
        .find(|f| f.0 != u32::from(SIZE) || f.1 != u32::from(SIZE))
    {
        return Err(format!(
            "a frame of {}x{}, {SIZE}x{SIZE} expected",
            f.0, f.1
        ));
    }
    let seq: Vec<u32> = a.seq.unwrap_or_else(|| (0..frames.len() as u32).collect());
    let rates: Vec<u32> = a.rate.unwrap_or_else(|| vec![jiffies; seq.len()]);
    let step = rates.iter().fold(0, |g, &r| gcd(g, r)).max(1);
    let mut rgba = Vec::new();
    for (i, &s) in seq.iter().enumerate() {
        let f = frames
            .get(s as usize)
            .ok_or_else(|| format!("sequence names frame {s} of {}", frames.len()))?;
        let rate = rates.get(i).copied().unwrap_or(step);
        for _ in 0..(rate / step).max(1) {
            rgba.extend_from_slice(&f.4);
        }
    }
    Ok(CursorArt {
        name: name.to_owned(),
        hot: (frames[0].2, frames[0].3),
        frame_ms: ((f64::from(step) * 1000.0 / 60.0).round() as u16).max(1),
        rgba,
    })
}

/// The cursors of [`NAMES`]; `read(stem)` gives the bytes of `<stem>.ani` (any case) if the install
/// has it. Returns the set and the cursors that are missing or did not convert, with the reason.
pub fn build_cursor_set(
    mut read: impl FnMut(&str) -> Option<Vec<u8>>,
) -> (CursorSet, Vec<(String, String)>) {
    let mut set = CursorSet {
        size: SIZE,
        cursors: Vec::new(),
    };
    let mut failed = Vec::new();
    for (name, stem) in NAMES {
        let Some(bytes) = read(stem) else {
            failed.push((name.to_string(), format!("{stem}.ani not found")));
            continue;
        };
        match convert_ani(name, &bytes) {
            Ok(c) => set.cursors.push(c),
            Err(e) => failed.push((name.to_string(), e)),
        }
    }
    (set, failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 32x32 cursor image (CUR with one 32 bpp image) of one colour, hot spot `hot`.
    fn cur(rgb: [u8; 3], hot: (u16, u16)) -> Vec<u8> {
        let (w, h) = (32usize, 32usize);
        let mut dib = Vec::new();
        dib.extend_from_slice(&40u32.to_le_bytes());
        dib.extend_from_slice(&(w as i32).to_le_bytes());
        dib.extend_from_slice(&((h * 2) as i32).to_le_bytes());
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&32u16.to_le_bytes());
        dib.extend_from_slice(&[0u8; 24]);
        for _ in 0..w * h {
            dib.extend_from_slice(&[rgb[2], rgb[1], rgb[0], 255]);
        }
        dib.extend_from_slice(&vec![0u8; 4 * h]);
        let mut out = Vec::new();
        out.extend_from_slice(&[0, 0, 2, 0, 1, 0]);
        out.extend_from_slice(&[32, 32, 0, 0]);
        out.extend_from_slice(&hot.0.to_le_bytes());
        out.extend_from_slice(&hot.1.to_le_bytes());
        out.extend_from_slice(&(dib.len() as u32).to_le_bytes());
        out.extend_from_slice(&22u32.to_le_bytes());
        out.extend_from_slice(&dib);
        out
    }

    fn chunk(id: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut c = id.to_vec();
        c.extend_from_slice(&(body.len() as u32).to_le_bytes());
        c.extend_from_slice(body);
        if body.len() % 2 == 1 {
            c.push(0);
        }
        c
    }

    /// An `.ani` of two frames, the second shown twice as long.
    fn ani() -> Vec<u8> {
        let mut anih = Vec::new();
        for v in [36u32, 2, 2, 32, 32, 32, 1, 6, 3] {
            anih.extend_from_slice(&v.to_le_bytes());
        }
        let mut rate = Vec::new();
        for v in [6u32, 12] {
            rate.extend_from_slice(&v.to_le_bytes());
        }
        let mut fram = b"fram".to_vec();
        fram.extend(chunk(b"icon", &cur([255, 0, 0], (4, 2))));
        fram.extend(chunk(b"icon", &cur([0, 0, 255], (9, 9))));
        let mut body = b"ACON".to_vec();
        body.extend(chunk(b"anih", &anih));
        body.extend(chunk(b"rate", &rate));
        body.extend(chunk(b"LIST", &fram));
        chunk(b"RIFF", &body)
    }

    #[test]
    fn an_animated_cursor_becomes_frames_of_equal_length() {
        let c = convert_ani("pointer", &ani()).unwrap();
        assert_eq!(c.hot, (4, 2));
        assert_eq!(c.frame_ms, 100);
        let fb = 32 * 32 * 4;
        assert_eq!(c.rgba.len(), 3 * fb);
        assert_eq!(&c.rgba[..4], &[255, 0, 0, 255]);
        assert_eq!(&c.rgba[fb..fb + 4], &[0, 0, 255, 255]);
        assert_eq!(&c.rgba[2 * fb..2 * fb + 4], &[0, 0, 255, 255]);
    }

    #[test]
    fn missing_and_broken_cursors_are_listed() {
        let (set, failed) = build_cursor_set(|stem| match stem {
            "Pointer" => Some(ani()),
            "Waiting" => Some(b"RIFFxxxx".to_vec()),
            _ => None,
        });
        assert_eq!(set.cursors.len(), 1);
        assert_eq!(failed.len(), NAMES.len() - 1);
        assert!(set.validate().is_ok());
    }
}
