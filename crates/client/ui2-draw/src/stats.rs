//! Counters of a [`crate::Scene`].

/// A snapshot of the scene's counters. The fields marked "frame" count since the last
/// [`crate::Scene::reset_frame_stats`]; the others describe the current state.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SceneStats {
    /// Chunks in the scene.
    pub chunks: usize,
    /// Instances in all layers.
    pub instances: usize,
    /// Frame: chunks encoded (a chunk set unchanged is not encoded).
    pub chunks_encoded: u32,
    /// Frame: milliseconds spent encoding (0 without a clock).
    pub encode_ms: f32,
    /// Frame: glyphs rasterized.
    pub raster_glyphs: u32,
    /// Frame: milliseconds spent rasterizing (0 without a clock).
    pub raster_ms: f32,
    /// Glyphs still waiting for rasterization.
    pub raster_deferred: u32,
    /// Glyphs in the atlas.
    pub atlas_glyphs: usize,
    /// Glyph pages made (the dynamic layers of the coverage array).
    pub atlas_pages: usize,
    /// Glyph page memory in bytes.
    pub atlas_bytes: usize,
    /// Glyphs that found no room in the glyph pages (each is retried a few frames later), since the start.
    pub atlas_full: u64,
    /// Frame: glyph pages cleared to make room (`ui2.atlas.evictions`).
    pub atlas_evictions: u32,
    /// Transform records and clips in use.
    pub xforms: usize,
    /// Clips in use.
    pub clips: usize,
    /// Since the start: transforms or clips that did not fit their table.
    pub table_overflow: u32,
    /// Frame: bytes written to anchor records (32 per `set_anchor` that changed a placement).
    pub anchor_bytes: u32,
    /// Picture pages of the package in the coverage array (`A8`).
    pub mask_pages: usize,
    /// Picture pages of the package in the image array (`Rgba8`).
    pub image_pages: usize,
    /// Bytes of picture page pixels held for the GPU.
    pub page_bytes: usize,
}
