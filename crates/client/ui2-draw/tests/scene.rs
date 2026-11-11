//! Chunks, layers, revisions, change records and anchors of the scene (no GPU).

use ui2_draw::{
    Affine2, AnchorXform, BoxItem, ChunkBuilder, ChunkId, ChunkItems, Clip, Color, ExtTexture,
    Fill, ImageItem, Item, LayerChange, LayerId, Rect, Scene, Vec2,
};

fn one(start: usize, end: usize) -> Vec<std::ops::Range<usize>> {
    std::iter::once(start..end).collect()
}

fn boxes(n: usize, color: u8) -> ChunkItems {
    let mut b = ChunkBuilder::new();
    for i in 0..n {
        b.push(Item::Box(BoxItem::solid(
            Rect::new(i as f32 * 12.0, 0.0, 10.0, 10.0),
            Color::rgba(color, 0, 0, 255),
        )));
    }
    b.build()
}

fn color_of(s: &Scene, id: ChunkId, i: usize) -> u8 {
    s.chunk_instances(id).unwrap()[i].colors[0][0]
}

#[test]
fn unchanged_chunk_changes_nothing() {
    let mut s = Scene::new(1.0);
    s.set_chunk(LayerId::Hud, ChunkId(1), boxes(3, 10));
    s.set_chunk(LayerId::Hud, ChunkId(2), boxes(2, 20));
    assert_eq!(s.take_layer_change(LayerId::Hud), LayerChange::Full);
    let rev = s.layer_revision(LayerId::Hud);
    let crev = s.chunk_revision(ChunkId(1)).unwrap();
    let encoded = s.stats().chunks_encoded;

    s.set_chunk(LayerId::Hud, ChunkId(1), boxes(3, 10));
    assert_eq!(s.layer_revision(LayerId::Hud), rev);
    assert_eq!(s.chunk_revision(ChunkId(1)), Some(crev));
    assert_eq!(s.take_layer_change(LayerId::Hud), LayerChange::None);
    assert_eq!(
        s.stats().chunks_encoded,
        encoded,
        "equal items are not encoded again"
    );
}

#[test]
fn same_count_change_patches_only_the_chunk_range() {
    let mut s = Scene::new(1.0);
    s.set_chunk(LayerId::Hud, ChunkId(1), boxes(3, 10));
    s.set_chunk(LayerId::Hud, ChunkId(2), boxes(2, 20));
    s.set_chunk(LayerId::Hud, ChunkId(3), boxes(4, 30));
    s.take_layer_change(LayerId::Hud);
    let rev = s.layer_revision(LayerId::Hud);

    s.set_chunk(LayerId::Hud, ChunkId(2), boxes(2, 99));
    assert_eq!(s.layer_revision(LayerId::Hud), rev + 1);
    assert_eq!(
        s.take_layer_change(LayerId::Hud),
        LayerChange::Ranges(one(3, 5))
    );
    assert_eq!(s.layer_instances(LayerId::Hud).len(), 9);
    assert_eq!(color_of(&s, ChunkId(2), 0), 99);
    assert_eq!(color_of(&s, ChunkId(1), 0), 10);
    assert_eq!(color_of(&s, ChunkId(3), 3), 30);
    assert_eq!(s.take_layer_change(LayerId::Hud), LayerChange::None);

    // Two neighbouring changes merge into one range; a gap keeps two.
    s.set_chunk(LayerId::Hud, ChunkId(1), boxes(3, 1));
    s.set_chunk(LayerId::Hud, ChunkId(2), boxes(2, 2));
    s.set_chunk(LayerId::Hud, ChunkId(3), boxes(4, 3));
    assert_eq!(
        s.take_layer_change(LayerId::Hud),
        LayerChange::Ranges(one(0, 9))
    );
    s.set_chunk(LayerId::Hud, ChunkId(1), boxes(3, 5));
    s.set_chunk(LayerId::Hud, ChunkId(3), boxes(4, 6));
    assert_eq!(
        s.take_layer_change(LayerId::Hud),
        LayerChange::Ranges(vec![0..3, 5..9])
    );
}

#[test]
fn a_change_that_encodes_to_the_same_bytes_is_not_a_change() {
    let mut s = Scene::new(1.0);
    let mut a = boxes(2, 10);
    s.set_chunk(LayerId::Hud, ChunkId(1), a.clone());
    s.take_layer_change(LayerId::Hud);
    let rev = s.layer_revision(LayerId::Hud);
    // A fully transparent extra box encodes to nothing: items differ, instances do not.
    if let Item::Box(b) = &mut a.items[0] {
        b.border.top = 3.0; // transparent border colour: no border drawn
    }
    s.set_chunk(LayerId::Hud, ChunkId(1), a);
    assert_eq!(s.layer_revision(LayerId::Hud), rev);
    assert_eq!(s.take_layer_change(LayerId::Hud), LayerChange::None);
}

#[test]
fn count_change_rebuilds_the_layer_in_order() {
    let mut s = Scene::new(1.0);
    s.set_chunk(LayerId::Hud, ChunkId(1), boxes(1, 1));
    s.set_chunk(LayerId::Hud, ChunkId(2), boxes(2, 2));
    s.set_chunk(LayerId::Hud, ChunkId(3), boxes(1, 3));
    s.take_layer_change(LayerId::Hud);

    s.set_chunk(LayerId::Hud, ChunkId(2), boxes(4, 22));
    assert_eq!(s.take_layer_change(LayerId::Hud), LayerChange::Full);
    assert_eq!(s.chunk_range(ChunkId(1)), Some(0..1));
    assert_eq!(s.chunk_range(ChunkId(2)), Some(1..5));
    assert_eq!(s.chunk_range(ChunkId(3)), Some(5..6));
    let colors: Vec<u8> = s
        .layer_instances(LayerId::Hud)
        .iter()
        .map(|i| i.colors[0][0])
        .collect();
    assert_eq!(colors, vec![1, 22, 22, 22, 22, 3]);

    s.remove_chunk(ChunkId(2));
    assert_eq!(s.take_layer_change(LayerId::Hud), LayerChange::Full);
    assert_eq!(s.chunk_range(ChunkId(3)), Some(1..2));
    assert!(s.chunk_instances(ChunkId(2)).is_none());
    let colors: Vec<u8> = s
        .layer_instances(LayerId::Hud)
        .iter()
        .map(|i| i.colors[0][0])
        .collect();
    assert_eq!(colors, vec![1, 3]);

    // A chunk with no instances neither changes the buffer nor the revision.
    let rev = s.layer_revision(LayerId::Hud);
    s.set_chunk(LayerId::Hud, ChunkId(9), ChunkItems::default());
    s.remove_chunk(ChunkId(9));
    assert_eq!(s.layer_revision(LayerId::Hud), rev);
    assert_eq!(s.take_layer_change(LayerId::Hud), LayerChange::None);
}

#[test]
fn layers_are_independent_and_chunks_move_between_them() {
    let mut s = Scene::new(1.0);
    s.set_chunk(LayerId::Hud, ChunkId(1), boxes(2, 1));
    s.set_chunk(LayerId::Window, ChunkId(2), boxes(1, 2));
    s.take_layer_change(LayerId::Hud);
    s.take_layer_change(LayerId::Window);
    let hud_rev = s.layer_revision(LayerId::Hud);

    s.set_chunk(LayerId::Window, ChunkId(2), boxes(1, 7));
    assert_eq!(s.layer_revision(LayerId::Hud), hud_rev);
    assert_eq!(s.take_layer_change(LayerId::Hud), LayerChange::None);

    s.set_chunk(LayerId::Tooltip, ChunkId(1), boxes(2, 1));
    assert_eq!(s.layer_instances(LayerId::Hud).len(), 0);
    assert_eq!(s.layer_instances(LayerId::Tooltip).len(), 2);
    assert_eq!(s.stats().chunks, 2);
    assert_eq!(s.stats().instances, 3);
}

#[test]
fn ops_cover_the_layer() {
    let mut s = Scene::new(1.0);
    assert!(s.layer_ops(LayerId::Hud).is_empty());
    s.set_chunk(LayerId::Hud, ChunkId(1), boxes(3, 1));
    s.set_chunk(LayerId::Hud, ChunkId(2), boxes(2, 1));
    let ops = s.layer_ops(LayerId::Hud).to_vec();
    assert_eq!(ops.len(), 1);
    assert_eq!((ops[0].range.clone(), ops[0].ext), (0..5, 0));
    s.remove_chunk(ChunkId(1));
    s.remove_chunk(ChunkId(2));
    assert!(s.layer_ops(LayerId::Hud).is_empty());
}

#[test]
fn table_entries_follow_the_chunks_that_use_them() {
    let mut s = Scene::new(1.0);
    let item = |angle: f32| {
        Item::Box(BoxItem {
            rect: Rect::new(0.0, 0.0, 10.0, 10.0),
            fill: Fill::Solid(Color::rgba(1, 2, 3, 255)),
            transform: Some(Affine2::from_angle(angle)),
            clip: Some(Clip::rect(Rect::new(0.0, 0.0, 5.0, 5.0))),
            ..BoxItem::default()
        })
    };
    let mut a = ChunkBuilder::new();
    a.push(item(0.1)).push(item(0.2));
    s.set_chunk(LayerId::Hud, ChunkId(1), a.build());
    let mut b = ChunkBuilder::new();
    b.push(item(0.1));
    s.set_chunk(LayerId::Hud, ChunkId(2), b.build());
    assert_eq!((s.stats().xforms, s.stats().clips), (2, 1));

    // Replacing chunk 1 keeps the record that both versions use and frees the other.
    let mut a2 = ChunkBuilder::new();
    a2.push(item(0.1)).push(item(0.3));
    s.set_chunk(LayerId::Hud, ChunkId(1), a2.build());
    assert_eq!((s.stats().xforms, s.stats().clips), (2, 1));
    s.remove_chunk(ChunkId(1));
    assert_eq!((s.stats().xforms, s.stats().clips), (1, 1));
    s.remove_chunk(ChunkId(2));
    assert_eq!((s.stats().xforms, s.stats().clips), (0, 0));
    assert_eq!(s.stats().table_overflow, 0);
}

#[test]
fn camera_motion_writes_only_the_anchor_table() {
    let mut s = Scene::new(1.0);
    let slots: Vec<_> = (0..200).map(|_| s.alloc_anchor().unwrap()).collect();
    for (i, slot) in slots.iter().enumerate() {
        let mut b = ChunkBuilder::new().anchor(*slot);
        b.push(Item::Box(BoxItem::solid(
            Rect::new(-20.0, -10.0, 40.0, 20.0),
            Color::rgba(5, 5, 5, 255),
        )));
        s.set_chunk(LayerId::Map, ChunkId(i as u32), b.build());
    }
    for slot in &slots {
        s.set_anchor(
            *slot,
            AnchorXform {
                pos: Vec2::ZERO,
                scale: 1.0,
                visible: true,
            },
        );
    }
    assert_eq!(s.layer_instances(LayerId::Map).len(), 200);
    assert_eq!(s.layer_instances(LayerId::Map)[17].anchor, slots[17].0);
    s.take_layer_change(LayerId::Map);
    s.take_table_uploads();
    s.reset_frame_stats();
    let rev = s.layer_revision(LayerId::Map);

    // The camera moves: every anchor gets a new screen position.
    for (i, slot) in slots.iter().enumerate() {
        s.set_anchor(
            *slot,
            AnchorXform {
                pos: Vec2::new(i as f32, 100.0),
                scale: 1.0,
                visible: i % 3 != 0,
            },
        );
    }
    assert_eq!(s.layer_revision(LayerId::Map), rev);
    assert_eq!(s.take_layer_change(LayerId::Map), LayerChange::None);
    let bytes: usize = s.take_table_uploads().iter().map(|g| g.bytes().len()).sum();
    assert_eq!(bytes, 200 * 32);
    assert_eq!(s.stats().anchor_bytes, 200 * 32);
    assert_eq!(s.stats().chunks_encoded, 0);
    assert!(s.take_glyph_dirty().is_empty());
}

#[test]
fn scale_change_keeps_non_glyph_chunks() {
    let mut s = Scene::new(1.0);
    s.set_chunk(LayerId::Hud, ChunkId(1), boxes(2, 1));
    s.take_layer_change(LayerId::Hud);
    s.set_scale(2.0);
    s.reencode_stale();
    assert_eq!(s.scale(), 2.0);
    assert_eq!(s.take_layer_change(LayerId::Hud), LayerChange::None);
}

#[test]
fn external_textures_split_ops_and_free_their_slots_with_the_chunk() {
    let picture = |x: f32, tex: u64| {
        Item::Image(ImageItem {
            rect: Rect::new(x, 0.0, 10.0, 10.0),
            external: Some(ExtTexture(tex)),
            ..ImageItem::default()
        })
    };
    let solid = |x: f32| {
        Item::Box(BoxItem::solid(
            Rect::new(x, 0.0, 10.0, 10.0),
            Color::rgba(1, 2, 3, 255),
        ))
    };
    let mut s = Scene::new(1.0);
    let mut b = ChunkBuilder::new();
    for item in [
        solid(0.0),
        picture(10.0, 7),
        picture(20.0, 7),
        solid(30.0),
        solid(40.0),
        picture(50.0, 8),
    ] {
        b.push(item);
    }
    s.set_chunk(LayerId::Hud, ChunkId(1), b.build());
    let ops: Vec<(std::ops::Range<u32>, u16)> = s
        .layer_ops(LayerId::Hud)
        .iter()
        .map(|o| (o.range.clone(), o.ext))
        .collect();
    assert_eq!(ops, vec![(0..1, 0), (1..3, 1), (3..5, 0), (5..6, 2)]);
    assert_eq!(s.ext_texture(1), Some(ExtTexture(7)));
    assert_eq!(s.ext_texture(2), Some(ExtTexture(8)));
    assert_eq!(s.ext_texture(0), None);
    // The chunk without its pictures frees their slots.
    s.set_chunk(LayerId::Hud, ChunkId(1), ChunkBuilder::new().build());
    assert_eq!(s.ext_slots().live(), 0);
    assert_eq!(s.ext_texture(1), None);
}
