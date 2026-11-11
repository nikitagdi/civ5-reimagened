//! Painting what the draw layer draws beyond plain boxes: outer shadows, background gradients (layers,
//! sizes, positions, repeats, clip boxes), outlines, elliptic corners, color pictures of the package and game
//! pictures (resolved by the host, bound by markup, the minimap); idle frames that do nothing and a style
//! change that builds one chunk again.

mod support;

use std::cell::RefCell;
use std::rc::Rc;

use glam::Vec2;
use serde_json::json;
use ui2_draw::{
    BoxItem, Color, ExtTexture, Fill, GradientShape, Item, LayerChange, LayerId, Scene,
};
use ui2_model::{FrameDelta, ModelStore, Sample, Schema};
use ui2_package::profile::expr::parse;
use ui2_package::style::{
    BgBox, BgImage, BgRepeat, BgSize, BorderStyle, ColorV, Display, GradDir, Gradient, Len,
    LenAuto, OutlineStyle, Position, Radius, Repeat, Shadow, Side, Stop, StyleRecord,
};
use ui2_package::{BindKind, ImageSrc, Layer, NodeKind, PropId};
use ui2_runtime::testkit::{Kit, TNode, TplPiece, path};
use ui2_runtime::{Env, ExtImage, FrameStats, Runtime, Services};
use ui2_text::TextEngine;

const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];

fn block(w: f32, h: f32) -> StyleRecord {
    let mut r = StyleRecord::initial();
    r.display = Display::Block;
    r.width = LenAuto::Len(Len::px(w));
    r.height = LenAuto::Len(Len::px(h));
    // Every case at the page's corner, sized by its border box.
    r.box_sizing = ui2_package::style::BoxSizing::BorderBox;
    r.position = Position::Absolute;
    r.left = LenAuto::Len(Len::ZERO);
    r.top = LenAuto::Len(Len::ZERO);
    r
}

fn stop(c: [u8; 4], pos: Option<f32>) -> Stop {
    Stop {
        color: ColorV::Lit(c),
        pos: pos.map(Len::pct),
    }
}

fn frame_of(store: &mut ModelStore, sample: &serde_json::Value) -> FrameDelta {
    let s = Sample::parse(Schema::v1(), &sample.to_string()).expect("sample");
    s.apply(store).expect("apply");
    store.take_frame().expect("a change")
}

struct Rig {
    rt: Runtime,
    engine: TextEngine,
    store: ModelStore,
    scene: Scene,
    sample: serde_json::Value,
    asked: Rc<RefCell<Vec<String>>>,
}

impl Rig {
    fn push(&mut self) -> FrameStats {
        let d = frame_of(&mut self.store, &self.sample);
        self.rt.apply_delta(&d, &self.store);
        self.tick()
    }

    fn tick(&mut self) -> FrameStats {
        let out = self
            .rt
            .frame(0.0, &self.store, &mut self.engine, &mut self.scene);
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        out.stats
    }

    fn items(&mut self, part: &str) -> Vec<Item> {
        let id = self.rt.find_part(part).expect("part");
        let chunk = self.rt.node(id).expect("node").chunk();
        self.rt.chunk_items(chunk).expect("chunk").1.items
    }
}

/// A HUD with one chunk per case, at device scale 2.
fn rig() -> Rig {
    let mut kit = Kit::new();
    let _mask = kit.mask_icons(&[("a-mask", [4, 4])]);
    let pics = kit.color_pictures(&[("b-picture", [10, 6])]);
    let portrait = kit.asset_ref("ui:tx:portrait.dds");
    let minimap = kit.asset_ref("render:minimap");

    let layer = {
        let mut r = StyleRecord::initial();
        r.display = Display::Block;
        r
    };
    // Shadow, color, two gradient layers, border and outline.
    let mut grad = block(100.0, 60.0);
    grad.background_color = ColorV::Lit([10, 20, 30, 255]);
    for (w, s) in [
        (&mut grad.border_top_width, &mut grad.border_top_style),
        (&mut grad.border_right_width, &mut grad.border_right_style),
        (&mut grad.border_bottom_width, &mut grad.border_bottom_style),
        (&mut grad.border_left_width, &mut grad.border_left_style),
    ] {
        *w = Len::px(2.0);
        *s = BorderStyle::Solid;
    }
    grad.background_image = vec![
        BgImage::Gradient(Gradient {
            conic: false,
            repeating: false,
            dir: GradDir::Angle(90.0),
            stops: vec![stop(RED, None), stop(BLUE, None)],
        }),
        BgImage::Gradient(Gradient {
            conic: true,
            repeating: true,
            dir: GradDir::Angle(90.0),
            stops: vec![
                stop(RED, Some(0.0)),
                stop(BLUE, None),
                stop(RED, Some(50.0)),
            ],
        }),
    ];
    grad.background_size = vec![
        BgSize::Size(LenAuto::Len(Len::px(20.0)), LenAuto::Len(Len::px(20.0))),
        BgSize::Size(LenAuto::Auto, LenAuto::Auto),
    ];
    grad.background_repeat = vec![BgRepeat::REPEAT];
    grad.background_clip = vec![BgBox::BorderBox, BgBox::PaddingBox];
    grad.box_shadow = vec![
        Shadow {
            inset: false,
            x: 1.0,
            y: 2.0,
            blur: 3.0,
            spread: 4.0,
            color: ColorV::Lit([0, 0, 0, 200]),
        },
        Shadow {
            inset: false,
            x: 0.0,
            y: 0.0,
            blur: 10.0,
            spread: 0.0,
            color: ColorV::Lit([0, 0, 0, 100]),
        },
    ];
    grad.outline_style = OutlineStyle::Solid;
    grad.outline_width = Len::px(1.0);
    grad.outline_offset = Len::px(2.0);
    grad.outline_color = ColorV::Lit([1, 2, 3, 255]);
    let r6 = Radius {
        h: Len::px(6.0),
        v: Len::px(6.0),
    };
    grad.border_top_left_radius = r6;
    grad.border_top_right_radius = r6;
    grad.border_bottom_right_radius = r6;
    grad.border_bottom_left_radius = r6;

    // A `to top right` gradient that does not repeat, at a position, and a bound size.
    let mut corner = block(40.0, 20.0);
    corner.background_image = vec![BgImage::Gradient(Gradient {
        conic: false,
        repeating: false,
        dir: GradDir::To(Side::TopRight),
        stops: vec![stop(RED, None), stop(BLUE, None)],
    })];
    corner.background_size = vec![BgSize::Size(
        LenAuto::Len(Len::px(10.0)),
        LenAuto::Len(Len::px(10.0)),
    )];
    corner.background_repeat = vec![BgRepeat {
        x: Repeat::NoRepeat,
        y: Repeat::NoRepeat,
    }];
    corner.background_position_x = vec![Len::px(5.0)];
    corner.background_position_y = vec![Len::pct(100.0)];

    // Elliptic corners: 20 px across, 10 px down.
    let mut ellipse = block(80.0, 40.0);
    ellipse.background_color = ColorV::Lit(RED);
    let r = Radius {
        h: Len::px(20.0),
        v: Len::px(10.0),
    };
    ellipse.border_top_left_radius = r;
    ellipse.border_top_right_radius = r;
    ellipse.border_bottom_right_radius = r;
    ellipse.border_bottom_left_radius = r;

    let mut picture = block(10.0, 6.0);
    picture.opacity = 0.5;
    picture.border_top_left_radius = Radius {
        h: Len::px(2.0),
        v: Len::px(2.0),
    };
    let asset = block(16.0, 16.0);

    let plain = |kit: &mut Kit, r: StyleRecord| {
        let id = kit.record(r);
        kit.plain(id)
    };
    let sets = [layer, grad, corner, ellipse, picture, asset].map(|r| plain(&mut kit, r));
    let names = [
        "grad", "corner", "ellipse", "picture", "portrait", "minimap", "bound",
    ]
    .map(|p| kit.str(p));
    let size = kit.tpl_of(vec![
        TplPiece::Expr(path("top.happiness")),
        TplPiece::Lit("px ".into()),
        TplPiece::Expr(path("top.happiness")),
        TplPiece::Lit("px".into()),
    ]);
    let src = kit.expr(parse("top.gold").expect("expr"));
    kit.template(
        "doc",
        vec![TNode::layer(Layer::Hud, sets[0]).children([
            TNode::el(sets[1]).part(names[0]),
            TNode::el(sets[2]).part(names[1]).bind(BindKind::Style {
                prop: PropId::BackgroundSize,
                tpl: size,
            }),
            TNode::el(sets[3]).part(names[2]),
            TNode::new(NodeKind::Image(ImageSrc::Pkg(pics[0])), sets[4]).part(names[3]),
            TNode::asset(portrait, sets[5]).part(names[4]),
            TNode::asset(minimap, sets[5]).part(names[5]),
            TNode::new(NodeKind::Image(ImageSrc::Bound(src)), sets[5]).part(names[6]),
        ])],
    );
    let pkg = kit.finish().expect("package");
    let env = Env {
        viewport_px: (800, 600),
        dpr: 1.0,
        ui_scale: 2.0,
        theme: 0,
    };
    let mut rt = Runtime::new(pkg, env, Services::default()).expect("runtime");
    let asked = Rc::new(RefCell::new(Vec::new()));
    let log = Rc::clone(&asked);
    rt.set_asset_resolver(Rc::new(move |name: &str| {
        log.borrow_mut().push(name.to_string());
        Some(ExtImage {
            texture: ExtTexture(1000 + name.len() as u64),
            uv: [0.0, 0.0, 0.5, 1.0],
        })
    }));
    let mut rig = Rig {
        rt,
        engine: TextEngine::new(),
        store: ModelStore::new(Schema::v1()),
        scene: Scene::new(1.0),
        sample: support::sample_json(),
        asked,
    };
    rig.sample["model"]["top"]["happiness"] = json!(10);
    rig.push();
    rig
}

fn boxes(items: &[Item]) -> Vec<&BoxItem> {
    items
        .iter()
        .filter_map(|i| match i {
            Item::Box(b) => Some(b),
            _ => None,
        })
        .collect()
}

fn gradient(b: &BoxItem) -> ui2_draw::Gradient {
    match b.fill {
        Fill::Gradient(g) => g,
        other => panic!("{other:?}"),
    }
}

#[test]
fn paint_shadows_gradient_layers_border_and_outline_in_order() {
    let mut rig = rig();
    let items = rig.items("grad");
    let kinds: Vec<&str> = items
        .iter()
        .map(|i| match i {
            Item::Shadow(_) => "shadow",
            Item::Box(b) => match b.fill {
                Fill::Solid(_) => "color",
                Fill::Gradient(_) => "gradient",
                _ => "border",
            },
            _ => "other",
        })
        .collect();
    // The last shadow first; the color; the bottom (conic) layer, then the top one; border; outline.
    assert_eq!(
        kinds,
        [
            "shadow", "shadow", "color", "gradient", "gradient", "border", "border"
        ]
    );
    // Device scale 2: the border box is 200 x 120, the padding box is inside the 4 px border.
    let (Item::Shadow(back), Item::Shadow(front)) = (&items[0], &items[1]) else {
        panic!("{items:?}");
    };
    assert_eq!((back.blur, back.color.0[3]), (20.0, 100));
    assert_eq!(
        (front.offset, front.blur, front.spread),
        (Vec2::new(2.0, 4.0), 6.0, 8.0)
    );
    assert_eq!(front.radii.tl, 12.0);
    let b = boxes(&items);
    // The color fills the clip box of the bottom layer (the padding box), with the inner radii.
    assert_eq!(b[0].rect, ui2_draw::Rect::new(4.0, 4.0, 192.0, 112.0));
    assert_eq!(b[0].radii.tl, 8.0);
    assert_eq!(b[0].fill, Fill::Solid(Color([10, 20, 30, 255])));
    // The conic layer fills its clip box (the padding box) once: centered, from 90 degrees, repeating,
    // with the middle stop spread between its neighbours.
    let conic = gradient(b[1]);
    assert_eq!(b[1].rect, b[0].rect);
    assert!(conic.repeating && conic.tile.is_none());
    assert_eq!(
        conic.shape,
        GradientShape::Conic {
            center: Vec2::new(96.0, 56.0),
            start: 0.25
        }
    );
    let pos: Vec<f32> = conic.stops().iter().map(|s| s.pos).collect();
    assert_eq!(pos, [0.0, 0.25, 0.5]);
    // The linear layer: 40 px tiles from the padding box corner, over the border box, cut to it.
    let linear = gradient(b[2]);
    assert_eq!(linear.tile, Some(Vec2::new(40.0, 40.0)));
    assert_eq!(b[2].rect, ui2_draw::Rect::new(-36.0, -36.0, 236.0, 156.0));
    let clip = b[2].clip.expect("cut to the border box");
    assert_eq!(clip.rect, Some(ui2_draw::Rect::new(0.0, 0.0, 200.0, 120.0)));
    assert_eq!(clip.radius, 12.0);
    match linear.shape {
        GradientShape::Linear { from, to } => {
            assert!((from - Vec2::new(0.0, 20.0)).length() < 1e-3, "{from}");
            assert!((to - Vec2::new(40.0, 20.0)).length() < 1e-3, "{to}");
        }
        other => panic!("{other:?}"),
    }
    let pos: Vec<f32> = linear.stops().iter().map(|s| s.pos).collect();
    assert_eq!(pos, [0.0, 1.0]);
    // The border, then the outline 2 px out and 2 px wide (device), its radii grown as much.
    assert_eq!(b[3].border.top, 4.0);
    assert_eq!(b[4].rect, ui2_draw::Rect::new(-6.0, -6.0, 212.0, 132.0));
    assert_eq!(
        (b[4].border.left, b[4].border.color),
        (2.0, Color([1, 2, 3, 255]))
    );
    assert_eq!(b[4].radii.tl, 18.0);
}

#[test]
fn paint_single_positioned_tile_and_corner_direction() {
    let mut rig = rig();
    let items = rig.items("corner");
    let b = boxes(&items);
    assert_eq!(b.len(), 1, "{items:?}");
    // Bound size 10 px (20 device): one tile at x = 5 px, at the bottom; not tiled, inside the box.
    assert_eq!(b[0].rect, ui2_draw::Rect::new(10.0, 20.0, 20.0, 20.0));
    assert!(b[0].clip.is_none());
    let g = gradient(b[0]);
    assert!(g.tile.is_none() && !g.repeating);
    match g.shape {
        GradientShape::Linear { from, to } => {
            // `to top right` in a square tile is 45 degrees: from the bottom left to the top right.
            assert!((from - Vec2::new(0.0, 20.0)).length() < 1e-3, "{from}");
            assert!((to - Vec2::new(20.0, 0.0)).length() < 1e-3, "{to}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn paint_elliptic_corners_share_one_ratio() {
    let mut rig = rig();
    let items = rig.items("ellipse");
    match items.as_slice() {
        [Item::EllipticBox(e)] => {
            assert_eq!(e.ratio, 0.5);
            assert_eq!(e.base.radii.tl, 40.0);
            assert_eq!(e.base.fill, Fill::Solid(Color(RED)));
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn paint_color_pictures_and_game_pictures() {
    let mut rig = rig();
    // A picture of the package: the first color page (after the mask page), without the margin.
    match rig.items("picture").as_slice() {
        [Item::Image(img)] => {
            assert_eq!(img.image.layer, 0);
            assert_eq!(
                img.image.uv,
                [1.0 / 12.0, 1.0 / 8.0, 11.0 / 12.0, 7.0 / 8.0]
            );
            assert_eq!(img.tint, Color([255, 255, 255, 128]));
            assert_eq!(img.rect, ui2_draw::Rect::new(0.0, 0.0, 20.0, 12.0));
            assert_eq!(img.radii.tl, 4.0);
            assert!(img.external.is_none());
        }
        other => panic!("{other:?}"),
    }
    // A game asset: the texture the host's resolver names.
    match rig.items("portrait").as_slice() {
        [Item::Image(img)] => {
            let name = "ui:tx:portrait.dds";
            assert_eq!(img.external, Some(ExtTexture(1000 + name.len() as u64)));
            assert_eq!(img.image.uv, [0.0, 0.0, 0.5, 1.0]);
        }
        other => panic!("{other:?}"),
    }
    // The minimap is known without the resolver.
    match rig.items("minimap").as_slice() {
        [Item::Image(img)] => assert_eq!(img.external, Some(ExtTexture::MINIMAP)),
        other => panic!("{other:?}"),
    }
    assert!(!rig.asked.borrow().iter().any(|n| n == "render:minimap"));
    // A bound picture follows its expression: one chunk is built again when it changes.
    let gold = rig.sample["model"]["top"]["gold"].to_string();
    assert!(rig.asked.borrow().contains(&gold), "{:?}", rig.asked);
    rig.sample["model"]["top"]["gold"] = json!(4321);
    let st = rig.push();
    assert_eq!((st.chunks_rebuilt, st.layout_runs), (1, 0));
    match rig.items("bound").as_slice() {
        [Item::Image(img)] => assert_eq!(img.external, Some(ExtTexture(1004))),
        other => panic!("{other:?}"),
    }
}

#[test]
fn paint_idle_frames_do_nothing_and_a_style_change_builds_one_chunk() {
    let mut rig = rig();
    rig.scene.take_layer_change(LayerId::Hud);
    rig.scene.reset_frame_stats();
    for _ in 0..50 {
        let st = rig.tick();
        assert_eq!(
            (
                st.chunks_rebuilt,
                st.layout_runs,
                st.binds_evaluated,
                st.visited
            ),
            (0, 0, 0, 0)
        );
    }
    assert_eq!(rig.scene.stats().chunks_encoded, 0);
    assert_eq!(rig.scene.take_layer_change(LayerId::Hud), LayerChange::None);
    let before = rig.items("corner");
    // The bound background size changes the gradient of one chunk only, without layout.
    rig.sample["model"]["top"]["happiness"] = json!(5);
    let st = rig.push();
    assert_eq!((st.chunks_rebuilt, st.layout_runs), (1, 0));
    assert_eq!(rig.scene.stats().chunks_encoded, 1);
    let after = rig.items("corner");
    assert_ne!(before, after);
    assert_eq!(boxes(&after)[0].rect.w, 10.0);
}
