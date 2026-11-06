//! The room (lobby-ui 4.4, AC-N4.2): the settings of the game, the slots, "Ready", [Start game], [Leave room],
//! the list of saves for [Load save...].
//!
//! The server is right (LU-D3): the screen shows the last `RoomState` as it came. A change of the host goes out
//! at once (lists and buttons) or when the field is left (text); the screen never pretends it worked. A field
//! that has the keyboard focus keeps what the player typed until it is left.
//!
//! SIMPLIFIED(SIMP-018): part of the Lekmod options are not in this window; no leader portraits. The AI makes
//! research, policy and similar choices for the player and declines AI offers (SIMP-002, SIMP-003); the window
//! says so.

use serde::{Deserialize, Serialize};

use super::UiAction;
use super::rooms::SaveRow;

/// A pick-list entry: the database type and its localized name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Choice {
    pub key: String,
    pub name: String,
}

/// The staging-room tip of one civilization (`CivChoice` of the server), by its `CIVILIZATION_*` key.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CivTipRow {
    pub key: String,
    pub leader: String,
    /// One `"Name: description"` per trait of the leader.
    pub traits: Vec<String>,
    /// One `"Unit: name"` / `"Building: name"` per unique item.
    pub unique: Vec<String>,
}

/// What the host puts into a slot without a human.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SlotKind {
    Open,
    Ai,
    Closed,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TurnModeChoice {
    Simultaneous,
    #[default]
    Hybrid,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimerChoice {
    #[default]
    Off,
    /// A row of the `TurnTimers` table.
    Original(String),
    Fixed(u16),
}

/// One changed field of the settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoomEdit {
    Name(String),
    WorldSize(String),
    MapScript(String),
    Speed(String),
    Era(String),
    Handicap(String),
    MinorCivs(u8),
    TurnMode(TurnModeChoice),
    Timer(TimerChoice),
    Option {
        key: String,
        value: i32,
    },
    Seed(Option<u32>),
    MapSeed(Option<u32>),
    /// A victory condition on or off (`key` is the `VICTORY_*` key of [`VictoryRow`]).
    Victory {
        key: String,
        on: bool,
    },
    /// The turn limit; 0 is "off" (the speed's own limit).
    MaxTurns(u16),
    /// A custom option of the map script (`id` of [`MapOptionRow`]).
    MapOption {
        id: String,
        value: i32,
    },
    /// One of the choices the AI makes for the human (`key` of [`AutoRow`]).
    AutoChoice {
        key: String,
        on: bool,
    },
    /// A mod of the server on or off in the room's set (`id` of [`ModRow`]).
    Mod {
        id: String,
        on: bool,
    },
}

/// A decision the AI makes for the human (SIMP-002): "Advanced".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct AutoRow {
    pub key: String,
    pub name: String,
    pub on: bool,
}

/// Where the room shows a game option (the Lekmod multiplayer screen's grouping).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub enum OptionGroup {
    #[default]
    General,
    /// Under the switch `GAMEOPTION_AI_OPTIONS`.
    Ai,
    /// Under the switch `GAMEOPTION_ADDITIONAL`.
    Additional,
    /// "More".
    Hidden,
}

/// The option that opens the group "AI options".
pub const AI_OPTIONS_SWITCH: &str = "GAMEOPTION_AI_OPTIONS";
/// The option that opens the group "Additional options".
pub const ADDITIONAL_SWITCH: &str = "GAMEOPTION_ADDITIONAL";
/// Highest turn limit the room takes.
pub const MAX_TURNS_LIMIT: u16 = 2000;

/// A game option: a check box (no `values`) or a list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct OptionRow {
    pub key: String,
    pub name: String,
    pub value: i32,
    pub values: Vec<(i32, String)>,
    /// The option's help text (a subtitle: tooltips do not work on phones), may be empty.
    pub help: String,
    pub group: OptionGroup,
}

/// A victory condition: on unless the room turned it off.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct VictoryRow {
    /// `VICTORY_TIME` ...
    pub key: String,
    pub name: String,
    pub on: bool,
}

/// A custom option of the chosen map script: a check box (no `values`) or a list of 1-based values.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct MapOptionRow {
    pub id: String,
    pub name: String,
    pub help: String,
    pub value: i32,
    pub values: Vec<(i32, String)>,
    pub read_only: bool,
}

/// An installed mod the host can pick (T44).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ModRow {
    pub id: String,
    pub title: String,
    pub version: String,
    /// "Gameplay" / "Cosmetic" / "Client".
    pub class: String,
    pub picked: bool,
    /// Players can join without it (cosmetic only).
    pub optional: bool,
    /// Picked but not installed on the server: the host can only take it off.
    pub missing: bool,
}

/// One mod of the resolved load plan.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct PlanRow {
    pub id: String,
    pub version: String,
    pub class: String,
    pub optional: bool,
    /// Pulled in by a dependency, not picked by the host.
    pub added: bool,
    /// Players download an asset pack for it.
    pub pack: bool,
}

/// The mods of the room: what the server has, what the host picked, the resolved plan or why there is none.
/// Empty (nothing installed, nothing picked) means the room shows no mods section at all.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ModsView {
    pub installed: Vec<ModRow>,
    /// In load order.
    pub plan: Vec<PlanRow>,
    /// The server's reasons the set cannot be loaded; the game does not start while there are any.
    pub problems: Vec<String>,
}

impl ModsView {
    pub fn shown(&self) -> bool {
        !(self.installed.is_empty() && self.plan.is_empty() && self.problems.is_empty())
    }
}

/// The settings with the lists to pick from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SettingsView {
    pub name: String,
    pub world_size: String,
    pub map_script: String,
    pub speed: String,
    pub era: String,
    pub handicap: String,
    pub minor_civs: u8,
    /// The most city-states the map size takes.
    pub max_minor: u8,
    /// "88 x 56, 8 players, 16 city-states" for the chosen size.
    pub size_info: String,
    pub turn_mode: TurnModeChoice,
    pub timer: TimerChoice,
    pub options: Vec<OptionRow>,
    pub victories: Vec<VictoryRow>,
    /// The turn limit; 0 is "off".
    pub max_turns: u16,
    /// The custom options of `map_script`.
    pub map_options: Vec<MapOptionRow>,
    pub auto_choices: Vec<AutoRow>,
    pub seed: Option<u32>,
    pub map_seed: Option<u32>,
    pub sizes: Vec<Choice>,
    pub scripts: Vec<Choice>,
    pub speeds: Vec<Choice>,
    pub eras: Vec<Choice>,
    pub handicaps: Vec<Choice>,
    pub timers: Vec<Choice>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum SlotWho {
    Open,
    Human { nick: String, ready: bool, me: bool },
    Ai,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SlotRow {
    pub index: u8,
    pub who: SlotWho,
    /// The civilization key; `None` is "Random".
    pub civ: Option<String>,
    /// A room from a save: the nick that sat here ("was: alice").
    pub was: Option<String>,
    /// The slot is kept for us ("Your civilization").
    pub yours: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct RoomView {
    pub host: String,
    /// We are the host: the settings and the slots can be changed.
    pub me_host: bool,
    /// The game is starting: nothing can be changed any more.
    pub starting: bool,
    pub settings: SettingsView,
    pub slots: Vec<SlotRow>,
    pub civs: Vec<Choice>,
    /// The hover tip of each civilization (leader, traits, unique units and buildings), by the key of `civs`; the
    /// DOM room shows it as a styled tooltip with the game's pictures (P15). Civilizations without one are not
    /// listed. The texts carry the game's markup (`[COLOR_*]`, `[ICON_*]`, `[NEWLINE]`); the page renders it.
    pub civ_tips: Vec<CivTipRow>,
    /// We sit in a slot.
    pub has_slot: bool,
    pub can_start: bool,
    /// "Waiting for: Anna, Bob" / "Need at least two civilizations".
    pub why_not: Option<String>,
    /// "Loaded from <name>, turn T. Players take their own civilizations".
    pub from_save: Option<String>,
    /// A note over the window ("alice is the host now.").
    pub note: Option<String>,
    /// The server's last refusal, red under the buttons.
    pub error: Option<String>,
    /// The saves [Load save...] offers.
    pub saves: Vec<SaveRow>,
    /// The mods of the room (T44).
    pub mods: ModsView,
}

/// What the picker window chooses for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pick {
    Script,
    /// The civilization of a slot of the host (an AI or open slot).
    SlotCiv(u8),
    MyCiv,
}

/// The text the player is typing and the window state, kept between frames.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoomForm {
    name: String,
    seconds: String,
    seed: String,
    map_seed: String,
    max_turns: String,
    picker: Option<Pick>,
    saves_open: bool,
}

const NAME_ID: &str = "room-name";
const SECONDS_ID: &str = "room-seconds";
const SEED_ID: &str = "room-seed";
const MAP_SEED_ID: &str = "room-map-seed";
const MAX_TURNS_ID: &str = "room-max-turns";
/// The fixed timer the radio button starts with.
const DEFAULT_FIXED_S: u16 = 60;
const MIN_FIXED_S: u16 = 10;
const MAX_FIXED_S: u16 = 600;
/// Height of the two columns: fixed, so the window does not jump while its content settles.
const ROOM_COLUMN_H: f32 = 420.0;
const SETTINGS_W: f32 = 380.0;
const SLOTS_W: f32 = 520.0;

impl SettingsView {
    /// The value of an option by key (a missing row is off).
    fn option_value(&self, key: &str) -> i32 {
        self.options
            .iter()
            .find(|o| o.key == key)
            .map_or(0, |o| o.value)
    }

    /// Whether the room lists this option under "Options": the AI and Additional groups only while their
    /// switch is on; the hidden rows are under "More" (see [`OptionGroup`]).
    pub fn option_shown(&self, o: &OptionRow) -> bool {
        match o.group {
            OptionGroup::General => true,
            OptionGroup::Ai => self.option_value(AI_OPTIONS_SWITCH) != 0,
            OptionGroup::Additional => self.option_value(ADDITIONAL_SWITCH) != 0,
            OptionGroup::Hidden => false,
        }
    }
}

fn name_of<'a>(list: &'a [Choice], key: &'a str) -> &'a str {
    list.iter()
        .find(|c| c.key == key)
        .map_or(key, |c| c.name.as_str())
}

/// A text field that sends its value when it is left (Enter, a click elsewhere) and shows the server's value
/// while it does not have the focus. `true` when the player committed a change.
fn text_field(ui: &mut egui::Ui, id: &str, buf: &mut String, current: &str, width: f32) -> bool {
    let id = egui::Id::unique(id);
    if !ui.memory(|m| m.has_focus(id)) && buf != current {
        buf.clear();
        buf.push_str(current);
    }
    let r = ui.add(egui::TextEdit::singleline(buf).id(id).desired_width(width));
    let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
    if r.lost_focus() && escape {
        buf.clear();
        buf.push_str(current);
        return false;
    }
    r.lost_focus() && buf.trim() != current
}

/// A drop-down of a short list; the key picked this frame.
fn combo(ui: &mut egui::Ui, salt: &str, list: &[Choice], current: &str) -> Option<String> {
    let mut picked = None;
    egui::ComboBox::from_id_salt(salt)
        .selected_text(name_of(list, current))
        .width(190.0)
        .show_ui(ui, |ui| {
            for c in list {
                if ui.selectable_label(c.key == current, &c.name).clicked() && c.key != current {
                    picked = Some(c.key.clone());
                }
            }
        });
    picked
}

fn parse_seed(text: &str) -> Result<Option<u32>, ()> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(None);
    }
    t.parse::<u32>().map(Some).map_err(|_| ())
}

fn seed_text(seed: Option<u32>) -> String {
    seed.map_or_else(String::new, |s| s.to_string())
}

pub fn show(ctx: &egui::Context, view: &RoomView, form: &mut RoomForm) -> Vec<UiAction> {
    let mut actions = Vec::new();
    let editable = view.me_host && !view.starting;
    egui::Window::new("Room")
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .collapsible(false)
        .resizable(false)
        .movable(false)
        .fixed_size([920.0, 0.0])
        .show(ctx, |ui| {
            if let Some(n) = &view.note {
                ui.label(n);
            }
            if let Some(f) = &view.from_save {
                ui.colored_label(ui.visuals().warn_fg_color, f);
            }
            // Two columns of their own widths: the slot rows need more room than the settings.
            ui.horizontal_top(|ui| {
                let top_down = egui::Layout::top_down(egui::Align::LEFT);
                ui.allocate_ui_with_layout(egui::vec2(SETTINGS_W, ROOM_COLUMN_H), top_down, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("room-settings")
                        .max_height(ROOM_COLUMN_H)
                        .min_scrolled_height(ROOM_COLUMN_H)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            settings(ui, view, form, editable, &mut actions);
                        });
                });
                ui.separator();
                ui.allocate_ui_with_layout(egui::vec2(SLOTS_W, ROOM_COLUMN_H), top_down, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("room-slots")
                        .max_height(ROOM_COLUMN_H)
                        .min_scrolled_height(ROOM_COLUMN_H)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            slots(ui, view, form, editable, &mut actions);
                        });
                });
            });
            ui.separator();
            footer(ui, view, form, &mut actions);
        });
    picker(ctx, view, form, &mut actions);
    actions
}

fn settings(
    ui: &mut egui::Ui,
    view: &RoomView,
    form: &mut RoomForm,
    editable: bool,
    actions: &mut Vec<UiAction>,
) {
    let s = &view.settings;
    ui.strong("Settings");
    if !view.me_host {
        ui.weak(format!("Only the host ({}) can change them.", view.host));
    }
    egui::Grid::new("room-settings-grid")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("Name");
            if editable {
                if text_field(ui, NAME_ID, &mut form.name, &s.name, 190.0)
                    && !form.name.trim().is_empty()
                {
                    actions.push(UiAction::RoomEdit(RoomEdit::Name(
                        form.name.trim().chars().take(40).collect(),
                    )));
                }
            } else {
                ui.label(&s.name);
            }
            ui.end_row();

            ui.label("Map size");
            if editable {
                if let Some(k) = combo(ui, "room-size", &s.sizes, &s.world_size) {
                    actions.push(UiAction::RoomEdit(RoomEdit::WorldSize(k)));
                }
            } else {
                ui.label(name_of(&s.sizes, &s.world_size));
            }
            ui.end_row();

            ui.label("Map script");
            // A long list: a window, not a drop-down (the scrolled popup was not drawn in the browser).
            if editable {
                if ui
                    .button(name_of(&s.scripts, &s.map_script))
                    .on_hover_text("Pick a map script")
                    .clicked()
                {
                    form.picker = Some(Pick::Script);
                }
            } else {
                ui.label(name_of(&s.scripts, &s.map_script));
            }
            ui.end_row();

            ui.label("Game speed");
            if editable {
                if let Some(k) = combo(ui, "room-speed", &s.speeds, &s.speed) {
                    actions.push(UiAction::RoomEdit(RoomEdit::Speed(k)));
                }
            } else {
                ui.label(name_of(&s.speeds, &s.speed));
            }
            ui.end_row();

            ui.label("Start era");
            if editable {
                if let Some(k) = combo(ui, "room-era", &s.eras, &s.era) {
                    actions.push(UiAction::RoomEdit(RoomEdit::Era(k)));
                }
            } else {
                ui.label(name_of(&s.eras, &s.era));
            }
            ui.end_row();

            ui.label("Difficulty");
            if editable {
                if let Some(k) = combo(ui, "room-handicap", &s.handicaps, &s.handicap) {
                    actions.push(UiAction::RoomEdit(RoomEdit::Handicap(k)));
                }
            } else {
                ui.label(name_of(&s.handicaps, &s.handicap));
            }
            ui.end_row();

            ui.label("City-states");
            ui.horizontal(|ui| {
                if editable {
                    if ui
                        .add_enabled(s.minor_civs > 0, egui::Button::new("-"))
                        .clicked()
                    {
                        actions.push(UiAction::RoomEdit(RoomEdit::MinorCivs(s.minor_civs - 1)));
                    }
                    ui.label(s.minor_civs.to_string());
                    if ui
                        .add_enabled(s.minor_civs < s.max_minor, egui::Button::new("+"))
                        .clicked()
                    {
                        actions.push(UiAction::RoomEdit(RoomEdit::MinorCivs(s.minor_civs + 1)));
                    }
                } else {
                    ui.label(s.minor_civs.to_string());
                }
                ui.weak(format!("of {}", s.max_minor));
            });
            ui.end_row();

            ui.label("Turn mode");
            ui.horizontal(|ui| {
                for (mode, label) in [
                    (TurnModeChoice::Simultaneous, "Simultaneous"),
                    (TurnModeChoice::Hybrid, "Hybrid"),
                ] {
                    let on = s.turn_mode == mode;
                    if ui
                        .add_enabled(editable, egui::RadioButton::new(on, label))
                        .clicked()
                        && !on
                    {
                        actions.push(UiAction::RoomEdit(RoomEdit::TurnMode(mode)));
                    }
                }
            });
            ui.end_row();

            ui.label("Turn timer");
            ui.vertical(|ui| {
                timer(ui, view, form, editable, actions);
            });
            ui.end_row();
        });

    // SIMPLIFIED(SIMP-uic-61): the native room is a plain list of the new fields (the web page has the
    // sections and the groups of the room window).
    ui.add_space(4.0);
    ui.weak(&s.size_info);
    if !s.map_options.is_empty() {
        ui.strong("Map options");
        for o in &s.map_options {
            if o.values.is_empty() {
                let mut on = o.value != 0;
                if ui
                    .add_enabled(
                        editable && !o.read_only,
                        egui::Checkbox::new(&mut on, &o.name),
                    )
                    .changed()
                {
                    actions.push(UiAction::RoomEdit(RoomEdit::MapOption {
                        id: o.id.clone(),
                        value: i32::from(on),
                    }));
                }
            } else {
                ui.horizontal(|ui| {
                    ui.label(&o.name);
                    let current = o
                        .values
                        .iter()
                        .find(|(v, _)| *v == o.value)
                        .map_or("?", |(_, n)| n.as_str());
                    if editable && !o.read_only {
                        egui::ComboBox::from_id_salt(("room-map-option", &o.id))
                            .selected_text(current)
                            .show_ui(ui, |ui| {
                                for (v, n) in &o.values {
                                    if ui.selectable_label(*v == o.value, n).clicked()
                                        && *v != o.value
                                    {
                                        actions.push(UiAction::RoomEdit(RoomEdit::MapOption {
                                            id: o.id.clone(),
                                            value: *v,
                                        }));
                                    }
                                }
                            });
                    } else {
                        ui.label(current);
                    }
                });
            }
        }
    }
    ui.strong("Victory");
    for v in &s.victories {
        let mut on = v.on;
        if ui
            .add_enabled(editable, egui::Checkbox::new(&mut on, &v.name))
            .changed()
        {
            actions.push(UiAction::RoomEdit(RoomEdit::Victory {
                key: v.key.clone(),
                on,
            }));
        }
    }
    ui.horizontal(|ui| {
        ui.label("Max turns");
        if editable {
            if text_field(
                ui,
                MAX_TURNS_ID,
                &mut form.max_turns,
                &max_turns_text(s.max_turns),
                60.0,
            ) && let Ok(v) = form.max_turns.trim().parse::<u16>()
            {
                actions.push(UiAction::RoomEdit(RoomEdit::MaxTurns(
                    v.min(MAX_TURNS_LIMIT),
                )));
            }
        } else {
            ui.label(max_turns_text(s.max_turns));
        }
        ui.weak("0 = the speed's own limit");
    });

    let shown: Vec<&OptionRow> = s.options.iter().filter(|o| s.option_shown(o)).collect();
    let more: Vec<&OptionRow> = s
        .options
        .iter()
        .filter(|o| o.group == OptionGroup::Hidden)
        .collect();
    if !shown.is_empty() {
        ui.add_space(4.0);
        ui.strong("Options");
        for o in shown {
            option_row(ui, o, editable, actions);
        }
    }
    if !more.is_empty() {
        egui::CollapsingHeader::new("More")
            .id_salt("room-more")
            .show(ui, |ui| {
                for o in more {
                    option_row(ui, o, editable, actions);
                }
            });
    }

    ui.add_space(4.0);
    egui::CollapsingHeader::new("Advanced")
        .id_salt("room-advanced")
        .show(ui, |ui| {
            egui::Grid::new("room-seeds")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Seed");
                    let seed_now = seed_text(s.seed);
                    if editable {
                        if text_field(ui, SEED_ID, &mut form.seed, &seed_now, 120.0)
                            && let Ok(v) = parse_seed(&form.seed)
                        {
                            actions.push(UiAction::RoomEdit(RoomEdit::Seed(v)));
                        }
                    } else {
                        ui.label(if seed_now.is_empty() {
                            "random"
                        } else {
                            &seed_now
                        });
                    }
                    ui.end_row();
                    ui.label("Map seed");
                    let map_seed_now = seed_text(s.map_seed);
                    if editable {
                        if text_field(ui, MAP_SEED_ID, &mut form.map_seed, &map_seed_now, 120.0)
                            && let Ok(v) = parse_seed(&form.map_seed)
                        {
                            actions.push(UiAction::RoomEdit(RoomEdit::MapSeed(v)));
                        }
                    } else {
                        ui.label(if map_seed_now.is_empty() {
                            "random"
                        } else {
                            &map_seed_now
                        });
                    }
                    ui.end_row();
                });
            ui.add_space(4.0);
            ui.label("The AI decides for you:");
            for a in &s.auto_choices {
                let mut on = a.on;
                if ui
                    .add_enabled(editable, egui::Checkbox::new(&mut on, &a.name))
                    .changed()
                {
                    actions.push(UiAction::RoomEdit(RoomEdit::AutoChoice {
                        key: a.key.clone(),
                        on,
                    }));
                }
            }
        });

    mods_section(ui, &view.mods, editable, actions);

    ui.add_space(6.0);
    ui.weak(
        "The AI makes research, policy and similar choices for you; AI offers are declined \
         (SIMP-002, SIMP-003).",
    );
}

/// "Mods": the installed ones with check boxes (the host), the resolved plan, the server's reasons it fails.
fn mods_section(ui: &mut egui::Ui, m: &ModsView, editable: bool, actions: &mut Vec<UiAction>) {
    if !m.shown() {
        return;
    }
    ui.add_space(4.0);
    ui.strong("Mods");
    for r in &m.installed {
        let mut on = r.picked;
        let mut label = format!("{} {} ({})", r.title, r.version, r.class);
        if r.optional {
            label.push_str(" - optional for players");
        }
        if r.missing {
            label.push_str(" - not installed on the server");
        }
        if ui
            .add_enabled(editable, egui::Checkbox::new(&mut on, label))
            .changed()
        {
            actions.push(UiAction::RoomEdit(RoomEdit::Mod {
                id: r.id.clone(),
                on,
            }));
        }
    }
    if !m.plan.is_empty() {
        ui.weak("Load order:");
        for (i, p) in m.plan.iter().enumerate() {
            let mut line = format!("{}. {} {}", i + 1, p.id, p.version);
            if p.added {
                line.push_str(" - added by a dependency");
            }
            if p.optional {
                line.push_str(" - optional for players");
            }
            ui.label(line);
        }
    }
    for p in &m.problems {
        ui.colored_label(ui.visuals().error_fg_color, p);
    }
}

/// "off" for no turn limit, else the number.
fn max_turns_text(n: u16) -> String {
    if n == 0 {
        "0".to_owned()
    } else {
        n.to_string()
    }
}

/// One game option: a check box, or a list when the option has values.
fn option_row(ui: &mut egui::Ui, o: &OptionRow, editable: bool, actions: &mut Vec<UiAction>) {
    if o.values.is_empty() {
        let mut on = o.value != 0;
        if ui
            .add_enabled(editable, egui::Checkbox::new(&mut on, &o.name))
            .changed()
        {
            actions.push(UiAction::RoomEdit(RoomEdit::Option {
                key: o.key.clone(),
                value: i32::from(on),
            }));
        }
    } else {
        ui.horizontal(|ui| {
            ui.label(&o.name);
            let current = o
                .values
                .iter()
                .find(|(v, _)| *v == o.value)
                .map_or("?", |(_, n)| n.as_str());
            if editable {
                egui::ComboBox::from_id_salt(("room-option", &o.key))
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        for (v, n) in &o.values {
                            if ui.selectable_label(*v == o.value, n).clicked() && *v != o.value {
                                actions.push(UiAction::RoomEdit(RoomEdit::Option {
                                    key: o.key.clone(),
                                    value: *v,
                                }));
                            }
                        }
                    });
            } else {
                ui.label(current);
            }
        });
    }
}

fn timer(
    ui: &mut egui::Ui,
    view: &RoomView,
    form: &mut RoomForm,
    editable: bool,
    actions: &mut Vec<UiAction>,
) {
    let s = &view.settings;
    let (off, original, fixed) = (
        matches!(s.timer, TimerChoice::Off),
        matches!(s.timer, TimerChoice::Original(_)),
        matches!(s.timer, TimerChoice::Fixed(_)),
    );
    ui.horizontal(|ui| {
        if ui
            .add_enabled(editable, egui::RadioButton::new(off, "Off"))
            .clicked()
            && !off
        {
            actions.push(UiAction::RoomEdit(RoomEdit::Timer(TimerChoice::Off)));
        }
        if !s.timers.is_empty()
            && ui
                .add_enabled(editable, egui::RadioButton::new(original, "Original"))
                .clicked()
            && !original
        {
            actions.push(UiAction::RoomEdit(RoomEdit::Timer(TimerChoice::Original(
                s.timers[0].key.clone(),
            ))));
        }
        if ui
            .add_enabled(editable, egui::RadioButton::new(fixed, "Fixed"))
            .clicked()
            && !fixed
        {
            actions.push(UiAction::RoomEdit(RoomEdit::Timer(TimerChoice::Fixed(
                DEFAULT_FIXED_S,
            ))));
        }
    });
    match &s.timer {
        TimerChoice::Off => {}
        TimerChoice::Original(row) => {
            if editable {
                if let Some(k) = combo(ui, "room-timer", &s.timers, row) {
                    actions.push(UiAction::RoomEdit(RoomEdit::Timer(TimerChoice::Original(
                        k,
                    ))));
                }
            } else {
                ui.label(name_of(&s.timers, row));
            }
        }
        TimerChoice::Fixed(secs) => {
            ui.horizontal(|ui| {
                let now = secs.to_string();
                if editable {
                    if text_field(ui, SECONDS_ID, &mut form.seconds, &now, 60.0)
                        && let Ok(v) = form.seconds.trim().parse::<u16>()
                    {
                        actions.push(UiAction::RoomEdit(RoomEdit::Timer(TimerChoice::Fixed(
                            v.clamp(MIN_FIXED_S, MAX_FIXED_S),
                        ))));
                    }
                } else {
                    ui.label(&now);
                }
                ui.weak(format!("seconds ({MIN_FIXED_S}-{MAX_FIXED_S})"));
            });
        }
    }
}

fn slots(
    ui: &mut egui::Ui,
    view: &RoomView,
    form: &mut RoomForm,
    editable: bool,
    actions: &mut Vec<UiAction>,
) {
    ui.strong("Players");
    egui::Grid::new("room-slots-grid")
        .num_columns(4)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            for s in &view.slots {
                ui.label((u32::from(s.index) + 1).to_string());
                slot_who(ui, view, s, editable, actions);
                slot_civ(ui, view, s, form, editable, actions);
                slot_actions(ui, view, s, editable, actions);
                ui.end_row();
            }
        });
    if editable {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let first_open = view
                .slots
                .iter()
                .find(|s| s.who == SlotWho::Open)
                .map(|s| s.index);
            let last_ai = view
                .slots
                .iter()
                .rev()
                .find(|s| s.who == SlotWho::Ai)
                .map(|s| s.index);
            if ui
                .add_enabled(first_open.is_some(), egui::Button::new("+AI"))
                .clicked()
                && let Some(i) = first_open
            {
                actions.push(set_slot(view, i, SlotKind::Ai));
            }
            if ui
                .add_enabled(last_ai.is_some(), egui::Button::new("-AI"))
                .clicked()
                && let Some(i) = last_ai
            {
                actions.push(set_slot(view, i, SlotKind::Open));
            }
        });
    }
}

/// `SlotSet` for slot `index` with its civilization kept.
fn set_slot(view: &RoomView, index: u8, kind: SlotKind) -> UiAction {
    let civ = view
        .slots
        .iter()
        .find(|s| s.index == index)
        .and_then(|s| s.civ.clone());
    UiAction::SlotSet { index, kind, civ }
}

fn slot_who(
    ui: &mut egui::Ui,
    view: &RoomView,
    s: &SlotRow,
    editable: bool,
    actions: &mut Vec<UiAction>,
) {
    match &s.who {
        SlotWho::Human { nick, ready, me } => {
            ui.horizontal(|ui| {
                ui.label(nick);
                if nick.eq_ignore_ascii_case(&view.host) {
                    ui.weak("host");
                }
                if *me {
                    ui.weak("(you)");
                }
                ui.weak(if *ready { "Ready" } else { "Not ready" });
            });
        }
        who => {
            let current = match who {
                SlotWho::Ai => SlotKind::Ai,
                SlotWho::Closed => SlotKind::Closed,
                _ => SlotKind::Open,
            };
            if editable {
                ui.horizontal(|ui| {
                    for (kind, label) in [
                        (SlotKind::Open, "Open"),
                        (SlotKind::Ai, "AI"),
                        (SlotKind::Closed, "Closed"),
                    ] {
                        if ui.selectable_label(current == kind, label).clicked() && current != kind
                        {
                            actions.push(set_slot(view, s.index, kind));
                        }
                    }
                });
            } else {
                ui.label(match current {
                    SlotKind::Open => "Open",
                    SlotKind::Ai => "AI",
                    SlotKind::Closed => "Closed",
                });
            }
        }
    }
}

fn slot_civ(
    ui: &mut egui::Ui,
    view: &RoomView,
    s: &SlotRow,
    form: &mut RoomForm,
    editable: bool,
    actions: &mut Vec<UiAction>,
) {
    let _ = actions;
    if s.who == SlotWho::Closed {
        ui.label("");
        return;
    }
    let text = s
        .civ
        .as_deref()
        .map_or("Random", |k| name_of(&view.civs, k));
    let mine = matches!(s.who, SlotWho::Human { me: true, .. });
    let host_slot = editable && !matches!(s.who, SlotWho::Human { .. });
    ui.vertical(|ui| {
        ui.set_min_width(110.0);
        if (mine && !view.starting) || host_slot {
            if ui
                .button(text)
                .on_hover_text("Pick a civilization")
                .clicked()
            {
                form.picker = Some(if mine {
                    Pick::MyCiv
                } else {
                    Pick::SlotCiv(s.index)
                });
            }
        } else {
            ui.label(text);
        }
        if let Some(was) = &s.was {
            ui.weak(format!("was: {was}"));
        }
        if s.yours {
            ui.colored_label(ui.visuals().warn_fg_color, "Your civilization");
        }
    });
}

// A button is drawn in the arm; putting its click into a match guard would hide the drawing in a condition.
#[allow(clippy::collapsible_match)]
fn slot_actions(
    ui: &mut egui::Ui,
    view: &RoomView,
    s: &SlotRow,
    editable: bool,
    actions: &mut Vec<UiAction>,
) {
    match &s.who {
        SlotWho::Human {
            me: true, ready, ..
        } => {
            let mut on = *ready;
            if ui
                .add_enabled(!view.starting, egui::Checkbox::new(&mut on, "Ready"))
                .changed()
            {
                actions.push(UiAction::Ready(on));
            }
        }
        SlotWho::Human { me: false, .. } => {
            if editable && ui.button("Remove").clicked() {
                actions.push(set_slot(view, s.index, SlotKind::Open));
            }
        }
        SlotWho::Open => {
            let may_take = !view.starting && (!view.has_slot || s.yours);
            if may_take && ui.button("Take").clicked() {
                actions.push(UiAction::TakeSlot(s.index));
            }
        }
        _ => {}
    }
}

fn footer(ui: &mut egui::Ui, view: &RoomView, form: &mut RoomForm, actions: &mut Vec<UiAction>) {
    ui.horizontal(|ui| {
        if ui
            .add_enabled(!view.starting, egui::Button::new("Leave room"))
            .clicked()
        {
            actions.push(UiAction::LeaveRoom);
        }
        if view.me_host {
            let start = ui.add_enabled(
                view.can_start && !view.starting,
                egui::Button::new("Start game"),
            );
            if start.clicked() {
                actions.push(UiAction::Start);
            }
            if ui
                .add_enabled(!view.starting, egui::Button::new("Load save..."))
                .clicked()
            {
                form.saves_open = !form.saves_open;
            }
        } else {
            ui.weak(format!("Waiting for {} to start the game.", view.host));
        }
        if view.starting {
            ui.weak("Starting the game...");
        }
    });
    if view.me_host
        && let Some(why) = &view.why_not
    {
        ui.weak(why);
    }
    if let Some(e) = &view.error {
        ui.colored_label(ui.visuals().error_fg_color, e);
    }
    if form.saves_open && view.me_host {
        ui.separator();
        if view.saves.is_empty() {
            ui.weak("No saved games.");
        }
        egui::ScrollArea::vertical()
            .id_salt("room-saves")
            .max_height(110.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for s in &view.saves {
                    ui.horizontal(|ui| {
                        ui.label(format!("{} - turn {} - {}", s.name, s.turn, s.saved));
                        ui.weak(&s.civs);
                        if ui.button("Load").clicked() {
                            actions.push(UiAction::LoadSave(s.id.clone()));
                            form.saves_open = false;
                        }
                    });
                }
            });
    }
}

/// The window that lists the long pick lists: map scripts and civilizations.
fn picker(ctx: &egui::Context, view: &RoomView, form: &mut RoomForm, actions: &mut Vec<UiAction>) {
    let Some(target) = form.picker else {
        return;
    };
    let (title, list, current, random): (&str, &[Choice], Option<&str>, bool) = match target {
        Pick::Script => (
            "Pick a map script",
            &view.settings.scripts,
            Some(view.settings.map_script.as_str()),
            false,
        ),
        Pick::MyCiv => (
            "Pick your civilization",
            &view.civs,
            view.slots.iter().find_map(|s| match s.who {
                SlotWho::Human { me: true, .. } => Some(s.civ.as_deref().unwrap_or("")),
                _ => None,
            }),
            true,
        ),
        Pick::SlotCiv(i) => (
            "Pick a civilization",
            &view.civs,
            view.slots
                .iter()
                .find(|s| s.index == i)
                .map(|s| s.civ.as_deref().unwrap_or("")),
            true,
        ),
    };
    let mut close = false;
    egui::Window::new(title)
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .collapsible(false)
        .resizable(false)
        .movable(false)
        .fixed_size([300.0, 0.0])
        .show(ctx, |ui| {
            let mut chosen: Option<Option<String>> = None;
            egui::ScrollArea::vertical()
                .id_salt("room-picker")
                .max_height(320.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    if random && ui.selectable_label(current == Some(""), "Random").clicked() {
                        chosen = Some(None);
                    }
                    for c in list {
                        if ui
                            .selectable_label(current == Some(c.key.as_str()), &c.name)
                            .clicked()
                        {
                            chosen = Some(Some(c.key.clone()));
                        }
                    }
                });
            if ui.button("Cancel").clicked() {
                close = true;
            }
            if let Some(pick) = chosen {
                close = true;
                match (target, pick) {
                    (Pick::Script, Some(key)) => {
                        actions.push(UiAction::RoomEdit(RoomEdit::MapScript(key)));
                    }
                    (Pick::Script, None) => {}
                    (Pick::MyCiv, civ) => actions.push(UiAction::PickCiv(civ)),
                    (Pick::SlotCiv(i), civ) => {
                        let kind = match view.slots.iter().find(|s| s.index == i).map(|s| &s.who) {
                            Some(SlotWho::Ai) => SlotKind::Ai,
                            _ => SlotKind::Open,
                        };
                        actions.push(UiAction::SlotSet {
                            index: i,
                            kind,
                            civ,
                        });
                    }
                }
            }
        });
    if close {
        form.picker = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::{click_at, find_widget, run_headless};

    fn c(k: &str, n: &str) -> Choice {
        Choice {
            key: k.into(),
            name: n.into(),
        }
    }

    fn human(nick: &str, ready: bool, me: bool) -> SlotWho {
        SlotWho::Human {
            nick: nick.into(),
            ready,
            me,
        }
    }

    fn view(me_host: bool) -> RoomView {
        RoomView {
            host: "alice".into(),
            me_host,
            settings: SettingsView {
                name: "Friday".into(),
                world_size: "WORLDSIZE_DUEL".into(),
                map_script: "pangaea".into(),
                speed: "SPEED_STANDARD".into(),
                era: "ERA_ANCIENT".into(),
                handicap: "HANDICAP_PRINCE".into(),
                minor_civs: 1,
                max_minor: 4,
                turn_mode: TurnModeChoice::Hybrid,
                timer: TimerChoice::Off,
                options: vec![
                    OptionRow {
                        key: "GAMEOPTION_NO_BARBARIANS".into(),
                        name: "No barbarians".into(),
                        value: 0,
                        values: vec![],
                        ..OptionRow::default()
                    },
                    OptionRow {
                        key: "GAMEOPTION_X".into(),
                        name: "Other".into(),
                        value: 1,
                        values: vec![(0, "Low".into()), (1, "High".into())],
                        ..OptionRow::default()
                    },
                ],
                victories: vec![VictoryRow {
                    key: "VICTORY_TIME".into(),
                    name: "Time".into(),
                    on: true,
                }],
                max_turns: 0,
                map_options: vec![MapOptionRow {
                    id: "1".into(),
                    name: "World Age".into(),
                    value: 2,
                    values: vec![(1, "Old".into()), (2, "Normal".into())],
                    ..MapOptionRow::default()
                }],
                auto_choices: vec![AutoRow {
                    key: "research".into(),
                    name: "Research".into(),
                    on: true,
                }],
                size_info: "44 x 28, 2 players, 4 city-states".into(),
                seed: None,
                map_seed: Some(7),
                sizes: vec![c("WORLDSIZE_DUEL", "Duel"), c("WORLDSIZE_TINY", "Tiny")],
                scripts: vec![c("pangaea", "Pangaea"), c("continents", "Continents")],
                speeds: vec![c("SPEED_STANDARD", "Standard")],
                eras: vec![c("ERA_ANCIENT", "Ancient")],
                handicaps: vec![c("HANDICAP_PRINCE", "Prince"), c("HANDICAP_KING", "King")],
                timers: vec![c("FAST", "Fast")],
            },
            slots: vec![
                SlotRow {
                    index: 0,
                    who: human("alice", false, me_host),
                    civ: None,
                    was: None,
                    yours: false,
                },
                SlotRow {
                    index: 1,
                    who: if me_host {
                        SlotWho::Open
                    } else {
                        human("bob", false, true)
                    },
                    civ: Some("CIVILIZATION_ROME".into()),
                    was: None,
                    yours: false,
                },
                SlotRow {
                    index: 2,
                    who: SlotWho::Closed,
                    civ: None,
                    was: None,
                    yours: false,
                },
            ],
            civs: vec![
                c("CIVILIZATION_ROME", "Rome"),
                c("CIVILIZATION_EGYPT", "Egypt"),
            ],
            has_slot: true,
            can_start: false,
            why_not: Some("Need at least two civilizations".into()),
            ..RoomView::default()
        }
    }

    fn run(v: &RoomView, f: &mut RoomForm) -> Vec<UiAction> {
        let mut out = Vec::new();
        run_headless(|ctx| out = show(ctx, v, f));
        out
    }

    fn click(label: &str, v: &RoomView) -> Vec<UiAction> {
        let at = find_widget(label, |ctx| show(ctx, v, &mut RoomForm::default()))
            .unwrap_or_else(|| panic!("no widget {label}"));
        click_at(at.center(), false, |ctx| {
            show(ctx, v, &mut RoomForm::default())
        })
    }

    #[test]
    fn renders_for_the_host_and_for_a_guest() {
        for host in [true, false] {
            let v = view(host);
            let mut f = RoomForm::default();
            assert!(!run_headless(|ctx| drop(show(ctx, &v, &mut f))).is_empty());
        }
        let v = view(true);
        assert!(
            find_widget("Need at least two civilizations", |ctx| {
                show(ctx, &v, &mut RoomForm::default())
            })
            .is_some()
        );
        // A guest sees who decides, and no host controls.
        let g = view(false);
        assert!(
            find_widget("Only the host (alice) can change them.", |ctx| {
                show(ctx, &g, &mut RoomForm::default())
            })
            .is_some()
        );
        assert!(
            find_widget("+AI", |ctx| show(ctx, &g, &mut RoomForm::default())).is_none(),
            "no host buttons"
        );
        assert!(find_widget("Start game", |ctx| show(ctx, &g, &mut RoomForm::default())).is_none());
    }

    #[test]
    fn start_is_off_until_the_rule_holds_and_the_reason_is_written() {
        let mut v = view(true);
        assert!(click("Start game", &v).is_empty(), "disabled");
        v.can_start = true;
        v.why_not = None;
        assert_eq!(click("Start game", &v), [UiAction::Start]);
    }

    #[test]
    fn a_guest_readies_and_leaves() {
        let v = view(false);
        assert_eq!(click("Ready", &v), [UiAction::Ready(true)]);
        assert_eq!(click("Leave room", &v), [UiAction::LeaveRoom]);
    }

    #[test]
    fn the_host_changes_slots_and_the_ai_count() {
        let v = view(true);
        assert_eq!(
            click("+AI", &v),
            [UiAction::SlotSet {
                index: 1,
                kind: SlotKind::Ai,
                civ: Some("CIVILIZATION_ROME".into())
            }]
        );
        let mut v = view(true);
        v.slots[1].who = SlotWho::Ai;
        assert_eq!(
            click("-AI", &v),
            [UiAction::SlotSet {
                index: 1,
                kind: SlotKind::Open,
                civ: Some("CIVILIZATION_ROME".into())
            }]
        );
        // Open -> Closed on slot 1 (the "Closed" of the open slot's switch, not the closed slot's).
        let at = find_widget("AI", |ctx| show(ctx, &v, &mut RoomForm::default()));
        assert!(at.is_some(), "the switch of the AI slot");
    }

    #[test]
    fn removing_a_guest_opens_his_slot() {
        let mut v = view(true);
        v.slots[1].who = human("bob", true, false);
        assert_eq!(
            click("Remove", &v),
            [UiAction::SlotSet {
                index: 1,
                kind: SlotKind::Open,
                civ: Some("CIVILIZATION_ROME".into())
            }]
        );
        v.me_host = false;
        assert!(find_widget("Remove", |ctx| show(ctx, &v, &mut RoomForm::default())).is_none());
    }

    #[test]
    fn settings_edits_go_out_as_one_changed_field() {
        let v = view(true);
        assert_eq!(click("+", &v), [UiAction::RoomEdit(RoomEdit::MinorCivs(2))]);
        assert_eq!(click("-", &v), [UiAction::RoomEdit(RoomEdit::MinorCivs(0))]);
        assert_eq!(
            click("Simultaneous", &v),
            [UiAction::RoomEdit(RoomEdit::TurnMode(
                TurnModeChoice::Simultaneous
            ))]
        );
        assert_eq!(
            click("Fixed", &v),
            [UiAction::RoomEdit(RoomEdit::Timer(TimerChoice::Fixed(60)))]
        );
        assert_eq!(
            click("Original", &v),
            [UiAction::RoomEdit(RoomEdit::Timer(TimerChoice::Original(
                "FAST".into()
            )))]
        );
        assert_eq!(
            click("No barbarians", &v),
            [UiAction::RoomEdit(RoomEdit::Option {
                key: "GAMEOPTION_NO_BARBARIANS".into(),
                value: 1
            })]
        );
    }

    #[test]
    fn a_guest_cannot_edit() {
        let v = view(false);
        // The check box and the buttons are there but grey: a click makes nothing.
        assert!(click("No barbarians", &v).is_empty());
        assert!(click("Simultaneous", &v).is_empty());
    }

    #[test]
    fn the_picker_lists_scripts_and_civilizations() {
        let v = view(true);
        // The map script button opens the list; a click on a name sends it.
        let f = RoomForm {
            picker: Some(Pick::Script),
            ..RoomForm::default()
        };
        let at = find_widget("Continents", |ctx| show(ctx, &v, &mut f.clone())).expect("the list");
        let actions = click_at(at.center(), false, |ctx| show(ctx, &v, &mut f.clone()));
        assert_eq!(
            actions,
            [UiAction::RoomEdit(RoomEdit::MapScript("continents".into()))]
        );
        // Our own civilization (a guest).
        let g = view(false);
        let f = RoomForm {
            picker: Some(Pick::MyCiv),
            ..RoomForm::default()
        };
        let at = find_widget("Egypt", |ctx| show(ctx, &g, &mut f.clone())).expect("the list");
        let actions = click_at(at.center(), false, |ctx| show(ctx, &g, &mut f.clone()));
        assert_eq!(
            actions,
            [UiAction::PickCiv(Some("CIVILIZATION_EGYPT".into()))]
        );
        // Random (no other "Random" on the screen: the host's slot has a civilization).
        let mut g = g;
        g.slots[0].civ = Some("CIVILIZATION_ROME".into());
        let at = find_widget("Random", |ctx| show(ctx, &g, &mut f.clone())).expect("random");
        let actions = click_at(at.center(), false, |ctx| show(ctx, &g, &mut f.clone()));
        assert_eq!(actions, [UiAction::PickCiv(None)]);
    }

    #[test]
    fn a_slot_of_the_host_picks_its_civilization_keeping_its_kind() {
        let mut v = view(true);
        v.slots[1].who = SlotWho::Ai;
        let f = RoomForm {
            picker: Some(Pick::SlotCiv(1)),
            ..RoomForm::default()
        };
        let at = find_widget("Egypt", |ctx| show(ctx, &v, &mut f.clone())).expect("the list");
        let actions = click_at(at.center(), false, |ctx| show(ctx, &v, &mut f.clone()));
        assert_eq!(
            actions,
            [UiAction::SlotSet {
                index: 1,
                kind: SlotKind::Ai,
                civ: Some("CIVILIZATION_EGYPT".into())
            }]
        );
    }

    #[test]
    fn a_room_from_a_save_names_it_and_marks_the_slot() {
        let mut v = view(true);
        v.from_save =
            Some("Loaded from Autosave, turn 12. Players take their own civilizations".into());
        v.slots[1].who = SlotWho::Open;
        v.slots[1].was = Some("bob".into());
        v.slots[1].yours = true;
        v.has_slot = false;
        for text in [
            "Loaded from Autosave, turn 12. Players take their own civilizations",
            "was: bob",
            "Your civilization",
        ] {
            assert!(
                find_widget(text, |ctx| show(ctx, &v, &mut RoomForm::default())).is_some(),
                "{text}"
            );
        }
        assert_eq!(click("Take", &v), [UiAction::TakeSlot(1)]);
    }

    #[test]
    fn load_save_lists_the_saves_for_the_host() {
        let mut v = view(true);
        v.saves = vec![SaveRow {
            id: "s1".into(),
            name: "Autosave".into(),
            turn: 12,
            saved: "2026-10-21 14:03 UTC".into(),
            civs: "Rome (alice)".into(),
            game: 1,
            host: "alice".into(),
        }];
        let f = RoomForm {
            saves_open: true,
            ..RoomForm::default()
        };
        let at = find_widget("Load", |ctx| show(ctx, &v, &mut f.clone())).expect("a Load button");
        let actions = click_at(at.center(), false, |ctx| show(ctx, &v, &mut f.clone()));
        assert_eq!(actions, [UiAction::LoadSave("s1".into())]);
    }

    #[test]
    fn a_text_field_keeps_what_is_typed_while_it_has_the_focus() {
        let mut v = view(true);
        let mut f = RoomForm::default();
        run(&v, &mut f);
        assert_eq!(f.name, "Friday", "the server's value fills the field");
        v.settings.name = "Saturday".into();
        run(&v, &mut f);
        assert_eq!(
            f.name, "Saturday",
            "a new snapshot replaces it while it is not edited"
        );
        assert_eq!(f.seed, "");
        assert_eq!(f.map_seed, "", "the section is folded; nothing synced yet");
    }

    #[test]
    fn seeds_parse() {
        assert_eq!(parse_seed(""), Ok(None));
        assert_eq!(parse_seed(" 42 "), Ok(Some(42)));
        assert_eq!(parse_seed("x"), Err(()));
        assert_eq!(parse_seed("-1"), Err(()));
    }

    #[test]
    fn nothing_can_be_changed_once_the_game_is_starting() {
        let mut v = view(true);
        v.starting = true;
        v.can_start = true;
        assert!(click("Start game", &v).is_empty());
        assert!(click("Leave room", &v).is_empty());
        assert!(click("Simultaneous", &v).is_empty());
    }
}
