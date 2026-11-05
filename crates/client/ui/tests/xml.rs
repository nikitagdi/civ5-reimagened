//! XML to controls: the tag table, styles, warnings, the attribute-case switch.
mod support;
use std::rc::Rc;
use support::harness::*;
use ui::style::StyleBook;
use ui::{Kind, State, StubHost, StubText, UiConfig};

fn kind_of(tag: &str) -> Option<&'static str> {
    let mut st = state();
    let xml = format!(r#"<{tag} Size="10,10"/>"#);
    let el = ui::xml::parse("t.xml", xml.as_bytes()).unwrap();
    st.build_control(&el, None)
        .map(|id| st.node(id).kind.name())
}

#[test]
fn tag_table_follows_ui_manager_build_control() {
    // UIManager::BuildControl: exact names.
    for (tag, class) in [
        ("Button", "Button"),
        ("Label", "Label"),
        ("Box", "Box"),
        ("BoxButton", "BoxButton"),
        ("Container", "ControlBase"),
        ("ScrollAnim", "ScrollAnim"),
        ("FlipAnim", "FlipAnim"),
        ("SlideAnim", "SlideAnim"),
        ("AlphaAnim", "AlphaAnim"),
        ("TextButton", "TextButton"),
        ("Image", "Image"),
        ("Stack", "Stack"),
        ("LuaContext", "LuaContext"),
        ("CheckBox", "CheckBox"),
        ("GridButton", "GridButton"),
        ("Line", "Line"),
        ("ReplayMap", "ReplayMap"),
        ("Grid", "Grid"),
        ("ScrollPanel", "ScrollPanel"),
        ("Meter", "Meter"),
        ("Bar", "Bar"),
        ("TextureBar", "TextureBar"),
        ("WorldAnchor", "WorldAnchor"),
        ("RadioButton", "RadioButton"),
        ("Slider", "Slider"),
        ("EditBox", "EditBox"),
        ("PullDown", "PullDown"),
    ] {
        assert_eq!(kind_of(tag), Some(class), "<{tag}>");
    }
    // Read by their parent: no control and no message.
    for tag in [
        "ColorSet",
        "Instance",
        "Shuttle",
        "UpButton",
        "DownButton",
        "ScrollBar",
        "ButtonData",
        "StackData",
        "GridData",
        "InstanceData",
        "ScrollPanelData",
        "ShowOnMouseOver",
        "HideOnMouseOver",
    ] {
        let mut st = state();
        let el = ui::xml::parse("t.xml", format!("<{tag}/>").as_bytes()).unwrap();
        assert!(st.build_control(&el, None).is_none());
        assert!(st.warnings.is_empty(), "<{tag}> must be silent");
    }
    // Comparison is exact: no case folding, no prefix matching.
    assert_eq!(kind_of("button"), None);
    assert_eq!(kind_of("Buttons"), None);
}

#[test]
fn unknown_tag_and_attribute_warn_once_with_file_and_line() {
    let mut st = state();
    let el = ui::xml::parse(
        "ui.xml",
        b"<Container>\n<Button Ancor=\"C,C\" Size=\"1,1\"/>\n<Button Ancor=\"L,T\" Size=\"1,1\"/>\n<Frob/>\n</Container>",
    )
    .unwrap();
    st.build_control(&el, None).unwrap();
    let w: Vec<_> = st
        .warnings
        .iter()
        .map(|w| (w.line, w.tag.as_str(), w.what.as_str()))
        .collect();
    assert!(
        w.iter()
            .any(|x| x.0 == 2 && x.1 == "Button" && x.2.contains("Ancor")),
        "{w:?}"
    );
    assert_eq!(
        w.iter().filter(|x| x.2.contains("Ancor")).count(),
        1,
        "once per file and name"
    );
    assert!(
        w.iter()
            .any(|x| x.0 == 4 && x.2.contains("Unknown control type Frob")),
        "{w:?}"
    );
}

#[test]
fn attribute_case_switch() {
    for (ci, hidden) in [(false, false), (true, true)] {
        let mut st = state();
        st.cfg.attr_case_insensitive = ci;
        let root = build(&mut st, r#"<Container hidden="1"/>"#);
        assert_eq!(
            st.node(root).flags & ui::flags::HIDDEN != 0,
            hidden,
            "ci={ci}"
        );
    }
}

fn styled() -> State {
    let base = ui::xml::parse(
        "Styles.xml",
        br#"<StyleSheet>
          <Wide Size="40,10" Anchor="R,B"><ShowOnMouseOver><Image ID="glow" Size="3,3"/></ShowOnMouseOver></Wide>
          <Chained Style="Wide" Size="1,1"/>
        </StyleSheet>"#,
    )
    .unwrap();
    state_with(
        StubHost::with_default_size(8, 8),
        StyleBook::new(&[], Some(&base), &[], None),
    )
}

#[test]
fn style_attributes_apply_first_and_own_attributes_win() {
    // ControlBase::BuildFromStyle: the style is built into the control, then its own attributes.
    let mut st = styled();
    let root = build(
        &mut st,
        r#"<Container Size="100,100"><Image ID="c" Style="Wide" Size="30,10"/></Container>"#,
    );
    let c = find(&st, root, "c");
    assert_eq!(size(&st, c), (30.0, 10.0));
    assert_ne!(
        st.node(c).flags & ui::flags::H_RIGHT,
        0,
        "Anchor comes from the style"
    );
    assert_eq!(pos(&st, c), (70.0, 90.0));
    // The style's ShowOnMouseOver child is a hidden child of the control.
    let glow = find(&st, c, "glow");
    assert_eq!(st.node(c).show_on_over, [glow]);
    assert_ne!(st.node(glow).flags & ui::flags::MOUSE_OVER_HIDDEN, 0);
}

#[test]
fn a_style_that_names_a_style_is_not_followed_and_a_missing_style_warns() {
    let mut st = styled();
    let root = build(
        &mut st,
        r#"<Container Size="100,100"><Image ID="c" Style="Chained"/><Image ID="d" Style="Nope" Size="2,2"/></Container>"#,
    );
    let c = find(&st, root, "c");
    assert_eq!(size(&st, c), (0.0, 0.0));
    let d = find(&st, root, "d");
    assert_eq!(size(&st, d), (2.0, 2.0), "built without the style");
    assert!(
        st.warnings
            .iter()
            .any(|w| w.what.contains("Unidentified style: Nope"))
    );
}

#[test]
fn context_default_values_do_not_leak_into_other_tags_and_ids_are_kept() {
    let mut st = state();
    let root = build(
        &mut st,
        r#"<Container ID="r"><Label ID="l" String="hello" Anchor="C,C"/></Container>"#,
    );
    let l = find(&st, root, "l");
    let Kind::Label(lab) = &st.node(l).kind else {
        panic!()
    };
    assert_eq!(lab.text, "hello");
    // Text size comes from the text engine: 5 chars of 8 px, one line of 16.
    assert_eq!(size(&st, l), (40.0, 16.0));
}

#[test]
fn colors_by_name_and_numbers() {
    let atlas = ui::xml::parse(
        "a.xml",
        br#"<ColorAtlas><Color Name="Red" Color="255,0,0,255"/></ColorAtlas>"#,
    )
    .unwrap();
    let mut st = state_with(
        StubHost::default(),
        StyleBook::new(&[], None, &[], Some(&atlas)),
    );
    let root = build(&mut st, r#"<Box ID="b" Color="Red.128" Size="4,4"/>"#);
    let Kind::ColorBox(b) = &st.node(root).kind else {
        panic!()
    };
    assert_eq!(b.color[..3], [1.0, 0.0, 0.0]);
    assert!((b.color[3] - 128.0 / 255.0).abs() < 1e-6);
    let _ = (Rc::new(1), StubText::default(), UiConfig::default());
}

#[test]
fn image_size_and_texture_offset() {
    let mut host = StubHost::default();
    host.add_texture("Assets/UI/Art/pic.dds", 100, 50);
    let mut st = state_with(host, StyleBook::default());
    let img = build(
        &mut st,
        r#"<Image Texture="pic.dds" TextureOffset="10,5" Size="20,10"/>"#,
    );
    let Kind::Image(i) = &st.node(img).kind else {
        panic!()
    };
    assert_eq!(i.quad.uv_offset, glam::Vec2::new(0.1, 0.1));
    assert_eq!(i.quad.uv_scale, glam::Vec2::new(0.2, 0.2));
    // A texture the host does not know is not an error: the image just has none.
    let mut st = state_with(StubHost::default(), StyleBook::default());
    let img = build(&mut st, r#"<Image Texture="missing.dds" Size="20,10"/>"#);
    let Kind::Image(i) = &st.node(img).kind else {
        panic!()
    };
    assert!(i.quad.texture.is_none());
}

#[test]
fn mask_texture_selects_blend_slot_two_and_sampler_names() {
    let mut st = state();
    let img = build(
        &mut st,
        r#"<Image Texture="a.dds" MaskTexture="m.dds" Size="4,4" Sampler="Linear"/>"#,
    );
    let Kind::Image(i) = &st.node(img).kind else {
        panic!()
    };
    assert_eq!(i.quad.blend_slot, 2);
    assert!(i.quad.mask.is_some());
    assert_ne!(st.node(img).flags & ui::tree::image_flag::SAMPLER_SET, 0);
}

#[test]
fn button_defaults_and_state_texture_windows() {
    // ButtonControl::Initialize / SetVisState: states are stacked by the height unless told otherwise.
    let mut host = StubHost::default();
    host.add_texture("b.dds", 64, 128);
    let mut st = state_with(host, StyleBook::default());
    let b = build(&mut st, r#"<Button Texture="b.dds" Size="64,32"/>"#);
    st.set_vis_state(b, 2);
    let Kind::Button(btn) = &st.node(b).kind else {
        panic!()
    };
    assert_eq!(btn.quad.src_y, 64.0);
    assert_ne!(st.node(b).flags & ui::flags::NEEDS_MOUSE_OVER, 0);
}
