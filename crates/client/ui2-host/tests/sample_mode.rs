//! The sample mode (`?ui2sample=`): the first skin's `sample/hud.json` plays the game; the strip shows its values
//! without a replica or a network, its `ui` part opens what it says, and a bad sample changes nothing.

mod support;

use std::path::PathBuf;

use ui2_host::{Env, V2Frame, V2Init, V2Ui};

fn sample(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../skins/engraved/sample")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn frame(ui: &mut V2Ui, now: f64) {
    let r = ui.frame(V2Frame {
        replica: None,
        steps: &[],
        net: None,
        events: &[],
        mods: None,
        now_ms: now,
    });
    assert!(r.errors.is_empty(), "{:?}", r.errors);
}

#[test]
fn the_strip_shows_the_sample_without_a_game() {
    let text = sample("hud.json");
    let json: serde_json::Value = serde_json::from_str(&text).expect("json");
    let gold = json["model"]["top"]["gold"].to_string();
    let turn = json["model"]["game"]["turn"].to_string();
    let mut ui = V2Ui::new(
        support::strip(),
        V2Init {
            env: Env {
                viewport_px: (1280, 720),
                ..Env::default()
            },
            ..V2Init::default()
        },
    )
    .expect("the skin loads");
    ui.set_sample(&text).expect("the sample reads");
    assert!(ui.sample_mode());
    frame(&mut ui, 16.0);
    let texts = ui.texts();
    assert!(texts.contains(&gold), "gold {gold} in {texts:?}");
    assert!(
        texts.contains(&format!("Turn {turn}")),
        "turn {turn} in {texts:?}"
    );
    // Later frames write nothing more: the sample is a still picture.
    let rev = ui.store().revision();
    frame(&mut ui, 32.0);
    assert_eq!(ui.store().revision(), rev);
    ui.place_sample_anchors(1.0);
}

/// A map layer with one banner per resource (an anchor of kind city keyed by the resource's id, 40 x 10 px, in flow:
/// laid out one under the other).
fn banners() -> std::sync::Arc<ui2_package::Package> {
    use ui2_package::profile::action::AnchorKind;
    use ui2_package::profile::expr::parse;
    use ui2_package::style::{Display, Len, LenAuto, StyleRecord};
    use ui2_package::{BindKind, Layer, NodeKind, TemplateId, node_flags};
    use ui2_runtime::testkit::{Kit, TNode};

    let mut kit = Kit::new();
    let mut layer = StyleRecord::initial();
    layer.display = Display::Block;
    let mut banner = layer.clone();
    banner.width = LenAuto::Len(Len::px(40.0));
    banner.height = LenAuto::Len(Len::px(10.0));
    let (layer, banner) = (kit.record(layer), kit.record(banner));
    let (layer, banner) = (kit.plain(layer), kit.plain(banner));
    let list = kit.expr(parse("top.resources").expect("list"));
    let key = kit.expr(parse("r.id").expect("key"));
    let item = kit.str("r");
    let part = kit.str("banner");
    kit.template(
        "doc",
        vec![TNode::layer(Layer::Map, layer).child(TNode::new(
            NodeKind::For {
                tpl: TemplateId(1),
                item,
                index: None,
                list,
                key,
                virt: false,
                sort: None,
                sort_desc: None,
                filter: None,
            },
            layer,
        ))],
    );
    let id = kit.expr(parse("r.id").expect("anchor key"));
    kit.template(
        "banner",
        vec![
            TNode::el(banner)
                .part(part)
                .flags(node_flags::ANCHOR)
                .bind(BindKind::Anchor {
                    kind: AnchorKind::City,
                    id,
                }),
        ],
    );
    kit.finish().expect("package")
}

/// A sample with no anchor points (`"anchors": {}`, as the skins' samples have) shows the map layer as the skin's
/// page in a browser lays it out: each banner where the layout puts it, at any device scale. Before, every anchor
/// the sample did not name was hidden, so the sample mode showed no city banner and no unit flag.
#[test]
fn banners_the_sample_does_not_place_stand_where_the_layout_puts_them() {
    let resource = |id: u32| serde_json::json!({ "id": format!("0:{id}"), "name": format!("r{id}"), "icon": "icon:R:1", "available": 1, "used": 0, "total": 1 });
    let sample = serde_json::json!({
        "model": { "top": { "resources": [resource(1), resource(2), resource(3)] } },
        "anchors": {}
    });
    for dpr in [1.0f32, 1.5] {
        let mut ui = V2Ui::new(
            banners(),
            V2Init {
                env: Env {
                    viewport_px: ((1280.0 * dpr) as u32, (720.0 * dpr) as u32),
                    dpr,
                    ..Env::default()
                },
                ..V2Init::default()
            },
        )
        .expect("the skin loads");
        ui.set_sample(&sample.to_string())
            .expect("the sample reads");
        frame(&mut ui, 16.0);
        assert_eq!(ui.anchor_count(), 3, "one anchor per banner");
        ui.place_sample_anchors(dpr);
        // The whole document's records (a part names its first element only): the banners are the 40 x 10 boxes.
        let mut tops: Vec<(f32, f32, f32, f32)> = ui
            .runtime()
            .dump_rects(None)
            .iter()
            .map(|r| (r.x, r.y, r.w, r.h))
            .filter(|r| (r.2, r.3) == (40.0, 10.0))
            .collect();
        tops.sort_by(|a, b| a.1.total_cmp(&b.1));
        assert_eq!(
            tops,
            vec![
                (0.0, 0.0, 40.0, 10.0),
                (0.0, 10.0, 40.0, 10.0),
                (0.0, 20.0, 40.0, 10.0)
            ],
            "the three banners as laid out (CSS px) at dpr {dpr}"
        );
    }
}

#[test]
fn a_sample_that_does_not_fit_the_model_is_refused() {
    let mut ui = V2Ui::new(support::strip(), V2Init::default()).expect("the skin loads");
    let err = ui
        .set_sample(r#"{"model": {"top": {"gold": "lots"}}}"#)
        .expect_err("refused");
    assert!(err.contains("top"), "{err}");
    assert!(!ui.sample_mode());
}
