//! A small top strip package built with the runtime's test kit: one HUD layer, a row of boxes whose text leaves
//! bind the first-slice model paths. Replaced by the converter's package of the first skin once the build
//! writes one.

// Each test file uses part of this module.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;

use ui2_package::profile::TypeSig;
use ui2_package::profile::expr::{TplSeg, parse, parse_template};
use ui2_package::style::{Display, Len, StyleRecord};
use ui2_package::{Layer, Package};
use ui2_runtime::testkit::{Kit, TNode, TplPiece};

/// The face of the strip.
pub fn fira() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../third_party/fonts/fira-sans/FiraSans-Regular.ttf");
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The bound texts of the strip, in document order: (part, binding template).
pub const BOUND: &[(&str, &str)] = &[
    ("turn", "Turn {{game.turn}}"),
    ("gold", "{{top.gold}}"),
    ("science", "{{fmt.signed(top.science_rate)}}"),
    ("research", "{{research.current.name}}"),
];

/// The skin text of the research turns (as in the first skin's texts).
pub const TURNS: (&str, &str) = ("top.turns", "{1_Num} {1_Num: plural 1?turn; other?turns;}");

fn tpl(kit: &mut Kit, src: &str) -> ui2_package::TplId {
    let t = parse_template(src).unwrap_or_else(|e| panic!("{src}: {e}"));
    let pieces = t
        .segs
        .into_iter()
        .map(|s| match s {
            TplSeg::Lit(l) => TplPiece::Lit(l),
            TplSeg::Expr(e) => TplPiece::Expr(e),
        })
        .collect();
    kit.tpl_of(pieces)
}

/// The kit of the strip, before `finish`, so tests can add to it.
pub fn strip_kit(font: &[u8]) -> Kit {
    let mut kit = Kit::new();
    kit.font("Fira Sans", 400, false, font);
    kit.l10n("en_US", TURNS.0, TURNS.1);
    kit.used_path("top.gold", TypeSig::Int);
    kit.used_path("game.turn", TypeSig::Int);
    let mut cell = StyleRecord::initial();
    cell.display = Display::Block;
    cell.font_family = vec![kit.str("Fira Sans")];
    cell.font_size = Len::px(14.0);
    cell.padding_left = Len::px(8.0);
    let mut row = cell.clone();
    row.display = Display::Flex;
    let (cell, row) = (kit.record(cell), kit.record(row));
    let (cell, row) = (kit.plain(cell), kit.plain(row));
    let mut items = Vec::new();
    for (part, src) in BOUND {
        let t = tpl(&mut kit, src);
        let p = kit.str(part);
        items.push(TNode::el(cell).part(p).child(TNode::bound_text(t, cell)));
    }
    let key = kit.str(TURNS.0);
    let arg = kit.expr(parse("research.current.turns").expect("expression"));
    let p = kit.str("turns");
    items.push(
        TNode::el(cell)
            .part(p)
            .child(TNode::l10n_text(key, vec![arg], cell)),
    );
    let top = kit.str("top");
    kit.template(
        "doc",
        vec![TNode::layer(Layer::Hud, row).child(TNode::el(row).part(top).children(items))],
    );
    kit
}

/// The strip package.
pub fn strip() -> Arc<Package> {
    strip_kit(&fira()).finish().expect("package")
}
