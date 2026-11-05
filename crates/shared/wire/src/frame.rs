//! WebSocket frame: `[enc: u8][body]` (net-protocol 4.13, NP-D8).
//!
//! - `enc 0`: body is the postcard message as is.
//! - `enc 1`: body is `[raw length: u32 LE][zstd frame]`; inside is the postcard message.
//! - `enc 2`: reserved (zstd with a dictionary), not accepted until measured.

use crate::{WireError, compress, decompress};
use protocol::{Codec, Postcard};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::borrow::Cow;

pub const ENC_RAW: u8 = 0;
pub const ENC_ZSTD: u8 = 1;
/// Reserved for a zstd dictionary; frames with it are rejected.
pub const ENC_DICT: u8 = 2;

/// Largest body after unpacking, for every receiver (NP-D8).
pub const MAX_RAW: usize = 16 << 20;
/// Largest frame the gateway accepts from a client (4.13).
pub const GATEWAY_MAX_FRAME: usize = 64 << 10;

/// Server-side packer. The client never packs: it sends `enc 0` only (`Encoder::plain`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Encoder {
    /// Bodies shorter than this go out as `enc 0`.
    pub threshold: usize,
    /// The packed body must be smaller than raw by at least this percent, otherwise `enc 0`.
    pub min_gain_pct: u8,
}

impl Default for Encoder {
    fn default() -> Self {
        Encoder {
            threshold: 256,
            min_gain_pct: 10,
        }
    }
}

impl Encoder {
    /// `enc 0` frame, never packed (client to server).
    pub fn plain(raw: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(raw.len() + 1);
        out.push(ENC_RAW);
        out.extend_from_slice(raw);
        out
    }

    /// Frames `raw`: packs from `threshold` bytes if that wins at least `min_gain_pct`, else `enc 0`.
    /// The frame is never longer than `raw.len() + 1`.
    pub fn frame(&self, raw: &[u8]) -> Vec<u8> {
        if raw.len() >= self.threshold && raw.len() <= MAX_RAW {
            let z = compress(raw);
            let gain = usize::from(self.min_gain_pct.min(100));
            // Keep packed only when it is below (100 - gain) percent of raw.
            if z.len() * 100 < raw.len() * (100 - gain) {
                let mut out = Vec::with_capacity(z.len() + 1);
                out.push(ENC_ZSTD);
                out.extend_from_slice(&z);
                return out;
            }
        }
        Self::plain(raw)
    }

    /// Encodes `msg` with postcard and frames it.
    pub fn frame_msg<T: Serialize>(&self, msg: &T) -> Result<Vec<u8>, WireError> {
        Ok(self.frame(&Postcard.encode(msg)?))
    }
}

/// What a receiver accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Longest frame as received, bytes (checked first, before any parsing).
    pub max_frame: usize,
    /// Longest body after unpacking.
    pub max_raw: usize,
    /// `false`: reject `enc 1` (the gateway for client frames).
    pub allow_compressed: bool,
}

impl Limits {
    /// Gateway, frames from clients: up to 64 KiB, no packed frames.
    pub const GATEWAY: Limits = Limits {
        max_frame: GATEWAY_MAX_FRAME,
        max_raw: MAX_RAW,
        allow_compressed: false,
    };
    /// Client, frames from the server: packed frames allowed, 16 MiB after unpacking.
    pub const CLIENT: Limits = Limits {
        max_frame: MAX_RAW,
        max_raw: MAX_RAW,
        allow_compressed: true,
    };
}

/// Frame to body: `enc 0` is borrowed, `enc 1` is unpacked.
pub fn unframe<'a>(frame: &'a [u8], limits: &Limits) -> Result<Cow<'a, [u8]>, WireError> {
    if frame.len() > limits.max_frame {
        return Err(WireError::FrameTooLarge {
            len: frame.len(),
            max: limits.max_frame,
        });
    }
    let (&enc, body) = frame.split_first().ok_or(WireError::Empty)?;
    match enc {
        ENC_RAW => {
            if body.len() > limits.max_raw {
                return Err(WireError::RawTooLarge {
                    declared: body.len() as u64,
                    max: limits.max_raw,
                });
            }
            Ok(Cow::Borrowed(body))
        }
        ENC_ZSTD if limits.allow_compressed => Ok(Cow::Owned(decompress(body, limits.max_raw)?)),
        ENC_ZSTD => Err(WireError::CompressedNotAllowed),
        other => Err(WireError::UnknownEnc(other)),
    }
}

/// Frame to message: `unframe`, then postcard (trailing bytes are an error).
pub fn decode<T: DeserializeOwned>(frame: &[u8], limits: &Limits) -> Result<T, WireError> {
    let body = unframe(frame, limits)?;
    Ok(Postcard.decode(&body)?)
}
