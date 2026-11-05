//! Zero-copy view of objects described by the type tree.
//!
//! An `Object` is validated once on creation (its bytes cover its type), so reading its fields
//! cannot fail; following pointers (references, arrays, strings) is lazy and fallible.

use std::sync::Arc;

use crate::bytes::{latin1, le, le_f32, le_u32};
use crate::types::{Member, MemberKind};
use crate::{GrannyError, GrannyFile, Ptr, TypeDef};

#[derive(Clone)]
pub struct Object<'f> {
    file: &'f GrannyFile<'f>,
    ty: Arc<TypeDef>,
    at: Ptr,
    bytes: &'f [u8],
}

impl std::fmt::Debug for Object<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Object(type {}, at {})", self.ty.at, self.at)
    }
}

/// Granny `granny_transform`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub flags: u32,
    pub position: [f32; 3],
    pub orientation: [f32; 4],
    pub scale_shear: [[f32; 3]; 3],
}

impl Transform {
    fn read(b: &[u8]) -> Self {
        let f = |i: usize| le_f32(b, 4 + i * 4);
        Transform {
            flags: le_u32(b, 0),
            position: [f(0), f(1), f(2)],
            orientation: [f(3), f(4), f(5), f(6)],
            scale_shear: [
                [f(7), f(8), f(9)],
                [f(10), f(11), f(12)],
                [f(13), f(14), f(15)],
            ],
        }
    }
}

/// One field value. Scalars are decoded; pointers are lazy handles.
#[derive(Debug, Clone)]
pub enum Value<'f> {
    Inline(Object<'f>),
    Reference(Ref<'f>),
    /// `ReferenceToArray` and `ReferenceToVariantArray`: a contiguous run of objects.
    Array(Array<'f>),
    ArrayOfReferences(RefArray<'f>),
    VariantReference(Ref<'f>),
    String(Str<'f>),
    Transform(Transform),
    Real32(f32),
    Int8(i8),
    UInt8(u8),
    BinormalInt8(i8),
    NormalUInt8(u8),
    Int16(i16),
    UInt16(u16),
    BinormalInt16(i16),
    NormalUInt16(u16),
    Int32(i32),
    UInt32(u32),
    /// IEEE half, raw bits.
    Real16(u16),
    EmptyReference,
    /// Member with a fixed array width (e.g. `Real32 Position[3]`).
    Fixed(FixedArray<'f>),
}

impl Value<'_> {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Value::Inline(_) => "inline",
            Value::Reference(_) => "reference",
            Value::Array(_) => "array",
            Value::ArrayOfReferences(_) => "array-of-references",
            Value::VariantReference(_) => "variant-reference",
            Value::String(_) => "string",
            Value::Transform(_) => "transform",
            Value::Real32(_) => "real32",
            Value::Int8(_) => "int8",
            Value::UInt8(_) => "uint8",
            Value::BinormalInt8(_) => "binormal-int8",
            Value::NormalUInt8(_) => "normal-uint8",
            Value::Int16(_) => "int16",
            Value::UInt16(_) => "uint16",
            Value::BinormalInt16(_) => "binormal-int16",
            Value::NormalUInt16(_) => "normal-uint16",
            Value::Int32(_) => "int32",
            Value::UInt32(_) => "uint32",
            Value::Real16(_) => "real16",
            Value::EmptyReference => "empty-reference",
            Value::Fixed(_) => "fixed-array",
        }
    }

    /// Integer scalars widened to i64.
    pub fn as_i64(&self) -> Option<i64> {
        Some(match *self {
            Value::Int8(v) | Value::BinormalInt8(v) => v.into(),
            Value::UInt8(v) | Value::NormalUInt8(v) => v.into(),
            Value::Int16(v) | Value::BinormalInt16(v) => v.into(),
            Value::UInt16(v) | Value::NormalUInt16(v) => v.into(),
            Value::Int32(v) => v.into(),
            Value::UInt32(v) => v.into(),
            _ => return None,
        })
    }
}

/// Builds the value of one element whose bytes are `b`, located at `at`.
fn element<'f>(file: &'f GrannyFile<'f>, m: &Member, at: Ptr, b: &'f [u8]) -> Value<'f> {
    let slot = |k: u32| at.offset_by(k).and_then(|p| file.pointer_at(p));
    let count = || i32::from_le_bytes(le(b, 0));
    match m.kind {
        MemberKind::Inline => match &m.inline_type {
            Some(t) => Value::Inline(Object {
                file,
                ty: t.clone(),
                at,
                bytes: b,
            }),
            // Unreachable: inline members always get their type at parse.
            None => Value::EmptyReference,
        },
        MemberKind::Reference => Value::Reference(Ref {
            file,
            ty: m.type_ref,
            target: slot(0),
        }),
        MemberKind::VariantReference => Value::VariantReference(Ref {
            file,
            ty: slot(0),
            target: slot(4),
        }),
        MemberKind::ReferenceToArray => Value::Array(Array {
            file,
            ty: m.type_ref,
            count: count(),
            data: slot(4),
        }),
        MemberKind::ReferenceToVariantArray => Value::Array(Array {
            file,
            ty: slot(0),
            count: i32::from_le_bytes(le(b, 4)),
            data: slot(8),
        }),
        MemberKind::ArrayOfReferences => Value::ArrayOfReferences(RefArray {
            file,
            ty: m.type_ref,
            count: count(),
            data: slot(4),
        }),
        MemberKind::String => Value::String(Str {
            file,
            target: slot(0),
        }),
        MemberKind::Transform => Value::Transform(Transform::read(b)),
        MemberKind::Real32 => Value::Real32(le_f32(b, 0)),
        MemberKind::Int8 => Value::Int8(i8::from_le_bytes(le(b, 0))),
        MemberKind::UInt8 => Value::UInt8(le::<1>(b, 0)[0]),
        MemberKind::BinormalInt8 => Value::BinormalInt8(i8::from_le_bytes(le(b, 0))),
        MemberKind::NormalUInt8 => Value::NormalUInt8(le::<1>(b, 0)[0]),
        MemberKind::Int16 => Value::Int16(i16::from_le_bytes(le(b, 0))),
        MemberKind::UInt16 => Value::UInt16(u16::from_le_bytes(le(b, 0))),
        MemberKind::BinormalInt16 => Value::BinormalInt16(i16::from_le_bytes(le(b, 0))),
        MemberKind::NormalUInt16 => Value::NormalUInt16(u16::from_le_bytes(le(b, 0))),
        MemberKind::Int32 => Value::Int32(i32::from_le_bytes(le(b, 0))),
        MemberKind::UInt32 => Value::UInt32(le_u32(b, 0)),
        MemberKind::Real16 => Value::Real16(u16::from_le_bytes(le(b, 0))),
        MemberKind::EmptyReference => Value::EmptyReference,
    }
}

fn mismatch(field: &str, expected: &'static str, v: &Value<'_>) -> GrannyError {
    GrannyError::TypeMismatch {
        field: field.to_owned(),
        expected,
        found: v.kind_name(),
    }
}

impl<'f> Object<'f> {
    pub(crate) fn new(
        file: &'f GrannyFile<'f>,
        ty: Arc<TypeDef>,
        at: Ptr,
    ) -> Result<Self, GrannyError> {
        let bytes = file.bytes(at, ty.size as usize)?;
        Ok(Object {
            file,
            ty,
            at,
            bytes,
        })
    }

    pub fn file(&self) -> &'f GrannyFile<'f> {
        self.file
    }

    pub fn type_def(&self) -> &Arc<TypeDef> {
        &self.ty
    }

    pub fn at(&self) -> Ptr {
        self.at
    }

    /// The object's raw bytes (exactly its type's size).
    pub fn bytes(&self) -> &'f [u8] {
        self.bytes
    }

    pub fn members(&self) -> &[Member] {
        &self.ty.members
    }

    /// Value of the member at `index` in `members()`.
    pub fn value(&self, index: usize) -> Option<Value<'f>> {
        let m = self.ty.members.get(index)?;
        let start = m.offset as usize;
        let b = self
            .bytes
            .get(start..start.saturating_add(m.size() as usize))
            .unwrap_or(&[]);
        let at = self.at.offset_by(m.offset)?;
        if m.array_width > 0 {
            return Some(Value::Fixed(FixedArray {
                file: self.file,
                member: m.clone(),
                at,
                bytes: b,
            }));
        }
        Some(element(self.file, m, at, b))
    }

    /// Field by name; stripped (hashed) names match too.
    pub fn field(&self, name: &str) -> Option<Value<'f>> {
        self.ty.member(name).and_then(|(i, _)| self.value(i))
    }

    pub fn fields(&self) -> impl Iterator<Item = (&Member, Value<'f>)> + '_ {
        self.ty
            .members
            .iter()
            .enumerate()
            .filter_map(|(i, m)| Some((m, self.value(i)?)))
    }

    /// Like `field`, but a missing field is an error.
    pub fn get(&self, name: &str) -> Result<Value<'f>, GrannyError> {
        self.field(name)
            .ok_or_else(|| GrannyError::MissingField(name.to_owned()))
    }

    pub fn i32(&self, name: &str) -> Result<i32, GrannyError> {
        let v = self.get(name)?;
        v.as_i64()
            .and_then(|x| i32::try_from(x).ok())
            .ok_or_else(|| mismatch(name, "int32", &v))
    }

    pub fn u32(&self, name: &str) -> Result<u32, GrannyError> {
        let v = self.get(name)?;
        v.as_i64()
            .and_then(|x| u32::try_from(x).ok())
            .ok_or_else(|| mismatch(name, "uint32", &v))
    }

    pub fn f32(&self, name: &str) -> Result<f32, GrannyError> {
        match self.get(name)? {
            Value::Real32(x) => Ok(x),
            v => Err(mismatch(name, "real32", &v)),
        }
    }

    pub fn transform(&self, name: &str) -> Result<Transform, GrannyError> {
        match self.get(name)? {
            Value::Transform(t) => Ok(t),
            v => Err(mismatch(name, "transform", &v)),
        }
    }

    /// String field (Latin-1 decoded); `None` for a null string.
    pub fn string(&self, name: &str) -> Result<Option<String>, GrannyError> {
        match self.get(name)? {
            Value::String(s) => Ok(s.get()?.map(latin1)),
            v => Err(mismatch(name, "string", &v)),
        }
    }

    /// The raw 4 bytes of a member as a u32, without following any pointer. Packed Civ5 files
    /// keep an ID (a name hash) in the `Name` string slots, where the pointer would be.
    pub fn raw_u32(&self, name: &str) -> Option<u32> {
        let (_, m) = self.ty.member(name)?;
        let b = self.bytes.get(m.offset as usize..)?;
        (b.len() >= 4).then(|| le_u32(b, 0))
    }

    /// `(name, id)` of a `Name`-like string member: the string if present, else the raw ID
    /// (non-zero) stored in the pointer slot.
    pub fn name_and_id(&self, name: &str) -> Result<(Option<String>, Option<u32>), GrannyError> {
        let s = self.string(name)?;
        let id = if s.is_none() {
            self.raw_u32(name).filter(|&v| v != 0)
        } else {
            None
        };
        Ok((s, id))
    }

    /// Inline sub-object.
    pub fn inline(&self, name: &str) -> Result<Object<'f>, GrannyError> {
        match self.get(name)? {
            Value::Inline(o) => Ok(o),
            v => Err(mismatch(name, "inline", &v)),
        }
    }

    /// Plain or variant reference; `None` for null.
    pub fn reference(&self, name: &str) -> Result<Option<Object<'f>>, GrannyError> {
        match self.get(name)? {
            Value::Reference(r) | Value::VariantReference(r) => r.get(),
            v => Err(mismatch(name, "reference", &v)),
        }
    }

    /// Contiguous array (plain or variant), resolved and bounds-checked.
    pub fn array(&self, name: &str) -> Result<ArrayData<'f>, GrannyError> {
        match self.get(name)? {
            Value::Array(a) => a.resolve(),
            v => Err(mismatch(name, "array", &v)),
        }
    }

    pub fn references(&self, name: &str) -> Result<RefArray<'f>, GrannyError> {
        match self.get(name)? {
            Value::ArrayOfReferences(a) => Ok(a),
            v => Err(mismatch(name, "array-of-references", &v)),
        }
    }
}

/// A (possibly null) pointer to an object of a known or variant type.
#[derive(Debug, Clone, Copy)]
pub struct Ref<'f> {
    file: &'f GrannyFile<'f>,
    ty: Option<Ptr>,
    target: Option<Ptr>,
}

impl<'f> Ref<'f> {
    pub fn is_null(&self) -> bool {
        self.target.is_none()
    }

    pub fn target(&self) -> Option<Ptr> {
        self.target
    }

    pub fn type_ptr(&self) -> Option<Ptr> {
        self.ty
    }

    pub fn get(&self) -> Result<Option<Object<'f>>, GrannyError> {
        let Some(target) = self.target else {
            return Ok(None);
        };
        let ty = self.ty.ok_or(GrannyError::MissingType("reference"))?;
        Object::new(self.file, self.file.type_at(ty)?, target).map(Some)
    }
}

/// Unresolved `{count, pointer}` array.
#[derive(Debug, Clone, Copy)]
pub struct Array<'f> {
    file: &'f GrannyFile<'f>,
    ty: Option<Ptr>,
    count: i32,
    data: Option<Ptr>,
}

impl<'f> Array<'f> {
    /// Stored element count (may be negative in corrupt files; `resolve` rejects that).
    pub fn count(&self) -> i32 {
        self.count
    }

    pub fn data(&self) -> Option<Ptr> {
        self.data
    }

    /// Resolves element type and checks that all elements are inside their section.
    pub fn resolve(&self) -> Result<ArrayData<'f>, GrannyError> {
        let len =
            usize::try_from(self.count).map_err(|_| GrannyError::NegativeCount(self.count))?;
        if len == 0 {
            return Ok(ArrayData {
                file: self.file,
                ty: None,
                at: self.data,
                bytes: &[],
                len: 0,
            });
        }
        let at = self
            .data
            .ok_or(GrannyError::NullReference("array data".into()))?;
        let ty = self
            .file
            .type_at(self.ty.ok_or(GrannyError::MissingType("array"))?)?;
        let total = (ty.size as usize)
            .checked_mul(len)
            .ok_or(GrannyError::SizeOverflow("array"))?;
        let bytes = self.file.bytes(at, total)?;
        Ok(ArrayData {
            file: self.file,
            ty: Some(ty),
            at: Some(at),
            bytes,
            len,
        })
    }
}

/// A resolved contiguous array; elements are views into `bytes`.
#[derive(Debug, Clone)]
pub struct ArrayData<'f> {
    file: &'f GrannyFile<'f>,
    ty: Option<Arc<TypeDef>>,
    at: Option<Ptr>,
    bytes: &'f [u8],
    len: usize,
}

impl<'f> ArrayData<'f> {
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Element type; `None` only for empty arrays.
    pub fn element_type(&self) -> Option<&Arc<TypeDef>> {
        self.ty.as_ref()
    }

    pub fn stride(&self) -> usize {
        self.ty.as_ref().map_or(0, |t| t.size as usize)
    }

    /// All element bytes (`len * stride`), e.g. for vertex buffers and curve knots.
    pub fn bytes(&self) -> &'f [u8] {
        self.bytes
    }

    pub fn at(&self) -> Option<Ptr> {
        self.at
    }

    pub fn get(&self, i: usize) -> Option<Object<'f>> {
        let ty = self.ty.as_ref()?;
        let stride = ty.size as usize;
        if i >= self.len {
            return None;
        }
        let start = i.checked_mul(stride)?;
        let bytes = self.bytes.get(start..start.checked_add(stride)?)?;
        Some(Object {
            file: self.file,
            ty: ty.clone(),
            at: self.at?.offset_by(u32::try_from(start).ok()?)?,
            bytes,
        })
    }

    pub fn iter(&self) -> impl Iterator<Item = Object<'f>> + '_ {
        (0..self.len).filter_map(move |i| self.get(i))
    }
}

/// `{count, pointer to pointers}` array; elements may be null.
#[derive(Debug, Clone, Copy)]
pub struct RefArray<'f> {
    file: &'f GrannyFile<'f>,
    ty: Option<Ptr>,
    count: i32,
    data: Option<Ptr>,
}

impl<'f> RefArray<'f> {
    pub fn is_empty(&self) -> bool {
        self.count <= 0
    }

    /// Element count; errors on a negative count or a table outside its section.
    pub fn len(&self) -> Result<usize, GrannyError> {
        let len =
            usize::try_from(self.count).map_err(|_| GrannyError::NegativeCount(self.count))?;
        if len > 0 {
            let at = self
                .data
                .ok_or(GrannyError::NullReference("reference array".into()))?;
            let total = len
                .checked_mul(4)
                .ok_or(GrannyError::SizeOverflow("array"))?;
            self.file.bytes(at, total)?;
        }
        Ok(len)
    }

    /// Element `i`; `Ok(None)` for a null entry.
    pub fn get(&self, i: usize) -> Result<Option<Object<'f>>, GrannyError> {
        let at = self
            .data
            .ok_or(GrannyError::NullReference("reference array".into()))?;
        let slot = u32::try_from(i)
            .ok()
            .and_then(|i| i.checked_mul(4))
            .and_then(|o| at.offset_by(o))
            .ok_or(GrannyError::SizeOverflow("array"))?;
        // Validates the slot itself, not just the fixup lookup.
        self.file.bytes(slot, 4)?;
        Ref {
            file: self.file,
            ty: self.ty,
            target: self.file.pointer_at(slot),
        }
        .get()
    }

    pub fn iter(
        &self,
    ) -> Result<impl Iterator<Item = Result<Option<Object<'f>>, GrannyError>> + '_, GrannyError>
    {
        let n = self.len()?;
        Ok((0..n).map(move |i| self.get(i)))
    }
}

/// A (possibly null) pointer to a NUL-terminated string.
#[derive(Debug, Clone, Copy)]
pub struct Str<'f> {
    file: &'f GrannyFile<'f>,
    target: Option<Ptr>,
}

impl<'f> Str<'f> {
    pub fn get(&self) -> Result<Option<&'f [u8]>, GrannyError> {
        self.target.map(|p| self.file.c_string(p)).transpose()
    }

    pub fn to_string_lossy(&self) -> Result<Option<String>, GrannyError> {
        Ok(self.get()?.map(latin1))
    }
}

/// A member with a fixed array width.
#[derive(Debug, Clone)]
pub struct FixedArray<'f> {
    file: &'f GrannyFile<'f>,
    member: Member,
    at: Ptr,
    bytes: &'f [u8],
}

impl<'f> FixedArray<'f> {
    pub fn len(&self) -> usize {
        self.member.array_width as usize
    }

    pub fn is_empty(&self) -> bool {
        self.member.array_width == 0
    }

    pub fn kind(&self) -> MemberKind {
        self.member.kind
    }

    pub fn bytes(&self) -> &'f [u8] {
        self.bytes
    }

    pub fn get(&self, i: usize) -> Option<Value<'f>> {
        if i >= self.len() {
            return None;
        }
        let size = self.member.element_size as usize;
        let start = i.checked_mul(size)?;
        let b = self.bytes.get(start..start.checked_add(size)?)?;
        let at = self.at.offset_by(u32::try_from(start).ok()?)?;
        Some(element(self.file, &self.member, at, b))
    }

    pub fn iter(&self) -> impl Iterator<Item = Value<'f>> + '_ {
        (0..self.len()).filter_map(move |i| self.get(i))
    }
}
