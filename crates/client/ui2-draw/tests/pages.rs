//! Picture pages of a skin package in the scene: registration, revision, layers, and the encoding of masks and
//! pictures that refer to them (no GPU).

use std::path::PathBuf;
use std::sync::Arc;

use ui2_draw::instance::{KIND_IMAGE, KIND_MASK, f16_bits};
use ui2_draw::{
    ChunkBuilder, ChunkId, ChunkItems, Color, DrawError, FaceId, Glyphs, ImageItem, ImageRef, Item,
    LayerChange, LayerId, MaskItem, Radii, Rect, Scene,
};
use ui2_package::{BlobRef, BlobSection, Images, Page, PageFormat};
use ui2_text::{RunStyle, TextEngine};

const WHITE: Color = Color::rgba(255, 255, 255, 255);
const FIRA: FaceId = FaceId(0);

fn page(format: PageFormat, scale: u8, size: [u16; 2]) -> Page {
    Page {
        format,
        scale,
        size,
        blob: BlobRef {
            section: BlobSection::Imgs,
            offset: 0,
            len: 0,
            raw_len: 0,
            zstd: false,
        },
        blake3: [0; 32],
    }
}

fn pixels(p: &Page, fill: u8) -> Arc<[u8]> {
    let n = usize::from(p.size[0]) * usize::from(p.size[1]) * p.format.bytes_per_pixel() as usize;
    Arc::from(vec![fill; n])
}

/// Package pages: mask 256 x 128, colour 512 x 256, mask 64 x 64 (2x), colour 128 x 128.
fn table() -> (Images, Vec<Arc<[u8]>>) {
    let pages = vec![
        page(PageFormat::A8, 1, [256, 128]),
        page(PageFormat::Rgba8, 1, [512, 256]),
        page(PageFormat::A8, 2, [64, 64]),
        page(PageFormat::Rgba8, 1, [128, 128]),
    ];
    let px = pages
        .iter()
        .enumerate()
        .map(|(i, p)| pixels(p, i as u8 + 1))
        .collect();
    (
        Images {
            pages,
            imgs: Vec::new(),
        },
        px,
    )
}

fn one(item: Item) -> ChunkItems {
    let mut b = ChunkBuilder::new();
    b.push(item);
    b.build()
}

fn mask_item(layer: u8, uv: [f32; 4]) -> Item {
    Item::Mask(MaskItem {
        rect: Rect::new(2.0, 3.0, 16.0, 16.0),
        mask: ImageRef { layer, uv },
        color: Color::rgba(200, 180, 120, 255),
        ..MaskItem::default()
    })
}

fn image_item(layer: u8, uv: [f32; 4]) -> Item {
    Item::Image(ImageItem {
        rect: Rect::new(10.0, 20.0, 64.0, 32.0),
        image: ImageRef { layer, uv },
        tint: Color::rgba(255, 255, 255, 128),
        radii: Radii::uniform(4.0),
        ..ImageItem::default()
    })
}

#[test]
fn pages_are_split_by_format_in_package_order() {
    let mut s = Scene::new(1.0);
    assert_eq!(s.pages().revision(), 0);
    let (images, px) = table();
    s.set_pages(&images, px.clone()).expect("pages");
    let p = s.pages();
    assert_eq!(p.revision(), 1);
    let mask: Vec<_> = p
        .mask()
        .iter()
        .map(|m| (m.package_page, m.size, m.scale))
        .collect();
    assert_eq!(mask, vec![(0, [256, 128], 1), (2, [64, 64], 2)]);
    let image: Vec<_> = p.image().iter().map(|m| (m.package_page, m.size)).collect();
    assert_eq!(image, vec![(1, [512, 256]), (3, [128, 128])]);
    assert!(Arc::ptr_eq(&p.mask()[1].pixels, &px[2]));
    assert_eq!(p.image()[0].pixels[0], 2);
    assert_eq!(p.image_side(), 512);
    assert_eq!(p.mask_edge(), 256);
    assert_eq!(s.mask_side(), 2048, "the glyph page size is larger");
    assert_eq!(
        s.glyphs().layer_base(),
        2,
        "glyph pages follow the two mask pages"
    );
    let st = s.stats();
    assert_eq!((st.mask_pages, st.image_pages), (2, 2));
    assert_eq!(
        st.page_bytes,
        256 * 128 + 512 * 256 * 4 + 64 * 64 + 128 * 128 * 4
    );
}

#[test]
fn equal_pages_keep_the_revision_and_the_chunks() {
    let mut s = Scene::new(1.0);
    let (images, px) = table();
    s.set_pages(&images, px.clone()).expect("pages");
    s.set_chunk(
        LayerId::Hud,
        ChunkId(1),
        one(mask_item(0, [0.0, 0.0, 0.5, 0.5])),
    );
    s.take_layer_change(LayerId::Hud);
    s.reset_frame_stats();
    let rev = s.layer_revision(LayerId::Hud);

    // The same buffers, then equal copies: nothing changes, nothing is encoded.
    s.set_pages(&images, px.clone()).expect("same");
    let copies: Vec<Arc<[u8]>> = px.iter().map(|p| Arc::from(p.to_vec())).collect();
    s.set_pages(&images, copies).expect("copies");
    assert_eq!(s.pages().revision(), 1);
    assert_eq!(s.layer_revision(LayerId::Hud), rev);
    assert_eq!(s.take_layer_change(LayerId::Hud), LayerChange::None);
    assert_eq!(s.stats().chunks_encoded, 0);

    // Other pixels are a change; an empty table drops every page.
    let mut other = px.clone();
    other[0] = pixels(&images.pages[0], 9);
    s.set_pages(&images, other).expect("other");
    assert_eq!(s.pages().revision(), 2);
    s.set_pages(&Images::default(), Vec::new()).expect("none");
    assert_eq!(s.pages().revision(), 3);
    assert!(s.pages().mask().is_empty() && s.pages().image().is_empty());
    assert_eq!(s.glyphs().layer_base(), 0);
    assert_eq!(s.stats().page_bytes, 0);
}

#[test]
fn a_page_table_that_does_not_match_is_refused_and_changes_nothing() {
    let mut s = Scene::new(1.0);
    let (images, px) = table();
    s.set_pages(&images, px.clone()).expect("pages");

    let mut short = px.clone();
    short.pop();
    assert!(matches!(
        s.set_pages(&images, short),
        Err(DrawError::InvalidPage(_))
    ));
    let mut wrong_len = px.clone();
    wrong_len[1] = Arc::from(vec![0u8; 10]);
    assert!(matches!(
        s.set_pages(&images, wrong_len),
        Err(DrawError::InvalidPage(_))
    ));
    let big = Images {
        pages: vec![page(PageFormat::A8, 1, [2048, 16])],
        imgs: Vec::new(),
    };
    let big_px = vec![pixels(&big.pages[0], 0)];
    assert!(matches!(
        s.set_pages(&big, big_px),
        Err(DrawError::InvalidPage(_))
    ));
    let empty = Images {
        pages: vec![page(PageFormat::Rgba8, 1, [0, 16])],
        imgs: Vec::new(),
    };
    assert!(matches!(
        s.set_pages(&empty, vec![Arc::from(Vec::new())]),
        Err(DrawError::InvalidPage(_))
    ));
    assert_eq!(s.pages().revision(), 1);
    assert_eq!(s.pages().mask().len(), 2);
}

#[test]
fn mask_sprites_are_mapped_from_page_to_array_coordinates() {
    let mut s = Scene::new(1.0);
    let (images, px) = table();
    s.set_pages(&images, px).expect("pages");
    // Page 0 is 256 x 128 in a 2048 square layer.
    s.set_chunk(
        LayerId::Hud,
        ChunkId(1),
        one(mask_item(0, [0.5, 0.25, 1.0, 0.5])),
    );
    let i = s.chunk_instances(ChunkId(1)).unwrap()[0];
    assert_eq!(i.kind, KIND_MASK);
    assert_eq!(i.atlas_layer, 0);
    assert_eq!(i.uv, [0.0625, 0.015625, 0.125, 0.03125]);
    assert_eq!(i.colors[0], [200, 180, 120, 255]);
    // Page 1 of the masks (the package's page 2) is 64 x 64.
    s.set_chunk(
        LayerId::Hud,
        ChunkId(2),
        one(mask_item(1, [0.0, 0.0, 1.0, 1.0])),
    );
    let i = s.chunk_instances(ChunkId(2)).unwrap()[0];
    assert_eq!((i.atlas_layer, i.uv), (1, [0.0, 0.0, 0.03125, 0.03125]));
    // A layer past the mask pages is a glyph page: used as given.
    s.set_chunk(
        LayerId::Hud,
        ChunkId(3),
        one(mask_item(5, [0.1, 0.2, 0.3, 0.4])),
    );
    let i = s.chunk_instances(ChunkId(3)).unwrap()[0];
    assert_eq!((i.atlas_layer, i.uv), (5, [0.1, 0.2, 0.3, 0.4]));
}

#[test]
fn pictures_encode_as_image_instances_of_their_page() {
    let mut s = Scene::new(1.0);
    // Before the pages arrive a picture makes no instance.
    s.set_chunk(
        LayerId::Window,
        ChunkId(1),
        one(image_item(1, [0.0, 0.0, 1.0, 1.0])),
    );
    assert_eq!(s.chunk_instances(ChunkId(1)).unwrap().len(), 0);

    let (images, px) = table();
    s.set_pages(&images, px).expect("pages");
    // The chunk was encoded again with the pages: the layer was rebuilt with one instance.
    assert_eq!(s.take_layer_change(LayerId::Window), LayerChange::Full);
    let i = s.chunk_instances(ChunkId(1)).unwrap()[0];
    assert_eq!(i.kind, KIND_IMAGE);
    assert_eq!(i.atlas_layer, 1);
    // Colour page 1 is 128 x 128 in a 512 square layer.
    assert_eq!(i.uv, [0.0, 0.0, 0.25, 0.25]);
    assert_eq!(i.rect, [10.0, 20.0, 64.0, 32.0]);
    assert_eq!(i.colors[0], [255, 255, 255, 128]);
    assert_eq!(i.radii, [4.0; 4].map(f16_bits));
    assert_eq!((i.flags, i.ext, i.anchor), (0, 0, 0));

    // Colour page 0 is the largest (512 x 256): its sprites keep their u.
    s.set_chunk(
        LayerId::Window,
        ChunkId(2),
        one(image_item(0, [0.5, 0.5, 1.0, 1.0])),
    );
    let i = s.chunk_instances(ChunkId(2)).unwrap()[0];
    assert_eq!((i.atlas_layer, i.uv), (0, [0.5, 0.25, 1.0, 0.5]));

    // No such page, a transparent tint, an empty rectangle: no instance.
    for item in [
        image_item(2, [0.0, 0.0, 1.0, 1.0]),
        Item::Image(ImageItem {
            tint: Color::TRANSPARENT,
            ..ImageItem {
                rect: Rect::new(0.0, 0.0, 8.0, 8.0),
                ..ImageItem::default()
            }
        }),
        Item::Image(ImageItem::default()),
    ] {
        s.set_chunk(LayerId::Window, ChunkId(3), one(item));
        assert_eq!(s.chunk_instances(ChunkId(3)).unwrap().len(), 0);
    }
}

fn engine() -> TextEngine {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../third_party/fonts/fira-sans/FiraSans-Regular.ttf");
    let bytes =
        std::fs::read(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let mut e = TextEngine::new();
    e.load_face(FIRA, Arc::from(bytes)).expect("Fira Sans");
    e
}

#[test]
fn glyph_layers_move_after_the_mask_pages_and_idle_calls_do_nothing() {
    let mut text = engine();
    let run = text
        .shape(&RunStyle::new(FIRA, 14.0), "Gold")
        .expect("shape");
    let mut s = Scene::new(1.0);
    s.set_chunk(
        LayerId::Hud,
        ChunkId(1),
        one(Item::Glyphs(Glyphs::from_run(&run, 1.0, 4.0, 20.0, WHITE))),
    );
    s.set_chunk(
        LayerId::Hud,
        ChunkId(2),
        one(Item::Box(ui2_draw::BoxItem::solid(
            Rect::new(0.0, 0.0, 4.0, 4.0),
            WHITE,
        ))),
    );
    s.resolve(&mut text, 1000.0);
    let drawn = |s: &Scene| -> Vec<u8> {
        s.chunk_instances(ChunkId(1))
            .unwrap()
            .iter()
            .filter(|i| i.rect[2] > 0.0)
            .map(|i| i.atlas_layer)
            .collect()
    };
    assert!(!drawn(&s).is_empty());
    assert!(drawn(&s).iter().all(|l| *l == 0));
    let box_rev = s.chunk_revision(ChunkId(2));

    let (images, px) = table();
    s.set_pages(&images, px).expect("pages");
    assert!(drawn(&s).iter().all(|l| *l == 2), "{:?}", drawn(&s));
    assert_eq!(
        s.chunk_revision(ChunkId(2)),
        box_rev,
        "a box chunk is not encoded again"
    );

    // An idle frame: no change to upload.
    s.take_layer_change(LayerId::Hud);
    let rev = s.layer_revision(LayerId::Hud);
    s.resolve(&mut text, 1000.0);
    assert_eq!(s.layer_revision(LayerId::Hud), rev);
    assert_eq!(s.take_layer_change(LayerId::Hud), LayerChange::None);
}
