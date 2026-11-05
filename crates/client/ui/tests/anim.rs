//! Animations (AnimControl::Update and the four classes).
mod support;
use support::harness::*;
use ui::{Kind, State};

fn anim(extra: &str) -> (State, ui::ControlId) {
    let mut st = state();
    let id = build(
        &mut st,
        &format!(r#"<AlphaAnim Size="1,1" AlphaStart="0" AlphaEnd="1" {extra}/>"#),
    );
    (st, id)
}

fn alpha(st: &State, id: ui::ControlId) -> f32 {
    st.node(id).alpha
}

#[test]
fn once_stops_at_the_end_and_calls_back() {
    let (mut st, a) = anim(r#"Cycle="Once" Speed="1""#);
    st.anim_tick(a, 0.5);
    assert!((alpha(&st, a) - 0.5).abs() < 1e-6);
    st.anim_tick(a, 0.6);
    assert_eq!(alpha(&st, a), 1.0);
    let Kind::Anim(x) = &st.node(a).kind else {
        panic!()
    };
    assert!(x.stopped);
    st.anim_tick(a, 1.0);
    assert_eq!(alpha(&st, a), 1.0, "stopped animations do not move");
}

#[test]
fn repeat_restarts_after_the_end_pause() {
    let (mut st, a) = anim(r#"Cycle="Repeat" EndPause="0.5""#);
    st.anim_tick(a, 1.0);
    assert_eq!(alpha(&st, a), 1.0);
    let Kind::Anim(x) = &st.node(a).kind else {
        panic!()
    };
    assert_eq!((x.progress, x.pause_remaining), (0.0, 0.5));
    st.anim_tick(a, 0.25);
    let Kind::Anim(x) = &st.node(a).kind else {
        panic!()
    };
    assert_eq!(
        x.pause_remaining, 0.25,
        "the pause runs before the next pass"
    );
}

#[test]
fn bounce_turns_around_with_the_end_pause() {
    let (mut st, a) = anim(r#"Cycle="Bounce" EndPause="0.2""#);
    st.anim_tick(a, 1.0);
    let Kind::Anim(x) = &st.node(a).kind else {
        panic!()
    };
    assert!(x.reversing && x.pause_remaining == 0.2 && !x.stopped);
    st.anim_tick(a, 0.2);
    st.anim_tick(a, 0.5);
    assert!((alpha(&st, a) - 0.5).abs() < 1e-6, "going back: 1 - 0.5");
    st.anim_tick(a, 0.6);
    let Kind::Anim(x) = &st.node(a).kind else {
        panic!()
    };
    assert!(!x.reversing, "turned again at the start");
}

#[test]
fn one_bounce_goes_there_and_back_then_stops() {
    let (mut st, a) = anim(r#"Cycle="OneBounce""#);
    st.anim_tick(a, 1.0);
    st.anim_tick(a, 1.0);
    let Kind::Anim(x) = &st.node(a).kind else {
        panic!()
    };
    assert!(x.stopped && !x.reversing);
}

#[test]
fn pause_functions_and_reverse() {
    let (mut st, a) = anim(r#"Cycle="Once" Pause="0.5" Function="Square""#);
    st.anim_tick(a, 0.5);
    assert_eq!(alpha(&st, a), 0.0, "paused");
    st.anim_tick(a, 0.5);
    // x^2 at 0.5
    assert!((alpha(&st, a) - 0.25).abs() < 1e-6, "{}", alpha(&st, a));
    st.anim_reverse(a);
    let Kind::Anim(x) = &st.node(a).kind else {
        panic!()
    };
    assert!(x.reversing && (x.progress - 0.5).abs() < 1e-6);
    let (mut st, a) = anim(r#"Cycle="Once" Function="Root""#);
    st.anim_tick(a, 0.25);
    assert!((alpha(&st, a) - 0.5).abs() < 1e-6, "x^(1/2) at 0.25");
}

#[test]
fn to_beginning_to_end_and_stopped_attribute() {
    let (mut st, a) = anim(r#"Stopped="1""#);
    st.anim_tick(a, 5.0);
    assert_eq!(alpha(&st, a), 0.0, "Stopped=1 starts stopped");
    st.anim_play(a);
    st.anim_tick(a, 0.5);
    assert!(alpha(&st, a) > 0.4);
    st.anim_to_end(a);
    assert_eq!(alpha(&st, a), 1.0);
    st.anim_to_beginning(a);
    assert_eq!(alpha(&st, a), 0.0);
}

#[test]
fn slide_moves_the_offset_and_a_hidden_branch_is_not_ticked_by_the_tree_walk() {
    let mut st = state();
    let root = build(
        &mut st,
        r#"<Container Size="100,100"><SlideAnim ID="s" Start="0,0" End="40,20" Cycle="Once"><Container ID="k" Size="5,5"/></SlideAnim></Container>"#,
    );
    let s = find(&st, root, "s");
    st.anim_tick(s, 0.5);
    assert_eq!((st.node(s).offset.x, st.node(s).offset.y), (20.0, 10.0));
    assert_eq!(
        pos(&st, find(&st, root, "k")),
        (0.0, 0.0),
        "children are positioned by their own parent"
    );
}

#[test]
fn flip_picks_frames_by_columns() {
    let mut host = ui::StubHost::default();
    host.add_texture("f.dds", 64, 64);
    let mut st = state_with(host, Default::default());
    let f = build(
        &mut st,
        r#"<FlipAnim Texture="f.dds" Size="16,16" StepSize="16,16" Columns="2" FrameCount="4" Cycle="Once"/>"#,
    );
    st.flip_set_frame(f, 3);
    let Kind::Anim(a) = &st.node(f).kind else {
        panic!()
    };
    let q = a.quad.as_ref().unwrap();
    assert_eq!((q.src_x, q.src_y), (16.0, 16.0));
}

#[test]
fn flip_speed_is_frames_per_second() {
    // The war notification: Speed=20 over 31 frames was read as 20 runs a second and played in 50 ms.
    let mut host = ui::StubHost::default();
    host.add_texture("f.dds", 64, 64);
    let mut st = state_with(host, Default::default());
    let f = build(
        &mut st,
        r#"<FlipAnim Texture="f.dds" Size="16,16" StepSize="16,16" Columns="2" FrameCount="4" Speed="2" Cycle="Once"/>"#,
    );
    let src = |st: &ui::State| {
        let Kind::Anim(a) = &st.node(f).kind else {
            panic!()
        };
        let q = a.quad.as_ref().unwrap();
        (q.src_x, q.src_y)
    };
    // 4 frames at 2 a second: a run of 2 s; half a second in, the second frame shows.
    st.anim_tick(f, 0.5);
    assert_eq!(src(&st), (16.0, 0.0));
    st.anim_tick(f, 0.5);
    assert_eq!(src(&st), (0.0, 16.0));
    st.anim_tick(f, 1.0);
    assert_eq!(src(&st), (16.0, 16.0), "the run ends on the last frame");
}
