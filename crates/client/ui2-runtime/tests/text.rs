//! Text in the tree: styled `span`s on one baseline, wrapping, the game's markup (`[NEWLINE]`,
//! `[COLOR_*]`, `[ICON_*]`), a language switch of the skin's texts, `text-overflow: ellipsis` and
//! `data-fit="shrink"`.

use std::rc::Rc;

use ui2_draw::{Color, Item, Scene};
use ui2_model::{ModelStore, Schema};
use ui2_package::style::{
    ColorV, Display, Len, LenAuto, Overflow, StyleRecord, TextOverflow, TextWrapMode,
};
use ui2_package::{BindKind, Layer, NodeKind, Tag, Theme, TokenId, Tokens};
use ui2_runtime::testkit::{Kit, TNode};
use ui2_runtime::{Env, ErrorCode, FrameOutput, NodeId, Runtime, Services, load_faces};
use ui2_text::TextEngine;

const RED: [u8; 4] = [200, 30, 30, 255];
const GREEN: [u8; 4] = [20, 220, 60, 255];

fn font(name: &str) -> Vec<u8> {
    std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../third_party/fonts/fira-sans")
            .join(name),
    )
    .expect("font")
}

fn env() -> Env {
    Env {
        viewport_px: (800, 600),
        dpr: 1.0,
        ui_scale: 1.0,
        theme: 0,
    }
}

struct Rig {
    rt: Runtime,
    engine: TextEngine,
    scene: Scene,
    store: ModelStore,
}

impl Rig {
    fn frame(&mut self) -> FrameOutput {
        self.rt
            .frame(0.0, &self.store, &mut self.engine, &mut self.scene)
    }

    fn idle(&mut self) {
        for _ in 0..10 {
            let s = self.frame().stats;
            assert_eq!(
                (
                    s.binds_evaluated,
                    s.shaped_runs,
                    s.layout_runs,
                    s.chunks_rebuilt,
                    s.visited
                ),
                (0, 0, 0, 0, 0)
            );
        }
    }

    fn part(&self, name: &str) -> NodeId {
        self.rt.find_part(name).expect("part")
    }

    /// The glyph runs of the HUD chunk: (color, first glyph x, y).
    fn glyph_runs(&mut self, part: &str) -> Vec<(Color, f32, f32)> {
        let chunk = self.rt.node(self.part(part)).expect("part").chunk();
        let (_, items) = self.rt.chunk_items(chunk).expect("chunk");
        items
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Glyphs(g) => g.glyphs.first().map(|p| (g.color, p.x, p.y)),
                _ => None,
            })
            .collect()
    }
}

/// A skin of text samples, each in its own part.
fn rig() -> Rig {
    let mut kit = Kit::new();
    kit.languages(&["en_US", "ru_RU"]);
    kit.font("Fira Sans", 400, false, &font("FiraSans-Regular.ttf"));
    kit.font("Fira Sans", 500, false, &font("FiraSans-Medium.ttf"));
    let loc_positive = kit.str("--ui-loc-positive");
    let (dark, light) = (kit.str("dark"), kit.str("light"));
    kit.tokens(Tokens {
        roots: vec![(loc_positive, GREEN)],
        aliases: vec![],
        themes: vec![
            Theme {
                name: dark,
                set: vec![],
            },
            Theme {
                name: light,
                set: vec![(TokenId(0), GREEN)],
            },
        ],
    });
    let mut base = StyleRecord::initial();
    base.display = Display::Block;
    base.font_family = vec![kit.str("Fira Sans")];
    base.font_size = Len::px(14.0);
    base.text_wrap_mode = TextWrapMode::Nowrap;
    let mut narrow = base.clone();
    narrow.width = LenAuto::Len(Len::px(90.0));
    narrow.text_wrap_mode = TextWrapMode::Wrap;
    let mut span = base.clone();
    span.display = Display::Inline;
    span.font_weight = 500;
    span.color = ColorV::Lit(RED);
    let mut clipped = base.clone();
    clipped.width = LenAuto::Len(Len::px(60.0));
    clipped.overflow_x = Overflow::Hidden;
    clipped.text_overflow = TextOverflow::Ellipsis;
    let mut fit = base.clone();
    fit.width = LenAuto::Len(Len::px(60.0));
    fit.overflow_x = Overflow::Hidden;
    let [base, narrow, span, clipped, fit] =
        [base, narrow, span, clipped, fit].map(|r| kit.record(r));
    let [base, narrow, span, clipped, fit] =
        [base, narrow, span, clipped, fit].map(|r| kit.plain(r));
    let s = |kit: &mut Kit, t: &str| kit.str(t);
    let parts = ["spans", "wrap", "markup", "lang", "ellipsis", "fit"].map(|p| kit.str(p));
    kit.l10n(
        "en_US",
        "x.gold",
        "Gold[NEWLINE][COLOR_POSITIVE]+5[ENDCOLOR] a turn[ICON_GOLD]",
    );
    kit.l10n("en_US", "x.hello", "Hello");
    kit.l10n("ru_RU", "x.hello", "Privet, mir");
    let gold_key = kit.str("x.gold");
    let hello_key = kit.str("x.hello");
    let texts = [
        s(&mut kit, "Gold "),
        s(&mut kit, "312"),
        s(&mut kit, " a turn"),
        s(&mut kit, "Production of the city goes on and on"),
        s(&mut kit, "Production of the city"),
    ];
    kit.template(
        "doc",
        vec![
            TNode::layer(Layer::Hud, base).children([
                TNode::el(base).part(parts[0]).children([
                    TNode::text(texts[0], base),
                    TNode::new(NodeKind::Box { tag: Tag::Span }, span)
                        .child(TNode::text(texts[1], span)),
                    TNode::text(texts[2], base),
                ]),
                TNode::el(narrow)
                    .part(parts[1])
                    .child(TNode::text(texts[3], narrow)),
                TNode::el(base)
                    .part(parts[2])
                    .child(TNode::l10n_text(gold_key, vec![], base)),
                TNode::el(base)
                    .part(parts[3])
                    .child(TNode::l10n_text(hello_key, vec![], base)),
                TNode::el(clipped)
                    .part(parts[4])
                    .child(TNode::text(texts[4], clipped)),
                TNode::el(fit)
                    .part(parts[5])
                    .bind(BindKind::Fit { min: Some(8.0) })
                    .child(TNode::text(texts[4], fit)),
            ]),
        ],
    );
    let pkg = kit.finish().expect("package");
    let mut engine = TextEngine::new();
    load_faces(&pkg, &mut engine).expect("fonts");
    let rt = Runtime::new(pkg, env(), Services::default()).expect("runtime");
    Rig {
        rt,
        engine,
        scene: Scene::new(1.0),
        store: ModelStore::new(Schema::v1()),
    }
}

fn rect(rt: &Runtime, part: &str, suffix: &str) -> [f32; 4] {
    let id = rt.find_part(part).expect("part");
    let label = format!("{}{suffix}", rt.path(id).label);
    let r = rt
        .dump_rects(Some(part))
        .into_iter()
        .find(|r| r.node == label)
        .unwrap_or_else(|| panic!("no rect {label}"));
    [r.x, r.y, r.w, r.h]
}

#[test]
fn text_spans_sit_on_one_baseline_and_paragraphs_wrap() {
    let mut rig = rig();
    let out = rig.frame();
    let errors: Vec<ErrorCode> = out.errors.iter().map(|e| e.code).collect();
    assert_eq!(
        errors,
        vec![ErrorCode::L10n],
        "the icon of the markup text, once"
    );

    // Three runs on one line: the span in its own weight and color, all on the same baseline.
    let runs = rig.glyph_runs("spans");
    assert_eq!(runs.len(), 3);
    assert_eq!(runs[1].0, Color(RED));
    assert!(runs[0].1 < runs[1].1 && runs[1].1 < runs[2].1);
    assert!(runs.iter().all(|r| r.2 == runs[0].2), "{runs:?}");
    let ifc = rig
        .rt
        .node(rig.part("spans"))
        .and_then(|n| n.ifc())
        .expect("ifc");
    assert_eq!(ifc.lines.lines.len(), 1);
    // The span starts where "Gold " ends.
    assert_eq!(ifc.lines.lines[0].segments[1].x, ifc.runs[0].shaped.width);

    // A paragraph of 90 px wraps at spaces; every line but overflowing words fits.
    let ifc = rig
        .rt
        .node(rig.part("wrap"))
        .and_then(|n| n.ifc())
        .expect("ifc");
    assert!(ifc.lines.lines.len() >= 3, "{}", ifc.lines.lines.len());
    assert!(ifc.lines.lines.iter().all(|l| l.width <= 90.0));
    let h = rect(&rig.rt, "wrap", "")[3];
    assert_eq!(h, ifc.lines.height);
    rig.idle();
}

#[test]
fn text_markup_breaks_lines_and_colors_pieces() {
    let mut rig = rig();
    rig.frame();
    let ifc = rig
        .rt
        .node(rig.part("markup"))
        .and_then(|n| n.ifc())
        .expect("ifc");
    let pieces: Vec<&str> = ifc.runs.iter().map(|r| &*r.shaped.text).collect();
    assert_eq!(pieces, ["Gold\n", "+5", " a turn"]);
    assert_eq!(ifc.lines.lines.len(), 2, "[NEWLINE] breaks the line");
    assert_eq!(ifc.runs[1].color, Some(ColorV::Tok(TokenId(0))));
    let runs = rig.glyph_runs("markup");
    let colors: Vec<Color> = runs.iter().map(|r| r.0).collect();
    assert_eq!(colors[1], Color(GREEN));
    assert_ne!(colors[0], Color(GREEN));
    assert!(
        runs[1].2 > runs[0].2,
        "the colored piece is on the second line"
    );
    rig.idle();
}

#[test]
fn text_a_language_switch_shapes_and_lays_out_the_skin_texts() {
    let mut rig = rig();
    rig.frame();
    let w = rect(&rig.rt, "lang", "#0")[2];
    let ru = loc::Localizer::new(
        loc::LanguageInfo::new("ru_RU", "Russian", 1),
        Box::new(loc::MapTable::new()),
    );
    rig.rt.set_localizer(Rc::new(ru));
    let s = rig.frame().stats;
    assert_eq!(s.binds_evaluated, 2, "the two skin texts");
    assert_eq!(s.shaped_runs, 1, "only the text that changed");
    assert_eq!(s.layout_runs, 1);
    let leaf = rig.rt.node(rig.part("lang")).expect("lang").children()[0];
    assert_eq!(
        rig.rt.node(leaf).and_then(|n| n.text()),
        Some("Privet, mir")
    );
    assert!(rect(&rig.rt, "lang", "#0")[2] > w);
    rig.idle();
}

/// An ellipsized text that is a shrinking flex item (`min-width: 0`): it is cut at the width the item
/// shrank to, and its rectangle is the kept text plus the ellipsis, as Chrome reports it.
#[test]
fn text_ellipsis_in_a_shrinking_flex_item_is_cut_at_the_item_width() {
    use ui2_package::style::LenNormal;
    let mut kit = Kit::new();
    kit.font("Fira Sans", 400, false, &font("FiraSans-Regular.ttf"));
    let mut base = StyleRecord::initial();
    base.display = Display::Block;
    base.font_family = vec![kit.str("Fira Sans")];
    base.font_size = Len::px(13.0);
    let mut row = base.clone();
    row.display = Display::Flex;
    row.width = LenAuto::Len(Len::px(160.0));
    row.column_gap = LenNormal::Len(Len::px(6.0));
    row.padding_right = Len::px(12.0);
    let mut item = base.clone();
    item.min_width = LenAuto::Len(Len::px(0.0));
    item.text_wrap_mode = TextWrapMode::Nowrap;
    item.overflow_x = Overflow::Hidden;
    item.overflow_y = Overflow::Hidden;
    item.text_overflow = TextOverflow::Ellipsis;
    let [base, row, item] = [base, row, item].map(|r| kit.record(r));
    let [base, row, item] = [base, row, item].map(|r| kit.plain(r));
    let (part, text) = (
        kit.str("tx"),
        kit.str("Production of the city goes on and on"),
    );
    kit.template(
        "doc",
        vec![TNode::layer(Layer::Hud, base).child(
            TNode::el(row).child(TNode::el(item).part(part).child(TNode::text(text, item))),
        )],
    );
    let pkg = kit.finish().expect("package");
    let mut engine = TextEngine::new();
    load_faces(&pkg, &mut engine).expect("fonts");
    let rt = Runtime::new(pkg, env(), Services::default()).expect("runtime");
    let mut rig = Rig {
        rt,
        engine,
        scene: Scene::new(1.0),
        store: ModelStore::new(Schema::v1()),
    };
    rig.frame();
    let boxr = rect(&rig.rt, "tx", "");
    let textr = rect(&rig.rt, "tx", "#0");
    let ifc = rig
        .rt
        .node(rig.part("tx"))
        .and_then(|n| n.ifc())
        .expect("ifc");
    assert_eq!(boxr[2], 160.0, "the item shrinks to the row's content box");
    let line = &ifc.lines.lines[0];
    assert_eq!(
        line.segments.last().map(|s| s.run),
        Some(ifc.runs.len()),
        "cut"
    );
    assert!(line.width <= boxr[2], "{} > {}", line.width, boxr[2]);
    // (Dumped rectangles are rounded to 1/1000 px.)
    assert!(
        (textr[2] - line.width).abs() < 1e-3,
        "the rectangle is the kept text and the ellipsis: {} vs {}",
        textr[2],
        line.width
    );
    rig.idle();
}

#[test]
fn text_ellipsis_cuts_and_fit_shrinks_one_line() {
    let mut rig = rig();
    rig.frame();
    // Ellipsis: the line ends with `…` inside the 60 px box.
    let ifc = rig
        .rt
        .node(rig.part("ellipsis"))
        .and_then(|n| n.ifc())
        .expect("ifc");
    let line = &ifc.lines.lines[0];
    let last = line.segments.last().expect("segment");
    assert_eq!(last.run, ifc.runs.len(), "the ellipsis run");
    assert!(line.width <= 60.0, "{}", line.width);
    let shown = ifc.shown();
    assert_eq!(
        &*shown.run(last.run).expect("ellipsis").shaped.text,
        "\u{2026}"
    );

    // Fit: the same text in a 60 px box is shrunk, at most down to 8 px, with at most four measures;
    // the box keeps its size.
    let ifc = rig
        .rt
        .node(rig.part("fit"))
        .and_then(|n| n.ifc())
        .expect("ifc");
    let fitted = ifc.fitted.as_ref().expect("fitted");
    assert!(
        fitted.scale < 1.0 && fitted.scale * 14.0 >= 8.0 - 1e-3,
        "{}",
        fitted.scale
    );
    assert!(
        fitted.lines.width <= 60.0 + 1e-3 || (fitted.scale * 14.0 - 8.0).abs() < 1e-3,
        "{}",
        fitted.lines.width
    );
    assert!(
        ifc.lines.width > 60.0,
        "the natural line is kept for layout"
    );
    assert_eq!(rect(&rig.rt, "fit", "")[2], 60.0);
    let size = |rig: &mut Rig, part: &str| {
        let chunk = rig.rt.node(rig.part(part)).expect("part").chunk();
        let (_, items) = rig.rt.chunk_items(chunk).expect("chunk");
        items
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Glyphs(g) => Some(g.size_px),
                _ => None,
            })
            .fold(f32::INFINITY, f32::min)
    };
    assert!(size(&mut rig, "fit") < 14.0);
    rig.idle();
}
