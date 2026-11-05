//! TextureBar fill direction: Up fills from the bottom (the citizen growth meter of the city screen), Down from the top.
mod support;
use support::harness::*;

/// (min y, max y, v at the min y, v at the max y, min x, max x) of the first command's quad in a 1:1 draw.
fn quad(st: &mut ui::State) -> [f32; 6] {
    let mut out = draw_list::UiDrawList::new();
    st.draw_all(&mut out, 1.0);
    let v = out.vertices();
    assert!(!v.is_empty(), "the bar draws a quad");
    let top = v
        .iter()
        .min_by(|a, b| a.pos[1].total_cmp(&b.pos[1]))
        .unwrap();
    let bot = v
        .iter()
        .max_by(|a, b| a.pos[1].total_cmp(&b.pos[1]))
        .unwrap();
    let xs = v.iter().map(|p| p.pos[0]);
    [
        top.pos[1],
        bot.pos[1],
        top.uv[1],
        bot.uv[1],
        xs.clone().fold(f32::MAX, f32::min),
        xs.fold(f32::MIN, f32::max),
    ]
}

fn bar(dir: &str, percent: f32) -> [f32; 6] {
    let mut st = state();
    let id = build(
        &mut st,
        &format!(
            r#"<TextureBar ID="M" Anchor="L,T" Size="40,40" Texture="m.dds" Direction="{dir}"/>"#
        ),
    );
    st.roots.push(id);
    if let ui::Kind::TextureBar(b) = &mut st.node_mut(id).kind {
        b.percent = percent;
    }
    quad(&mut st)
}

#[test]
fn an_up_bar_fills_from_the_bottom_and_shows_the_bottom_of_the_picture() {
    let full = bar("Up", 1.0);
    let q = bar("Up", 0.25);
    assert!(
        (q[1] - full[1]).abs() < 0.01,
        "the bottom edge stays: {q:?} vs {full:?}"
    );
    assert!(
        (q[1] - q[0] - 10.0).abs() < 0.01,
        "a quarter of the height: {q:?}"
    );
    assert!(
        (q[3] - full[3]).abs() < 1e-4,
        "the bottom of the picture is at the bottom: {q:?} vs {full:?}"
    );
    assert!(
        q[2] > full[2],
        "the top of the shown part is lower in the picture"
    );
}

#[test]
fn a_down_bar_fills_from_the_top_and_shows_the_top_of_the_picture() {
    let full = bar("Down", 1.0);
    let q = bar("Down", 0.25);
    assert!((q[0] - full[0]).abs() < 0.01, "the top edge stays: {q:?}");
    assert!(
        (q[1] - q[0] - 10.0).abs() < 0.01,
        "a quarter of the height: {q:?}"
    );
    assert!(
        (q[2] - full[2]).abs() < 1e-4,
        "the top of the picture is at the top"
    );
}

#[test]
fn right_and_left_bars_fill_from_their_own_side() {
    let full = bar("Right", 1.0);
    let r = bar("Right", 0.5);
    assert!(
        (r[4] - full[4]).abs() < 0.01 && (r[5] - r[4] - 20.0).abs() < 0.01,
        "{r:?}"
    );
    let l = bar("Left", 0.5);
    assert!(
        (l[5] - full[5]).abs() < 0.01 && (l[5] - l[4] - 20.0).abs() < 0.01,
        "{l:?}"
    );
}
