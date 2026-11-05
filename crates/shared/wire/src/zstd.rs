//! zstd compression over `ruzstd` (pure Rust: unpack in wasm, pack on the server). net-protocol NP-D9, 4.13.
//!
//! A packed block is `[raw length: u32 LE][plain ruzstd frame]`. The length is ours, not the zstd
//! content-size field (ruzstd does not write it, and we do not rewrite zstd headers). The reader checks
//! the length against its limit before allocating and decodes with a hard cap of exactly that length.

use crate::WireError;
use ruzstd::decoding::FrameDecoder;
use ruzstd::encoding::{CompressionLevel, compress_to_vec};

/// Level used for packing. ruzstd 0.9 implements only `Uncompressed` and `Fastest` (about zstd level 1);
/// `Default`, `Better` and `Best` are in the enum but panic with `unimplemented!()`, never use them.
const LEVEL: CompressionLevel = CompressionLevel::Fastest;

/// Window the decoder may allocate, regardless of what a frame header claims.
const MIN_WINDOW_LIMIT: u64 = 1 << 20;

/// Packs `raw` into `[len u32 LE][zstd frame]`. `raw` must be at most `u32::MAX` bytes.
pub fn compress(raw: &[u8]) -> Vec<u8> {
    compress_with(raw, |raw| compress_to_vec(raw, LEVEL))
}

/// [`compress`] with the encoder given: `frame_of` makes any standard zstd frame of `raw` (the C library at a
/// higher level, say). [`decompress`] reads the result, whoever made the frame.
pub fn compress_with(raw: &[u8], frame_of: impl FnOnce(&[u8]) -> Vec<u8>) -> Vec<u8> {
    let frame = frame_of(raw);
    let mut out = Vec::with_capacity(frame.len() + 4);
    out.extend_from_slice(&(raw.len() as u32).to_le_bytes());
    out.extend_from_slice(&frame);
    out
}

/// Unpacks `[len u32 LE][zstd frame]`. `len` above `max_raw` is an error before any decoding; output
/// longer or shorter than `len` is an error.
pub fn decompress(z: &[u8], max_raw: usize) -> Result<Vec<u8>, WireError> {
    let (head, frame) = z
        .split_first_chunk::<4>()
        .ok_or_else(|| WireError::Zstd("missing length prefix".into()))?;
    let declared = u32::from_le_bytes(*head) as usize;
    if declared > max_raw {
        return Err(WireError::RawTooLarge {
            declared: declared as u64,
            max: max_raw,
        });
    }
    let mut decoder = FrameDecoder::new();
    decoder.set_max_window_size((max_raw as u64).max(MIN_WINDOW_LIMIT));
    // Capacity is the hard cap: `decode_all_to_vec` never grows the vector.
    let mut out = Vec::with_capacity(declared);
    decoder
        .decode_all_to_vec(frame, &mut out)
        .map_err(|e| WireError::Zstd(e.to_string()))?;
    if out.len() != declared {
        return Err(WireError::SizeMismatch {
            declared,
            actual: out.len(),
        });
    }
    if let (Some(stored), Some(calc)) = (
        decoder.get_checksum_from_data(),
        decoder.get_calculated_checksum(),
    ) && stored != calc
    {
        return Err(WireError::Zstd("checksum mismatch".into()));
    }
    Ok(out)
}
