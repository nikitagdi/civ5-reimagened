//! Bounds-checked little-endian reads.

use crate::GrannyError;

pub(crate) fn sub<'b>(
    b: &'b [u8],
    off: usize,
    len: usize,
    what: &'static str,
) -> Result<&'b [u8], GrannyError> {
    off.checked_add(len)
        .and_then(|end| b.get(off..end))
        .ok_or(GrannyError::OutOfBounds(what))
}

pub(crate) fn u32_at(b: &[u8], off: usize, what: &'static str) -> Result<u32, GrannyError> {
    let s = sub(b, off, 4, what)?;
    Ok(u32::from_le_bytes(
        s.try_into().map_err(|_| GrannyError::OutOfBounds(what))?,
    ))
}

pub(crate) fn i32_at(b: &[u8], off: usize, what: &'static str) -> Result<i32, GrannyError> {
    u32_at(b, off, what).map(|v| v as i32)
}

/// Total read for slices whose length was validated up front (object bytes cover their type);
/// missing bytes read as zero instead of panicking.
pub(crate) fn le<const N: usize>(b: &[u8], off: usize) -> [u8; N] {
    let mut out = [0u8; N];
    if let Some(s) = b.get(off..) {
        let n = s.len().min(N);
        out[..n].copy_from_slice(&s[..n]);
    }
    out
}

pub(crate) fn le_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(le(b, off))
}

pub(crate) fn le_f32(b: &[u8], off: usize) -> f32 {
    f32::from_le_bytes(le(b, off))
}

/// Latin-1 decode: Granny strings are byte strings, not UTF-8.
pub fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}
