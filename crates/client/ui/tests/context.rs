//! Contexts on `lua-rt`: creation order, handlers, lookup, instances, isolation, tool tip types.
mod support;
use support::ctx::*;

fn has(lines: &[String], needle: &str) -> bool {
    lines.iter().any(|l| l.contains(needle))
}

fn index_of(lines: &[String], needle: &str) -> usize {
    lines
        .iter()
        .position(|l| l.contains(needle))
        .unwrap_or_else(|| panic!("{needle} not in {lines:?}"))
}

#[test]
fn child_contexts_load_before_the_parent_script_and_controls_are_exposed() {
    let mut f = fixture(&[
        (
            "UI/Root.xml",
            r#"<Context><Button ID="B" Size="10,10"/><LuaContext ID="Kid" FileName="Assets/UI/Kid"/></Context>"#,
        ),
        (
            "UI/Root.lua",
            r#"print("root sees B: " .. tostring(Controls.B ~= nil))"#,
        ),
        (
            "UI/Kid.xml",
            r#"<Context><Label ID="L" String="hi"/></Context>"#,
        ),
        (
            "UI/Kid.lua",
            r#"print("kid sees L: " .. tostring(Controls.L ~= nil) .. " not B: " .. tostring(Controls.B == nil))"#,
        ),
    ]);
    f.ui.load_root("Assets/UI/Root");
    let log = f.log();
    assert!(
        index_of(&log, "kid sees L: true not B: true") < index_of(&log, "root sees B: true"),
        "{log:?}"
    );
    assert!(f.errors().is_empty(), "{:?}", f.errors());
    let _ = &mut f;
}

#[test]
fn show_hide_handler_update_and_post_init() {
    let mut f = fixture(&[
        (
            "UI/Root.xml",
            r#"<Context><Container ID="C" Size="5,5"/></Context>"#,
        ),
        (
            "UI/Root.lua",
            r#"
            local frames = 0
            ContextPtr:SetShowHideHandler(function(hide, init) print("showhide " .. tostring(hide) .. " " .. tostring(init)) end)
            ContextPtr:SetPostInit(function() print("postinit") end)
            ContextPtr:SetUpdate(function(dt) frames = frames + 1; print("update " .. frames) if frames == 2 then ContextPtr:ClearUpdate() end end)
            "#,
        ),
    ]);
    let root = f.ui.load_root("Assets/UI/Root");
    let log = f.log();
    assert!(has(&log, "showhide false true"), "{log:?}");
    f.ui.update(0.016);
    f.ui.update(0.016);
    f.ui.update(0.016);
    let log = f.log();
    // PostInit runs once, at the first update; the update function stops after ClearUpdate.
    assert_eq!(
        log.iter().filter(|l| l.contains("postinit")).count(),
        1,
        "{log:?}"
    );
    assert!(
        has(&log, "update 1") && has(&log, "update 2") && !has(&log, "update 3"),
        "{log:?}"
    );
    // SetHide calls the handler with bIsInit false.
    f.ui.shared().set_hide(root, true);
    assert!(has(&f.log(), "showhide true false"));
    assert!(f.errors().is_empty(), "{:?}", f.errors());
}

#[test]
fn hidden_contexts_are_not_updated() {
    let mut f = fixture(&[
        (
            "UI/Root.xml",
            r#"<Context><LuaContext ID="K" FileName="Assets/UI/K" Hidden="1"/></Context>"#,
        ),
        ("UI/Root.lua", ""),
        ("UI/K.xml", "<Context/>"),
        (
            "UI/K.lua",
            r#"ContextPtr:SetUpdate(function() print("k update") end)"#,
        ),
    ]);
    f.ui.load_root("Assets/UI/Root");
    f.log();
    f.ui.update(0.016);
    assert!(!has(&f.log(), "k update"));
    let k = f.find("K");
    f.ui.shared().set_hide(k, false);
    f.ui.update(0.016);
    assert!(has(&f.log(), "k update"));
}

#[test]
fn defer_load_waits_for_the_first_show() {
    let f = fixture(&[
        (
            "UI/Root.xml",
            r#"<Context><LuaContext ID="D" FileName="Assets/UI/D" DeferLoad="1" Hidden="1"/></Context>"#,
        ),
        ("UI/Root.lua", ""),
        (
            "UI/D.xml",
            r#"<Context><Container ID="Inner" Size="3,3"/></Context>"#,
        ),
        ("UI/D.lua", r#"print("d loaded")"#),
    ]);
    f.ui.load_root("Assets/UI/Root");
    assert!(!has(&f.log(), "d loaded"));
    let d = f.find("D");
    f.ui.shared().set_hide(d, false);
    assert!(has(&f.log(), "d loaded"));
}

#[test]
fn look_up_control_absolute_and_relative_with_dotdot() {
    let f = fixture(&[
        (
            "UI/Root.xml",
            r#"<Context ID="RootCtx"><Container ID="Top" Size="1,1"/><LuaContext ID="Kid" FileName="Assets/UI/Kid"/></Context>"#,
        ),
        ("UI/Root.lua", ""),
        (
            "UI/Kid.xml",
            r#"<Context><Container ID="Leaf" Size="1,1"/></Context>"#,
        ),
        (
            "UI/Kid.lua",
            r#"
            print("abs " .. tostring(LookUpControl("/root/Kid/Leaf") ~= nil))
            print("up " .. tostring(ContextPtr:LookUpControl("../Top") ~= nil))
            print("none " .. tostring(LookUpControl("/root/Nope") == nil))
            "#,
        ),
    ]);
    f.ui.load_root("Assets/UI/Root");
    let log = f.log();
    assert!(has(&log, "abs true"), "{log:?}");
    assert!(has(&log, "up true"), "{log:?}");
    assert!(has(&log, "none true"), "{log:?}");
    assert!(f.errors().is_empty(), "{:?}", f.errors());
}

#[test]
fn load_new_context_adds_a_child_of_the_caller() {
    let f = fixture(&[
        ("UI/Root.xml", "<Context/>"),
        (
            "UI/Root.lua",
            r#"local c = ContextPtr:LoadNewContext("Extra") print("got " .. tostring(c ~= nil))"#,
        ),
        (
            "UI/Extra.xml",
            r#"<Context><Container ID="E" Size="2,2"/></Context>"#,
        ),
        ("UI/Extra.lua", r#"print("extra ran")"#),
    ]);
    let root = f.ui.load_root("Assets/UI/Root");
    let log = f.log();
    assert!(has(&log, "extra ran") && has(&log, "got true"), "{log:?}");
    assert_eq!(f.ui.shared().st.borrow().node(root).children.len(), 1);
}

#[test]
fn instances_are_built_into_the_parent_and_their_ids_go_to_the_table() {
    let f = fixture(&[
        (
            "UI/Root.xml",
            r#"<Context><Stack ID="S" StackGrowth="Bottom"/><Instance Name="Row"><Container ID="Row" Size="10,5"><Label ID="Text" String="x"/></Container></Instance></Context>"#,
        ),
        (
            "UI/Root.lua",
            r#"
            for i = 1, 3 do
              local inst = {}
              ContextPtr:BuildInstanceForControl("Row", inst, Controls.S)
              print("inst " .. i .. " " .. tostring(inst.Text ~= nil) .. " " .. tostring(inst.Row ~= nil))
            end
            Controls.S:CalculateSize()
            local w, h = Controls.S:GetSizeVal()
            print("stack " .. w .. "x" .. h .. " n=" .. Controls.S:GetNumChildren())
            Controls.S:DestroyAllChildren()
            print("after " .. Controls.S:GetNumChildren())
            "#,
        ),
    ]);
    f.ui.load_root("Assets/UI/Root");
    let log = f.log();
    assert!(has(&log, "inst 3 true true"), "{log:?}");
    assert!(has(&log, "stack 10x15 n=3"), "{log:?}");
    assert!(has(&log, "after 0"), "{log:?}");
    assert!(f.errors().is_empty(), "{:?}", f.errors());
}

#[test]
fn an_empty_context_and_a_broken_one_do_not_hurt_the_neighbour() {
    let f = fixture(&[
        (
            "UI/Root.xml",
            r#"<Context><LuaContext ID="E" FileName="Assets/UI/Empty"/><LuaContext ID="Bad" FileName="Assets/UI/Bad"/><LuaContext ID="Good" FileName="Assets/UI/Good"/></Context>"#,
        ),
        ("UI/Root.lua", ""),
        ("UI/Empty.xml", r#"<?xml version="1.0" encoding="utf-8"?>"#),
        ("UI/Bad.xml", "<Context>"),
        ("UI/Bad.lua", r#"error("boom")"#),
        ("UI/Good.xml", "<Context/>"),
        ("UI/Good.lua", r#"print("good alive")"#),
    ]);
    f.ui.load_root("Assets/UI/Root");
    assert!(has(&f.log(), "good alive"));
    // Only the Lua error is counted; the broken XML is a warning.
    assert_eq!(f.ui.runtime().error_count(), 1, "{:?}", f.errors());
    assert!(f.ui.stats().xml_warnings >= 1);
}

#[test]
fn methods_the_class_does_not_have_are_nil() {
    let f = fixture(&[
        (
            "UI/Root.xml",
            r#"<Context><Image ID="I" Size="4,4"/><Label ID="L" String="a"/><Button ID="B" Size="4,4"/></Context>"#,
        ),
        (
            "UI/Root.lua",
            r#"
            print("image settext " .. tostring(Controls.I.SetText))
            print("label settext " .. type(Controls.L.SetText))
            print("button callback " .. type(Controls.B.RegisterCallback))
            print("label callback " .. tostring(Controls.L.RegisterCallback))
            print("same " .. tostring(Controls.B == Controls.B))
            "#,
        ),
    ]);
    f.ui.load_root("Assets/UI/Root");
    let log = f.log();
    assert!(has(&log, "image settext nil"), "{log:?}");
    assert!(has(&log, "label settext function"));
    assert!(has(&log, "button callback function"));
    assert!(has(&log, "label callback nil"));
    assert!(has(&log, "same true"));
}

#[test]
fn a_call_on_a_destroyed_control_is_a_lua_error_not_a_crash() {
    let f = fixture(&[
        (
            "UI/Root.xml",
            r#"<Context><Container ID="P" Size="4,4"><Container ID="C" Size="1,1"/></Container></Context>"#,
        ),
        (
            "UI/Root.lua",
            r#"
            local c = Controls.C
            Controls.P:DestroyAllChildren()
            print(pcall(function() return c:GetSizeVal() end))
            "#,
        ),
    ]);
    f.ui.load_root("Assets/UI/Root");
    let log = f.log();
    assert!(has(&log, "control destroyed"), "{log:?}");
}

#[test]
fn tool_tip_types_are_global_across_contexts() {
    let f = fixture(&[
        (
            "UI/ToolTips.xml",
            r#"<Context><Container ID="Root" Size="1,1" Hidden="1"><Container ID="Content" Size="1,1"/></Container><Label ID="Text" String=""/><Grid ID="Grid" Size="10,10"/></Context>"#,
        ),
        (
            "UI/ToolTips.lua",
            r#"TTManager:RegisterBasicControls(Controls.Root, Controls.Content, Controls.Text, Controls.Grid)"#,
        ),
        (
            "UI/Other.xml",
            r#"<Context><ToolTipType Name="EUI_X"><Grid ID="Box" Size="30,30"><Label ID="Text" String="t"/></Grid></ToolTipType></Context>"#,
        ),
        ("UI/Other.lua", ""),
        ("UI/User.xml", "<Context/>"),
        (
            "UI/User.lua",
            r#"local t = {} TTManager:GetTypeControlTable("EUI_X", t) print("tt " .. tostring(t.Box ~= nil) .. " " .. tostring(t.Text ~= nil))"#,
        ),
    ]);
    f.ui.load_root("Assets/UI/ToolTips");
    f.ui.load_root("Assets/UI/Other");
    f.ui.load_root("Assets/UI/User");
    let log = f.log();
    assert!(has(&log, "tt true true"), "{log:?}");
    assert!(f.errors().is_empty(), "{:?}", f.errors());
}

#[test]
fn a_type_table_asked_for_before_the_type_is_built_fills_on_its_first_read() {
    // EUI_context's types wait for the basic controls; UnitFlagManager.lua asks for its table at load, before them.
    let mut f = fixture_with(
        &[
            (
                "UI/Other.xml",
                r#"<Context><ToolTipType Name="EUI_X"><Grid ID="Box" Size="30,30"><Label ID="Text" String="t"/></Grid></ToolTipType></Context>"#,
            ),
            ("UI/Other.lua", ""),
            (
                "UI/User.xml",
                r#"<Context><Button ID="B" Size="10,10"/></Context>"#,
            ),
            (
                "UI/User.lua",
                r#"g_t = {} TTManager:GetTypeControlTable("EUI_X", g_t) print("early " .. tostring(rawget(g_t, "Text") ~= nil))
               function Later() print("late " .. tostring(g_t.Box ~= nil) .. " " .. tostring(g_t.Text ~= nil) .. " " .. tostring(g_t.Nope)) end
               Controls.B:RegisterCallback(Mouse.eLClick, Later)"#,
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
    f.ui.load_root("Assets/UI/Other");
    f.ui.load_root("Assets/UI/User");
    f.ui.load_root("Assets/UI/ToolTips");
    f.mv(4.0, 4.0);
    f.click(5.0, 5.0);
    let log = f.log();
    // A type read before the basic controls is built at once (build.rs), so the table is full already at load.
    assert!(has(&log, "early true"), "{log:?}");
    assert!(has(&log, "late true true nil"), "{log:?}");
    assert!(f.errors().is_empty(), "{:?}", f.errors());
}

/// SIMP-exe-955: a removed control is hidden when its context is built and `SetHide(false)` does not show it.
#[test]
fn a_removed_control_stays_hidden() {
    let f = fixture(&[
        (
            "UI/Root.xml",
            r#"<Context><Button ID="Delete" Size="4,4"/><Button ID="Other" Size="4,4"/></Context>"#,
        ),
        (
            "UI/Root.lua",
            r#"
            print("before " .. tostring(Controls.Delete:IsHidden()) .. " " .. tostring(Controls.Other:IsHidden()))
            Controls.Delete:SetHide(false)
            Controls.Other:SetHide(true)
            print("after " .. tostring(Controls.Delete:IsHidden()) .. " " .. tostring(Controls.Other:IsHidden()))
            "#,
        ),
    ]);
    f.ui.set_removed_controls(vec![("Root".to_owned(), "Delete".to_owned())]);
    f.ui.load_root("Assets/UI/Root");
    let log = f.log();
    assert!(has(&log, "before true false"), "{log:?}");
    assert!(has(&log, "after true true"), "{log:?}");
    assert!(f.errors().is_empty(), "{:?}", f.errors());
}
