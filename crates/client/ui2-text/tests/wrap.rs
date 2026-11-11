//! Rule 6 and the line boxes: greedy breaking, hanging spaces, line feeds, runs, ellipsis.

mod common;

use common::*;
use ui2_text::{BreakOptions, LineHeight, RunStyle, TextEngine, TextMeasure};

fn st() -> RunStyle {
    RunStyle::new(FIRA_400, 14.0)
}

fn w(e: &mut TextEngine, s: &str) -> f32 {
    e.measure(&st(), s).unwrap()
}

/// The text of a line: its segments cut out of the run.
fn line_text(text: &str, l: &ui2_text::Line) -> String {
    l.segments.iter().map(|s| &text[s.start..s.end]).collect()
}

fn texts(text: &str, lines: &ui2_text::Lines) -> Vec<String> {
    lines.lines.iter().map(|l| line_text(text, l)).collect()
}

#[test]
fn breaks_greedily_at_spaces() {
    let mut e = engine();
    let text = "aaa aaa aaa aaa";
    let run = e.shape(&st(), text).unwrap();
    let two = w(&mut e, "aaa aaa");
    let lines = e.break_lines(std::slice::from_ref(&run), two);
    assert_eq!(texts(text, &lines), ["aaa aaa", "aaa aaa"]);
    // A hair narrower and the second word no longer fits.
    let lines = e.break_lines(std::slice::from_ref(&run), two - 0.02);
    assert_eq!(texts(text, &lines), ["aaa", "aaa", "aaa", "aaa"]);
    // Wide enough for everything.
    let lines = e.break_lines(std::slice::from_ref(&run), 1000.0);
    assert_eq!(texts(text, &lines), [text]);
}

#[test]
fn trailing_spaces_hang_and_do_not_count() {
    let mut e = engine();
    let text = "aaa aaa aaa";
    let run = e.shape(&st(), text).unwrap();
    // Wide enough for "aaa" but not for two: the space after the first "aaa" does not need to fit.
    let width = w(&mut e, "aaa");
    let lines = e.break_lines(std::slice::from_ref(&run), width);
    assert_eq!(texts(text, &lines), ["aaa", "aaa", "aaa"]);
    let sp = w(&mut e, " ");
    assert!((lines.lines[0].hanging - sp).abs() <= 1.0 / 32.0);
    assert_eq!(lines.lines[0].width, width);
    assert_eq!(
        lines.lines[2].hanging, 0.0,
        "the last line has no hanging space"
    );
    assert_eq!(lines.width, width);
}

#[test]
fn explicit_line_feeds_break() {
    let mut e = engine();
    let text = "aa\nbb\n\ncc\n";
    let run = e.shape(&st(), text).unwrap();
    let lines = e.break_lines(std::slice::from_ref(&run), 1000.0);
    assert_eq!(
        texts(text, &lines),
        ["aa", "bb", "", "cc"],
        "no extra line after the final line feed"
    );
    assert_eq!(lines.lines[0].width, w(&mut e, "aa"));
    assert_eq!(lines.lines[2].width, 0.0);
    assert_eq!(
        lines.lines[2].height, lines.lines[0].height,
        "empty lines keep the line height"
    );
    // nowrap still honours line feeds.
    let opts = BreakOptions {
        max_width: 1.0,
        wrap: false,
        line_height: LineHeight::Normal,
    };
    let lines = e.break_lines_with(std::slice::from_ref(&run), &opts);
    assert_eq!(lines.lines.len(), 4);
}

#[test]
fn long_word_overflows_and_nowrap_keeps_one_line() {
    let mut e = engine();
    let text = "short extraordinarily short";
    let run = e.shape(&st(), text).unwrap();
    let lines = e.break_lines(std::slice::from_ref(&run), 20.0);
    assert_eq!(texts(text, &lines), ["short", "extraordinarily", "short"]);
    assert!(lines.lines[1].width > 20.0);
    let opts = BreakOptions {
        max_width: 20.0,
        wrap: false,
        line_height: LineHeight::Normal,
    };
    let lines = e.break_lines_with(std::slice::from_ref(&run), &opts);
    assert_eq!(texts(text, &lines), [text]);
    assert_eq!(lines.lines[0].width, run.width);
}

#[test]
fn leading_spaces_are_dropped_and_nbsp_does_not_break() {
    let mut e = engine();
    let text = "  aaa";
    let run = e.shape(&st(), text).unwrap();
    let lines = e.break_lines(std::slice::from_ref(&run), 1000.0);
    assert_eq!(texts(text, &lines), ["aaa"]);

    let text = "aaa\u{a0}bbb ccc";
    let run = e.shape(&st(), text).unwrap();
    let narrow = w(&mut e, "aaa bbb") - 0.5; // too narrow for "aaa bbb" only if it could break at the NBSP
    let lines = e.break_lines(std::slice::from_ref(&run), narrow);
    assert_eq!(texts(text, &lines), ["aaa\u{a0}bbb", "ccc"]);
}

#[test]
fn several_runs_form_one_flow() {
    let mut e = engine();
    let a = e.shape(&st(), "Hel").unwrap();
    let b = e.shape(&st().with_wght(500.0), "lo wor").unwrap();
    let c = e.shape(&st(), "ld again").unwrap();
    let runs = [a.clone(), b.clone(), c.clone()];
    // "Hello" and "world" are each split over run boundaries and must stay unbroken.
    let wide = a.width + b.width + c.width;
    let lines = e.break_lines(&runs, wide + 10.0);
    assert_eq!(lines.lines.len(), 1);
    let segs = &lines.lines[0].segments;
    assert_eq!(segs.len(), 3);
    assert_eq!(segs[0].x, 0.0);
    assert!((segs[1].x - a.width).abs() < 1e-3);
    assert!((segs[2].x - (a.width + b.width)).abs() < 1e-3);
    // Narrow: breaks only at the spaces, never inside "Hello" or "world".
    let lines = e.break_lines(&runs, 40.0);
    let all: Vec<String> = lines
        .lines
        .iter()
        .map(|l| {
            l.segments
                .iter()
                .map(|s| &runs[s.run].text[s.start..s.end])
                .collect::<String>()
        })
        .collect();
    assert_eq!(all, ["Hello", "world", "again"]);
}

#[test]
fn line_boxes_follow_rules_4_and_5() {
    let mut e = engine();
    let text = "aaa bbb ccc";
    let run = e.shape(&RunStyle::new(FIRA_400, 13.0), text).unwrap();
    let m = e.metrics(FIRA_400, 13.0, 400.0).unwrap();
    let narrow = e.measure(&RunStyle::new(FIRA_400, 13.0), "aaa").unwrap();
    let lines = e.break_lines(std::slice::from_ref(&run), narrow);
    assert_eq!(lines.lines.len(), 3);
    assert_eq!(lines.lines[0].height, m.normal_line_height());
    assert_eq!(
        lines.lines[0].baseline,
        m.baseline_in(m.normal_line_height())
    );
    assert_eq!(lines.lines[2].top, 2.0 * m.normal_line_height());

    let opts = BreakOptions {
        max_width: narrow,
        wrap: true,
        line_height: LineHeight::Px(20.0),
    };
    let lines = e.break_lines_with(std::slice::from_ref(&run), &opts);
    let tops: Vec<f32> = lines.lines.iter().map(|l| l.top).collect();
    assert_eq!(tops, [0.0, 20.0, 40.0]);
    assert_eq!(lines.lines[1].baseline, 14.0, "floor((20 - 15) / 2) + 12");
    assert_eq!(lines.height, 60.0);
}

#[test]
fn empty_text_has_no_lines() {
    let mut e = engine();
    let run = e.shape(&st(), "").unwrap();
    assert!(
        e.break_lines(std::slice::from_ref(&run), 100.0)
            .lines
            .is_empty()
    );
}

#[test]
fn ellipsis_keeps_the_longest_prefix_that_fits() {
    let mut e = engine();
    let text = "Alexandria was a major centre";
    let run = e.shape(&st(), text).unwrap();
    // Fits: untouched.
    let r = e.ellipsize(&run, run.width).unwrap();
    assert!(!r.truncated);
    assert_eq!(r.run, run);
    // Truncated.
    let max = 80.0;
    let r = e.ellipsize(&run, max).unwrap();
    assert!(r.truncated);
    assert!(r.run.text.ends_with('\u{2026}'));
    assert!(r.run.width <= max, "{}", r.run.width);
    assert_eq!(&*r.run.text, format!("{}\u{2026}", &text[..r.kept]));
    // One more cluster would not fit.
    let next = run
        .cluster_starts()
        .into_iter()
        .find(|&b| b > r.kept)
        .unwrap();
    let longer = e
        .shape(&st(), &format!("{}\u{2026}", &text[..next]))
        .unwrap();
    assert!(longer.width > max, "{} should exceed {max}", longer.width);
    // Narrower than the ellipsis alone: just the ellipsis.
    let r = e.ellipsize(&run, 2.0).unwrap();
    assert_eq!((&*r.run.text, r.kept), ("\u{2026}", 0));
}

#[test]
fn engine_works_behind_the_measure_trait() {
    fn lines_of(m: &mut dyn TextMeasure, s: &str, width: f32) -> usize {
        let run = m.shape(&st(), s).unwrap();
        m.break_lines(std::slice::from_ref(&run), &BreakOptions::new(width))
            .lines
            .len()
    }
    let mut e = engine();
    assert_eq!(lines_of(&mut e, "one two three", 1000.0), 1);
    assert_eq!(lines_of(&mut e, "one two three", 10.0), 3);
}
