//! The pack: a manifest of files plus their bytes, as one blob.
//!
//! Container: `"C5MODPK1" | manifest_len u32 LE | manifest (UTF-8 text) | file bytes, concatenated in manifest order`.
//! The manifest is canonical text (LF, sorted, one record per line), so the same files always give the same bytes
//! and the same pack hash:
//!
//! ```text
//! c5-mod-assets 1
//! mod hello
//! source minecraft-client.jar <sha256>
//! file textures/dirt texture-rgba8:16x16x1 1024 <sha256>
//! file sounds/dig sound 6700 <sha256>
//! ```

use std::collections::BTreeSet;

use mod_manifest::ModId;

use crate::error::PackError;
use crate::sha256::Sha256;

/// Container magic, includes the format version.
pub const PACK_MAGIC: [u8; 8] = *b"C5MODPK1";
/// Most files in one pack.
pub const MAX_FILES: usize = 4096;
/// Longest logical asset id, in bytes.
pub const MAX_ID_LEN: usize = 128;
/// Largest manifest, in bytes.
pub const MAX_MANIFEST_BYTES: usize = 1 << 20;
/// Largest texture side.
pub const MAX_TEXTURE_SIDE: u32 = 16384;
/// Most mip levels of a texture.
pub const MAX_MIPS: u8 = 16;

const MANIFEST_HEADER: &str = "c5-mod-assets 1";

/// What a file is. The pack stores bytes; this tells the loader how to hand them to the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum AssetKind {
    /// Opaque bytes.
    Raw,
    /// An encoded sound (the loader decides what it can decode).
    Sound,
    /// Tightly packed RGBA8 pixels, row-major, top row first; `mips` levels follow each other from the largest
    /// (`width x height`) down, each side halved and clamped to 1.
    TextureRgba8 { width: u32, height: u32, mips: u8 },
}

impl AssetKind {
    /// The text token in the manifest: `raw`, `sound` or `texture-rgba8:WxHxM`.
    pub fn token(&self) -> String {
        match *self {
            AssetKind::Raw => "raw".into(),
            AssetKind::Sound => "sound".into(),
            AssetKind::TextureRgba8 {
                width,
                height,
                mips,
            } => format!("texture-rgba8:{width}x{height}x{mips}"),
        }
    }

    fn parse(tok: &str) -> Option<AssetKind> {
        match tok {
            "raw" => Some(AssetKind::Raw),
            "sound" => Some(AssetKind::Sound),
            _ => {
                let rest = tok.strip_prefix("texture-rgba8:")?;
                let mut it = rest.split('x');
                let width = it.next()?.parse().ok()?;
                let height = it.next()?.parse().ok()?;
                let mips = it.next()?.parse().ok()?;
                if it.next().is_some() {
                    return None;
                }
                Some(AssetKind::TextureRgba8 {
                    width,
                    height,
                    mips,
                })
            }
        }
    }

    /// The exact byte size a file of this kind must have, if the kind fixes it.
    pub fn exact_size(&self) -> Option<u64> {
        match *self {
            AssetKind::TextureRgba8 {
                width,
                height,
                mips,
            } => {
                let mut total = 0u64;
                for level in 0..u32::from(mips) {
                    let w = (width >> level).max(1);
                    let h = (height >> level).max(1);
                    total += u64::from(w) * u64::from(h) * 4;
                }
                Some(total)
            }
            _ => None,
        }
    }
}

/// An external file the pack was made from (a game jar, a sound index) and its digest. Mirrors `assets.lock`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub name: String,
    pub sha256: Sha256,
}

/// One file of a pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackFile {
    /// Logical id, e.g. `textures/dirt`.
    pub id: String,
    pub kind: AssetKind,
    pub size: u64,
    pub sha256: Sha256,
}

/// Valid logical id: `/`-separated segments of `[a-z0-9_][a-z0-9_.-]*`, at most [`MAX_ID_LEN`] bytes.
pub fn valid_asset_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && id.split('/').all(|seg| {
            let mut c = seg.bytes();
            matches!(c.next(), Some(b'a'..=b'z' | b'0'..=b'9' | b'_'))
                && c.all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-'))
        })
}

fn valid_source_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// The manifest of a pack: whose it is, what it was made from, and its files. Sources and files are sorted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackManifest {
    mod_id: ModId,
    sources: Vec<Source>,
    files: Vec<PackFile>,
}

impl PackManifest {
    /// Validates and sorts. Errors: bad or duplicate ids, bad texture shape or size, too many files.
    pub fn new(
        mod_id: ModId,
        mut sources: Vec<Source>,
        mut files: Vec<PackFile>,
    ) -> Result<Self, PackError> {
        if files.len() > MAX_FILES {
            return Err(PackError::TooManyFiles {
                count: files.len(),
                max: MAX_FILES,
            });
        }
        sources.sort_by(|a, b| a.name.cmp(&b.name));
        let mut seen = BTreeSet::new();
        for s in &sources {
            if !valid_source_name(&s.name) {
                return Err(PackError::BadSourceName(s.name.clone()));
            }
            if !seen.insert(s.name.as_str()) {
                return Err(PackError::DuplicateSource(s.name.clone()));
            }
        }
        files.sort_by(|a, b| a.id.cmp(&b.id));
        let mut seen = BTreeSet::new();
        for f in &files {
            if !valid_asset_id(&f.id) {
                return Err(PackError::BadId(f.id.clone()));
            }
            if !seen.insert(f.id.as_str()) {
                return Err(PackError::DuplicateId(f.id.clone()));
            }
            if let AssetKind::TextureRgba8 {
                width,
                height,
                mips,
            } = f.kind
            {
                let ok = (1..=MAX_TEXTURE_SIDE).contains(&width)
                    && (1..=MAX_TEXTURE_SIDE).contains(&height)
                    && (1..=MAX_MIPS).contains(&mips);
                if !ok {
                    return Err(PackError::BadTexture {
                        id: f.id.clone(),
                        width,
                        height,
                        mips,
                    });
                }
            }
            if let Some(expected) = f.kind.exact_size()
                && expected != f.size
            {
                return Err(PackError::BadSize {
                    id: f.id.clone(),
                    kind: f.kind.token(),
                    expected,
                    actual: f.size,
                });
            }
        }
        Ok(Self {
            mod_id,
            sources,
            files,
        })
    }

    pub fn mod_id(&self) -> &ModId {
        &self.mod_id
    }

    pub fn sources(&self) -> &[Source] {
        &self.sources
    }

    /// Files sorted by id.
    pub fn files(&self) -> &[PackFile] {
        &self.files
    }

    pub fn file(&self, id: &str) -> Option<&PackFile> {
        self.files
            .binary_search_by(|f| f.id.as_str().cmp(id))
            .ok()
            .map(|i| &self.files[i])
    }

    /// Total size of all files.
    pub fn data_len(&self) -> u64 {
        self.files.iter().map(|f| f.size).sum()
    }

    /// Canonical text.
    pub fn to_text(&self) -> String {
        let mut out = format!("{MANIFEST_HEADER}\nmod {}\n", self.mod_id);
        for s in &self.sources {
            out.push_str(&format!("source {} {}\n", s.name, s.sha256));
        }
        for f in &self.files {
            out.push_str(&format!(
                "file {} {} {} {}\n",
                f.id,
                f.kind.token(),
                f.size,
                f.sha256
            ));
        }
        out
    }

    /// Strict parse of the canonical text: unknown records, unsorted or duplicate entries and a missing final
    /// newline are errors.
    pub fn parse_text(text: &str) -> Result<Self, PackError> {
        let err = |line: usize, msg: &str| PackError::Manifest {
            line,
            msg: msg.to_string(),
        };
        if text.len() > MAX_MANIFEST_BYTES {
            return Err(PackError::ManifestTooLarge {
                len: text.len(),
                max: MAX_MANIFEST_BYTES,
            });
        }
        if !text.ends_with('\n') {
            return Err(err(0, "missing final newline"));
        }
        let mut lines = text[..text.len() - 1].split('\n').enumerate();
        match lines.next() {
            Some((_, l)) if l == MANIFEST_HEADER => {}
            _ => return Err(err(1, "expected the header `c5-mod-assets 1`")),
        }
        let mut mod_id = None;
        let mut sources: Vec<Source> = Vec::new();
        let mut files: Vec<PackFile> = Vec::new();
        for (i, line) in lines {
            let n = i + 1;
            let parts: Vec<&str> = line.split(' ').collect();
            match parts.as_slice() {
                ["mod", id] => {
                    if mod_id.is_some() || !sources.is_empty() || !files.is_empty() {
                        return Err(err(n, "`mod` must come once, first"));
                    }
                    mod_id = Some(ModId::new(id).map_err(|e| err(n, e))?);
                }
                ["source", name, sha] => {
                    if mod_id.is_none() || !files.is_empty() {
                        return Err(err(n, "`source` records come after `mod`, before `file`"));
                    }
                    let sha256 = sha.parse().map_err(|_| err(n, "bad sha256"))?;
                    sources.push(Source {
                        name: (*name).to_string(),
                        sha256,
                    });
                }
                ["file", id, kind, size, sha] => {
                    if mod_id.is_none() {
                        return Err(err(n, "`file` before `mod`"));
                    }
                    files.push(PackFile {
                        id: (*id).to_string(),
                        kind: AssetKind::parse(kind).ok_or_else(|| err(n, "unknown kind"))?,
                        size: size.parse().map_err(|_| err(n, "bad size"))?,
                        sha256: sha.parse().map_err(|_| err(n, "bad sha256"))?,
                    });
                }
                _ => return Err(err(n, "unknown or malformed record")),
            }
        }
        let mod_id = mod_id.ok_or_else(|| err(0, "no `mod` record"))?;
        // Canonical order is part of the format: a manifest that `new` would reorder is not canonical.
        let in_order = sources.windows(2).all(|w| w[0].name <= w[1].name)
            && files.windows(2).all(|w| w[0].id <= w[1].id);
        // `new` reports duplicates and invalid entries precisely; disorder is checked after it.
        let manifest = Self::new(mod_id, sources, files)?;
        if !in_order {
            return Err(err(0, "sources and files must be sorted by name / id"));
        }
        Ok(manifest)
    }
}

/// A manifest with its bytes. Every `Pack` is consistent: sizes match, every file hashes to its manifest entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pack {
    manifest: PackManifest,
    data: Vec<u8>,
}

/// A file to put into a new pack.
#[derive(Debug, Clone)]
pub struct NewFile {
    pub id: String,
    pub kind: AssetKind,
    pub bytes: Vec<u8>,
}

impl Pack {
    /// Builds a pack from files; sizes and hashes are computed. Files may come in any order.
    pub fn build(
        mod_id: ModId,
        sources: Vec<Source>,
        mut files: Vec<NewFile>,
    ) -> Result<Pack, PackError> {
        files.sort_by(|a, b| a.id.cmp(&b.id));
        let entries = files
            .iter()
            .map(|f| PackFile {
                id: f.id.clone(),
                kind: f.kind,
                size: f.bytes.len() as u64,
                sha256: Sha256::of(&f.bytes),
            })
            .collect();
        let manifest = PackManifest::new(mod_id, sources, entries)?;
        let mut data = Vec::with_capacity(manifest.data_len() as usize);
        for f in &files {
            data.extend_from_slice(&f.bytes);
        }
        Ok(Pack { manifest, data })
    }

    /// Reads and checks a pack: magic, manifest, sizes, every file's sha256.
    pub fn parse(bytes: &[u8]) -> Result<Pack, PackError> {
        let head = bytes.get(..12).ok_or(PackError::Truncated("the header"))?;
        if head[..8] != PACK_MAGIC {
            return Err(PackError::BadMagic);
        }
        let mlen = u32::from_le_bytes([head[8], head[9], head[10], head[11]]) as usize;
        if mlen > MAX_MANIFEST_BYTES {
            return Err(PackError::ManifestTooLarge {
                len: mlen,
                max: MAX_MANIFEST_BYTES,
            });
        }
        let mbytes = bytes
            .get(12..12 + mlen)
            .ok_or(PackError::Truncated("the manifest"))?;
        let text = std::str::from_utf8(mbytes).map_err(|_| PackError::Manifest {
            line: 0,
            msg: "not UTF-8".into(),
        })?;
        let manifest = PackManifest::parse_text(text)?;
        let data = &bytes[12 + mlen..];
        let want = manifest.data_len();
        if (data.len() as u64) < want {
            return Err(PackError::Truncated("the file data"));
        }
        if data.len() as u64 > want {
            return Err(PackError::TrailingBytes(data.len() - want as usize));
        }
        let mut at = 0usize;
        for f in manifest.files() {
            let end = at + f.size as usize;
            let actual = Sha256::of(&data[at..end]);
            if actual != f.sha256 {
                return Err(PackError::Corrupt {
                    id: f.id.clone(),
                    expected: f.sha256,
                    actual,
                });
            }
            at = end;
        }
        Ok(Pack {
            manifest,
            data: data.to_vec(),
        })
    }

    /// Like [`Pack::parse`], with the pack hash checked against `expected` first.
    pub fn parse_expecting(bytes: &[u8], expected: &Sha256) -> Result<Pack, PackError> {
        let actual = Sha256::of(bytes);
        if actual != *expected {
            return Err(PackError::HashMismatch {
                expected: *expected,
                actual,
            });
        }
        Pack::parse(bytes)
    }

    /// The pack as bytes (the same bytes `build` and `parse` round-trip to).
    pub fn to_bytes(&self) -> Vec<u8> {
        let text = self.manifest.to_text();
        let mut out = Vec::with_capacity(12 + text.len() + self.data.len());
        out.extend_from_slice(&PACK_MAGIC);
        out.extend_from_slice(&(text.len() as u32).to_le_bytes());
        out.extend_from_slice(text.as_bytes());
        out.extend_from_slice(&self.data);
        out
    }

    /// The hash the server serves the pack under (`/mods/{hash}`): sha256 of [`Pack::to_bytes`].
    pub fn hash(&self) -> Sha256 {
        Sha256::of(&self.to_bytes())
    }

    pub fn manifest(&self) -> &PackManifest {
        &self.manifest
    }

    /// Bytes of a file, `None` for an unknown id.
    pub fn get(&self, id: &str) -> Option<&[u8]> {
        let mut at = 0usize;
        for f in self.manifest.files() {
            let end = at + f.size as usize;
            if f.id == id {
                return Some(&self.data[at..end]);
            }
            at = end;
        }
        None
    }

    /// Ids of all files, sorted.
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.manifest.files().iter().map(|f| f.id.as_str())
    }
}
