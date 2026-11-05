//! Tooltips of buttons built from an instance manager template (UnitFlagManager): one tip text per button.
mod support;
use support::ctx::*;

fn tip_text(f: &Fixture) -> String {
    let st = f.ui.shared().st.borrow();
    let t = st.tooltips.text.expect("tip text");
    match &st.node(t).kind {
        ui::tree::Kind::Label(l) => l.text.clone(),
        _ => panic!("not a label"),
    }
}

#[test]
fn each_instance_button_shows_its_own_tooltip_text() {
    let mut f = fixture_with(
        &[
            (
                "UI/ToolTips.xml",
                r#"<Context><Container ID="Root" Size="1,1" Hidden="1"><Container ID="Content" Size="1,1"/></Container><Label ID="Text" String=""/><Grid ID="Grid" Size="10,10"/></Context>"#,
            ),
            (
                "UI/ToolTips.lua",
                r#"TTManager:RegisterBasicControls(Controls.Root, Controls.Content, Controls.Text, Controls.Grid)"#,
            ),
            (
                "UI/Flags.xml",
                r#"<Context>
                  <ToolTipType Name="EUI_UnitTooltip"><Grid ID="Box" Size="30,30"><Label ID="Text" String="Brief description"/></Grid></ToolTipType>
                  <Container ID="Parent" Size="200,50"/>
                  <Instance Name="UnitFlag"><Container ID="Anchor" Size="40,40"><Button ID="Button" Size="40,40" Texture="b.dds" ToolTipType="EUI_UnitTooltip"/></Container></Instance>
                </Context>"#,
            ),
            (
                "UI/Flags.lua",
                r#"
                local tt = {}
                TTManager:GetTypeControlTable("EUI_UnitTooltip", tt)
                local function tip(button)
                  tt.Text:SetText("unit " .. button:GetVoid1() .. "/" .. button:GetVoid2())
                end
                for i = 1, 2 do
                  local flag = {}
                  ContextPtr:BuildInstanceForControl("UnitFlag", flag, Controls.Parent)
                  flag.Anchor:SetOffsetVal((i - 1) * 60, 0)
                  flag.Button:SetToolTipCallback(tip)
                  flag.Button:SetVoids(i, i * 10)
                end
                "#,
            ),
        ],
        enums,
    );
    f.ui.load_root("Assets/UI/ToolTips");
    f.ui.load_root("Assets/UI/Flags");
    f.log();
    for (x, want) in [
        (20.0, "unit 1/10"),
        (80.0, "unit 2/20"),
        (20.0, "unit 1/10"),
    ] {
        f.mv(300.0, 300.0);
        f.mv(x, 20.0);
        f.mv(x + 1.0, 21.0);
        assert!(f.errors().is_empty(), "{:?}", f.errors());
        let st = f.ui.shared().st.borrow();
        let tt = st.tooltips.current;
        drop(st);
        assert!(tt.is_some(), "no tip requested at x={x}");
        let st = f.ui.shared().st.borrow();
        let found = st
            .tree
            .ids()
            .filter(|&i| st.node(i).id == "Text")
            .filter_map(|i| match &st.node(i).kind {
                ui::tree::Kind::Label(l) => Some(l.text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            found.iter().any(|t| t == want),
            "x={x} want {want} got {found:?} basic={}",
            {
                drop(st);
                tip_text(&f)
            }
        );
    }
}

/// UnitFlagManager asks for the controls of `EUI_UnitTooltip` when it loads, before ToolTips registered the basic
/// controls (the type is defined by an earlier context): the table is filled and the tip is the type's, not its default.
#[test]
fn a_context_that_loads_before_tooltips_gets_the_controls_of_a_type() {
    let mut f = fixture_with(
        &[
            (
                "UI/Types.xml",
                r#"<Context><ToolTipType Name="EUI_T"><Grid ID="Box" Size="30,30"><Label ID="Text" String="Brief description"/></Grid></ToolTipType></Context>"#,
            ),
            ("UI/Types.lua", ""),
            (
                "UI/Flags.xml",
                r#"<Context><Button ID="B" Size="40,40" Texture="b.dds" ToolTipType="EUI_T"/></Context>"#,
            ),
            (
                "UI/Flags.lua",
                r#"
                local tt = {}
                TTManager:GetTypeControlTable("EUI_T", tt)
                print("early " .. tostring(tt.Text ~= nil))
                Controls.B:SetToolTipCallback(function() tt.Text:SetText("unit") end)
                "#,
            ),
            (
                "UI/ToolTips.xml",
                r#"<Context><Container ID="Root" Size="1,1" Hidden="1"><Container ID="Content" Size="1,1"/></Container><Label ID="Text" String=""/><Grid ID="Grid" Size="10,10"/></Context>"#,
            ),
            (
                "UI/ToolTips.lua",
                r#"TTManager:RegisterBasicControls(Controls.Root, Controls.Content, Controls.Text, Controls.Grid)"#,
            ),
        ],
        enums,
    );
    f.ui.load_root("Assets/UI/Types");
    f.ui.load_root("Assets/UI/Flags");
    f.ui.load_root("Assets/UI/ToolTips");
    assert!(f.log().iter().any(|l| l.contains("early true")));
    f.mv(300.0, 300.0);
    f.mv(20.0, 20.0);
    f.mv(21.0, 21.0);
    assert!(f.errors().is_empty(), "{:?}", f.errors());
    let st = f.ui.shared().st.borrow();
    let texts: Vec<String> = st
        .tree
        .ids()
        .filter(|&i| st.node(i).id == "Text")
        .filter_map(|i| match &st.node(i).kind {
            ui::tree::Kind::Label(l) => Some(l.text.clone()),
            _ => None,
        })
        .collect();
    assert!(texts.iter().any(|t| t == "unit"), "{texts:?}");
    assert!(st.tooltips.current.is_some(), "a tip is shown");
}

#[test]
fn the_unit_tip_box_stretches_to_its_text_lines() {
    // EUI_UnitTooltip: Box (Grid) > Stack > Label + empty Stack. UnitFlagToolTip sets the text, SetWrapWidth on the inner
    // stack and calls Box:DoAutoSize() (no CalculateSize on the stack): the box must grow with the lines.
    let f = fixture_with(
        &[
            (
                "UI/Tip.xml",
                r#"<Context><Grid ID="Box" Anchor="L,C" Size="300,50" Padding="6,6" Style="GridBlack8" NoClip="1">
                  <Stack ID="Outer" Offset="60,14" StackGrowth="Down" Padding="8">
                    <Label ID="Text" String="Brief description" WrapWidth="500" Font="TwCenMT16"/>
                    <Stack ID="Stack" StackGrowth="Right" Padding="-6" WrapGrowth="Bottom" WrapPadding="-6" WrapWidth="260"/>
                  </Stack></Grid></Context>"#,
            ),
            (
                "UI/Tip.lua",
                r#"
                local function sz() local w, h = Controls.Box:GetSizeVal() return w .. "x" .. h end
                local function lw() local w, h = Controls.Text:GetSizeVal() return w .. "x" .. h end
                Controls.Text:SetText("one")
                Controls.Stack:SetWrapWidth(0)
                Controls.Box:DoAutoSize()
                Controls.Box:DoAutoSize()
                print("L1 box " .. sz() .. " text " .. lw())
                Controls.Text:SetText("one[NEWLINE]two[NEWLINE]three")
                Controls.Stack:SetWrapWidth(0)
                Controls.Box:DoAutoSize()
                Controls.Box:DoAutoSize()
                print("L3 box " .. sz() .. " text " .. lw())
                Controls.Text:SetText("a much longer first line of this tip[NEWLINE]two")
                Controls.Box:DoAutoSize()
                print("LW box " .. sz() .. " text " .. lw())
                "#,
            ),
        ],
        enums,
    );
    f.ui.load_root("Assets/UI/Tip");
    let log = f.log();
    assert!(f.errors().is_empty(), "{:?}", f.errors());
    // The frame is the text plus the stack's offset (60,14) and the padding 6+6: it follows the lines and the width.
    assert_eq!(
        log,
        [
            "L1 box 96x42 text 24x16",
            "L3 box 112x74 text 40x48",
            "LW box 360x58 text 288x32"
        ]
    );
}

/// Lua keys tables by `tostring(control)` (TechButtonInclude's `g_buttonItemInfo`): the buttons "B1" of two instances of
/// one template are two controls, so their strings differ (the tech tree showed one tooltip for every unlocked building).
#[test]
fn the_buttons_of_two_instances_have_different_strings() {
    let f = fixture_with(
        &[
            (
                "UI/Tree.xml",
                r#"<Context>
                  <Container ID="Parent" Size="200,50"/>
                  <Instance Name="Tech"><Button ID="B1" Size="40,40" Texture="b.dds"/></Instance>
                </Context>"#,
            ),
            (
                "UI/Tree.lua",
                r#"
                local a, b = {}, {}
                ContextPtr:BuildInstanceForControl("Tech", a, Controls.Parent)
                ContextPtr:BuildInstanceForControl("Tech", b, Controls.Parent)
                local info = {}
                info[tostring(a.B1)] = "airport"
                info[tostring(b.B1)] = "walls"
                print("tostring " .. tostring(tostring(a.B1) ~= tostring(b.B1)) .. " " .. info[tostring(a.B1)] .. " " .. tostring(a.B1 == Controls.Parent))
                "#,
            ),
        ],
        enums,
    );
    f.ui.load_root("Assets/UI/Tree");
    assert!(f.errors().is_empty(), "{:?}", f.errors());
    assert!(
        f.log()
            .iter()
            .any(|l| l.contains("tostring true airport false")),
        "{:?}",
        f.log()
    );
}
