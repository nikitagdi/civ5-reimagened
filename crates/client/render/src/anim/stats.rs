//! Counters of the service (`?stats=1`) and the report of what moves in the
//! world (WAN0, 4.8): which models have an idle clip, what the machines are, which models lack a
//! code a graph asks for, how many slots the baked clips take.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use super::data::ModelAnim;
use super::graph::NoteKind;

/// Bytes of one bone in the bone texture (3 texels of 4 floats).
pub const BYTES_PER_SLOT: u64 = 48;
/// Slots in a row of the bone texture (render-core 4.12).
pub const SLOTS_PER_ROW: u32 = 512;

/// One line of the budget of the object subsystems (WOB9): what a subsystem hands to the draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BudgetRow {
    pub name: String,
    /// Instanced draw calls (one per model batch).
    pub calls: u32,
    pub instances: u32,
    /// Bytes of the instance buffer: instances times `ObjectInstance::STRIDE`.
    pub bytes: u64,
}

/// The budget by subsystem, and the total as the last row (`name` "total").
pub fn budget(
    objects: &[(String, crate::world::objects::input::ObjectCounters)],
) -> Vec<BudgetRow> {
    let stride = crate::world::objects::instance::ObjectInstance::STRIDE;
    let mut rows: Vec<BudgetRow> = objects
        .iter()
        .map(|(n, c)| BudgetRow {
            name: n.clone(),
            calls: c.batches,
            instances: c.instances,
            bytes: u64::from(c.instances) * stride,
        })
        .collect();
    let total = BudgetRow {
        name: "total".into(),
        calls: rows.iter().map(|r| r.calls).sum(),
        instances: rows.iter().map(|r| r.instances).sum(),
        bytes: rows.iter().map(|r| r.bytes).sum(),
    };
    rows.push(total);
    rows
}

/// The table of the budget for the log: `budget <name> calls N instances N bytes N`, one per line.
pub fn budget_text(rows: &[BudgetRow]) -> String {
    let mut s = String::from("budget");
    for r in rows {
        let _ = write!(
            s,
            "
budget {:<10} calls {} instances {} bytes {}",
            r.name, r.calls, r.instances, r.bytes
        );
    }
    s
}

/// What the service did, for `?stats=1`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AnimStats {
    /// Models registered, and handles attached.
    pub models: u32,
    pub attached: u32,
    /// Clips in the cache and clips waiting for their bake.
    pub clips_cached: u32,
    pub clips_pending: u32,
    /// Slots taken in the bone texture and the rows they reach.
    pub used_slots: u32,
    pub rows: u32,
    /// In the last `prepare`: clips baked, clips evicted, bytes handed to the texture.
    pub baked_frame: u32,
    pub evicted_frame: u32,
    pub uploaded_frame: u64,
    /// Since the start.
    pub baked_total: u64,
    pub evicted_total: u64,
    /// Bakes that found no room even after evicting what could go.
    pub failed_total: u64,
    /// The last `AnimService::step`: microseconds (0 in the browser, which has no `Instant`) and
    /// the ticks the automata ran.
    pub step_us: u64,
    pub fsm_ticks: u64,
}

impl AnimStats {
    /// `anim: 41 attached, 12 clips (1 pending), 9216 slots in 18 rows; frame: baked 2, evicted 0, 98304 B`.
    pub fn line(&self) -> String {
        format!(
            "anim: {} attached ({} models), {} clips ({} pending), {} slots in {} rows; \
             frame: baked {}, evicted {}, {} B",
            self.attached,
            self.models,
            self.clips_cached,
            self.clips_pending,
            self.used_slots,
            self.rows,
            self.baked_frame,
            self.evicted_frame,
            self.uploaded_frame
        )
    }

    /// The pairs of `RenderSystem::counters`.
    pub fn counters(&self) -> Vec<(&'static str, u64)> {
        vec![
            ("anim.attached", u64::from(self.attached)),
            ("anim.clips", u64::from(self.clips_cached)),
            ("anim.pending", u64::from(self.clips_pending)),
            ("anim.rows", u64::from(self.rows)),
            ("anim.baked", u64::from(self.baked_frame)),
            ("anim.evicted", u64::from(self.evicted_frame)),
            ("anim.uploaded", self.uploaded_frame),
            ("anim.baked_total", self.baked_total),
            ("anim.evicted_total", self.evicted_total),
            ("anim.failed_total", self.failed_total),
            ("anim.step_us", self.step_us),
            ("anim.fsm_ticks", self.fsm_ticks),
        ]
    }
}

/// The rows of the bone texture that `slots` reach.
pub fn rows_for(slots: u32) -> u32 {
    slots.div_ceil(SLOTS_PER_ROW)
}

/// One model of the report.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelRow {
    pub category: String,
    pub name: String,
    pub bones: u32,
    pub clips: usize,
    /// The model has a clip that is idle by code or by name.
    pub idle: bool,
    pub machine: Option<String>,
    /// Slots all its clips take once baked.
    pub slots: u64,
    pub legacy_clips: usize,
}

/// The report WAN0: what moves, in numbers. Built from the models of the manifest; the same
/// builder is the base of the coverage test (AC-R4.1).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CoverageReport {
    pub rows: Vec<ModelRow>,
    /// Graph names that are not in the table, with the models that use them.
    pub unknown_graphs: BTreeMap<String, Vec<String>>,
    /// Codes some state asks for and the model does not have, with the models.
    pub missing_ec: BTreeMap<u32, Vec<String>>,
    /// Models whose machine could not give a state anything to play.
    pub nothing_to_play: Vec<String>,
}

impl CoverageReport {
    pub fn add(&mut self, category: &str, m: &ModelAnim) {
        for n in &m.notes {
            match &n.kind {
                NoteKind::UnknownGraph(g) => {
                    push_unique(self.unknown_graphs.entry(g.clone()).or_default(), &m.source)
                }
                NoteKind::MissingClip(ec) => {
                    push_unique(self.missing_ec.entry(*ec).or_default(), &m.source)
                }
                NoteKind::NothingToPlay => push_unique(&mut self.nothing_to_play, &m.source),
                NoteKind::IdleInstead => {}
            }
        }
        self.rows.push(ModelRow {
            category: category.to_owned(),
            name: m.source.clone(),
            bones: m.bones(),
            clips: m.clips.len(),
            idle: m.clips.has_idle(),
            machine: m.machine.as_ref().map(|x| x.name.clone()),
            slots: m.clips.slots(m.bones()),
            legacy_clips: m.legacy_clips,
        });
    }

    /// Models that have clips and no idle one: the coverage test (AC-R4.1) wants none.
    pub fn animated_without_idle(&self) -> Vec<&str> {
        self.rows
            .iter()
            .filter(|r| r.clips > 0 && !r.idle)
            .map(|r| r.name.as_str())
            .collect()
    }

    /// Slots and bytes of every clip of every model, baked at once: an upper bound of the cache.
    pub fn total_slots(&self) -> u64 {
        self.rows.iter().map(|r| r.slots).sum()
    }

    /// The table of 3.1 with the numbers, then the lists; plain text for the log or a file.
    pub fn render(&self) -> String {
        let mut by_cat: BTreeMap<&str, [u64; 5]> = BTreeMap::new();
        for r in &self.rows {
            let c = by_cat.entry(r.category.as_str()).or_default();
            c[0] += 1;
            c[1] += u64::from(r.clips > 0);
            c[2] += u64::from(r.idle);
            c[3] += u64::from(r.machine.is_some());
            c[4] += r.slots;
        }
        let mut s = String::new();
        let _ = writeln!(
            s,
            "{:<14}{:>8}{:>10}{:>8}{:>10}{:>12}",
            "category", "models", "animated", "idle", "machine", "slots"
        );
        let mut total = [0u64; 5];
        for (cat, c) in &by_cat {
            let _ = writeln!(
                s,
                "{cat:<14}{:>8}{:>10}{:>8}{:>10}{:>12}",
                c[0], c[1], c[2], c[3], c[4]
            );
            for (t, v) in total.iter_mut().zip(c) {
                *t += v;
            }
        }
        let _ = writeln!(
            s,
            "{:<14}{:>8}{:>10}{:>8}{:>10}{:>12}",
            "all", total[0], total[1], total[2], total[3], total[4]
        );
        let _ = writeln!(
            s,
            "baked at once: {} slots, {} rows, {:.1} MiB",
            total[4],
            total[4].div_ceil(u64::from(SLOTS_PER_ROW)),
            (total[4] * BYTES_PER_SLOT) as f64 / (1024.0 * 1024.0)
        );
        let list = |s: &mut String, title: &str, items: Vec<String>| {
            if !items.is_empty() {
                let _ = writeln!(s, "{title} ({}): {}", items.len(), items.join(", "));
            }
        };
        list(
            &mut s,
            "animated without an idle clip",
            self.animated_without_idle()
                .iter()
                .map(|n| (*n).to_owned())
                .collect(),
        );
        list(
            &mut s,
            "graphs not in the table",
            self.unknown_graphs
                .iter()
                .map(|(g, m)| format!("{g} [{} models]", m.len()))
                .collect(),
        );
        list(
            &mut s,
            "codes asked for and missing",
            self.missing_ec
                .iter()
                .map(|(ec, m)| format!("{ec} [{} models]", m.len()))
                .collect(),
        );
        list(
            &mut s,
            "states with nothing to play",
            self.nothing_to_play.clone(),
        );
        s
    }
}

fn push_unique(v: &mut Vec<String>, name: &str) {
    if !v.iter().any(|x| x == name) {
        v.push(name.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::data::fixtures;

    #[test]
    fn the_budget_sums_calls_instances_and_bytes() {
        use crate::world::objects::input::ObjectCounters;
        let c = |i, b| ObjectCounters {
            instances: i,
            batches: b,
            ..ObjectCounters::default()
        };
        let rows = budget(&[("City".into(), c(10, 2)), ("Route".into(), c(5, 1))]);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[2].name, "total");
        assert_eq!(
            (rows[2].calls, rows[2].instances, rows[2].bytes),
            (3, 15, 15 * 64)
        );
        assert!(budget_text(&rows).contains("budget total      calls 3 instances 15 bytes 960"));
    }

    #[test]
    fn rows_round_up() {
        assert_eq!(rows_for(0), 0);
        assert_eq!(rows_for(1), 1);
        assert_eq!(rows_for(512), 1);
        assert_eq!(rows_for(513), 2);
    }

    #[test]
    fn the_line_names_every_counter() {
        let s = AnimStats {
            attached: 41,
            models: 3,
            clips_cached: 12,
            clips_pending: 1,
            used_slots: 9216,
            rows: 18,
            baked_frame: 2,
            evicted_frame: 0,
            uploaded_frame: 98_304,
            ..AnimStats::default()
        };
        let l = s.line();
        for want in [
            "41 attached",
            "12 clips",
            "1 pending",
            "9216 slots",
            "18 rows",
            "98304 B",
        ] {
            assert!(l.contains(want), "{l}");
        }
        assert_eq!(s.counters().len(), 12);
    }

    /// WAN0: the report counts what moves by category and lists what is wrong.
    #[test]
    fn the_report_counts_and_lists() {
        let fsm = fixtures::omni_machine();
        let mut r = CoverageReport::default();
        let (m, pack) = fixtures::model("warrior", 3);
        let warrior = crate::anim::data::ModelAnim::new(&m, Some(pack), Some(&fsm)).unwrap();
        r.add("unit", &warrior);
        let wall = crate::anim::data::ModelAnim::new(
            &asset_format::ModelBlobV2 {
                source: "wall".into(),
                skeleton: fixtures::skeleton(1),
                ..Default::default()
            },
            None,
            None,
        )
        .unwrap();
        r.add("city", &wall);
        // A model with clips and no idle one.
        let (mut m, pack) = fixtures::model("odd", 2);
        for c in &mut m.clips {
            c.ec = 3000 + u32::from(c.index);
            c.name = format!("move{}", c.index);
        }
        let odd = crate::anim::data::ModelAnim::new(&m, Some(pack), None).unwrap();
        r.add("unit", &odd);

        assert_eq!(r.rows.len(), 3);
        assert_eq!(r.animated_without_idle(), ["odd"]);
        assert_eq!(r.unknown_graphs["MYSTERY_GRAPH.dge"], ["warrior"]);
        assert_eq!(r.missing_ec[&1500], ["warrior"]);
        let text = r.render();
        assert!(text.contains("unit"), "{text}");
        assert!(text.contains("graphs not in the table (1)"), "{text}");
        assert!(
            text.contains("animated without an idle clip (1): odd"),
            "{text}"
        );
        // Six clips of the fixture, three bones.
        let frames: u32 = [2.0, 0.5, 1.0, 2.5, 0.5, 1.2]
            .into_iter()
            .map(crate::anim::data::frames_of)
            .sum();
        assert_eq!(r.rows[0].slots, u64::from(frames) * 3);
        assert!(r.total_slots() > u64::from(frames) * 3);
    }
    /// Whether the idle clip of `m` moves: the skinning matrices of its baked frames are not all
    /// those of frame 0 (what the client draws at two times of the loop). `None`: no idle clip.
    fn idle_moves(m: &crate::anim::data::ModelAnim) -> Option<bool> {
        let idle = m.clips.idle()?;
        let (bones, frames) = crate::anim::clip_cache::bake_clip(m, idle.index)?;
        let n = bones.len() / frames.max(1) as usize;
        let first = &bones[..n];
        Some(bones.chunks(n.max(1)).skip(1).any(|f| {
            f.iter()
                .zip(first)
                .any(|(a, b)| a.iter().zip(b).any(|(x, y)| (x - y).abs() > 1e-4))
        }))
    }

    /// Unit models whose idle clip stands still in the pack of the full build: the guided and the
    /// nuclear missile (their idle gr2 holds no motion).
    const STILL_UNIT_IDLES: usize = 2;

    /// WAN0 on the real models (AC-R4.1): every model of the manifest in `CIV5_B2_ASSETS` (a folder
    /// of blobs by hash with the file `manifest`), read through the three formats, with the report
    /// of what moves. It skips (and says so) when the variable is not set.
    #[test]
    fn the_report_of_the_real_models() {
        use asset_format::{ArtRecord, ArtTable, LogicalId, Manifest, ModelBlobV2};
        let Some(dir) = std::env::var_os("CIV5_B2_ASSETS") else {
            eprintln!("the_report_of_the_real_models: CIV5_B2_ASSETS is not set; skipped");
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        let line = std::fs::read_to_string(dir.join("manifest")).expect("manifest");
        let manifest_hash = line.lines().next().expect("hash").trim().to_owned();
        let manifest =
            Manifest::decode(&std::fs::read(dir.join(manifest_hash)).expect("manifest blob"))
                .expect("manifest");
        let blob = |id: &LogicalId| -> Option<Vec<u8>> {
            let v = manifest.variants(id).first()?;
            std::fs::read(dir.join(v.hash.to_string())).ok()
        };
        // The models of the units: the figures of the art table.
        let table = blob(&LogicalId::new("tb:art-b2")).and_then(|b| {
            ArtTable::decode(&b)
                .map_err(|e| eprintln!("art table: {e}"))
                .ok()
        });
        let mut unit_models = std::collections::BTreeSet::new();
        for (_, rec) in table.iter().flat_map(|t| t.iter()) {
            if let ArtRecord::UnitB2(r) = rec {
                let arts = std::iter::once(&r.base)
                    .chain(r.style_variants.iter().map(|v| &v.art))
                    .chain(r.era_variants.iter().map(|v| &v.art));
                for a in arts {
                    unit_models.extend(a.figures.iter().map(|f| f.model.clone()));
                }
            }
        }
        let mut report = CoverageReport::default();
        let mut errors = Vec::new();
        let (mut moving, mut still_units, mut still_other) = (0usize, Vec::new(), Vec::new());
        for (id, _) in manifest.iter() {
            if !id.as_str().starts_with("md:") {
                continue;
            }
            let bytes = blob(id).expect("model blob");
            let model = match ModelBlobV2::decode(&bytes) {
                Ok(m) => m,
                Err(e) => {
                    errors.push(format!("{id:?}: {e}"));
                    continue;
                }
            };
            let pack = model
                .anim_pack
                .as_ref()
                .and_then(&blob)
                .map(|b| asset_format::AnimPack::decode(&b).map_err(|e| e.to_string()));
            let fsm = model
                .fsm
                .as_ref()
                .and_then(&blob)
                .map(|b| asset_format::IdleFsm::decode(&b).map_err(|e| e.to_string()));
            let (pack, fsm) = match (pack.transpose(), fsm.transpose()) {
                (Ok(p), Ok(f)) => (p, f),
                (Err(e), _) | (_, Err(e)) => {
                    errors.push(format!("{id:?}: {e}"));
                    continue;
                }
            };
            match crate::anim::data::ModelAnim::new(&model, pack, fsm.as_ref()) {
                Ok(m) => {
                    let unit = unit_models.contains(id);
                    report.add(if unit { "unit" } else { "other" }, &m);
                    match idle_moves(&m) {
                        Some(true) => moving += 1,
                        Some(false) if unit => still_units.push(m.source.clone()),
                        Some(false) => still_other.push(m.source.clone()),
                        None => {}
                    }
                }
                Err(e) => errors.push(format!("{id:?}: {e}")),
            }
        }
        eprintln!("{}", report.render());
        eprintln!(
            "idle clips that move: {moving}; stand still: {} unit(s) {:?}, {} other {:?}",
            still_units.len(),
            still_units,
            still_other.len(),
            still_other
        );
        // The settler figures with two skeletons played the bind pose before the pipeline packed
        // the body's track group, and Lekmod meshes with bone names (`baghlah`) before a name
        // matched the ID of a stripped base-game clip; what stands still now is still in the clip
        // files too (the missiles), not a binding the pipeline missed.
        for m in ["eurofemale18", "settlers_asian_f3", "baghlah"] {
            assert!(
                !still_units.iter().any(|s| s == m),
                "{m}: the idle stands still"
            );
        }
        assert!(
            still_units.len() <= STILL_UNIT_IDLES,
            "{} unit idles stand still: {still_units:?}",
            still_units.len()
        );
        for e in errors.iter().take(20) {
            eprintln!("error: {e}");
        }
        assert!(errors.is_empty(), "{} models refused", errors.len());
        assert!(
            report.animated_without_idle().is_empty(),
            "{:?}",
            report.animated_without_idle()
        );
    }
}
