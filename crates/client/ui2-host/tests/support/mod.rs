//! A small top strip package built with the runtime's test kit: one HUD layer, a row of boxes whose text leaves
//! bind the first-slice model paths. Replaced by the converter's package of the first skin once the build
//! writes one.

// Each test file uses part of this module.
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;

use replica::testing::FakeRead;
use replica::{Activity, Notification, Unit, UnitFlags};
use ui2_actions::{OrderSink, RecordingSink};
use ui2_host::HostNet;
use ui2_package::profile::TypeSig;
use ui2_package::profile::expr::{TplSeg, parse, parse_template};
use ui2_package::style::{Display, Len, StyleRecord};
use ui2_package::{Layer, Package};
use ui2_runtime::testkit::{Kit, TNode, TplPiece};
use ui2_source::SessionRead;
use ui2_source::session::testing::FakeSession;

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

/// A package of one empty HUD layer with script files (`(path, code)`) that read the model roots `reads`. The
/// test kit writes no scripts, so this one is built with the package writer directly.
pub fn scripted(files: &[(&str, &str)], reads: &[&str]) -> Arc<Package> {
    use ui2_package::{Node, NodeKind, PackageBuilder, StyleSet, Template};
    let mut b = PackageBuilder::new();
    let record = b.style_record(StyleRecord::initial());
    let style = b.style_set(StyleSet::plain(vec![record]));
    let name = b.str("doc");
    b.template(Template {
        name,
        nodes: vec![Node {
            kind: NodeKind::Layer(Layer::Hud),
            subtree: 1,
            style,
            part: None,
            flags: 0,
        }],
    });
    for (path, code) in files {
        b.script(path, code.as_bytes());
    }
    b.script_reads(reads);
    let bytes = b.finish(Kit::manifest()).expect("package");
    Arc::new(Package::read(Arc::from(bytes)).expect("readable"))
}

/// A network for the tests: orders are recorded and numbered, the session is plain data.
#[derive(Default)]
pub struct FakeNet {
    pub sink: RecordingSink,
    pub session: FakeSession,
}

impl HostNet for FakeNet {
    fn sink(&mut self) -> &mut dyn OrderSink {
        &mut self.sink
    }

    fn session(&self) -> &dyn SessionRead {
        &self.session
    }
}

/// A unit of seat 0 at `(x, 4)`.
pub fn unit(id: i32, x: i16) -> Unit {
    Unit {
        id,
        owner: 0,
        unit_type: 1,
        x,
        y: 4,
        hp: 100,
        max_hp: 100,
        moves_left: 120,
        max_moves: 120,
        strength: 8,
        ranged: 0,
        range: 0,
        domain: 2,
        activity: Activity::Awake,
        level: 1,
        experience: 0,
        flags: UnitFlags::default(),
        transport_id: None,
        build_type: None,
        facing: None,
        name: format!("Warrior {id}"),
    }
}

/// A notification without a plot.
pub fn note(id: i32) -> Notification {
    Notification {
        id,
        kind: 0,
        x: -1,
        y: -1,
        data1: 0,
        data2: 0,
        turn: 87,
        dismissed: false,
        message: format!("note {id}"),
        summary: format!("note {id}"),
    }
}

/// Seat 0 at turn 87 with two units (at (10, 4) and (11, 4)) and two notifications.
pub fn game() -> FakeRead {
    let mut f = FakeRead::new(0, 0);
    f.info.turn = 87;
    f.units = vec![unit(1, 10), unit(2, 11)];
    f.notes = vec![note(2), note(3)];
    f
}
