//! Pictures of the package in styles: `background-image: url()` layers (found by their path, sized, placed,
//! repeated, mixed with gradient layers whose per-layer lists keep their places) and `border-image` (a 9-slice
//! instead of the border; a missing picture leaves the border).

use ui2_draw::{Fill, ImageItem, Item, Scene};
use ui2_model::{ModelStore, Schema};
use ui2_package::style::{
    BgBox, BgImage, BgRepeat, BgSize, BorderStyle, ColorV, Display, GradDir, Gradient, ImageSlice,
    ImageWidth, Len, LenAuto, Position, Repeat, Stop, StyleRecord,
};
use ui2_package::{Layer, StrId};
use ui2_runtime::testkit::{Kit, TNode};
use ui2_runtime::{Env, Runtime, Services, UiError};
use ui2_text::TextEngine;

const RED: [u8; 4] = [255, 0, 0, 255];

fn block(x: f32, w: f32, h: f32) -> StyleRecord {
    let mut r = StyleRecord::initial();
    r.display = Display::Block;
    r.position = Position::Absolute;
    r.left = LenAuto::Len(Len::px(x));
    r.top = LenAuto::Len(Len::ZERO);
    r.width = LenAuto::Len(Len::px(w));
    r.height = LenAuto::Len(Len::px(h));
    r.box_sizing = ui2_package::style::BoxSizing::BorderBox;
    r
}

fn bordered(mut r: StyleRecord, w: f32) -> StyleRecord {
    for (width, style, color) in [
        (
            &mut r.border_top_width,
            &mut r.border_top_style,
            &mut r.border_top_color,
        ),
        (
            &mut r.border_right_width,
            &mut r.border_right_style,
            &mut r.border_right_color,
        ),
        (
            &mut r.border_bottom_width,
            &mut r.border_bottom_style,
            &mut r.border_bottom_color,
        ),
        (
            &mut r.border_left_width,
            &mut r.border_left_style,
            &mut r.border_left_color,
        ),
    ] {
        *width = Len::px(w);
        *style = BorderStyle::Solid;
        *color = ColorV::Lit(RED);
    }
    r
}

struct Rig {
    rt: Runtime,
    store: ModelStore,
    engine: TextEngine,
    scene: Scene,
}

impl Rig {
    fn tick(&mut self) -> (ui2_runtime::FrameStats, Vec<UiError>) {
        let out = self
            .rt
            .frame(0.0, &self.store, &mut self.engine, &mut self.scene);
        (out.stats, out.errors)
    }

    fn items(&mut self, part: &str) -> Vec<Item> {
        let id = self.rt.find_part(part).expect("part");
        let chunk = self.rt.node(id).expect("node").chunk();
        self.rt.chunk_items(chunk).expect("chunk").1.items
    }
}

fn images(items: &[Item]) -> Vec<&ImageItem> {
    items
        .iter()
        .filter_map(|i| match i {
            Item::Image(img) => Some(img),
            _ => None,
        })
        .collect()
}

/// At device scale 2: a mask page first (so the color pictures are on the first `Rgba8` page, the second
/// page of the package), then a 10 x 6 tile and a 30 x 30 frame picture.
fn rig() -> (Rig, Vec<UiError>) {
    let mut kit = Kit::new();
    kit.mask_icons(&[("icons/a", [4, 4])]);
    kit.color_pictures(&[("art/tile.png", [10, 6]), ("art/frame.png", [30, 30])]);
    let tile = kit.str("art/tile.png");
    let frame = kit.str("art/frame.png");
    let missing = kit.str("art/missing.png");

    let mut layer = StyleRecord::initial();
    layer.display = Display::Block;
    // A gradient on top of a picture layer: the picture is the second layer and takes the second entry of
    // every list (20 px wide, `auto` high, repeated across only, cut to the padding box).
    let mut bg = block(0.0, 100.0, 60.0);
    bg.background_image = vec![
        BgImage::Gradient(Gradient {
            conic: false,
            repeating: false,
            dir: GradDir::Angle(90.0),
            stops: vec![
                Stop {
                    color: ColorV::Lit(RED),
                    pos: None,
                },
                Stop {
                    color: ColorV::Lit([0, 0, 255, 128]),
                    pos: None,
                },
            ],
        }),
        BgImage::Url(tile),
    ];
    bg.background_size = vec![
        BgSize::Size(LenAuto::Auto, LenAuto::Auto),
        BgSize::Size(LenAuto::Len(Len::px(20.0)), LenAuto::Auto),
    ];
    bg.background_repeat = vec![
        BgRepeat::REPEAT,
        BgRepeat {
            x: Repeat::Repeat,
            y: Repeat::NoRepeat,
        },
    ];
    bg.background_clip = vec![BgBox::BorderBox, BgBox::PaddingBox];
    // A single picture, `contain`ed and centered.
    let mut single = block(200.0, 40.0, 20.0);
    single.background_image = vec![BgImage::Url(frame)];
    single.background_size = vec![BgSize::Contain];
    single.background_position_x = vec![Len::pct(50.0)];
    single.background_position_y = vec![Len::pct(50.0)];
    single.background_repeat = vec![BgRepeat {
        x: Repeat::NoRepeat,
        y: Repeat::NoRepeat,
    }];
    // A border image over a 5 px border: slices of 10 picture px, widths of one border width, the middle too.
    let mut framed = bordered(block(300.0, 60.0, 40.0), 5.0);
    framed.background_color = ColorV::Lit([1, 2, 3, 255]);
    framed.border_image_source = Some(frame);
    framed.border_image_slice = ImageSlice {
        edges: [Len::px(10.0); 4],
        fill: true,
    };
    framed.border_image_width = [ImageWidth::Factor(1.0); 4];
    // A border image whose picture the package lacks: the border stays.
    let mut lost = bordered(block(400.0, 20.0, 20.0), 2.0);
    lost.border_image_source = Some(missing);

    let mut sets = Vec::new();
    for r in [layer, bg, single, framed, lost] {
        let id = kit.record(r);
        sets.push(kit.plain(id));
    }
    let names: Vec<StrId> = ["bg", "single", "framed", "lost"]
        .iter()
        .map(|n| kit.str(n))
        .collect();
    kit.template(
        "doc",
        vec![
            TNode::layer(Layer::Hud, sets[0])
                .children((0..4).map(|i| TNode::el(sets[i + 1]).part(names[i]))),
        ],
    );
    let pkg = kit.finish().expect("package");
    let env = Env {
        viewport_px: (1600, 600),
        dpr: 1.0,
        ui_scale: 2.0,
        theme: 0,
    };
    let mut rig = Rig {
        rt: Runtime::new(pkg, env, Services::default()).expect("runtime"),
        store: ModelStore::new(Schema::v1()),
        engine: TextEngine::new(),
        scene: Scene::new(1.0),
    };
    let (_, errors) = rig.tick();
    (rig, errors)
}

#[test]
fn background_picture_layers_keep_their_place_among_gradients() {
    let (mut rig, _) = rig();
    let items = rig.items("bg");
    // The bottom layer (the picture) first: five tiles across, then the gradient over the border box.
    let kinds: Vec<&str> = items
        .iter()
        .map(|i| match i {
            Item::Image(_) => "picture",
            Item::Box(b) if matches!(b.fill, Fill::Gradient(_)) => "gradient",
            _ => "other",
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "picture", "picture", "picture", "picture", "picture", "gradient"
        ]
    );
    let tiles = images(&items);
    // 20 CSS px wide and, keeping the 10 x 6 ratio, 12 high: 40 x 24 device px from the corner.
    for (i, t) in tiles.iter().enumerate() {
        assert_eq!(
            t.rect,
            ui2_draw::Rect::new(40.0 * i as f32, 0.0, 40.0, 24.0)
        );
        assert!(t.nine.is_none() && t.external.is_none());
        assert_eq!(t.tint, ui2_draw::Color([255, 255, 255, 255]));
    }
    assert!(
        tiles[0].clip.is_none(),
        "the tiles fill the clip box exactly"
    );
    // The tile is the first sprite of the first color page: 12 x 8 with its margin, on a 44 px page.
    assert_eq!(tiles[0].image.layer, 0);
    assert_eq!(
        tiles[0].image.uv,
        [1.0 / 44.0, 1.0 / 32.0, 11.0 / 44.0, 7.0 / 32.0]
    );
}

#[test]
fn a_contained_picture_is_centered_once() {
    let (mut rig, _) = rig();
    let items = rig.items("single");
    let pics = images(&items);
    assert_eq!(pics.len(), 1, "{items:?}");
    // 30 x 30 in a 40 x 20 box, contained: 20 x 20, centered across (device px, scale 2).
    assert_eq!(pics[0].rect, ui2_draw::Rect::new(420.0, 0.0, 40.0, 40.0));
    // The second picture of the page: x 12..44 with its margin.
    assert_eq!(
        pics[0].image.uv,
        [13.0 / 44.0, 1.0 / 32.0, 43.0 / 44.0, 31.0 / 32.0]
    );
}

#[test]
fn a_border_image_replaces_the_border_with_a_nine_slice() {
    let (mut rig, _) = rig();
    let items = rig.items("framed");
    let borders = items
        .iter()
        .filter(|i| matches!(i, Item::Box(b) if b.border.top > 0.0 || b.border.left > 0.0));
    assert_eq!(borders.count(), 0, "no border box: {items:?}");
    let pics = images(&items);
    assert_eq!(pics.len(), 1);
    let nine = pics[0].nine.expect("a 9-slice");
    assert_eq!(pics[0].rect, ui2_draw::Rect::new(600.0, 0.0, 120.0, 80.0));
    let third = 10.0 / 30.0;
    assert_eq!(nine.src, [third; 4]);
    assert_eq!(nine.dst, [10.0; 4], "one border width (5 px) at scale 2");
    assert!(nine.fill);
    // The background is still painted, under the picture.
    assert!(
        matches!(items.first(), Some(Item::Box(b)) if b.fill == Fill::Solid(ui2_draw::Color([1, 2, 3, 255])))
    );
}

#[test]
fn a_missing_border_image_keeps_the_border_and_is_reported_once() {
    let (mut rig, errors) = rig();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].msg.contains("art/missing.png"),
        "{}",
        errors[0].msg
    );
    let items = rig.items("lost");
    assert!(images(&items).is_empty());
    assert!(matches!(items.as_slice(), [Item::Box(b)] if b.border.top == 4.0));
    // Idle frames: nothing is built or reported again.
    for _ in 0..20 {
        let (st, errors) = rig.tick();
        assert!(errors.is_empty());
        assert_eq!((st.chunks_rebuilt, st.layout_runs, st.visited), (0, 0, 0));
    }
}
