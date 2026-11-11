//! Glyph placement and the glyph store: the pages of rasterized glyph coverage, the queue of glyphs that are
//! waiting to be rasterized, and the eviction of pages when they are all full.
//!
//! Every page is its own [`ui2_text::GlyphAtlas`] of one page (shelves 4 px high at least, 1 px margins, dirty
//! rectangles for upload), so that one page can be cleared without touching the others. The store adds what a
//! scene needs on top of the packing:
//!
//! - a lookup that never blocks: a missing glyph is queued and the caller gets a placeholder;
//! - a rasterization step with a budget;
//! - pages on demand, up to `AtlasConfig::max_pages` (the dynamic layers of the coverage array);
//! - eviction when a glyph fits on no page and no new page may be made. Every page remembers the frame it was
//!   last used in (a glyph placed on it or found on it) and how many chunks draw from it ([`GlyphStore::retain_pages`]).
//!   A page used in the last [`KEEP_FRAMES`] frames is never cleared. Of the others, a page no chunk draws from is
//!   cleared first (nothing visible is lost); otherwise the least recently used page is cleared and its old
//!   contents are kept aside until the scene has encoded the chunks that draw from it again: their glyphs are then
//!   copied from the old contents into the cleared page instead of being rasterized again
//!   ([`RasterReport::live_page`], [`GlyphStore::drop_stash`]). A page that is cleared bumps
//!   [`GlyphStore::evictions`] and its pixels are not uploaded again: the new glyphs overwrite their cells.
//! - When every page was used in the last frames, the glyph is not placed: [`GlyphStore::full_count`] counts it,
//!   [`GlyphStore::take_full_error`] reports it once, and it is retried [`KEEP_FRAMES`] frames later (the chunks
//!   that want it keep waiting for it).
//!
//! The pages follow the static mask pages of a skin package in the coverage array: [`crate::Scene::set_pages`] sets
//! [`GlyphStore::layer_base`].

use std::collections::{HashMap, HashSet, VecDeque};

use ui2_text::{
    AtlasConfig, AtlasError, AtlasSlot, DirtyRect, GlyphAtlas, GlyphBitmap, GlyphKey, TextEngine,
    quantize_x,
};

use crate::{GlyphPos, Glyphs};

/// A page that received or served a glyph in the current frame or the one before is not cleared.
pub const KEEP_FRAMES: u64 = 2;

/// Most glyph pages a store keeps track of in a chunk's page mask (`u8`).
pub const MAX_GLYPH_PAGES: u32 = 8;

/// The glyph key of `pos` in `run`, the whole pixel its bitmap origin is placed at, and the whole-pixel baseline.
// SIMPLIFIED(SIMP-246): glyphs are unhinted and sit at 4 horizontal subpixel positions (pen x rounded to a quarter pixel), baseline on a whole pixel
pub fn glyph_key(run: &Glyphs, pos: &GlyphPos) -> (GlyphKey, i32, i32) {
    let (px, sub) = quantize_x(pos.x);
    let key = GlyphKey::new(run.face, pos.glyph, run.size_px, run.wght, sub);
    (key, px, pos.y.round() as i32)
}

/// What a lookup found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lookup {
    /// The glyph is in the atlas (its slot may be empty: a space has no pixels). `slot.page` is the store's page.
    Ready(AtlasSlot),
    /// The glyph is queued for rasterization, or waits for room in a full atlas.
    Pending,
    /// The glyph cannot be placed (larger than a page).
    Unavailable,
}

/// What one [`GlyphStore::rasterize`] call did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RasterReport {
    /// Glyphs rasterized and placed.
    pub rasterized: u32,
    /// Glyphs still queued afterwards.
    pub deferred: u32,
    /// Glyphs not placed: larger than a page, or no page could be cleared for them.
    pub dropped: u32,
    /// Pages cleared to make room.
    pub evicted: u32,
    /// A page that chunks still draw from was cleared and the step stopped: the caller encodes those chunks
    /// again (their glyphs move from the old contents into the cleared page), then calls
    /// [`GlyphStore::drop_stash`] and may continue rasterizing.
    pub live_page: Option<u32>,
}

/// One page of the store.
struct GlyphPage {
    atlas: GlyphAtlas,
    /// Frame of the last placement or lookup hit.
    last_used: u64,
    /// Chunks that draw from this page.
    refs: u32,
}

/// The glyph pages and the queue of glyphs to rasterize.
pub struct GlyphStore {
    cfg: AtlasConfig,
    pages: Vec<GlyphPage>,
    /// Glyphs without pixels (spaces): they take no room and live on no page.
    blank: HashMap<GlyphKey, AtlasSlot>,
    pending: VecDeque<GlyphKey>,
    pending_set: HashSet<GlyphKey>,
    /// Glyphs larger than a page: never retried until [`GlyphStore::clear`].
    failed: HashSet<GlyphKey>,
    /// Glyphs that found no room: retried [`KEEP_FRAMES`] frames after the last such drop.
    full_keys: HashSet<GlyphKey>,
    full_frame: u64,
    /// The old contents of a cleared page that chunks still draw from, until they are encoded again.
    stash: Option<(usize, GlyphAtlas)>,
    layer_base: u8,
    frame: u64,
    full: u64,
    full_error: bool,
    full_reported: bool,
    evictions: u64,
    moved: u64,
    hits: u64,
    misses: u64,
}

impl GlyphStore {
    /// An empty store; pages are allocated when first needed. `cfg.max_pages` is capped at
    /// [`MAX_GLYPH_PAGES`].
    pub fn new(cfg: AtlasConfig) -> Self {
        let cfg = AtlasConfig {
            max_pages: cfg.max_pages.clamp(1, MAX_GLYPH_PAGES),
            ..cfg
        };
        Self {
            cfg,
            pages: Vec::new(),
            blank: HashMap::new(),
            pending: VecDeque::new(),
            pending_set: HashSet::new(),
            failed: HashSet::new(),
            full_keys: HashSet::new(),
            full_frame: 0,
            stash: None,
            layer_base: 0,
            frame: 0,
            full: 0,
            full_error: false,
            full_reported: false,
            evictions: 0,
            moved: 0,
            hits: 0,
            misses: 0,
        }
    }

    fn page_config(&self) -> AtlasConfig {
        AtlasConfig {
            max_pages: 1,
            ..self.cfg
        }
    }

    /// The page layout (`max_pages` is the most pages the store makes).
    pub fn config(&self) -> AtlasConfig {
        self.cfg
    }

    /// Side of a page in px.
    pub fn page_size(&self) -> u32 {
        self.cfg.page_size
    }

    /// Pages made so far (they stay until the store is dropped; a cleared page is reused).
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    /// Pixels of a page (`page_size * page_size` bytes, row-major, R8), if it has any.
    pub fn page(&self, page: u32) -> Option<&[u8]> {
        self.pages.get(page as usize)?.atlas.page(0)
    }

    /// The pixels of a dirty rectangle of [`GlyphStore::take_dirty`], tightly packed (`w * h` bytes).
    pub fn copy_rect(&self, r: &DirtyRect) -> Vec<u8> {
        match self.pages.get(r.page as usize) {
            Some(p) => p.atlas.copy_rect(&DirtyRect { page: 0, ..*r }),
            None => Vec::new(),
        }
    }

    /// The slot of a glyph in the store, without queueing or touching anything.
    pub fn get(&self, key: &GlyphKey) -> Option<AtlasSlot> {
        if let Some(s) = self.blank.get(key) {
            return Some(*s);
        }
        self.pages.iter().enumerate().find_map(|(i, p)| {
            p.atlas.get(key).map(|s| AtlasSlot {
                page: i as u32,
                ..s
            })
        })
    }

    /// Glyphs in the store (blank ones included).
    pub fn glyph_count(&self) -> usize {
        self.blank.len()
            + self
                .pages
                .iter()
                .map(|p| p.atlas.stats().glyphs)
                .sum::<usize>()
    }

    /// Lookups answered from the store, and glyphs placed (rasterized or moved), since the start.
    pub fn hits_misses(&self) -> (u64, u64) {
        (self.hits, self.misses)
    }

    /// Index in the `mask` texture array of page 0 (the static mask pages of a package come first).
    pub fn layer_base(&self) -> u8 {
        self.layer_base
    }

    /// Sets [`GlyphStore::layer_base`]; instances that carry glyph layers must be encoded again.
    pub fn set_layer_base(&mut self, base: u8) {
        self.layer_base = base;
    }

    /// The rectangles to upload since the last call (`page` is the store's page), and forgets them.
    pub fn take_dirty(&mut self) -> Vec<DirtyRect> {
        let mut out = Vec::new();
        for (i, p) in self.pages.iter_mut().enumerate() {
            out.extend(p.atlas.take_dirty().into_iter().map(|r| DirtyRect {
                page: i as u32,
                ..r
            }));
        }
        out
    }

    /// Glyphs waiting for rasterization.
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Glyphs that found no room, since the start.
    pub fn full_count(&self) -> u64 {
        self.full
    }

    /// True once, after the first glyph that found no room (for an error message).
    pub fn take_full_error(&mut self) -> bool {
        std::mem::take(&mut self.full_error)
    }

    /// Pages cleared to make room, since the start.
    pub fn evictions(&self) -> u64 {
        self.evictions
    }

    /// Glyphs copied from a cleared page into its new contents instead of being rasterized again.
    pub fn moved(&self) -> u64 {
        self.moved
    }

    /// The frame counter ([`GlyphStore::begin_frame`]).
    pub fn frame(&self) -> u64 {
        self.frame
    }

    /// Starts a frame: pages used before it age, and glyphs that found no room are queued again once
    /// [`KEEP_FRAMES`] frames passed.
    pub fn begin_frame(&mut self) {
        self.frame += 1;
        if !self.full_keys.is_empty() && self.frame >= self.full_frame + KEEP_FRAMES {
            for key in std::mem::take(&mut self.full_keys) {
                if self.pending_set.insert(key) {
                    self.pending.push_back(key);
                }
            }
        }
    }

    /// One more chunk draws from every page in `mask` (bit `i`: page `i`).
    pub fn retain_pages(&mut self, mask: u8) {
        for (i, p) in self.pages.iter_mut().enumerate() {
            if mask & (1 << i) != 0 {
                p.refs += 1;
            }
        }
    }

    /// One chunk less draws from every page in `mask`.
    pub fn release_pages(&mut self, mask: u8) {
        for (i, p) in self.pages.iter_mut().enumerate() {
            if mask & (1 << i) != 0 {
                p.refs = p.refs.saturating_sub(1);
            }
        }
    }

    /// Chunks that draw from a page.
    pub fn page_refs(&self, page: u32) -> u32 {
        self.pages.get(page as usize).map_or(0, |p| p.refs)
    }

    /// Finds `key`; a glyph that is not in the store is queued (once) and reported [`Lookup::Pending`]. A glyph
    /// of a cleared page that chunks still draw from is copied into the page's new contents.
    pub fn lookup(&mut self, key: GlyphKey) -> Lookup {
        if let Some(s) = self.blank.get(&key) {
            self.hits += 1;
            return Lookup::Ready(*s);
        }
        let frame = self.frame;
        for (i, p) in self.pages.iter_mut().enumerate() {
            if let Some(s) = p.atlas.get(&key) {
                p.last_used = frame;
                self.hits += 1;
                return Lookup::Ready(AtlasSlot {
                    page: i as u32,
                    ..s
                });
            }
        }
        if let Some(slot) = self.move_from_stash(key) {
            return Lookup::Ready(slot);
        }
        if self.failed.contains(&key) {
            return Lookup::Unavailable;
        }
        if !self.full_keys.contains(&key) && self.pending_set.insert(key) {
            self.pending.push_back(key);
        }
        Lookup::Pending
    }

    /// Copies a glyph of the old contents of a cleared page into the store.
    fn move_from_stash(&mut self, key: GlyphKey) -> Option<AtlasSlot> {
        let (_, old) = self.stash.as_ref()?;
        let s = old.get(&key)?;
        let bitmap = GlyphBitmap {
            w: s.w,
            h: s.h,
            left: s.left,
            top: s.top,
            coverage: old.copy_rect(&DirtyRect {
                page: 0,
                x: s.x,
                y: s.y,
                w: s.w,
                h: s.h,
            }),
        };
        let slot = self.place(key, &bitmap).ok()?;
        self.moved += 1;
        Some(slot)
    }

    /// The old contents of a cleared page are no longer needed (the chunks that drew from it were encoded again).
    pub fn drop_stash(&mut self) {
        self.stash = None;
    }

    /// Puts a bitmap on the first page with room, making a page when none has room and the store may grow.
    fn place(&mut self, key: GlyphKey, bitmap: &GlyphBitmap) -> Result<AtlasSlot, AtlasError> {
        if bitmap.w == 0 || bitmap.h == 0 {
            let slot = AtlasSlot {
                page: 0,
                x: 0,
                y: 0,
                w: 0,
                h: 0,
                left: bitmap.left,
                top: bitmap.top,
            };
            self.blank.insert(key, slot);
            self.misses += 1;
            return Ok(slot);
        }
        let frame = self.frame;
        for i in 0..self.pages.len() {
            match self.insert_on(i, key, bitmap, frame) {
                Err(AtlasError::Full) => continue,
                other => return other,
            }
        }
        if (self.pages.len() as u32) < self.cfg.max_pages {
            self.pages.push(GlyphPage {
                atlas: GlyphAtlas::new(self.page_config()),
                last_used: frame,
                refs: 0,
            });
            return self.insert_on(self.pages.len() - 1, key, bitmap, frame);
        }
        Err(AtlasError::Full)
    }

    fn insert_on(
        &mut self,
        i: usize,
        key: GlyphKey,
        bitmap: &GlyphBitmap,
        frame: u64,
    ) -> Result<AtlasSlot, AtlasError> {
        let p = &mut self.pages[i];
        let s = p.atlas.insert(key, bitmap)?;
        p.last_used = frame;
        self.misses += 1;
        Ok(AtlasSlot {
            page: i as u32,
            ..s
        })
    }

    /// Clears a page to make room: one no chunk draws from first, then the least recently used one, never one
    /// used in the last [`KEEP_FRAMES`] frames. Returns the page and whether chunks still draw from it (its old
    /// contents are then kept as the stash).
    fn evict(&mut self) -> Option<(usize, bool)> {
        let frame = self.frame;
        let (i, page) = self
            .pages
            .iter()
            .enumerate()
            .filter(|(_, p)| p.last_used + KEEP_FRAMES <= frame)
            .min_by_key(|(_, p)| (p.refs > 0, p.last_used))?;
        let live = page.refs > 0;
        if live && self.stash.is_some() {
            // One page at a time is moved; the caller has not finished the previous one.
            return None;
        }
        let fresh = GlyphAtlas::new(self.page_config());
        let old = std::mem::replace(&mut self.pages[i].atlas, fresh);
        self.pages[i].last_used = frame;
        self.evictions += 1;
        if live {
            self.stash = Some((i, old));
        }
        Some((i, live))
    }

    /// Rasterizes queued glyphs in request order. `may_continue(done)` is asked before every glyph after the first
    /// of the call (`done` glyphs were handled so far) and ends the step when it says no: that is how the caller
    /// keeps to a time budget. A glyph that fits nowhere clears a page ([`GlyphStore::evict`] rules); when the
    /// cleared page still had glyphs in use, the step stops with [`RasterReport::live_page`] set.
    pub fn rasterize(
        &mut self,
        text: &TextEngine,
        mut may_continue: impl FnMut(u32) -> bool,
    ) -> RasterReport {
        let mut report = RasterReport::default();
        let mut handled = 0u32;
        while let Some(&key) = self.pending.front() {
            if handled > 0 && !may_continue(handled) {
                break;
            }
            self.pending.pop_front();
            self.pending_set.remove(&key);
            handled += 1;
            if self.get(&key).is_some() {
                continue;
            }
            let bitmap = text.raster_glyph(key);
            match self.place(key, &bitmap) {
                Ok(_) => report.rasterized += 1,
                Err(AtlasError::TooLarge) => {
                    self.failed.insert(key);
                    report.dropped += 1;
                }
                Err(AtlasError::Full) => match self.evict() {
                    Some((i, live)) => {
                        report.evicted += 1;
                        let frame = self.frame;
                        match self.insert_on(i, key, &bitmap, frame) {
                            Ok(_) => report.rasterized += 1,
                            Err(_) => {
                                self.failed.insert(key);
                                report.dropped += 1;
                            }
                        }
                        if live {
                            report.live_page = Some(i as u32);
                            break;
                        }
                    }
                    None => {
                        self.full_keys.insert(key);
                        self.full_frame = self.frame;
                        self.full += 1;
                        if !self.full_reported {
                            self.full_reported = true;
                            self.full_error = true;
                        }
                        report.dropped += 1;
                    }
                },
            }
        }
        report.deferred = self.pending.len() as u32;
        report
    }

    /// True if the glyph is queued.
    pub fn is_pending(&self, key: &GlyphKey) -> bool {
        self.pending_set.contains(key)
    }

    /// Drops every glyph, the queue and the failure memory. The pages stay (zeroed, their used areas reported
    /// dirty so that the uploaded copy is zeroed too).
    pub fn clear(&mut self) {
        for p in &mut self.pages {
            p.atlas.clear();
        }
        self.blank.clear();
        self.pending.clear();
        self.pending_set.clear();
        self.failed.clear();
        self.full_keys.clear();
        self.stash = None;
    }
}
