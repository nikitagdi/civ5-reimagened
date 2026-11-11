//! Font faces: parsed once, kept as bytes plus the cached shaper data.

#![allow(dead_code)] // the shaping and raster paths that use the rest follow in the next commits

use std::collections::HashMap;
use std::sync::Arc;

use harfrust::{ShaperData, ShaperInstance, Tag, Variation};
use skrifa::raw::TableProvider;
use skrifa::{FontRef, MetadataProvider};

use crate::TextError;

/// OS/2 `fsSelection` bit 7: take line metrics from the typographic fields.
const USE_TYPO_METRICS: u16 = 1 << 7;

/// Ascent, descent and line gap in font units (descent positive), as Skia/Blink pick them:
/// OS/2 typographic metrics when `USE_TYPO_METRICS` is set, otherwise `hhea`. Win metrics are never used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RawMetrics {
    pub ascent: i32,
    pub descent: i32,
    pub line_gap: i32,
}

/// A loaded face. The `ShaperData` is built once; shaper instances are cached per `wght` value.
pub(crate) struct Face {
    pub bytes: Arc<[u8]>,
    pub data: ShaperData,
    instances: HashMap<u32, ShaperInstance>,
    pub upem: f32,
    pub has_wght: bool,
    pub raw: RawMetrics,
}

fn wght_tag() -> Tag {
    Tag::new(b"wght")
}

impl Face {
    /// Parses `bytes` (TrueType or OpenType) and reads the unit and metric tables.
    pub fn parse(bytes: Arc<[u8]>) -> Result<Self, TextError> {
        let font = FontRef::new(&bytes).map_err(|e| TextError::Font(e.to_string()))?;
        let missing = |t: &str| TextError::Font(format!("missing {t} table"));
        let head = font.head().map_err(|_| missing("head"))?;
        let upem = head.units_per_em();
        if upem == 0 {
            return Err(TextError::Font("units per em is zero".into()));
        }
        let hhea = font.hhea().map_err(|_| missing("hhea"))?;
        let raw = match font.os2() {
            Ok(os2) if os2.fs_selection().bits() & USE_TYPO_METRICS != 0 => RawMetrics {
                ascent: i32::from(os2.s_typo_ascender()),
                descent: -i32::from(os2.s_typo_descender()),
                line_gap: i32::from(os2.s_typo_line_gap()),
            },
            _ => RawMetrics {
                ascent: i32::from(hhea.ascender().to_i16()),
                descent: -i32::from(hhea.descender().to_i16()),
                line_gap: i32::from(hhea.line_gap().to_i16()),
            },
        };
        let has_wght = font.axes().iter().any(|a| a.tag() == wght_tag());
        let data = ShaperData::new(&font);
        Ok(Self {
            bytes,
            data,
            instances: HashMap::new(),
            upem: f32::from(upem),
            has_wght,
            raw,
        })
    }

    /// The parsed font. Parsing only reads the table directory, so this is cheap.
    pub fn font(&self) -> Option<FontRef<'_>> {
        FontRef::new(&self.bytes).ok()
    }

    /// The `wght` value a face actually uses for a requested CSS weight: static faces ignore it.
    pub fn effective_wght(&self, wght: f32) -> Option<f32> {
        self.has_wght.then_some(wght)
    }

    /// Shaper instance for a variable face at `wght`, created on first use. `None` for static faces.
    pub fn instance(&mut self, wght: f32) -> Option<&ShaperInstance> {
        if !self.has_wght {
            return None;
        }
        let key = wght.to_bits();
        if !self.instances.contains_key(&key) {
            let font = FontRef::new(&self.bytes).ok()?;
            let inst = ShaperInstance::from_variations(
                &font,
                [Variation {
                    tag: wght_tag(),
                    value: wght,
                }],
            );
            self.instances.insert(key, inst);
        }
        self.instances.get(&key)
    }
}
