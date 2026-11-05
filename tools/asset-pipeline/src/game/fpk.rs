//! Firaxis FPK pack reader.
//!
//! Layout (little endian), verified on all 147 packs of the Steam install:
//!
//! ```text
//! 00: 06 00 00 00 'F' 'P' 'K' '_' 00 00   magic (10 bytes)
//! 0A: u32 count
//! per entry:
//!   u32 name_len, name[name_len]           lower-case ASCII, no directories
//!   pad[p]                                 p = (2 - pos) mod 4, first byte == p
//!   u64-ish block[8]                       timestamp (often zero) + zero dword
//!   u32 size, u32 offset                   payload is stored uncompressed
//! ```
//!
//! The "variable 8..12 byte block" is therefore a fixed 8-byte block preceded by 0..3
//! alignment bytes: the pad makes the `size` field start 4-aligned relative to the
//! byte after the magic. Reading the pad length from the first block byte does not
//! work (when `p == 0` that byte is a timestamp byte), so it is derived from the position.

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

pub const MAGIC: [u8; 10] = *b"\x06\x00\x00\x00FPK_\x00\x00";

const HEADER_LEN: u64 = MAGIC.len() as u64 + 4;
/// name_len + 1 name byte + 8-byte block + size + offset.
const MIN_ENTRY_LEN: u64 = 4 + 1 + 8 + 8;
const MAX_NAME_LEN: usize = 1024;

#[derive(Debug, thiserror::Error)]
pub enum FpkError {
    #[error("i/o error: {0}")]
    Io(#[from] io::Error),
    #[error("not an FPK file (bad magic)")]
    BadMagic,
    #[error("truncated directory at byte {0}")]
    Truncated(u64),
    #[error("entry count {count} cannot fit in a {file_len}-byte file")]
    CountTooLarge { count: u32, file_len: u64 },
    #[error("entry {index}: invalid name length {len}")]
    BadNameLen { index: u32, len: u32 },
    #[error("entry {index}: name is not valid UTF-8")]
    BadName { index: u32 },
    #[error("entry {name:?}: payload {offset}+{size} is outside the {file_len}-byte file")]
    OutOfBounds {
        name: String,
        offset: u32,
        size: u32,
        file_len: u64,
    },
    #[error("entry {name:?}: payload offset {offset} overlaps the {dir_end}-byte directory")]
    OverlapsDirectory {
        name: String,
        offset: u32,
        dir_end: u64,
    },
    #[error("entry {name:?}: payload is shorter than its recorded size")]
    ShortPayload { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FpkEntry {
    /// As stored in the pack (already lower-case in the real packs).
    pub name: String,
    pub size: u32,
    pub offset: u32,
}

/// Counts consumed bytes: the pad length depends on the absolute position.
struct Counting<R> {
    inner: R,
    pos: u64,
}

impl<R: Read> Counting<R> {
    fn fill(&mut self, buf: &mut [u8]) -> Result<(), FpkError> {
        match self.inner.read_exact(buf) {
            Ok(()) => {
                self.pos += buf.len() as u64;
                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                Err(FpkError::Truncated(self.pos))
            }
            Err(e) => Err(e.into()),
        }
    }

    fn u32(&mut self) -> Result<u32, FpkError> {
        let mut b = [0u8; 4];
        self.fill(&mut b)?;
        Ok(u32::from_le_bytes(b))
    }
}

/// Parses the directory from the start of a pack. `file_len` is the size of the whole
/// file; every entry is validated against it, so payload reads can't go out of range.
pub fn read_directory<R: Read>(reader: R, file_len: u64) -> Result<Vec<FpkEntry>, FpkError> {
    let mut r = Counting {
        inner: reader,
        pos: 0,
    };
    let mut magic = [0u8; MAGIC.len()];
    r.fill(&mut magic)?;
    if magic != MAGIC {
        return Err(FpkError::BadMagic);
    }
    let count = r.u32()?;
    if u64::from(count) * MIN_ENTRY_LEN > file_len.saturating_sub(HEADER_LEN) {
        return Err(FpkError::CountTooLarge { count, file_len });
    }

    let mut entries = Vec::with_capacity(count as usize);
    for index in 0..count {
        let name_len = r.u32()?;
        if name_len == 0 || name_len as usize > MAX_NAME_LEN {
            return Err(FpkError::BadNameLen {
                index,
                len: name_len,
            });
        }
        let mut name = vec![0u8; name_len as usize];
        r.fill(&mut name)?;
        let name = String::from_utf8(name).map_err(|_| FpkError::BadName { index })?;

        let pad = (6 - r.pos % 4) % 4;
        let mut skip = [0u8; 3 + 8];
        r.fill(&mut skip[..(pad + 8) as usize])?;

        let size = r.u32()?;
        let offset = r.u32()?;
        if u64::from(offset) + u64::from(size) > file_len {
            return Err(FpkError::OutOfBounds {
                name,
                offset,
                size,
                file_len,
            });
        }
        entries.push(FpkEntry { name, size, offset });
    }

    let dir_end = r.pos;
    if let Some(e) = entries
        .iter()
        .find(|e| e.size > 0 && u64::from(e.offset) < dir_end)
    {
        return Err(FpkError::OverlapsDirectory {
            name: e.name.clone(),
            offset: e.offset,
            dir_end,
        });
    }
    Ok(entries)
}

/// Opens a pack and reads only its directory (a few KB), not the payloads.
pub fn open_directory(path: &Path) -> Result<Vec<FpkEntry>, FpkError> {
    let file = File::open(path)?;
    let file_len = file.metadata()?.len();
    read_directory(BufReader::new(file), file_len)
}

/// Reads one payload by seeking. Payloads are read on demand through a fresh handle
/// instead of mmap or whole-pack buffering: packs reach 200 MB while the pipeline
/// touches a few KB of each, and this needs no `unsafe`.
pub fn read_payload<R: Read + Seek>(r: &mut R, entry: &FpkEntry) -> Result<Vec<u8>, FpkError> {
    r.seek(SeekFrom::Start(u64::from(entry.offset)))?;
    let mut buf = Vec::new();
    r.take(u64::from(entry.size)).read_to_end(&mut buf)?;
    if buf.len() != entry.size as usize {
        return Err(FpkError::ShortPayload {
            name: entry.name.clone(),
        });
    }
    Ok(buf)
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::MAGIC;

    /// Builds a pack following the real layout; timestamps vary so the pad can't be
    /// mistaken for part of the block.
    pub fn build_fpk(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut dir_len = MAGIC.len() + 4;
        for (name, _) in files {
            dir_len += 4 + name.len();
            dir_len += (6 - dir_len % 4) % 4 + 8 + 8;
        }
        let mut data_off = dir_len.next_multiple_of(4);

        let mut out = MAGIC.to_vec();
        out.extend((files.len() as u32).to_le_bytes());
        let mut payloads = Vec::new();
        for (i, (name, bytes)) in files.iter().enumerate() {
            out.extend((name.len() as u32).to_le_bytes());
            out.extend(name.as_bytes());
            let pad = (6 - out.len() % 4) % 4;
            if pad > 0 {
                out.push(pad as u8);
                out.extend(std::iter::repeat_n(0u8, pad - 1));
            }
            let stamp: u32 = if i % 2 == 0 { 0 } else { 0x4bfc_5372 };
            out.extend(stamp.to_le_bytes());
            out.extend([0u8; 4]);
            out.extend((bytes.len() as u32).to_le_bytes());
            out.extend((data_off as u32).to_le_bytes());
            payloads.push((data_off, *bytes));
            data_off = (data_off + bytes.len()).next_multiple_of(4);
        }
        assert_eq!(out.len(), dir_len);
        for (off, bytes) in payloads {
            out.resize(off, 0);
            out.extend(bytes);
        }
        out.resize(data_off, 0);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::build_fpk;
    use super::*;
    use std::io::Cursor;

    const FILES: &[(&str, &[u8])] = &[
        ("a.txt", b"hello"),
        ("bb.bin", b""),
        ("ccc.gr2", &[1, 2, 3, 4, 5, 6, 7]),
        ("dddd.dds", b"DDS payload"),
        ("eeeee.xml", b"<x/>"),
    ];

    #[test]
    fn roundtrip_all_pad_lengths() {
        let pack = build_fpk(FILES);
        let entries = read_directory(Cursor::new(&pack), pack.len() as u64).unwrap();
        assert_eq!(entries.len(), FILES.len());
        let mut cur = Cursor::new(&pack);
        for (e, (name, bytes)) in entries.iter().zip(FILES) {
            assert_eq!(e.name, *name);
            assert_eq!(read_payload(&mut cur, e).unwrap(), *bytes);
        }
    }

    #[test]
    fn every_truncation_is_an_error_not_a_panic() {
        let pack = build_fpk(FILES);
        // The last payload ends at the end of the pack, so any cut breaks something.
        for len in 0..pack.len() {
            let r = read_directory(Cursor::new(&pack[..len]), len as u64);
            assert!(r.is_err(), "len {len} unexpectedly parsed");
        }
    }

    #[test]
    fn rejects_bad_magic() {
        let mut pack = build_fpk(FILES);
        pack[4] = b'X';
        let err = read_directory(Cursor::new(&pack), pack.len() as u64).unwrap_err();
        assert!(matches!(err, FpkError::BadMagic));
    }

    #[test]
    fn rejects_huge_count_and_name_len() {
        let mut pack = build_fpk(FILES);
        pack[10..14].copy_from_slice(&u32::MAX.to_le_bytes());
        let err = read_directory(Cursor::new(&pack), pack.len() as u64).unwrap_err();
        assert!(matches!(err, FpkError::CountTooLarge { .. }));

        let mut pack = build_fpk(FILES);
        pack[14..18].copy_from_slice(&u32::MAX.to_le_bytes());
        let err = read_directory(Cursor::new(&pack), pack.len() as u64).unwrap_err();
        assert!(matches!(err, FpkError::BadNameLen { index: 0, .. }));
    }

    #[test]
    fn rejects_out_of_bounds_payload() {
        let pack = build_fpk(FILES);
        // The same bytes seen as a shorter file: the last payload no longer fits.
        let short = pack.len() as u64 - 8;
        let err = read_directory(Cursor::new(&pack), short).unwrap_err();
        assert!(matches!(err, FpkError::OutOfBounds { .. }), "{err}");
    }

    #[test]
    fn rejects_payload_inside_directory() {
        let mut pack = build_fpk(&[("a.txt", b"hello")]);
        // 14 header + 4 + 5 name + 3 pad + 8 block + 8 size/offset = 42; offset is the last u32.
        let off_pos = 42 - 4;
        pack[off_pos..off_pos + 4].copy_from_slice(&2u32.to_le_bytes());
        let err = read_directory(Cursor::new(&pack), pack.len() as u64).unwrap_err();
        assert!(matches!(err, FpkError::OverlapsDirectory { .. }), "{err}");
    }

    #[test]
    fn short_payload_is_an_error() {
        let pack = build_fpk(&[("a.txt", b"hello")]);
        let entry = FpkEntry {
            name: "a.txt".into(),
            size: 100,
            offset: (pack.len() - 4) as u32,
        };
        let err = read_payload(&mut Cursor::new(&pack), &entry).unwrap_err();
        assert!(matches!(err, FpkError::ShortPayload { .. }));
    }
}
