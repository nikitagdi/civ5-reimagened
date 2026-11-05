//! Tests on synthetic gr2 files built by a minimal writer.

use crate::*;

/// Builds raw (uncompressed) 32-bit LE revision-7 gr2 files.
#[derive(Default)]
pub(crate) struct Writer {
    secs: Vec<Sec>,
}

#[derive(Default)]
pub(crate) struct Sec {
    data: Vec<u8>,
    /// (source offset, target section, target offset)
    fixups: Vec<[u32; 3]>,
}

pub(crate) struct Def<'a> {
    kind: u32,
    name: &'a str,
    hashed: bool,
    child: Option<Ptr>,
    width: i32,
}

pub(crate) fn def(kind: u32, name: &str) -> Def<'_> {
    Def {
        kind,
        name,
        hashed: false,
        child: None,
        width: 0,
    }
}

impl Def<'_> {
    pub(crate) fn child(mut self, p: Ptr) -> Self {
        self.child = Some(p);
        self
    }
    pub(crate) fn width(mut self, w: i32) -> Self {
        self.width = w;
        self
    }
    pub(crate) fn hashed(mut self) -> Self {
        self.hashed = true;
        self
    }
}

impl Writer {
    pub(crate) fn new(n: usize) -> Self {
        Self {
            secs: (0..n).map(|_| Sec::default()).collect(),
        }
    }

    pub(crate) fn pos(&self, s: u32) -> Ptr {
        Ptr {
            section: s,
            offset: self.secs[s as usize].data.len() as u32,
        }
    }

    pub(crate) fn bytes(&mut self, s: u32, b: &[u8]) -> Ptr {
        let p = self.pos(s);
        self.secs[s as usize].data.extend_from_slice(b);
        p
    }

    pub(crate) fn u32(&mut self, s: u32, v: u32) -> Ptr {
        self.bytes(s, &v.to_le_bytes())
    }

    pub(crate) fn ptr(&mut self, s: u32, target: Option<Ptr>) -> Ptr {
        let p = self.u32(s, 0);
        if let Some(t) = target {
            self.secs[s as usize]
                .fixups
                .push([p.offset, t.section, t.offset]);
        }
        p
    }

    pub(crate) fn cstr(&mut self, s: u32, text: &str) -> Ptr {
        let p = self.bytes(s, text.as_bytes());
        self.bytes(s, &[0]);
        while !self.secs[s as usize].data.len().is_multiple_of(4) {
            self.bytes(s, &[0]);
        }
        p
    }

    pub(crate) fn typedef(&mut self, s: u32, members: &[Def]) -> Ptr {
        let names: Vec<_> = members
            .iter()
            .map(|m| (!m.hashed).then(|| self.cstr(s, m.name)))
            .collect();
        let at = self.pos(s);
        for (m, name) in members.iter().zip(names) {
            self.u32(s, m.kind);
            match name {
                Some(p) => self.ptr(s, Some(p)),
                None => self.u32(s, name_hash(m.name)),
            };
            self.ptr(s, m.child);
            self.u32(s, m.width as u32);
            self.bytes(s, &[0; 16]);
        }
        self.bytes(s, &[0; 32]);
        at
    }

    pub(crate) fn build(&self, root_type: Ptr, root: Ptr) -> Vec<u8> {
        let n = self.secs.len();
        let header_size = 32 + 72 + 44 * n;
        let mut offsets = Vec::new();
        let mut pos = header_size;
        for s in &self.secs {
            offsets.push(pos);
            pos += s.data.len().next_multiple_of(4);
        }
        let mut fix_offsets = Vec::new();
        for s in &self.secs {
            fix_offsets.push(pos);
            pos += 12 * s.fixups.len();
        }
        let mut out = Vec::with_capacity(pos);
        let w = |out: &mut Vec<u8>, v: u32| out.extend_from_slice(&v.to_le_bytes());
        out.extend_from_slice(&MAGIC_LE32);
        w(&mut out, header_size as u32);
        out.resize(32, 0);
        for v in [7, pos as u32, 0, 72, n as u32] {
            w(&mut out, v);
        }
        for v in [
            root_type.section,
            root_type.offset,
            root.section,
            root.offset,
        ] {
            w(&mut out, v);
        }
        out.resize(32 + 72, 0);
        for (i, s) in self.secs.iter().enumerate() {
            let len = s.data.len() as u32;
            for v in [
                0,
                offsets[i] as u32,
                len,
                len,
                4,
                len,
                len,
                fix_offsets[i] as u32,
                s.fixups.len() as u32,
                0,
                0,
            ] {
                w(&mut out, v);
            }
        }
        for s in &self.secs {
            out.extend_from_slice(&s.data);
            out.resize(out.len().next_multiple_of(4), 0);
        }
        for s in &self.secs {
            for f in &s.fixups {
                for &v in f {
                    w(&mut out, v);
                }
            }
        }
        out
    }
}

// Granny member type codes.
pub(crate) const INLINE: u32 = 1;
pub(crate) const REFERENCE: u32 = 2;
pub(crate) const REF_TO_ARRAY: u32 = 3;
pub(crate) const ARRAY_OF_REFS: u32 = 4;
pub(crate) const VARIANT: u32 = 5;
pub(crate) const REF_TO_VARIANT_ARRAY: u32 = 7;
pub(crate) const STRING: u32 = 8;
pub(crate) const TRANSFORM: u32 = 9;
pub(crate) const REAL32: u32 = 10;
pub(crate) const INT8: u32 = 11;
pub(crate) const UINT8: u32 = 12;
pub(crate) const INT16: u32 = 15;
pub(crate) const UINT16: u32 = 16;
pub(crate) const INT32: u32 = 19;
pub(crate) const UINT32: u32 = 20;
pub(crate) const REAL16: u32 = 21;
pub(crate) const EMPTY_REF: u32 = 22;

/// A file exercising every member kind; types live in section 1, data in sections 0 and 2.
fn sample() -> Vec<u8> {
    let mut w = Writer::new(3);
    let bone_t = w.typedef(
        1,
        &[def(STRING, "Name"), def(INT32, "ParentIndex").hashed()],
    );
    let vec3_t = w.typedef(1, &[def(REAL32, "Real32").width(3)]);
    let curve_t = w.typedef(1, &[def(UINT8, "Format"), def(UINT8, "Degree")]);
    let root_t = w.typedef(
        1,
        &[
            def(STRING, "Name"),
            def(REAL32, "Duration").hashed(),
            def(INT8, "I8"),
            def(UINT8, "U8"),
            def(INT16, "I16"),
            def(UINT16, "U16"),
            def(INT32, "I32"),
            def(UINT32, "U32"),
            def(REAL16, "Half"),
            def(TRANSFORM, "Transform"),
            def(REAL32, "Origin").width(3),
            def(INLINE, "Inline").child(vec3_t),
            def(REFERENCE, "Skeleton").child(bone_t),
            def(REFERENCE, "Missing").child(bone_t),
            def(REF_TO_ARRAY, "Bones").child(bone_t).hashed(),
            def(ARRAY_OF_REFS, "Meshes").child(bone_t),
            def(VARIANT, "CurveData"),
            def(REF_TO_VARIANT_ARRAY, "Curves"),
            def(EMPTY_REF, "Empty"),
            def(UINT32, "MysteryField").hashed(),
        ],
    );

    // Data objects in section 2.
    let bone_name = w.cstr(2, "root_bone");
    let bones = w.pos(2);
    for (name, parent) in [(Some(bone_name), -1i32), (None, 0)] {
        w.ptr(2, name);
        w.u32(2, parent as u32);
    }
    let refs = w.pos(2);
    w.ptr(2, Some(bones));
    w.ptr(2, None);
    let curve = w.bytes(2, &[18, 3]);
    let curves = w.bytes(2, &[1, 2, 3, 4]);

    // Root in section 0.
    let name = w.cstr(0, "root");
    let root = w.pos(0);
    w.ptr(0, Some(name));
    w.bytes(0, &2.5f32.to_le_bytes());
    w.bytes(0, &[0xFE, 200]);
    w.bytes(0, &(-300i16).to_le_bytes());
    w.bytes(0, &60000u16.to_le_bytes());
    w.u32(0, -7i32 as u32);
    w.u32(0, 0xDEAD_BEEF);
    w.bytes(0, &0x3C00u16.to_le_bytes());
    w.u32(0, 3);
    for f in [
        1.0f32, 2.0, 3.0, 0.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0,
    ] {
        w.bytes(0, &f.to_le_bytes());
    }
    for f in [4.0f32, 5.0, 6.0, 7.0, 8.0, 9.0] {
        w.bytes(0, &f.to_le_bytes());
    }
    w.ptr(0, Some(bones));
    w.ptr(0, None);
    w.u32(0, 2);
    w.ptr(0, Some(bones));
    w.u32(0, 2);
    w.ptr(0, Some(refs));
    w.ptr(0, Some(curve_t));
    w.ptr(0, Some(curve));
    w.ptr(0, Some(curve_t));
    w.u32(0, 2);
    w.ptr(0, Some(curves));
    w.u32(0, 0);
    w.u32(0, 42);
    w.build(root_t, root)
}

#[test]
fn reads_every_member_kind() {
    let data = sample();
    let f = GrannyFile::parse(&data).unwrap();
    let r = f.root().unwrap();
    assert_eq!(r.string("Name").unwrap().as_deref(), Some("root"));
    assert_eq!(r.f32("Duration").unwrap(), 2.5);
    assert!(matches!(r.field("I8"), Some(Value::Int8(-2))));
    assert!(matches!(r.field("U8"), Some(Value::UInt8(200))));
    assert!(matches!(r.field("I16"), Some(Value::Int16(-300))));
    assert!(matches!(r.field("U16"), Some(Value::UInt16(60000))));
    assert_eq!(r.i32("I32").unwrap(), -7);
    assert_eq!(r.u32("U32").unwrap(), 0xDEAD_BEEF);
    assert!(matches!(r.field("Half"), Some(Value::Real16(0x3C00))));
    let t = r.transform("Transform").unwrap();
    assert_eq!((t.flags, t.position), (3, [1.0, 2.0, 3.0]));
    assert_eq!(t.orientation, [0.0, 0.0, 0.0, 1.0]);
    assert_eq!(t.scale_shear[2], [0.0, 0.0, 1.0]);

    let Some(Value::Fixed(origin)) = r.field("Origin") else {
        panic!("fixed array")
    };
    let v: Vec<_> = origin
        .iter()
        .map(|v| match v {
            Value::Real32(x) => x,
            _ => f32::NAN,
        })
        .collect();
    assert_eq!(v, [4.0, 5.0, 6.0]);
    let inline = r.inline("Inline").unwrap();
    assert_eq!(
        inline.bytes(),
        &[7f32, 8., 9.].map(f32::to_le_bytes).concat()[..]
    );

    let skel = r.reference("Skeleton").unwrap().unwrap();
    assert_eq!(skel.at().section, 2);
    assert_eq!(skel.string("Name").unwrap().as_deref(), Some("root_bone"));
    assert!(r.reference("Missing").unwrap().is_none());

    let bones = r.array("Bones").unwrap();
    assert_eq!((bones.len(), bones.stride()), (2, 8));
    let parents: Vec<_> = bones
        .iter()
        .map(|b| b.i32("ParentIndex").unwrap())
        .collect();
    assert_eq!(parents, [-1, 0]);
    assert_eq!(bones.get(1).unwrap().string("Name").unwrap(), None);

    let meshes = r.references("Meshes").unwrap();
    assert_eq!(meshes.len().unwrap(), 2);
    assert!(meshes.get(0).unwrap().is_some());
    assert!(meshes.get(1).unwrap().is_none());

    let curve = r.reference("CurveData").unwrap().unwrap();
    assert_eq!(curve.u32("Format").unwrap(), 18);
    let curves = r.array("Curves").unwrap();
    assert_eq!(curves.bytes(), &[1, 2, 3, 4]);
    assert_eq!(curves.get(1).unwrap().u32("Degree").unwrap(), 4);
    assert!(matches!(r.field("Empty"), Some(Value::EmptyReference)));

    let names: Vec<_> = r.fields().map(|(m, _)| m.name.to_string()).collect();
    assert_eq!(names.len(), 20);
    assert_eq!(names[19], format!("#{:08x}", name_hash("MysteryField")));
}

#[test]
fn hashed_names_resolve_through_dictionary() {
    let data = sample();
    let f = GrannyFile::parse(&data).unwrap();
    let ty = f.root_type().unwrap();
    let (_, d) = ty.member("Duration").unwrap();
    assert_eq!(
        (&*d.name, d.name_source),
        ("Duration", NameSource::Dictionary)
    );
    let (_, n) = ty.member("Name").unwrap();
    assert_eq!(n.name_source, NameSource::Stored);
    let (_, u) = ty.member("MysteryField").unwrap();
    assert_eq!(u.name_source, NameSource::Unknown);
    assert_eq!(f.root().unwrap().u32("MysteryField").unwrap(), 42);
}

#[test]
fn walk_visits_whole_graph() {
    let data = sample();
    let f = GrannyFile::parse(&data).unwrap();
    let s = f.walk().unwrap();
    assert!(s.objects >= 4, "{s:?}");
    assert!(s.strings >= 2, "{s:?}");
    assert_eq!(s.unknown_names, 1);
    assert_eq!(s.types, 4);
}

#[test]
fn typed_getters_report_mismatch_and_missing() {
    let data = sample();
    let f = GrannyFile::parse(&data).unwrap();
    let r = f.root().unwrap();
    assert!(matches!(
        r.f32("Name"),
        Err(GrannyError::TypeMismatch { .. })
    ));
    assert!(matches!(r.i32("Nope"), Err(GrannyError::MissingField(_))));
    assert!(matches!(
        r.i32("U32"),
        Err(GrannyError::TypeMismatch { .. })
    ));
}

#[test]
fn rejects_other_magics_and_revisions() {
    let mut data = sample();
    data[0] ^= 1;
    assert_eq!(GrannyFile::parse(&data).unwrap_err(), GrannyError::BadMagic);
    let le64 = [
        0xE5, 0x9B, 0x49, 0x5E, 0x6F, 0x63, 0x1F, 0x14, 0x1E, 0x13, 0xEB, 0xA9, 0x90, 0xBE, 0xED,
        0xC4,
    ];
    data[..16].copy_from_slice(&le64);
    assert!(matches!(
        GrannyFile::parse(&data),
        Err(GrannyError::UnsupportedVariant(_))
    ));
    let mut data = sample();
    data[32] = 6;
    assert_eq!(
        GrannyFile::parse(&data).unwrap_err(),
        GrannyError::UnsupportedRevision(6)
    );
}

#[test]
fn every_truncation_is_an_error() {
    let data = sample();
    for len in 0..data.len() {
        assert!(GrannyFile::parse(&data[..len]).is_err(), "len {len}");
    }
}

#[test]
fn bad_fixups_are_rejected() {
    let mut w = Writer::new(1);
    let t = w.typedef(0, &[def(UINT32, "A")]);
    let root = w.u32(0, 1);
    w.secs[0].fixups.push([1000, 0, 0]);
    let data = w.build(t, root);
    assert!(matches!(
        GrannyFile::parse(&data),
        Err(GrannyError::BadFixup { .. })
    ));
    w.secs[0].fixups[0] = [0, 5, 0];
    assert!(GrannyFile::parse(&w.build(t, root)).is_err());
    w.secs[0].fixups[0] = [0, 0, 100_000];
    assert!(GrannyFile::parse(&w.build(t, root)).is_err());
}

#[test]
fn unsupported_compression_is_rejected() {
    let mut data = sample();
    // Compression field of section 0 (section table starts at 32 + 72).
    data[104] = 1;
    assert_eq!(
        GrannyFile::parse(&data).unwrap_err(),
        GrannyError::UnsupportedCompression {
            section: 0,
            kind: 1
        }
    );
}

#[test]
fn malformed_types_are_errors() {
    // Unknown member kind.
    let mut w = Writer::new(1);
    let t = w.typedef(0, &[def(6, "Removed")]);
    let root = w.u32(0, 0);
    let data = w.build(t, root);
    let f = GrannyFile::parse(&data).unwrap();
    assert!(matches!(
        f.root(),
        Err(GrannyError::UnknownMemberType { kind: 6, .. })
    ));

    // An inline member containing its own type.
    let mut w = Writer::new(1);
    let at = w.pos(0);
    w.typedef(0, &[def(INLINE, "Me").child(at).hashed()]);
    let root = w.u32(0, 0);
    let data = w.build(at, root);
    let f = GrannyFile::parse(&data).unwrap();
    assert!(matches!(f.root(), Err(GrannyError::TypeTooDeep(_))));

    // Type record running off the section end.
    let mut w = Writer::new(1);
    let root = w.u32(0, 0);
    let t = w.u32(0, UINT32);
    let data = w.build(t, root);
    let f = GrannyFile::parse(&data).unwrap();
    assert!(matches!(f.root(), Err(GrannyError::OutOfBounds(_))));
}

#[test]
fn bad_arrays_and_strings_are_errors() {
    let mut w = Writer::new(1);
    let elem = w.typedef(0, &[def(UINT32, "X")]);
    let t = w.typedef(
        0,
        &[
            def(REF_TO_ARRAY, "Neg").child(elem),
            def(REF_TO_ARRAY, "Huge").child(elem),
        ],
    );
    let root = w.pos(0);
    w.u32(0, -1i32 as u32);
    w.ptr(0, Some(root));
    w.u32(0, 1_000_000);
    w.ptr(0, Some(root));
    let data = w.build(t, root);
    let f = GrannyFile::parse(&data).unwrap();
    let r = f.root().unwrap();
    assert_eq!(r.array("Neg").unwrap_err(), GrannyError::NegativeCount(-1));
    assert!(matches!(r.array("Huge"), Err(GrannyError::BadPointer(_))));
    assert!(f.walk().is_err());
    let mut w = Writer::new(1);
    let t = w.typedef(0, &[def(STRING, "Str")]);
    let root = w.pos(0);
    let end = Ptr {
        section: 0,
        offset: root.offset + 4,
    };
    w.ptr(0, Some(end));
    w.bytes(0, b"abcd");
    let data = w.build(t, root);
    let f = GrannyFile::parse(&data).unwrap();
    assert!(matches!(
        f.root().unwrap().string("Str"),
        Err(GrannyError::UnterminatedString(_))
    ));
}

#[test]
fn corrupted_bytes_never_panic() {
    let data = sample();
    let mut seed = 0x9E37_79B9u32;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    for _ in 0..3000 {
        let mut d = data.clone();
        for _ in 0..1 + next() % 4 {
            let i = next() as usize % d.len();
            d[i] = next() as u8;
        }
        if let Ok(f) = GrannyFile::parse(&d) {
            let _ = f.walk();
        }
    }
}

// FireGrafixMaterial (a material's ExtendedData), see `extended.rs`.

fn d(stripped: bool, kind: u32, name: &'static str) -> Def<'static> {
    let m = def(kind, name);
    if stripped { m.hashed() } else { m }
}

/// Text of a string slot written ahead of the object holding the slot (stored files only).
fn text(w: &mut Writer, s: &str, stripped: bool) -> Option<Ptr> {
    (!stripped).then(|| w.cstr(2, s))
}

/// One string slot: a pointer to the stored text or, stripped, `name_hash` in its place.
/// `None` is a null string.
fn slot(w: &mut Writer, s: Option<&str>, stored: Option<Ptr>, stripped: bool) {
    match (s, stored) {
        (Some(s), None) if stripped => w.u32(2, name_hash(s)),
        (_, p) => w.ptr(2, p),
    };
}

/// A run of string slots, with their texts.
fn slots(w: &mut Writer, strings: &[Option<&str>], stripped: bool) -> Ptr {
    let stored: Vec<_> = strings
        .iter()
        .map(|s| s.and_then(|s| text(w, s, stripped)))
        .collect();
    let at = w.pos(2);
    for (s, p) in strings.iter().zip(stored) {
        slot(w, *s, p, stripped);
    }
    at
}

const FGX_UNKNOWN_PARAM: &str = "NotInAnyDictionary";
const FGX_TEXTURE: &str = "D:\\art\\Units\\a_DIFF.dds";

/// Two materials: the first with a `FireGrafixMaterial` block of `version` and one texture
/// `ParameterSet0`, the second without `ExtendedData`.
fn fgx_file(stripped: bool, version: i32) -> Vec<u8> {
    let s = stripped;
    let mut w = Writer::new(3);
    let str_t = w.typedef(1, &[d(s, STRING, "String")]);
    let i32_t = w.typedef(1, &[d(s, INT32, "Int32")]);
    let f32_t = w.typedef(1, &[d(s, REAL32, "Real32")]);
    let set_t = w.typedef(
        1,
        &[
            d(s, STRING, "ParamBlock"),
            d(s, INT32, "ParamCount"),
            d(s, REF_TO_ARRAY, "ParamNames").child(str_t),
            d(s, REF_TO_ARRAY, "ParamTypes").child(i32_t),
            d(s, REF_TO_ARRAY, "ParamDataF").child(f32_t),
            d(s, REF_TO_ARRAY, "ParamDataT").child(str_t),
            d(s, INT32, "ParamActive"),
        ],
    );
    let fgx_t = w.typedef(
        1,
        &[
            d(s, STRING, "ShaderSet"),
            d(s, REF_TO_ARRAY, "FallbackMtl").child(str_t),
            d(s, INT32, "AlphaMode"),
            d(s, INT32, "ZMode"),
            d(s, INT32, "AlphaRef"),
            d(s, INT32, "SkinBoneCount"),
            d(s, INT32, "Version"),
            d(s, INT32, "Z-Fail"),
            d(s, STRING, "typeName"),
            d(s, INT32, "ParameterSetCount"),
            d(s, REFERENCE, "ParameterSet0").child(set_t),
        ],
    );
    let mat_t = w.typedef(1, &[d(s, STRING, "Name"), d(s, VARIANT, "ExtendedData")]);
    let root_t = w.typedef(1, &[def(ARRAY_OF_REFS, "Materials").child(mat_t)]);

    let names = slots(&mut w, &[Some("Diffuse"), Some(FGX_UNKNOWN_PARAM)], s);
    let types = w.pos(2);
    for t in [6u32, 7] {
        w.u32(2, t);
    }
    let floats = w.pos(2);
    for f in [0.5f32, -2.0, 8.0] {
        w.bytes(2, &f.to_le_bytes());
    }
    let paths = slots(&mut w, &[Some(FGX_TEXTURE), None, None, None], s);
    let fallback = slots(&mut w, &[None], s);

    let block = text(&mut w, "BuildingTextures", s);
    let set = w.pos(2);
    slot(&mut w, Some("BuildingTextures"), block, s);
    w.u32(2, 2);
    for (n, at) in [(2, names), (2, types), (3, floats), (4, paths)] {
        w.u32(2, n);
        w.ptr(2, Some(at));
    }
    w.u32(2, 1);

    let shader = text(&mut w, "BuildingShader_Skinned", s);
    let type_name = text(&mut w, "FireGrafixMaterial", s);
    let ext = w.pos(2);
    slot(&mut w, Some("BuildingShader_Skinned"), shader, s);
    w.u32(2, 1);
    w.ptr(2, Some(fallback));
    for v in [8, 4, 128, 32, version, 0] {
        w.u32(2, v as u32);
    }
    slot(&mut w, Some("FireGrafixMaterial"), type_name, s);
    w.u32(2, 1);
    w.ptr(2, Some(set));

    let name0 = text(&mut w, "Game Cow 01", s);
    let name1 = text(&mut w, "Plain", s);
    let mat0 = w.pos(2);
    slot(&mut w, Some("Game Cow 01"), name0, s);
    w.ptr(2, Some(fgx_t));
    w.ptr(2, Some(ext));
    let mat1 = w.pos(2);
    slot(&mut w, Some("Plain"), name1, s);
    w.ptr(2, None);
    w.ptr(2, None);
    let refs = w.pos(2);
    w.ptr(2, Some(mat0));
    w.ptr(2, Some(mat1));

    let root = w.u32(0, 2);
    w.ptr(0, Some(refs));
    w.build(root_t, root)
}

#[test]
fn reads_fgx_block_stored_and_stripped() {
    use crate::extended::read_fgx_materials;
    for stripped in [false, true] {
        let data = fgx_file(stripped, 4);
        let f = GrannyFile::parse(&data).unwrap();
        let mats = read_fgx_materials(&f).unwrap();
        assert_eq!(mats.len(), 2);
        assert!(mats[1].is_none(), "a material without ExtendedData");
        let m = mats[0].as_ref().unwrap();

        assert_eq!(m.shader_set.resolved(), Some("BuildingShader_Skinned"));
        assert_eq!(m.shader_set.text.is_none(), stripped);
        if stripped {
            assert_eq!(m.shader_set.id, Some(name_hash("BuildingShader_Skinned")));
        }
        assert_eq!(
            (
                m.version,
                m.alpha_mode,
                m.z_mode,
                m.alpha_ref,
                m.skin_bone_count
            ),
            (4, 8, 4, 128, 32)
        );
        assert_eq!((m.z_fail, m.parameter_set_count), (0, 1));
        assert_eq!(m.fallback_mtl.len(), 1);
        assert!(m.fallback_mtl[0].is_empty(), "null fallback");

        assert_eq!(m.present_sets().count(), 1);
        assert!(m.sets[1..].iter().all(Option::is_none));
        let set = m.sets[0].as_ref().unwrap();
        assert_eq!(set.block.resolved(), Some("BuildingTextures"));
        assert_eq!((set.count, set.active), (2, 1));
        assert_eq!(set.types, [6, 7]);
        assert_eq!(set.data_f, [0.5, -2.0, 8.0]);
        assert_eq!(set.names[0].display().as_deref(), Some("Diffuse"));
        assert!(!set.names[0].is_unresolved());
        // A name outside the dictionary: kept as text, or as `#hash` when stripped.
        let unknown = format!("#{:08x}", name_hash(FGX_UNKNOWN_PARAM));
        assert_eq!(set.names[1].is_unresolved(), stripped);
        assert_eq!(
            set.names[1].display().as_deref(),
            Some(if stripped {
                &unknown
            } else {
                FGX_UNKNOWN_PARAM
            })
        );
        // Texture paths: four slots per texture, unused ones null.
        assert_eq!(set.data_t.len(), 4);
        assert_eq!(set.data_t[0].text.is_some(), !stripped);
        assert_eq!(set.data_t[0].id.is_some(), stripped);
        if stripped {
            assert_eq!(set.data_t[0].id, Some(name_hash(FGX_TEXTURE)));
        }
        assert!(
            set.data_t[1..]
                .iter()
                .all(crate::material::NameRef::is_empty)
        );
        assert_eq!(m.strings().count(), 1 + 1 + 1 + 2 + 4);
    }
}

#[test]
fn fgx_rejects_unknown_version() {
    use crate::extended::read_fgx_materials;
    for v in [1, 5, -1] {
        let data = fgx_file(true, v);
        let f = GrannyFile::parse(&data).unwrap();
        assert!(
            matches!(read_fgx_materials(&f), Err(GrannyError::Invalid(m)) if m.contains("version")),
            "version {v}"
        );
    }
}

#[test]
fn fgx_hash_is_inverted_crc32() {
    for n in ["ShaderSet", "BuildingShader_Skinned", "ParamDataT", ""] {
        assert_eq!(name_hash(n), crc32(n.as_bytes()) ^ 0xFFFF_FFFF, "{n}");
    }
    // The positional names of the stripped parameter members (stripped files store crc32 hashes in place of names).
    for (n, h) in [
        ("ParamBlock", 0x1d5f_0625),
        ("ParamCount", 0x1b9d_d565),
        ("ParamNames", 0x6c77_d847),
        ("ParamTypes", 0xc774_1837),
        ("ParamUsage", 0x4eaf_cf77),
        ("ParamRegs", 0x62b0_b707),
        ("ParamSize", 0xd8ae_f04b),
        ("ParamSemantics", 0x5f80_a331),
        ("ParamStoreIdx", 0xea73_a40a),
        ("ParamDataF", 0x07ef_593e),
        ("ParamDataI", 0x9750_44af),
        ("ParamDataT", 0xf456_2876),
        ("ParamActive", 0x07e6_c2bd),
        ("ParamFlags", 0x9541_d0bd),
    ] {
        assert_eq!(name_hash(n), h, "{n}");
        assert_eq!(fgx_name(h), Some(n));
    }
    assert_eq!(fgx_name(name_hash("")), Some(""));
}

#[test]
fn corrupted_fgx_never_panics() {
    use crate::extended::read_fgx_materials;
    let data = fgx_file(true, 4);
    let mut seed = 0x1234_5679u32;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    for _ in 0..3000 {
        let mut d = data.clone();
        for _ in 0..1 + next() % 4 {
            let i = next() as usize % d.len();
            d[i] = next() as u8;
        }
        if let Ok(f) = GrannyFile::parse(&d) {
            let _ = read_fgx_materials(&f);
        }
    }
}
