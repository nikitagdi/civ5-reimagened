//! UI v2 text stack: font loading, shaping, line metrics, line breaking and glyph rasterization,
//! laid out to match Chrome's text layout.
//!
//! One code path for the browser (wasm) and for native tests. The crate has no GPU, no I/O and no
//! clock: fonts arrive as bytes, glyph coverage leaves as byte buffers.
//!
//! The matching rules (Blink behaviour reproduced here) are documented where they are implemented:
//! feature selection and NBSP in [`shape`], width snapping in [`shape`], ascent/descent rounding and
//! baseline placement in [`metrics`], greedy breaking with hanging spaces in [`breaker`].

#![forbid(unsafe_code)]

pub mod breaker;
mod face;
pub mod metrics;
pub mod shape;

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

pub use breaker::{BreakOptions, Ellipsized, Line, LineHeight, LineSegment, Lines, TextMeasure};
use face::Face;
pub use metrics::LineMetrics;
use shape::ShapeCache;
pub use shape::{
    NumericFigure, NumericSpacing, NumericVariant, RunStyle, SHAPE_CACHE_CAPACITY, ShapeCacheStats,
    ShapedGlyph, ShapedRun,
};

/// Identifier of a loaded font face, chosen by the caller (for example an index into the skin's font table).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FaceId(pub u32);

impl fmt::Display for FaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "face#{}", self.0)
    }
}

/// Errors of the text stack.
#[derive(Debug, thiserror::Error)]
pub enum TextError {
    /// The font bytes could not be parsed.
    #[error("cannot parse font: {0}")]
    Font(String),
    /// No face was loaded under this id.
    #[error("unknown font face {0}")]
    UnknownFace(FaceId),
}

/// The text engine: loaded faces plus the shaping cache. One instance serves layout and rasterization.
pub struct TextEngine {
    faces: HashMap<FaceId, Face>,
    cache: ShapeCache,
}

impl Default for TextEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl TextEngine {
    /// An engine with no faces.
    pub fn new() -> Self {
        Self {
            faces: HashMap::new(),
            cache: ShapeCache::new(SHAPE_CACHE_CAPACITY),
        }
    }

    /// An engine whose shaping cache holds at most `capacity` runs (the default is [`SHAPE_CACHE_CAPACITY`]).
    pub fn with_shape_cache_capacity(capacity: usize) -> Self {
        let mut e = Self::new();
        e.cache = ShapeCache::new(capacity.max(1));
        e
    }
    /// Parses and registers a face under `id`, replacing any face already there. `bytes` is the (decompressed)
    /// TrueType/OpenType file.
    pub fn load_face(&mut self, id: FaceId, bytes: Arc<[u8]>) -> Result<(), TextError> {
        let face = Face::parse(bytes)?;
        self.faces.insert(id, face);
        Ok(())
    }

    /// True if a face is loaded under `id`.
    pub fn has_face(&self, id: FaceId) -> bool {
        self.faces.contains_key(&id)
    }

    pub(crate) fn face(&self, id: FaceId) -> Result<&Face, TextError> {
        self.faces.get(&id).ok_or(TextError::UnknownFace(id))
    }
}
