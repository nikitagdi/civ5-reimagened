//! The layout at 1280x720 (lobby-ui §9): the panels that are open by default of a running server
//! game do not overlap, and they stay on the screen.

use std::sync::Arc;

use interim_ui::screens::chat::ChatView;
use interim_ui::screens::end_turn::{EndTurnState, EndTurnView};
use interim_ui::screens::journal::{JournalView, LogTab};
use interim_ui::screens::minimap::{MinimapImage, MinimapSource, MinimapView};
use interim_ui::screens::players::PlayersView;
use interim_ui::screens::top_bar::{TimerView, TopBarView};
use interim_ui::screens::unit_panel::UnitPanelView;
use interim_ui::screens::{self, Forms, Screens};

const SCREEN: egui::Vec2 = egui::vec2(1280.0, 720.0);

/// The rectangles of the areas that `screens` drew, after the layout settled.
fn rects(screens: &Screens) -> Vec<egui::Rect> {
    let ctx = egui::Context::default();
    let mut forms = Forms::default();
    let raw = || egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN)),
        ..egui::RawInput::default()
    };
    for _ in 0..3 {
        ctx.run_pass(raw(), |ctx| {
            screens::show(ctx, screens, &mut forms);
        })
        .drop_without_applying_deltas();
    }
    ctx.memory(|m| {
        m.areas()
            .visible_layer_ids()
            .into_iter()
            .filter(|l| l.order != egui::Order::Background)
            .filter_map(|l| m.area_rect(l.id))
            .collect()
    })
}

fn game() -> Screens {
    Screens {
        top_bar: Some(TopBarView {
            civ: "Rome".into(),
            timer: Some(TimerView {
                text: "1:23".into(),
                can_pause: true,
                orders_enabled: true,
                ..TimerView::default()
            }),
            line: Some((
                interim_ui::screens::conn::Dot::Green,
                "Online, 35 ms".into(),
            )),
            ..TopBarView::default()
        }),
        journal: Some(JournalView {
            open: true,
            tab: LogTab::Chat,
            chat: Some(ChatView::default()),
            above_minimap: true,
            ..JournalView::default()
        }),
        players: Some(PlayersView::default()),
        end_turn: Some(EndTurnView {
            state: EndTurnState::Ready,
            note: Some("End turn cancelled by your order.".into()),
            no_connection: false,
        }),
        unit: Some(UnitPanelView::default()),
        minimap: Some(MinimapView {
            source: MinimapSource::Image(MinimapImage {
                size: [64, 40],
                rgba: Arc::new(vec![128; 64 * 40 * 4]),
                version: 1,
            }),
            camera_frame: Some([1000, 1000, 9000, 9000]),
        }),
        ..Screens::default()
    }
}

#[test]
fn the_default_panels_do_not_overlap_at_1280_by_720() {
    let all = rects(&game());
    assert!(all.len() >= 6, "every panel was drawn: {all:?}");
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN);
    for (i, a) in all.iter().enumerate() {
        assert!(screen.contains_rect(*a), "{a:?} is off the screen");
        for b in &all[i + 1..] {
            assert!(!a.intersects(*b), "{a:?} overlaps {b:?}");
        }
    }
}

#[test]
fn the_journal_stands_above_the_minimap() {
    let journal_only = |s: Screens| rects(&Screens { minimap: None, ..s });
    let with = rects(&Screens {
        top_bar: None,
        players: None,
        end_turn: None,
        unit: None,
        ..game()
    });
    let without = journal_only(Screens {
        top_bar: None,
        players: None,
        end_turn: None,
        unit: None,
        journal: Some(JournalView {
            above_minimap: false,
            ..game().journal.expect("journal")
        }),
        ..game()
    });
    assert_eq!(with.len(), 2);
    assert_eq!(without.len(), 1);
    let (low, high) = if with[0].min.y > with[1].min.y {
        (with[0], with[1])
    } else {
        (with[1], with[0])
    };
    assert!(high.max.y <= low.min.y, "{high:?} over {low:?}");
    assert!(
        (low.max.y - 708.0).abs() < 4.0,
        "the minimap keeps the corner: {low:?}"
    );
    assert!(without[0].max.y > high.max.y, "without a minimap it drops");
}
