//! Style selection: condition masks, conditions on ancestors, hover sensitivity, viewport cells, theme tokens,
//! UI scale.

use std::sync::Arc;

use ui2_package::style::{ColorV, Len, LenAuto, StyleRecord};
use ui2_package::{Cond, CondKind, Layer, Package, StyleSetId, Theme, TokenId, Tokens};
use ui2_runtime::testkit::{Kit, TNode};
use ui2_runtime::{Dirty, Env, ErrorCode, NodeId, Runtime, Services, StateBits};

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const BONE: [u8; 4] = [10, 20, 30, 255];
const LIGHT: [u8; 4] = [200, 200, 200, 255];

fn bg(c: [u8; 4]) -> StyleRecord {
    let mut r = StyleRecord::initial();
    r.background_color = ColorV::Lit(c);
    r
}

fn width(px: f32) -> StyleRecord {
    let mut r = StyleRecord::initial();
    r.width = LenAuto::Len(Len::px(px));
    r
}

/// Two viewport cells (width 1600 breakpoint) and nodes exercising every kind of selection.
fn package() -> Arc<Package> {
    let mut kit = Kit::new();
    kit.media(&[1600.0], &[]);
    let base = kit.record(StyleRecord::initial());
    let hov = kit.record(bg(RED));
    let cls = kit.record(bg(GREEN));
    let both = kit.record(bg(BLUE));
    let narrow = kit.record(width(50.0));
    let wide = kit.record(width(100.0));
    let mut tok = StyleRecord::initial();
    tok.color = ColorV::Tok(TokenId(0));
    let tok = kit.record(tok);

    let on = kit.str("on");
    let open = kit.str("open");
    let bone = kit.str("--bone");
    let dark = kit.str("dark");
    let light = kit.str("light");
    kit.tokens(Tokens {
        roots: vec![(bone, BONE)],
        aliases: vec![],
        themes: vec![
            Theme {
                name: dark,
                set: vec![],
            },
            Theme {
                name: light,
                set: vec![(TokenId(0), LIGHT)],
            },
        ],
    });

    let plain = kit.plain(base);
    let cond = |up, kind| Cond { up, kind };
    let btn_table = [base, hov, cls, both];
    let btn = kit.set(
        vec![cond(0, CondKind::Hover), cond(0, CondKind::Class(on))],
        [btn_table, btn_table].concat(),
    );
    let child = kit.set(vec![cond(1, CondKind::Hover)], vec![base, hov, base, hov]);
    let resp = kit.set(vec![], vec![narrow, wide]);
    let tok = kit.plain(tok);
    let deep = kit.set(
        vec![cond(2, CondKind::Class(open))],
        vec![base, cls, base, cls],
    );

    let mut part = |name: &str| kit.str(name);
    let p = [
        "hud", "panel", "btn", "icon", "resp", "tok", "a", "b", "deep", "c1", "c2", "c3",
    ]
    .map(&mut part);
    let label = kit.str("Label");
    let el = |set: StyleSetId, name| TNode::el(set).part(name);
    kit.template(
        "doc",
        vec![
            TNode::layer(Layer::Hud, plain).part(p[0]).child(
                el(plain, p[1]).children([
                    el(btn, p[2])
                        .child(TNode::text(label, btn))
                        .child(el(child, p[3])),
                    el(resp, p[4]),
                    el(tok, p[5]),
                    el(plain, p[6]).child(el(plain, p[7]).child(el(deep, p[8]))),
                    el(plain, p[9]).child(el(plain, p[10]).child(el(plain, p[11]))),
                ]),
            ),
        ],
    );
    kit.finish().expect("package")
}

fn env(w: u32, h: u32) -> Env {
    Env {
        viewport_px: (w, h),
        ..Env::default()
    }
}

/// A runtime after its first frame, with every queue drained (as the later passes would).
fn settled(e: Env) -> Runtime {
    let mut rt = Runtime::new(package(), e, Services::default()).expect("runtime");
    tick(&mut rt);
    drain(&mut rt);
    rt
}

fn drain(rt: &mut Runtime) {
    for d in [Dirty::LAYOUT, Dirty::TEXT, Dirty::PAINT, Dirty::BIND] {
        rt.take_dirty(d);
    }
}

fn part(rt: &Runtime, name: &str) -> NodeId {
    rt.find_part(name).unwrap_or_else(|| panic!("part {name}"))
}

fn background(rt: &Runtime, id: NodeId) -> [u8; 4] {
    rt.color(rt.style(id).expect("style").background_color)
}

fn class(rt: &Runtime, name: &str) -> ui2_package::StrId {
    rt.package().str_id(name).expect("class")
}

#[test]
fn style_mask_follows_state_and_classes() {
    let mut rt = settled(Env::default());
    let btn = part(&rt, "btn");
    let on = class(&rt, "on");
    assert_eq!(background(&rt, btn), [0, 0, 0, 0]);

    rt.set_state(btn, StateBits::HOVER, true);
    let stats = tick(&mut rt);
    assert_eq!(stats.restyled, 3, "the button, its text leaf and the icon");
    assert_eq!(background(&rt, btn), RED);
    assert_eq!(rt.node(btn).map(|n| n.cond_mask()), Some(0b01));
    // A color change needs paint only: the chunk of the button.
    assert_eq!((stats.layout_runs, stats.chunks_rebuilt), (0, 1));

    rt.set_class(btn, on, true);
    tick(&mut rt);
    assert_eq!(background(&rt, btn), BLUE);
    rt.set_state(btn, StateBits::HOVER, false);
    tick(&mut rt);
    assert_eq!(background(&rt, btn), GREEN);
    rt.set_class(btn, on, false);
    tick(&mut rt);
    assert_eq!(background(&rt, btn), [0, 0, 0, 0]);
    // Switching a class that is already off changes nothing.
    rt.set_class(btn, on, false);
    assert_eq!(tick(&mut rt).restyled, 0);
}

#[test]
fn style_conditions_on_ancestors_follow_them() {
    let mut rt = settled(Env::default());
    let btn = part(&rt, "btn");
    let icon = part(&rt, "icon");
    let text = rt.node(btn).expect("btn").children()[0];
    rt.set_state(btn, StateBits::HOVER, true);
    tick(&mut rt);
    assert_eq!(background(&rt, icon), RED, "`:hover` one element up");
    assert_eq!(
        background(&rt, text),
        RED,
        "a text leaf selects like its parent"
    );

    let a = part(&rt, "a");
    let deep = part(&rt, "deep");
    rt.set_class(a, class(&rt, "open"), true);
    assert_eq!(tick(&mut rt).restyled, 1);
    assert_eq!(background(&rt, deep), GREEN, "a class two elements up");
    // The class is on `a`, not on `b`: setting it on `b` does nothing for `deep`.
    let b = part(&rt, "b");
    rt.set_class(a, class(&rt, "open"), false);
    rt.set_class(b, class(&rt, "open"), true);
    tick(&mut rt);
    assert_eq!(background(&rt, deep), [0, 0, 0, 0]);
}

#[test]
fn style_hover_restyles_only_sensitive_nodes() {
    let mut rt = settled(Env::default());
    let c3 = part(&rt, "c3");
    rt.set_hover(Some(c3));
    assert_eq!(
        tick(&mut rt).restyled,
        0,
        "nothing tests hover on this chain"
    );
    for name in ["c3", "c2", "c1", "panel", "hud"] {
        let n = part(&rt, name);
        assert!(
            rt.node(n)
                .is_some_and(|n| n.state().contains(StateBits::HOVER)),
            "{name}"
        );
    }

    let icon = part(&rt, "icon");
    rt.set_hover(Some(icon));
    let stats = tick(&mut rt);
    assert_eq!(stats.restyled, 3, "the button, its text leaf and the icon");
    assert_eq!(stats.visited, 6, "restyled, then queued for paint");
    assert!(!rt.node(c3).expect("c3").state().contains(StateBits::HOVER));
    assert!(
        rt.node(part(&rt, "panel"))
            .expect("panel")
            .state()
            .contains(StateBits::HOVER)
    );

    rt.set_hover(None);
    assert_eq!(tick(&mut rt).restyled, 3);
    assert_eq!(background(&rt, part(&rt, "btn")), [0, 0, 0, 0]);
}

#[test]
fn style_cell_change_restyles_varying_sets_only() {
    let mut rt = settled(env(1280, 720));
    let resp = part(&rt, "resp");
    let w = |rt: &Runtime| rt.style(resp).map(|s| s.width);
    assert_eq!(rt.style_env().cell, 0);
    assert_eq!(w(&rt), Some(LenAuto::Len(Len::px(50.0))));

    rt.set_env(env(1920, 1080));
    let stats = tick(&mut rt);
    assert_eq!(
        stats.restyled, 1,
        "only the node whose set differs between cells"
    );
    assert_eq!(w(&rt), Some(LenAuto::Len(Len::px(100.0))));
    assert_eq!(
        stats.layout_runs, 1,
        "a width change and a new viewport lay out again"
    );
    assert_eq!(
        stats.chunks_rebuilt, 2,
        "the layer root (a new size) and the panel"
    );

    // UI scale 150 %: the logical viewport is 1280 px wide, back in the first cell.
    rt.set_env(Env {
        ui_scale: 1.5,
        ..env(1920, 1080)
    });
    let stats = tick(&mut rt);
    assert_eq!(rt.style_env().scale.k, 1.5);
    assert_eq!(rt.style_env().scale.viewport_css, [1280.0, 720.0]);
    assert_eq!(rt.style_env().cell, 0);
    assert_eq!(w(&rt), Some(LenAuto::Len(Len::px(50.0))));
    // The device scale changed: every chunk repaints (the layer root and the panel).
    assert_eq!(stats.chunks_rebuilt, 2);
    let _ = resp;
}

#[test]
fn style_theme_switch_repaints_token_nodes_only() {
    let mut rt = settled(Env::default());
    let tok = part(&rt, "tok");
    let color = |rt: &Runtime| rt.color(rt.style(tok).expect("style").color);
    assert_eq!(color(&rt), BONE);

    rt.set_env(Env {
        theme: 1,
        ..Env::default()
    });
    let stats = tick(&mut rt);
    assert_eq!(stats.restyled, 0, "a theme reselects nothing");
    assert_eq!(stats.chunks_rebuilt, 1, "the chunk of tok only");
    assert_eq!(stats.layout_runs, 0);
    assert_eq!(color(&rt), LIGHT);
}

#[test]
fn style_idle_frames_do_no_work() {
    let mut rt = settled(Env::default());
    let btn = part(&rt, "btn");
    rt.set_hover(Some(btn));
    rt.set_env(Env {
        theme: 1,
        ..env(1280, 720)
    });
    tick(&mut rt);
    drain(&mut rt);
    let rev = rt.arena().revision();
    for _ in 0..100 {
        let s = tick(&mut rt);
        assert_eq!((s.restyled, s.visited), (0, 0));
    }
    // Setting the same state again is not a change.
    rt.set_hover(Some(btn));
    rt.set_env(Env {
        theme: 1,
        ..env(1280, 720)
    });
    assert_eq!(tick(&mut rt).visited, 0);
    assert_eq!(rt.arena().revision(), rev);
    for d in [
        Dirty::STYLE,
        Dirty::LAYOUT,
        Dirty::TEXT,
        Dirty::PAINT,
        Dirty::BIND,
    ] {
        assert_eq!(rt.arena().pending(d), 0);
    }
}

#[test]
fn style_removed_dependents_are_forgotten() {
    let mut rt = settled(Env::default());
    let btn = part(&rt, "btn");
    let icon = part(&rt, "icon");
    rt.remove(icon);
    rt.set_state(btn, StateBits::HOVER, true);
    assert_eq!(tick(&mut rt).restyled, 2, "the button and its text leaf");
    assert_eq!(background(&rt, btn), RED);
}

#[test]
fn style_bad_scale_is_replaced_and_reported_once() {
    let mut rt = settled(Env::default());
    let mut errors = Vec::new();
    for _ in 0..10 {
        rt.set_env(Env {
            dpr: f32::NAN,
            ..Env::default()
        });
        errors.extend(frame(&mut rt).errors);
    }
    assert_eq!(rt.style_env().scale.k, 1.0);
    let bad_env = errors
        .iter()
        .filter(|e| e.code == ErrorCode::BadEnv)
        .count();
    assert_eq!(bad_env, 1, "{errors:?}");
}

fn tick(rt: &mut Runtime) -> ui2_runtime::FrameStats {
    frame(rt).stats
}

fn frame(rt: &mut Runtime) -> ui2_runtime::FrameOutput {
    let store = ui2_model::ModelStore::new(ui2_model::Schema::v1());
    rt.frame(
        0.0,
        &store,
        &mut ui2_text::TextEngine::new(),
        &mut ui2_draw::Scene::new(1.0),
    )
}
