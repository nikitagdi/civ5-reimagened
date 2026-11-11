//! `FONT`, `IMGS`, `SCRP` and `AREF`: the sections that carry bytes (or, for `AREF`, only names).
//!
//! A blob section is `u32 LE index length`, the index in postcard, then the byte area. A `BlobRef`
//! locates a blob inside the area of its section, so `Package::blob` hands out a slice of the
//! file without copying. Fonts and picture pages are stored zstd-compressed (the converter
//! compresses, the client only decompresses with `ruzstd`); scripts are stored as they are.

use serde::{Deserialize, Serialize};

use crate::container::SectionTag;
use crate::error::PackageError;
use crate::ids::StrId;

/// The largest blob after decompression (a 1024 x 1024 RGBA page is 4 MiB).
pub const MAX_BLOB_RAW: u32 = 32 << 20;
/// The largest picture page edge.
pub const MAX_PAGE_EDGE: u16 = 1024;

/// Which section's byte area a blob lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlobSection {
    /// `FONT`.
    Font,
    /// `IMGS`.
    Imgs,
    /// `SCRP`.
    Scrp,
}

impl BlobSection {
    /// The tag of the section.
    pub fn tag(self) -> SectionTag {
        match self {
            BlobSection::Font => SectionTag::FONT,
            BlobSection::Imgs => SectionTag::IMGS,
            BlobSection::Scrp => SectionTag::SCRP,
        }
    }
}

/// Where the bytes of a blob are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlobRef {
    /// The section whose area holds the blob.
    pub section: BlobSection,
    /// Start inside the area.
    pub offset: u32,
    /// Stored length (compressed when `zstd`).
    pub len: u32,
    /// Length after decompression (equal to `len` when not compressed).
    pub raw_len: u32,
    /// The stored bytes are one zstd frame.
    pub zstd: bool,
}

impl BlobRef {
    /// A placeholder for a blob the builder has not placed yet.
    pub fn unplaced(section: BlobSection, zstd: bool) -> Self {
        Self {
            section,
            offset: 0,
            len: 0,
            raw_len: 0,
            zstd,
        }
    }
}

/// The role a face plays for mods.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FontRole {
    /// `ui-sans`.
    Sans,
    /// `ui-display`.
    Display,
}

/// One face of the skin: a subset TTF or OTF (the shaper turns font features on, they are not
/// frozen into the file).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Face {
    /// The family name.
    pub family: StrId,
    /// The weight, 1 to 1000.
    pub weight: u16,
    /// An italic face.
    pub italic: bool,
    /// The role, when the face is one of the skin's role faces.
    pub role: Option<FontRole>,
    /// The code point ranges the subset covers, inclusive, ascending.
    pub ranges: Vec<(u32, u32)>,
    /// The font file.
    pub blob: BlobRef,
    /// blake3 of the font file after decompression.
    pub blake3: [u8; 32],
}

/// The `FONT` index.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fonts {
    /// Sorted by family, weight, italic; no two alike.
    pub faces: Vec<Face>,
}

/// The pixel format of a picture page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PageFormat {
    /// Four bytes per pixel.
    Rgba8,
    /// One byte per pixel: coverage of a mask icon.
    A8,
}

impl PageFormat {
    /// Bytes per pixel.
    pub fn bytes_per_pixel(self) -> u32 {
        match self {
            PageFormat::Rgba8 => 4,
            PageFormat::A8 => 1,
        }
    }
}

/// One atlas page, rasterized by Chrome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page {
    /// The pixel format.
    pub format: PageFormat,
    /// 1 or 2: the density the page holds.
    pub scale: u8,
    /// Width and height in pixels, at most 1024.
    pub size: [u16; 2],
    /// The pixels.
    pub blob: BlobRef,
    /// blake3 of the pixels after decompression.
    pub blake3: [u8; 32],
}

/// A rectangle of a page, in pixels (including the 1 px margin the packer leaves around it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sprite {
    /// The page.
    pub page: u16,
    /// `[x, y, width, height]`.
    pub rect: [u16; 4],
}

/// A picture of the skin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Img {
    /// The path inside the skin.
    pub path: StrId,
    /// The size in CSS px.
    pub size: [f32; 2],
    /// A mask icon: painted with the node's `color`; its pages are `A8`.
    pub mask: bool,
    /// The 1x sprite.
    pub x1: Sprite,
    /// The 2x sprite, when the skin has one.
    pub x2: Option<Sprite>,
}

/// The `IMGS` index.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Images {
    /// The atlas pages.
    pub pages: Vec<Page>,
    /// The pictures, ascending by path.
    pub imgs: Vec<Img>,
}

/// The `SCRP` index.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scripts {
    /// Script files in execution order.
    pub files: Vec<(StrId, BlobRef)>,
    /// The model roots the scripts see, ascending.
    pub reads: Vec<StrId>,
}

/// The `AREF` section: references to game assets (`portrait:...`, `ui-tx:...`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ARefs {
    /// Ascending, no repeats.
    pub items: Vec<StrId>,
}

/// Splits a blob section into its index bytes and the offset of its area in the payload.
pub(crate) fn split(at: SectionTag, payload: &[u8]) -> Result<(&[u8], usize), PackageError> {
    let corrupt = || PackageError::Corrupt { at: Some(at) };
    let (head, rest) = payload.split_first_chunk::<4>().ok_or_else(corrupt)?;
    let index_len = u32::from_le_bytes(*head) as usize;
    if index_len > rest.len() {
        return Err(corrupt());
    }
    Ok((&rest[..index_len], 4 + index_len))
}

fn check_blob(
    at: SectionTag,
    what: &str,
    blob: &BlobRef,
    section: BlobSection,
    area_len: usize,
) -> Result<(), PackageError> {
    let bad = |m: &str| Err(PackageError::invalid(at, format!("{what}: {m}")));
    if blob.section != section {
        return bad("the blob points into another section");
    }
    if u64::from(blob.offset) + u64::from(blob.len) > area_len as u64 {
        return bad("the blob is outside the byte area");
    }
    if blob.raw_len > MAX_BLOB_RAW {
        return bad("the blob is too large");
    }
    if !blob.zstd && blob.raw_len != blob.len {
        return bad("an uncompressed blob must have raw_len equal to len");
    }
    Ok(())
}

impl Fonts {
    pub(crate) fn validate(
        &self,
        at: SectionTag,
        strs: usize,
        area_len: usize,
    ) -> Result<(), PackageError> {
        let key = |f: &Face| (f.family, f.weight, f.italic);
        if self.faces.windows(2).any(|w| key(&w[0]) >= key(&w[1])) {
            return Err(PackageError::invalid(
                at,
                "faces are not sorted by family, weight, italic or two are alike",
            ));
        }
        for (i, f) in self.faces.iter().enumerate() {
            let what = format!("face {i}");
            if f.family.index() >= strs {
                return Err(PackageError::invalid(
                    at,
                    format!("{what}: family is not in the string table"),
                ));
            }
            if !(1..=1000).contains(&f.weight) {
                return Err(PackageError::invalid(
                    at,
                    format!("{what}: weight out of range"),
                ));
            }
            if f.ranges.iter().any(|(a, b)| a > b) || f.ranges.windows(2).any(|w| w[0].1 >= w[1].0)
            {
                return Err(PackageError::invalid(
                    at,
                    format!("{what}: ranges are not ascending"),
                ));
            }
            check_blob(at, &what, &f.blob, BlobSection::Font, area_len)?;
        }
        Ok(())
    }
}

impl Images {
    pub(crate) fn validate(
        &self,
        at: SectionTag,
        strs: usize,
        area_len: usize,
    ) -> Result<(), PackageError> {
        let bad = |m: String| Err(PackageError::invalid(at, m));
        if self.pages.len() > usize::from(u16::MAX) {
            return bad("too many pages".into());
        }
        for (i, p) in self.pages.iter().enumerate() {
            let what = format!("page {i}");
            if !matches!(p.scale, 1 | 2) {
                return bad(format!("{what}: scale must be 1 or 2"));
            }
            let [w, h] = p.size;
            if w == 0 || h == 0 || w > MAX_PAGE_EDGE || h > MAX_PAGE_EDGE {
                return bad(format!("{what}: size out of range"));
            }
            if p.blob.raw_len != u32::from(w) * u32::from(h) * p.format.bytes_per_pixel() {
                return bad(format!(
                    "{what}: raw_len does not match the size and format"
                ));
            }
            check_blob(at, &what, &p.blob, BlobSection::Imgs, area_len)?;
        }
        if self.imgs.windows(2).any(|w| w[0].path >= w[1].path) {
            return bad("pictures are not sorted by path or two are alike".into());
        }
        for (i, img) in self.imgs.iter().enumerate() {
            let what = format!("picture {i}");
            if img.path.index() >= strs {
                return bad(format!("{what}: path is not in the string table"));
            }
            if img.size.iter().any(|v| !v.is_finite() || *v <= 0.0) {
                return bad(format!("{what}: size must be finite and positive"));
            }
            let sprites = std::iter::once((&img.x1, 1u8)).chain(img.x2.as_ref().map(|s| (s, 2u8)));
            for (s, scale) in sprites {
                let Some(page) = self.pages.get(usize::from(s.page)) else {
                    return bad(format!("{what}: sprite page is out of range"));
                };
                let [x, y, w, h] = s.rect.map(u32::from);
                if page.scale != scale
                    || x + w > u32::from(page.size[0])
                    || y + h > u32::from(page.size[1])
                    || w == 0
                    || h == 0
                {
                    return bad(format!("{what}: sprite does not fit its page"));
                }
                if (page.format == PageFormat::A8) != img.mask {
                    return bad(format!(
                        "{what}: a mask picture lives on an A8 page, others on RGBA8"
                    ));
                }
            }
        }
        Ok(())
    }
}

impl Scripts {
    pub(crate) fn validate(
        &self,
        at: SectionTag,
        strs: usize,
        area_len: usize,
    ) -> Result<(), PackageError> {
        for (i, (path, blob)) in self.files.iter().enumerate() {
            let what = format!("script {i}");
            if path.index() >= strs {
                return Err(PackageError::invalid(
                    at,
                    format!("{what}: path is not in the string table"),
                ));
            }
            if blob.zstd {
                return Err(PackageError::invalid(
                    at,
                    format!("{what}: scripts are stored as they are"),
                ));
            }
            check_blob(at, &what, blob, BlobSection::Scrp, area_len)?;
        }
        if self.reads.iter().any(|r| r.index() >= strs)
            || self.reads.windows(2).any(|w| w[0] >= w[1])
        {
            return Err(PackageError::invalid(
                at,
                "reads are not ascending names of the string table",
            ));
        }
        Ok(())
    }
}

impl ARefs {
    pub(crate) fn validate(&self, at: SectionTag, strs: usize) -> Result<(), PackageError> {
        if self.items.iter().any(|r| r.index() >= strs)
            || self.items.windows(2).any(|w| w[0] >= w[1])
        {
            return Err(PackageError::invalid(
                at,
                "references are not ascending names of the string table",
            ));
        }
        Ok(())
    }
}

#[cfg(feature = "write")]
impl crate::remap::Remapped for Face {
    fn remap(&mut self, r: &crate::remap::Remap) {
        r.str(&mut self.family);
    }
}

#[cfg(feature = "write")]
impl crate::remap::Remapped for Img {
    fn remap(&mut self, r: &crate::remap::Remap) {
        r.str(&mut self.path);
    }
}

#[cfg(feature = "write")]
impl crate::remap::Remapped for Scripts {
    fn remap(&mut self, r: &crate::remap::Remap) {
        for (path, _) in &mut self.files {
            r.str(path);
        }
        for read in &mut self.reads {
            r.str(read);
        }
        self.reads.sort_unstable();
        self.reads.dedup();
    }
}

/// Lays out a blob section: index length, index, area.
#[cfg(feature = "write")]
pub(crate) fn join(index: &[u8], area: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + index.len() + area.len());
    out.extend_from_slice(&(index.len() as u32).to_le_bytes());
    out.extend_from_slice(index);
    out.extend_from_slice(area);
    out
}
