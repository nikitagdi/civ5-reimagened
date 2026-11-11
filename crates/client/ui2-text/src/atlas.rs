//! A CPU-side R8 glyph atlas: shelf packing, a per-glyph cache and dirty rectangles for upload.
//!
//! The atlas owns the pixels of up to `max_pages` square pages; the renderer uploads what
//! [`GlyphAtlas::take_dirty`] reports. Every glyph sits in a cell with a 1 px margin of zeros around it, and shelves
//! are as tall as their tallest cell rounded up to a multiple of 4 px. There is no eviction: when the pages are full
//! [`AtlasError::Full`] is returned and the owner calls [`GlyphAtlas::clear`] and re-rasterizes what it still
//! needs (the new frame's glyphs).

use std::collections::HashMap;

use crate::TextEngine;
use crate::raster::{GlyphBitmap, GlyphKey};

/// Page layout of a [`GlyphAtlas`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtlasConfig {
    /// Side of a square page in px.
    pub page_size: u32,
    /// Maximum number of pages (texture array layers).
    pub max_pages: u32,
    /// Margin of zeros around every glyph, px.
    pub padding: u32,
    /// Shelf heights are rounded up to a multiple of this.
    pub shelf_quantum: u32,
}

impl Default for AtlasConfig {
    /// 2048 px pages, up to 4 of them, 1 px margin, shelves in steps of 4 px.
    fn default() -> Self {
        Self {
            page_size: 2048,
            max_pages: 4,
            padding: 1,
            shelf_quantum: 4,
        }
    }
}

/// Where a glyph lives in the atlas, and how to place it on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtlasSlot {
    /// Page (array layer).
    pub page: u32,
    /// Left edge of the glyph's pixels in the page.
    pub x: u32,
    /// Top edge of the glyph's pixels in the page.
    pub y: u32,
    /// Width of the glyph's pixels (0 for glyphs without an outline).
    pub w: u32,
    /// Height of the glyph's pixels.
    pub h: u32,
    /// [`GlyphBitmap::left`].
    pub left: i32,
    /// [`GlyphBitmap::top`].
    pub top: i32,
}

/// A rectangle of a page that changed since the last [`GlyphAtlas::take_dirty`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirtyRect {
    /// Page (array layer).
    pub page: u32,
    /// Left edge.
    pub x: u32,
    /// Top edge.
    pub y: u32,
    /// Width.
    pub w: u32,
    /// Height.
    pub h: u32,
}

/// Why a glyph could not be placed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AtlasError {
    /// All pages are full.
    #[error("glyph atlas is full")]
    Full,
    /// The glyph is larger than a page.
    #[error("glyph does not fit in an atlas page")]
    TooLarge,
}

struct Shelf {
    y: u32,
    h: u32,
    next_x: u32,
}

struct Page {
    pixels: Vec<u8>,
    shelves: Vec<Shelf>,
    next_y: u32,
}

/// Counters of the atlas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AtlasStats {
    /// Lookups answered from the cache.
    pub hits: u64,
    /// Glyphs rasterized and inserted.
    pub misses: u64,
    /// Glyphs cached now.
    pub glyphs: usize,
    /// Pages in use.
    pub pages: usize,
}

/// The glyph cache and its pages.
pub struct GlyphAtlas {
    cfg: AtlasConfig,
    pages: Vec<Page>,
    slots: HashMap<GlyphKey, AtlasSlot>,
    dirty: Vec<DirtyRect>,
    hits: u64,
    misses: u64,
}

impl GlyphAtlas {
    /// An empty atlas; pages are allocated when first needed.
    pub fn new(cfg: AtlasConfig) -> Self {
        Self {
            cfg,
            pages: Vec::new(),
            slots: HashMap::new(),
            dirty: Vec::new(),
            hits: 0,
            misses: 0,
        }
    }

    /// The layout the atlas was made with.
    pub fn config(&self) -> AtlasConfig {
        self.cfg
    }

    /// The slot of a cached glyph, without touching the counters.
    pub fn get(&self, key: &GlyphKey) -> Option<AtlasSlot> {
        self.slots.get(key).copied()
    }

    /// The slot of `key`, rasterizing and packing the glyph on a miss.
    pub fn get_or_raster(
        &mut self,
        engine: &TextEngine,
        key: GlyphKey,
    ) -> Result<AtlasSlot, AtlasError> {
        if let Some(slot) = self.slots.get(&key) {
            self.hits += 1;
            return Ok(*slot);
        }
        let bitmap = engine.raster_glyph(key);
        self.insert(key, &bitmap)
    }

    /// Packs `bitmap` under `key` (replacing nothing: an existing slot is returned unchanged).
    pub fn insert(&mut self, key: GlyphKey, bitmap: &GlyphBitmap) -> Result<AtlasSlot, AtlasError> {
        if let Some(slot) = self.slots.get(&key) {
            return Ok(*slot);
        }
        let slot = if bitmap.w == 0 || bitmap.h == 0 {
            AtlasSlot {
                page: 0,
                x: 0,
                y: 0,
                w: 0,
                h: 0,
                left: bitmap.left,
                top: bitmap.top,
            }
        } else {
            self.place(bitmap)?
        };
        self.misses += 1;
        self.slots.insert(key, slot);
        Ok(slot)
    }

    fn place(&mut self, bitmap: &GlyphBitmap) -> Result<AtlasSlot, AtlasError> {
        let pad = self.cfg.padding;
        let size = self.cfg.page_size;
        let (cell_w, cell_h) = (bitmap.w + 2 * pad, bitmap.h + 2 * pad);
        if cell_w > size || cell_h > size {
            return Err(AtlasError::TooLarge);
        }
        let shelf_h = cell_h.div_ceil(self.cfg.shelf_quantum) * self.cfg.shelf_quantum;

        // The shortest existing shelf that is tall enough and has room.
        let mut best: Option<(usize, usize, u32)> = None; // (page, shelf, height)
        for (pi, page) in self.pages.iter().enumerate() {
            for (si, s) in page.shelves.iter().enumerate() {
                if s.h >= cell_h && s.next_x + cell_w <= size && best.is_none_or(|b| s.h < b.2) {
                    best = Some((pi, si, s.h));
                }
            }
        }
        let (pi, si) = match best {
            Some((pi, si, _)) => (pi, si),
            None => {
                // A new shelf: on a page with vertical room, else on a new page.
                let pi = match self.pages.iter().position(|p| p.next_y + shelf_h <= size) {
                    Some(pi) => pi,
                    None if (self.pages.len() as u32) < self.cfg.max_pages => {
                        self.pages.push(Page {
                            pixels: vec![0; (size * size) as usize],
                            shelves: Vec::new(),
                            next_y: 0,
                        });
                        self.pages.len() - 1
                    }
                    None => return Err(AtlasError::Full),
                };
                let page = &mut self.pages[pi];
                page.shelves.push(Shelf {
                    y: page.next_y,
                    h: shelf_h,
                    next_x: 0,
                });
                page.next_y += shelf_h;
                (pi, page.shelves.len() - 1)
            }
        };

        let page = &mut self.pages[pi];
        let shelf = &mut page.shelves[si];
        let (cell_x, cell_y, h) = (shelf.next_x, shelf.y, shelf.h);
        shelf.next_x += cell_w;
        let (gx, gy) = (cell_x + pad, cell_y + pad);
        for row in 0..bitmap.h as usize {
            let src = &bitmap.coverage[row * bitmap.w as usize..(row + 1) * bitmap.w as usize];
            let dst = ((gy as usize + row) * size as usize) + gx as usize;
            page.pixels[dst..dst + bitmap.w as usize].copy_from_slice(src);
        }
        self.mark_dirty(DirtyRect {
            page: pi as u32,
            x: cell_x,
            y: cell_y,
            w: cell_w,
            h,
        });
        Ok(AtlasSlot {
            page: pi as u32,
            x: gx,
            y: gy,
            w: bitmap.w,
            h: bitmap.h,
            left: bitmap.left,
            top: bitmap.top,
        })
    }

    /// Records a changed rectangle, merging it into the previous one when they touch on the same shelf.
    fn mark_dirty(&mut self, r: DirtyRect) {
        if let Some(last) = self.dirty.last_mut()
            && last.page == r.page
            && last.y == r.y
            && last.h == r.h
            && last.x + last.w == r.x
        {
            last.w += r.w;
            return;
        }
        self.dirty.push(r);
    }

    /// The rectangles to upload since the last call, and forgets them.
    pub fn take_dirty(&mut self) -> Vec<DirtyRect> {
        std::mem::take(&mut self.dirty)
    }

    /// True if [`GlyphAtlas::take_dirty`] would return something.
    pub fn has_dirty(&self) -> bool {
        !self.dirty.is_empty()
    }

    /// Pixels of a page (`page_size * page_size` bytes, row-major, R8), if the page exists.
    pub fn page(&self, page: u32) -> Option<&[u8]> {
        self.pages.get(page as usize).map(|p| p.pixels.as_slice())
    }

    /// The pixels of a dirty rectangle, tightly packed (`w * h` bytes), ready for a texture upload.
    pub fn copy_rect(&self, r: &DirtyRect) -> Vec<u8> {
        let Some(page) = self.pages.get(r.page as usize) else {
            return Vec::new();
        };
        let size = self.cfg.page_size as usize;
        let mut out = Vec::with_capacity((r.w * r.h) as usize);
        for row in 0..r.h as usize {
            let start = (r.y as usize + row) * size + r.x as usize;
            out.extend_from_slice(&page.pixels[start..start + r.w as usize]);
        }
        out
    }

    /// Drops every glyph and zeroes the pages. The used area of each page is reported dirty so that the uploaded
    /// copy is cleared too.
    pub fn clear(&mut self) {
        self.slots.clear();
        self.dirty.clear();
        let size = self.cfg.page_size;
        for (pi, page) in self.pages.iter_mut().enumerate() {
            if page.next_y > 0 {
                self.dirty.push(DirtyRect {
                    page: pi as u32,
                    x: 0,
                    y: 0,
                    w: size,
                    h: page.next_y,
                });
            }
            page.pixels.fill(0);
            page.shelves.clear();
            page.next_y = 0;
        }
    }

    /// Counters.
    pub fn stats(&self) -> AtlasStats {
        AtlasStats {
            hits: self.hits,
            misses: self.misses,
            glyphs: self.slots.len(),
            pages: self.pages.len(),
        }
    }
}
