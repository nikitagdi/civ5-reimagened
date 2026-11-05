//! `.lean` files of the original (the codec question RW-27 is
//! answered: there is none).
//!
//! A `.lean` file is the output of the original's `LeanMaker` tool: a 36 byte header and two raw
//! mip chains, the two textures of a LEAN map written one after the other. The first chain has
//! `4 * N` bytes per texel (`A16B16G16R16` for `N = 2`, `A8R8G8B8`-order bytes R, G, B, A for
//! `N = 1`): the mean slopes and the first two moments; the second has `N` bytes per texel (`L16`
//! or `L8`): the cross moment. Nothing is compressed. The proof is in the install: `riverrough_new16_1.dds`
//! and `riverrough_new16_2.dds` hold the same two chains as the payload of `riverrough_new16.lean`,
//! byte for byte (the real-data test checks it).
//!
//! ```text
//!  0  u32  "LEAN"
//!  4  f32  output scale (`-output_scale` of LeanMaker: 0.7, 0.33, 2.5, ...)
//!  8  u32  width
//! 12  u32  height
//! 16  u32  mip levels (the full chain)
//! 20  u32  N, bytes per channel (1 or 2)
//! 24  u32  36: the offset of the first chain
//! 28  u32  the offset of the second chain
//! 32  u32  low byte 1; the other three are the 0xCC fill of uninitialised memory and are dropped
//! ```
//!
//! The output is what the client samples without a calculation (AB-D17): the first chain as
//! `Rgba16F` and the second as `R16F`, each with all its levels. The values are the unorm values
//! of the file (`u / 65535` or `u / 255`), the same a D3D sampler returns, so the shader reads
//! them as the original does. Half floats keep 11 bits, the file has 16: the error is at most
//! 2^-12 (`HALF_ERROR`).

use asset_format::{TextureBlob, TextureFormat, f32_to_f16_bits};

pub const LEAN_MAGIC: [u8; 4] = *b"LEAN";
pub const LEAN_HEADER_BYTES: usize = 36;

/// Largest error of a value after the conversion to half float, for values in 0..=1.
pub const HALF_ERROR: f32 = 1.0 / 4096.0;

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LeanError {
    #[error("not a .lean file: {0}")]
    Invalid(String),
}

fn bad<T>(msg: impl Into<String>) -> Result<T, LeanError> {
    Err(LeanError::Invalid(msg.into()))
}

/// The header of a `.lean` file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LeanHeader {
    pub scale: f32,
    pub width: u32,
    pub height: u32,
    pub mips: u32,
    /// `N`: 1 for 8 bit files, 2 for 16 bit.
    pub bytes_per_channel: u32,
    /// Low byte of the last header word.
    pub flag: u8,
}

impl LeanHeader {
    /// Texels of the whole chain.
    fn chain_texels(&self) -> u64 {
        (0..self.mips)
            .map(|l| u64::from((self.width >> l).max(1)) * u64::from((self.height >> l).max(1)))
            .sum()
    }

    /// Bytes of the first and the second chain.
    pub fn block_bytes(&self) -> (u64, u64) {
        let t = self.chain_texels();
        let n = u64::from(self.bytes_per_channel);
        (t * 4 * n, t * n)
    }
}

/// A parsed file: the header and the two chains, level by level.
#[derive(Debug, Clone, PartialEq)]
pub struct Lean {
    pub header: LeanHeader,
    /// Level `l` of the first chain: `4 * N` bytes per texel, R, G, B, A.
    pub slopes: Vec<Vec<u8>>,
    /// Level `l` of the second chain: `N` bytes per texel.
    pub cross: Vec<Vec<u8>>,
}

fn word(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

pub fn parse_lean(bytes: &[u8]) -> Result<Lean, LeanError> {
    if bytes.len() < LEAN_HEADER_BYTES {
        return bad(format!("{} bytes, shorter than the header", bytes.len()));
    }
    if bytes[..4] != LEAN_MAGIC {
        return bad("the magic is not LEAN");
    }
    let header = LeanHeader {
        scale: f32::from_bits(word(bytes, 4)),
        width: word(bytes, 8),
        height: word(bytes, 12),
        mips: word(bytes, 16),
        bytes_per_channel: word(bytes, 20),
        flag: bytes[32],
    };
    if !header.scale.is_finite() || header.scale <= 0.0 {
        return bad(format!(
            "the scale {} is not a positive number",
            header.scale
        ));
    }
    if !matches!(header.bytes_per_channel, 1 | 2) {
        return bad(format!(
            "{} bytes per channel, expected 1 or 2",
            header.bytes_per_channel
        ));
    }
    if header.width == 0 || header.height == 0 || header.width > 16384 || header.height > 16384 {
        return bad(format!("size {}x{}", header.width, header.height));
    }
    let full = 32 - header.width.max(header.height).leading_zeros();
    if header.mips != full {
        return bad(format!(
            "{} mip levels, the full chain of {}x{} has {full}",
            header.mips, header.width, header.height
        ));
    }
    if header.flag != 1 {
        return bad(format!("the flag byte is {}, expected 1", header.flag));
    }
    let first = word(bytes, 24) as usize;
    if first != LEAN_HEADER_BYTES {
        return bad(format!(
            "the first chain starts at {first}, expected {LEAN_HEADER_BYTES}"
        ));
    }
    let (b1, b2) = header.block_bytes();
    let second = word(bytes, 28) as u64;
    if second != LEAN_HEADER_BYTES as u64 + b1 {
        return bad(format!(
            "the second chain starts at {second}, expected {}",
            LEAN_HEADER_BYTES as u64 + b1
        ));
    }
    if bytes.len() as u64 != second + b2 {
        return bad(format!(
            "{} bytes, the two chains need {}",
            bytes.len(),
            second + b2
        ));
    }
    let split = |from: usize, per_texel: usize| -> Vec<Vec<u8>> {
        let mut at = from;
        (0..header.mips)
            .map(|l| {
                let n = (header.width >> l).max(1) as usize
                    * (header.height >> l).max(1) as usize
                    * per_texel;
                let level = bytes[at..at + n].to_vec();
                at += n;
                level
            })
            .collect()
    };
    let n = header.bytes_per_channel as usize;
    Ok(Lean {
        header,
        slopes: split(LEAN_HEADER_BYTES, 4 * n),
        cross: split(second as usize, n),
    })
}

/// The unorm values of `bytes` (`n` bytes each, little endian) as half floats, little endian.
fn unorm_to_half(bytes: &[u8], n: usize) -> Vec<u8> {
    let max = if n == 1 { 255.0 } else { 65535.0 };
    let mut out = Vec::with_capacity(bytes.len() / n * 2);
    for v in bytes.chunks_exact(n) {
        let u = if n == 1 {
            u32::from(v[0])
        } else {
            u32::from(u16::from_le_bytes([v[0], v[1]]))
        };
        out.extend_from_slice(&f32_to_f16_bits(u as f32 / max).to_le_bytes());
    }
    out
}

/// The two textures of a LEAN map, ready for the client.
#[derive(Debug, Clone, PartialEq)]
pub struct LeanTextures {
    pub header: LeanHeader,
    /// Mean slopes and moments: `Rgba16F`, every level.
    pub slopes: TextureBlob,
    /// Cross moment: `R16F`, every level.
    pub cross: TextureBlob,
}

pub fn lean_textures(bytes: &[u8]) -> Result<LeanTextures, LeanError> {
    let lean = parse_lean(bytes)?;
    let h = lean.header;
    let n = h.bytes_per_channel as usize;
    let convert = |levels: &[Vec<u8>]| -> Vec<Vec<u8>> {
        levels.iter().map(|l| unorm_to_half(l, n)).collect()
    };
    Ok(LeanTextures {
        header: h,
        slopes: TextureBlob::new_2d(
            TextureFormat::Rgba16F,
            false,
            h.width,
            h.height,
            convert(&lean.slopes),
        ),
        cross: TextureBlob::new_2d(
            TextureFormat::R16F,
            false,
            h.width,
            h.height,
            convert(&lean.cross),
        ),
    })
}

#[cfg(test)]
mod tests {
    use asset_format::f16_bits_to_f32;

    use super::*;

    /// A `.lean` file of `w`x`h` with the full chain; the texel bytes count up.
    pub(crate) fn sample(w: u32, h: u32, n: u32, scale: f32) -> Vec<u8> {
        let mips = 32 - w.max(h).leading_zeros();
        let head = LeanHeader {
            scale,
            width: w,
            height: h,
            mips,
            bytes_per_channel: n,
            flag: 1,
        };
        let (b1, b2) = head.block_bytes();
        let mut out = Vec::new();
        out.extend_from_slice(&LEAN_MAGIC);
        out.extend_from_slice(&scale.to_le_bytes());
        for v in [w, h, mips, n, 36, 36 + b1 as u32] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        // The last word: flag 1 and the 0xCC fill of uninitialised memory above it.
        out.extend_from_slice(&0xcccc_cc01u32.to_le_bytes());
        out.extend((0..b1 + b2).map(|i| (i * 7 + 3) as u8));
        out
    }

    #[test]
    fn parses_the_header_and_splits_the_chains() {
        let f = sample(8, 2, 2, 0.7);
        let l = parse_lean(&f).unwrap();
        assert_eq!((l.header.width, l.header.height, l.header.mips), (8, 2, 4));
        assert_eq!(l.header.scale, 0.7);
        let sizes: Vec<usize> = l.slopes.iter().map(Vec::len).collect();
        assert_eq!(sizes, [8 * 2 * 8, 4 * 8, 2 * 8, 8]);
        let sizes: Vec<usize> = l.cross.iter().map(Vec::len).collect();
        assert_eq!(sizes, [8 * 2 * 2, 4 * 2, 2 * 2, 2]);
        // The chains are the file's bytes, in order.
        let all: Vec<u8> = l.slopes.iter().chain(&l.cross).flatten().copied().collect();
        assert_eq!(all, f[36..]);
    }

    #[test]
    fn values_become_half_floats_within_the_error() {
        for n in [1u32, 2] {
            let f = sample(4, 4, n, 1.0);
            let t = lean_textures(&f).unwrap();
            assert_eq!(t.slopes.format, TextureFormat::Rgba16F);
            assert_eq!(t.cross.format, TextureFormat::R16F);
            assert_eq!((t.slopes.mips.len(), t.cross.mips.len()), (3, 3));
            let l = parse_lean(&f).unwrap();
            let max = if n == 1 { 255.0 } else { 65535.0 };
            let mut worst = 0f32;
            for (src, dst) in l.slopes.iter().zip(&t.slopes.mips) {
                assert_eq!(dst.len(), src.len() / n as usize * 2);
                for (v, h) in src.chunks_exact(n as usize).zip(dst.chunks_exact(2)) {
                    let u = if n == 1 {
                        f32::from(v[0])
                    } else {
                        f32::from(u16::from_le_bytes([v[0], v[1]]))
                    };
                    let got = f16_bits_to_f32(u16::from_le_bytes([h[0], h[1]]));
                    worst = worst.max((got - u / max).abs());
                }
            }
            assert!(worst <= HALF_ERROR + 1e-7, "n={n}: {worst}");
            // The blobs are valid and survive their own encoding.
            assert_eq!(TextureBlob::decode(&t.slopes.encode()).unwrap(), t.slopes);
            assert_eq!(TextureBlob::decode(&t.cross.encode()).unwrap(), t.cross);
        }
    }

    #[test]
    fn the_fill_bytes_of_the_flag_word_do_not_matter() {
        let a = sample(4, 4, 2, 1.0);
        let mut b = a.clone();
        b[33..36].copy_from_slice(&[0, 0, 0]);
        assert_eq!(parse_lean(&a).unwrap(), parse_lean(&b).unwrap());
    }

    #[test]
    fn damaged_files_are_refused() {
        let good = sample(8, 8, 2, 0.5);
        assert!(parse_lean(&good).is_ok());
        let patched = |at: usize, v: u32| {
            let mut f = good.clone();
            f[at..at + 4].copy_from_slice(&v.to_le_bytes());
            f
        };
        let mut cases = vec![
            ("short", good[..20].to_vec()),
            ("tail", [good.clone(), vec![0]].concat()),
            ("cut", good[..good.len() - 1].to_vec()),
            ("mips", patched(16, 3)),
            ("channels", patched(20, 3)),
            ("first offset", patched(24, 40)),
            ("second offset", patched(28, 100)),
            ("flag", patched(32, 0xcccc_cc00)),
            ("size", patched(8, 0)),
            ("scale", patched(4, f32::NAN.to_bits())),
        ];
        let mut magic = good.clone();
        magic[0] = b'X';
        cases.push(("magic", magic));
        for (what, f) in cases {
            assert!(parse_lean(&f).is_err(), "{what}");
        }
    }
}
