//! CheckBox: the exe's built-in textures, and the tick drawn only while checked.
mod support;
use support::harness::*;

fn draw(st: &mut ui::State) -> usize {
    let mut out = draw_list::UiDrawList::new();
    st.draw_all(&mut out, 1.0);
    out.commands().len()
}

#[test]
fn a_checkbox_without_textures_draws_the_default_box_and_the_tick_when_checked() {
    let mut st = state();
    let id = build(
        &mut st,
        r#"<CheckBox Anchor="L,T" ID="C" IsChecked="0" String="X" />"#,
    );
    st.roots.push(id);
    let unchecked = draw(&mut st);
    assert!(unchecked >= 1, "the box is drawn from CheckBox.dds");
    if let ui::Kind::CheckBox(c) = &mut st.node_mut(id).kind {
        c.checked = true;
    }
    assert_eq!(draw(&mut st), unchecked + 1, "CheckMark.dds when checked");
}

#[test]
fn the_tooltip_root_draws_after_every_other_root() {
    let mut st = state();
    let tip = build(
        &mut st,
        r#"<Image ID="Tip" Size="10,10" Texture="tip.dds" />"#,
    );
    let other = build(
        &mut st,
        r#"<Image ID="Other" Size="10,10" Texture="other.dds" />"#,
    );
    // roots are newest first and draw oldest first: `tip` (last) would draw first without the rule.
    st.roots = vec![other, tip];
    st.tooltips.root = Some(tip);
    let mut out = draw_list::UiDrawList::new();
    st.draw_all(&mut out, 1.0);
    let cmds = out.commands();
    assert_eq!(cmds.len(), 2);
    assert_ne!(cmds[0].texture, cmds[1].texture);
    let mut plain = draw_list::UiDrawList::new();
    st.tooltips.root = None;
    st.draw_all(&mut plain, 1.0);
    assert_eq!(plain.commands()[0].texture, cmds[1].texture);
}
