//! Raw access to a package file: cut it into sections, change a section, lay it out again with a
//! valid hash. The corrupt-input and limit tests use it to make files the builder would refuse.

use serde::Serialize;
use serde::de::DeserializeOwned;

pub const HEADER: usize = 16;
pub const ENTRY: usize = 16;
pub const HASH: usize = 32;

/// One section of a file as stored.
#[derive(Clone, Debug)]
pub struct Entry {
    pub tag: [u8; 4],
    pub flags: u32,
    pub payload: Vec<u8>,
}

fn word(b: &[u8], at: usize) -> usize {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]) as usize
}

/// The sections of a well-formed file, in table order.
pub fn split(bytes: &[u8]) -> Vec<Entry> {
    (0..word(bytes, 12))
        .map(|i| {
            let at = HEADER + ENTRY * i;
            let (offset, len) = (word(bytes, at + 8), word(bytes, at + 12));
            Entry {
                tag: [bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]],
                flags: word(bytes, at + 4) as u32,
                payload: bytes[offset..offset + len].to_vec(),
            }
        })
        .collect()
}

/// The byte range of a section's payload in the file.
pub fn range_of(bytes: &[u8], tag: &[u8; 4]) -> std::ops::Range<usize> {
    for i in 0..word(bytes, 12) {
        let at = HEADER + ENTRY * i;
        if &bytes[at..at + 4] == tag {
            let (offset, len) = (word(bytes, at + 8), word(bytes, at + 12));
            return offset..offset + len;
        }
    }
    panic!("no section {}", String::from_utf8_lossy(tag));
}

/// Writes a file: header, table, sections 8-aligned in the given order, zero padding, hash. The
/// order is not sorted and nothing is checked, so a test can lay out what the reader must refuse.
/// The format minor is the writer's: 1 with a `STYP` section, else 0.
pub fn assemble(entries: &[Entry]) -> Vec<u8> {
    let minor = u16::from(entries.iter().any(|e| &e.tag == b"STYP"));
    let mut out = Vec::new();
    out.extend_from_slice(b"C5UI2PKG");
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&minor.to_le_bytes());
    out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
    let mut offset = HEADER + ENTRY * entries.len();
    let mut starts = Vec::new();
    for e in entries {
        starts.push(offset);
        offset = (offset + e.payload.len()).next_multiple_of(8);
    }
    for (e, start) in entries.iter().zip(&starts) {
        out.extend_from_slice(&e.tag);
        out.extend_from_slice(&e.flags.to_le_bytes());
        out.extend_from_slice(&(*start as u32).to_le_bytes());
        out.extend_from_slice(&(e.payload.len() as u32).to_le_bytes());
    }
    for (e, start) in entries.iter().zip(&starts) {
        out.resize(*start, 0);
        out.extend_from_slice(&e.payload);
    }
    out.resize(offset, 0);
    let hash = blake3::hash(&out);
    out.extend_from_slice(hash.as_bytes());
    out
}

/// Recomputes the trailing hash after the body was changed.
pub fn rehash(bytes: &mut [u8]) {
    let n = bytes.len() - HASH;
    let hash = blake3::hash(&bytes[..n]);
    bytes[n..].copy_from_slice(hash.as_bytes());
}

/// The file with the payload of a section replaced.
pub fn with_payload(bytes: &[u8], tag: &[u8; 4], f: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let mut entries = split(bytes);
    let e = entries
        .iter_mut()
        .find(|e| &e.tag == tag)
        .expect("the section is in the file");
    f(&mut e.payload);
    assemble(&entries)
}

/// The file with a structural section decoded, changed and written back.
pub fn with<T: Serialize + DeserializeOwned>(
    bytes: &[u8],
    tag: &[u8; 4],
    f: impl FnOnce(&mut T),
) -> Vec<u8> {
    with_payload(bytes, tag, |p| {
        let mut value: T = postcard::from_bytes(p).expect("the section decodes");
        f(&mut value);
        *p = postcard::to_allocvec(&value).expect("the section encodes");
    })
}

/// The file with a blob section's index changed; the byte area stays as it is.
pub fn with_index<T: Serialize + DeserializeOwned>(
    bytes: &[u8],
    tag: &[u8; 4],
    f: impl FnOnce(&mut T),
) -> Vec<u8> {
    with_payload(bytes, tag, |p| {
        let index_len = word(p, 0);
        let mut value: T = postcard::from_bytes(&p[4..4 + index_len]).expect("the index decodes");
        f(&mut value);
        let index = postcard::to_allocvec(&value).expect("the index encodes");
        let mut out = (index.len() as u32).to_le_bytes().to_vec();
        out.extend_from_slice(&index);
        out.extend_from_slice(&p[4 + index_len..]);
        *p = out;
    })
}
