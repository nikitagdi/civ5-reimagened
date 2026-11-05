//! The lobby (lobby-ui 4.3, AC-N4.2, AC-N4.4, AC-N4.5): the window "Games" with three lists (rooms, games in
//! progress, saves), [Create room], [Quick start], and the footer (nick, line, [Sign out]).
//!
//! SIMPLIFIED(SIMP-001): plain tables; original: the multiplayer lobby screens.

use serde::Serialize;

use super::UiAction;
use super::conn::{self, ConnView};

/// A room that is gathering players.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RoomRow {
    pub id: u32,
    pub name: String,
    pub host: String,
    /// "2/4": humans and open or taken slots.
    pub players: String,
    /// "Duel, Pangaea".
    pub map: String,
    /// The worker is starting: "starting...", no [Join].
    pub starting: bool,
}

/// A seat of a game in progress.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SeatRow {
    pub player: i8,
    pub civ: String,
    /// A nick, "AI" or "closed".
    pub who: String,
    pub online: bool,
    /// [Take <civ>] is offered (an AI seat, or our own after we left).
    pub takeable: bool,
    /// It was ours: "(yours)".
    pub yours: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct GameRow {
    pub id: u32,
    pub name: String,
    pub turn: i32,
    pub map: String,
    /// "running", "waking up...", "sleeping (joining wakes it)", "finished".
    pub status: String,
    pub seats: Vec<SeatRow>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SaveRow {
    pub id: String,
    pub name: String,
    pub turn: i32,
    /// "2026-10-21 14:03 UTC" (SIMP-028).
    pub saved: String,
    /// "Rome (alice), Egypt".
    pub civs: String,
    /// The game the save belongs to.
    pub game: u32,
    /// The nick that hosted the game (may be empty).
    pub host: String,
}

/// A signed-in player and where they are (the page's online list).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct OnlineRow {
    pub nick: String,
    /// It is the player looking at the list.
    pub me: bool,
    /// "lobby" | "room" | "game".
    pub place: String,
    /// "In the lobby", "In room <name>", "Playing <name>, turn N".
    pub status: String,
    /// Some(ready) in a room.
    pub ready: Option<bool>,
    /// The host of the room the player is in.
    pub host: bool,
}

/// The seat the player still has in a game (a leave or a drop keeps it): the top line of the window.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct MySeat {
    pub game: u32,
    pub player: i8,
    pub game_name: String,
    pub turn: i32,
    /// [Return] is offered (the game can take us back now).
    pub can_return: bool,
}

/// Whether a refusal of the server says the player has a seat that stands in the way.
pub fn is_seat_error(text: &str) -> bool {
    text.contains("have a seat") || text.contains("your seat")
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct LobbyListView {
    pub rooms: Vec<RoomRow>,
    pub games: Vec<GameRow>,
    pub saves: Vec<SaveRow>,
    /// The server's last refusal, red under the lists for 8 s.
    pub error: Option<String>,
    /// Why the player is here again ("The host removed you from the room").
    pub note: Option<String>,
    pub nick: String,
    pub conn: ConnView,
    /// The seat the player has in a game: "You have a seat in ...: [Return] [Give up seat]".
    pub my_seat: Option<MySeat>,
    /// [Give up seat] was pressed: the question is asked in the seat line.
    pub confirm_give_up: bool,
    /// Everybody who is signed in and where they are; the player first, then the server's order.
    pub online: Vec<OnlineRow>,
}

/// The window never gets smaller than this (points); a screen narrower than that scrolls the content instead.
const MIN_W: f32 = 260.0;
const MIN_H: f32 = 260.0;
/// The window is at most this big however large the screen is: wider tables only spread the columns.
const MAX_W: f32 = 1180.0;
const MAX_H: f32 = 900.0;
/// Below this width the lists are stacked cards instead of table rows.
const COMPACT_BELOW: f32 = 640.0;
/// Room for a section's heading and separator, and for the title bar and frame of the window.
const SECTION_CHROME: f32 = 34.0;
const WINDOW_CHROME_H: f32 = 52.0;
const WINDOW_CHROME_W: f32 = 28.0;
/// Estimated heights of one list row, for sharing the height of the window between the lists.
const ROW_H: f32 = 28.0;
const CARD_H: f32 = 58.0;
const GAME_H: f32 = 64.0;

/// The size of the window's content for a screen: 90 % of it, between a floor and a ceiling, never past the
/// screen (lobby-ui 4.3: the window follows the viewport; a phone gets a full-screen column).
pub fn window_size(screen: egui::Vec2) -> egui::Vec2 {
    let fit = |avail: f32, frac: f32, chrome: f32, min: f32, max: f32| {
        (avail * frac - chrome)
            .clamp(min, max)
            .min((avail - chrome - 8.0).max(120.0))
    };
    egui::vec2(
        fit(screen.x, 0.92, WINDOW_CHROME_W, MIN_W, MAX_W),
        fit(screen.y, 0.90, WINDOW_CHROME_H, MIN_H, MAX_H),
    )
}

/// Divides `avail` between lists that want `want` (0: no list). A list that wants less than its fair share
/// gets what it wants and the others share the rest; a list never gets less than `floor` (it scrolls).
fn share_height(avail: f32, want: [f32; 3], floor: f32) -> [f32; 3] {
    let mut out = [0.0; 3];
    let mut open: Vec<usize> = (0..3).filter(|i| want[*i] > 0.0).collect();
    let mut left = avail.max(0.0);
    loop {
        if open.is_empty() {
            return out;
        }
        let fair = (left / open.len() as f32).max(floor);
        match open.iter().position(|i| want[*i] <= fair) {
            Some(k) => {
                let i = open.remove(k);
                out[i] = want[i];
                left -= want[i];
            }
            None => {
                for i in open {
                    out[i] = fair;
                }
                return out;
            }
        }
    }
}

pub fn show(ctx: &egui::Context, view: &LobbyListView) -> Vec<UiAction> {
    let mut actions = Vec::new();
    let size = window_size(ctx.content_rect().size());
    let compact = size.x < COMPACT_BELOW;
    egui::Window::new("Games")
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .collapsible(false)
        .resizable(false)
        .movable(false)
        .fixed_size(size)
        .show(ctx, |ui| {
            // The footer is pinned to the bottom, the rest of the window is the lists.
            ui.with_layout(egui::Layout::bottom_up(egui::Align::LEFT), |ui| {
                footer(ui, view, &mut actions);
                ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                    header(ui, view, &mut actions);
                    lists(ui, view, compact, &mut actions);
                });
            });
        });
    actions
}

fn header(ui: &mut egui::Ui, view: &LobbyListView, actions: &mut Vec<UiAction>) {
    ui.horizontal_wrapped(|ui| {
        if ui.button("Create room").clicked() {
            actions.push(UiAction::CreateRoom);
        }
        if ui.button("Quick start").clicked() {
            actions.push(UiAction::QuickStart);
        }
        ui.weak("Quick start: a game against one AI, no waiting.");
    });
    if let Some(n) = &view.note {
        ui.label(n);
    }
    if let Some(seat) = &view.my_seat {
        seat_line(ui, seat, view.confirm_give_up, false, actions);
    }
    ui.separator();
}

/// "You have a seat in <game>, turn N: [Return] [Give up seat]"; after [Give up seat] the question instead.
/// `inline`: the line under a refusal that came because of the seat.
fn seat_line(
    ui: &mut egui::Ui,
    seat: &MySeat,
    confirm: bool,
    inline: bool,
    actions: &mut Vec<UiAction>,
) {
    ui.horizontal_wrapped(|ui| {
        if confirm {
            ui.label(format!(
                "Give up your seat in {}? The AI plays it from now on.",
                seat.game_name
            ));
            if ui.button("Yes, give up").clicked() {
                actions.push(UiAction::ConfirmGiveUp(true));
            }
            if ui.button("Keep the seat").clicked() {
                actions.push(UiAction::ConfirmGiveUp(false));
            }
            return;
        }
        if !inline {
            ui.label(format!(
                "You have a seat in {}, turn {}:",
                seat.game_name, seat.turn
            ));
        }
        if seat.can_return && ui.button("Return").clicked() {
            actions.push(UiAction::TakeSeat {
                game: seat.game,
                player: seat.player,
            });
        }
        if ui.button("Give up seat").clicked() {
            actions.push(UiAction::GiveUpSeat);
        }
    });
}

fn footer(ui: &mut egui::Ui, view: &LobbyListView, actions: &mut Vec<UiAction>) {
    ui.horizontal(|ui| {
        ui.label(&view.nick);
        conn::dot(ui, view.conn.dot, &view.conn.text);
        if ui.button("Sign out").clicked() {
            actions.push(UiAction::SignOut);
        }
    });
    ui.separator();
    if let Some(e) = &view.error {
        ui.colored_label(ui.visuals().error_fg_color, e);
        // The refusal came from the seat: the same choice as in the seat line, right here.
        if let Some(seat) = view
            .my_seat
            .as_ref()
            .filter(|_| is_seat_error(e) && !view.confirm_give_up)
        {
            ui.label("Return to your seat or give it up:");
            seat_line(ui, seat, view.confirm_give_up, true, actions);
        }
    }
}

/// The three lists share the height that is left; each scrolls when it is longer than its share.
fn lists(ui: &mut egui::Ui, view: &LobbyListView, compact: bool, actions: &mut Vec<UiAction>) {
    let (row, game) = if compact {
        (CARD_H, CARD_H + 14.0)
    } else {
        (ROW_H, GAME_H)
    };
    let table = |n: usize| match n {
        0 => 0.0,
        n if compact => n as f32 * row,
        n => (n + 1) as f32 * row,
    };
    let want = [
        table(view.rooms.len()),
        view.games.len() as f32 * game,
        table(view.saves.len()),
    ];
    // Empty lists are one weak line.
    let empties = want.iter().filter(|w| **w == 0.0).count() as f32;
    let avail = ui.available_height() - 3.0 * SECTION_CHROME - empties * 20.0;
    let h = share_height(avail, want, 90.0);

    ui.strong("Rooms");
    if view.rooms.is_empty() {
        ui.weak("No rooms yet. Create one.");
    } else {
        list_area(ui, "rooms-list", h[0], |ui| {
            if compact {
                for r in &view.rooms {
                    room_card(ui, r, actions);
                }
            } else {
                rooms_table(ui, view, actions);
            }
        });
    }
    ui.separator();

    ui.strong("Games in progress");
    if view.games.is_empty() {
        ui.weak("No games in progress.");
    } else {
        list_area(ui, "games-list", h[1], |ui| {
            for g in &view.games {
                game_row(ui, g, actions);
            }
        });
    }
    ui.separator();

    ui.strong("Saves");
    if view.saves.is_empty() {
        ui.weak("No saved games.");
    } else {
        list_area(ui, "saves-list", h[2], |ui| {
            if compact {
                for s in &view.saves {
                    save_card(ui, s, actions);
                }
            } else {
                saves_table(ui, view, actions);
            }
        });
    }
}

/// A scrolling list of at most the height it was given (a short one shrinks to its rows).
fn list_area(ui: &mut egui::Ui, id: &str, height: f32, add: impl FnOnce(&mut egui::Ui)) {
    egui::ScrollArea::vertical()
        .id_salt(id)
        .max_height(height)
        .auto_shrink([false, true])
        .show(ui, add);
}

fn rooms_table(ui: &mut egui::Ui, view: &LobbyListView, actions: &mut Vec<UiAction>) {
    egui::Grid::new("rooms")
        .num_columns(5)
        .spacing([16.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            for h in ["Name", "Host", "Players", "Map", ""] {
                ui.weak(h);
            }
            ui.end_row();
            for r in &view.rooms {
                ui.label(&r.name);
                ui.label(&r.host);
                ui.label(&r.players);
                ui.label(&r.map);
                room_action(ui, r, actions);
                ui.end_row();
            }
        });
}

fn room_action(ui: &mut egui::Ui, r: &RoomRow, actions: &mut Vec<UiAction>) {
    if r.starting {
        ui.weak("starting...");
    } else if ui.button("Join").clicked() {
        actions.push(UiAction::JoinRoom(r.id));
    }
}

/// A room as a card (narrow screens): the name, then host, players and map, then the button.
fn room_card(ui: &mut egui::Ui, r: &RoomRow, actions: &mut Vec<UiAction>) {
    ui.horizontal_wrapped(|ui| {
        ui.strong(&r.name);
        ui.label(format!("{} - {} - {}", r.host, r.players, r.map));
        room_action(ui, r, actions);
    });
    ui.add_space(4.0);
}

fn saves_table(ui: &mut egui::Ui, view: &LobbyListView, actions: &mut Vec<UiAction>) {
    egui::Grid::new("saves")
        .num_columns(5)
        .spacing([16.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            for h in ["Name", "Turn", "Saved", "Civilizations", ""] {
                ui.weak(h);
            }
            ui.end_row();
            for s in &view.saves {
                ui.label(&s.name);
                ui.label(s.turn.to_string());
                ui.label(&s.saved);
                ui.label(&s.civs);
                if ui.button("Load").clicked() {
                    actions.push(UiAction::LoadSave(s.id.clone()));
                }
                ui.end_row();
            }
        });
}

/// A save as a card (narrow screens).
fn save_card(ui: &mut egui::Ui, s: &SaveRow, actions: &mut Vec<UiAction>) {
    ui.horizontal_wrapped(|ui| {
        ui.strong(&s.name);
        ui.label(format!("turn {} - {} - {}", s.turn, s.saved, s.civs));
        if ui.button("Load").clicked() {
            actions.push(UiAction::LoadSave(s.id.clone()));
        }
    });
    ui.add_space(4.0);
}

/// One game of the list: its line and a row of seats with [Take <civ>].
fn game_row(ui: &mut egui::Ui, g: &GameRow, actions: &mut Vec<UiAction>) {
    ui.horizontal_wrapped(|ui| {
        ui.strong(&g.name);
        ui.label(format!("turn {}", g.turn));
        ui.label(&g.map);
        ui.weak(&g.status);
    });
    ui.horizontal_wrapped(|ui| {
        ui.add_space(12.0);
        for s in &g.seats {
            let who = if s.yours {
                format!("{}: {} (yours)", s.civ, s.who)
            } else {
                format!("{}: {}", s.civ, s.who)
            };
            ui.label(who);
            if s.takeable && ui.button(format!("Take {}", s.civ)).clicked() {
                actions.push(UiAction::TakeSeat {
                    game: g.id,
                    player: s.player,
                });
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::{click_at, find_widget, run_headless};

    fn view() -> LobbyListView {
        LobbyListView {
            rooms: vec![
                RoomRow {
                    id: 3,
                    name: "Friday".into(),
                    host: "alice".into(),
                    players: "1/4".into(),
                    map: "Duel, Pangaea".into(),
                    starting: false,
                },
                RoomRow {
                    id: 4,
                    name: "Busy".into(),
                    host: "bob".into(),
                    players: "2/2".into(),
                    map: "Duel".into(),
                    starting: true,
                },
            ],
            games: vec![GameRow {
                id: 9,
                name: "Old game".into(),
                turn: 41,
                map: "Small".into(),
                status: "running".into(),
                seats: vec![
                    SeatRow {
                        player: 2,
                        civ: "Rome".into(),
                        who: "AI".into(),
                        online: false,
                        takeable: true,
                        yours: false,
                    },
                    SeatRow {
                        player: 0,
                        civ: "Egypt".into(),
                        who: "alice".into(),
                        online: true,
                        takeable: false,
                        yours: false,
                    },
                ],
            }],
            saves: vec![SaveRow {
                id: "s1".into(),
                name: "Autosave".into(),
                turn: 12,
                saved: "2026-10-21 14:03 UTC".into(),
                civs: "Rome (alice)".into(),
                game: 1,
                host: "alice".into(),
            }],
            error: Some("the room is full".into()),
            note: None,
            nick: "alice".into(),
            conn: ConnView::default(),
            my_seat: None,
            confirm_give_up: false,
            online: vec![],
        }
    }

    fn seated() -> LobbyListView {
        let mut v = view();
        v.error = None;
        v.my_seat = Some(MySeat {
            game: 9,
            player: 0,
            game_name: "Old game".into(),
            turn: 41,
            can_return: true,
        });
        v
    }

    #[test]
    fn a_seat_is_shown_at_the_top_with_return_and_give_up() {
        let v = seated();
        assert!(
            find_widget("You have a seat in Old game, turn 41:", |ctx| show(ctx, &v)).is_some()
        );
        assert_eq!(
            click("Return", &v),
            [UiAction::TakeSeat { game: 9, player: 0 }]
        );
        assert_eq!(click("Give up seat", &v), [UiAction::GiveUpSeat]);
        // No seat, no line.
        assert!(find_widget("Give up seat", |ctx| show(ctx, &view())).is_none());
        // A game that cannot take us back offers only the give-up.
        let mut v = seated();
        v.my_seat.as_mut().unwrap().can_return = false;
        assert!(find_widget("Return", |ctx| show(ctx, &v)).is_none());
        assert!(find_widget("Give up seat", |ctx| show(ctx, &v)).is_some());
    }

    #[test]
    fn giving_up_asks_first() {
        let mut v = seated();
        v.confirm_give_up = true;
        assert!(find_widget("Give up seat", |ctx| show(ctx, &v)).is_none());
        assert_eq!(click("Yes, give up", &v), [UiAction::ConfirmGiveUp(true)]);
        assert_eq!(click("Keep the seat", &v), [UiAction::ConfirmGiveUp(false)]);
    }

    #[test]
    fn the_seat_refusal_offers_the_choice_inline_instead_of_a_bare_line() {
        let mut v = seated();
        v.error = Some("you have a seat in a game".into());
        assert!(find_widget("you have a seat in a game", |ctx| show(ctx, &v)).is_some());
        assert!(find_widget("Return to your seat or give it up:", |ctx| show(ctx, &v)).is_some());
        // The seat line is not duplicated: each button is there twice (the line and the refusal) and works.
        assert_eq!(click("Give up seat", &v), [UiAction::GiveUpSeat]);
        // Another refusal stays a bare line.
        v.error = Some("the room is full".into());
        assert!(find_widget("Return to your seat or give it up:", |ctx| show(ctx, &v)).is_none());
        assert!(is_seat_error("you have a seat in a game"));
        assert!(!is_seat_error("the room is full"));
    }

    fn click(label: &str, v: &LobbyListView) -> Vec<UiAction> {
        let at = find_widget(label, |ctx| show(ctx, v)).unwrap_or_else(|| panic!("no {label}"));
        click_at(at.center(), false, |ctx| show(ctx, v))
    }

    #[test]
    fn renders_the_three_lists() {
        let v = view();
        assert!(!run_headless(|ctx| drop(show(ctx, &v))).is_empty());
        for text in [
            "Friday",
            "Old game",
            "Autosave",
            "the room is full",
            "starting...",
        ] {
            assert!(find_widget(text, |ctx| show(ctx, &v)).is_some(), "{text}");
        }
    }

    #[test]
    fn the_buttons_make_their_actions() {
        let v = view();
        assert_eq!(click("Create room", &v), [UiAction::CreateRoom]);
        assert_eq!(click("Quick start", &v), [UiAction::QuickStart]);
        assert_eq!(click("Join", &v), [UiAction::JoinRoom(3)]);
        assert_eq!(
            click("Take Rome", &v),
            [UiAction::TakeSeat { game: 9, player: 2 }]
        );
        assert_eq!(click("Load", &v), [UiAction::LoadSave("s1".into())]);
        assert_eq!(click("Sign out", &v), [UiAction::SignOut]);
    }

    #[test]
    fn a_room_that_is_starting_has_no_join() {
        let mut v = view();
        v.rooms.remove(0);
        assert!(find_widget("Join", |ctx| show(ctx, &v)).is_none());
    }

    #[test]
    fn only_takeable_seats_have_a_take_button() {
        let v = view();
        assert!(find_widget("Take Egypt", |ctx| show(ctx, &v)).is_none());
    }

    /// The window of `v` on a screen of `screen` points after a few passes.
    fn window_rect(v: &LobbyListView, screen: egui::Vec2) -> egui::Rect {
        let ctx = egui::Context::default();
        let raw = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, screen)),
            ..egui::RawInput::default()
        };
        for _ in 0..3 {
            ctx.run_pass(raw(), |ctx| drop(show(ctx, v)))
                .drop_without_applying_deltas();
        }
        // The window is the only thing on the screen.
        ctx.globally_used_rect()
    }

    fn crowded() -> LobbyListView {
        let mut v = view();
        v.saves = (0..40)
            .map(|i| SaveRow {
                id: format!("s{i}"),
                name: format!("guest{i}'s game"),
                turn: 400 + i,
                saved: "2026-10-28 16:22 UTC".into(),
                civs: "Manchu (guest), Moorish".into(),
                game: 1,
                host: String::new(),
            })
            .collect();
        v
    }

    #[test]
    fn the_window_follows_the_screen_between_a_floor_and_a_ceiling() {
        let big = window_size(egui::vec2(1920.0, 1080.0));
        assert_eq!(big.x, MAX_W);
        assert!(big.y > 900.0 - 1.0 && big.y <= MAX_H);
        let hd = window_size(egui::vec2(1280.0, 720.0));
        assert!(hd.x > 1100.0 && hd.y > 560.0 && hd.y < 650.0, "{hd:?}");
        let phone = window_size(egui::vec2(390.0, 844.0));
        assert!(
            phone.x > 300.0 && phone.x < 390.0 && phone.y > 650.0,
            "{phone:?}"
        );
        let tiny = window_size(egui::vec2(200.0, 150.0));
        assert!(tiny.x <= 200.0 && tiny.y <= 150.0, "{tiny:?}");
    }

    #[test]
    fn lists_share_the_height_and_a_short_one_keeps_only_what_it_needs() {
        // Everything fits.
        assert_eq!(
            share_height(600.0, [60.0, 100.0, 200.0], 90.0),
            [60.0, 100.0, 200.0]
        );
        // The short list stays whole, the long ones split the rest.
        let h = share_height(400.0, [60.0, 500.0, 500.0], 90.0);
        assert_eq!(h[0], 60.0);
        assert!(
            (h[1] - 170.0).abs() < 0.01 && (h[2] - 170.0).abs() < 0.01,
            "{h:?}"
        );
        // No list at all gets nothing; a tiny budget still gives the floor.
        assert_eq!(share_height(300.0, [0.0, 0.0, 0.0], 90.0), [0.0; 3]);
        assert_eq!(
            share_height(10.0, [0.0, 500.0, 500.0], 90.0),
            [0.0, 90.0, 90.0]
        );
    }

    #[test]
    fn the_window_fills_the_screen_on_a_desktop_and_a_phone() {
        for screen in [
            egui::vec2(1920.0, 1080.0),
            egui::vec2(1280.0, 720.0),
            egui::vec2(390.0, 844.0),
            egui::vec2(360.0, 640.0),
        ] {
            let r = window_rect(&crowded(), screen);
            assert!(r.min.x >= 0.0 && r.min.y >= 0.0, "{screen:?} {r:?}");
            assert!(
                r.max.x <= screen.x + 0.5 && r.max.y <= screen.y + 0.5,
                "{screen:?} {r:?}"
            );
            assert!(
                r.width() > screen.x * 0.8 || r.width() > MAX_W - 100.0,
                "{screen:?} {r:?}"
            );
            assert!(r.height() > screen.y * 0.75, "{screen:?} {r:?}");
        }
    }
}
