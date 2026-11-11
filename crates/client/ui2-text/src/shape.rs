//! Shaping and single-line measurement.
//!
//! Rule 1: harfrust shapes the run text with `guess_segment_properties` and its default features (`kern`,
//! `liga`, `clig`, `calt`, ...). CSS adds `tnum`/`pnum`/`lnum`/`onum` from `font-variant-numeric`; a
//! non-zero `letter-spacing` switches `liga` and `clig` off (Blink). Variable faces get `wght` from the CSS weight.
//!
//! Rule 2: U+00A0 is shaped as U+0020 (and a line feed as a space); the text of the run keeps the original
//! characters so that line breaking still sees a no-break space.
//!
//! Rule 3: the width of a run is `sum(x_advance) * size / upem + letter_spacing * chars` (the spacing follows
//! every character, the last one included), snapped up to 1/64 px: `ceil(w * 64) / 64`. Glyph advances are
//! never rounded to device pixels and nothing here depends on the device pixel ratio.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use harfrust::{Feature, ShapeOptions, Tag, UnicodeBuffer};

use crate::{FaceId, TextEngine, TextError};

/// Entries of the shaping cache before the least recently used quarter is dropped.
pub const SHAPE_CACHE_CAPACITY: usize = 8192;

/// `font-variant-numeric` figure forms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum NumericFigure {
    /// The font's default figures.
    #[default]
    Normal,
    /// `lining-nums` (`lnum`).
    Lining,
    /// `oldstyle-nums` (`onum`).
    Oldstyle,
}

/// `font-variant-numeric` spacing forms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum NumericSpacing {
    /// The font's default spacing.
    #[default]
    Normal,
    /// `tabular-nums` (`tnum`).
    Tabular,
    /// `proportional-nums` (`pnum`).
    Proportional,
}

/// The `font-variant-numeric` subset the profile allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct NumericVariant {
    /// Lining or oldstyle figures.
    pub figure: NumericFigure,
    /// Tabular or proportional spacing.
    pub spacing: NumericSpacing,
}

impl NumericVariant {
    /// Parses a computed `font-variant-numeric` value such as `"lining-nums tabular-nums"`; `normal` and unknown
    /// words leave the default.
    pub fn from_css(value: &str) -> Self {
        let mut v = Self::default();
        for word in value.split_whitespace() {
            match word {
                "lining-nums" => v.figure = NumericFigure::Lining,
                "oldstyle-nums" => v.figure = NumericFigure::Oldstyle,
                "tabular-nums" => v.spacing = NumericSpacing::Tabular,
                "proportional-nums" => v.spacing = NumericSpacing::Proportional,
                _ => {}
            }
        }
        v
    }
}

/// Everything besides the text that decides how a run is shaped and measured.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RunStyle {
    /// Face to shape with.
    pub face: FaceId,
    /// Font size in CSS px (layout units, before the device scale).
    pub size: f32,
    /// CSS `font-weight`; sets the `wght` axis of variable faces, ignored by static ones.
    pub wght: f32,
    /// `letter-spacing` in px (0 for `normal`).
    pub letter_spacing: f32,
    /// `font-variant-numeric`.
    pub numeric: NumericVariant,
}

impl RunStyle {
    /// A style with weight 400, no letter spacing and default numerics.
    pub fn new(face: FaceId, size: f32) -> Self {
        Self {
            face,
            size,
            wght: 400.0,
            letter_spacing: 0.0,
            numeric: NumericVariant::default(),
        }
    }

    /// The same style at another weight.
    pub fn with_wght(mut self, wght: f32) -> Self {
        self.wght = wght;
        self
    }

    /// The same style with letter spacing in px.
    pub fn with_letter_spacing(mut self, px: f32) -> Self {
        self.letter_spacing = px;
        self
    }

    /// The same style with numeric variants.
    pub fn with_numeric(mut self, numeric: NumericVariant) -> Self {
        self.numeric = numeric;
        self
    }

    fn hash_into<H: Hasher>(&self, h: &mut H) {
        self.face.hash(h);
        self.size.to_bits().hash(h);
        self.wght.to_bits().hash(h);
        self.letter_spacing.to_bits().hash(h);
        self.numeric.hash(h);
    }
}

/// One glyph of a shaped run. Positions are in CSS px relative to the run origin and the baseline (y down).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapedGlyph {
    /// Glyph id in the face.
    pub glyph_id: u32,
    /// Byte index in the run text of the first character of the glyph's cluster.
    pub cluster: u32,
    /// Pen x of the glyph, offsets included, letter spacing of earlier characters included.
    pub x: f32,
    /// Vertical offset from the baseline (y down).
    pub y: f32,
    /// Pen advance in px: font advance plus the letter spacing of the characters of this cluster.
    pub advance: f32,
    /// Font advance in font units (the exact input of rule 3).
    pub units: i32,
    /// Characters of the cluster charged to this glyph for letter spacing (0 for later glyphs of a cluster).
    pub chars: u16,
}

/// A shaped run: glyphs, positions and the width of rule 3. Cloning is cheap (shared glyph and text buffers).
#[derive(Debug, Clone, PartialEq)]
pub struct ShapedRun {
    /// The text as given (no-break spaces kept).
    pub text: Arc<str>,
    /// Glyphs in visual order.
    pub glyphs: Arc<[ShapedGlyph]>,
    /// The style the run was shaped with.
    pub style: RunStyle,
    /// `size / units_per_em`.
    pub scale: f32,
    /// Run width in px, snapped up to 1/64 (rule 3).
    pub width: f32,
}

/// Snaps a width up to the next 1/64 px (Blink's `LayoutUnit::FromFloatCeil`). The tiny bias keeps sums that
/// are exact multiples of 1/64 from creeping up through floating point noise.
pub(crate) fn snap_up(w: f64) -> f64 {
    (w * 64.0 - 1e-6).ceil() / 64.0
}

impl ShapedRun {
    /// Advance of a glyph in px as an f64 sum term: `units * scale` plus the letter spacing it carries.
    pub(crate) fn glyph_advance(&self, g: &ShapedGlyph) -> f64 {
        f64::from(g.units) * f64::from(self.scale)
            + f64::from(self.style.letter_spacing) * f64::from(g.chars)
    }

    /// Width of the byte range `[start, end)` of the text: the glyphs whose cluster starts inside it, summed and
    /// snapped (rule 3). Use cluster boundaries as the range ends.
    pub fn range_width(&self, start: usize, end: usize) -> f32 {
        let sum: f64 = self
            .glyphs
            .iter()
            .filter(|g| (g.cluster as usize) >= start && (g.cluster as usize) < end)
            .map(|g| self.glyph_advance(g))
            .sum();
        snap_up(sum) as f32
    }

    /// Byte offsets where a cluster starts, ascending, without duplicates (the points a text can be cut at).
    pub fn cluster_starts(&self) -> Vec<usize> {
        let mut v: Vec<usize> = self.glyphs.iter().map(|g| g.cluster as usize).collect();
        v.sort_unstable();
        v.dedup();
        v
    }
}

/// OpenType features for a style (rule 1). Kerning and the default ligature set need no entry.
pub(crate) fn features_for(style: &RunStyle) -> Vec<Feature> {
    let on = |tag: &[u8; 4], value: u32| Feature::new(Tag::new(tag), value, ..);
    let mut v = Vec::new();
    match style.numeric.spacing {
        NumericSpacing::Tabular => v.push(on(b"tnum", 1)),
        NumericSpacing::Proportional => v.push(on(b"pnum", 1)),
        NumericSpacing::Normal => {}
    }
    match style.numeric.figure {
        NumericFigure::Lining => v.push(on(b"lnum", 1)),
        NumericFigure::Oldstyle => v.push(on(b"onum", 1)),
        NumericFigure::Normal => {}
    }
    if style.letter_spacing != 0.0 {
        v.push(on(b"liga", 0));
        v.push(on(b"clig", 0));
    }
    v
}

/// Text as harfrust sees it (rule 2), with a map from its byte offsets back to the original text when the two
/// differ in length. `None` means the offsets are identical.
fn shaping_text(text: &str) -> (String, Option<Vec<u32>>) {
    let mut out = String::with_capacity(text.len());
    let mut map: Option<Vec<u32>> = None;
    for (i, c) in text.char_indices() {
        let mapped = match c {
            '\u{a0}' | '\n' => ' ',
            c => c,
        };
        if mapped.len_utf8() != c.len_utf8() && map.is_none() {
            // Everything so far maps one to one.
            map = Some((0..out.len() as u32).collect());
        }
        if let Some(m) = map.as_mut() {
            m.extend(std::iter::repeat_n(i as u32, mapped.len_utf8()));
        }
        out.push(mapped);
    }
    if let Some(m) = map.as_mut() {
        m.push(text.len() as u32);
    }
    (out, map)
}

struct CacheEntry {
    run: ShapedRun,
    stamp: u64,
}

/// LRU cache of shaped runs, keyed by a hash of the style and the text (the entry keeps both for the check).
pub(crate) struct ShapeCache {
    map: HashMap<u64, CacheEntry>,
    clock: u64,
    capacity: usize,
    pub hits: u64,
    pub misses: u64,
}

impl ShapeCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            map: HashMap::new(),
            clock: 0,
            capacity,
            hits: 0,
            misses: 0,
        }
    }

    fn key(style: &RunStyle, text: &str) -> u64 {
        let mut h = std::hash::DefaultHasher::new();
        style.hash_into(&mut h);
        text.hash(&mut h);
        h.finish()
    }

    fn get(&mut self, key: u64, style: &RunStyle, text: &str) -> Option<ShapedRun> {
        self.clock += 1;
        let clock = self.clock;
        match self.map.get_mut(&key) {
            Some(e) if e.run.style == *style && &*e.run.text == text => {
                e.stamp = clock;
                self.hits += 1;
                Some(e.run.clone())
            }
            _ => None,
        }
    }

    fn insert(&mut self, key: u64, run: ShapedRun) {
        self.misses += 1;
        self.clock += 1;
        if self.map.len() >= self.capacity {
            let mut stamps: Vec<u64> = self.map.values().map(|e| e.stamp).collect();
            stamps.sort_unstable();
            let cut = stamps[stamps.len() / 4];
            self.map.retain(|_, e| e.stamp > cut);
        }
        self.map.insert(
            key,
            CacheEntry {
                run,
                stamp: self.clock,
            },
        );
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }
}

/// Counters of the shaping cache, for tests and the perf overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShapeCacheStats {
    /// Runs currently cached.
    pub entries: usize,
    /// Lookups answered from the cache.
    pub hits: u64,
    /// Lookups that shaped the text.
    pub misses: u64,
}

impl TextEngine {
    /// Shapes `text` in `style` and measures it (rules 1 to 3). Results are cached by (style, text).
    pub fn shape(&mut self, style: &RunStyle, text: &str) -> Result<ShapedRun, TextError> {
        // Fail on an unknown face even if the answer would be cached under the same key.
        self.face(style.face)?;
        let key = ShapeCache::key(style, text);
        if let Some(run) = self.cache.get(key, style, text) {
            return Ok(run);
        }
        let run = self.shape_uncached(style, text)?;
        self.cache.insert(key, run.clone());
        Ok(run)
    }

    /// Width of `text` in `style` in px (rule 3), without keeping the glyphs around.
    pub fn measure(&mut self, style: &RunStyle, text: &str) -> Result<f32, TextError> {
        Ok(self.shape(style, text)?.width)
    }

    /// Counters of the shaping cache.
    pub fn shape_cache_stats(&self) -> ShapeCacheStats {
        ShapeCacheStats {
            entries: self.cache.len(),
            hits: self.cache.hits,
            misses: self.cache.misses,
        }
    }

    fn shape_uncached(&mut self, style: &RunStyle, text: &str) -> Result<ShapedRun, TextError> {
        let face = self
            .faces
            .get_mut(&style.face)
            .ok_or(TextError::UnknownFace(style.face))?;
        face.ensure_instance(style.wght);
        let face = &*face;
        let upem = face.upem;
        let scale = style.size / upem;
        let (shaping, map) = shaping_text(text);
        let feats = features_for(style);
        let font = face
            .font()
            .ok_or_else(|| TextError::Font("font bytes no longer parse".into()))?;

        // Shape. The instance borrows `face`, so it is fetched after the font and used right away.
        let mut buf = UnicodeBuffer::new();
        buf.push_str(&shaping);
        buf.guess_segment_properties();
        let (infos, positions) = {
            let instance = face.instance(style.wght);
            let shaper = face.data.shaper(&font).instance(instance).build();
            let out = shaper.shape(buf, ShapeOptions::new().features(&feats));
            (out.glyph_infos().to_vec(), out.glyph_positions().to_vec())
        };

        // Characters of every cluster, for the letter spacing: a cluster runs to the next cluster start.
        let orig =
            |c: u32| -> usize { map.as_ref().map_or(c as usize, |m| m[c as usize] as usize) };
        let mut starts: Vec<usize> = infos.iter().map(|i| orig(i.cluster)).collect();
        starts.sort_unstable();
        starts.dedup();
        let chars_of = |cluster: usize| -> u16 {
            let next = match starts.binary_search(&cluster) {
                Ok(i) => starts.get(i + 1).copied().unwrap_or(text.len()),
                Err(_) => text.len(),
            };
            text.get(cluster..next).map_or(1, |s| s.chars().count()) as u16
        };

        let ls = f64::from(style.letter_spacing);
        let mut pen = 0.0f64;
        let mut seen_cluster: Option<usize> = None;
        let mut glyphs = Vec::with_capacity(infos.len());
        for (info, pos) in infos.iter().zip(positions.iter()) {
            let cluster = orig(info.cluster);
            let chars = if seen_cluster == Some(cluster) {
                0
            } else {
                chars_of(cluster)
            };
            seen_cluster = Some(cluster);
            let s = f64::from(scale);
            let advance = f64::from(pos.x_advance) * s + ls * f64::from(chars);
            glyphs.push(ShapedGlyph {
                glyph_id: info.glyph_id,
                cluster: cluster as u32,
                x: (pen + f64::from(pos.x_offset) * s) as f32,
                y: (-f64::from(pos.y_offset) * s) as f32,
                advance: advance as f32,
                units: pos.x_advance,
                chars,
            });
            pen += advance;
        }
        let total: f64 = glyphs
            .iter()
            .map(|g| f64::from(g.units) * f64::from(scale) + ls * f64::from(g.chars))
            .sum();
        Ok(ShapedRun {
            text: Arc::from(text),
            glyphs: glyphs.into(),
            style: *style,
            scale,
            width: snap_up(total) as f32,
        })
    }
}
