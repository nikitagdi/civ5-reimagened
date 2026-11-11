//! Glyph placement and the glyph store: the atlas of rasterized glyph coverage plus the queue of glyphs that
//! are waiting to be rasterized.
//!
//! The packing itself (shelves 4 px high at least, 1 px margins, dirty rectangles for upload) is
//! [`ui2_text::GlyphAtlas`]; this module adds what a scene needs on top of it: a lookup that never blocks (a
//! missing glyph is queued and the caller gets a placeholder), and a rasterization step with a budget.
//!
//! Gaps against the render-layers design, left for the eviction task: the atlas has no eviction (when its pages are
//! full the glyph is dropped, counted in [`GlyphStore::full_count`]), and its pages are separate from the static
//! mask pages of a skin package (the scene only adds a base layer index, [`GlyphStore::layer_base`]).

use std::collections::{HashSet, VecDeque};

use ui2_text::{AtlasConfig, AtlasSlot, DirtyRect, GlyphAtlas, GlyphKey, quantize_x};

use crate::{GlyphPos, Glyphs};

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
    /// The glyph is in the atlas (its slot may be empty: a space has no pixels).
    Ready(AtlasSlot),
    /// The glyph is queued for rasterization.
    Pending,
    /// The glyph cannot be placed (atlas full, glyph larger than a page).
    Unavailable,
}

/// The atlas of glyph coverage and the queue of glyphs to rasterize.
pub struct GlyphStore {
    atlas: GlyphAtlas,
    pending: VecDeque<GlyphKey>,
    pending_set: HashSet<GlyphKey>,
    failed: HashSet<GlyphKey>,
    layer_base: u8,
    full: u64,
}

impl GlyphStore {
    /// An empty store; pages are allocated when first needed.
    pub fn new(cfg: AtlasConfig) -> Self {
        Self {
            atlas: GlyphAtlas::new(cfg),
            pending: VecDeque::new(),
            pending_set: HashSet::new(),
            failed: HashSet::new(),
            layer_base: 0,
            full: 0,
        }
    }

    /// The atlas (page pixels and slots).
    pub fn atlas(&self) -> &GlyphAtlas {
        &self.atlas
    }

    /// Side of an atlas page in px.
    pub fn page_size(&self) -> u32 {
        self.atlas.config().page_size
    }

    /// Index in the `mask` texture array of atlas page 0 (the static mask pages of a package come first).
    pub fn layer_base(&self) -> u8 {
        self.layer_base
    }

    /// Sets [`GlyphStore::layer_base`]; instances that carry glyph layers must be encoded again.
    pub fn set_layer_base(&mut self, base: u8) {
        self.layer_base = base;
    }

    /// The rectangles of the atlas to upload since the last call.
    pub fn take_dirty(&mut self) -> Vec<DirtyRect> {
        self.atlas.take_dirty()
    }

    /// Glyphs waiting for rasterization.
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// Glyphs dropped because the atlas was full, since the start.
    pub fn full_count(&self) -> u64 {
        self.full
    }

    /// Finds `key`; a glyph that is not in the atlas is queued (once) and reported [`Lookup::Pending`].
    pub fn lookup(&mut self, key: GlyphKey) -> Lookup {
        if let Some(slot) = self.atlas.get(&key) {
            return Lookup::Ready(slot);
        }
        if self.failed.contains(&key) {
            return Lookup::Unavailable;
        }
        if self.pending_set.insert(key) {
            self.pending.push_back(key);
        }
        Lookup::Pending
    }

    /// True if the glyph is queued.
    pub fn is_pending(&self, key: &GlyphKey) -> bool {
        self.pending_set.contains(key)
    }

    /// Drops every glyph, the queue and the failure memory (the atlas pages are reported dirty so that the
    /// uploaded copy is zeroed).
    pub fn clear(&mut self) {
        self.atlas.clear();
        self.pending.clear();
        self.pending_set.clear();
        self.failed.clear();
    }
}
