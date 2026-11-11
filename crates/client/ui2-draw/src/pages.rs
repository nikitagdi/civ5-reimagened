//! The picture pages of a skin package, as the scene keeps them for the GPU pass.
//!
//! A package carries its pictures already rasterized into pages (`ui2_package::Images`): `A8` pages hold
//! coverage masks (icons painted in one colour), `Rgba8` pages hold premultiplied colour pictures. The scene
//! keeps the decompressed pixels of both kinds in package order:
//!
//! - mask pages are the first layers of the coverage array, in the order of the package's `A8` pages; the glyph
//!   pages follow them ([`crate::GlyphStore::layer_base`]);
//! - colour pages are the layers of the image array, in the order of the package's `Rgba8` pages.
//!
//! An item refers to a page sprite with an [`crate::ImageRef`] whose `uv` is normalized to that page. The GPU
//! layers are square and at least as large as every page they hold, and a page sits in the top left corner of
//! its layer, so the encoder scales the `uv` by `page size / layer side` ([`Pages::mask_uv`],
//! [`Pages::image_uv`]).
//!
//! The pages change only when a skin is loaded or replaced ([`crate::Scene::set_pages`]); their revision tells
//! the pass when to upload them, so a frame without a skin change uploads none.

use std::collections::HashMap;
use std::sync::Arc;

use ui2_package::sections::blobs::MAX_PAGE_EDGE;
use ui2_package::{Images, PageFormat};

use crate::{DrawError, ExtTexture};

/// The most mask pages a package may bring: the layer index of a mask is 8 bits wide and the glyph pages need
/// layers after them.
pub const MAX_MASK_PAGES: usize = 64;
/// The most colour pages a package may bring (8-bit layer index; WebGL2 allows 256 array layers).
pub const MAX_IMAGE_PAGES: usize = 255;

/// One page of pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct StaticPage {
    /// The page's index in `Images::pages`.
    pub package_page: u16,
    /// 1 or 2: the device scale the page's sprites were rasterized for.
    pub scale: u8,
    /// Width and height in pixels.
    pub size: [u16; 2],
    /// Rows top to bottom: one byte per pixel for a mask page, premultiplied RGBA for a colour page.
    pub pixels: Arc<[u8]>,
}

impl StaticPage {
    /// Width in pixels.
    pub fn width(&self) -> u32 {
        u32::from(self.size[0])
    }

    /// Height in pixels.
    pub fn height(&self) -> u32 {
        u32::from(self.size[1])
    }

    fn same(&self, other: &StaticPage) -> bool {
        self.package_page == other.package_page
            && self.scale == other.scale
            && self.size == other.size
            && (Arc::ptr_eq(&self.pixels, &other.pixels) || self.pixels == other.pixels)
    }
}

/// The picture pages of the current skin.
#[derive(Debug, Clone, Default)]
pub struct Pages {
    mask: Vec<StaticPage>,
    image: Vec<StaticPage>,
    revision: u64,
}

fn scale_uv(uv: [f32; 4], size: [u16; 2], side: u32) -> [f32; 4] {
    if side == 0 {
        return uv;
    }
    let sx = f32::from(size[0]) / side as f32;
    let sy = f32::from(size[1]) / side as f32;
    [uv[0] * sx, uv[1] * sy, uv[2] * sx, uv[3] * sy]
}

impl Pages {
    /// The mask pages: layers `0..len` of the coverage array.
    pub fn mask(&self) -> &[StaticPage] {
        &self.mask
    }

    /// The colour pages: the layers of the image array.
    pub fn image(&self) -> &[StaticPage] {
        &self.image
    }

    /// Grows by one every time the pages change; 0 before the first change.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The longest edge of the mask pages (0 without any).
    pub fn mask_edge(&self) -> u32 {
        max_edge(&self.mask)
    }

    /// The side of the square layers of the image array: the longest edge of the colour pages (0 without any).
    pub fn image_side(&self) -> u32 {
        max_edge(&self.image)
    }

    /// Bytes of pixels held, both kinds.
    pub fn bytes(&self) -> usize {
        self.mask
            .iter()
            .chain(&self.image)
            .map(|p| p.pixels.len())
            .sum()
    }

    /// The `uv` of a sprite on mask page `layer` (normalized to the page) in a coverage array whose layers are
    /// `side` pixels square. A layer past the mask pages (a glyph page) is returned unchanged.
    pub fn mask_uv(&self, layer: u8, uv: [f32; 4], side: u32) -> [f32; 4] {
        match self.mask.get(usize::from(layer)) {
            Some(p) => scale_uv(uv, p.size, side),
            None => uv,
        }
    }

    /// The `uv` of a sprite on colour page `layer` (normalized to the page) in the image array; `None` when
    /// there is no such page.
    pub fn image_uv(&self, layer: u8, uv: [f32; 4]) -> Option<[f32; 4]> {
        let p = self.image.get(usize::from(layer))?;
        Some(scale_uv(uv, p.size, self.image_side()))
    }

    /// Replaces the pages with those of a package: `pages[i]` holds the decompressed pixels of
    /// `images.pages[i]`. Returns whether anything changed (equal pages keep the revision). Nothing changes on
    /// an error.
    pub(crate) fn replace(
        &mut self,
        images: &Images,
        pages: Vec<Arc<[u8]>>,
    ) -> Result<bool, DrawError> {
        if pages.len() != images.pages.len() {
            return Err(DrawError::InvalidPage(format!(
                "{} pixel buffers for {} pages",
                pages.len(),
                images.pages.len()
            )));
        }
        let mut mask = Vec::new();
        let mut image = Vec::new();
        for (i, (page, pixels)) in images.pages.iter().zip(pages).enumerate() {
            let [w, h] = page.size;
            if w == 0 || h == 0 || w > MAX_PAGE_EDGE || h > MAX_PAGE_EDGE {
                return Err(DrawError::InvalidPage(format!("page {i} is {w} x {h}")));
            }
            let want = usize::from(w) * usize::from(h) * page.format.bytes_per_pixel() as usize;
            if pixels.len() != want {
                return Err(DrawError::InvalidPage(format!(
                    "page {i} has {} bytes, {w} x {h} {:?} needs {want}",
                    pixels.len(),
                    page.format
                )));
            }
            let entry = StaticPage {
                package_page: i as u16,
                scale: page.scale,
                size: page.size,
                pixels,
            };
            match page.format {
                PageFormat::A8 => mask.push(entry),
                PageFormat::Rgba8 => image.push(entry),
            }
        }
        if mask.len() > MAX_MASK_PAGES || image.len() > MAX_IMAGE_PAGES {
            return Err(DrawError::InvalidPage(format!(
                "{} mask and {} colour pages (at most {MAX_MASK_PAGES} and {MAX_IMAGE_PAGES})",
                mask.len(),
                image.len()
            )));
        }
        let same = |a: &[StaticPage], b: &[StaticPage]| {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.same(y))
        };
        if same(&mask, &self.mask) && same(&image, &self.image) {
            return Ok(false);
        }
        self.mask = mask;
        self.image = image;
        self.revision += 1;
        Ok(true)
    }
}

/// The external textures the scene's instances refer to: each gets a slot (1 to 65535; 0 means none) held as
/// long as an encoded chunk uses it, shared by all items of the same texture. An instance carries the slot in
/// its `ext` field; the pass asks [`crate::Scene::ext_texture`] which texture that is.
#[derive(Debug, Default)]
pub struct ExtSlots {
    /// Slot `i + 1`: the texture and its reference count.
    slots: Vec<Option<(ExtTexture, u32)>>,
    map: HashMap<ExtTexture, u16>,
    free: Vec<u16>,
}

impl ExtSlots {
    /// The slot of `tex`, one reference more; `None` when all slots are taken.
    pub(crate) fn acquire(&mut self, tex: ExtTexture) -> Option<u16> {
        if let Some(&slot) = self.map.get(&tex) {
            if let Some(Some((_, refs))) = self.slots.get_mut(usize::from(slot) - 1) {
                *refs += 1;
            }
            return Some(slot);
        }
        let slot = match self.free.pop() {
            Some(s) => s,
            None => {
                let s = u16::try_from(self.slots.len() + 1).ok()?;
                self.slots.push(None);
                s
            }
        };
        self.slots[usize::from(slot) - 1] = Some((tex, 1));
        self.map.insert(tex, slot);
        Some(slot)
    }

    /// Drops one reference of a slot.
    pub(crate) fn release(&mut self, slot: u16) {
        let Some(entry) = usize::from(slot)
            .checked_sub(1)
            .and_then(|i| self.slots.get_mut(i))
        else {
            return;
        };
        let Some((tex, refs)) = entry else { return };
        *refs -= 1;
        if *refs == 0 {
            self.map.remove(tex);
            *entry = None;
            self.free.push(slot);
        }
    }

    /// The texture of a slot.
    pub fn get(&self, slot: u16) -> Option<ExtTexture> {
        let i = usize::from(slot).checked_sub(1)?;
        self.slots.get(i).copied().flatten().map(|(t, _)| t)
    }

    /// Slots in use.
    pub fn live(&self) -> usize {
        self.map.len()
    }
}

fn max_edge(pages: &[StaticPage]) -> u32 {
    pages
        .iter()
        .map(|p| p.width().max(p.height()))
        .max()
        .unwrap_or(0)
}
