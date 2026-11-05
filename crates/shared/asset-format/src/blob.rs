//! Common blob framing: 4-byte magic, u16 LE format version, postcard payload.
//!
//! Every blob type owns its version (`VERSION` is what it writes, `MIN_VERSION` the oldest it
//! still reads), so changing one format does not touch the others (AB-D1).

use serde::{Serialize, de::DeserializeOwned};

use crate::FormatError;

/// Version written by the blob types that have not changed since B1.
pub const FORMAT_VERSION: u16 = 1;
const HEADER_LEN: usize = 6;

/// `magic | version | postcard(value)`.
pub(crate) fn encode_v<T: Serialize>(magic: [u8; 4], version: u16, value: &T) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + 64);
    out.extend_from_slice(&magic);
    out.extend_from_slice(&version.to_le_bytes());
    // Vec sink is infallible and our types have no failing Serialize impls.
    let payload = postcard::to_allocvec(value).expect("postcard to Vec cannot fail");
    out.extend_from_slice(&payload);
    out
}

/// Checks the magic and that the version is in `min..=max`; returns the version and the payload.
/// For types whose older payload layouts differ from the current one.
pub(crate) fn split_range(
    magic: [u8; 4],
    min: u16,
    max: u16,
    bytes: &[u8],
) -> Result<(u16, &[u8]), FormatError> {
    if bytes.len() < HEADER_LEN {
        return Err(FormatError::Truncated);
    }
    let found = [bytes[0], bytes[1], bytes[2], bytes[3]];
    if found != magic {
        return Err(FormatError::BadMagic {
            expected: magic,
            found,
        });
    }
    let version = u16::from_le_bytes([bytes[4], bytes[5]]);
    if !(min..=max).contains(&version) {
        return Err(FormatError::UnsupportedVersion {
            magic,
            found: version,
            supported: max,
            min_supported: min,
        });
    }
    Ok((version, &bytes[HEADER_LEN..]))
}

/// The whole postcard payload must be consumed.
pub(crate) fn payload<T: DeserializeOwned>(payload: &[u8]) -> Result<T, FormatError> {
    let (value, rest) = postcard::take_from_bytes(payload)?;
    if !rest.is_empty() {
        return Err(FormatError::TrailingBytes(rest.len()));
    }
    Ok(value)
}

/// Decodes a payload whose layout is the same in every version of `min..=max`.
pub(crate) fn decode_range<T: DeserializeOwned>(
    magic: [u8; 4],
    min: u16,
    max: u16,
    bytes: &[u8],
) -> Result<(u16, T), FormatError> {
    let (version, body) = split_range(magic, min, max, bytes)?;
    Ok((version, payload(body)?))
}
