//! The client-side fog audit: a second, independent check of N1.3 on
//! what really reached the bot after the gateway materialised the view. The worker audits its own filtering;
//! the bot audits what it holds. Rules:
//!
//! - a foreign unit stands on a plot with `VISIBLE`;
//! - a foreign city stands on a plot with `REVEALED`;
//! - a foreign city on a `REVEALED` plot that is not `VISIBLE` shows what the bot saw last time the plot was
//!   visible (`population`, `hp`, `max_hp`, `flags`, `buildings`): the live value of an unseen city is a leak;
//! - the targets of a context are on visible plots;
//! - a journal entry with a plot names a visible plot or one of the bot's own; a notification may also name a
//!   `REVEALED` plot (the original's rule for "barbarian encampment found");
//! - strict: the `PlotView` of a plot that is not `REVEALED` carries the world but nothing only sight gives
//!   (`PlotView::has_sight_data`; the whole world goes to the client, NP-D19, user decision 2026-10-22), and
//!   the relief fields of every plot that is not an empty record equal the map basis.
//!
//! The map basis itself is on the white list (NP-D19, NP-D22): it is not cut by fog, natural wonders under
//! the fog included, and it is not a violation. The combat-readiness check is withdrawn (NP-D24).

use std::collections::BTreeMap;

use protocol::{
    BasisState, CityView, ContextMsg, JournalEntry, JournalKind, PlayerId, PlotFlags, PlotView,
    ViewDiff, WorldView,
};
use serde_json::{Value, json};

/// How many violations keep their description (the count has no limit).
const EXAMPLES: usize = 20;

/// What the bot remembers of a foreign city while it was visible.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CityMemo {
    population: i16,
    hp: i16,
    max_hp: i16,
    flags: u16,
    buildings: Vec<i16>,
}

impl CityMemo {
    fn of(c: &CityView) -> CityMemo {
        CityMemo {
            population: c.population,
            hp: c.hp,
            max_hp: c.max_hp,
            flags: c.flags.0,
            buildings: c.buildings.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Audit {
    /// The `PlotView` of an unrevealed plot must carry nothing of sight (on by default).
    pub strict_unrevealed: bool,
    pub violations: u64,
    pub examples: Vec<Value>,
    /// Views and messages looked at.
    pub checked: u64,
    memo: BTreeMap<(PlayerId, i32), CityMemo>,
}

impl Default for Audit {
    fn default() -> Self {
        Audit {
            strict_unrevealed: true,
            violations: 0,
            examples: Vec::new(),
            checked: 0,
            memo: BTreeMap::new(),
        }
    }
}

impl Audit {
    /// An audit with the strict check of unrevealed plots on or off.
    pub fn with_strict(strict_unrevealed: bool) -> Audit {
        Audit {
            strict_unrevealed,
            ..Audit::default()
        }
    }
}

/// The plot at `(x, y)`.
fn plot_at(view: &WorldView, x: i16, y: i16) -> Option<&PlotView> {
    let (w, h) = (i32::from(view.map.width), i32::from(view.map.height));
    let (x, y) = (i32::from(x), i32::from(y));
    if x < 0 || y < 0 || x >= w || y >= h {
        return None;
    }
    view.map.plots.get((y * w + x) as usize)
}

impl Audit {
    fn flag(&mut self, kind: &str, turn: i32, detail: String) {
        self.violations += 1;
        if self.examples.len() < EXAMPLES {
            self.examples
                .push(json!({ "kind": kind, "turn": turn, "detail": detail }));
        }
    }

    /// A whole keyframe.
    pub fn check_keyframe(&mut self, view: &WorldView, basis: &BasisState, turn: i32) {
        self.checked += 1;
        if self.strict_unrevealed {
            for i in 0..view.map.plots.len() {
                self.check_plot(view, basis, i, turn);
            }
        }
        self.check_records(view, turn);
    }

    /// A view after a diff: the plots the diff names, and every unit and city (they are few).
    pub fn check_diff(&mut self, view: &WorldView, basis: &BasisState, diff: &ViewDiff, turn: i32) {
        self.checked += 1;
        if self.strict_unrevealed {
            for (i, _) in &diff.plots {
                self.check_plot(view, basis, *i as usize, turn);
            }
        }
        self.check_records(view, turn);
    }

    fn check_plot(&mut self, view: &WorldView, basis: &BasisState, i: usize, turn: i32) {
        let (Some(p), Some(b)) = (view.map.plots.get(i), basis.map().plots.get(i)) else {
            return;
        };
        let at = |i: usize| {
            let w = usize::from(view.map.width).max(1);
            (i % w, i / w)
        };
        if !p.flags.contains(PlotFlags::REVEALED) {
            // The whole world comes to the client (NP-D19, 2026-10-22): an unrevealed plot carries its world
            // fields, but nothing only sight gives (owner, yields, the job, VISIBLE, WORKED, a city).
            if *p == PlotView::default() {
                return;
            }
            if p.has_sight_data() {
                self.flag(
                    "unrevealed_plot_has_sight_data",
                    turn,
                    format!("plot {:?} has data of sight without REVEALED", at(i)),
                );
            }
        }
        let relief = PlotFlags::IS_LAKE | PlotFlags::IS_COASTAL | PlotFlags::NATURAL_WONDER;
        if p.plot_type != b.plot_type
            || p.terrain != b.terrain
            || p.continent_art != b.continent_art
            || p.rivers != b.rivers
            || p.flags.0 & relief != b.flags.0 & relief
        {
            self.flag(
                "plot_differs_from_basis",
                turn,
                format!("plot {:?}: relief fields differ from the map basis", at(i)),
            );
        }
    }

    /// Units and cities against the plots they stand on.
    fn check_records(&mut self, view: &WorldView, turn: i32) {
        let me = view.human;
        for u in &view.units {
            if u.owner == me {
                continue;
            }
            let visible =
                plot_at(view, u.x, u.y).is_some_and(|p| p.flags.contains(PlotFlags::VISIBLE));
            if !visible {
                self.flag(
                    "unit_in_fog",
                    turn,
                    format!(
                        "unit {} of player {} at ({}, {}) is not on a visible plot",
                        u.id, u.owner, u.x, u.y
                    ),
                );
            }
        }
        for c in &view.cities {
            if c.owner == me {
                continue;
            }
            let flags = plot_at(view, c.x, c.y).map_or(PlotFlags::default(), |p| p.flags);
            let key = (c.owner, c.id);
            let now = CityMemo::of(c);
            if !flags.contains(PlotFlags::REVEALED) {
                self.flag(
                    "city_on_unrevealed_plot",
                    turn,
                    format!("city {} of player {} at ({}, {})", c.id, c.owner, c.x, c.y),
                );
            } else if flags.contains(PlotFlags::VISIBLE) {
                self.memo.insert(key, now);
            } else if let Some(seen) = self.memo.get(&key) {
                if *seen != now {
                    self.flag(
                        "city_live_in_fog",
                        turn,
                        format!(
                            "city {} of player {} at ({}, {}) changed while its plot was not visible: {seen:?} -> {now:?}",
                            c.id, c.owner, c.x, c.y
                        ),
                    );
                }
            } else {
                // Known without ever being seen (a map from diplomacy): this is the baseline.
                self.memo.insert(key, now);
            }
        }
    }

    /// The targets of a unit or city context must be on visible plots.
    pub fn check_context(&mut self, view: &WorldView, ctx: &ContextMsg, turn: i32) {
        self.checked += 1;
        let targets = match ctx {
            ContextMsg::Unit(u) => &u.targets,
            ContextMsg::City(c) => &c.targets,
            _ => return,
        };
        for t in targets {
            let visible =
                plot_at(view, t.x, t.y).is_some_and(|p| p.flags.contains(PlotFlags::VISIBLE));
            if !visible {
                self.flag(
                    "context_target_in_fog",
                    turn,
                    format!("target ({}, {}) is not on a visible plot", t.x, t.y),
                );
            }
        }
    }

    /// A journal entry with a plot names a visible plot or one of the bot's own; a notification may also
    /// name a revealed plot out of sight (the original adds "barbarian encampment found" when the team's
    /// revealed improvement becomes the camp, `CvPlot::setRevealedImprovementType`, out of sight too).
    pub fn check_journal(&mut self, view: &WorldView, entries: &[JournalEntry], turn: i32) {
        self.checked += 1;
        for e in entries {
            let Some((x, y)) = e.at else { continue };
            let notification = e.kind == JournalKind::Notification;
            let ok = plot_at(view, x, y).is_some_and(|p| {
                p.flags.contains(PlotFlags::VISIBLE)
                    || p.owner == view.human
                    || (notification && p.flags.contains(PlotFlags::REVEALED))
            });
            if !ok {
                self.flag(
                    "journal_plot_in_fog",
                    turn,
                    format!(
                        "journal line {:?} names ({x}, {y}), not visible and not own",
                        e.text
                    ),
                );
            }
        }
    }

    /// Forgets what was seen (a new stream starts with a keyframe and no memory).
    pub fn reset(&mut self) {
        self.memo.clear();
    }
}
