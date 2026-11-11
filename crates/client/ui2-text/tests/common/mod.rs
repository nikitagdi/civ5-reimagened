//! Shared helpers of the integration tests: the vendored fonts, loaded by path relative to the crate.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;

use ui2_text::{FaceId, TextEngine};

pub const FIRA_400: FaceId = FaceId(0);
pub const FIRA_500: FaceId = FaceId(1);
pub const ALEGREYA: FaceId = FaceId(2);

pub fn fonts_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../third_party/fonts")
}

pub fn read_font(rel: &str) -> Arc<[u8]> {
    let path = fonts_dir().join(rel);
    let bytes =
        std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    Arc::from(bytes)
}

/// An engine with Fira Sans 400/500 and the variable Alegreya loaded under the ids above.
pub fn engine() -> TextEngine {
    let mut e = TextEngine::new();
    e.load_face(FIRA_400, read_font("fira-sans/FiraSans-Regular.ttf"))
        .expect("Fira Sans 400");
    e.load_face(FIRA_500, read_font("fira-sans/FiraSans-Medium.ttf"))
        .expect("Fira Sans 500");
    e.load_face(ALEGREYA, read_font("alegreya/Alegreya[wght].ttf"))
        .expect("Alegreya");
    e
}
