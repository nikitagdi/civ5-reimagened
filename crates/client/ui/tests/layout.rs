//! Layout rules, each with the exe function it comes from.
mod support;
use support::harness::*;

#[test]
fn anchoring_table_inside_and_outside() {
    // ControlBase::ProcessAnchoring: parent 100x60, child 20x10 at offset (5,3).
    let cases: &[(&str, &str, (f32, f32))] = &[
        ("L,T", "", (5.0, 3.0)),
        ("R,T", "", (75.0, 3.0)),
        ("C,T", "", (45.0, 3.0)),
        ("L,B", "", (5.0, 47.0)),
        ("L,C", "", (5.0, 28.0)),
        ("R,B", "", (75.0, 47.0)),
        ("C,C", "", (45.0, 28.0)),
        ("R,C", "", (75.0, 28.0)),
        ("C,B", "", (45.0, 47.0)),
        // Outside: left of / above the parent, or past its far edge.
        ("L,T", "O.O", (-25.0, -13.0)),
        ("R,B", "O.O", (105.0, 63.0)),
        ("R,T", "O.I", (105.0, 3.0)),
        ("L,B", "I.O", (5.0, 63.0)),
        // Centre wins over the outer bit.
        ("C,C", "O.O", (45.0, 28.0)),
    ];
    for (anchor, side, want) in cases {
        let mut st = state();
        let xml = format!(
            r#"<Container Size="100,60"><Container ID="c" Size="20,10" Offset="5,3" Anchor="{anchor}" AnchorSide="{side}"/></Container>"#
        );
        let root = build(&mut st, &xml);
        let c = find(&st, root, "c");
        assert_eq!(pos(&st, c), *want, "Anchor={anchor} AnchorSide={side}");
    }
}

#[test]
fn layout_is_not_recomputed_without_a_call() {
    // The exe positions at Initialize and on explicit calls only.
    let mut st = state();
    let root = build(
        &mut st,
        r#"<Container Size="100,100"><Container ID="c" Size="10,10" Anchor="R,B"/></Container>"#,
    );
    let c = find(&st, root, "c");
    assert_eq!(pos(&st, c), (90.0, 90.0));
    st.node_mut(root).size = glam::Vec2::new(200.0, 200.0);
    assert_eq!(pos(&st, c), (90.0, 90.0), "stale until ReprocessAnchoring");
    st.reprocess_anchoring(root);
    assert_eq!(pos(&st, c), (190.0, 190.0));
}

#[test]
fn full_size_follows_the_screen_not_the_parent() {
    let mut st = state();
    let root = build(
        &mut st,
        r#"<Container Size="100,100"><Container ID="c" Size="Full.32"/></Container>"#,
    );
    let c = find(&st, root, "c");
    assert_eq!(size(&st, c), (1920.0, 32.0));
    st.cfg.screen_width = 1000;
    st.apply_screen_size(root);
    assert_eq!(size(&st, c), (1000.0, 32.0));
}

fn stack_positions(dir: &str, extra: &str) -> Vec<(f32, f32)> {
    let mut st = state();
    let xml = format!(
        r#"<Stack ID="s" StackGrowth="{dir}" {extra}>
            <Container ID="a" Size="10,10"/><Container ID="b" Size="20,5"/><Container ID="c" Size="10,10"/></Stack>"#
    );
    let root = build(&mut st, &xml);
    let offsets = st.stack_offsets(root);
    let origin = match &st.node(root).kind {
        ui::Kind::Stack(s) => s.content,
        _ => unreachable!(),
    };
    offsets
        .iter()
        .map(|o| (origin.x + o.x, origin.y + o.y))
        .collect()
}

#[test]
fn stack_grows_in_four_directions() {
    // StackControl::CalculateNextStackOffset
    assert_eq!(
        stack_positions("Bottom", ""),
        [(0.0, 0.0), (0.0, 10.0), (0.0, 15.0)]
    );
    assert_eq!(
        stack_positions("Right", ""),
        [(0.0, 0.0), (10.0, 0.0), (30.0, 0.0)]
    );
    // Up and left: the first child is at the far end.
    assert_eq!(
        stack_positions("Top", ""),
        [(0.0, 15.0), (0.0, 10.0), (0.0, 0.0)]
    );
    assert_eq!(
        stack_positions("Left", ""),
        [(30.0, 0.0), (10.0, 0.0), (0.0, 0.0)]
    );
}

#[test]
fn stack_size_padding_and_skipped_children() {
    // StackControl::CalculateSize: hidden, zero-size and outer children are not in the flow.
    let mut st = state();
    let root = build(
        &mut st,
        r#"<Stack ID="s" StackGrowth="Bottom" Padding="4">
            <Container Size="10,10"/><Container Size="10,10" Hidden="1"/><Container Size="0,5"/>
            <Container Size="50,50" AnchorSide="O.O"/><Container Size="10,6"/></Stack>"#,
    );
    // 10 + 4 + 6 and the widest of the flow, 10.
    assert_eq!(size(&st, root), (10.0, 20.0));
}

#[test]
fn stack_add_child_drops_the_anchor_on_its_axis() {
    // StackControl::AddChild
    let mut st = state();
    let root = build(
        &mut st,
        r#"<Stack StackGrowth="Bottom"><Container ID="a" Size="10,10" Anchor="R,B" AnchorSide="I.O"/>
           <Container ID="b" Size="10,10" Anchor="R,B"/></Stack>"#,
    );
    let a = find(&st, root, "a");
    let b = find(&st, root, "b");
    use ui::flags::*;
    // Vertical stack: V anchors go, except V_OUTER; H anchors stay.
    assert_ne!(st.node(a).flags & V_OUTER, 0);
    assert_ne!(
        st.node(a).flags & V_BOTTOM,
        0,
        "an outer child keeps its anchor"
    );
    assert_eq!(st.node(b).flags & (V_BOTTOM | V_CENTER), 0);
    assert_ne!(st.node(b).flags & H_RIGHT, 0);
}

#[test]
fn stack_wraps_at_wrap_width() {
    // StackControl::CalculateNextStackOffset with WrapWidth: a full row starts the next one.
    let p = stack_positions("Right", r#"WrapWidth="25" WrapGrowth="Bottom""#);
    assert_eq!(p[0], (0.0, 0.0));
    // 10 + 20 reaches 30 > 25: the second starts the next row, and so does the third.
    assert_eq!(p[1], (0.0, 10.0));
    assert_eq!(p[2], (0.0, 15.0));
}

#[test]
fn grid_nine_slice_and_auto_size() {
    // GridControl::AdjustComponents / CalculateAutoSize
    let mut st = state();
    let root = build(
        &mut st,
        r#"<Grid ID="g" Size="100,60" Texture="g.dds"
             ULSize="8,8" URSize="8,8" LLSize="8,8" LRSize="8,8" LSize="8,0" RSize="8,0" UCSize="0,8" LCSize="0,8"
             CSize="0,0" ULTexStart="0,0" UCTexStart="8,0" URTexStart="16,0" LTexStart="0,8" CTexStart="8,8" RTexStart="16,8"
             LLTexStart="0,16" LCTexStart="8,16" LRTexStart="16,16"/>"#,
    );
    let ui::Kind::Grid(g) = &st.node(root).kind else {
        panic!()
    };
    // order C, UC, LC, L, R, UL, UR, LL, LR
    assert_eq!(g.cell_size[1].x, 100.0 - 8.0 - 8.0);
    assert_eq!(g.cell_size[3].y, 60.0 - 8.0 - 8.0);
    assert_eq!(g.cell_size[0].x, 100.0 - 16.0);
    assert_eq!(g.cell_size[0].y, 60.0 - 16.0);
    assert_eq!(g.cell_offset[6].x, 92.0);
    assert_eq!(g.cell_offset[7].y, 52.0);
    assert_eq!(g.cell_offset[0].x, 8.0);

    let root = build(
        &mut st,
        r#"<Grid AutoSize="1" Padding="6,4" Texture="g.dds"><Container Size="30,20"/><Container Size="10,50" AnchorSide="O.O"/></Grid>"#,
    );
    assert_eq!(size(&st, root), (42.0, 28.0));
}

fn scroll_panel(extra: &str) -> (ui::State, ui::ControlId) {
    let mut st = state();
    let root = build(
        &mut st,
        &format!(
            r#"<ScrollPanel ID="p" Size="100,50" Vertical="1" {extra}>
                <Container ID="a" Size="100,200"/>
                <ScrollBar Length="50" Size="18,50"><Shuttle Size="18,18"/></ScrollBar>
            </ScrollPanel>"#
        ),
    );
    (st, root)
}

#[test]
fn a_scroll_panel_child_keeps_its_right_margin_inward() {
    // EUI NotificationPanel: SmallStack Anchor="R,B" Offset="-6,0" in a panel as wide as the icons. The sign of the
    // offset made it 6 px out of the panel and the screen (the icons' right edge was clipped).
    let mut st = state();
    let root = build(
        &mut st,
        r#"<ScrollPanel Size="80,80" Vertical="1"><Container ID="s" Anchor="R,B" Size="80,80" Offset="-6,0"/><Container ID="t" Anchor="R,T" Size="40,10" Offset="6,0"/></ScrollPanel>"#,
    );
    let pos = |id| {
        let p = st.node(id).pos;
        (p.x, p.y)
    };
    assert_eq!(pos(find(&st, root, "s")).0, -6.0);
    assert_eq!(pos(find(&st, root, "t")).0, 80.0 - 40.0 - 6.0);
}

#[test]
fn scroll_panel_range_step_and_value() {
    // ScrollPanelControl::CalculateInternalSize / SetScrollValue
    let (mut st, p) = scroll_panel("");
    let ui::Kind::ScrollPanel(sp) = &st.node(p).kind else {
        panic!()
    };
    assert_eq!(sp.max_offset, 50.0 - 200.0);
    assert_eq!(sp.ratio, 0.25);
    // Below the ratio threshold the step is proportional: 50 * 0.1 / 200.
    assert!((sp.step - 0.025).abs() < 1e-6);
    st.scroll_set_value(p, 0.5);
    let ui::Kind::ScrollPanel(sp) = &st.node(p).kind else {
        panic!()
    };
    assert_eq!(sp.scroll.y, -75.0);
    st.scroll_set_value(p, 3.0);
    let ui::Kind::ScrollPanel(sp) = &st.node(p).kind else {
        panic!()
    };
    assert_eq!(sp.value, 1.0);
    assert_eq!(sp.scroll.y, -150.0);
}

#[test]
fn scroll_bar_is_hidden_when_nothing_scrolls_and_buttons_disabled_without_auto() {
    let mut st = state();
    let p = build(
        &mut st,
        r#"<ScrollPanel Size="100,300" Vertical="1"><Container Size="100,50"/>
            <UpButton Size="18,18"/><DownButton Size="18,18"/></ScrollPanel>"#,
    );
    assert_ne!(st.node(p).flags & ui::tree::sp_flag::BAR_HIDDEN, 0);
    let mut st = state();
    let p = build(
        &mut st,
        r#"<ScrollPanel Size="100,300" Vertical="1" AutoScrollBar="0"><Container Size="100,50"/>
            <UpButton Size="18,18"/><DownButton Size="18,18"/></ScrollPanel>"#,
    );
    assert_eq!(st.node(p).flags & ui::tree::sp_flag::BAR_HIDDEN, 0);
    let ui::Kind::ScrollPanel(sp) = &st.node(p).kind else {
        panic!()
    };
    assert_ne!(st.node(sp.up.unwrap()).flags & ui::flags::DISABLED, 0);
}

#[test]
fn scroll_panel_children_anchor_to_the_content_size() {
    let (st, p) = scroll_panel("");
    let _ = (st, p);
    let mut st = state();
    let p = build(
        &mut st,
        r#"<ScrollPanel Size="100,50" Vertical="1"><Container Size="100,200"/>
            <Container ID="r" Size="10,10" Anchor="R,B"/></ScrollPanel>"#,
    );
    let r = find(&st, p, "r");
    // ScrollPanel: the size of the content (+0x8c), not the panel's own.
    assert_eq!(pos(&st, r), (90.0, 190.0));
}

#[test]
fn visibility_box_unites_children_and_is_not_clipped_by_the_parent() {
    // ControlBase::CalculateVisibilityBox
    let mut st = state();
    let root = build(
        &mut st,
        r#"<Container Size="10,10"><Container ID="c" Size="5,5" Offset="20,30"/></Container>"#,
    );
    assert_eq!(st.node(root).vis, [0.0, 0.0, 25.0, 35.0]);
}

#[test]
fn set_offset_floors_and_reanchors() {
    // ControlBase::SetOffset
    let mut st = state();
    let root = build(
        &mut st,
        r#"<Container Size="100,100"><Container ID="c" Size="10,10"/></Container>"#,
    );
    let c = find(&st, root, "c");
    st.set_offset(c, 3.7, 4.2);
    assert_eq!(pos(&st, c), (3.0, 4.0));
}

#[test]
fn a_label_text_change_measures_the_stack_it_sits_in() {
    // EUI NotificationPanel LeaderButtonInstanceL: a stack without an ID, anchored left, growing left; its labels get
    // their text after the build. The stack must follow, or its children are drawn left of its anchor point (over the
    // leader portrait). SIMP-uic-21.
    let mut st = state();
    let root = build(
        &mut st,
        r#"<Button ID="b" Size="64,64"><Stack ID="s" Anchor="L,C" Offset="60,-10" Padding="2" StackGrowth="Left">
            <Label ID="a" Font="TwCenMT16"/><Label ID="c" Font="TwCenMT16"/></Stack></Button>"#,
    );
    let (s, a, c) = (
        find(&st, root, "s"),
        find(&st, root, "a"),
        find(&st, root, "c"),
    );
    st.label_change(a, |l| l.text = "123".to_owned());
    st.label_change(c, |l| l.text = "xy".to_owned());
    // 24 + 2 + 16 + 2 (the padding is counted after every child), from x = 60.
    assert_eq!(size(&st, s).0, 44.0);
    assert_eq!(pos(&st, s).0, 60.0);
    let offsets = st.stack_offsets(s);
    let origin = match &st.node(s).kind {
        ui::Kind::Stack(k) => k.content,
        _ => unreachable!(),
    };
    assert!(
        origin.x + offsets[0].x > origin.x + offsets[1].x,
        "the first child is on the right"
    );
    assert_eq!(origin.x + offsets[1].x, 60.0, "no child left of the anchor");
}
