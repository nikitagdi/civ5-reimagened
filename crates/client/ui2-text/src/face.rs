//! Font faces: parsed once, kept as bytes plus the cached shaper data.

use std::collections::HashMap;
use std::sync::Arc;

use harfrust::{
    Direction, Feature, Script, ShapePlan, ShapePlanKey, ShaperData, ShaperInstance, Tag, Variation,
};
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

/// Shape plans kept per face (one per weight instance, direction, script and feature set in use).
const MAX_PLANS: usize = 64;

/// A loaded face. The `ShaperData` is built once; shaper instances are cached per `wght` value, shape
/// plans per instance, direction, script and features (building a plan costs more than shaping a short run).
pub(crate) struct Face {
    pub bytes: Arc<[u8]>,
    pub data: ShaperData,
    instances: HashMap<u32, ShaperInstance>,
    pub plans: Vec<ShapePlan>,
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
            plans: Vec::new(),
            upem: f32::from(upem),
            has_wght,
            raw,
        })
    }

    /// The parsed font. Parsing only reads the table directory, so this is cheap.
    pub fn font(&self) -> Option<FontRef<'_>> {
        FontRef::new(&self.bytes).ok()
    }

    /// Creates the shaper instance of a variable face at `wght` if it does not exist yet.
    pub fn ensure_instance(&mut self, wght: f32) {
        if !self.has_wght {
            return;
        }
        let key = wght.to_bits();
        if self.instances.contains_key(&key) {
            return;
        }
        if let Ok(font) = FontRef::new(&self.bytes) {
            let inst = ShaperInstance::from_variations(
                &font,
                [Variation {
                    tag: wght_tag(),
                    value: wght,
                }],
            );
            self.instances.insert(key, inst);
        }
    }

    /// The shaper instance made by [`Face::ensure_instance`]; `None` for static faces.
    pub fn instance(&self, wght: f32) -> Option<&ShaperInstance> {
        self.instances.get(&wght.to_bits())
    }

    /// The index in `plans` of the shape plan for a weight instance, a direction, a script and features,
    /// made on first use (`None` when the font does not parse or the cache is full: the caller shapes
    /// without a plan and harfrust makes the same plan for that one call). Call
    /// [`Face::ensure_instance`] first. The plan is the one harfrust would build for a buffer of these
    /// properties without a language, which is how runs are shaped here.
    pub fn plan(
        &mut self,
        wght: f32,
        direction: Direction,
        script: Option<Script>,
        features: &[Feature],
    ) -> Option<usize> {
        let instance = self.instances.get(&wght.to_bits());
        let key = ShapePlanKey::new(script, direction)
            .instance(instance)
            .features(features);
        if let Some(i) = self.plans.iter().position(|p| key.matches(p)) {
            return Some(i);
        }
        if self.plans.len() >= MAX_PLANS {
            return None;
        }
        let font = FontRef::new(&self.bytes).ok()?;
        let plan = {
            let shaper = self.data.shaper(&font).instance(instance).build();
            ShapePlan::new(&shaper, direction, script, None, features)
        };
        self.plans.push(plan);
        Some(self.plans.len() - 1)
    }
}
