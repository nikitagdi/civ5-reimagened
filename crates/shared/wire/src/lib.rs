//! Transport frames (net-protocol 4.13, 4.14): the WebSocket frame with an encoding byte, zstd compression
//! over `ruzstd`, and length-prefixed frames for the gateway-worker IPC. No I/O except in `ipc`.

pub mod catalog;
mod frame;
pub mod ipc;
pub mod replica;
mod zstd;

pub use frame::{
    ENC_DICT, ENC_RAW, ENC_ZSTD, Encoder, GATEWAY_MAX_FRAME, Limits, MAX_RAW, decode, unframe,
};
pub use zstd::{compress, compress_with, decompress};

/// A frame or a message was rejected. Network data never panics; it ends here.
#[derive(Debug, thiserror::Error)]
pub enum WireError {
    #[error("empty frame")]
    Empty,
    #[error("unknown encoding byte {0}")]
    UnknownEnc(u8),
    #[error("compressed frame not allowed here")]
    CompressedNotAllowed,
    #[error("frame of {len} bytes exceeds {max}")]
    FrameTooLarge { len: usize, max: usize },
    #[error("content of {declared} bytes exceeds {max}")]
    RawTooLarge { declared: u64, max: usize },
    #[error("packed block declares {declared} bytes, holds {actual}")]
    SizeMismatch { declared: usize, actual: usize },
    #[error("zstd: {0}")]
    Zstd(String),
    #[error("codec: {0}")]
    Codec(#[from] protocol::CodecError),
}
