//! A game picture whose asset arrives after the skin has drawn (the game's UI icon set loads after the skin): the
//! host sets the resolver again (`V2Ui::set_asset_resolver`) and the next frame draws the picture, on the map
//! layer too (a unit flag's icon over its fallback mark), with nothing else changed.

use std::rc::Rc;
use std::sync::Arc;

use ui2_host::ExtTexture;
use ui2_host::{Env, ExtImage, V2Frame, V2Init, V2Ui};

/// A map layer with one flag per resource (an anchor keyed by the resource's id), each with the picture its
/// `icon` names, bound as a unit flag's is.
fn flags() -> Arc<ui2_package::Package> {
    use ui2_package::profile::action::AnchorKind;
    use ui2_package::profile::expr::parse;
    use ui2_package::style::{Display, Len, LenAuto, StyleRecord};
    use ui2_package::{BindKind, ImageSrc, Layer, NodeKind, TemplateId, node_flags};
    use ui2_runtime::testkit::{Kit, TNode};

    let mut kit = Kit::new();
    let mut layer = StyleRecord::initial();
    layer.display = Display::Block;
    let mut flag = layer.clone();
    flag.width = LenAuto::Len(Len::px(40.0));
    flag.height = LenAuto::Len(Len::px(40.0));
    let mut pic = layer.clone();
    pic.width = LenAuto::Len(Len::px(32.0));
    pic.height = LenAuto::Len(Len::px(32.0));
    let (layer, flag, pic) = (kit.record(layer), kit.record(flag), kit.record(pic));
    let (layer, flag, pic) = (kit.plain(layer), kit.plain(flag), kit.plain(pic));
    let list = kit.expr(parse("top.resources").expect("list"));
    let key = kit.expr(parse("r.id").expect("key"));
    let item = kit.str("r");
    let part = kit.str("flag");
    let pic_part = kit.str("flag.pic");
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
    let icon = kit.expr(parse("r.icon").expect("icon"));
    kit.template(
        "flag",
        vec![
            TNode::el(flag)
                .part(part)
                .flags(node_flags::ANCHOR)
                .bind(BindKind::Anchor {
                    kind: AnchorKind::Unit,
                    id,
                })
                .child(TNode::new(NodeKind::Image(ImageSrc::Bound(icon)), pic).part(pic_part)),
        ],
    );
    kit.finish().expect("package")
}

fn frame(ui: &mut V2Ui, now: f64) {
    // The report's errors are the pictures the game has no asset for yet: expected before the set arrives.
    let _ = ui.frame(V2Frame {
        replica: None,
        steps: &[],
        net: None,
        events: &[],
        mods: None,
        now_ms: now,
    });
}

/// The external textures the scene draws with.
fn drawn(ui: &V2Ui) -> Vec<ExtTexture> {
    (1..=64u16)
        .filter_map(|s| ui.scene().ext_texture(s))
        .collect()
}

#[test]
fn a_flag_icon_whose_set_arrives_later_is_drawn_without_any_other_change() {
    let resource = |id: u32| serde_json::json!({ "id": format!("0:{id}"), "name": format!("r{id}"), "icon": format!("icon:UNIT_ATLAS_1:{id}"), "available": 1, "used": 0, "total": 1 });
    let sample = serde_json::json!({
        "model": { "top": { "resources": [resource(1), resource(2)] } },
        "anchors": {}
    });
    let mut ui = V2Ui::new(
        flags(),
        V2Init {
            env: Env {
                viewport_px: (1280, 720),
                ..Env::default()
            },
            ..V2Init::default()
        },
    )
    .expect("the skin loads");
    ui.set_sample(&sample.to_string())
        .expect("the sample reads");
    // The set is not loaded: the host's resolver finds nothing (as `GameIcons` before `set_store`).
    ui.set_asset_resolver(Rc::new(|_: &str| None));
    frame(&mut ui, 16.0);
    ui.place_sample_anchors(1.0);
    frame(&mut ui, 32.0);
    assert_eq!(ui.anchor_count(), 2, "one anchor per flag");
    assert!(
        drawn(&ui).is_empty(),
        "nothing to draw yet: {:?}",
        drawn(&ui)
    );
    // Idle frames change nothing.
    frame(&mut ui, 48.0);
    assert!(drawn(&ui).is_empty());

    // The set came: the same names now resolve. The model, the anchors and the layout are as they were.
    let rev = ui.store().revision();
    ui.set_asset_resolver(Rc::new(|name: &str| {
        let index: u64 = name.rsplit(':').next()?.parse().ok()?;
        Some(ExtImage {
            texture: ExtTexture(100 + index),
            uv: [0.0, 0.0, 0.25, 0.25],
        })
    }));
    frame(&mut ui, 64.0);
    assert_eq!(ui.store().revision(), rev, "no model change");
    let mut got = drawn(&ui);
    got.sort_by_key(|t| t.0);
    assert_eq!(
        got,
        vec![ExtTexture(101), ExtTexture(102)],
        "both flags draw their icon in the next frame"
    );
}
