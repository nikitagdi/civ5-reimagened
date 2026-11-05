//! Self-describing type tree: member definitions and their 32-bit layout.

use std::sync::Arc;

use crate::bytes::{i32_at, latin1, u32_at};
use crate::names::{member_name, name_hash_bytes};
use crate::{GrannyError, GrannyFile, Ptr};

/// Size of one `granny_data_type_definition` record with 32-bit pointers.
pub(crate) const MEMBER_RECORD_SIZE: u32 = 32;
const MAX_MEMBERS: usize = 4096;
const MAX_INLINE_DEPTH: u32 = 32;

/// Granny `granny_member_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MemberKind {
    Inline,
    Reference,
    ReferenceToArray,
    ArrayOfReferences,
    VariantReference,
    ReferenceToVariantArray,
    String,
    Transform,
    Real32,
    Int8,
    UInt8,
    BinormalInt8,
    NormalUInt8,
    Int16,
    UInt16,
    BinormalInt16,
    NormalUInt16,
    Int32,
    UInt32,
    Real16,
    EmptyReference,
}

impl MemberKind {
    /// Maps the stored code; 0 (end of list) and 6 (removed member) are not member kinds.
    pub fn from_code(code: u32) -> Option<Self> {
        use MemberKind::*;
        Some(match code {
            1 => Inline,
            2 => Reference,
            3 => ReferenceToArray,
            4 => ArrayOfReferences,
            5 => VariantReference,
            7 => ReferenceToVariantArray,
            8 => String,
            9 => Transform,
            10 => Real32,
            11 => Int8,
            12 => UInt8,
            13 => BinormalInt8,
            14 => NormalUInt8,
            15 => Int16,
            16 => UInt16,
            17 => BinormalInt16,
            18 => NormalUInt16,
            19 => Int32,
            20 => UInt32,
            21 => Real16,
            22 => EmptyReference,
            _ => return None,
        })
    }

    /// Stored size of one element, except `Inline` which takes its type's size.
    pub fn slot_size(self) -> u32 {
        use MemberKind::*;
        match self {
            Inline => 0,
            Reference | String | Real32 | Int32 | UInt32 | EmptyReference => 4,
            ReferenceToArray | ArrayOfReferences | VariantReference => 8,
            ReferenceToVariantArray => 12,
            Transform => 68,
            Int8 | UInt8 | BinormalInt8 | NormalUInt8 => 1,
            Int16 | UInt16 | BinormalInt16 | NormalUInt16 | Real16 => 2,
        }
    }

    /// True for kinds that hold pointers (the object graph continues through them).
    pub fn is_pointer(self) -> bool {
        use MemberKind::*;
        matches!(
            self,
            Reference
                | ReferenceToArray
                | ArrayOfReferences
                | VariantReference
                | ReferenceToVariantArray
                | String
        )
    }

    pub fn name(self) -> &'static str {
        use MemberKind::*;
        match self {
            Inline => "inline",
            Reference => "reference",
            ReferenceToArray => "reference-to-array",
            ArrayOfReferences => "array-of-references",
            VariantReference => "variant-reference",
            ReferenceToVariantArray => "reference-to-variant-array",
            String => "string",
            Transform => "transform",
            Real32 => "real32",
            Int8 => "int8",
            UInt8 => "uint8",
            BinormalInt8 => "binormal-int8",
            NormalUInt8 => "normal-uint8",
            Int16 => "int16",
            UInt16 => "uint16",
            BinormalInt16 => "binormal-int16",
            NormalUInt16 => "normal-uint16",
            Int32 => "int32",
            UInt32 => "uint32",
            Real16 => "real16",
            EmptyReference => "empty-reference",
        }
    }
}

/// Where a member name came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameSource {
    /// The type tree stores the string.
    Stored,
    /// Stripped tree; the hash was found in the dictionary.
    Dictionary,
    /// Stripped tree; unknown hash, the name reads `#xxxxxxxx`.
    Unknown,
}

#[derive(Debug, Clone)]
pub struct Member {
    pub kind: MemberKind,
    /// Display name: the stored or dictionary name, else `#xxxxxxxx`.
    pub name: Box<str>,
    /// `!crc32(name)`; identical for stored and stripped trees, so lookups use it.
    pub name_hash: u32,
    pub name_source: NameSource,
    /// Referenced type (element type for arrays, layout for inline members).
    pub type_ref: Option<Ptr>,
    /// Fixed inline array width; 0 means a single element.
    pub array_width: u32,
    /// Byte offset inside the owning object.
    pub offset: u32,
    pub element_size: u32,
    /// Parsed layout for `Inline` members.
    pub inline_type: Option<Arc<TypeDef>>,
    pub extra: [u32; 3],
}

impl Member {
    pub fn count(&self) -> u32 {
        self.array_width.max(1)
    }

    /// Total bytes this member occupies (element size times count; cannot overflow, checked at parse).
    pub fn size(&self) -> u32 {
        self.element_size.saturating_mul(self.count())
    }
}

#[derive(Debug)]
pub struct TypeDef {
    pub at: Ptr,
    pub members: Vec<Member>,
    pub size: u32,
    has_pointers: bool,
}

impl TypeDef {
    /// Whether instances (including inline sub-objects) contain pointers worth following.
    pub fn has_pointers(&self) -> bool {
        self.has_pointers
    }

    pub fn member(&self, name: &str) -> Option<(usize, &Member)> {
        let h = crate::names::name_hash(name);
        self.members
            .iter()
            .enumerate()
            .find(|(_, m)| m.name_hash == h)
    }

    pub(crate) fn parse(
        file: &GrannyFile<'_>,
        at: Ptr,
        depth: u32,
    ) -> Result<TypeDef, GrannyError> {
        if depth > MAX_INLINE_DEPTH {
            return Err(GrannyError::TypeTooDeep(at));
        }
        let sec = file.section_bytes(at.section)?;
        let mut members = Vec::new();
        let mut offset: u32 = 0;
        let mut has_pointers = false;
        for i in 0.. {
            if i >= MAX_MEMBERS {
                return Err(GrannyError::TooManyMembers(at));
            }
            let rec = (i as u32)
                .checked_mul(MEMBER_RECORD_SIZE)
                .and_then(|o| at.offset.checked_add(o))
                .ok_or(GrannyError::OutOfBounds("type record"))?;
            let r = rec as usize;
            let code = u32_at(sec, r, "type record")?;
            if code == 0 {
                break;
            }
            let kind = MemberKind::from_code(code)
                .ok_or(GrannyError::UnknownMemberType { kind: code, at })?;
            let slot = |k: u32| Ptr {
                section: at.section,
                offset: rec.saturating_add(k),
            };
            let (name, name_hash, name_source) = match file.pointer_at(slot(4)) {
                Some(p) => {
                    let s = file.c_string(p)?;
                    (
                        latin1(s).into_boxed_str(),
                        name_hash_bytes(s),
                        NameSource::Stored,
                    )
                }
                None => {
                    let h = u32_at(sec, r.saturating_add(4), "type record")?;
                    match member_name(h) {
                        Some(n) => (n.into(), h, NameSource::Dictionary),
                        None => (format!("#{h:08x}").into(), h, NameSource::Unknown),
                    }
                }
            };
            let type_ref = file.pointer_at(slot(8));
            let array_width = i32_at(sec, r.saturating_add(12), "type record")?.max(0) as u32;
            let extra = [
                u32_at(sec, r.saturating_add(16), "type record")?,
                u32_at(sec, r.saturating_add(20), "type record")?,
                u32_at(sec, r.saturating_add(24), "type record")?,
            ];
            let (element_size, inline_type) = if kind == MemberKind::Inline {
                let child = type_ref.ok_or(GrannyError::MissingType("inline member"))?;
                let t = file.type_at_depth(child, depth + 1)?;
                has_pointers |= t.has_pointers;
                (t.size, Some(t))
            } else {
                has_pointers |= kind.is_pointer();
                (kind.slot_size(), None)
            };
            let member_size = element_size
                .checked_mul(array_width.max(1))
                .ok_or(GrannyError::SizeOverflow("type size"))?;
            members.push(Member {
                kind,
                name,
                name_hash,
                name_source,
                type_ref,
                array_width,
                offset,
                element_size,
                inline_type,
                extra,
            });
            offset = offset
                .checked_add(member_size)
                .ok_or(GrannyError::SizeOverflow("type size"))?;
        }
        Ok(TypeDef {
            at,
            members,
            size: offset,
            has_pointers,
        })
    }
}
