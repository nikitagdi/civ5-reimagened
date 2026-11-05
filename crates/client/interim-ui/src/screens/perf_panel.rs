//! The performance panel (build B3, `perf-hud`, AC-H1..H7): fps, the frame-time graph of the last
//! ~10 s, CPU against GPU time and the verdict, the pass table, memory (GPU by cluster, RAM) and the
//! counters of every group (`terrain.*`, `net.*`, `assets.*`, `storage.*`, ...). F3 toggles it
//! (`?hud=1` shows it from the start). The numbers are `render::perf::PerfSnapshot`, the same record
//! that `window.__civ5stats` and the desktop log line are made from.
//!
//! Diagnosing the GPU cost on a phone (WebGPU times whole passes only):
//!
//! * "Main by group" (under the pass table): draws, triangles and instances of each draw group of the
//!   `Main` pass (`render::perf::groups`), the largest first, next to the GPU time of `Main`. A
//!   checkbox in front of a group says it is drawn; unticking skips it (`UiAction::SetPerfGroups`, debug
//!   only, not saved; "All on" and closing the panel draw everything again), so the GPU time of `Main`
//!   shows what the group cost. Picking and the game are not affected.
//! * "Send report": one snapshot of everything the panel shows, with the device, the graphics settings
//!   and the recent log lines, goes to the server (`UiAction::SendPerfReport`; the shell makes the
//!   document, `client-app/src/perf_report.rs`, and shows the answer here as [`ReportState`]).

use render::perf::{PerfSnapshot, Verdict};

use super::UiAction;

/// Where the last "Send report" is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ReportState {
    /// Nothing was sent since the panel opened.
    #[default]
    Idle,
    Sending,
    /// The server took it: the id of the report.
    Sent(String),
    /// Written to a file (the desktop client): its path.
    Saved(String),
    Failed(String),
}

/// What the panel shows; `None` in `Screens::perf` hides it (and costs nothing).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PerfView {
    pub snapshot: std::rc::Rc<PerfSnapshot>,
    pub report: ReportState,
}

/// Groups drawn open at first: the ones the other B3 streams publish into.
const OPEN_GROUPS: [&str; 4] = ["terrain", "net", "assets", "storage"];
/// Groups the panel draws with its own widgets above the generic tables.
const OWN_GROUPS: [&str; 2] = ["frame", "render"];

const MS_144: f32 = 1000.0 / 144.0;
const MS_60: f32 = 1000.0 / 60.0;
/// The graph's top is this many ms (frames slower than that are clipped to the top).
const GRAPH_MAX_MS: f32 = 40.0;

pub fn fmt_bytes(b: f64) -> String {
    const KB: f64 = 1024.0;
    if b >= KB * KB * KB {
        format!("{:.2} GB", b / (KB * KB * KB))
    } else if b >= KB * KB {
        format!("{:.1} MB", b / (KB * KB))
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{b:.0} B")
    }
}

/// Counter names that hold bytes (shown with units): the group `gpu_mem`, `ram` and anything that
/// ends in `_bytes` / `.bytes`.
fn is_bytes(group: &str, name: &str) -> bool {
    match group {
        "gpu_mem" => !name.ends_with("_count"),
        "ram" => !name.starts_with("allocs"),
        _ => name.ends_with("bytes") || name.ends_with("_b"),
    }
}

fn verdict_color(v: Option<Verdict>) -> egui::Color32 {
    match v {
        Some(Verdict::Cpu) => egui::Color32::from_rgb(240, 160, 60),
        Some(Verdict::Gpu) => egui::Color32::from_rgb(230, 90, 90),
        Some(Verdict::Vsync) => egui::Color32::from_rgb(110, 200, 120),
        _ => egui::Color32::GRAY,
    }
}

fn graph(ui: &mut egui::Ui, series: &[f32]) {
    let size = egui::vec2(ui.available_width().max(200.0), 64.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 2.0, egui::Color32::from_black_alpha(160));
    let y_of = |ms: f32| rect.bottom() - rect.height() * (ms.min(GRAPH_MAX_MS) / GRAPH_MAX_MS);
    for (ms, label) in [(MS_144, "144"), (MS_60, "60")] {
        let y = y_of(ms);
        p.hline(
            rect.x_range(),
            y,
            egui::Stroke::new(1.0, egui::Color32::from_white_alpha(50)),
        );
        p.text(
            egui::pos2(rect.left() + 2.0, y),
            egui::Align2::LEFT_BOTTOM,
            label,
            egui::FontId::monospace(9.0),
            egui::Color32::from_white_alpha(120),
        );
    }
    if series.is_empty() {
        return;
    }
    let w = rect.width() / series.len() as f32;
    for (i, &ms) in series.iter().enumerate() {
        let color = if ms <= MS_144 * 1.05 {
            egui::Color32::from_rgb(110, 200, 120)
        } else if ms <= MS_60 * 1.05 {
            egui::Color32::from_rgb(220, 200, 90)
        } else {
            egui::Color32::from_rgb(230, 90, 90)
        };
        let x = rect.left() + i as f32 * w;
        p.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(x, y_of(ms)),
                egui::pos2((x + w.max(1.0)).min(rect.right()), rect.bottom()),
            ),
            0.0,
            color,
        );
    }
}

fn ms(v: Option<f32>) -> String {
    v.map_or_else(|| "-".to_owned(), |v| format!("{v:.2}"))
}

/// The line about the render scale: `Render 75%: scene 1800x810 -> screen 2400x1080, shadows Medium
/// (1024), MSAA 4x, preset Low`.
/// `None` before the renderer published the sizes.
pub fn render_line(s: &PerfSnapshot) -> Option<String> {
    let n = |name: &str| s.counter(&format!("render.{name}"));
    let (pct, sw, sh) = (n("scale_pct")?, n("scene_w")?, n("scene_h")?);
    let (w, h) = (n("screen_w")?, n("screen_h")?);
    let mut line = format!("Render {pct:.0}%: scene {sw:.0}x{sh:.0} -> screen {w:.0}x{h:.0}");
    if let (Some(q), Some(side)) = (n("shadow_quality"), n("shadow_map_side")) {
        let name = u32::try_from(q as i64)
            .ok()
            .and_then(render::shadow::ShadowQuality::from_index)
            .map_or("?", render::shadow::ShadowQuality::name);
        line += &format!(", shadows {name} ({side:.0})");
    }
    if let Some(samples) = n("msaa") {
        line += &if samples > 1.0 {
            format!(", MSAA {samples:.0}x")
        } else {
            ", MSAA off".to_owned()
        };
    }
    if let Some(preset) = n("gfx_preset") {
        let name = render::gfx_choice::GfxPreset::name_of_index(preset as u32);
        line += &format!(", preset {name}");
    }
    Some(line)
}

/// The line about the terrain detail: `Terrain Medium: lattice step 2, pages up to 256, budget 384 MB`.
/// `None` before the terrain published them (no map yet).
pub fn terrain_line(s: &PerfSnapshot) -> Option<String> {
    let level = s.counter("render.terrain_detail")?;
    let name = u32::try_from(level as i64)
        .ok()
        .and_then(render::terrain_detail::TerrainDetail::from_index)
        .map_or("?", render::terrain_detail::TerrainDetail::name);
    let mut line = format!("Terrain {name}");
    if let (Some(step), Some(page), Some(bytes)) = (
        s.counter("terrain.lattice_step"),
        s.counter("terrain.max_page"),
        s.counter("terrain.page_budget_bytes"),
    ) {
        line += &format!(
            ": lattice step {step:.0}, pages up to {page:.0}, budget {:.0} MB",
            bytes / 1_048_576.0
        );
    }
    Some(line)
}

pub fn show(ctx: &egui::Context, view: &PerfView) -> Vec<UiAction> {
    let mut actions = Vec::new();
    let s = &view.snapshot;
    let f = &s.frame;
    egui::Window::new("Performance (F3)")
        .anchor(egui::Align2::RIGHT_TOP, [-8.0, 40.0])
        .collapsible(true)
        .resizable(false)
        .default_width(440.0)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical()
                .max_height((ctx.content_rect().height() - 120.0).max(200.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.heading(format!("{:.0} fps", f.fps));
                        ui.label(format!("p50 {:.2} ms", f.dt_p50_ms));
                        ui.label(format!("1% low {:.0} fps", f.low1_fps));
                    });
                    ui.horizontal(|ui| {
                        ui.label(format!("CPU {:.2} ms", f.cpu_ms));
                        ui.label(format!("GPU {} ms", ms(f.gpu_ms)));
                        let v = f.verdict.map_or("-", |v| v.name());
                        ui.colored_label(verdict_color(f.verdict), format!("bound: {v}"));
                        // `app.threads`: the workers of the jobs pool (0: single-threaded).
                        let threads = s
                            .counters
                            .iter()
                            .find(|(k, _)| k == "app.threads")
                            .map_or(0.0, |(_, v)| *v);
                        ui.label(if threads > 0.0 {
                            format!("threads {threads:.0}")
                        } else {
                            "single-threaded".to_owned()
                        });
                    });
                    // The sizes the render scale makes, and the way to the sliders (the window of the
                    // settings; the game screen of the original UI has no other way to it).
                    ui.horizontal(|ui| {
                        if let Some(line) = render_line(s) {
                            ui.label(line);
                        }
                        if ui.small_button("Graphics settings").clicked() {
                            actions.push(UiAction::ToggleSettings);
                        }
                    });
                    if let Some(line) = terrain_line(s) {
                        ui.label(line);
                    }
                    // One snapshot to the server (a button, not a key: it works with touch).
                    ui.horizontal(|ui| {
                        let sending = view.report == ReportState::Sending;
                        if ui
                            .add_enabled(!sending, egui::Button::new("Send report"))
                            .clicked()
                        {
                            actions.push(UiAction::SendPerfReport);
                        }
                        match &view.report {
                            ReportState::Idle => {}
                            ReportState::Sending => {
                                ui.weak("sending ...");
                            }
                            ReportState::Sent(id) => {
                                ui.colored_label(
                                    egui::Color32::from_rgb(110, 200, 120),
                                    format!("sent: {id}"),
                                );
                            }
                            ReportState::Saved(path) => {
                                ui.colored_label(
                                    egui::Color32::from_rgb(110, 200, 120),
                                    format!("saved: {path}"),
                                );
                            }
                            ReportState::Failed(why) => {
                                ui.colored_label(egui::Color32::from_rgb(230, 90, 90), why);
                            }
                        }
                    });
                    graph(ui, &s.graph);

                    ui.collapsing("Passes", |ui| {
                        passes(ui, s);
                        main_groups(ui, s, &mut actions);
                    });
                    ui.collapsing("GPU memory", |ui| group_table(ui, s, "gpu_mem"));
                    ui.collapsing("RAM", |ui| group_table(ui, s, "ram"));
                    for (group, rows) in s.groups() {
                        if group == "gpu_mem"
                            || group == "ram"
                            || OWN_GROUPS.contains(&group.as_str())
                        {
                            continue;
                        }
                        let title = if group.is_empty() { "other" } else { &group };
                        egui::CollapsingHeader::new(title)
                            .id_salt(("perf-group", title))
                            .default_open(OPEN_GROUPS.contains(&group.as_str()))
                            .show(ui, |ui| rows_grid(ui, &group, &rows));
                    }
                });
        });
    actions
}

fn passes(ui: &mut egui::Ui, s: &PerfSnapshot) {
    egui::Grid::new("perf-passes")
        .num_columns(8)
        .spacing([10.0, 1.0])
        .striped(true)
        .show(ui, |ui| {
            for h in [
                "pass", "gpu", "cpu", "draws", "tris", "inst", "pipe", "bind",
            ] {
                ui.weak(h);
            }
            ui.end_row();
            for p in &s.passes {
                if p.draw_calls == 0 && p.gpu_ms.is_none() && p.cpu_ms < 0.005 {
                    continue;
                }
                ui.label(p.name);
                ui.monospace(ms(p.gpu_ms));
                ui.monospace(format!("{:.2}", p.cpu_ms));
                ui.monospace(p.draw_calls.to_string());
                ui.monospace(p.triangles.to_string());
                ui.monospace(p.instances.to_string());
                ui.monospace(p.pipeline_sets.to_string());
                ui.monospace(p.bind_group_sets.to_string());
                ui.end_row();
            }
            ui.label("total");
            ui.monospace(ms(Some(s.passes.iter().filter_map(|p| p.gpu_ms).sum())));
            ui.monospace(format!(
                "{:.2}",
                s.passes.iter().map(|p| p.cpu_ms).sum::<f32>()
            ));
            ui.monospace(
                s.passes
                    .iter()
                    .map(|p| p.draw_calls)
                    .sum::<u32>()
                    .to_string(),
            );
            ui.monospace(
                s.passes
                    .iter()
                    .map(|p| p.triangles)
                    .sum::<u64>()
                    .to_string(),
            );
            ui.monospace(
                s.passes
                    .iter()
                    .map(|p| p.instances)
                    .sum::<u64>()
                    .to_string(),
            );
            ui.monospace(
                s.passes
                    .iter()
                    .map(|p| p.pipeline_sets)
                    .sum::<u32>()
                    .to_string(),
            );
            ui.monospace(
                s.passes
                    .iter()
                    .map(|p| p.bind_group_sets)
                    .sum::<u32>()
                    .to_string(),
            );
            ui.end_row();
        });
}

/// "Main by group": what each draw group of the `Main` pass drew, the largest in triangles first, with a
/// checkbox that draws the group or skips it (debug), next to the GPU time of `Main`.
pub fn main_groups(ui: &mut egui::Ui, s: &PerfSnapshot, actions: &mut Vec<UiAction>) {
    let gpu = s
        .passes
        .iter()
        .find(|p| p.name == "Main")
        .and_then(|p| p.gpu_ms);
    let total: u64 = s.main_groups.iter().map(|r| r.triangles).sum();
    egui::CollapsingHeader::new(format!("Main by group (GPU {} ms)", ms(gpu)))
        .id_salt("perf-main-groups")
        .default_open(true)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.monospace(format!("Main GPU {} ms", ms(gpu)));
                if ui.small_button("All on").clicked() {
                    actions.push(UiAction::SetPerfGroups(0));
                }
            });
            if s.groups_off != 0 {
                ui.colored_label(
                    egui::Color32::from_rgb(240, 160, 60),
                    "some groups are not drawn (debug)",
                );
            }
            egui::Grid::new("perf-main-groups-rows")
                .num_columns(5)
                .spacing([10.0, 1.0])
                .striped(true)
                .show(ui, |ui| {
                    for h in ["group (drawn)", "draws", "tris", "inst", "tris %"] {
                        ui.weak(h);
                    }
                    ui.end_row();
                    for row in &s.main_groups {
                        let mut drawn = s.groups_off & row.bit == 0;
                        if row.switchable() {
                            if ui.checkbox(&mut drawn, row.name).changed() {
                                let mask = if drawn {
                                    s.groups_off & !row.bit
                                } else {
                                    s.groups_off | row.bit
                                };
                                actions.push(UiAction::SetPerfGroups(mask));
                            }
                        } else {
                            ui.label(row.name);
                        }
                        ui.monospace(row.draw_calls.to_string());
                        ui.monospace(row.triangles.to_string());
                        ui.monospace(row.instances.to_string());
                        let pct = if total == 0 {
                            0.0
                        } else {
                            row.triangles as f64 * 100.0 / total as f64
                        };
                        ui.monospace(format!("{pct:.1}"));
                        ui.end_row();
                    }
                });
        });
}

fn group_table(ui: &mut egui::Ui, s: &PerfSnapshot, group: &str) {
    let rows: Vec<(&str, f64)> = s
        .groups()
        .into_iter()
        .find(|(g, _)| g == group)
        .map(|(_, r)| r)
        .unwrap_or_default();
    if rows.is_empty() {
        ui.weak("no data");
    } else {
        rows_grid(ui, group, &rows);
    }
}

fn rows_grid(ui: &mut egui::Ui, group: &str, rows: &[(&str, f64)]) {
    egui::Grid::new(("perf-rows", group))
        .num_columns(2)
        .spacing([12.0, 1.0])
        .striped(true)
        .show(ui, |ui| {
            for (name, v) in rows {
                ui.label(*name);
                if is_bytes(group, name) {
                    ui.monospace(fmt_bytes(*v));
                } else if v.fract() == 0.0 && v.abs() < 1e15 {
                    ui.monospace(format!("{v:.0}"));
                } else {
                    ui.monospace(format!("{v:.2}"));
                }
                ui.end_row();
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use render::perf::{PassRow, snapshot::PerfSnapshot as Snap};

    #[test]
    fn bytes_are_formatted() {
        assert_eq!(fmt_bytes(512.0), "512 B");
        assert_eq!(fmt_bytes(2048.0), "2.0 KB");
        assert_eq!(fmt_bytes(5.0 * 1024.0 * 1024.0), "5.0 MB");
    }

    fn view() -> PerfView {
        let snapshot = Snap {
            passes: vec![PassRow {
                name: "Main",
                draw_calls: 12,
                ..Default::default()
            }],
            graph: vec![6.9, 7.0, 16.0, 30.0],
            counters: vec![
                ("gpu_mem.total".into(), 1e9),
                ("ram.heap_live".into(), 2e8),
                ("terrain.holes".into(), 3.0),
                ("net.in_bps".into(), 1200.0),
            ],
            ..Default::default()
        };
        PerfView {
            snapshot: std::rc::Rc::new(snapshot),
            ..PerfView::default()
        }
    }

    #[test]
    fn the_render_line_names_the_scale_and_both_sizes() {
        let mut s = (*view().snapshot).clone();
        assert_eq!(render_line(&s), None);
        s.counters.extend([
            ("render.scale_pct".into(), 75.0),
            ("render.scene_w".into(), 1800.0),
            ("render.scene_h".into(), 810.0),
            ("render.screen_w".into(), 2400.0),
            ("render.screen_h".into(), 1080.0),
            ("render.shadow_quality".into(), 2.0),
            ("render.shadow_map_side".into(), 1024.0),
        ]);
        assert_eq!(
            render_line(&s).as_deref(),
            Some("Render 75%: scene 1800x810 -> screen 2400x1080, shadows Medium (1024)")
        );
    }

    #[test]
    fn the_render_line_names_the_msaa_and_the_preset() {
        let mut s = (*view().snapshot).clone();
        s.counters.extend([
            ("render.scale_pct".into(), 60.0),
            ("render.scene_w".into(), 1440.0),
            ("render.scene_h".into(), 648.0),
            ("render.screen_w".into(), 2400.0),
            ("render.screen_h".into(), 1080.0),
            ("render.msaa".into(), 1.0),
            ("render.gfx_preset".into(), 1.0),
        ]);
        assert_eq!(
            render_line(&s).as_deref(),
            Some("Render 60%: scene 1440x648 -> screen 2400x1080, MSAA off, preset Low")
        );
        s.counters
            .retain(|(k, _)| k != "render.msaa" && k != "render.gfx_preset");
        s.counters.extend([
            ("render.msaa".into(), 4.0),
            ("render.gfx_preset".into(), 0.0),
        ]);
        assert_eq!(
            render_line(&s).as_deref(),
            Some("Render 60%: scene 1440x648 -> screen 2400x1080, MSAA 4x, preset Custom")
        );
    }

    #[test]
    fn the_terrain_line_names_the_detail_and_what_it_caps() {
        let mut s = (*view().snapshot).clone();
        assert_eq!(terrain_line(&s), None);
        s.counters.push(("render.terrain_detail".into(), 1.0));
        assert_eq!(terrain_line(&s).as_deref(), Some("Terrain Medium"));
        s.counters.extend([
            ("terrain.lattice_step".into(), 2.0),
            ("terrain.max_page".into(), 256.0),
            ("terrain.page_budget_bytes".into(), 402_653_184.0),
        ]);
        assert_eq!(
            terrain_line(&s).as_deref(),
            Some("Terrain Medium: lattice step 2, pages up to 256, budget 384 MB")
        );
    }

    use render::perf::GroupRow;

    fn grouped() -> PerfView {
        let mut s = (*view().snapshot).clone();
        s.passes[0].gpu_ms = Some(19.3);
        s.main_groups = vec![
            GroupRow {
                name: "units",
                bit: 1 << 3,
                draw_calls: 5,
                triangles: 3000,
                instances: 40,
            },
            GroupRow {
                name: "terrain",
                bit: 1,
                draw_calls: 10,
                triangles: 1000,
                instances: 10,
            },
            GroupRow {
                name: "other",
                bit: 1 << 11,
                draw_calls: 1,
                triangles: 0,
                instances: 0,
            },
        ];
        PerfView {
            snapshot: std::rc::Rc::new(s),
            ..PerfView::default()
        }
    }

    /// The group table alone, in a window of its own (the panel folds it under "Passes").
    fn groups_window(v: &PerfView) -> impl FnMut(&egui::Context) -> Vec<UiAction> + '_ {
        move |ctx| {
            let mut actions = Vec::new();
            egui::Window::new("groups").show(ctx, |ui| main_groups(ui, &v.snapshot, &mut actions));
            actions
        }
    }

    #[test]
    fn a_checkbox_per_group_skips_it_and_a_second_click_draws_it_again() {
        use crate::screens::{click_at, find_widget};
        let mut v = grouped();
        let at = find_widget("units", groups_window(&v)).expect("a checkbox for units");
        assert_eq!(
            click_at(at.center(), false, groups_window(&v)),
            [UiAction::SetPerfGroups(1 << 3)]
        );
        // Another group off already: the click adds to the mask, and a click on a skipped group clears
        // its bit only.
        let mut s = (*v.snapshot).clone();
        s.groups_off = 1;
        v.snapshot = std::rc::Rc::new(s);
        let at = find_widget("units", groups_window(&v)).unwrap();
        assert_eq!(
            click_at(at.center(), false, groups_window(&v)),
            [UiAction::SetPerfGroups(1 | 1 << 3)]
        );
        let at = find_widget("terrain", groups_window(&v)).unwrap();
        assert_eq!(
            click_at(at.center(), false, groups_window(&v)),
            [UiAction::SetPerfGroups(0)]
        );
        let at = find_widget("All on", groups_window(&v)).unwrap();
        assert_eq!(
            click_at(at.center(), false, groups_window(&v)),
            [UiAction::SetPerfGroups(0)]
        );
    }

    #[test]
    fn the_group_table_shows_main_gpu_time_the_shares_and_no_box_for_other() {
        use crate::screens::find_widget;
        let v = grouped();
        for text in ["Main GPU 19.30 ms", "75.0", "25.0", "units", "other"] {
            assert!(
                find_widget(text, groups_window(&v)).is_some(),
                "{text} is shown"
            );
        }
        let none = PerfView::default();
        assert!(find_widget("Main GPU - ms", groups_window(&none)).is_some());
    }

    #[test]
    fn send_report_is_a_button_that_waits_while_it_sends() {
        use crate::screens::{click_at, find_widget};
        let mut v = view();
        let at = find_widget("Send report", |ctx| show(ctx, &v)).expect("a button");
        assert_eq!(
            click_at(at.center(), false, |ctx| show(ctx, &v)),
            [UiAction::SendPerfReport]
        );
        v.report = ReportState::Sending;
        let at = find_widget("Send report", |ctx| show(ctx, &v)).unwrap();
        assert!(
            click_at(at.center(), false, |ctx| show(ctx, &v)).is_empty(),
            "no second report while one is on its way"
        );
        assert!(find_widget("sending ...", |ctx| show(ctx, &v)).is_some());
        v.report = ReportState::Sent("20261103-101500-Ann-abc123".into());
        assert!(
            find_widget("sent: 20261103-101500-Ann-abc123", |ctx| show(ctx, &v)).is_some(),
            "the id of the report is shown"
        );
        v.report = ReportState::Failed("Too many reports for now".into());
        assert!(find_widget("Too many reports for now", |ctx| show(ctx, &v)).is_some());
    }

    #[test]
    fn shows_every_section_without_actions() {
        let v = view();
        let shapes = super::super::run_headless(|ctx| {
            assert!(show(ctx, &v).is_empty(), "nothing is clicked");
        });
        assert!(!shapes.is_empty());
    }

    /// AC-H9, the shown panel: egui layout and tessellation of the panel with a full snapshot.
    /// Run: `cargo test -p interim-ui --release --lib shown_panel_cost -- --ignored --nocapture`.
    #[test]
    #[ignore = "a measurement, not a check"]
    fn shown_panel_cost() {
        let mut v = view();
        let mut s = (*v.snapshot).clone();
        s.passes = (0..14)
            .map(|i| PassRow {
                name: "Pass",
                draw_calls: 100 + i,
                ..Default::default()
            })
            .collect();
        s.counters
            .extend((0..150).map(|i| (format!("terrain.counter_{i}"), f64::from(i))));
        s.graph = (0..300).map(|i| 6.0 + (i % 7) as f32).collect();
        v.snapshot = std::rc::Rc::new(s);
        let ctx = egui::Context::default();
        let run = |ctx: &egui::Context| {
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                show(ui.ctx(), &v);
            });
        };
        for _ in 0..5 {
            run(&ctx);
        }
        let n = 200;
        let t = std::time::Instant::now();
        for _ in 0..n {
            run(&ctx);
        }
        println!(
            "shown panel: {:.3} ms/frame",
            t.elapsed().as_secs_f64() * 1000.0 / f64::from(n)
        );
    }
}
