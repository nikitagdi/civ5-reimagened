//! Input: clicks, hover, consume flags, modal stack, pull-downs, focus, handlers.
mod support;
use support::ctx::*;
use ui::InputResult::{Consumed, PassToWorld};

fn has(lines: &[String], needle: &str) -> bool {
    lines.iter().any(|l| l.contains(needle))
}

fn button_ui(extra: &str, lua: &str) -> Fixture {
    let xml = format!(
        r#"<Context><Button ID="B" Offset="10,10" Size="40,20" Texture="b.dds" {extra}/>
           <Container ID="Other" Offset="100,100" Size="20,20" /></Context>"#
    );
    let f = fixture_with(&[("UI/Root.xml", &xml), ("UI/Root.lua", lua)], enums);
    f.ui.load_root("Assets/UI/Root");
    f.log();
    f
}

#[test]
fn a_click_inside_calls_the_callback_with_void_values_control_and_grab() {
    let mut f = button_ui(
        r#"Void1="7" Void2="9""#,
        r#"Controls.B:RegisterCallback(Mouse.eLClick, function(v1, v2, ctrl, gx, gy)
             print("click " .. v1 .. " " .. v2 .. " " .. tostring(ctrl == Controls.B) .. " " .. gx .. " " .. gy) end)"#,
    );
    f.mv(15.0, 15.0);
    let (d, u) = f.click(25.0, 18.0);
    assert_eq!((d, u), (Consumed, Consumed));
    assert!(has(&f.log(), "click 7 9 true 15 8"), "{:?}", f.errors());
}

#[test]
fn press_inside_release_outside_is_not_a_click() {
    let mut f = button_ui(
        "",
        r#"Controls.B:RegisterCallback(Mouse.eLClick, function() print("click") end)"#,
    );
    f.mv(15.0, 15.0);
    f.ui.handle_input(&ptr(
        platform::PointerPhase::Down,
        15.0,
        15.0,
        Some(platform::MouseButton::Left),
    ));
    f.mv(300.0, 300.0);
    f.ui.handle_input(&ptr(
        platform::PointerPhase::Up,
        300.0,
        300.0,
        Some(platform::MouseButton::Left),
    ));
    assert!(!has(&f.log(), "click"));
    // And a click on empty space goes to the world.
    assert_eq!(f.click(300.0, 300.0), (PassToWorld, PassToWorld));
}

#[test]
fn a_disabled_button_takes_no_click() {
    let mut f = button_ui(
        r#"Disabled="1""#,
        r#"Controls.B:RegisterCallback(Mouse.eLClick, function() print("click") end)"#,
    );
    f.mv(15.0, 15.0);
    f.click(15.0, 15.0);
    assert!(!has(&f.log(), "click"));
}

#[test]
fn sound_only_with_a_handler_and_without_no_default_sound() {
    let count = |extra: &str, lua: &str| {
        let mut f = button_ui(extra, lua);
        f.mv(15.0, 15.0);
        f.click(15.0, 15.0);

        f.sounds.borrow().len()
    };
    let handler = "Controls.B:RegisterCallback(Mouse.eLClick, function() end)";
    assert_eq!(
        count("", handler),
        1,
        "a handler and no mute: one click sound"
    );
    assert_eq!(count("NoDefaultSound=\"1\"", handler), 0, "muted");
    assert_eq!(count("", ""), 0, "no handler, no sound");
}

#[test]
fn hover_shows_the_children_of_show_on_mouse_over() {
    let xml = r#"<Context><Button ID="B" Offset="10,10" Size="40,20" Texture="b.dds">
        <ShowOnMouseOver><Image ID="Glow" Size="5,5"/></ShowOnMouseOver>
        <HideOnMouseOver><Image ID="Dim" Size="5,5"/></HideOnMouseOver></Button></Context>"#;
    let mut f = fixture_with(&[("UI/Root.xml", xml), ("UI/Root.lua", "")], enums);
    f.ui.load_root("Assets/UI/Root");
    let hidden = |f: &Fixture, id: &str| {
        f.ui.shared()
            .st
            .borrow()
            .node(f.find(id))
            .has(ui::flags::MOUSE_OVER_HIDDEN)
    };
    assert!(hidden(&f, "Glow") && !hidden(&f, "Dim"));
    f.mv(15.0, 15.0);
    assert!(!hidden(&f, "Glow") && hidden(&f, "Dim"));
    f.mv(300.0, 300.0);
    assert!(hidden(&f, "Glow") && !hidden(&f, "Dim"));
}

#[test]
fn mouse_enter_and_exit_callbacks() {
    let mut f = button_ui(
        "",
        r#"Controls.B:RegisterCallback(Mouse.eMouseEnter, function() print("enter") end)
           Controls.B:RegisterCallback(Mouse.eMouseExit, function() print("exit") end)"#,
    );
    f.mv(15.0, 15.0);
    assert!(has(&f.log(), "enter"));
    f.mv(300.0, 15.0);
    assert!(has(&f.log(), "exit"));
}

/// CityView's queue reorder (EUI `SwapQueueItem`): a slider lies over the queue items; pressing its shuttle and moving over
/// another item fires that item's `eMouseEnter` while `IsTrackingLeftMouseButton` of the slider is true (player report
/// 2026-10-31: the drag did nothing, the call was false for a slider).
#[test]
fn a_slider_drag_over_buttons_is_tracked_and_they_get_mouse_enter() {
    let xml = r#"<Context>
        <Stack ID="Q" StackGrowth="Bottom">
          <Button ID="A" Size="100,38" Texture="b.dds"/>
          <Button ID="B" Size="100,38" Texture="b.dds"/>
        </Stack>
        <Slider ID="S" Vertical="1" Texture="b.dds" Length="40" Size="100,38">
          <Shuttle Vertical="1" Texture="b.dds" Size="100,38"/>
        </Slider></Context>"#;
    let lua = r#"
        local function enter(i) print("enter " .. i .. " " .. tostring(Controls.S:IsTrackingLeftMouseButton())) end
        Controls.A:SetVoid1(0) Controls.A:RegisterCallback(Mouse.eMouseEnter, enter)
        Controls.B:SetVoid1(1) Controls.B:RegisterCallback(Mouse.eMouseEnter, enter)"#;
    let mut f = fixture_with(&[("UI/Root.xml", xml), ("UI/Root.lua", lua)], enums);
    f.ui.load_root("Assets/UI/Root");
    f.log();
    f.mv(10.0, 10.0);
    assert!(has(&f.log(), "enter 0 false"), "{:?}", f.errors());
    f.ui.handle_input(&ptr(
        platform::PointerPhase::Down,
        10.0,
        10.0,
        Some(platform::MouseButton::Left),
    ));
    f.mv(10.0, 50.0);
    assert!(has(&f.log(), "enter 1 true"), "{:?}", f.errors());
    f.ui.handle_input(&ptr(
        platform::PointerPhase::Up,
        10.0,
        50.0,
        Some(platform::MouseButton::Left),
    ));
    f.mv(10.0, 10.0);
    assert!(has(&f.log(), "enter 0 false"), "released: not tracking");
}

#[test]
fn consume_mouse_takes_the_event_without_a_handler() {
    let xml = r#"<Context><Container ID="Panel" Size="50,50" ConsumeMouse="1"/></Context>"#;
    let mut f = fixture_with(&[("UI/Root.xml", xml), ("UI/Root.lua", "")], enums);
    f.ui.load_root("Assets/UI/Root");
    assert_eq!(f.click(10.0, 10.0), (Consumed, Consumed));
    assert_eq!(f.click(80.0, 80.0), (PassToWorld, PassToWorld));
}

#[test]
fn a_context_input_handler_decides_and_unhandled_input_goes_to_the_world() {
    let xml = "<Context/>";
    let lua = r#"ContextPtr:SetInputHandler(function(msg, w, l)
        if msg == KeyEvents.KeyDown and w == Keys.VK_ESCAPE then print("esc") return true end
        return false end)"#;
    let mut f = fixture_with(&[("UI/Root.xml", xml), ("UI/Root.lua", lua)], enums);
    f.ui.load_root("Assets/UI/Root");
    let key = |code: platform::NamedKey| {
        platform::InputEvent::Key(platform::KeyEvent {
            key: platform::Key::Named(code),
            code: None,
            pressed: true,
            repeat: false,
            modifiers: platform::Modifiers::default(),
        })
    };
    assert_eq!(
        f.ui.handle_input(&key(platform::NamedKey::Escape)),
        Consumed
    );
    assert!(has(&f.log(), "esc"));
    assert_eq!(
        f.ui.handle_input(&key(platform::NamedKey::Tab)),
        PassToWorld
    );
}

#[test]
fn the_off_screen_release_of_a_camera_drag_never_reaches_a_lua_input_handler() {
    let xml = "<Context/>";
    let lua = r#"ContextPtr:SetInputHandler(function(msg, w, l)
        if msg == MouseEvents.LButtonUp then print("up") return true end
        return false end)"#;
    let mut f = fixture_with(&[("UI/Root.xml", xml), ("UI/Root.lua", lua)], enums);
    f.ui.load_root("Assets/UI/Root");
    let left = Some(platform::MouseButton::Left);
    f.ui.handle_input(&ptr(platform::PointerPhase::Down, 50.0, 50.0, left));
    f.ui.handle_input(&ptr(platform::PointerPhase::Up, -1.0, -1.0, left));
    assert!(!has(&f.log(), "up"), "a drag release selected a unit");
    // A click that stayed put still reaches WorldView's handler.
    assert_eq!(f.click(50.0, 50.0).1, Consumed);
    assert!(has(&f.log(), "up"));
}

#[test]
fn modal_blocks_everything_below_and_pop_restores() {
    let xml = r#"<Context><Button ID="B" Offset="10,10" Size="40,20" Texture="b.dds"/>
        <Container ID="M" Offset="200,200" Size="50,50"/></Context>"#;
    let lua = r#"Controls.B:RegisterCallback(Mouse.eLClick, function() print("under") end)
                 UIManager:PushModal(Controls.M)"#;
    let mut f = fixture_with(&[("UI/Root.xml", xml), ("UI/Root.lua", lua)], enums);
    f.ui.load_root("Assets/UI/Root");
    f.mv(15.0, 15.0);
    f.click(15.0, 15.0);
    assert!(!has(&f.log(), "under"), "the modal blocks the button");
    let m = f.find("M");
    f.ui.shared().run(|st| st.pop_modal(m));
    f.mv(16.0, 16.0);
    f.click(16.0, 16.0);
    assert!(has(&f.log(), "under"));
}

/// MPList's kick (live report 2026-10-30): `UIManager:PushModal(Controls.ConfirmKick, true)` on a child LuaContext declared
/// `Hidden="1"` must show it; a hidden modal drew nothing and swallowed every click (the UI looked frozen).
#[test]
fn a_hidden_child_context_pushed_modal_is_shown_and_takes_the_clicks() {
    let root_xml = r#"<Context><Button ID="B" Offset="10,10" Size="40,20" Texture="b.dds"/>
        <LuaContext ID="Confirm" FileName="Assets/UI/Confirm" Hidden="1"/></Context>"#;
    let root_lua = r#"Controls.B:RegisterCallback(Mouse.eLClick, function()
            print("under")
            UIManager:PushModal(Controls.Confirm, true)
        end)"#;
    let kid_xml =
        r#"<Context><Button ID="Ok" Offset="100,100" Size="40,20" Texture="b.dds"/></Context>"#;
    let kid_lua = r#"ContextPtr:SetShowHideHandler(function(hide, init) if not init then print("confirm hide " .. tostring(hide) .. " hidden " .. tostring(ContextPtr:IsHidden())) end end)
        Controls.Ok:RegisterCallback(Mouse.eLClick, function()
            print("accepted")
            UIManager:PopModal(ContextPtr)
            ContextPtr:SetHide(true)
        end)"#;
    let mut f = fixture_with(
        &[
            ("UI/Root.xml", root_xml),
            ("UI/Root.lua", root_lua),
            ("UI/Confirm.xml", kid_xml),
            ("UI/Confirm.lua", kid_lua),
        ],
        enums,
    );
    f.ui.load_root("Assets/UI/Root");
    f.mv(15.0, 15.0);
    f.click(15.0, 15.0);
    f.ui.update(0.016);
    let log = f.log();
    // Shown, not only told so: a modal left hidden is not drawn (draw_all skips it) while it still takes every click.
    assert!(
        has(&log, "under") && has(&log, "confirm hide false hidden false"),
        "{log:?}"
    );
    f.mv(110.0, 110.0);
    f.click(110.0, 110.0);
    f.ui.update(0.016);
    let log = f.log();
    assert!(
        has(&log, "accepted"),
        "the modal's button takes the click: {log:?}"
    );
    // Popped and hidden: the button below answers again.
    f.mv(16.0, 16.0);
    f.click(16.0, 16.0);
    assert!(has(&f.log(), "under"));
}

#[test]
fn popup_queue_orders_by_priority_and_shows_the_head() {
    let xml = r#"<Context><Container ID="A" Size="5,5" Hidden="1"/><Container ID="B" Size="5,5" Hidden="1"/><Container ID="C" Size="5,5" Hidden="1"/></Context>"#;
    let mut f = fixture_with(&[("UI/Root.xml", xml), ("UI/Root.lua", "")], enums);
    f.ui.load_root("Assets/UI/Root");
    let (a, b, c) = (f.find("A"), f.find("B"), f.find("C"));
    let sh = f.ui.shared().clone();
    sh.run(|st| st.queue_popup(a, 10));
    assert_eq!(sh.st.borrow().popup_current, Some(a));
    sh.run(|st| st.queue_popup(b, 20));
    assert_eq!(
        sh.st.borrow().popup_current,
        Some(b),
        "higher priority takes over"
    );
    sh.run(|st| st.queue_popup(c, 5));
    assert_eq!(sh.st.borrow().popup_current, Some(b));
    sh.run(|st| st.dequeue_popup(b));
    assert_eq!(
        sh.st.borrow().popup_current,
        Some(a),
        "the next by priority"
    );
    sh.run(|st| st.dequeue_popup(a));
    assert_eq!(sh.st.borrow().popup_current, Some(c));
    let _ = &mut f;
}

#[test]
fn wheel_over_a_scroll_panel_moves_it_by_the_step() {
    let xml = r#"<Context><ScrollPanel ID="P" Size="100,50" Vertical="1"><Container Size="100,200"/></ScrollPanel></Context>"#;
    let mut f = fixture_with(&[("UI/Root.xml", xml), ("UI/Root.lua", "")], enums);
    f.ui.load_root("Assets/UI/Root");
    let w = platform::InputEvent::Wheel(platform::WheelEvent {
        delta: platform::WheelDelta::Lines { x: 0.0, y: -1.0 },
        position: platform::PhysicalPosition { x: 10.0, y: 10.0 },
        modifiers: platform::Modifiers::default(),
    });
    assert_eq!(f.ui.handle_input(&w), Consumed);
    let p = f.find("P");
    let v = match &f.ui.shared().st.borrow().node(p).kind {
        ui::Kind::ScrollPanel(s) => (s.value, s.step),
        _ => unreachable!(),
    };
    assert!((v.0 - v.1).abs() < 1e-6, "one step down: {v:?}");
}

#[test]
fn tab_walks_focus_stops_and_wraps() {
    let xml = r#"<Context><EditBox ID="E1" FocusStop="1" Size="50,20"/><EditBox ID="E2" FocusStop="2" Size="50,20" Offset="0,30"/></Context>"#;
    let mut f = fixture_with(&[("UI/Root.xml", xml), ("UI/Root.lua", "")], enums);
    f.ui.load_root("Assets/UI/Root");
    let (e1, e2) = (f.find("E1"), f.find("E2"));
    let sh = f.ui.shared().clone();
    sh.st.borrow_mut().focus = Some(e1);
    let tab = || {
        platform::InputEvent::Key(platform::KeyEvent {
            key: platform::Key::Named(platform::NamedKey::Tab),
            code: None,
            pressed: true,
            repeat: false,
            modifiers: platform::Modifiers::default(),
        })
    };
    // ControlBase::FindNextFocus(forward): the nearest stop below the limit, wrapping.
    let order: Vec<_> = (0..3)
        .map(|_| {
            f.ui.handle_input(&tab());
            sh.st.borrow().focus
        })
        .collect();
    assert_eq!(order.len(), 3);
    assert!(order.iter().all(|x| x.is_some()));
    assert_ne!(order[0], Some(e1), "focus moved");
    assert!(order.contains(&Some(e2)));
}

#[test]
fn pull_down_opens_as_a_modal_and_selection_calls_back() {
    let xml = r#"<Context><PullDown ID="PD" Offset="10,10" Size="100,24" ScrollThreshold="100">
        <ButtonData><Button Size="100,24" Texture="b.dds"/></ButtonData>
        <GridData Style="Nope"/>
        <ScrollPanelData Size="100,100" Vertical="1"/>
        <StackData StackGrowth="Bottom"/>
        <InstanceData Name="Entry"><Button Size="100,20" Texture="b.dds"/></InstanceData></PullDown></Context>"#;
    let lua = r#"
        local t = {}
        Controls.PD:RegisterSelectionCallback(function(v1, v2, ctrl) print("selected " .. v1 .. " " .. v2) end)
        local e = Controls.PD:BuildEntry("Entry", t)
        e:SetVoids(4, 5)
        Controls.PD:CalculateInternals()
    "#;
    let mut f = fixture_with(&[("UI/Root.xml", xml), ("UI/Root.lua", lua)], enums);
    f.ui.load_root("Assets/UI/Root");
    assert!(f.errors().is_empty(), "{:?}", f.errors());
    let pd = f.find("PD");
    f.mv(20.0, 15.0);
    f.click(20.0, 15.0);
    assert!(
        f.ui.shared().st.borrow().is_modal(pd),
        "the click on the main button opens it"
    );
    // Click on the first entry, below the main button.
    f.mv(20.0, 10.0 + 24.0 + 5.0);
    f.click(20.0, 10.0 + 24.0 + 5.0);
    assert!(
        has(&f.log(), "selected 4 5"),
        "{:?} {:?}",
        f.log(),
        f.errors()
    );
    assert!(
        !f.ui.shared().st.borrow().is_modal(pd),
        "selecting closes it"
    );
}

#[test]
fn the_screen_rect_of_a_control_is_where_a_click_lands() {
    // A button in a stack inside a container: the rectangle follows the stack's flow, and a click in its middle
    // reaches the button (the playtests click controls by this rectangle).
    let xml = r#"<Context><Container ID="C" Offset="30,40" Size="200,200">
           <Stack ID="S" Offset="5,6" StackGrowth="Bottom" Padding="4">
             <Container ID="A" Size="50,10"/>
             <Button ID="B" Size="40,20" Texture="b.dds"/>
           </Stack></Container></Context>"#;
    let lua = r#"Controls.B:RegisterCallback(Mouse.eLClick, function() print("click") end)
        local x, y, w, h, shown = unpack(__ui_screen_rect(Controls.B))
        print(string.format("rect %d %d %d %d %d", x, y, w, h, shown))
        print((__ui_dump_screen("/B") or ""):gsub("\t", " "))"#;
    let mut f = fixture_with(&[("UI/Root.xml", xml), ("UI/Root.lua", lua)], enums);
    f.ui.load_root("Assets/UI/Root");
    let log = f.log();
    let rect = log
        .iter()
        .find_map(|l| l.strip_prefix("rect "))
        .unwrap_or_else(|| panic!("{log:?} {:?}", f.errors()));
    let v: Vec<f32> = rect.split(' ').map(|n| n.parse().unwrap()).collect();
    assert_eq!((v[2], v[3], v[4]), (40.0, 20.0, 1.0), "{rect}");
    assert!(has(&log, "/B Button"), "{log:?}");
    let (cx, cy) = (f64::from(v[0] + v[2] / 2.0), f64::from(v[1] + v[3] / 2.0));
    f.mv(cx, cy);
    f.click(cx, cy);
    assert!(has(&f.log(), "click"), "rect {rect}: {:?}", f.errors());
}
