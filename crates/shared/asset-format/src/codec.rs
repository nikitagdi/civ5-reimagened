//! Compressed byte sections shared by the packed blob formats.
//!
//! A [`Section`] holds `raw_len` bytes of data in one of the [`Codec`]s. `Raw`, `Const` and `Zstd`
//! are decoded here (`Zstd` through `ruzstd`, which is pure Rust and builds for wasm32; the packer
//! in `asset-pipeline` writes it with `zstd`). `Deflate` is part of the type so blobs can name it
//! if `ruzstd` ever fails on wasm (AB-D3); nothing writes or reads it.

use ruzstd::decoding::FrameDecoder;
use serde::{Deserialize, Serialize};

use crate::FormatError;

/// Upper bound for one decoded section; keeps size arithmetic and allocation sane on wasm32.
pub const MAX_SECTION_LEN: usize = 1 << 28;

/// Width of the elements the byte shuffle separates (u16: low bytes, then high bytes).
const SHUFFLE_WIDTH: usize = 2;

/// Postcard writes the variant index, so new codecs go at the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Codec {
    /// `data` is the bytes.
    Raw,
    /// `data` is a pattern repeated to fill `raw_len` (a flat plane is one element).
    Const,
    Zstd,
    Deflate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Section {
    pub codec: Codec,
    /// The bytes were permuted before coding: all low bytes of the u16 elements, then all
    /// high bytes. Undone after decoding. Never set for `Const`.
    pub shuffle: bool,
    /// Length after decoding and unshuffling.
    pub raw_len: u32,
    pub data: Vec<u8>,
}

impl Section {
    pub fn raw(data: Vec<u8>) -> Result<Self, FormatError> {
        let raw_len = u32::try_from(data.len())
            .map_err(|_| FormatError::invalid("section larger than 4 GiB"))?;
        Ok(Self {
            codec: Codec::Raw,
            shuffle: false,
            raw_len,
            data,
        })
    }

    /// A section of `raw_len` bytes made of `pattern` repeated.
    pub fn constant(pattern: &[u8], raw_len: u32) -> Self {
        Self {
            codec: Codec::Const,
            shuffle: false,
            raw_len,
            data: pattern.to_vec(),
        }
    }

    /// Structural checks that need no decoding: the lengths are consistent and bounded.
    pub fn validate(&self) -> Result<(), FormatError> {
        let raw_len = self.raw_len as usize;
        if raw_len > MAX_SECTION_LEN {
            return Err(FormatError::invalid("section longer than the limit"));
        }
        if self.shuffle && !raw_len.is_multiple_of(SHUFFLE_WIDTH) {
            return Err(FormatError::invalid("shuffled section has an odd length"));
        }
        match self.codec {
            Codec::Raw => {
                if self.shuffle {
                    // Shuffle only pays off before a coder; a raw section is stored in place.
                    return Err(FormatError::invalid("raw section cannot be shuffled"));
                }
                if self.data.len() != raw_len {
                    return Err(FormatError::invalid("raw section length mismatch"));
                }
            }
            Codec::Const => {
                if self.shuffle {
                    return Err(FormatError::invalid("const section cannot be shuffled"));
                }
                if self.data.is_empty() && raw_len != 0
                    || !self.data.is_empty() && !raw_len.is_multiple_of(self.data.len())
                {
                    return Err(FormatError::invalid(
                        "const pattern does not tile the section",
                    ));
                }
            }
            Codec::Zstd | Codec::Deflate => {}
        }
        Ok(())
    }

    /// The decoded bytes.
    pub fn decode(&self) -> Result<Vec<u8>, FormatError> {
        self.validate()?;
        let raw_len = self.raw_len as usize;
        match self.codec {
            Codec::Raw => Ok(self.data.clone()),
            Codec::Const => {
                let mut out = Vec::with_capacity(raw_len);
                if !self.data.is_empty() {
                    for _ in 0..raw_len / self.data.len() {
                        out.extend_from_slice(&self.data);
                    }
                }
                Ok(out)
            }
            Codec::Zstd => {
                let out = unzstd(&self.data, raw_len)?;
                Ok(if self.shuffle {
                    unshuffle_u16(&out)
                } else {
                    out
                })
            }
            Codec::Deflate => Err(FormatError::invalid(
                "Deflate sections are not decoded by this build",
            )),
        }
    }
}

/// Smallest window the decoder accepts, whatever the frame header claims.
const MIN_WINDOW: u64 = 1 << 22;

/// One zstd frame to exactly `raw_len` bytes. The output vector has `raw_len` as its capacity and
/// the decoder never grows it, so a frame that claims more than the section says is an error before
/// it can allocate; the window is capped at twice the output (and not below [`MIN_WINDOW`]).
fn unzstd(frame: &[u8], raw_len: usize) -> Result<Vec<u8>, FormatError> {
    let mut decoder = FrameDecoder::new();
    decoder.set_max_window_size((raw_len as u64).saturating_mul(2).max(MIN_WINDOW));
    let mut out = Vec::with_capacity(raw_len);
    decoder
        .decode_all_to_vec(frame, &mut out)
        .map_err(|e| FormatError::invalid(format!("zstd section: {e}")))?;
    if out.len() != raw_len {
        return Err(FormatError::invalid(format!(
            "zstd section decodes to {} bytes, the section says {raw_len}",
            out.len()
        )));
    }
    if let (Some(stored), Some(calc)) = (
        decoder.get_checksum_from_data(),
        decoder.get_calculated_checksum(),
    ) && stored != calc
    {
        return Err(FormatError::invalid("zstd section: checksum mismatch"));
    }
    Ok(out)
}

/// Splits u16 elements into low bytes then high bytes (the inverse of [`unshuffle_u16`]).
/// The length must be even.
pub fn shuffle_u16(data: &[u8]) -> Vec<u8> {
    debug_assert!(data.len().is_multiple_of(SHUFFLE_WIDTH));
    let n = data.len() / SHUFFLE_WIDTH;
    let mut out = vec![0u8; data.len()];
    let (lo, hi) = out.split_at_mut(n);
    for (i, pair) in data.chunks_exact(SHUFFLE_WIDTH).enumerate() {
        lo[i] = pair[0];
        hi[i] = pair[1];
    }
    out
}

pub fn unshuffle_u16(data: &[u8]) -> Vec<u8> {
    debug_assert!(data.len().is_multiple_of(SHUFFLE_WIDTH));
    let n = data.len() / SHUFFLE_WIDTH;
    let (lo, hi) = data.split_at(n);
    let mut out = Vec::with_capacity(data.len());
    for i in 0..n {
        out.push(lo[i]);
        out.push(hi[i]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_and_const_roundtrip() {
        let s = Section::raw(vec![1, 2, 3]).unwrap();
        assert_eq!(s.decode().unwrap(), [1, 2, 3]);
        let c = Section::constant(&[0x34, 0x12], 6);
        assert_eq!(c.decode().unwrap(), [0x34, 0x12, 0x34, 0x12, 0x34, 0x12]);
        assert_eq!(
            Section::constant(&[7], 0).decode().unwrap(),
            Vec::<u8>::new()
        );
    }

    #[test]
    fn inconsistent_sections_are_errors() {
        let mut s = Section::raw(vec![1, 2, 3]).unwrap();
        s.raw_len = 4;
        assert!(s.decode().is_err());
        assert!(Section::constant(&[1, 2], 3).decode().is_err());
        assert!(Section::constant(&[], 3).decode().is_err());
        // A tiny blob must not ask for a huge allocation.
        assert!(Section::constant(&[1], u32::MAX).decode().is_err());
        let mut z = Section::raw(vec![0; 4]).unwrap();
        z.codec = Codec::Zstd;
        assert!(z.decode().is_err());
    }

    #[test]
    fn shuffle_is_invertible() {
        let data: Vec<u8> = (0..10).collect();
        let s = shuffle_u16(&data);
        assert_eq!(s, [0, 2, 4, 6, 8, 1, 3, 5, 7, 9]);
        assert_eq!(unshuffle_u16(&s), data);
    }

    fn zstd_section(raw: &[u8], shuffle: bool) -> Section {
        use ruzstd::encoding::{CompressionLevel, compress_to_vec};
        let src = if shuffle {
            shuffle_u16(raw)
        } else {
            raw.to_vec()
        };
        Section {
            codec: Codec::Zstd,
            shuffle,
            raw_len: raw.len() as u32,
            data: compress_to_vec(&src[..], CompressionLevel::Fastest),
        }
    }

    #[test]
    fn zstd_sections_decode_with_and_without_shuffle() {
        // Smooth u16 data, the kind the terrain heights are.
        let raw: Vec<u8> = (0..4096u32)
            .flat_map(|i| ((i * 3 + 1000) as u16).to_le_bytes())
            .collect();
        for shuffle in [false, true] {
            let s = zstd_section(&raw, shuffle);
            assert!(s.data.len() < raw.len() || !shuffle, "it compresses");
            assert_eq!(s.decode().unwrap(), raw, "shuffle {shuffle}");
        }
    }

    #[test]
    fn a_zstd_section_that_lies_about_its_size_or_holds_garbage_is_an_error() {
        let raw = vec![7u8; 1000];
        let mut s = zstd_section(&raw, false);
        s.raw_len = 999;
        assert!(s.decode().is_err(), "output longer than the section says");
        s.raw_len = 1001;
        assert!(s.decode().is_err(), "output shorter than the section says");
        let mut g = zstd_section(&raw, false);
        g.data = vec![0x28, 0xb5, 0x2f, 0xfd, 1, 2, 3];
        assert!(g.decode().is_err());
        g.data.clear();
        assert!(g.decode().is_err());
        // A shuffled section needs an even length.
        let mut odd = zstd_section(&raw[..10], true);
        odd.raw_len = 9;
        assert!(odd.decode().is_err());
        // A tiny frame must not make the decoder allocate a huge output.
        let mut huge = zstd_section(&raw, false);
        huge.raw_len = u32::MAX;
        assert!(huge.decode().is_err());
    }

    #[test]
    fn section_postcard_roundtrip() {
        let s = Section::constant(&[9, 9], 8);
        let bytes = postcard::to_allocvec(&s).unwrap();
        assert_eq!(postcard::from_bytes::<Section>(&bytes).unwrap(), s);
    }
}
