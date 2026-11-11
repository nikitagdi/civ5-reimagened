//! The retained scene: layers of chunks, their encoded instances and revisions, anchors and the glyph store.
//!
//! A chunk is the unit of re-encoding: setting a chunk whose items are equal to the stored ones does nothing,
//! otherwise the chunk is encoded again and compared with the instances it had. A layer keeps the instances of its
//! chunks, in chunk order, in one contiguous vector; its revision grows only when those bytes change, and
//! [`Scene::take_layer_change`] tells the GPU pass what to upload: nothing, the byte ranges of chunks that kept their
//! instance count, the tail of the buffer from the first chunk whose count changed, or the whole buffer.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use ui2_package::Images;
use ui2_text::{AtlasConfig, DirtyRect, TextEngine};

use crate::encode::{EncodeCtx, encode_chunk};
use crate::glyphs::GlyphStore;
use crate::instance::Ui2Instance;
use crate::pages::{ExtSlots, Pages};
use crate::stats::SceneStats;
use crate::tables::{TableSegment, Tables};
use crate::{AnchorSlot, AnchorXform, ChunkId, ChunkItems, DrawError, ExtTexture, Item, LayerId};

/// Estimated cost of rasterizing one glyph in milliseconds, used to turn a time budget into a glyph count when the
/// scene has no clock (about 1.5 ms for the 235 glyphs of the HUD in wasm).
pub const FALLBACK_MS_PER_GLYPH: f64 = 0.006;

/// A monotonic clock in milliseconds. The host provides one (the scene itself reads no time source, so that the
/// crate builds for wasm).
pub type Clock = Box<dyn Fn() -> f64>;

/// What the GPU pass has to upload for a layer since the last [`Scene::take_layer_change`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LayerChange {
    /// Nothing changed.
    #[default]
    None,
    /// Chunks changed without changing the instance count: write these instance ranges (sorted, disjoint).
    Ranges(Vec<Range<usize>>),
    /// The instance count changed: the instances before this index are the same as before, write the rest (and
    /// the whole layer when the buffer has to grow).
    Tail(usize),
    /// Write the whole layer again (the first upload, or the count changed from the first chunk on).
    Full,
}

/// A draw call: instances `range` of `layer`'s buffer with external texture slot `ext` (0 = none). Consecutive
/// instances with the same `ext` share one op.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Op {
    /// The layer.
    pub layer: LayerId,
    /// Instance range in the layer's buffer.
    pub range: Range<u32>,
    /// External texture slot.
    pub ext: u16,
}

struct Chunk {
    layer: LayerId,
    items: ChunkItems,
    xforms: Vec<u16>,
    clips: Vec<u16>,
    exts: Vec<u16>,
    unresolved: u32,
    /// Glyph pages its instances sample (bit per page).
    glyph_pages: u8,
    revision: u64,
    offset: usize,
    len: usize,
    /// Its glyphs belong to a cleared atlas.
    stale: bool,
}

#[derive(Default)]
struct Layer {
    order: Vec<ChunkId>,
    instances: Vec<Ui2Instance>,
    revision: u64,
    change: LayerChange,
    ops: Vec<Op>,
    ops_stale: bool,
}

impl Layer {
    fn mark(&mut self, range: Range<usize>) {
        match &mut self.change {
            LayerChange::Full => {}
            LayerChange::Tail(t) => *t = (*t).min(range.start),
            LayerChange::None => self.change = LayerChange::Ranges(vec![range]),
            LayerChange::Ranges(r) => r.push(range),
        }
    }

    /// Everything from instance `start` on changed (`0`: the whole layer).
    fn mark_tail(&mut self, start: usize) {
        let start = match &self.change {
            LayerChange::Full => 0,
            LayerChange::None => start,
            LayerChange::Tail(t) => start.min(*t),
            LayerChange::Ranges(r) => r.iter().map(|x| x.start).fold(start, usize::min),
        };
        self.change = if start == 0 {
            LayerChange::Full
        } else {
            LayerChange::Tail(start)
        };
    }
}

#[derive(Default)]
struct Frame {
    chunks_encoded: u32,
    encode_ms: f64,
    raster_glyphs: u32,
    raster_ms: f64,
    anchor_bytes: u32,
    evictions: u32,
}

fn same_bytes(a: &[Ui2Instance], b: &[Ui2Instance]) -> bool {
    bytemuck::cast_slice::<_, u8>(a) == bytemuck::cast_slice::<_, u8>(b)
}

/// The retained UI scene.
pub struct Scene {
    k: f32,
    chunks: HashMap<ChunkId, Chunk>,
    layers: [Layer; 4],
    tables: Tables,
    glyphs: GlyphStore,
    pages: Pages,
    exts: ExtSlots,
    clock: Option<Clock>,
    overflow: u32,
    frame: Frame,
}

impl Scene {
    /// An empty scene for UI scale factor `k` (device pixels per CSS pixel).
    pub fn new(k: f32) -> Self {
        Self::with_atlas(k, AtlasConfig::default())
    }

    /// An empty scene whose glyph pages have the layout `atlas` (tests use small pages to exercise eviction).
    pub fn with_atlas(k: f32, atlas: AtlasConfig) -> Self {
        Self {
            k,
            chunks: HashMap::new(),
            layers: Default::default(),
            tables: Tables::new(),
            glyphs: GlyphStore::new(atlas),
            pages: Pages::default(),
            exts: ExtSlots::default(),
            clock: None,
            overflow: 0,
            frame: Frame::default(),
        }
    }

    /// Sets the time source used for the timing counters and the rasterization budget.
    pub fn set_clock(&mut self, clock: Clock) {
        self.clock = Some(clock);
    }

    pub(crate) fn now(&self) -> f64 {
        self.clock.as_ref().map_or(0.0, |c| c())
    }

    /// The scale factor.
    pub fn scale(&self) -> f32 {
        self.k
    }

    /// Changes the scale factor. When it changes, the glyph atlas is cleared: glyph sizes carry the factor, so the
    /// glyphs of the old factor are useless. The runtime sends its chunks again with the new sizes; chunks that
    /// still hold glyphs of the old size are encoded again by [`Scene::reencode_stale`] (and `resolve`).
    pub fn set_scale(&mut self, k: f32) {
        if k == self.k {
            return;
        }
        self.k = k;
        self.glyphs.clear();
        for ch in self.chunks.values_mut() {
            ch.stale = ch.items.items.iter().any(|i| matches!(i, Item::Glyphs(_)));
        }
    }

    /// Sets the items of a chunk in a layer (creating the chunk, appended after the layer's chunks, or replacing
    /// its items in place). Equal items leave everything untouched. A chunk id that lives in another layer is
    /// moved.
    pub fn set_chunk(&mut self, layer: LayerId, id: ChunkId, items: ChunkItems) {
        match self.chunks.remove(&id) {
            Some(old) if old.layer == layer && old.items == items && !old.stale => {
                self.chunks.insert(id, old);
            }
            Some(old) if old.layer != layer => {
                self.drop_chunk(id, old);
                self.install(id, layer, items, None);
            }
            old => self.install(id, layer, items, old),
        }
    }

    /// Removes a chunk and its instances.
    pub fn remove_chunk(&mut self, id: ChunkId) {
        if let Some(old) = self.chunks.remove(&id) {
            self.drop_chunk(id, old);
        }
    }

    /// Encodes again the chunks whose glyphs belong to a cleared atlas (after [`Scene::set_scale`]).
    pub fn reencode_stale(&mut self) {
        let ids: Vec<ChunkId> = self
            .chunks
            .iter()
            .filter(|(_, c)| c.stale)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.reencode(id);
        }
    }

    /// Encodes one chunk again from its stored items.
    pub(crate) fn reencode(&mut self, id: ChunkId) {
        if let Some(old) = self.chunks.remove(&id) {
            let (layer, items) = (old.layer, old.items.clone());
            self.install(id, layer, items, Some(old));
        }
    }

    fn release(&mut self, ch: &Chunk) {
        for x in &ch.xforms {
            self.tables.release_xform(*x);
        }
        for c in &ch.clips {
            self.tables.release_clip(*c);
        }
        for e in &ch.exts {
            self.exts.release(*e);
        }
        self.glyphs.release_pages(ch.glyph_pages);
    }

    fn drop_chunk(&mut self, id: ChunkId, old: Chunk) {
        self.release(&old);
        let li = old.layer.index();
        self.layers[li].order.retain(|c| *c != id);
        if old.len > 0 {
            self.rebuild_layer(li, None, old.offset);
        }
    }

    fn install(&mut self, id: ChunkId, layer: LayerId, items: ChunkItems, old: Option<Chunk>) {
        let t0 = self.now();
        let mask_side = self.mask_side();
        let enc = {
            let mut ctx = EncodeCtx {
                tables: &mut self.tables,
                glyphs: &mut self.glyphs,
                table_overflow: &mut self.overflow,
                pages: &self.pages,
                mask_side,
                exts: &mut self.exts,
            };
            encode_chunk(&items, &mut ctx)
        };
        self.glyphs.retain_pages(enc.glyph_pages);
        // Release after acquiring, so that entries both versions use keep their records.
        if let Some(o) = &old {
            self.release(o);
        }
        self.frame.chunks_encoded += 1;
        self.frame.encode_ms += self.now() - t0;

        let li = layer.index();
        let len = enc.instances.len();
        let mut chunk = Chunk {
            layer,
            items,
            xforms: enc.xforms,
            clips: enc.clips,
            exts: enc.exts,
            unresolved: enc.unresolved,
            glyph_pages: enc.glyph_pages,
            revision: old.as_ref().map_or(1, |o| o.revision + 1),
            offset: 0,
            len,
            stale: false,
        };
        match old {
            Some(o) if o.len == len => {
                chunk.offset = o.offset;
                let range = o.offset..o.offset + len;
                let lay = &mut self.layers[li];
                if same_bytes(&lay.instances[range.clone()], &enc.instances) {
                    chunk.revision = o.revision;
                } else {
                    lay.instances[range.clone()].copy_from_slice(&enc.instances);
                    lay.mark(range);
                    lay.revision += 1;
                    lay.ops_stale = true;
                }
                self.chunks.insert(id, chunk);
            }
            old => {
                match &old {
                    None => {
                        self.layers[li].order.push(id);
                        chunk.offset = self.layers[li].instances.len();
                    }
                    Some(o) => chunk.offset = o.offset,
                }
                // The instances before the chunk stay where they are.
                let start = chunk.offset;
                self.chunks.insert(id, chunk);
                if old.is_some() || len > 0 {
                    self.rebuild_layer(li, Some((id, &enc.instances)), start);
                }
            }
        }
    }

    /// Concatenates the chunks of a layer again (`replace` supplies the new instances of one chunk); the instances
    /// before `start` do not move.
    fn rebuild_layer(
        &mut self,
        li: usize,
        replace: Option<(ChunkId, &[Ui2Instance])>,
        start: usize,
    ) {
        let lay = &mut self.layers[li];
        let old = std::mem::take(&mut lay.instances);
        let mut new = Vec::with_capacity(old.len() + replace.map_or(0, |r| r.1.len()));
        for cid in &lay.order {
            let Some(ch) = self.chunks.get_mut(cid) else {
                continue;
            };
            let start = new.len();
            match replace {
                Some((rid, ins)) if rid == *cid => new.extend_from_slice(ins),
                _ => {
                    if let Some(src) = old.get(ch.offset..ch.offset + ch.len) {
                        new.extend_from_slice(src);
                    }
                }
            }
            ch.offset = start;
            ch.len = new.len() - start;
        }
        lay.instances = new;
        lay.revision += 1;
        lay.mark_tail(start);
        lay.ops_stale = true;
    }

    // ----- glyphs -----

    /// Rasterizes queued glyphs for at most `budget_ms` (at least one glyph per call, so the queue always makes
    /// progress), places them in the atlas and encodes again the chunks that were waiting for them. What is left
    /// stays queued for the next call: a text change shows within two frames. Call it once per frame, after the
    /// chunks were set and before the pass prepares.
    ///
    /// The budget is measured with the [`Clock`] of [`Scene::set_clock`]; without one it is converted to a glyph
    /// count at [`FALLBACK_MS_PER_GLYPH`] per glyph.
    pub fn resolve(&mut self, text: &mut TextEngine, budget_ms: f32) {
        self.glyphs.begin_frame();
        self.reencode_stale();
        let budget = f64::from(budget_ms);
        let t0 = self.now();
        let mut done_before = 0u32;
        let (mut placed, mut dropped) = (0u32, 0u32);
        // Each round ends with the queue empty, the budget spent, or a page with glyphs in use cleared; at most
        // one round per page clears a page in use.
        for _ in 0..=self.glyphs.config().max_pages {
            let report = {
                let clock = self.clock.as_ref();
                let base = done_before;
                self.glyphs.rasterize(text, |done| {
                    let spent = match clock {
                        Some(c) => c() - t0,
                        None => f64::from(base + done) * FALLBACK_MS_PER_GLYPH,
                    };
                    spent < budget
                })
            };
            done_before += report.rasterized + report.dropped;
            placed += report.rasterized;
            dropped += report.dropped;
            self.frame.evictions += report.evicted;
            let Some(page) = report.live_page else {
                break;
            };
            // The chunks that drew from the cleared page take their glyphs into its new contents.
            let users: Vec<ChunkId> = self
                .chunks
                .iter()
                .filter(|(_, c)| c.glyph_pages & (1 << page) != 0)
                .map(|(id, _)| *id)
                .collect();
            for id in users {
                self.reencode(id);
            }
            self.glyphs.drop_stash();
            let spent = match &self.clock {
                Some(c) => c() - t0,
                None => f64::from(done_before) * FALLBACK_MS_PER_GLYPH,
            };
            if spent >= budget {
                break;
            }
        }
        self.frame.raster_glyphs += placed;
        self.frame.raster_ms += self.now() - t0;
        if placed + dropped > 0 {
            let waiting: Vec<ChunkId> = self
                .chunks
                .iter()
                .filter(|(_, c)| c.unresolved > 0)
                .map(|(id, _)| *id)
                .collect();
            for id in waiting {
                self.reencode(id);
            }
        }
    }

    // ----- picture pages -----

    /// Takes the picture pages of a skin package: `pages[i]` is the decompressed pixels of `images.pages[i]`
    /// (`Package::page_data`). `A8` pages become the first layers of the coverage array (the glyph pages move
    /// after them), `Rgba8` pages the layers of the image array; the pass uploads them on its next `prepare`.
    /// Replaces the pages of an earlier skin; an empty table drops them. Pages equal to the held ones change
    /// nothing. When the pages change, the chunks that draw masks, pictures or glyphs are encoded again (their
    /// layers and sprite rectangles depend on the pages).
    ///
    /// A page count, size or pixel length that does not match the table is an error and leaves the scene as it
    /// was.
    pub fn set_pages(&mut self, images: &Images, pages: Vec<Arc<[u8]>>) -> Result<(), DrawError> {
        if !self.pages.replace(images, pages)? {
            return Ok(());
        }
        // `replace` keeps the mask page count within `MAX_MASK_PAGES`, so it fits a layer index.
        self.glyphs
            .set_layer_base(u8::try_from(self.pages.mask().len()).unwrap_or(u8::MAX));
        let ids: Vec<ChunkId> = self
            .chunks
            .iter()
            .filter(|(_, c)| {
                c.items
                    .items
                    .iter()
                    .any(|i| matches!(i, Item::Mask(_) | Item::Image(_) | Item::Glyphs(_)))
            })
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.reencode(id);
        }
        Ok(())
    }

    /// The picture pages the scene holds.
    pub fn pages(&self) -> &Pages {
        &self.pages
    }

    /// Side of the square layers of the coverage array, px: the glyph page size, or the longest mask page edge
    /// when that is larger.
    pub fn mask_side(&self) -> u32 {
        self.glyphs.page_size().max(self.pages.mask_edge())
    }

    // ----- anchors -----

    /// Takes an anchor slot; it is hidden until [`Scene::set_anchor`].
    pub fn alloc_anchor(&mut self) -> Result<AnchorSlot, DrawError> {
        self.tables.alloc_anchor()
    }

    /// Returns an anchor slot (the chunks that use it must be removed or moved by the caller).
    pub fn free_anchor(&mut self, slot: AnchorSlot) {
        self.tables.free_anchor(slot);
    }

    /// Writes the screen placement of an anchor: 32 bytes of the transform table, nothing else.
    pub fn set_anchor(&mut self, slot: AnchorSlot, xform: AnchorXform) {
        self.tables.set_anchor(slot, xform);
        self.frame.anchor_bytes += 32;
    }

    // ----- what the GPU pass reads -----

    /// The instances of a layer, chunks in order.
    pub fn layer_instances(&self, layer: LayerId) -> &[Ui2Instance] {
        &self.layers[layer.index()].instances
    }

    /// Revision of a layer: grows exactly when its instance bytes change.
    pub fn layer_revision(&self, layer: LayerId) -> u64 {
        self.layers[layer.index()].revision
    }

    /// What to upload for the layer, and forgets it.
    pub fn take_layer_change(&mut self, layer: LayerId) -> LayerChange {
        match std::mem::take(&mut self.layers[layer.index()].change) {
            LayerChange::Ranges(mut r) => {
                r.sort_by_key(|x| x.start);
                let mut out: Vec<Range<usize>> = Vec::with_capacity(r.len());
                for x in r {
                    match out.last_mut() {
                        Some(last) if x.start <= last.end => last.end = last.end.max(x.end),
                        _ => out.push(x),
                    }
                }
                LayerChange::Ranges(out)
            }
            other => other,
        }
    }

    /// The draw calls of a layer: instance runs by external texture.
    pub fn layer_ops(&mut self, layer: LayerId) -> &[Op] {
        let lay = &mut self.layers[layer.index()];
        if lay.ops_stale {
            lay.ops.clear();
            let mut start = 0usize;
            for i in 1..=lay.instances.len() {
                let boundary =
                    i == lay.instances.len() || lay.instances[i].ext != lay.instances[start].ext;
                if boundary {
                    lay.ops.push(Op {
                        layer,
                        range: start as u32..i as u32,
                        ext: lay.instances[start].ext,
                    });
                    start = i;
                }
            }
            lay.ops_stale = false;
        }
        &lay.ops
    }

    /// The instances of one chunk (a slice of its layer's buffer).
    pub fn chunk_instances(&self, id: ChunkId) -> Option<&[Ui2Instance]> {
        let ch = self.chunks.get(&id)?;
        self.layers[ch.layer.index()]
            .instances
            .get(ch.offset..ch.offset + ch.len)
    }

    /// The instance range of a chunk in its layer's buffer.
    pub fn chunk_range(&self, id: ChunkId) -> Option<Range<usize>> {
        self.chunks.get(&id).map(|c| c.offset..c.offset + c.len)
    }

    /// Glyphs of a chunk that were queued for rasterization when it was encoded.
    pub fn chunk_unresolved(&self, id: ChunkId) -> Option<u32> {
        self.chunks.get(&id).map(|c| c.unresolved)
    }

    /// Revision of a chunk: grows when its encoded instances change.
    pub fn chunk_revision(&self, id: ChunkId) -> Option<u64> {
        self.chunks.get(&id).map(|c| c.revision)
    }

    /// The external texture of an instance's xt slot (None for 0 and for free slots).
    pub fn ext_texture(&self, slot: u16) -> Option<ExtTexture> {
        self.exts.get(slot)
    }

    /// The external texture slots.
    pub fn ext_slots(&self) -> &ExtSlots {
        &self.exts
    }

    /// The changed ranges of the transform and clip tables, and forgets them.
    pub fn take_table_uploads(&mut self) -> Vec<TableSegment> {
        self.tables.take_dirty()
    }

    /// The tables (for the first, full upload).
    pub fn tables(&self) -> &Tables {
        &self.tables
    }

    /// Atlas rectangles to upload since the last call.
    pub fn take_glyph_dirty(&mut self) -> Vec<DirtyRect> {
        self.glyphs.take_dirty()
    }

    /// The glyph store (pages, slots, counters).
    pub fn glyphs(&self) -> &GlyphStore {
        &self.glyphs
    }

    /// True once, after the first glyph that found no room in the glyph pages (every page was in use in the
    /// last frames): the host logs it as an error. [`SceneStats::atlas_full`] keeps counting.
    pub fn take_atlas_full_error(&mut self) -> bool {
        self.glyphs.take_full_error()
    }

    // ----- counters -----

    /// A snapshot of the counters.
    pub fn stats(&self) -> SceneStats {
        let page = self.glyphs.page_size() as usize;
        let pages = self.glyphs.page_count();
        let (xforms, clips) = self.tables.live();
        SceneStats {
            chunks: self.chunks.len(),
            instances: self.layers.iter().map(|l| l.instances.len()).sum(),
            chunks_encoded: self.frame.chunks_encoded,
            encode_ms: self.frame.encode_ms as f32,
            raster_glyphs: self.frame.raster_glyphs,
            raster_ms: self.frame.raster_ms as f32,
            raster_deferred: self.glyphs.pending_len() as u32,
            atlas_glyphs: self.glyphs.glyph_count(),
            atlas_pages: pages,
            atlas_bytes: pages * page * page,
            atlas_full: self.glyphs.full_count(),
            atlas_evictions: self.frame.evictions,
            xforms,
            clips,
            table_overflow: self.overflow,
            anchor_bytes: self.frame.anchor_bytes,
            mask_pages: self.pages.mask().len(),
            image_pages: self.pages.image().len(),
            page_bytes: self.pages.bytes(),
        }
    }

    /// Zeroes the per-frame counters; the host calls it after publishing [`Scene::stats`].
    pub fn reset_frame_stats(&mut self) {
        self.frame = Frame::default();
    }
}
