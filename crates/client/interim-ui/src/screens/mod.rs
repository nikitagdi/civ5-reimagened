//! SIMPLIFIED(SIMP-017): English labels, no localization (the reference names stay localized).
//!
//! Screens are functions `fn show(ctx: &egui::Context, view: &View, ...) -> Vec<UiAction>` over
//! plain view data (no protocol types): they never send commands themselves; `client-app` turns
//! the returned [`UiAction`]s into protocol messages. Layout follows play-ui §3.3: the top bar at
//! the top, the end-turn button bottom right, the minimap bottom left with the journal above it
//! (lobby-ui 4.8; `tests/layout.rs` checks 1280x720), the unit panel bottom centre,
//! the city panel on the right, a yes / no question in the middle; flags and banners under them all.

pub mod chat;
pub mod city_panel;
pub mod confirm;
pub mod conn;
pub mod defeated;
pub mod dom;
pub mod end_turn;
pub mod fatal;
pub mod journal;
pub mod loading;
pub mod lobby;
pub mod login;
pub mod minimap;
pub mod notice;
pub mod perf_panel;
pub mod players;
pub mod room;
pub mod rooms;
pub mod settings;
pub mod tooltip;
pub mod top_bar;
pub mod unit_panel;

use crate::labels;

/// What the player did on a screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiAction {
    /// B1 lobby: start a new game (on a server session: a quick start).
    StartGame(lobby::NewGameChoice),
    /// B1 lobby: continue the saved game.
    ContinueGame,
    // ---- sign-in and the lobby of the server (lobby-ui 5)
    /// "Connect": the nick and the server password.
    Login {
        nick: String,
        password: String,
    },
    /// After `Full`: try again.
    Retry,
    /// Forget the token of this nick and go back to the sign-in window.
    SignOut,
    /// The "OK" of a notice.
    DismissNotice,
    /// "Use this tab" after the game was taken by another tab.
    UseThisTab,
    CreateRoom,
    /// A game against one AI without the room window.
    QuickStart,
    JoinRoom(u32),
    LeaveRoom,
    /// A save of the lobby's list becomes a room.
    LoadSave(String),
    /// "Give up seat" of the seat line: the shell asks first.
    GiveUpSeat,
    /// The answer to that question.
    ConfirmGiveUp(bool),
    /// "Take <civ>" of a game in progress.
    TakeSeat {
        game: u32,
        player: i8,
    },
    // ---- the room
    RoomEdit(room::RoomEdit),
    /// The host sets slot `index`; `civ` `None` is "Random".
    SlotSet {
        index: u8,
        kind: room::SlotKind,
        civ: Option<String>,
    },
    /// Our own civilization; `None` is "Random".
    PickCiv(Option<String>),
    TakeSlot(u8),
    Ready(bool),
    Start,
    /// The end-turn button: like Enter (go to the blocker or end the turn), `force` like
    /// Shift+Enter (Shift+click).
    EndTurn {
        force: bool,
    },
    /// City panel: the build at this index of [`city_panel::CityPanelView::builds`].
    ChooseBuild(usize),
    /// City panel: buy the build at this index with gold, or with faith.
    BuyBuild {
        index: usize,
        faith: bool,
    },
    CloseCity,
    /// Journal: fold or unfold.
    ToggleJournal,
    /// Journal: a click on the line of an event that happened on plot (x, y).
    JournalGoto {
        x: i16,
        y: i16,
    },
    /// Error screen: start again.
    Reload,
    /// Unit panel: the action with this gc-abi-v1 unit command number.
    UnitAction(i16),
    /// A left (or right, `secondary`) click on a unit flag or a city banner standing on `cell`.
    LabelClicked {
        target: labels::LabelTarget,
        cell: (i32, i32),
        secondary: bool,
    },
    /// A city banner's "Strike": the city's ranged attack mode.
    CityRangeAttack(i32),
    /// The answer to the open question ([`confirm`]).
    Confirm(bool),
    // ---- the game of a server session (lobby-ui 4.7); the dangerous ones become a question first
    /// "Cancel end turn".
    CancelEndTurn,
    /// A chat line; `to` is the player, `None` is "All".
    Chat {
        to: Option<i8>,
        text: String,
    },
    /// Players panel: war on the team (the shell asks "Declare war?").
    DeclareWar {
        team: i8,
    },
    ProposePeace {
        to: i8,
    },
    AnswerPeace {
        from: i8,
        accept: bool,
    },
    /// Players panel: remove the player (the shell asks first).
    Kick {
        player: i8,
    },
    /// The host's timer button: `true` stops the timer.
    Pause(bool),
    /// "Leave game" (the settings window): the shell asks first.
    LeaveGame,
    /// Defeated: keep the map, no orders.
    DefeatWatch,
    /// Defeated: back to the lobby.
    DefeatLeave,
    /// Players panel: fold or unfold.
    TogglePlayers,
    /// The journal window: show this tab (and unfold).
    LogTab(journal::LogTab),
    // ---- settings and the minimap (lobby-ui 4.8, 4.9)
    /// The top bar's [Settings] and the window's [Close].
    ToggleSettings,
    /// A quality preset of the settings window.
    SetQuality(settings::QualityPreset),
    /// The render scale slider, as a whole percent (50..=100, steps of 5).
    SetRenderScale(u32),
    /// The size of the shadow map (or none).
    SetShadowQuality(render::shadow::ShadowQuality),
    /// The multisampling of the main pass; in force at the next start.
    SetMsaa(render::gfx_choice::Msaa),
    /// A preset of the graphics choices: the render scale, the shadow quality and the multisampling together.
    SetGfxPreset(render::gfx_choice::GfxPreset),
    /// The terrain detail (Low, Medium, High), independent of the render scale.
    SetTerrainDetail(render::terrain_detail::TerrainDetail),
    /// "Show performance statistics".
    ToggleStats(bool),
    /// The notice of a failed game interface was closed.
    DismissUiNotice,
    /// The performance panel's checkboxes: the draw groups `Main` skips (bit = `render::perf::Group::bit`; 0
    /// draws all). A debug aid, not saved.
    SetPerfGroups(u32),
    /// The performance panel's "Send report" button.
    SendPerfReport,
    /// The minimap was pressed or dragged (`down`) or let go: the position on the picture in
    /// 0..=65535 of its width and height from the top left corner.
    MinimapPointer {
        u: u16,
        v: u16,
        down: bool,
    },
}

/// The screens of one frame, as data; `None` hides one. [`show`] draws them in a fixed order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Screens {
    /// Unit flags and city banners, under every other screen.
    pub labels: Option<labels::MapLabelsView>,
    pub unit: Option<unit_panel::UnitPanelView>,
    /// A yes / no question over the game ("Declare war?").
    pub confirm: Option<confirm::ConfirmView>,
    /// The tooltip over the hovered plot, next to the pointer.
    pub tooltip: Option<tooltip::TooltipView>,
    pub loading: Option<loading::LoadingView>,
    /// The B1 "New game" window (local game only, N4.5).
    pub lobby: Option<lobby::LobbyView>,
    pub top_bar: Option<top_bar::TopBarView>,
    pub city: Option<city_panel::CityPanelView>,
    pub journal: Option<journal::JournalView>,
    /// The players of a server game.
    pub players: Option<players::PlayersView>,
    pub end_turn: Option<end_turn::EndTurnView>,
    /// The `Fatal` message: the error screen over everything.
    pub fatal: Option<String>,
    // ---- server session (lobby-ui 4.1)
    pub login: Option<login::LoginView>,
    /// "Games": rooms, games in progress, saves.
    pub lobby_list: Option<rooms::LobbyListView>,
    pub room: Option<room::RoomView>,
    pub notice: Option<notice::NoticeView>,
    /// The human is eliminated and the game goes on.
    pub defeated: Option<defeated::DefeatedView>,
    /// The strip "No connection..." and the line's dot.
    pub conn: Option<conn::ConnView>,
    /// The minimap panel (bottom left), when there is a source.
    pub minimap: Option<minimap::MinimapView>,
    /// The settings window, while it is open.
    pub settings: Option<settings::SettingsView>,
    /// The performance panel (F3, ?hud=1).
    pub perf: Option<perf_panel::PerfView>,
    /// A game interface failed: what happened and which one shows now.
    pub ui_notice: Option<String>,
}

/// What the player has typed and picked on the screens, kept between frames.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Forms {
    /// The B1 lobby's choices.
    pub lobby: lobby::LobbyForm,
    pub login: login::LoginForm,
    pub room: room::RoomForm,
    /// The chat input and its recipient.
    pub chat: chat::ChatForm,
}

impl Forms {
    pub fn new(lobby: &lobby::LobbyView) -> Self {
        Self {
            lobby: lobby::LobbyForm::new(lobby),
            ..Self::default()
        }
    }
}

/// Draws `screens`; `forms` holds the typed text and the choices between frames.
pub fn show(ctx: &egui::Context, screens: &Screens, forms: &mut Forms) -> Vec<UiAction> {
    let mut actions = Vec::new();
    if let Some(v) = &screens.labels {
        actions.extend(labels::show(ctx, v));
    }
    if let Some(v) = &screens.unit {
        actions.extend(unit_panel::show(ctx, v));
    }
    if let Some(v) = &screens.top_bar {
        actions.extend(top_bar::show(ctx, v));
    }
    if let Some(v) = &screens.loading {
        actions.extend(loading::show(ctx, v));
    }
    if let Some(v) = &screens.lobby {
        actions.extend(lobby::show(ctx, v, &mut forms.lobby));
    }
    if let Some(v) = &screens.lobby_list {
        actions.extend(rooms::show(ctx, v));
    }
    if let Some(v) = &screens.room {
        actions.extend(room::show(ctx, v, &mut forms.room));
    }
    if let Some(v) = &screens.login {
        actions.extend(login::show(ctx, v, &mut forms.login));
    }
    if let Some(v) = &screens.conn {
        actions.extend(conn::show_strip(ctx, v));
    }
    if let Some(v) = &screens.city {
        actions.extend(city_panel::show(ctx, v));
    }
    if let Some(v) = &screens.journal {
        actions.extend(journal::show(ctx, v, &mut forms.chat));
    }
    if let Some(v) = &screens.players {
        actions.extend(players::show(ctx, v));
    }
    if let Some(v) = &screens.end_turn {
        actions.extend(end_turn::show(ctx, v));
    }
    if let Some(v) = &screens.minimap {
        actions.extend(minimap::show(ctx, v));
    }
    if let Some(v) = &screens.settings {
        actions.extend(settings::show(ctx, v));
    }
    if let Some(v) = &screens.perf {
        actions.extend(perf_panel::show(ctx, v));
    }
    if let Some(text) = &screens.ui_notice {
        actions.extend(settings::show_ui_notice(ctx, text));
    }
    if let Some(v) = &screens.confirm {
        actions.extend(confirm::show(ctx, v));
    }
    if let Some(v) = &screens.tooltip {
        actions.extend(tooltip::show(ctx, v));
    }
    if let Some(v) = &screens.defeated {
        actions.extend(defeated::show(ctx, v));
    }
    if let Some(v) = &screens.notice {
        actions.extend(notice::show(ctx, v));
    }
    if let Some(m) = &screens.fatal {
        actions.extend(fatal::show(ctx, m));
    }
    actions
}

/// Runs `f` for two frames in a headless egui context; the shapes of the second (tests).
#[cfg(test)]
pub(crate) fn run_headless(mut f: impl FnMut(&egui::Context)) -> Vec<egui::epaint::ClippedShape> {
    let ctx = egui::Context::default();
    let raw = || egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1280.0, 720.0),
        )),
        ..egui::RawInput::default()
    };
    // Two passes: egui lays out windows in the first one.
    ctx.run_pass(raw(), &mut f).drop_without_applying_deltas();
    let mut out = ctx.run_pass(raw(), &mut f);
    let shapes = std::mem::take(&mut out.shapes);
    out.drop_without_applying_deltas();
    shapes
}

/// Clicks at `at` (points) on the second of three passes and returns the actions of that pass.
#[cfg(test)]
pub(crate) fn click_at(
    at: egui::Pos2,
    shift: bool,
    mut f: impl FnMut(&egui::Context) -> Vec<UiAction>,
) -> Vec<UiAction> {
    let ctx = egui::Context::default();
    let modifiers = egui::Modifiers {
        shift,
        ..egui::Modifiers::default()
    };
    let raw = |events: Vec<egui::Event>| egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1280.0, 720.0),
        )),
        events,
        ..egui::RawInput::default()
    };
    let button = |pressed| egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers,
    };
    let mut all = Vec::new();
    // Three passes to lay the windows out (as ind_widget), then hover, press, release.
    for events in [
        vec![],
        vec![],
        vec![],
        vec![
            egui::Event::ModifiersChanged(modifiers),
            egui::Event::PointerMoved(at),
        ],
        vec![button(true)],
        vec![button(false)],
        vec![],
    ] {
        let out = ctx.run_pass(raw(events), |ctx| all.extend(f(ctx)));
        out.drop_without_applying_deltas();
    }
    all
}

/// The rectangle of the widget labelled `text` after a headless pass of `f` (tests).
#[cfg(test)]
pub(crate) fn find_widget(
    text: &str,
    f: impl FnMut(&egui::Context) -> Vec<UiAction>,
) -> Option<egui::Rect> {
    find_nth_widget(text, 0, f)
}

/// The `n`th (from 0, in drawing order) widget labelled `text`: a label that two rows share.
#[cfg(test)]
pub(crate) fn find_nth_widget(
    text: &str,
    n: usize,
    mut f: impl FnMut(&egui::Context) -> Vec<UiAction>,
) -> Option<egui::Rect> {
    let ctx = egui::Context::default();
    let raw = || egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1280.0, 720.0),
        )),
        ..egui::RawInput::default()
    };
    for _ in 0..3 {
        ctx.run_pass(raw(), |ctx| drop(f(ctx)))
            .drop_without_applying_deltas();
    }
    let out = ctx.run_pass(raw(), |ctx| drop(f(ctx)));
    let found = out
        .shapes
        .iter()
        .filter_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.job.text == text => {
                Some(t.galley.rect.translate(t.pos.to_vec2()))
            }
            _ => None,
        })
        .nth(n);
    out.drop_without_applying_deltas();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_screen_at_once_runs_headless() {
        let lobby = lobby::LobbyView::default();
        let mut form = Forms::new(&lobby);
        let screens = Screens {
            labels: Some(labels::MapLabelsView::default()),
            unit: Some(unit_panel::UnitPanelView::default()),
            confirm: Some(confirm::ConfirmView::default()),
            tooltip: Some(tooltip::TooltipView::default()),
            loading: Some(loading::LoadingView::default()),
            lobby: Some(lobby),
            top_bar: Some(top_bar::TopBarView::default()),
            city: Some(city_panel::CityPanelView::default()),
            journal: Some(journal::JournalView::default()),
            players: Some(players::PlayersView::default()),
            end_turn: Some(end_turn::EndTurnView {
                state: end_turn::EndTurnState::Ready,
                note: Some("Refused: no".into()),
                no_connection: false,
            }),
            fatal: Some("boom".into()),
            login: Some(login::LoginView::default()),
            lobby_list: Some(rooms::LobbyListView::default()),
            room: Some(room::RoomView::default()),
            notice: Some(notice::NoticeView::default()),
            defeated: Some(defeated::DefeatedView::default()),
            conn: Some(conn::ConnView::default()),
            minimap: Some(minimap::MinimapView::default()),
            settings: Some(settings::SettingsView::default()),
            perf: None,
            ui_notice: Some("Interface v2 failed".into()),
        };
        let shapes = run_headless(|ctx| {
            assert!(show(ctx, &screens, &mut form).is_empty());
        });
        assert!(!shapes.is_empty());
        let none = run_headless(|ctx| {
            assert!(show(ctx, &Screens::default(), &mut form).is_empty());
        });
        assert!(none.is_empty());
    }
}
