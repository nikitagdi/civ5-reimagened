//! City window, right side: the city's numbers (population, growth, HP, the six yields), its
//! current build with progress, what it can build (`CityBuilds`) in the sections "Units",
//! "Buildings", "Wonders", "Other" with turns, and the purchase buttons (gold, faith). A click on
//! a build orders it; a purchase button buys it.
//!
//! SIMPLIFIED(SIMP-001): a window of plain widgets instead of the CityView / ProductionPopup Lua
//! screens; the core does not report tiles worked, specialists or the order queue (only its head),
//! so those are left out.

use super::UiAction;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildSection {
    Units,
    Buildings,
    Wonders,
    Other,
}

impl BuildSection {
    pub const ALL: [BuildSection; 4] = [
        BuildSection::Units,
        BuildSection::Buildings,
        BuildSection::Wonders,
        BuildSection::Other,
    ];

    fn title(self) -> &'static str {
        match self {
            BuildSection::Units => "Units",
            BuildSection::Buildings => "Buildings",
            BuildSection::Wonders => "Wonders",
            BuildSection::Other => "Other",
        }
    }
}

/// A purchase the core offers for a build: the price and whether the core accepts it now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Offer {
    pub cost: i32,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildLine {
    pub section: BuildSection,
    pub name: String,
    /// Turns to build; negative when unknown.
    pub turns: i16,
    pub current: bool,
    /// Production needed (0 for processes).
    pub cost: i32,
    pub gold: Option<Offer>,
    pub faith: Option<Offer>,
}

impl BuildLine {
    pub fn text(&self) -> String {
        let current = if self.current { " (current)" } else { "" };
        if self.turns >= 0 {
            format!("{}: {} turns{current}", self.name, self.turns)
        } else {
            format!("{}{current}", self.name)
        }
    }
}

/// The current build of the city.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Producing {
    pub name: String,
    pub stored: i32,
    pub needed: i32,
    /// Negative when never.
    pub turns: i16,
}

impl Producing {
    pub fn text(&self) -> String {
        let turns = if self.turns >= 0 {
            format!(", {} turns", self.turns)
        } else {
            String::new()
        };
        format!(
            "Producing {} ({}/{}{turns})",
            self.name, self.stored, self.needed
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CityPanelView {
    pub name: String,
    /// "Population 3, grows in 5 turns, HP 20/20".
    pub summary: String,
    /// Per turn: food, production, gold, science, culture, faith.
    pub yields: Vec<(String, i16)>,
    /// `None`: nothing in production.
    pub producing: Option<Producing>,
    /// What the city can build, in the core's order; the action index points here.
    pub builds: Vec<BuildLine>,
    /// The answer to `CityBuilds` has not come yet.
    pub loading: bool,
    /// The empire's gold and faith, for the hints of disabled purchase buttons.
    pub gold: i32,
    pub faith: i32,
    /// The core's refusal of the last order, shown until the next world.
    pub refused: Option<String>,
    /// The line is down (lobby-ui LU-D19): choosing and buying are grey, "No connection".
    pub no_connection: bool,
}

fn yield_color(name: &str) -> egui::Color32 {
    match name {
        "food" => egui::Color32::from_rgb(140, 210, 110),
        "production" => egui::Color32::from_rgb(220, 160, 90),
        "gold" => egui::Color32::from_rgb(255, 210, 80),
        "science" => egui::Color32::from_rgb(110, 170, 255),
        "culture" => egui::Color32::from_rgb(210, 120, 255),
        _ => egui::Color32::from_rgb(230, 230, 230),
    }
}

const NO_CONNECTION: &str = "No connection";

#[allow(clippy::too_many_arguments)]
fn buy_button(
    ui: &mut egui::Ui,
    offer: Offer,
    unit: &str,
    have: i32,
    index: usize,
    faith: bool,
    connected: bool,
    actions: &mut Vec<UiAction>,
) {
    let r = ui.add_enabled(
        offer.enabled && connected,
        egui::Button::new(format!("Buy {} {unit}", offer.cost)),
    );
    if r.clicked() {
        r.surrender_focus();
        actions.push(UiAction::BuyBuild { index, faith });
    }
    if !connected {
        r.on_disabled_hover_text(NO_CONNECTION);
    } else if !offer.enabled {
        let why = if have < offer.cost {
            format!("Not enough {unit}: {have} of {}", offer.cost)
        } else {
            format!("Cannot be bought with {unit} now")
        };
        r.on_disabled_hover_text(why);
    }
}

pub fn show(ctx: &egui::Context, view: &CityPanelView) -> Vec<UiAction> {
    let mut actions = Vec::new();
    let mut open = true;
    egui::Window::new(&view.name)
        .id(egui::Id::unique("city-panel"))
        .anchor(egui::Align2::RIGHT_TOP, [-12.0, 40.0])
        .collapsible(false)
        .resizable(false)
        .movable(false)
        .open(&mut open)
        .default_width(380.0)
        .show(ctx, |ui| {
            ui.label(&view.summary);
            ui.horizontal_wrapped(|ui| {
                for (name, value) in &view.yields {
                    ui.colored_label(yield_color(name), format!("{name} {value:+}"));
                }
            });
            match &view.producing {
                Some(p) => {
                    ui.strong(p.text());
                    if p.needed > 0 {
                        ui.add(
                            egui::ProgressBar::new((p.stored as f32 / p.needed as f32).min(1.0))
                                .desired_width(300.0),
                        );
                    }
                }
                None => {
                    ui.strong("Producing nothing");
                }
            }
            if let Some(why) = &view.refused {
                ui.colored_label(egui::Color32::from_rgb(240, 110, 90), why);
            }
            if view.no_connection {
                ui.colored_label(egui::Color32::from_rgb(240, 110, 90), NO_CONNECTION);
            }
            ui.separator();
            if view.loading {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Asking what the city can build...");
                });
                return;
            }
            egui::ScrollArea::vertical()
                .max_height(420.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for section in BuildSection::ALL {
                        let mut lines = view
                            .builds
                            .iter()
                            .enumerate()
                            .filter(|(_, b)| b.section == section)
                            .peekable();
                        if lines.peek().is_none() {
                            continue;
                        }
                        ui.label(egui::RichText::new(section.title()).strong());
                        for (i, b) in lines {
                            ui.horizontal(|ui| {
                                let r = ui.add_enabled(
                                    !view.no_connection,
                                    egui::Button::new(b.text())
                                        .selected(b.current)
                                        .min_size(egui::vec2(200.0, 0.0)),
                                );
                                if view.no_connection {
                                    r.clone().on_disabled_hover_text(NO_CONNECTION);
                                }
                                if r.clicked() {
                                    r.surrender_focus();
                                    actions.push(UiAction::ChooseBuild(i));
                                }
                                if let Some(o) = b.gold {
                                    let c = !view.no_connection;
                                    buy_button(ui, o, "gold", view.gold, i, false, c, &mut actions);
                                }
                                if let Some(o) = b.faith {
                                    let c = !view.no_connection;
                                    buy_button(
                                        ui,
                                        o,
                                        "faith",
                                        view.faith,
                                        i,
                                        true,
                                        c,
                                        &mut actions,
                                    );
                                }
                            });
                        }
                    }
                    if view.builds.is_empty() {
                        ui.weak("Nothing can be built now.");
                    }
                });
        });
    if !open {
        actions.push(UiAction::CloseCity);
    }
    actions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::{click_at, find_widget, run_headless};

    fn view() -> CityPanelView {
        let line = |section, name: &str, current| BuildLine {
            section,
            name: name.into(),
            turns: 4,
            current,
            cost: 40,
            gold: None,
            faith: None,
        };
        let mut v = CityPanelView {
            name: "Muscat".into(),
            summary: "Population 1".into(),
            yields: vec![("food".into(), 2), ("gold".into(), 1)],
            producing: Some(Producing {
                name: "Monument".into(),
                stored: 10,
                needed: 40,
                turns: 4,
            }),
            builds: vec![
                line(BuildSection::Units, "Warrior", false),
                line(BuildSection::Buildings, "Monument", true),
                line(BuildSection::Wonders, "Stonehenge", false),
            ],
            gold: 50,
            faith: 0,
            ..CityPanelView::default()
        };
        v.builds[0].gold = Some(Offer {
            cost: 30,
            enabled: true,
        });
        v.builds[1].gold = Some(Offer {
            cost: 90,
            enabled: false,
        });
        v.builds[1].faith = Some(Offer {
            cost: 60,
            enabled: true,
        });
        v
    }

    #[test]
    fn sections_and_choice() {
        let v = view();
        for t in [
            "Units",
            "Buildings",
            "Wonders",
            "Monument: 4 turns (current)",
            "Producing Monument (10/40, 4 turns)",
            "food +2",
        ] {
            assert!(find_widget(t, |ctx| show(ctx, &v)).is_some(), "{t}");
        }
        assert!(
            find_widget("Other", |ctx| show(ctx, &v)).is_none(),
            "empty section"
        );
        let at = find_widget("Stonehenge: 4 turns", |ctx| show(ctx, &v)).unwrap();
        assert_eq!(
            click_at(at.center(), false, |ctx| show(ctx, &v)),
            [UiAction::ChooseBuild(2)]
        );
    }

    #[test]
    fn purchase_buttons_follow_the_cores_offer() {
        let v = view();
        let at = find_widget("Buy 30 gold", |ctx| show(ctx, &v)).unwrap();
        assert_eq!(
            click_at(at.center(), false, |ctx| show(ctx, &v)),
            [UiAction::BuyBuild {
                index: 0,
                faith: false
            }]
        );
        let at = find_widget("Buy 60 faith", |ctx| show(ctx, &v)).unwrap();
        assert_eq!(
            click_at(at.center(), false, |ctx| show(ctx, &v)),
            [UiAction::BuyBuild {
                index: 1,
                faith: true
            }]
        );
        // Too dear for the treasury: shown, but a click does nothing.
        let at = find_widget("Buy 90 gold", |ctx| show(ctx, &v)).unwrap();
        assert!(click_at(at.center(), false, |ctx| show(ctx, &v)).is_empty());
        // Stonehenge has no offer at all.
        assert_eq!(
            v.builds[2].gold.or(v.builds[2].faith),
            None,
            "nothing to buy"
        );
    }

    #[test]
    fn shows_the_refusal_and_loading_state() {
        let mut v = view();
        v.refused = Some("Refused: not enough gold".into());
        assert!(find_widget("Refused: not enough gold", |ctx| show(ctx, &v)).is_some());
        v.loading = true;
        v.builds.clear();
        assert!(find_widget("Asking what the city can build...", |ctx| show(ctx, &v)).is_some());
        v.producing = None;
        let shapes = run_headless(|ctx| {
            show(ctx, &v);
        });
        assert!(!shapes.is_empty());
    }

    #[test]
    fn no_connection_greys_choosing_and_buying() {
        let mut v = view();
        v.no_connection = true;
        assert!(find_widget("No connection", |ctx| show(ctx, &v)).is_some());
        for label in ["Stonehenge: 4 turns", "Buy 30 gold", "Buy 60 faith"] {
            let at = find_widget(label, |ctx| show(ctx, &v)).expect(label);
            assert!(
                click_at(at.center(), false, |ctx| show(ctx, &v)).is_empty(),
                "{label}"
            );
        }
    }
}
