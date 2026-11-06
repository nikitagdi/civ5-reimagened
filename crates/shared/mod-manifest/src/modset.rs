//! `ModSet`: the frozen, ordered result of a resolution, and its stable hash (`modset_hash`).

use std::collections::BTreeSet;
use std::fmt;

use crate::error::ModSetError;
use crate::hash::xxh64;
use crate::manifest::{ClientPolicy, ModId, Part, SyncClass};
use crate::semver::Version;

/// Opaque 32-byte digest of a mod's content bundle, computed by the host (the algorithm is its business).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentHash(pub [u8; 32]);

impl ContentHash {
    pub const ZERO: ContentHash = ContentHash([0; 32]);
}

pub(crate) fn to_hex(bytes: &[u8]) -> String {
    use fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

pub(crate) fn from_hex<const N: usize>(s: &str) -> Option<[u8; N]> {
    if s.len() != N * 2
        || !s
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    let mut out = [0u8; N];
    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
        let hi = (chunk[0] as char).to_digit(16)?;
        let lo = (chunk[1] as char).to_digit(16)?;
        out[i] = (hi * 16 + lo) as u8;
    }
    Some(out)
}

impl fmt::Display for ContentHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&to_hex(&self.0))
    }
}

impl ContentHash {
    /// Parses 64 lowercase hex digits.
    pub fn from_hex(s: &str) -> Option<Self> {
        from_hex::<32>(s).map(Self)
    }
}

/// 128-bit digest of a `ModSet` (two xxh64 lanes). It detects an accidental mismatch between server, saves and
/// clients; it is not a security boundary (mods are trusted code, SPEC допущение 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModsetHash(pub [u8; 16]);

impl fmt::Display for ModsetHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&to_hex(&self.0))
    }
}

impl ModsetHash {
    /// Parses 32 lowercase hex digits.
    pub fn from_hex(s: &str) -> Option<Self> {
        from_hex::<16>(s).map(Self)
    }
}

/// One mod of a game, frozen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMod {
    pub id: ModId,
    pub version: Version,
    /// Position in the load order; the `mod_idx` of the wire protocol.
    pub load_idx: u16,
    pub sync: SyncClass,
    /// Effective policy (see `Manifest::client_policy`).
    pub client: ClientPolicy,
    pub parts: BTreeSet<Part>,
    pub content_hash: ContentHash,
}

/// Ordered mods of a game. Invariants: ids are unique and `mods[i].load_idx == i`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModSet {
    mods: Vec<ResolvedMod>,
}

const HASH_MAGIC: &[u8; 8] = b"C5MODSET";
const HASH_FORMAT: u8 = 1;
const SEED_LO: u64 = 0x6D6F_6473_6574_0001;
const SEED_HI: u64 = 0x6D6F_6473_6574_0002;

impl ModSet {
    pub fn new(mods: Vec<ResolvedMod>) -> Result<Self, ModSetError> {
        if mods.len() > usize::from(u16::MAX) {
            return Err(ModSetError::TooMany(mods.len()));
        }
        let mut seen = BTreeSet::new();
        for (i, m) in mods.iter().enumerate() {
            if !seen.insert(&m.id) {
                return Err(ModSetError::DuplicateId(m.id.clone()));
            }
            // The length check above makes the index fit a u16.
            let expected = i as u16;
            if m.load_idx != expected {
                return Err(ModSetError::BadLoadIdx {
                    id: m.id.clone(),
                    expected,
                    found: m.load_idx,
                });
            }
        }
        Ok(Self { mods })
    }

    pub fn mods(&self) -> &[ResolvedMod] {
        &self.mods
    }

    pub fn len(&self) -> usize {
        self.mods.len()
    }

    pub fn is_empty(&self) -> bool {
        self.mods.is_empty()
    }

    pub fn get(&self, id: &ModId) -> Option<&ResolvedMod> {
        self.mods.iter().find(|m| &m.id == id)
    }

    pub fn by_idx(&self, load_idx: u16) -> Option<&ResolvedMod> {
        self.mods.get(usize::from(load_idx))
    }

    /// The canonical byte string that is hashed. Layout (all integers little-endian):
    /// `"C5MODSET" | format u8 | count u32 | per mod: id_len u8, id bytes, major u32, minor u32, patch u32,
    /// load_idx u16, sync u8, client u8, parts mask u16, content_hash [32]`.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(16 + self.mods.len() * 80);
        b.extend_from_slice(HASH_MAGIC);
        b.push(HASH_FORMAT);
        b.extend_from_slice(&(self.mods.len() as u32).to_le_bytes());
        for m in &self.mods {
            let id = m.id.as_str().as_bytes();
            // A mod id is at most 32 bytes (checked by `ModId::new`).
            b.push(id.len() as u8);
            b.extend_from_slice(id);
            b.extend_from_slice(&m.version.major.to_le_bytes());
            b.extend_from_slice(&m.version.minor.to_le_bytes());
            b.extend_from_slice(&m.version.patch.to_le_bytes());
            b.extend_from_slice(&m.load_idx.to_le_bytes());
            b.push(match m.sync {
                SyncClass::Gameplay => 0,
                SyncClass::Cosmetic => 1,
                SyncClass::Client => 2,
            });
            b.push(match m.client {
                ClientPolicy::Optional => 0,
                ClientPolicy::Required => 1,
            });
            let mask = m.parts.iter().fold(0u16, |acc, p| acc | p.bit());
            b.extend_from_slice(&mask.to_le_bytes());
            b.extend_from_slice(&m.content_hash.0);
        }
        b
    }

    /// Stable hash of the set: changes with any id, version, order, class, part set or content hash.
    pub fn modset_hash(&self) -> ModsetHash {
        let bytes = self.canonical_bytes();
        let lo = xxh64(SEED_LO, &bytes);
        let hi = xxh64(SEED_HI, &bytes);
        let mut out = [0u8; 16];
        out[..8].copy_from_slice(&lo.to_be_bytes());
        out[8..].copy_from_slice(&hi.to_be_bytes());
        ModsetHash(out)
    }
}
