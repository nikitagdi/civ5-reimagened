//! CTAB: the constant table inside the shader (`D3DXSHADER_CONSTANTTABLE`, d3dx9shader.h).
//!
//! Layout (all offsets are in bytes from the first byte after the `CTAB` tag):
//! header (28 bytes), constant infos (20 bytes each), type infos (16 bytes each), struct member
//! infos (8 bytes each), strings.

/// `'CTAB'` as a little-endian dword.
pub const CTAB_TAG: u32 = 0x4241_5443;
/// `sizeof(D3DXSHADER_CONSTANTTABLE)`.
const HEADER_SIZE: usize = 28;
/// `sizeof(D3DXSHADER_CONSTANTINFO)`.
const CONSTANT_INFO_SIZE: usize = 20;
/// `sizeof(D3DXSHADER_STRUCTMEMBERINFO)`.
const MEMBER_INFO_SIZE: usize = 8;
/// A constant table with more constants than this is not real.
const MAX_CONSTANTS: usize = 4096;
/// Nesting limit of struct members (guards against cycles in hostile data).
const MAX_DEPTH: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CtabError {
    #[error("CTAB: {0} runs past the end of the table (offset {1})")]
    OutOfRange(&'static str, usize),
    #[error("CTAB: header size {0}, expected 28")]
    BadHeader(u32),
    #[error("CTAB: string at {0} is not terminated")]
    BadString(usize),
    #[error("CTAB: unknown {0} {1}")]
    BadEnum(&'static str, u32),
    #[error("CTAB: too many constants ({0})")]
    TooMany(usize),
    #[error("CTAB: struct members nested deeper than {MAX_DEPTH}")]
    TooDeep,
}

/// `D3DXREGISTER_SET`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RegisterSet {
    Bool,
    Int4,
    Float4,
    Sampler,
}

impl RegisterSet {
    pub fn from_code(c: u32) -> Option<RegisterSet> {
        Some(match c {
            0 => RegisterSet::Bool,
            1 => RegisterSet::Int4,
            2 => RegisterSet::Float4,
            3 => RegisterSet::Sampler,
            _ => return None,
        })
    }
    pub fn code(self) -> u32 {
        self as u32
    }
    /// Register prefix: `b`, `i`, `c`, `s`.
    pub fn prefix(self) -> char {
        match self {
            RegisterSet::Bool => 'b',
            RegisterSet::Int4 => 'i',
            RegisterSet::Float4 => 'c',
            RegisterSet::Sampler => 's',
        }
    }
}

/// `D3DXPARAMETER_CLASS`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamClass {
    Scalar,
    Vector,
    MatrixRows,
    MatrixColumns,
    Object,
    Struct,
}

impl ParamClass {
    pub fn from_code(c: u32) -> Option<ParamClass> {
        Some(match c {
            0 => ParamClass::Scalar,
            1 => ParamClass::Vector,
            2 => ParamClass::MatrixRows,
            3 => ParamClass::MatrixColumns,
            4 => ParamClass::Object,
            5 => ParamClass::Struct,
            _ => return None,
        })
    }
    pub fn code(self) -> u32 {
        self as u32
    }
    /// Spelling used in the catalog and in diagnostics.
    pub fn name(self) -> &'static str {
        match self {
            ParamClass::Scalar => "scalar",
            ParamClass::Vector => "vector",
            ParamClass::MatrixRows => "matrix_rows",
            ParamClass::MatrixColumns => "matrix_columns",
            ParamClass::Object => "object",
            ParamClass::Struct => "struct",
        }
    }
}

/// `D3DXPARAMETER_TYPE`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamType {
    Void,
    Bool,
    Int,
    Float,
    String,
    Texture,
    Texture1d,
    Texture2d,
    Texture3d,
    TextureCube,
    Sampler,
    Sampler1d,
    Sampler2d,
    Sampler3d,
    SamplerCube,
    PixelShader,
    VertexShader,
    PixelFragment,
    VertexFragment,
    Unsupported,
}

impl ParamType {
    const ALL: [ParamType; 20] = [
        ParamType::Void,
        ParamType::Bool,
        ParamType::Int,
        ParamType::Float,
        ParamType::String,
        ParamType::Texture,
        ParamType::Texture1d,
        ParamType::Texture2d,
        ParamType::Texture3d,
        ParamType::TextureCube,
        ParamType::Sampler,
        ParamType::Sampler1d,
        ParamType::Sampler2d,
        ParamType::Sampler3d,
        ParamType::SamplerCube,
        ParamType::PixelShader,
        ParamType::VertexShader,
        ParamType::PixelFragment,
        ParamType::VertexFragment,
        ParamType::Unsupported,
    ];

    pub fn from_code(c: u32) -> Option<ParamType> {
        ParamType::ALL.get(c as usize).copied()
    }
    pub fn code(self) -> u32 {
        self as u32
    }
    /// HLSL spelling in the `D3DDisassemble` parameter list.
    pub fn name(self) -> &'static str {
        match self {
            ParamType::Void => "void",
            ParamType::Bool => "bool",
            ParamType::Int => "int",
            ParamType::Float => "float",
            ParamType::String => "string",
            ParamType::Texture => "texture",
            ParamType::Texture1d => "Texture1D",
            ParamType::Texture2d => "Texture2D",
            ParamType::Texture3d => "Texture3D",
            ParamType::TextureCube => "TextureCube",
            ParamType::Sampler => "sampler",
            ParamType::Sampler1d => "sampler1D",
            ParamType::Sampler2d => "sampler2D",
            ParamType::Sampler3d => "sampler3D",
            ParamType::SamplerCube => "samplerCUBE",
            ParamType::PixelShader => "pixelshader",
            ParamType::VertexShader => "vertexshader",
            ParamType::PixelFragment => "pixelfragment",
            ParamType::VertexFragment => "vertexfragment",
            ParamType::Unsupported => "unsupported",
        }
    }
}

/// Type of a constant (`D3DXSHADER_TYPEINFO`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CtabType {
    pub class: ParamClass,
    pub ty: ParamType,
    pub rows: u16,
    pub columns: u16,
    /// Array length (1 for a plain value).
    pub elements: u16,
    pub members: Vec<CtabMember>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CtabMember {
    pub name: String,
    pub ty: CtabType,
}

/// One constant or sampler of the table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CtabConstant {
    pub name: String,
    pub set: RegisterSet,
    pub reg: u16,
    /// Number of registers.
    pub count: u16,
    pub ty: CtabType,
    /// Default value bytes (little-endian floats/ints), if the table carries one.
    pub default: Option<Vec<u8>>,
}

impl CtabConstant {
    /// Register range `[reg, reg + count)`.
    pub fn contains(&self, set: RegisterSet, reg: u32) -> bool {
        self.set == set && reg >= self.reg as u32 && reg < self.reg as u32 + self.count as u32
    }
}

/// The whole constant table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ctab {
    /// Compiler string ("Microsoft (R) HLSL Shader Compiler 9.27.952.3022").
    pub creator: String,
    /// Version token of the shader the table belongs to.
    pub version: u32,
    pub flags: u32,
    /// Target profile string (`ps_3_0`); empty when the table has none.
    pub target: String,
    /// In table order.
    pub constants: Vec<CtabConstant>,
}

impl Ctab {
    /// The constant that owns `set` register `reg`.
    pub fn find(&self, set: RegisterSet, reg: u32) -> Option<&CtabConstant> {
        self.constants.iter().find(|c| c.contains(set, reg))
    }
}

struct Reader<'a> {
    d: &'a [u8],
}

impl Reader<'_> {
    fn bytes(&self, off: usize, len: usize, what: &'static str) -> Result<&[u8], CtabError> {
        off.checked_add(len)
            .and_then(|end| self.d.get(off..end))
            .ok_or(CtabError::OutOfRange(what, off))
    }
    fn u32(&self, off: usize, what: &'static str) -> Result<u32, CtabError> {
        Ok(u32::from_le_bytes(
            self.bytes(off, 4, what)?.try_into().unwrap(),
        ))
    }
    fn u16(&self, off: usize, what: &'static str) -> Result<u16, CtabError> {
        Ok(u16::from_le_bytes(
            self.bytes(off, 2, what)?.try_into().unwrap(),
        ))
    }
    /// NUL-terminated string at `off`; offset 0 means "none" and gives an empty string.
    fn string(&self, off: usize) -> Result<String, CtabError> {
        if off == 0 {
            return Ok(String::new());
        }
        let tail = self
            .d
            .get(off..)
            .ok_or(CtabError::OutOfRange("string", off))?;
        let end = tail
            .iter()
            .position(|&b| b == 0)
            .ok_or(CtabError::BadString(off))?;
        Ok(String::from_utf8_lossy(&tail[..end]).into_owned())
    }

    fn type_info(&self, off: usize, depth: usize) -> Result<CtabType, CtabError> {
        if depth > MAX_DEPTH {
            return Err(CtabError::TooDeep);
        }
        let class = self.u16(off, "type info")? as u32;
        let ty = self.u16(off + 2, "type info")? as u32;
        let rows = self.u16(off + 4, "type info")?;
        let columns = self.u16(off + 6, "type info")?;
        let elements = self.u16(off + 8, "type info")?;
        let n_members = self.u16(off + 10, "type info")? as usize;
        let members_off = self.u32(off + 12, "type info")? as usize;
        let class = ParamClass::from_code(class).ok_or(CtabError::BadEnum("class", class))?;
        let ty = ParamType::from_code(ty).ok_or(CtabError::BadEnum("type", ty))?;
        let mut members = Vec::with_capacity(n_members.min(64));
        for i in 0..n_members {
            let m = members_off + i * MEMBER_INFO_SIZE;
            let name = self.string(self.u32(m, "struct member")? as usize)?;
            let ty = self.type_info(self.u32(m + 4, "struct member")? as usize, depth + 1)?;
            members.push(CtabMember { name, ty });
        }
        Ok(CtabType {
            class,
            ty,
            rows,
            columns,
            elements,
            members,
        })
    }
}

/// Parses the payload of a `CTAB` comment (the bytes after the `CTAB` tag).
pub fn parse(d: &[u8]) -> Result<Ctab, CtabError> {
    let r = Reader { d };
    let size = r.u32(0, "header")?;
    if size as usize != HEADER_SIZE {
        return Err(CtabError::BadHeader(size));
    }
    let creator = r.string(r.u32(4, "header")? as usize)?;
    let version = r.u32(8, "header")?;
    let count = r.u32(12, "header")? as usize;
    let info_off = r.u32(16, "header")? as usize;
    let flags = r.u32(20, "header")?;
    let target = r.string(r.u32(24, "header")? as usize)?;
    if count > MAX_CONSTANTS {
        return Err(CtabError::TooMany(count));
    }
    let mut constants = Vec::with_capacity(count);
    for i in 0..count {
        let c = info_off + i * CONSTANT_INFO_SIZE;
        let name = r.string(r.u32(c, "constant info")? as usize)?;
        let set = r.u16(c + 4, "constant info")? as u32;
        let reg = r.u16(c + 6, "constant info")?;
        let n = r.u16(c + 8, "constant info")?;
        // c + 10: reserved word.
        let type_off = r.u32(c + 12, "constant info")? as usize;
        let default_off = r.u32(c + 16, "constant info")? as usize;
        let set = RegisterSet::from_code(set).ok_or(CtabError::BadEnum("register set", set))?;
        let ty = r.type_info(type_off, 0)?;
        // Default value: `elements * rows * columns` dwords, at most what the registers hold.
        let default = if default_off != 0 {
            let dwords = (ty.elements.max(1) as usize)
                * (ty.rows.max(1) as usize)
                * (ty.columns.max(1) as usize);
            let max = n as usize * 16;
            Some(
                r.bytes(default_off, (dwords * 4).min(max.max(4)), "default value")?
                    .to_vec(),
            )
        } else {
            None
        };
        constants.push(CtabConstant {
            name,
            set,
            reg,
            count: n,
            ty,
            default,
        });
    }
    Ok(Ctab {
        creator,
        version,
        flags,
        target,
        constants,
    })
}

/// Serialises a table back to the payload layout (used by tests to build synthetic blocks).
/// Strings and type infos are laid out after the constant infos; no deduplication.
pub fn build(ctab: &Ctab) -> Vec<u8> {
    struct B {
        out: Vec<u8>,
    }
    impl B {
        fn u32_at(&mut self, at: usize, v: u32) {
            self.out[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        fn string(&mut self, s: &str) -> u32 {
            let off = self.out.len() as u32;
            self.out.extend_from_slice(s.as_bytes());
            self.out.push(0);
            off
        }
        fn align(&mut self) {
            while !self.out.len().is_multiple_of(4) {
                self.out.push(0);
            }
        }
        fn ty(&mut self, t: &CtabType) -> u32 {
            // Members first (their own infos), then this info.
            let mut member_infos = Vec::new();
            for m in &t.members {
                let name = self.string(&m.name);
                self.align();
                let ty = self.ty(&m.ty);
                member_infos.push((name, ty));
            }
            self.align();
            let members_off = self.out.len() as u32;
            for (n, ty) in &member_infos {
                self.out.extend_from_slice(&n.to_le_bytes());
                self.out.extend_from_slice(&ty.to_le_bytes());
            }
            let off = self.out.len() as u32;
            self.out
                .extend_from_slice(&(t.class.code() as u16).to_le_bytes());
            self.out
                .extend_from_slice(&(t.ty.code() as u16).to_le_bytes());
            self.out.extend_from_slice(&t.rows.to_le_bytes());
            self.out.extend_from_slice(&t.columns.to_le_bytes());
            self.out.extend_from_slice(&t.elements.to_le_bytes());
            self.out
                .extend_from_slice(&(t.members.len() as u16).to_le_bytes());
            let mo = if t.members.is_empty() { 0 } else { members_off };
            self.out.extend_from_slice(&mo.to_le_bytes());
            off
        }
    }
    let mut b = B {
        out: vec![0; HEADER_SIZE + ctab.constants.len() * CONSTANT_INFO_SIZE],
    };
    let creator = b.string(&ctab.creator);
    let target = if ctab.target.is_empty() {
        0
    } else {
        b.string(&ctab.target)
    };
    b.align();
    b.u32_at(0, HEADER_SIZE as u32);
    b.u32_at(4, creator);
    b.u32_at(8, ctab.version);
    b.u32_at(12, ctab.constants.len() as u32);
    b.u32_at(16, HEADER_SIZE as u32);
    b.u32_at(20, ctab.flags);
    b.u32_at(24, target);
    for (i, c) in ctab.constants.iter().enumerate() {
        let at = HEADER_SIZE + i * CONSTANT_INFO_SIZE;
        let name = b.string(&c.name);
        b.align();
        let ty = b.ty(&c.ty);
        let def = match &c.default {
            Some(bytes) => {
                b.align();
                let off = b.out.len() as u32;
                b.out.extend_from_slice(bytes);
                off
            }
            None => 0,
        };
        b.u32_at(at, name);
        b.out[at + 4..at + 6].copy_from_slice(&(c.set.code() as u16).to_le_bytes());
        b.out[at + 6..at + 8].copy_from_slice(&c.reg.to_le_bytes());
        b.out[at + 8..at + 10].copy_from_slice(&c.count.to_le_bytes());
        b.u32_at(at + 12, ty);
        b.u32_at(at + 16, def);
    }
    b.align();
    b.out
}
