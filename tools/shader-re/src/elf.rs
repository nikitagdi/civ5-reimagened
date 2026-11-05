//! ELF32 little-endian reader: section headers and `.dynsym`/`.dynstr`.
//!
//! Layout constants: System V ABI, "ELF-32 Object File Format" (Elf32_Ehdr, Elf32_Shdr, Elf32_Sym).

use crate::scan::Section;

#[derive(Debug, thiserror::Error)]
pub enum ElfError {
    #[error("not an ELF32 little-endian file")]
    NotElf32,
    #[error("ELF truncated or inconsistent: {0}")]
    Malformed(&'static str),
}

const EI_CLASS: usize = 4;
const ELFCLASS32: u8 = 1;
const EI_DATA: usize = 5;
const ELFDATA2LSB: u8 = 1;
const EHDR_SIZE: usize = 52;
const E_SHOFF: usize = 0x20;
const E_SHENTSIZE: usize = 0x2E;
const E_SHNUM: usize = 0x30;
const E_SHSTRNDX: usize = 0x32;
const SHDR_SIZE: usize = 40;
const SYM_SIZE: usize = 16;

/// A `.dynsym` entry with its name resolved through `.dynstr`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub value: u32,
    pub size: u32,
    /// Index of the section the symbol lives in (0 = undefined).
    pub shndx: u16,
}

#[derive(Debug)]
pub struct Elf<'a> {
    data: &'a [u8],
    sections: Vec<Section>,
}

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(o..o + 2)?.try_into().ok()?))
}

fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

fn cstr_at(d: &[u8], o: usize) -> Option<String> {
    let rest = d.get(o..)?;
    let end = rest.iter().position(|&b| b == 0)?;
    Some(String::from_utf8_lossy(&rest[..end]).into_owned())
}

impl<'a> Elf<'a> {
    pub fn parse(data: &'a [u8]) -> Result<Elf<'a>, ElfError> {
        if data.len() < EHDR_SIZE
            || &data[..4] != b"\x7fELF"
            || data[EI_CLASS] != ELFCLASS32
            || data[EI_DATA] != ELFDATA2LSB
        {
            return Err(ElfError::NotElf32);
        }
        let shoff = u32_at(data, E_SHOFF).ok_or(ElfError::Malformed("e_shoff"))? as usize;
        let shentsize =
            u16_at(data, E_SHENTSIZE).ok_or(ElfError::Malformed("e_shentsize"))? as usize;
        let shnum = u16_at(data, E_SHNUM).ok_or(ElfError::Malformed("e_shnum"))? as usize;
        let shstrndx = u16_at(data, E_SHSTRNDX).ok_or(ElfError::Malformed("e_shstrndx"))? as usize;
        if shentsize < SHDR_SIZE || shnum == 0 || shstrndx >= shnum {
            return Err(ElfError::Malformed("section header table"));
        }
        // (name offset, type, addr, file offset, size)
        let mut raw = Vec::with_capacity(shnum);
        for i in 0..shnum {
            let h = shoff
                .checked_add(i * shentsize)
                .ok_or(ElfError::Malformed("section header offset"))?;
            let rd = |o: usize| u32_at(data, h + o).ok_or(ElfError::Malformed("section header"));
            raw.push((rd(0)?, rd(4)?, rd(12)?, rd(16)? as usize, rd(20)? as usize));
        }
        let (_, _, _, str_off, str_size) = raw[shstrndx];
        let strtab = data
            .get(str_off..str_off.saturating_add(str_size))
            .ok_or(ElfError::Malformed(".shstrtab out of file"))?;
        let mut sections = Vec::with_capacity(shnum);
        for (name_off, sh_type, addr, offset, size) in raw {
            let name =
                cstr_at(strtab, name_off as usize).ok_or(ElfError::Malformed("section name"))?;
            // SHT_NOBITS (8) has no bytes in the file.
            let size = if sh_type == 8 { 0 } else { size };
            if offset.checked_add(size).is_none_or(|e| e > data.len()) {
                return Err(ElfError::Malformed("section out of file"));
            }
            sections.push(Section {
                name,
                addr: u64::from(addr),
                offset,
                size,
            });
        }
        Ok(Elf { data, sections })
    }

    pub fn sections(&self) -> &[Section] {
        &self.sections
    }

    pub fn section(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|s| s.name == name)
    }

    /// `.dynsym` with names from `.dynstr`; empty when the file has no dynamic symbols.
    pub fn dynamic_symbols(&self) -> Result<Vec<Symbol>, ElfError> {
        let (Some(sym), Some(strs)) = (self.section(".dynsym"), self.section(".dynstr")) else {
            return Ok(Vec::new());
        };
        let syms = &self.data[sym.offset..sym.offset + sym.size];
        let strtab = &self.data[strs.offset..strs.offset + strs.size];
        let mut out = Vec::with_capacity(syms.len() / SYM_SIZE);
        for e in syms.chunks_exact(SYM_SIZE) {
            let name_off = u32_at(e, 0).ok_or(ElfError::Malformed("symbol"))? as usize;
            out.push(Symbol {
                name: cstr_at(strtab, name_off).ok_or(ElfError::Malformed("symbol name"))?,
                value: u32_at(e, 4).ok_or(ElfError::Malformed("symbol"))?,
                size: u32_at(e, 8).ok_or(ElfError::Malformed("symbol"))?,
                shndx: u16_at(e, 14).ok_or(ElfError::Malformed("symbol"))?,
            });
        }
        Ok(out)
    }
}

/// Builds a small ELF32 for tests (also used by the scan tests): `sections` are
/// `(name, sh_type, addr, bytes)`; `.shstrtab` is appended.
#[cfg(test)]
pub(crate) fn build_test_elf(sections: &[(&str, u32, u32, &[u8])]) -> Vec<u8> {
    let mut body = Vec::new();
    let mut shstr = vec![0u8];
    let mut hdrs = Vec::new(); // (name_off, type, addr, offset, size)
    let base = EHDR_SIZE;
    for (name, ty, addr, bytes) in sections {
        let name_off = shstr.len() as u32;
        shstr.extend_from_slice(name.as_bytes());
        shstr.push(0);
        hdrs.push((
            name_off,
            *ty,
            *addr,
            (base + body.len()) as u32,
            bytes.len() as u32,
        ));
        body.extend_from_slice(bytes);
    }
    let name_off = shstr.len() as u32;
    shstr.extend_from_slice(b".shstrtab\0");
    hdrs.push((
        name_off,
        3,
        0,
        (base + body.len()) as u32,
        shstr.len() as u32,
    ));
    body.extend_from_slice(&shstr);

    let shoff = (base + body.len()) as u32;
    let shnum = (hdrs.len() + 1) as u16; // + the null section
    let mut out = vec![0u8; EHDR_SIZE];
    out[..4].copy_from_slice(b"\x7fELF");
    out[EI_CLASS] = ELFCLASS32;
    out[EI_DATA] = ELFDATA2LSB;
    out[E_SHOFF..E_SHOFF + 4].copy_from_slice(&shoff.to_le_bytes());
    out[E_SHENTSIZE..E_SHENTSIZE + 2].copy_from_slice(&(SHDR_SIZE as u16).to_le_bytes());
    out[E_SHNUM..E_SHNUM + 2].copy_from_slice(&shnum.to_le_bytes());
    out[E_SHSTRNDX..E_SHSTRNDX + 2].copy_from_slice(&(shnum - 1).to_le_bytes());
    out.extend_from_slice(&body);
    out.extend_from_slice(&[0u8; SHDR_SIZE]);
    for (name_off, ty, addr, off, size) in hdrs {
        let mut h = [0u8; SHDR_SIZE];
        h[0..4].copy_from_slice(&name_off.to_le_bytes());
        h[4..8].copy_from_slice(&ty.to_le_bytes());
        h[12..16].copy_from_slice(&addr.to_le_bytes());
        h[16..20].copy_from_slice(&off.to_le_bytes());
        h[20..24].copy_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&h);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_and_symbols() {
        let mut dynstr = vec![0u8];
        let n1 = dynstr.len() as u32;
        dynstr.extend_from_slice(b"FireGrafix::g_X\0");
        let mut dynsym = vec![0u8; SYM_SIZE]; // null symbol
        let mut e = [0u8; SYM_SIZE];
        e[0..4].copy_from_slice(&n1.to_le_bytes());
        e[4..8].copy_from_slice(&0x0a30_f834u32.to_le_bytes());
        e[8..12].copy_from_slice(&2u32.to_le_bytes());
        e[14..16].copy_from_slice(&7u16.to_le_bytes());
        dynsym.extend_from_slice(&e);
        let elf = build_test_elf(&[
            (".rodata", 1, 0x0900_0000, &[1, 2, 3, 4]),
            (".dynsym", 11, 0, &dynsym),
            (".dynstr", 3, 0, &dynstr),
        ]);
        let parsed = Elf::parse(&elf).unwrap();
        let ro = parsed.section(".rodata").unwrap();
        assert_eq!((ro.addr, ro.size), (0x0900_0000, 4));
        assert_eq!(&elf[ro.offset..ro.offset + ro.size], &[1, 2, 3, 4]);
        let syms = parsed.dynamic_symbols().unwrap();
        assert_eq!(syms.len(), 2);
        assert_eq!(syms[1].name, "FireGrafix::g_X");
        assert_eq!(
            (syms[1].value, syms[1].size, syms[1].shndx),
            (0x0a30_f834, 2, 7)
        );
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(matches!(Elf::parse(b"MZ"), Err(ElfError::NotElf32)));
        let mut elf = build_test_elf(&[(".rodata", 1, 0, &[0; 8])]);
        let n = elf.len();
        elf.truncate(n - 20);
        assert!(Elf::parse(&elf).is_err());
        // Section pointing outside the file.
        let mut elf = build_test_elf(&[(".rodata", 1, 0, &[0; 8])]);
        let shoff = u32_at(&elf, E_SHOFF).unwrap() as usize;
        let h = shoff + SHDR_SIZE; // first real section
        elf[h + 20..h + 24].copy_from_slice(&0x7fff_ffffu32.to_le_bytes());
        assert!(matches!(Elf::parse(&elf), Err(ElfError::Malformed(_))));
    }
}
