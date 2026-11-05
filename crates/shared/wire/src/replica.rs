//! The body of `GET /replica/{hash:016x}`: a GCS1 snapshot (or a patch from one) of the
//! replica game. Layout, all integers little-endian:
//!
//! `C5RB` | kind u8 (0 snapshot, 1 patch) | GCS1 length u32 | `raw_hash` u64 | `from_raw_hash` u64 (patch only)
//! | one zstd frame (ruzstd) of the GCS1 bytes (of the patch: the frame against the base snapshot)
//!
//! `raw_hash` is the xxh64 (seed 0) of the unpacked GCS1 bytes. The hash that names the body in the URL is
//! the xxh64 of the whole body. One implementation for the worker, the gateway and the client.

use protocol::xxh64;
use ruzstd::decoding::{BlockDecodingStrategy, Dictionary, FrameDecoder};
use ruzstd::encoding::{CompressionLevel, compress_to_vec};

/// First bytes of the body.
pub const REPLICA_MAGIC: [u8; 4] = *b"C5RB";

/// Largest body, and largest unpacked GCS1 length, either side accepts.
pub const REPLICA_MAX: usize = 64 << 20;

/// Header length of a snapshot body; a patch has 8 more bytes.
pub const REPLICA_HEAD_SNAPSHOT: usize = 4 + 1 + 4 + 8;

/// Header of a patch body: a snapshot header and `from_raw_hash`.
pub const REPLICA_HEAD_PATCH: usize = REPLICA_HEAD_SNAPSHOT + 8;

/// The zstd dictionary a patch frame is decoded with: this constant header (magic, id,
/// entropy tables, repeat offsets 1/4/8) and, as the content, the raw bytes of the snapshot the patch is made
/// against. The server compresses with the same bytes as a raw prefix (`zstd --patch-from`, long-distance
/// matching), so the frame carries no tables of its own from the dictionary; the header only gives `ruzstd` the
/// dictionary object it needs. The id is ours (`RPL1`).
pub const PATCH_DICT_HEAD: [u8; 109] = [
    0x37, 0xA4, 0x30, 0xEC, b'R', b'P', b'L', b'1', 0x19, 0x10, 0xE0, 0x0A, 0x95, 0x03, 0xFF, 0xFF,
    0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x03, 0x40, 0x88, 0x6D, 0xBB, 0x22, 0x52, 0x56, 0xA4, 0xAC, 0x88,
    0x48, 0x19, 0xE3, 0x08, 0x00, 0x00, 0x00, 0x0C, 0xCB, 0x22, 0xA5, 0x1A, 0x00, 0x00, 0x04, 0x40,
    0x05, 0x40, 0xC2, 0x88, 0x54, 0xC2, 0x4F, 0x5E, 0x86, 0x91, 0x18, 0x0A, 0x1F, 0x11, 0x02, 0x08,
    0x01, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xD4,
    0x6E, 0x1E, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x01, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00,
];

/// What the body holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyKind {
    Snapshot,
    Patch,
}

/// A body whose header was checked; `frame` is the packed part.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BodyHeader<'a> {
    pub kind: BodyKind,
    /// Length of the unpacked GCS1.
    pub raw_len: usize,
    pub raw_hash: u64,
    /// Patch only: `raw_hash` of the snapshot the patch is made against.
    pub from_raw_hash: Option<u64>,
    pub frame: &'a [u8],
}

/// Why a body was refused. Network data never panics.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReplicaError {
    #[error("replica body: truncated")]
    Truncated,
    #[error("replica body: bad magic")]
    BadMagic,
    #[error("replica body: unknown kind {0}")]
    BadKind(u8),
    #[error("replica body of {len} bytes exceeds {max}")]
    TooLarge { len: u64, max: usize },
    #[error("replica body: zstd: {0}")]
    Zstd(String),
    #[error("replica body: declares {declared} bytes, holds {actual}")]
    SizeMismatch { declared: usize, actual: usize },
    #[error("replica body: raw hash {actual:016x}, header says {declared:016x}")]
    HashMismatch { declared: u64, actual: u64 },
    #[error("replica body: a patch needs its base snapshot")]
    PatchNotSupported,
    #[error("replica body: not a patch")]
    NotAPatch,
    #[error("replica body: the patch is made against {declared:016x}, the base is {actual:016x}")]
    WrongBase { declared: u64, actual: u64 },
}

/// Packs a snapshot (GCS1 bytes) into a body.
pub fn encode_snapshot(raw: &[u8]) -> Result<Vec<u8>, ReplicaError> {
    if raw.len() > REPLICA_MAX {
        return Err(ReplicaError::TooLarge {
            len: raw.len() as u64,
            max: REPLICA_MAX,
        });
    }
    let frame = compress_to_vec(raw, CompressionLevel::Fastest);
    let mut out = Vec::with_capacity(REPLICA_HEAD_SNAPSHOT + frame.len());
    out.extend_from_slice(&REPLICA_MAGIC);
    out.push(0);
    out.extend_from_slice(&(raw.len() as u32).to_le_bytes());
    out.extend_from_slice(&xxh64(0, raw).to_le_bytes());
    out.extend_from_slice(&frame);
    if out.len() > REPLICA_MAX {
        return Err(ReplicaError::TooLarge {
            len: out.len() as u64,
            max: REPLICA_MAX,
        });
    }
    Ok(out)
}

/// Puts a snapshot body together from a zstd frame of the GCS1 bytes made elsewhere (the server uses the C library,
/// which packs a snapshot about 20 times smaller than `ruzstd`'s encoder); [`decode_snapshot`] reads it.
pub fn snapshot_body(raw_len: usize, raw_hash: u64, frame: &[u8]) -> Result<Vec<u8>, ReplicaError> {
    let total = REPLICA_HEAD_SNAPSHOT + frame.len();
    if raw_len > REPLICA_MAX || total > REPLICA_MAX {
        return Err(ReplicaError::TooLarge {
            len: raw_len.max(total) as u64,
            max: REPLICA_MAX,
        });
    }
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&REPLICA_MAGIC);
    out.push(0);
    out.extend_from_slice(&(raw_len as u32).to_le_bytes());
    out.extend_from_slice(&raw_hash.to_le_bytes());
    out.extend_from_slice(frame);
    Ok(out)
}

/// Puts a patch body together: `frame` is the zstd frame of the new snapshot against the old one (as a raw
/// prefix, see [`PATCH_DICT_HEAD`]), `raw_len`/`raw_hash` describe the new snapshot, `from_raw_hash` the old one.
pub fn patch_body(
    raw_len: usize,
    raw_hash: u64,
    from_raw_hash: u64,
    frame: &[u8],
) -> Result<Vec<u8>, ReplicaError> {
    let total = REPLICA_HEAD_PATCH + frame.len();
    if raw_len > REPLICA_MAX || total > REPLICA_MAX {
        return Err(ReplicaError::TooLarge {
            len: raw_len.max(total) as u64,
            max: REPLICA_MAX,
        });
    }
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&REPLICA_MAGIC);
    out.push(1);
    out.extend_from_slice(&(raw_len as u32).to_le_bytes());
    out.extend_from_slice(&raw_hash.to_le_bytes());
    out.extend_from_slice(&from_raw_hash.to_le_bytes());
    out.extend_from_slice(frame);
    Ok(out)
}

/// Reads and checks the header; every length is checked before anything is allocated.
pub fn parse_header(body: &[u8]) -> Result<BodyHeader<'_>, ReplicaError> {
    if body.len() > REPLICA_MAX {
        return Err(ReplicaError::TooLarge {
            len: body.len() as u64,
            max: REPLICA_MAX,
        });
    }
    if body.len() < REPLICA_HEAD_SNAPSHOT {
        return Err(if body.len() >= 4 && body[..4] != REPLICA_MAGIC {
            ReplicaError::BadMagic
        } else {
            ReplicaError::Truncated
        });
    }
    if body[..4] != REPLICA_MAGIC {
        return Err(ReplicaError::BadMagic);
    }
    let kind = match body[4] {
        0 => BodyKind::Snapshot,
        1 => BodyKind::Patch,
        k => return Err(ReplicaError::BadKind(k)),
    };
    let raw_len = u32::from_le_bytes(body[5..9].try_into().unwrap()) as usize;
    if raw_len > REPLICA_MAX {
        return Err(ReplicaError::TooLarge {
            len: raw_len as u64,
            max: REPLICA_MAX,
        });
    }
    let raw_hash = u64::from_le_bytes(body[9..17].try_into().unwrap());
    let (from_raw_hash, at) = match kind {
        BodyKind::Snapshot => (None, REPLICA_HEAD_SNAPSHOT),
        BodyKind::Patch => {
            let b = body
                .get(REPLICA_HEAD_SNAPSHOT..REPLICA_HEAD_SNAPSHOT + 8)
                .ok_or(ReplicaError::Truncated)?;
            (
                Some(u64::from_le_bytes(b.try_into().unwrap())),
                REPLICA_HEAD_SNAPSHOT + 8,
            )
        }
    };
    Ok(BodyHeader {
        kind,
        raw_len,
        raw_hash,
        from_raw_hash,
        frame: &body[at..],
    })
}

/// Unpacks a snapshot body to its GCS1 bytes and checks length and `raw_hash`.
pub fn decode_snapshot(body: &[u8]) -> Result<Vec<u8>, ReplicaError> {
    let head = parse_header(body)?;
    if head.kind != BodyKind::Snapshot {
        return Err(ReplicaError::PatchNotSupported);
    }
    let mut decoder = FrameDecoder::new();
    decoder.set_max_window_size(REPLICA_MAX as u64);
    // The capacity is the hard cap: `decode_all_to_vec` never grows the vector.
    let mut out = Vec::with_capacity(head.raw_len);
    decoder
        .decode_all_to_vec(head.frame, &mut out)
        .map_err(|e| ReplicaError::Zstd(e.to_string()))?;
    if out.len() != head.raw_len {
        return Err(ReplicaError::SizeMismatch {
            declared: head.raw_len,
            actual: out.len(),
        });
    }
    let actual = xxh64(0, &out);
    if actual != head.raw_hash {
        return Err(ReplicaError::HashMismatch {
            declared: head.raw_hash,
            actual,
        });
    }
    Ok(out)
}

/// Applies a patch body to the snapshot it was made against and returns the new GCS1 bytes. `base_raw_hash` is the
/// `raw_hash` the client already knows for `base` (from `GcBase`/`GcTurn`; it is not computed again here); it
/// must be the one the patch names. Length and `raw_hash` of the result are checked.
pub fn apply_patch(body: &[u8], base: &[u8], base_raw_hash: u64) -> Result<Vec<u8>, ReplicaError> {
    let head = parse_header(body)?;
    if head.kind != BodyKind::Patch {
        return Err(ReplicaError::NotAPatch);
    }
    let from = head.from_raw_hash.unwrap_or_default();
    if from != base_raw_hash {
        return Err(ReplicaError::WrongBase {
            declared: from,
            actual: base_raw_hash,
        });
    }
    if base.len() > REPLICA_MAX {
        return Err(ReplicaError::TooLarge {
            len: base.len() as u64,
            max: REPLICA_MAX,
        });
    }
    let mut dict =
        Dictionary::decode_dict(&PATCH_DICT_HEAD).map_err(|e| ReplicaError::Zstd(e.to_string()))?;
    dict.dict_content = base.to_vec();
    let dict_id = dict.id;
    let mut decoder = FrameDecoder::new();
    decoder.set_max_window_size(REPLICA_MAX as u64);
    decoder
        .add_dict(dict)
        .map_err(|e| ReplicaError::Zstd(e.to_string()))?;
    let mut src = head.frame;
    decoder
        .init(&mut src)
        .map_err(|e| ReplicaError::Zstd(e.to_string()))?;
    // The frame names no dictionary: the base is its past all the same.
    decoder
        .force_dict(dict_id)
        .map_err(|e| ReplicaError::Zstd(e.to_string()))?;
    let mut out: Vec<u8> = Vec::with_capacity(head.raw_len);
    while !decoder.is_finished() {
        decoder
            .decode_blocks(&mut src, BlockDecodingStrategy::UptoBytes(4 << 20))
            .map_err(|e| ReplicaError::Zstd(e.to_string()))?;
        drain(&mut decoder, &mut out, head.raw_len)?;
    }
    drain(&mut decoder, &mut out, head.raw_len)?;
    if out.len() != head.raw_len {
        return Err(ReplicaError::SizeMismatch {
            declared: head.raw_len,
            actual: out.len(),
        });
    }
    let actual = xxh64(0, &out);
    if actual != head.raw_hash {
        return Err(ReplicaError::HashMismatch {
            declared: head.raw_hash,
            actual,
        });
    }
    Ok(out)
}

/// Moves what the decoder may give out into `out`; more than `cap` bytes in all is an error.
fn drain(decoder: &mut FrameDecoder, out: &mut Vec<u8>, cap: usize) -> Result<(), ReplicaError> {
    while let Some(chunk) = decoder.collect() {
        if chunk.is_empty() {
            break;
        }
        if out.len() + chunk.len() > cap {
            return Err(ReplicaError::SizeMismatch {
                declared: cap,
                actual: out.len() + chunk.len(),
            });
        }
        out.extend_from_slice(&chunk);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        // Compressible, with some structure.
        (0..50_000u32)
            .flat_map(|i| (i % 977).to_le_bytes())
            .collect()
    }

    #[test]
    fn a_snapshot_round_trips() {
        let raw = sample();
        let body = encode_snapshot(&raw).unwrap();
        assert!(body.len() < raw.len() / 2);
        let head = parse_header(&body).unwrap();
        assert_eq!(head.kind, BodyKind::Snapshot);
        assert_eq!(head.raw_len, raw.len());
        assert_eq!(head.raw_hash, xxh64(0, &raw));
        assert_eq!(decode_snapshot(&body).unwrap(), raw);
        assert_eq!(
            decode_snapshot(&encode_snapshot(&[]).unwrap()).unwrap(),
            b""
        );
    }

    #[test]
    fn every_truncation_is_an_error() {
        let body = encode_snapshot(&sample()[..2000]).unwrap();
        for n in 0..body.len() {
            assert!(decode_snapshot(&body[..n]).is_err(), "cut at {n}");
        }
    }

    #[test]
    fn a_wrong_raw_hash_is_refused() {
        let mut body = encode_snapshot(&sample()).unwrap();
        body[9] ^= 1;
        assert!(matches!(
            decode_snapshot(&body),
            Err(ReplicaError::HashMismatch { .. })
        ));
    }

    #[test]
    fn a_wrong_length_is_refused() {
        let mut body = encode_snapshot(&sample()).unwrap();
        let n = u32::from_le_bytes(body[5..9].try_into().unwrap());
        body[5..9].copy_from_slice(&(n - 1).to_le_bytes());
        assert!(decode_snapshot(&body).is_err());
        body[5..9].copy_from_slice(&(n + 1).to_le_bytes());
        assert!(matches!(
            decode_snapshot(&body),
            Err(ReplicaError::SizeMismatch { .. })
        ));
    }

    #[test]
    fn the_limit_is_enforced_before_allocating() {
        let mut body = encode_snapshot(b"abc").unwrap();
        body[5..9].copy_from_slice(&((REPLICA_MAX as u32) + 1).to_le_bytes());
        assert!(matches!(
            parse_header(&body),
            Err(ReplicaError::TooLarge { .. })
        ));
        let big = vec![0u8; REPLICA_MAX + 1];
        assert!(matches!(
            parse_header(&big),
            Err(ReplicaError::TooLarge { .. })
        ));
        assert!(matches!(
            encode_snapshot(&big),
            Err(ReplicaError::TooLarge { .. })
        ));
        // A frame that unpacks to more than it declares does not grow the buffer.
        let raw = vec![7u8; 10_000];
        let mut body = encode_snapshot(&raw).unwrap();
        body[5..9].copy_from_slice(&100u32.to_le_bytes());
        assert!(decode_snapshot(&body).is_err());
    }

    #[test]
    fn magic_and_kind_are_checked() {
        let mut body = encode_snapshot(b"abc").unwrap();
        body[0] = b'X';
        assert_eq!(parse_header(&body), Err(ReplicaError::BadMagic));
        let mut body = encode_snapshot(b"abc").unwrap();
        body[4] = 9;
        assert_eq!(parse_header(&body), Err(ReplicaError::BadKind(9)));
    }

    #[test]
    fn a_patch_header_parses_and_is_not_a_snapshot() {
        let snap = encode_snapshot(b"abcdef").unwrap();
        let mut body = snap[..REPLICA_HEAD_SNAPSHOT].to_vec();
        body[4] = 1;
        body.extend_from_slice(&42u64.to_le_bytes());
        body.extend_from_slice(&snap[REPLICA_HEAD_SNAPSHOT..]);
        let head = parse_header(&body).unwrap();
        assert_eq!((head.kind, head.from_raw_hash), (BodyKind::Patch, Some(42)));
        assert_eq!(decode_snapshot(&body), Err(ReplicaError::PatchNotSupported));
        assert_eq!(
            parse_header(&body[..REPLICA_HEAD_SNAPSHOT + 3]),
            Err(ReplicaError::Truncated)
        );
    }

    /// A patch body whose frame does not use the base (a plain frame is a valid patch frame too).
    fn plain_patch(new: &[u8], base: &[u8]) -> Vec<u8> {
        let frame = compress_to_vec(new, CompressionLevel::Fastest);
        patch_body(new.len(), xxh64(0, new), xxh64(0, base), &frame).unwrap()
    }

    #[test]
    fn a_patch_applies_and_is_checked() {
        let base = sample();
        let mut new = base.clone();
        new[100] ^= 1;
        let body = plain_patch(&new, &base);
        let head = parse_header(&body).unwrap();
        assert_eq!(head.kind, BodyKind::Patch);
        assert_eq!(head.from_raw_hash, Some(xxh64(0, &base)));
        assert_eq!(apply_patch(&body, &base, xxh64(0, &base)).unwrap(), new);
        // The wrong base, a snapshot body, a wrong result hash.
        assert!(matches!(
            apply_patch(&body, &base, 5),
            Err(ReplicaError::WrongBase { .. })
        ));
        assert_eq!(
            apply_patch(&encode_snapshot(&new).unwrap(), &base, 0),
            Err(ReplicaError::NotAPatch)
        );
        let mut bad = body.clone();
        bad[9] ^= 1;
        assert!(matches!(
            apply_patch(&bad, &base, xxh64(0, &base)),
            Err(ReplicaError::HashMismatch { .. })
        ));
    }

    #[test]
    fn every_truncation_of_a_patch_is_an_error() {
        let base = sample()[..3000].to_vec();
        let mut new = base.clone();
        new[7] = 9;
        let body = plain_patch(&new, &base);
        let h = xxh64(0, &base);
        for n in 0..body.len() {
            assert!(apply_patch(&body[..n], &base, h).is_err(), "cut at {n}");
        }
        assert!(apply_patch(&body, &base, h).is_ok());
    }

    #[test]
    fn a_patch_over_the_limit_is_refused() {
        let mut body = plain_patch(b"abc", b"abd");
        body[5..9].copy_from_slice(&((REPLICA_MAX as u32) + 1).to_le_bytes());
        assert!(matches!(
            apply_patch(&body, b"abd", xxh64(0, b"abd")),
            Err(ReplicaError::TooLarge { .. })
        ));
        assert!(matches!(
            patch_body(REPLICA_MAX + 1, 0, 0, b""),
            Err(ReplicaError::TooLarge { .. })
        ));
        // A frame that unpacks to more than the header says does not grow the buffer.
        let frame = compress_to_vec(&vec![7u8; 10_000][..], CompressionLevel::Fastest);
        let body = patch_body(100, 0, 1, &frame).unwrap();
        assert!(apply_patch(&body, b"", 1).is_err());
    }

    #[test]
    fn the_dictionary_header_parses() {
        let dict = Dictionary::decode_dict(&PATCH_DICT_HEAD).unwrap();
        assert_eq!(dict.id, u32::from_le_bytes(*b"RPL1"));
        assert_eq!(dict.offset_hist, [1, 4, 8]);
        assert!(dict.dict_content.is_empty());
    }
}
