//! DXBC container: the chunk table and the signature chunks.
//!
//! Layout (Microsoft "DXBC container format", as in `d3dcompiler` output): `"DXBC"`, a 16-byte
//! checksum, version 1, total size, chunk count, one offset per chunk; every chunk is a FourCC, a
//! size and the payload. The checksum is not verified: the container is read from the pinned
//! binaries and a damaged one fails the structure checks first.

use super::DxbcError;

pub const MAGIC: &[u8; 4] = b"DXBC";
/// Magic, checksum (16), version, total size, chunk count.
pub const HEADER_SIZE: usize = 32;
/// Offset of the version dword.
const VERSION_AT: usize = 20;
const SIZE_AT: usize = 24;
const COUNT_AT: usize = 28;
/// Version dword of every container written by `d3dcompiler`.
const CONTAINER_VERSION: u32 = 1;
/// A container with more chunks is not real (the real ones have 3 or 4).
const MAX_CHUNKS: usize = 64;

/// Program chunk of SM4.0/4.1 and of SM5.0.
pub const SHDR: [u8; 4] = *b"SHDR";
pub const SHEX: [u8; 4] = *b"SHEX";
/// Input, output and patch-constant signatures (24-byte elements).
pub const ISGN: [u8; 4] = *b"ISGN";
pub const OSGN: [u8; 4] = *b"OSGN";
pub const PCSG: [u8; 4] = *b"PCSG";
/// `D3D11_SIGNATURE_PARAMETER_DESC` as stored in the file: name offset, semantic index, system
/// value, component type, register, mask, read-write mask, two padding bytes.
const SIG_ELEMENT_SIZE: usize = 24;
/// Count, key (8 in every compiler version).
const SIG_HEADER_SIZE: usize = 8;
/// A signature with more elements is not real (32 registers is the limit of the hardware).
const MAX_SIG_ELEMENTS: usize = 128;

pub fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        d.get(o..o.checked_add(4)?)?.try_into().ok()?,
    ))
}

/// One chunk of the container: FourCC and the payload's place in the container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub fourcc: [u8; 4],
    /// Offset of the payload (after the 8-byte chunk header).
    pub offset: usize,
    pub size: usize,
}

impl Chunk {
    pub fn name(&self) -> String {
        String::from_utf8_lossy(&self.fourcc).into_owned()
    }

    pub fn payload<'a>(&self, d: &'a [u8]) -> &'a [u8] {
        &d[self.offset..self.offset + self.size]
    }
}

/// Container size and chunk table. Every chunk lies inside the container and after the table.
pub fn read_chunks(d: &[u8]) -> Result<(usize, Vec<Chunk>), DxbcError> {
    if d.get(..4) != Some(&MAGIC[..]) {
        return Err(DxbcError::NotDxbc);
    }
    let short = || DxbcError::Truncated("header");
    let version = u32_at(d, VERSION_AT).ok_or_else(short)?;
    if version != CONTAINER_VERSION {
        return Err(DxbcError::Container(format!(
            "container version {version}, expected {CONTAINER_VERSION}"
        )));
    }
    let total = u32_at(d, SIZE_AT).ok_or_else(short)? as usize;
    let count = u32_at(d, COUNT_AT).ok_or_else(short)? as usize;
    if total > d.len() {
        return Err(DxbcError::Container(format!(
            "size {total} is past the end of the data ({})",
            d.len()
        )));
    }
    if count == 0 || count > MAX_CHUNKS {
        return Err(DxbcError::Container(format!("{count} chunks")));
    }
    let table_end = HEADER_SIZE + count * 4;
    if total < table_end {
        return Err(DxbcError::Container("chunk table past the end".to_string()));
    }
    let mut chunks = Vec::with_capacity(count);
    for i in 0..count {
        let at = u32_at(d, HEADER_SIZE + i * 4).ok_or_else(short)? as usize;
        if at < table_end || at.checked_add(8).is_none_or(|e| e > total) {
            return Err(DxbcError::Container(format!(
                "chunk {i} at {at} is outside the container"
            )));
        }
        let fourcc: [u8; 4] = d[at..at + 4].try_into().expect("4 bytes");
        if !fourcc.iter().all(u8::is_ascii_alphanumeric) {
            return Err(DxbcError::Container(format!(
                "chunk {i} has the name {fourcc:02x?}"
            )));
        }
        let size = u32_at(d, at + 4).ok_or_else(short)? as usize;
        if at.checked_add(8 + size).is_none_or(|e| e > total) {
            return Err(DxbcError::Container(format!(
                "chunk {} ({size} bytes at {at}) is past the end",
                String::from_utf8_lossy(&fourcc)
            )));
        }
        chunks.push(Chunk {
            fourcc,
            offset: at + 8,
            size,
        });
    }
    Ok((total, chunks))
}

/// `D3D_NAME`: system value of a signature element.
pub fn sysval_name(v: u32) -> &'static str {
    match v {
        0 => "",
        1 => "POS",
        2 => "CLIPDST",
        3 => "CULLDST",
        4 => "RTINDEX",
        5 => "VPINDEX",
        6 => "VERTID",
        7 => "PRIMID",
        8 => "INSTID",
        9 => "FFACE",
        10 => "SAMPLE",
        11..=16 => "TESSFACTOR",
        17 | 18 => "INSIDETESSFACTOR",
        64 => "TARGET",
        65 => "DEPTH",
        66 => "COVERAGE",
        67 => "DEPTHGE",
        68 => "DEPTHLE",
        _ => "?",
    }
}

/// An element of `ISGN`, `OSGN` or `PCSG`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SigElement {
    /// `TEXCOORD`, `SV_Position`, ... as the HLSL wrote it.
    pub name: String,
    pub index: u32,
    /// `D3D_NAME`; 0 for an ordinary semantic.
    pub sysval: u32,
    /// `D3D_REGISTER_COMPONENT_TYPE`: 1 uint, 2 sint, 3 float.
    pub comp_type: u32,
    pub register: u32,
    /// Components the signature declares, bit 0 = x.
    pub mask: u8,
    /// Components the program reads (input) or writes (output).
    pub rw_mask: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Signature {
    pub elements: Vec<SigElement>,
}

impl Signature {
    /// Parses the payload of a signature chunk.
    pub fn parse(payload: &[u8], chunk: &'static str) -> Result<Signature, DxbcError> {
        let bad = |why: String| DxbcError::Signature { chunk, why };
        let count = u32_at(payload, 0).ok_or_else(|| bad("no header".into()))? as usize;
        if count > MAX_SIG_ELEMENTS {
            return Err(bad(format!("{count} elements")));
        }
        if payload.len() < SIG_HEADER_SIZE + count * SIG_ELEMENT_SIZE {
            return Err(bad(format!(
                "{count} elements do not fit in {} bytes",
                payload.len()
            )));
        }
        let mut elements = Vec::with_capacity(count);
        for i in 0..count {
            let e = SIG_HEADER_SIZE + i * SIG_ELEMENT_SIZE;
            let field = |k: usize| u32_at(payload, e + 4 * k).expect("checked above");
            let name_at = field(0) as usize;
            let tail = payload
                .get(name_at..)
                .ok_or_else(|| bad(format!("element {i}: name at {name_at}")))?;
            let len = tail
                .iter()
                .position(|b| *b == 0)
                .ok_or_else(|| bad(format!("element {i}: name is not terminated")))?;
            let name = std::str::from_utf8(&tail[..len])
                .map_err(|_| bad(format!("element {i}: name is not UTF-8")))?
                .to_string();
            elements.push(SigElement {
                name,
                index: field(1),
                sysval: field(2),
                comp_type: field(3),
                register: field(4),
                mask: payload[e + 20],
                rw_mask: payload[e + 21],
            });
        }
        Ok(Signature { elements })
    }

    /// `name` + `index` of the elements, for matching.
    pub fn semantics(&self) -> impl Iterator<Item = (&str, u32)> {
        self.elements.iter().map(|e| (e.name.as_str(), e.index))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A signature chunk payload with these elements `(name, index, sysval, register, mask)`.
    pub fn sig_payload(els: &[(&str, u32, u32, u32, u8)]) -> Vec<u8> {
        let mut head = Vec::new();
        head.extend_from_slice(&(els.len() as u32).to_le_bytes());
        head.extend_from_slice(&8u32.to_le_bytes());
        let mut names = Vec::new();
        let names_at = SIG_HEADER_SIZE + els.len() * SIG_ELEMENT_SIZE;
        for (name, index, sv, reg, mask) in els {
            head.extend_from_slice(&((names_at + names.len()) as u32).to_le_bytes());
            head.extend_from_slice(&index.to_le_bytes());
            head.extend_from_slice(&sv.to_le_bytes());
            head.extend_from_slice(&3u32.to_le_bytes());
            head.extend_from_slice(&reg.to_le_bytes());
            head.extend_from_slice(&[*mask, *mask, 0, 0]);
            names.extend_from_slice(name.as_bytes());
            names.push(0);
        }
        head.extend_from_slice(&names);
        head
    }

    #[test]
    fn signature_round_trip() {
        let p = sig_payload(&[("SV_Position", 0, 1, 0, 15), ("TEXCOORD", 1, 0, 1, 7)]);
        let s = Signature::parse(&p, "ISGN").unwrap();
        assert_eq!(s.elements.len(), 2);
        assert_eq!(s.elements[0].name, "SV_Position");
        assert_eq!(s.elements[0].sysval, 1);
        assert_eq!(s.elements[1].index, 1);
        assert_eq!(s.elements[1].mask, 7);
        assert_eq!(s.elements[1].register, 1);
        assert_eq!(
            s.semantics().collect::<Vec<_>>(),
            [("SV_Position", 0), ("TEXCOORD", 1)]
        );
    }

    #[test]
    fn signature_rejects_garbage() {
        assert!(Signature::parse(&[], "ISGN").is_err());
        // Count says 2, the payload holds none.
        assert!(Signature::parse(&[2, 0, 0, 0, 8, 0, 0, 0], "ISGN").is_err());
        let mut p = sig_payload(&[("A", 0, 0, 0, 15)]);
        // Name offset past the end.
        p[8..12].copy_from_slice(&0xFFFFu32.to_le_bytes());
        assert!(Signature::parse(&p, "ISGN").is_err());
        // Name without a terminator.
        let mut q = sig_payload(&[("A", 0, 0, 0, 15)]);
        let n = q.len();
        q[n - 1] = b'B';
        assert!(Signature::parse(&q, "ISGN").is_err());
    }

    #[test]
    fn chunk_table_is_checked() {
        let mut d = Vec::new();
        d.extend_from_slice(MAGIC);
        d.extend_from_slice(&[0; 16]);
        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes()); // total, patched below
        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(&36u32.to_le_bytes());
        d.extend_from_slice(b"ISGN");
        d.extend_from_slice(&4u32.to_le_bytes());
        d.extend_from_slice(&[1, 2, 3, 4]);
        let total = d.len() as u32;
        d[24..28].copy_from_slice(&total.to_le_bytes());
        let (t, ch) = read_chunks(&d).unwrap();
        assert_eq!(t, d.len());
        assert_eq!(ch.len(), 1);
        assert_eq!(ch[0].name(), "ISGN");
        assert_eq!(ch[0].payload(&d), &[1, 2, 3, 4]);
        // Chunk longer than the container.
        let mut bad = d.clone();
        bad[40..44].copy_from_slice(&100u32.to_le_bytes());
        assert!(read_chunks(&bad).is_err());
        // Not DXBC.
        assert!(matches!(read_chunks(b"MZ\0\0"), Err(DxbcError::NotDxbc)));
        // Wrong container version.
        let mut v = d.clone();
        v[20] = 2;
        assert!(read_chunks(&v).is_err());
    }
}
