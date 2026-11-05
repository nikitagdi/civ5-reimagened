//! Asset manifest: logical id → variants per device profile.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{ContentHash, FormatError, LogicalId, blob};

pub const MANIFEST_MAGIC: [u8; 4] = *b"C5MF";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum VariantFormat {
    /// Device-independent (mesh, font, data).
    Generic,
    /// Uncompressed RGBA8; works everywhere but large.
    Uncompressed,
    Bc,
    Astc,
    Etc2,
}

/// 0 means "do not prefetch".
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize,
)]
pub struct PrefetchGroup(pub u16);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Variant {
    pub format: VariantFormat,
    /// The blob is stored at `/assets/<hash>`.
    pub hash: ContentHash,
    pub size: u64,
    pub prefetch: PrefetchGroup,
}

/// Device capabilities used to pick a variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformProfile {
    /// Most preferred first. `Generic` is always usable; `Uncompressed` is an implicit fallback.
    pub texture_formats: Vec<VariantFormat>,
}

impl PlatformProfile {
    pub fn new(texture_formats: Vec<VariantFormat>) -> Self {
        Self { texture_formats }
    }

    pub fn uncompressed_only() -> Self {
        Self::new(vec![VariantFormat::Uncompressed])
    }

    /// A desktop GPU (`TEXTURE_COMPRESSION_BC`): the BC blocks, then the plain fallback.
    pub fn bc_device() -> Self {
        Self::new(vec![VariantFormat::Bc, VariantFormat::Uncompressed])
    }

    /// A phone (`TEXTURE_COMPRESSION_ASTC`, no BC): ASTC, then the plain fallback, and BC last: the
    /// client decodes BC1-BC5 on the CPU, which is what a texture without the other two gets
    /// (every texture has an `Astc` variant or a written exception).
    pub fn astc_device() -> Self {
        Self::new(vec![
            VariantFormat::Astc,
            VariantFormat::Uncompressed,
            VariantFormat::Bc,
        ])
    }

    /// A device with neither (WebGL2 on some GPUs): the plain fallback, then BC decoded on the CPU.
    pub fn no_compression() -> Self {
        Self::new(vec![VariantFormat::Uncompressed, VariantFormat::Bc])
    }

    /// Lower is better; `None` means unsupported.
    fn rank(&self, format: VariantFormat) -> Option<usize> {
        match format {
            VariantFormat::Generic => Some(0),
            f => self
                .texture_formats
                .iter()
                .position(|&x| x == f)
                .map(|i| i + 1)
                .or((f == VariantFormat::Uncompressed).then_some(usize::MAX)),
        }
    }
}

/// Logical id → variants.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manifest {
    entries: HashMap<LogicalId, Vec<Variant>>,
}

impl Manifest {
    pub const VERSION: u16 = 1;
    pub const MIN_VERSION: u16 = 1;

    pub fn new() -> Self {
        Self::default()
    }

    /// Deterministic: entries sorted by id, variants by (format, hash), regardless of insert order.
    pub fn encode(&self) -> Vec<u8> {
        let mut entries: Vec<(&LogicalId, Vec<&Variant>)> = self
            .entries
            .iter()
            .map(|(id, vs)| {
                let mut vs: Vec<&Variant> = vs.iter().collect();
                vs.sort_by_key(|v| (v.format, v.hash));
                (id, vs)
            })
            .collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        blob::encode_v(MANIFEST_MAGIC, Self::VERSION, &entries)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, FormatError> {
        let (_, entries): (u16, Vec<(LogicalId, Vec<Variant>)>) =
            blob::decode_range(MANIFEST_MAGIC, Self::MIN_VERSION, Self::VERSION, bytes)?;
        let mut m = Self::new();
        for (id, variants) in entries {
            if m.entries.insert(id.clone(), variants).is_some() {
                return Err(FormatError::invalid(format!(
                    "duplicate manifest id {}",
                    id.as_str()
                )));
            }
        }
        Ok(m)
    }

    pub fn insert(&mut self, id: LogicalId, variant: Variant) {
        self.entries.entry(id).or_default().push(variant);
    }

    /// Drops all variants of `id`; returns whether it was present.
    pub fn remove(&mut self, id: &LogicalId) -> bool {
        self.entries.remove(id).is_some()
    }

    pub fn variants(&self, id: &LogicalId) -> &[Variant] {
        self.entries.get(id).map_or(&[], Vec::as_slice)
    }

    /// Every id with its variants, sorted by id (the build report and `verify` walk the whole
    /// manifest).
    pub fn iter(&self) -> impl Iterator<Item = (&LogicalId, &[Variant])> {
        let mut v: Vec<(&LogicalId, &[Variant])> = self
            .entries
            .iter()
            .map(|(id, vs)| (id, vs.as_slice()))
            .collect();
        v.sort_by(|a, b| a.0.cmp(b.0));
        v.into_iter()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Lowest format rank, then smallest size. `None` if unknown or unsupported.
    pub fn resolve(&self, id: &LogicalId, profile: &PlatformProfile) -> Option<&Variant> {
        self.entries
            .get(id)?
            .iter()
            .filter_map(|v| profile.rank(v.format).map(|r| (r, v)))
            .min_by_key(|(r, v)| (*r, v.size))
            .map(|(_, v)| v)
    }

    /// One suitable variant per asset in the group.
    pub fn prefetch_group<'a>(
        &'a self,
        group: PrefetchGroup,
        profile: &'a PlatformProfile,
    ) -> impl Iterator<Item = &'a Variant> + 'a {
        self.entries
            .keys()
            .filter_map(move |id| self.resolve(id, profile))
            .filter(move |v| v.prefetch == group)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn variant(format: VariantFormat, n: u8, size: u64) -> Variant {
        Variant {
            format,
            hash: ContentHash([n; 32]),
            size,
            prefetch: PrefetchGroup(1),
        }
    }

    fn tex_manifest() -> (Manifest, LogicalId) {
        let id = LogicalId::new("tex.grass");
        let mut m = Manifest::new();
        m.insert(id.clone(), variant(VariantFormat::Uncompressed, 1, 4000));
        m.insert(id.clone(), variant(VariantFormat::Bc, 2, 1000));
        m.insert(id.clone(), variant(VariantFormat::Astc, 3, 800));
        (m, id)
    }

    #[test]
    fn picks_by_profile_preference() {
        let (m, id) = tex_manifest();
        let desktop = PlatformProfile::new(vec![VariantFormat::Bc, VariantFormat::Etc2]);
        assert_eq!(m.resolve(&id, &desktop).unwrap().format, VariantFormat::Bc);
        let apple = PlatformProfile::new(vec![VariantFormat::Astc, VariantFormat::Bc]);
        assert_eq!(m.resolve(&id, &apple).unwrap().format, VariantFormat::Astc);
    }

    #[test]
    fn the_three_device_profiles_pick_their_variant_and_bc_comes_last_without_bc() {
        let (m, id) = tex_manifest();
        let pick = |p: &PlatformProfile| m.resolve(&id, p).unwrap().format;
        assert_eq!(pick(&PlatformProfile::bc_device()), VariantFormat::Bc);
        assert_eq!(pick(&PlatformProfile::astc_device()), VariantFormat::Astc);
        assert_eq!(
            pick(&PlatformProfile::no_compression()),
            VariantFormat::Uncompressed
        );
        // A BC original with no other variant still resolves: the client decodes it on the CPU.
        let only_bc = LogicalId::new("tex.dxt");
        let mut m = Manifest::new();
        m.insert(only_bc.clone(), variant(VariantFormat::Bc, 1, 10));
        for p in [
            PlatformProfile::astc_device(),
            PlatformProfile::no_compression(),
        ] {
            assert_eq!(m.resolve(&only_bc, &p).unwrap().format, VariantFormat::Bc);
        }
    }

    #[test]
    fn falls_back_to_uncompressed() {
        let (m, id) = tex_manifest();
        let p = PlatformProfile::new(vec![VariantFormat::Etc2]);
        assert_eq!(
            m.resolve(&id, &p).unwrap().format,
            VariantFormat::Uncompressed
        );
    }

    #[test]
    fn none_when_unsupported_or_unknown() {
        let id = LogicalId::new("tex.only-bc");
        let mut m = Manifest::new();
        m.insert(id.clone(), variant(VariantFormat::Bc, 1, 10));
        assert!(
            m.resolve(&id, &PlatformProfile::uncompressed_only())
                .is_none()
        );
        assert!(
            m.resolve(
                &LogicalId::new("nope"),
                &PlatformProfile::uncompressed_only()
            )
            .is_none()
        );
    }

    #[test]
    fn generic_always_matches_and_smaller_wins_on_tie() {
        let id = LogicalId::new("mesh.unit");
        let mut m = Manifest::new();
        m.insert(id.clone(), variant(VariantFormat::Generic, 1, 500));
        m.insert(id.clone(), variant(VariantFormat::Generic, 2, 300));
        let v = m.resolve(&id, &PlatformProfile::new(vec![])).unwrap();
        assert_eq!(v.size, 300);
    }

    #[test]
    fn encode_roundtrip() {
        let (m, id) = tex_manifest();
        let back = Manifest::decode(&m.encode()).unwrap();
        assert_eq!(back, m);
        assert_eq!(back.variants(&id).len(), 3);
        assert_eq!(
            Manifest::decode(&Manifest::new().encode()).unwrap(),
            Manifest::new()
        );
    }

    #[test]
    fn encoding_ignores_insert_order() {
        let a = LogicalId::new("a");
        let b = LogicalId::new("b");
        let vs = [
            variant(VariantFormat::Bc, 2, 10),
            variant(VariantFormat::Astc, 3, 20),
            variant(VariantFormat::Bc, 1, 30),
        ];
        let mut m1 = Manifest::new();
        let mut m2 = Manifest::new();
        for v in &vs {
            m1.insert(a.clone(), v.clone());
        }
        m1.insert(b.clone(), vs[0].clone());
        m2.insert(b, vs[0].clone());
        for v in vs.iter().rev() {
            m2.insert(a.clone(), v.clone());
        }
        assert_eq!(m1.encode(), m2.encode());
    }

    #[test]
    fn iter_is_sorted_by_id() {
        let mut m = Manifest::new();
        for id in ["b", "c", "a"] {
            m.insert(LogicalId::new(id), variant(VariantFormat::Bc, 1, 1));
        }
        let ids: Vec<&str> = m.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "c"]);
    }

    #[test]
    fn decode_rejects_bad_header() {
        let (m, _) = tex_manifest();
        let bytes = m.encode();
        let mut bad = bytes.clone();
        bad[..4].copy_from_slice(b"C5MD");
        assert!(matches!(
            Manifest::decode(&bad),
            Err(FormatError::BadMagic { .. })
        ));
        let mut bad = bytes.clone();
        bad[4] = 0xff;
        assert!(matches!(
            Manifest::decode(&bad),
            Err(FormatError::UnsupportedVersion { .. })
        ));
        assert!(Manifest::decode(&bytes[..bytes.len() - 3]).is_err());
    }

    #[test]
    fn prefetch_group_filters() {
        let (m, _) = tex_manifest();
        let p = PlatformProfile::new(vec![VariantFormat::Bc]);
        assert_eq!(m.prefetch_group(PrefetchGroup(1), &p).count(), 1);
        assert_eq!(m.prefetch_group(PrefetchGroup(2), &p).count(), 0);
    }
}
