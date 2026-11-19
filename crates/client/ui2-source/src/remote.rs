//! Whole-root subscriptions for server-built screens. No game-core dependency.
use protocol::{Query, QueryAnswer};
use std::collections::BTreeMap;
use ui2_model::generated::{SCREEN_NAMES, TargetKind, Ui};
use ui2_model::{ModelWriter, Sample, Schema, Value};

#[derive(Default)]
struct Subscription {
    args: Vec<(String, i64)>,
    sent_args: Vec<(String, i64)>,
    revision: Option<u64>,
    last_sent: Option<u64>,
    waiting: Option<u32>,
    dirty: bool,
    has_root: bool,
}

#[derive(Default)]
pub struct RemoteRoots {
    subscriptions: BTreeMap<String, Subscription>,
    connected: bool,
    clear_due: bool,
}

impl RemoteRoots {
    /// Forget in-flight requests when a different game is attached.
    pub fn reset(&mut self) {
        *self = Self {
            clear_due: true,
            ..Self::default()
        };
    }

    /// Refresh after a final outcome of this screen's action.
    pub fn invalidate(&mut self, screen: &str) {
        if let Some(s) = self.subscriptions.get_mut(screen) {
            s.dirty = true;
        }
    }

    /// Take a response only while the same subscription is open. Errors clear its root.
    pub fn answer(
        &mut self,
        id: u32,
        answer: &QueryAnswer,
        writer: &mut ModelWriter<'_>,
    ) -> Option<String> {
        let (screen, subscription) = self
            .subscriptions
            .iter_mut()
            .find(|(_, s)| s.waiting == Some(id))?;
        subscription.waiting = None;
        if subscription.sent_args != subscription.args {
            return None;
        }
        let result = match answer {
            QueryAnswer::Screen {
                screen: actual,
                json,
            } if actual == screen && json.len() <= 512 * 1024 => {
                let json: serde_json::Value = match serde_json::from_str(json) {
                    Ok(v @ serde_json::Value::Object(_)) => v,
                    _ => {
                        subscription.has_root = false;
                        return Some(clear_error(writer, screen, "invalid screen JSON object"));
                    }
                };
                let sample = serde_json::json!({ "model": { screen: json } }).to_string();
                Sample::parse(Schema::v1(), &sample)
                    .map_err(|e| e.join("; "))
                    .and_then(|s| {
                        let (_, value) = s.roots().first().ok_or("missing screen root")?;
                        put(writer, screen, value)
                    })
            }
            QueryAnswer::Error { reason } => Err(reason.clone()),
            _ => Err("unexpected screen response".into()),
        };
        subscription.has_root = result.is_ok();
        result.err().map(|e| clear_error(writer, screen, &e))
    }

    /// Subscribe open registry windows. The host supplies a monotonic clock and the world's revision.
    /// SIMPLIFIED(SIMP-322): screen refreshes transfer the entire root, without deltas.
    pub fn frame(
        &mut self,
        now_ms: u64,
        revision: u64,
        connected: bool,
        ui: &Ui,
        writer: &mut ModelWriter<'_>,
        mut query: impl FnMut(Query) -> u32,
    ) -> Vec<String> {
        let mut errors = Vec::new();
        for name in SCREEN_NAMES {
            if self.clear_due
                && let Err(e) = put(writer, name, &Value::None)
            {
                errors.push(e);
            }
            let args = if *name == "hud" {
                let city = if ui.windows.production.open {
                    ui.windows.production.city.or(ui.selection.city)
                } else {
                    ui.selection.city
                };
                [("city", city), ("unit", ui.selection.unit)]
                    .into_iter()
                    .filter_map(|(name, key)| {
                        key.map(|key| (name.into(), i64::from(key.0 as u32 as i32)))
                    })
                    .collect::<Vec<_>>()
            } else if *name == "civ_ribbon" {
                Vec::new()
            } else if *name == "plot_help"
                && let Some(plot) = ui.hover.iter().next().map(|h| (h.plot.x, h.plot.y))
            {
                // The plot the mouse rests on, and the selected unit for the combat preview. Another plot clears the
                // root and drops the answer still on its way; the same plot is asked again only when the world
                // changed (its revision).
                let mut args = vec![
                    ("x".into(), i64::from(plot.0)),
                    ("y".into(), i64::from(plot.1)),
                ];
                if let Some(unit) = ui.selection.unit {
                    args.push(("unit".into(), i64::from(unit.0 as u32 as i32)));
                }
                args
            } else if *name == "targets"
                && let Some(t) = ui.target.as_ref()
            {
                // The map's targeting mode: what acts and the order (`ui2_actions` matches its clicks against the
                // answer by the same `id` and `action`). Asked again when the world changes.
                let id = |k: Option<ui2_model::Key>| k.map_or(-1, |k| i64::from(k.0 as u32 as i32));
                let (id, action) = match t.kind {
                    TargetKind::Unit => (id(t.unit), i64::from(t.a)),
                    TargetKind::City => (id(t.city), 0),
                    TargetKind::GiftUnit | TargetKind::GiftImprovement => (i64::from(t.a), 0),
                };
                vec![
                    ("kind".into(), i64::from(t.kind as u16)),
                    ("id".into(), id),
                    ("action".into(), action),
                ]
            } else if *name == "rich_tooltips"
                && let Some(id) = ui.tip.id
            {
                // The game thing the pointer rests on in a window (`ui.tip`, set by the skin): another one clears the
                // root and drops the answer on its way, as for the plot.
                let mut args = vec![
                    ("kind".into(), i64::from(ui.tip.kind)),
                    ("id".into(), i64::from(id)),
                ];
                if let Some(city) = ui.tip.city {
                    args.push(("city".into(), i64::from(city)));
                }
                args
            } else if let Some(window) = ui.windows.remote(name).filter(|w| w.open) {
                [
                    ("a", window.a),
                    ("b", window.b),
                    ("popup_id", window.popup_id),
                ]
                .into_iter()
                .filter_map(|(k, v)| v.map(|v| (k.into(), i64::from(v))))
                .collect::<Vec<_>>()
            } else {
                if self.subscriptions.remove(*name).is_some()
                    && let Err(e) = put(writer, name, &Value::None)
                {
                    errors.push(e);
                }
                continue;
            };
            let s = self.subscriptions.entry((*name).into()).or_default();
            if args != s.args {
                if *name == "hud" && s.has_root {
                    // Focus changes invalidate only the facts owned by the changed argument.
                    // Global research, resources, turn state and popup facts remain usable.
                    for field in ["city", "unit"] {
                        let previous = s.args.iter().find(|(k, _)| k == field);
                        let next = args.iter().find(|(k, _)| k == field);
                        if previous != next
                            && let Err(e) = writer.set_value(&format!("hud.{field}"), Value::None)
                        {
                            errors.push(e.to_string());
                        }
                    }
                } else if let Err(e) = put(writer, name, &Value::None) {
                    errors.push(e);
                }
                s.args = args;
                s.dirty = true;
            }
            if !connected || !self.connected {
                if let Err(e) = put(writer, name, &Value::None) {
                    errors.push(e);
                }
                s.has_root = false;
                s.waiting = None;
                s.revision = None;
            }
            if s.waiting.is_some()
                && s.last_sent
                    .is_some_and(|t| now_ms.saturating_sub(t) >= 15_000)
            {
                s.has_root = false;
                s.waiting = None;
                s.revision = None;
                errors.push(clear_error(writer, name, "screen query timed out"));
            }
            if connected
                && s.waiting.is_none()
                && (matches!(*name, "hud" | "civ_ribbon")
                    || s.dirty
                    || s.revision != Some(revision))
                && s.last_sent.is_none_or(|t| now_ms.saturating_sub(t) >= 250)
            {
                let id = query(Query::Screen {
                    screen: (*name).into(),
                    args: s.args.clone(),
                });
                if id != 0 {
                    s.waiting = Some(id);
                    s.sent_args.clone_from(&s.args);
                    s.last_sent = Some(now_ms);
                    s.revision = Some(revision);
                    s.dirty = false;
                }
            }
        }
        self.clear_due = false;
        self.connected = connected;
        errors
    }
}

fn put(writer: &mut ModelWriter<'_>, screen: &str, value: &Value) -> Result<(), String> {
    ui2_model::generated::put_root(writer, screen, value)
        .ok_or_else(|| format!("unknown screen root {screen}"))?
        .map_err(|e| e.to_string())
}

fn clear_error(writer: &mut ModelWriter<'_>, screen: &str, error: &str) -> String {
    let _ = put(writer, screen, &Value::None);
    format!("{screen}: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ui2_model::generated::Techs;
    use ui2_model::{ModelStore, Owner};

    #[test]
    fn rich_tooltips_follow_the_hovered_thing_and_go_when_it_is_left() {
        let mut remote = RemoteRoots::default();
        let mut store = ModelStore::new(Schema::v1());
        let mut ui = Ui::default();
        let asked: std::cell::RefCell<Vec<Vec<(String, i64)>>> = Default::default();
        let tick = |remote: &mut RemoteRoots, store: &mut ModelStore, ui: &Ui, now| {
            remote.frame(
                now,
                1,
                true,
                ui,
                &mut store.writer(Owner::Source),
                |q| match q {
                    Query::Screen { screen, args } if screen == "rich_tooltips" => {
                        asked.borrow_mut().push(args);
                        asked.borrow().len() as u32
                    }
                    _ => 0,
                },
            );
        };
        tick(&mut remote, &mut store, &ui, 0);
        assert!(!remote.subscriptions.contains_key("rich_tooltips"));
        // A technology row under the pointer, then a building seen from a city.
        ui.tip.kind = 0;
        ui.tip.id = Some(12);
        tick(&mut remote, &mut store, &ui, 10);
        assert_eq!(remote.subscriptions["rich_tooltips"].waiting, Some(1));
        let answer = QueryAnswer::Screen {
            screen: "rich_tooltips".into(),
            json: "{}".into(),
        };
        assert!(
            remote
                .answer(1, &answer, &mut store.writer(Owner::Source))
                .is_none()
        );
        ui.tip.kind = 2;
        ui.tip.id = Some(3);
        ui.tip.city = Some(7);
        tick(&mut remote, &mut store, &ui, 400);
        // Left: the subscription and its root go.
        ui.tip.id = None;
        tick(&mut remote, &mut store, &ui, 700);
        assert!(!remote.subscriptions.contains_key("rich_tooltips"));
        assert_eq!(
            *asked.borrow(),
            [
                vec![("kind".to_string(), 0), ("id".to_string(), 12)],
                vec![
                    ("kind".to_string(), 2),
                    ("id".to_string(), 3),
                    ("city".to_string(), 7)
                ],
            ]
        );
    }

    #[test]
    fn plot_help_follows_the_hovered_plot_and_drops_answers_for_a_plot_left() {
        use ui2_model::Key;
        use ui2_model::generated::{HoverPlot, Plot, PlotHelp};
        let at = |x: i16, y: i16| {
            ui2_model::KeyedList::from_vec(vec![HoverPlot {
                cell: i32::from(x) * 65_536 + i32::from(y),
                plot: Plot { x, y },
            }])
            .unwrap()
        };
        let mut remote = RemoteRoots::default();
        let mut store = ModelStore::new(Schema::v1());
        let mut ui = Ui::default();
        let asked: std::cell::RefCell<Vec<Vec<(String, i64)>>> = Default::default();
        let tick = |remote: &mut RemoteRoots, store: &mut ModelStore, ui: &Ui, now, rev| {
            remote.frame(
                now,
                rev,
                true,
                ui,
                &mut store.writer(Owner::Source),
                |q| match q {
                    Query::Screen { screen, args } if screen == "plot_help" => {
                        asked.borrow_mut().push(args);
                        asked.borrow().len() as u32
                    }
                    _ => 0,
                },
            );
        };
        // No plot under the mouse: nothing is asked.
        tick(&mut remote, &mut store, &ui, 0, 1);
        assert!(!remote.subscriptions.contains_key("plot_help"));
        ui.hover = at(4, 5);
        ui.selection.unit = Some(Key::new(0, 9));
        tick(&mut remote, &mut store, &ui, 10, 1);
        assert_eq!(remote.subscriptions["plot_help"].waiting, Some(1));
        let answer = |x: i16| QueryAnswer::Screen {
            screen: "plot_help".into(),
            json: format!(r#"{{"x":{x},"y":5,"revealed":true}}"#),
        };
        assert!(
            remote
                .answer(1, &answer(4), &mut store.writer(Owner::Source))
                .is_none()
        );
        assert_eq!(
            store
                .get::<Option<PlotHelp>>()
                .unwrap()
                .as_ref()
                .map(|p| p.x),
            Some(4)
        );
        // The same plot and world: not asked again (the answer is kept for the revision).
        tick(&mut remote, &mut store, &ui, 400, 1);
        assert_eq!(asked.borrow().len(), 1);
        // Another plot: the root goes at once and the next answer is for the new plot only.
        ui.hover = at(6, 5);
        tick(&mut remote, &mut store, &ui, 700, 1);
        assert!(store.get::<Option<PlotHelp>>().unwrap().is_none());
        assert_eq!(remote.subscriptions["plot_help"].waiting, Some(2));
        ui.hover = at(7, 5);
        tick(&mut remote, &mut store, &ui, 800, 1);
        remote.answer(2, &answer(6), &mut store.writer(Owner::Source));
        assert!(
            store.get::<Option<PlotHelp>>().unwrap().is_none(),
            "an answer for a plot left"
        );
        // The mouse leaves the map: the subscription and its root go.
        ui.hover = Default::default();
        tick(&mut remote, &mut store, &ui, 1100, 1);
        assert!(!remote.subscriptions.contains_key("plot_help"));
        assert_eq!(
            *asked.borrow(),
            [
                vec![
                    ("x".to_string(), 4),
                    ("y".to_string(), 5),
                    ("unit".to_string(), 9)
                ],
                vec![
                    ("x".to_string(), 6),
                    ("y".to_string(), 5),
                    ("unit".to_string(), 9)
                ],
            ]
        );
    }

    #[test]
    fn targets_follow_the_targeting_mode_and_go_with_it() {
        use ui2_model::Key;
        use ui2_model::generated::{TargetKind, Targets, UiTarget};
        let mut remote = RemoteRoots::default();
        let mut store = ModelStore::new(Schema::v1());
        let mut ui = Ui::default();
        let asked: std::cell::RefCell<Vec<Vec<(String, i64)>>> = Default::default();
        let tick = |remote: &mut RemoteRoots, store: &mut ModelStore, ui: &Ui, now, rev| {
            remote.frame(
                now,
                rev,
                true,
                ui,
                &mut store.writer(Owner::Source),
                |q| match q {
                    Query::Screen { screen, args } if screen == "targets" => {
                        asked.borrow_mut().push(args);
                        asked.borrow().len() as u32
                    }
                    _ => 0,
                },
            );
        };
        tick(&mut remote, &mut store, &ui, 0, 1);
        assert!(!remote.subscriptions.contains_key("targets"));
        // A unit's ranged attack: asked once with the unit's seat-local id and the order's row.
        ui.target = Some(UiTarget {
            kind: TargetKind::Unit,
            unit: Some(Key::new(0, 8193)),
            city: None,
            a: -1,
        });
        tick(&mut remote, &mut store, &ui, 10, 1);
        let answer = QueryAnswer::Screen {
            screen: "targets".into(),
            json: r#"{"id":8193,"action":-1,"name":"Ranged Attack","look":"fire","plots":[{"cell":655371,"x":10,"y":11}]}"#.into(),
        };
        assert!(
            remote
                .answer(1, &answer, &mut store.writer(Owner::Source))
                .is_none()
        );
        let root = store.get::<Option<Targets>>().unwrap().as_ref().unwrap();
        assert_eq!((root.id, root.plots.len()), (8193, 1));
        // The same mode and world: not asked again; the world changes: asked again (the unit may have moved).
        tick(&mut remote, &mut store, &ui, 400, 1);
        assert_eq!(asked.borrow().len(), 1);
        tick(&mut remote, &mut store, &ui, 700, 2);
        assert_eq!(asked.borrow().len(), 2);
        remote.answer(2, &answer, &mut store.writer(Owner::Source));
        // A city's strike: the old plots go at once.
        ui.target = Some(UiTarget {
            kind: TargetKind::City,
            unit: None,
            city: Some(Key::new(0, 24577)),
            a: 0,
        });
        tick(&mut remote, &mut store, &ui, 1000, 2);
        assert!(store.get::<Option<Targets>>().unwrap().is_none());
        // The mode ends: the subscription and its root go.
        ui.target = None;
        tick(&mut remote, &mut store, &ui, 1300, 2);
        assert!(!remote.subscriptions.contains_key("targets"));
        let want = |kind: i64, id: i64, action: i64| {
            vec![
                ("kind".to_string(), kind),
                ("id".to_string(), id),
                ("action".to_string(), action),
            ]
        };
        assert_eq!(
            *asked.borrow(),
            [want(0, 8193, -1), want(0, 8193, -1), want(1, 24577, 0)]
        );
    }

    #[test]
    fn ribbon_service_polls_without_a_window_and_discards_disconnected_answers() {
        use ui2_model::generated::CivRibbon;
        let mut remote = RemoteRoots::default();
        let mut store = ModelStore::new(Schema::v1());
        let ui = Ui::default();
        assert!(ui.windows.remote("civ_ribbon").is_none());
        assert!(!ui2_model::generated::WINDOW_SCREEN_NAMES.contains(&"civ_ribbon"));
        let mut sent = 0;
        let mut tick = |remote: &mut RemoteRoots, store: &mut ModelStore, now, connected| {
            remote.frame(
                now,
                1,
                connected,
                &ui,
                &mut store.writer(Owner::Source),
                |q| {
                    if let Query::Screen { screen, args } = q
                        && screen == "civ_ribbon"
                    {
                        assert!(args.is_empty());
                        sent += 1;
                        return sent;
                    }
                    0
                },
            )
        };
        tick(&mut remote, &mut store, 0, true);
        let answer = QueryAnswer::Screen {
            screen: "civ_ribbon".into(),
            json: "{}".into(),
        };
        assert!(
            remote
                .answer(1, &answer, &mut store.writer(Owner::Source))
                .is_none()
        );
        assert!(store.get::<Option<CivRibbon>>().unwrap().is_some());
        tick(&mut remote, &mut store, 249, true);
        assert_eq!(remote.subscriptions["civ_ribbon"].waiting, None);
        tick(&mut remote, &mut store, 250, true);
        assert_eq!(remote.subscriptions["civ_ribbon"].waiting, Some(2));
        tick(&mut remote, &mut store, 251, false);
        remote.answer(2, &answer, &mut store.writer(Owner::Source));
        assert!(store.get::<Option<CivRibbon>>().unwrap().is_none());
        tick(&mut remote, &mut store, 500, true);
        assert_eq!(remote.subscriptions["civ_ribbon"].waiting, Some(3));
        remote.reset();
        remote.answer(3, &answer, &mut store.writer(Owner::Source));
        assert!(store.get::<Option<CivRibbon>>().unwrap().is_none());
    }

    #[test]
    fn hud_is_always_subscribed_and_tracks_focus_refresh_outcomes_and_reconnects() {
        use ui2_model::{Key, generated::Hud};
        let mut remote = RemoteRoots::default();
        let mut store = ModelStore::new(Schema::v1());
        let mut ui = Ui::default();
        assert!(ui.windows.remote("hud").is_none());
        let mut queries = Vec::new();
        let mut tick = |remote: &mut RemoteRoots,
                        store: &mut ModelStore,
                        ui: &Ui,
                        now,
                        revision,
                        connected| {
            remote.frame(
                now,
                revision,
                connected,
                ui,
                &mut store.writer(Owner::Source),
                |q| {
                    if matches!(&q, Query::Screen { screen, .. } if screen == "civ_ribbon") {
                        return 0;
                    }
                    queries.push(q);
                    queries.len() as u32
                },
            )
        };
        tick(&mut remote, &mut store, &ui, 0, 1, true);
        assert_eq!(remote.subscriptions["hud"].waiting, Some(1));
        let answer = QueryAnswer::Screen {
            screen: "hud".into(),
            json: r#"{"top":{"tourism":17},"city":{"id":3},"unit":{"id":4},"popup_count":2}"#
                .into(),
        };
        remote.answer(1, &answer, &mut store.writer(Owner::Source));
        assert!(store.get::<Option<Hud>>().unwrap().is_some());
        ui.selection.city = Some(Key::new(0, 3));
        ui.selection.unit = Some(Key::new(0, 4));
        tick(&mut remote, &mut store, &ui, 249, 1, true);
        let retained = store.get::<Option<Hud>>().unwrap().as_ref().unwrap();
        assert_eq!(retained.top.tourism, Some(17));
        assert_eq!(retained.popup_count, 2);
        assert!(retained.city.is_none());
        assert!(retained.unit.is_none());
        tick(&mut remote, &mut store, &ui, 250, 1, true);
        assert_eq!(remote.subscriptions["hud"].waiting, Some(2));
        ui.selection.unit = None;
        tick(&mut remote, &mut store, &ui, 500, 2, true);
        remote.answer(2, &answer, &mut store.writer(Owner::Source));
        assert!(
            store
                .get::<Option<Hud>>()
                .unwrap()
                .as_ref()
                .unwrap()
                .unit
                .is_none(),
            "stale focus is discarded while shared facts survive"
        );
        tick(&mut remote, &mut store, &ui, 501, 2, true);
        remote.answer(3, &answer, &mut store.writer(Owner::Source));
        remote.invalidate("hud");
        tick(&mut remote, &mut store, &ui, 751, 2, true);
        assert_eq!(remote.subscriptions["hud"].waiting, Some(4));
        tick(&mut remote, &mut store, &ui, 752, 2, false);
        remote.answer(4, &answer, &mut store.writer(Owner::Source));
        assert!(store.get::<Option<Hud>>().unwrap().is_none());
        tick(&mut remote, &mut store, &ui, 1001, 2, true);
        assert_eq!(remote.subscriptions["hud"].waiting, Some(5));
        assert_eq!(
            queries[0],
            Query::Screen {
                screen: "hud".into(),
                args: vec![]
            }
        );
        assert_eq!(
            queries[1],
            Query::Screen {
                screen: "hud".into(),
                args: vec![("city".into(), 3), ("unit".into(), 4)]
            }
        );
        assert_eq!(
            queries[2],
            Query::Screen {
                screen: "hud".into(),
                args: vec![("city".into(), 3)]
            }
        );
    }

    #[test]
    fn unit_focus_change_preserves_city_and_global_hud_facts() {
        use ui2_model::{Key, generated::Hud};
        let mut remote = RemoteRoots::default();
        let mut store = ModelStore::new(Schema::v1());
        let mut ui = Ui::default();
        ui.selection.city = Some(Key::new(0, 3));
        ui.selection.unit = Some(Key::new(0, 4));
        remote.frame(0, 1, true, &ui, &mut store.writer(Owner::Source), |q| {
            if matches!(q, Query::Screen { screen, .. } if screen == "hud") {
                1
            } else {
                0
            }
        });
        let answer = QueryAnswer::Screen {
            screen: "hud".into(),
            json: r#"{"top":{"tourism":17,"resources":[{"id":0,"available":2}]},"research":{"techs":[{"id":1}]},"city":{"id":3},"unit":{"id":4},"popup":{"id":7,"screen":"techs"}}"#.into(),
        };
        assert!(
            remote
                .answer(1, &answer, &mut store.writer(Owner::Source))
                .is_none()
        );
        let before = store.get::<Option<Hud>>().unwrap().clone().unwrap();
        ui.selection.unit = Some(Key::new(0, 5));
        remote.frame(250, 1, true, &ui, &mut store.writer(Owner::Source), |_| 2);
        let after = store.get::<Option<Hud>>().unwrap().as_ref().unwrap();
        assert_eq!(after.city, before.city);
        assert_eq!(after.top, before.top);
        assert_eq!(after.research, before.research);
        assert_eq!(after.popup, before.popup);
        assert!(after.unit.is_none());
    }

    #[test]
    fn subscription_throttles_and_discards_closed_window_answers() {
        let mut remote = RemoteRoots::default();
        let mut store = ModelStore::new(Schema::v1());
        let mut ui = Ui::default();
        ui.windows.techs.open = true;
        ui.windows.techs.a = Some(7);
        let mut queries = Vec::new();
        let mut tick = |remote: &mut RemoteRoots, now, rev, ui: &Ui, store: &mut ModelStore| {
            remote.frame(now, rev, true, ui, &mut store.writer(Owner::Source), |q| {
                if matches!(&q, Query::Screen { screen, .. } if matches!(screen.as_str(), "hud" | "civ_ribbon")) {
                    return 0;
                }
                queries.push(q);
                queries.len() as u32
            })
        };
        assert!(tick(&mut remote, 0, 1, &ui, &mut store).is_empty());
        assert_eq!(remote.subscriptions["techs"].waiting, Some(1));
        tick(&mut remote, 100, 2, &ui, &mut store);
        assert_eq!(remote.subscriptions["techs"].waiting, Some(1));
        let answer = QueryAnswer::Screen {
            screen: "techs".into(),
            json: "{\"techs\":[]}".into(),
        };
        assert!(
            remote
                .answer(1, &answer, &mut store.writer(Owner::Source))
                .is_none()
        );
        assert!(store.get::<Option<Techs>>().unwrap().is_some());
        tick(&mut remote, 249, 2, &ui, &mut store);
        assert_eq!(remote.subscriptions["techs"].waiting, None);
        tick(&mut remote, 250, 2, &ui, &mut store);
        assert_eq!(remote.subscriptions["techs"].waiting, Some(2));
        remote.invalidate("techs");
        remote.answer(2, &answer, &mut store.writer(Owner::Source));
        tick(&mut remote, 500, 2, &ui, &mut store);
        assert_eq!(remote.subscriptions["techs"].waiting, Some(3));
        ui.windows.techs.open = false;
        tick(&mut remote, 501, 2, &ui, &mut store);
        assert!(store.get::<Option<Techs>>().unwrap().is_none());
        assert!(
            remote
                .answer(3, &answer, &mut store.writer(Owner::Source))
                .is_none()
        );
        assert!(store.get::<Option<Techs>>().unwrap().is_none());
        assert_eq!(
            queries[0],
            Query::Screen {
                screen: "techs".into(),
                args: vec![("a".into(), 7)]
            }
        );
    }

    #[test]
    fn changing_arguments_waits_for_and_discards_the_previous_response() {
        let mut remote = RemoteRoots::default();
        let mut store = ModelStore::new(Schema::v1());
        let mut ui = Ui::default();
        ui.windows.techs.open = true;
        ui.windows.techs.a = Some(7);
        remote.frame(0, 1, true, &ui, &mut store.writer(Owner::Source), |q| {
            if matches!(q, Query::Screen { screen, .. } if matches!(screen.as_str(), "hud" | "civ_ribbon")) {
                0
            } else {
                1
            }
        });
        ui.windows.techs.a = Some(8);
        remote.frame(250, 1, true, &ui, &mut store.writer(Owner::Source), |q| {
            if matches!(q, Query::Screen { screen, .. } if matches!(screen.as_str(), "hud" | "civ_ribbon")) {
                return 0;
            }
            panic!("previous request is still outstanding")
        });
        let answer = QueryAnswer::Screen {
            screen: "techs".into(),
            json: "{\"techs\":[]}".into(),
        };
        assert!(
            remote
                .answer(1, &answer, &mut store.writer(Owner::Source))
                .is_none()
        );
        assert!(store.get::<Option<Techs>>().unwrap().is_none());
        remote.frame(251, 1, true, &ui, &mut store.writer(Owner::Source), |q| {
            if matches!(&q, Query::Screen { screen, .. } if matches!(screen.as_str(), "hud" | "civ_ribbon")) {
                return 0;
            }
            assert_eq!(
                q,
                Query::Screen {
                    screen: "techs".into(),
                    args: vec![("a".into(), 8)]
                }
            );
            2
        });
        remote.answer(2, &answer, &mut store.writer(Owner::Source));
        assert!(store.get::<Option<Techs>>().unwrap().is_some());
    }

    #[test]
    fn invalid_json_and_server_errors_clear_the_root_and_return_a_diagnostic() {
        let mut remote = RemoteRoots::default();
        let mut store = ModelStore::new(Schema::v1());
        let mut ui = Ui::default();
        ui.windows.techs.open = true;
        for (i, answer) in [
            QueryAnswer::Screen {
                screen: "techs".into(),
                json: "{\"unknown\":1}".into(),
            },
            QueryAnswer::Screen {
                screen: "techs".into(),
                json: "null".into(),
            },
            QueryAnswer::Error {
                reason: "screen failed".into(),
            },
        ]
        .iter()
        .enumerate()
        {
            remote.invalidate("techs");
            remote.frame(
                i as u64 * 250,
                i as u64,
                true,
                &ui,
                &mut store.writer(Owner::Source),
                |q| {
                    if matches!(q, Query::Screen { screen, .. } if matches!(screen.as_str(), "hud" | "civ_ribbon")) {
                        0
                    } else {
                        1
                    }
                },
            );
            assert!(
                remote
                    .answer(1, answer, &mut store.writer(Owner::Source))
                    .unwrap()
                    .starts_with("techs:")
            );
            assert!(store.get::<Option<Techs>>().unwrap().is_none());
        }
        store
            .writer(Owner::Source)
            .put(Some(Techs::default()))
            .unwrap();
        remote.reset();
        ui.windows.techs.open = false;
        remote.frame(1000, 5, true, &ui, &mut store.writer(Owner::Source), |_| 2);
        assert!(store.get::<Option<Techs>>().unwrap().is_none());
    }
}
