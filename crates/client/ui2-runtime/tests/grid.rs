//! Grid layout: the record's grid fields laid out by taffy, against boxes worked out by hand from the CSS
//! grid algorithm (explicit and implicit tracks, `fr`, `repeat`, `minmax`, gaps, placements, column flow,
//! `justify-items`, `justify-self`, `aspect-ratio`).

use ui2_package::style::{
    AspectRatio, Display, GridAutoFlow, GridLine, ItemsAlign, Len, LenAuto, LenNormal, Position,
    SelfAlign, StyleRecord, Track, TrackBreadth, TrackSize,
};
use ui2_package::{Layer, StyleSetId};
use ui2_runtime::testkit::{Kit, TNode};
use ui2_runtime::{Env, Runtime, Services};

fn px(v: f32) -> TrackSize {
    TrackSize::Breadth(TrackBreadth::Len(Len::px(v)))
}

fn fr(v: f32) -> TrackSize {
    TrackSize::Breadth(TrackBreadth::Fr(v))
}

/// A grid container at the page's corner, `w` wide (height auto).
fn grid(w: f32) -> StyleRecord {
    let mut r = StyleRecord::initial();
    r.display = Display::Grid;
    r.position = Position::Absolute;
    r.left = LenAuto::Len(Len::ZERO);
    r.top = LenAuto::Len(Len::ZERO);
    r.width = LenAuto::Len(Len::px(w));
    r
}

fn item() -> StyleRecord {
    let mut r = StyleRecord::initial();
    r.display = Display::Block;
    r
}

fn sized(w: Option<f32>, h: Option<f32>) -> StyleRecord {
    let mut r = item();
    if let Some(w) = w {
        r.width = LenAuto::Len(Len::px(w));
    }
    if let Some(h) = h {
        r.height = LenAuto::Len(Len::px(h));
    }
    r
}

/// Lays out one grid with its items (each a part `i0`, `i1`, ...) and returns the item boxes `[x, y, w, h]`
/// relative to the grid, and the grid's own box.
fn lay_out(container: StyleRecord, items: Vec<StyleRecord>) -> (Vec<[f32; 4]>, [f32; 4]) {
    let mut kit = Kit::new();
    let plain = |kit: &mut Kit, r: StyleRecord| -> StyleSetId {
        let id = kit.record(r);
        kit.plain(id)
    };
    let mut layer = StyleRecord::initial();
    layer.display = Display::Block;
    let layer = plain(&mut kit, layer);
    let g = plain(&mut kit, container);
    let names: Vec<_> = (0..items.len())
        .map(|i| kit.str(&format!("i{i}")))
        .collect();
    let grid_part = kit.str("grid");
    let children: Vec<TNode> = items
        .into_iter()
        .zip(&names)
        .map(|(r, n)| {
            let s = plain(&mut kit, r);
            TNode::el(s).part(*n)
        })
        .collect();
    kit.template(
        "doc",
        vec![
            TNode::layer(Layer::Hud, layer).child(TNode::el(g).part(grid_part).children(children)),
        ],
    );
    let pkg = kit.finish().expect("package");
    let mut rt = Runtime::new(pkg, Env::default(), Services::default()).expect("runtime");
    let store = ui2_model::ModelStore::new(ui2_model::Schema::v1());
    let out = rt.frame(
        0.0,
        &store,
        &mut ui2_text::TextEngine::new(),
        &mut ui2_draw::Scene::new(1.0),
    );
    assert!(out.errors.is_empty(), "{:?}", out.errors);
    let rect = |part: &str| {
        let r = rt.dump_rects(Some(part));
        let r = r.first().expect("a box");
        [r.x, r.y, r.w, r.h]
    };
    let boxes = (0..names.len()).map(|i| rect(&format!("i{i}"))).collect();
    (boxes, rect("grid"))
}

#[test]
fn grid_fixed_and_fr_columns_share_the_free_space() {
    // 400 px: 100 px, then 300 px split 1:2.
    let mut g = grid(400.0);
    g.grid_template_columns = vec![
        Track::Size(px(100.0)),
        Track::Size(fr(1.0)),
        Track::Size(fr(2.0)),
    ];
    let (b, own) = lay_out(g, vec![sized(None, Some(20.0)); 3]);
    assert_eq!(
        b,
        [
            [0.0, 0.0, 100.0, 20.0],
            [100.0, 0.0, 100.0, 20.0],
            [200.0, 0.0, 200.0, 20.0]
        ]
    );
    assert_eq!(own, [0.0, 0.0, 400.0, 20.0]);
}

#[test]
fn grid_repeat_gaps_and_implicit_rows() {
    // Two 50 px columns 10 apart; implicit rows of 30 px, 5 apart; items stretch to their cells.
    let mut g = grid(200.0);
    g.grid_template_columns = vec![Track::Repeat(2, vec![px(50.0)])];
    g.grid_auto_rows = vec![px(30.0)];
    g.column_gap = LenNormal::Len(Len::px(10.0));
    g.row_gap = LenNormal::Len(Len::px(5.0));
    let (b, own) = lay_out(g, vec![item(); 5]);
    assert_eq!(
        b,
        [
            [0.0, 0.0, 50.0, 30.0],
            [60.0, 0.0, 50.0, 30.0],
            [0.0, 35.0, 50.0, 30.0],
            [60.0, 35.0, 50.0, 30.0],
            [0.0, 70.0, 50.0, 30.0]
        ]
    );
    assert_eq!(own[3], 100.0, "three rows and two gaps");
}

#[test]
fn grid_placements_lines_and_spans() {
    // Columns 3 x 40 px, rows 20 px + 20 px. The first item at column line 2 spanning 2, row 2; the second
    // at the last column line counted from the end (-2 is the start of the last track), row 1; the third
    // flows into the first free cell.
    let mut g = grid(120.0);
    g.grid_template_columns = vec![Track::Repeat(3, vec![px(40.0)])];
    g.grid_template_rows = vec![Track::Size(px(20.0)), Track::Size(px(20.0))];
    let mut a = item();
    a.grid_column_start = GridLine::Line(2);
    a.grid_column_end = GridLine::Span(2);
    a.grid_row_start = GridLine::Line(2);
    let mut c = item();
    c.grid_column_start = GridLine::Line(-2);
    c.grid_row_start = GridLine::Line(1);
    let (b, _) = lay_out(g, vec![a, c, item()]);
    assert_eq!(
        b,
        [
            [40.0, 20.0, 80.0, 20.0],
            [80.0, 0.0, 40.0, 20.0],
            [0.0, 0.0, 40.0, 20.0]
        ]
    );
}

#[test]
fn grid_column_flow_fills_columns_first() {
    let mut g = grid(200.0);
    g.grid_template_rows = vec![Track::Size(px(10.0)), Track::Size(px(10.0))];
    g.grid_auto_columns = vec![px(30.0)];
    g.grid_auto_flow = GridAutoFlow::Column;
    let (b, _) = lay_out(g, vec![item(); 3]);
    assert_eq!(
        b,
        [
            [0.0, 0.0, 30.0, 10.0],
            [0.0, 10.0, 30.0, 10.0],
            [30.0, 0.0, 30.0, 10.0]
        ]
    );
}

#[test]
fn grid_minmax_and_content_tracks() {
    // `minmax(50px, 1fr)` next to a `max-content` track holding a 70 px item: 200 - 70 = 130 px.
    let mut g = grid(200.0);
    g.grid_template_columns = vec![
        Track::Size(TrackSize::MinMax(
            TrackBreadth::Len(Len::px(50.0)),
            TrackBreadth::Fr(1.0),
        )),
        Track::Size(TrackSize::Breadth(TrackBreadth::MaxContent)),
    ];
    let (b, _) = lay_out(
        g,
        vec![sized(None, Some(10.0)), sized(Some(70.0), Some(10.0))],
    );
    assert_eq!(b, [[0.0, 0.0, 130.0, 10.0], [130.0, 0.0, 70.0, 10.0]]);
}

#[test]
fn grid_justify_items_self_and_aspect_ratio() {
    // One 100 px column: items are centered by `justify-items`, one is pushed to the end by `justify-self`;
    // a 40 px wide item with `aspect-ratio: 2 / 1` (not stretched) is 20 px high.
    let mut g = grid(100.0);
    g.grid_template_columns = vec![Track::Size(px(100.0))];
    g.justify_items = ItemsAlign::Center;
    let mut end = sized(Some(20.0), Some(10.0));
    end.justify_self = SelfAlign::End;
    let mut ratio = sized(Some(40.0), None);
    ratio.aspect_ratio = AspectRatio::Ratio(2.0, 1.0);
    ratio.align_self = SelfAlign::Start;
    let (b, _) = lay_out(g, vec![sized(Some(40.0), Some(10.0)), end, ratio]);
    assert_eq!(
        b,
        [
            [30.0, 0.0, 40.0, 10.0],
            [80.0, 10.0, 20.0, 10.0],
            [30.0, 20.0, 40.0, 20.0]
        ]
    );
}
