//! Line breaking, line boxes and ellipsis.
//!
//! Rule 6 (`white-space: normal`): greedy breaking at spaces. Widths come from the shaped runs (sums of cluster
//! advances over a byte range, snapped as in rule 3). A word moves to the next line when the line content without
//! its trailing space is wider than the available width. Trailing spaces hang: they are not part of the line's
//! content width (`Line::width`), and Chrome's text rects of a wrapped paragraph do not include them either: all 288
//! measured lines match `width` and none match `width + hanging` (the spike's rule 6 text says the rect includes the
//! space; its data does not). `Line::hanging` still reports the width of the hanging space. A word wider than the line overflows; there is no mid-word breaking.
//! SIMPLIFIED(SIMP-225): lines break only at spaces and line feeds; the line breaking rules of Unicode (hyphens, CJK, soft hyphens) are not applied.
//!
//! Explicit line feeds always break. Leading spaces of a line are dropped (whitespace collapsing is the
//! converter's job, rule 7: the text arrives collapsed). Several runs form one inline flow; a break opportunity
//! exists only at a space, never at a run boundary.
//!
//! Line boxes: with several runs on a line the ascent, descent and gap are the maxima over the runs. (Blink
//! combines inline boxes slightly differently; for one font per line, the case measured here, the two agree.)

use crate::metrics::LineMetrics;
use crate::shape::snap_up;
use crate::{FaceId, RunStyle, ShapedRun, TextEngine, TextError};

/// CSS `line-height`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum LineHeight {
    /// `normal`: ascent + descent + line gap of the font.
    #[default]
    Normal,
    /// A length in px.
    Px(f32),
}

/// Parameters of [`TextEngine::break_lines_with`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BreakOptions {
    /// Available width in px.
    pub max_width: f32,
    /// False for `white-space: nowrap`: only explicit line feeds break.
    pub wrap: bool,
    /// Line height.
    pub line_height: LineHeight,
}

impl BreakOptions {
    /// Wrapping at `max_width` with `line-height: normal`.
    pub fn new(max_width: f32) -> Self {
        Self {
            max_width,
            wrap: true,
            line_height: LineHeight::Normal,
        }
    }
}

/// A slice of one run placed on a line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineSegment {
    /// Index into the runs given to the breaker.
    pub run: usize,
    /// Byte range of the run text.
    pub start: usize,
    /// End of the byte range (exclusive).
    pub end: usize,
    /// Left edge of the segment from the line start, px.
    pub x: f32,
    /// Pen x, inside the run, of the first glyph of the segment: a glyph is drawn at
    /// `line_x + x + (glyph.x - run_x)`.
    pub run_x: f32,
    /// Snapped width of the segment, px.
    pub width: f32,
}

/// One laid-out line.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// The pieces of runs on this line, left to right.
    pub segments: Vec<LineSegment>,
    /// Content width, px, trailing space excluded.
    pub width: f32,
    /// Width of the trailing spaces that hang past the content (0 on the last line and before line feeds).
    pub hanging: f32,
    /// Top of the line box from the top of the text block, px.
    pub top: f32,
    /// Height of the line box, px.
    pub height: f32,
    /// Baseline from the top of the line box, px (rule 5).
    pub baseline: f32,
}

impl Line {
    /// Content width plus the hanging space (the extent the line occupies if the trailing space is kept).
    pub fn width_with_hanging(&self) -> f32 {
        self.width + self.hanging
    }

    /// Byte offset in run `run` where the line's content starts, if the line has a segment of that run.
    pub fn start_in(&self, run: usize) -> Option<usize> {
        self.segments.iter().find(|s| s.run == run).map(|s| s.start)
    }
}

/// A broken paragraph.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Lines {
    /// Lines, top to bottom.
    pub lines: Vec<Line>,
    /// Widest content width, px.
    pub width: f32,
    /// Total height, px.
    pub height: f32,
}

/// Result of [`TextEngine::ellipsize`].
#[derive(Debug, Clone, PartialEq)]
pub struct Ellipsized {
    /// The run to paint: the original, or the kept prefix followed by U+2026.
    pub run: ShapedRun,
    /// True if the text was cut.
    pub truncated: bool,
    /// Bytes of the original text that were kept.
    pub kept: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Part {
    run: usize,
    start: usize,
    end: usize,
}

/// Appends a part, merging it into the previous one when they are adjacent in the same run.
fn push_part(parts: &mut Vec<Part>, p: Part) {
    if let Some(last) = parts.last_mut()
        && last.run == p.run
        && last.end == p.start
    {
        last.end = p.end;
        return;
    }
    parts.push(p);
}

enum Item {
    Word(Vec<Part>),
    Space(Vec<Part>),
    Newline(usize),
}

#[derive(PartialEq, Clone, Copy)]
enum Class {
    Word,
    Space,
}

/// Splits the runs into words, space groups and line feeds.
fn items(runs: &[ShapedRun]) -> Vec<Item> {
    let mut out = Vec::new();
    let mut cur: Vec<Part> = Vec::new();
    let mut cur_class = Class::Word;
    let flush = |out: &mut Vec<Item>, cur: &mut Vec<Part>, class: Class| {
        if !cur.is_empty() {
            let parts = std::mem::take(cur);
            out.push(match class {
                Class::Word => Item::Word(parts),
                Class::Space => Item::Space(parts),
            });
        }
    };
    for (ri, run) in runs.iter().enumerate() {
        for (i, c) in run.text.char_indices() {
            if c == '\n' {
                flush(&mut out, &mut cur, cur_class);
                out.push(Item::Newline(ri));
                continue;
            }
            let class = if c == ' ' { Class::Space } else { Class::Word };
            if class != cur_class {
                flush(&mut out, &mut cur, cur_class);
                cur_class = class;
            }
            push_part(
                &mut cur,
                Part {
                    run: ri,
                    start: i,
                    end: i + c.len_utf8(),
                },
            );
        }
    }
    flush(&mut out, &mut cur, cur_class);
    out
}

/// Prefix sums of the glyph advances by byte index (`p[k]` = unsnapped width of the clusters before byte `k`).
/// Line feeds count as zero width.
fn byte_prefix(run: &ShapedRun) -> Vec<f64> {
    let bytes = run.text.as_bytes();
    let mut at = vec![0.0f64; bytes.len() + 1];
    for g in run.glyphs.iter() {
        let i = g.cluster as usize;
        if i < bytes.len() && bytes[i] != b'\n' {
            at[i] += run.glyph_advance(g);
        }
    }
    let mut p = vec![0.0f64; bytes.len() + 1];
    for k in 0..bytes.len() {
        p[k + 1] = p[k] + at[k];
    }
    p
}

fn parts_width(prefix: &[Vec<f64>], parts: &[Part]) -> f64 {
    parts
        .iter()
        .map(|p| snap_up(prefix[p.run][p.end] - prefix[p.run][p.start]))
        .sum()
}

impl TextEngine {
    /// Breaks the inline flow made of `runs` into lines of at most `width` px (rule 6) with
    /// `line-height: normal`.
    pub fn break_lines(&self, runs: &[ShapedRun], width: f32) -> Lines {
        self.break_lines_with(runs, &BreakOptions::new(width))
    }

    /// [`TextEngine::break_lines`] with explicit options.
    pub fn break_lines_with(&self, runs: &[ShapedRun], options: &BreakOptions) -> Lines {
        let prefix: Vec<Vec<f64>> = runs.iter().map(byte_prefix).collect();
        let avail = if options.wrap {
            f64::from(options.max_width)
        } else {
            f64::INFINITY
        };
        let mut raw: Vec<(Vec<Part>, f64, usize)> = Vec::new(); // content, hanging width, run for metrics
        let mut content: Vec<Part> = Vec::new();
        let mut pending: Vec<Part> = Vec::new();
        let mut last_run = 0usize;

        for item in items(runs) {
            match item {
                Item::Word(parts) => {
                    last_run = parts[0].run;
                    if content.is_empty() {
                        content = parts;
                        pending.clear();
                        continue;
                    }
                    let mut tentative = content.clone();
                    for p in pending.iter().chain(parts.iter()) {
                        push_part(&mut tentative, *p);
                    }
                    if parts_width(&prefix, &tentative) > avail {
                        let hang = parts_width(&prefix, &pending);
                        raw.push((std::mem::take(&mut content), hang, last_run));
                        content = parts;
                    } else {
                        content = tentative;
                    }
                    pending.clear();
                }
                Item::Space(parts) => {
                    // Spaces at the start of a line collapse away.
                    if !content.is_empty() {
                        for p in parts {
                            push_part(&mut pending, p);
                        }
                    }
                }
                Item::Newline(ri) => {
                    raw.push((std::mem::take(&mut content), 0.0, ri));
                    pending.clear();
                    last_run = ri;
                }
            }
        }
        if !content.is_empty() {
            raw.push((content, 0.0, last_run));
        }

        let mut out = Lines::default();
        let mut top = 0.0f32;
        for (parts, hang, metric_run) in raw {
            let mut segments = Vec::with_capacity(parts.len());
            let mut x = 0.0f64;
            let mut lm: Option<LineMetrics> = None;
            for p in &parts {
                let run = &runs[p.run];
                let w = snap_up(prefix[p.run][p.end] - prefix[p.run][p.start]);
                let run_x = run
                    .glyphs
                    .iter()
                    .filter(|g| (g.cluster as usize) >= p.start && (g.cluster as usize) < p.end)
                    .map(|g| g.x)
                    .fold(f32::INFINITY, f32::min);
                segments.push(LineSegment {
                    run: p.run,
                    start: p.start,
                    end: p.end,
                    x: x as f32,
                    run_x: if run_x.is_finite() { run_x } else { 0.0 },
                    width: w as f32,
                });
                x += w;
                lm = Some(self.max_metrics(lm, &run.style));
            }
            let width = x as f32;
            let lm = lm
                .or_else(|| {
                    runs.get(metric_run)
                        .map(|r| self.max_metrics(None, &r.style))
                })
                .unwrap_or(LineMetrics {
                    ascent: 0.0,
                    descent: 0.0,
                    line_gap: 0.0,
                });
            let height = match options.line_height {
                LineHeight::Normal => lm.normal_line_height(),
                LineHeight::Px(h) => h,
            };
            out.width = out.width.max(width);
            out.lines.push(Line {
                segments,
                width,
                hanging: hang as f32,
                top,
                height,
                baseline: lm.baseline_in(height),
            });
            top += height;
        }
        out.height = top;
        out
    }

    /// Folds the metrics of `style` into `acc` by taking the maximum of each value (unknown faces count as zero).
    fn max_metrics(&self, acc: Option<LineMetrics>, style: &RunStyle) -> LineMetrics {
        let m = self
            .metrics(style.face, style.size, style.wght)
            .unwrap_or(LineMetrics {
                ascent: 0.0,
                descent: 0.0,
                line_gap: 0.0,
            });
        match acc {
            None => m,
            Some(a) => LineMetrics {
                ascent: a.ascent.max(m.ascent),
                descent: a.descent.max(m.descent),
                line_gap: a.line_gap.max(m.line_gap),
            },
        }
    }

    /// `text-overflow: ellipsis` for a single line: if `run` is wider than `max_width`, keeps the longest prefix
    /// (cut at a cluster start) whose width plus the width of U+2026 fits, and appends the ellipsis.
    ///
    /// Where exactly Chrome cuts was not measured; the longest fitting prefix is the CSS definition.
    pub fn ellipsize(&mut self, run: &ShapedRun, max_width: f32) -> Result<Ellipsized, TextError> {
        if run.width <= max_width {
            return Ok(Ellipsized {
                run: run.clone(),
                truncated: false,
                kept: run.text.len(),
            });
        }
        let ellipsis = self.shape(&run.style, "\u{2026}")?;
        let prefix = byte_prefix(run);
        let starts = run.cluster_starts();
        for &b in starts.iter().rev() {
            if b == 0 || b >= run.text.len() {
                continue;
            }
            if snap_up(prefix[b]) as f32 + ellipsis.width > max_width {
                continue;
            }
            // The ellipsis can kern against the last letter, so confirm with a real shaping.
            let text = format!("{}\u{2026}", &run.text[..b]);
            let shaped = self.shape(&run.style, &text)?;
            if shaped.width <= max_width {
                return Ok(Ellipsized {
                    run: shaped,
                    truncated: true,
                    kept: b,
                });
            }
        }
        Ok(Ellipsized {
            run: ellipsis,
            truncated: true,
            kept: 0,
        })
    }
}

/// What the layout needs from the text stack. `TextEngine` is the implementation; the trait lets layout code be
/// tested against a stub.
pub trait TextMeasure {
    /// Shapes and measures a run (rules 1 to 3).
    fn shape(&mut self, style: &RunStyle, text: &str) -> Result<ShapedRun, TextError>;
    /// Line metrics (rule 4).
    fn metrics(&self, face: FaceId, size: f32, wght: f32) -> Result<LineMetrics, TextError>;
    /// Breaks an inline flow into lines (rule 6).
    fn break_lines(&self, runs: &[ShapedRun], options: &BreakOptions) -> Lines;
    /// Single-line ellipsis truncation.
    fn ellipsize(&mut self, run: &ShapedRun, max_width: f32) -> Result<Ellipsized, TextError>;
}

impl TextMeasure for TextEngine {
    fn shape(&mut self, style: &RunStyle, text: &str) -> Result<ShapedRun, TextError> {
        TextEngine::shape(self, style, text)
    }

    fn metrics(&self, face: FaceId, size: f32, wght: f32) -> Result<LineMetrics, TextError> {
        TextEngine::metrics(self, face, size, wght)
    }

    fn break_lines(&self, runs: &[ShapedRun], options: &BreakOptions) -> Lines {
        TextEngine::break_lines_with(self, runs, options)
    }

    fn ellipsize(&mut self, run: &ShapedRun, max_width: f32) -> Result<Ellipsized, TextError> {
        TextEngine::ellipsize(self, run, max_width)
    }
}
