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

use std::fmt;

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
