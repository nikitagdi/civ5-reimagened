//! Chrome comparison: widths of single-line runs, line metrics and the 30 wrapped paragraphs.
//!
//! `tests/data/chrome-text.json` holds numbers dumped from headless Chrome 148 (see `tests/data/README.md`).

mod common;

use common::*;
use serde::Deserialize;
use ui2_text::{BreakOptions, FaceId, LineHeight, NumericVariant, RunStyle, TextEngine};

#[derive(Deserialize)]
struct Fixture {
    runs: Vec<Run>,
    paragraphs: Vec<Paragraph>,
}

#[derive(Deserialize)]
struct Run {
    s: String,
    family: String,
    weight: u32,
    size: f32,
    numeric: String,
    letter_spacing: f32,
    width: f64,
    line_height: Option<serde_json::Value>,
    box_h: Option<f32>,
    baseline: Option<f32>,
    content_h: Option<f32>,
}

#[derive(Deserialize)]
struct Paragraph {
    s: String,
    family: String,
    weight: u32,
    size: f32,
    numeric: String,
    max_width: f32,
    line_height: serde_json::Value,
    line_starts: Vec<usize>,
    line_widths: Vec<f64>,
}

fn fixture() -> Fixture {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/chrome-text.json");
    let text = std::fs::read_to_string(path).expect("fixture");
    serde_json::from_str(&text).expect("fixture json")
}

fn face_for(family: &str, weight: u32) -> FaceId {
    match (family, weight) {
        ("Fira Sans", 500) => FIRA_500,
        ("Fira Sans", _) => FIRA_400,
        ("Alegreya", _) => ALEGREYA,
        other => panic!("unexpected family {other:?}"),
    }
}

fn style(family: &str, weight: u32, size: f32, numeric: &str, ls: f32) -> RunStyle {
    RunStyle::new(face_for(family, weight), size)
        .with_wght(weight as f32)
        .with_numeric(NumericVariant::from_css(numeric))
        .with_letter_spacing(ls)
}

fn css_line_height(v: &serde_json::Value) -> LineHeight {
    match v {
        serde_json::Value::String(_) => LineHeight::Normal,
        n => LineHeight::Px(n.as_f64().expect("number") as f32),
    }
}

#[test]
fn single_line_widths_match_chrome() {
    let fx = fixture();
    let mut e: TextEngine = engine();
    let (mut checked, mut skipped, mut worst) = (0, 0, 0.0f64);
    let mut worst_desc = String::new();
    for r in &fx.runs {
        let st = style(&r.family, r.weight, r.size, &r.numeric, r.letter_spacing);
        let run = e.shape(&st, &r.s).unwrap();
        if run.glyphs.iter().any(|g| g.glyph_id == 0) {
            skipped += 1; // a character the face lacks: Chrome used a fallback font
            continue;
        }
        let err = (f64::from(run.width) - r.width).abs();
        if err > worst {
            worst = err;
            worst_desc = format!(
                "{:?} {} {} {}px: ours {} chrome {}",
                r.s, r.family, r.weight, r.size, run.width, r.width
            );
        }
        assert!(
            err <= 0.05,
            "{:?} {} {} {}px: ours {} chrome {}",
            r.s,
            r.family,
            r.weight,
            r.size,
            run.width,
            r.width
        );
        checked += 1;
    }
    println!("checked {checked} runs, skipped {skipped}; worst error {worst:.5} px ({worst_desc})");
    assert!(checked >= 150, "only {checked} runs checked");
    assert!(skipped <= 10, "{skipped} runs skipped");
}

#[test]
fn line_metrics_match_chrome() {
    let fx = fixture();
    let e = engine();
    let mut checked = 0;
    for r in fx.runs.iter().filter(|r| r.box_h.is_some()) {
        let face = face_for(&r.family, r.weight);
        let m = e.metrics(face, r.size, r.weight as f32).unwrap();
        let lh = match css_line_height(r.line_height.as_ref().expect("lh")) {
            LineHeight::Normal => m.normal_line_height(),
            LineHeight::Px(h) => h,
        };
        let what = format!("{} {} {}px lh {lh}", r.family, r.weight, r.size);
        assert_eq!(
            m.content_height(),
            r.content_h.unwrap(),
            "content area, {what}"
        );
        assert_eq!(m.baseline_in(lh), r.baseline.unwrap(), "baseline, {what}");
        // Chrome's probe grows a line box that is shorter than the content area; skip the box height then.
        if lh >= m.content_height() {
            assert_eq!(lh, r.box_h.unwrap(), "line box, {what}");
        }
        checked += 1;
    }
    assert!(checked >= 300, "{checked}");
}

#[test]
fn wrapped_paragraphs_match_chrome() {
    let fx = fixture();
    assert_eq!(fx.paragraphs.len(), 30);
    let mut e = engine();
    let mut worst = 0.0f64;
    let mut lines_total = 0;
    for p in &fx.paragraphs {
        let st = style(&p.family, p.weight, p.size, &p.numeric, 0.0);
        let run = e.shape(&st, &p.s).unwrap();
        let opts = BreakOptions {
            max_width: p.max_width,
            wrap: true,
            line_height: css_line_height(&p.line_height),
        };
        let lines = e.break_lines_with(std::slice::from_ref(&run), &opts);
        let what = format!("{} {} {}px in {}", p.family, p.weight, p.size, p.max_width);
        let starts: Vec<usize> = lines.lines.iter().map(|l| l.start_in(0).unwrap()).collect();
        assert_eq!(starts, p.line_starts, "line breaks, {what}");
        assert_eq!(lines.lines.len(), p.line_widths.len(), "{what}");
        for (l, cw) in lines.lines.iter().zip(&p.line_widths) {
            let err = (f64::from(l.width) - cw).abs();
            worst = worst.max(err);
            assert!(
                err <= 0.05,
                "line width, {what}: ours {} chrome {cw}",
                l.width
            );
        }
        lines_total += lines.lines.len();
    }
    println!("30 paragraphs, {lines_total} lines, worst line width error {worst:.5} px");
}
