//! The sounds of the units (re-b15 ITEM 4): the sound rows of the units' timed triggers, played at the members as the
//! clips pass them. The rules of the rows are `audio::UnitSoundBook`; this watches the clips the renderer plays and the
//! actions the port does not animate, and says which 3D scripts sound where.
//!
//! The figures near the camera come every frame with the clip each one plays and the time in it; a row fires once as
//! the time passes it. The port plays only the standing clips (idle, fortify, a worker's work: SIMP-010), so a move, an
//! attack and a death, which the exe animates, come as cues from the game's events and play every row of the clip the
//! exe would play, timed from the cue.

use std::collections::{HashMap, HashSet};

use audio::{Macros, UnitSoundBook, resolve_unit_sound, tier_fits};
use render::glam::{Vec2, Vec3};
use render::world::unit::FigureClip;

/// `ec` of the clips the cues stand for (the `Animation` codes of the units' `.fxsxml`).
pub const EC_RUN: u32 = 1400;
pub const EC_ATTACK: u32 = 1100;
pub const EC_ATTACK_CITY: u32 = 1160;
pub const EC_DEATH: u32 = 1200;
pub const EC_VICTORY: u32 = 1800;

/// A cue is forgotten if its rows did not come due by then (a unit that left the view).
const CUE_LIFE_MS: u64 = 10_000;
/// A death comes in the same batch as the attack that caused it: its clip starts when the blow has landed, and the
/// killer's victory clip after the fall.
const DEATH_DELAY_MS: u64 = 500;
const VICTORY_DELAY_MS: u64 = 1000;

/// An action of a unit the exe animates and the port does not (`ExeHost::unit_clip_sound`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnitCue {
    pub owner: i32,
    pub id: i32,
    pub ec: u32,
    /// The plot it happens on (a move: the plot it went to).
    pub x: i32,
    pub y: i32,
    /// The other side of a fight.
    pub opponent: Option<(i32, i32)>,
}

impl UnitCue {
    /// The cue of a queue entry (`m2::UNIT_CUE_PREFIX` stripped): `ec:owner:id:x:y:opp_owner:opp_id`.
    pub fn parse(s: &str) -> Option<UnitCue> {
        let v: Vec<i32> = s
            .split(':')
            .map(|p| p.parse::<i32>())
            .collect::<Result<_, _>>()
            .ok()?;
        let [ec, owner, id, x, y, oo, oid] = v[..] else {
            return None;
        };
        Some(UnitCue {
            owner,
            id,
            ec: u32::try_from(ec).ok()?,
            x,
            y,
            opponent: (oo >= 0 && oid >= 0).then_some((oo, oid)),
        })
    }

    /// The queue entry of the cue (without the prefix).
    pub fn format(&self) -> String {
        let (oo, oid) = self.opponent.unwrap_or((-1, -1));
        format!(
            "{}:{}:{}:{}:{}:{oo}:{oid}",
            self.ec, self.owner, self.id, self.x, self.y
        )
    }
}

/// What the units sounded like at the last look: their figures (models and places), for the cues of a unit whose
/// figures are gone by the time its cue comes (a death) or that jumped (a move).
#[derive(Debug, Clone)]
struct Squad {
    figures: Vec<(String, Vec3)>,
    seen_ms: u64,
}

/// A row of a cue's clip that waits for its time.
#[derive(Debug, Clone)]
struct Due {
    at_ms: u64,
    script: String,
    position: Vec3,
}

/// One sound to play now: a 3D script at a world point.
#[derive(Debug, Clone, PartialEq)]
pub struct UnitPlay {
    pub script: String,
    pub position: [f32; 3],
}

pub struct UnitSound {
    book: UnitSoundBook,
    /// The clip and the time each figure was at, at the last look.
    last: HashMap<((i8, i32), u8), (u32, f32)>,
    squads: HashMap<(i8, i32), Squad>,
    /// The last unit that attacked each unit: its weapon is the `$(WEAPON)` of the death's impact.
    attackers: HashMap<(i32, i32), (i32, i32)>,
    due: Vec<Due>,
    /// The models seen so far, and the scripts of the ones seen since [`Self::take_preloads`]: their files are fetched
    /// ahead, so the first step of a unit is not lost to the fetch.
    known: HashSet<String>,
    preloads: Vec<String>,
}

impl UnitSound {
    pub fn new(book: UnitSoundBook) -> Self {
        Self {
            book,
            last: HashMap::new(),
            squads: HashMap::new(),
            attackers: HashMap::new(),
            due: Vec::new(),
            known: HashSet::new(),
            preloads: Vec::new(),
        }
    }

    /// The scripts of the models that came into view since the last call (their rows, on the plot they stand on).
    pub fn take_preloads(&mut self) -> Vec<String> {
        std::mem::take(&mut self.preloads)
    }

    pub fn models(&self) -> usize {
        self.book.len()
    }

    fn macros<'a>(
        &'a self,
        model: &str,
        terrain: &'a str,
        opponent: Option<&'a str>,
    ) -> Macros<'a> {
        let m = self.book.model(model);
        Macros {
            terrain,
            weapon: m.and_then(|m| m.weapon.as_deref()).or(opponent),
            material: m.and_then(|m| m.material.as_deref()),
        }
    }

    /// The weapon of the first member of a unit (an opponent's `$(WEAPON)`).
    fn weapon_of(&self, unit: (i32, i32)) -> Option<&str> {
        let squad = self.squads.get(&(unit.0 as i8, unit.1))?;
        let (model, _) = squad.figures.first()?;
        self.book.model(model)?.weapon.as_deref()
    }

    /// One frame: the figures near the listener and their clips. Returns what sounds now: the rows the clips passed
    /// since the last look, and the rows of the cues that came due. `terrain` gives the `EffectTypeTag` of the plot
    /// under a point (X, Z).
    pub fn step(
        &mut self,
        now_ms: u64,
        figures: &[FigureClip],
        terrain: &dyn Fn(Vec2) -> &'static str,
    ) -> Vec<UnitPlay> {
        let mut out = Vec::new();
        let mut seen: HashMap<((i8, i32), u8), (u32, f32)> = HashMap::with_capacity(figures.len());
        let mut squads: HashMap<(i8, i32), Squad> = HashMap::new();
        for f in figures {
            let model = f.model.as_str();
            squads
                .entry(f.unit)
                .or_insert_with(|| Squad {
                    figures: Vec::new(),
                    seen_ms: now_ms,
                })
                .figures
                .push((model.to_owned(), f.position));
            if !self.known.contains(model) && self.book.model(model).is_some() {
                self.known.insert(model.to_owned());
                let tag = terrain(Vec2::new(f.position.x, f.position.z));
                let mut scripts: Vec<String> = self
                    .book
                    .model(model)
                    .map(|m| m.triggers.iter().map(|r| r.event.clone()).collect())
                    .unwrap_or_default();
                scripts.sort();
                scripts.dedup();
                let macros = self.macros(model, tag, None);
                let resolved: Vec<String> = scripts
                    .iter()
                    .filter_map(|e| resolve_unit_sound(e, &macros))
                    .collect();
                self.preloads.extend(resolved);
            }
            let Some((ec, t)) = f.clip else { continue };
            let key = (f.unit, f.index);
            seen.insert(key, (ec, t));
            // A figure seen for the first time sounds from its next row on (it came into view mid-clip).
            let Some(&(last_ec, last_t)) = self.last.get(&key) else {
                continue;
            };
            let from = (last_ec == ec).then_some(last_t);
            if from == Some(t) {
                continue;
            }
            let tag = terrain(Vec2::new(f.position.x, f.position.z));
            for row in self.book.crossed(model, ec, from, t) {
                if !tier_fits(row.tier, usize::from(f.members)) {
                    continue;
                }
                if let Some(script) = resolve_unit_sound(&row.event, &self.macros(model, tag, None))
                {
                    out.push(UnitPlay {
                        script,
                        position: f.position.to_array(),
                    });
                }
            }
        }
        self.last = seen;
        // The squads seen now replace their old entry; the others are kept a while (a cue may still come for them).
        for (k, s) in squads {
            self.squads.insert(k, s);
        }
        self.squads
            .retain(|_, s| now_ms.saturating_sub(s.seen_ms) <= CUE_LIFE_MS);
        let (now, later): (Vec<Due>, Vec<Due>) = std::mem::take(&mut self.due)
            .into_iter()
            .partition(|d| d.at_ms <= now_ms);
        self.due = later;
        out.extend(
            now.into_iter()
                .filter(|d| now_ms - d.at_ms <= CUE_LIFE_MS)
                .map(|d| UnitPlay {
                    script: d.script,
                    position: d.position.to_array(),
                }),
        );
        out
    }

    /// An action the port does not animate: every row of its clip, for each member of the unit at its place, timed
    /// from `now_ms`. A unit that was not seen near the listener is not heard (it is beyond the reach of its sounds).
    ///
    /// SIMPLIFIED(SIMP-exe-1202): a move plays one cycle of the run clip at the plot it went to (the port's move is a
    /// jump, WO-D6), an attack the attack clip (the city attack against a city), a death the death clip half a second
    /// later and the killer's victory clip a second after that; the exe plays the clips its combat and move simulation picks
    /// (`cvCombatSimSystem`, the `.fsmxml` of the unit), which are not run here.
    pub fn cue(&mut self, now_ms: u64, cue: &UnitCue, terrain: &dyn Fn(Vec2) -> &'static str) {
        let key = (cue.owner as i8, cue.id);
        if let Some(opp) = cue.opponent
            && matches!(cue.ec, EC_ATTACK | EC_ATTACK_CITY)
        {
            self.attackers.insert(opp, (cue.owner, cue.id));
        }
        let Some(squad) = self.squads.get(&key).cloned() else {
            return;
        };
        let start_ms = if cue.ec == EC_DEATH {
            now_ms + DEATH_DELAY_MS
        } else {
            now_ms
        };
        let opponent = cue.opponent.or_else(|| {
            (cue.ec == EC_DEATH)
                .then(|| self.attackers.get(&(cue.owner, cue.id)).copied())
                .flatten()
        });
        let opp_weapon = opponent.and_then(|o| self.weapon_of(o)).map(str::to_owned);
        // A move puts the squad on its new plot, as the next placement will.
        let shift = if cue.ec == EC_RUN {
            let centre = squad
                .figures
                .iter()
                .map(|(_, p)| Vec2::new(p.x, p.z))
                .sum::<Vec2>()
                / squad.figures.len().max(1) as f32;
            render::hexgrid::cell_center(cue.x, cue.y) - centre
        } else {
            Vec2::ZERO
        };
        let members = squad.figures.len();
        for (model, p) in &squad.figures {
            let position = *p + Vec3::new(shift.x, 0.0, shift.y);
            let tag = terrain(Vec2::new(position.x, position.z));
            for row in self.book.rows(model, cue.ec) {
                if !tier_fits(row.tier, members) {
                    continue;
                }
                if let Some(script) =
                    resolve_unit_sound(&row.event, &self.macros(model, tag, opp_weapon.as_deref()))
                {
                    self.due.push(Due {
                        at_ms: start_ms + (row.time.max(0.0) * 1000.0) as u64,
                        script,
                        position,
                    });
                }
            }
        }
        // The killer cheers.
        if cue.ec == EC_DEATH
            && let Some(k) = opponent
        {
            self.attackers.remove(&(cue.owner, cue.id));
            let victory = UnitCue {
                owner: k.0,
                id: k.1,
                ec: EC_VICTORY,
                x: cue.x,
                y: cue.y,
                opponent: None,
            };
            self.cue(start_ms + VICTORY_DELAY_MS, &victory, terrain);
        }
    }

    /// The game is left: nothing waits.
    pub fn clear(&mut self) {
        self.preloads.clear();
        self.last.clear();
        self.squads.clear();
        self.attackers.clear();
        self.due.clear();
    }
}

#[cfg(test)]
mod tests {
    use asset_format::{LogicalId, ModelSounds, Sound3DTable, SoundTrigger};

    use super::*;

    fn row(ec: u32, time: f32, tier: i8, event: &str) -> SoundTrigger {
        SoundTrigger {
            ec,
            time,
            tier,
            event: event.into(),
        }
    }

    fn book() -> UnitSoundBook {
        UnitSoundBook::new(&Sound3DTable {
            scripts: vec![],
            models: vec![
                ModelSounds {
                    model: "archer".into(),
                    weapon: Some("ARROW".into()),
                    material: Some("CLOTH".into()),
                    triggers: vec![
                        row(1000, 0.5, 0, "AS3D_A_IDLE_$(TERRAIN)"),
                        row(1000, 0.7, 1, "AS3D_A_T1"),
                        row(EC_RUN, 0.2, 0, "AS3D_A_FOOT_$(TERRAIN)"),
                        row(EC_RUN, 0.6, 0, "AS3D_A_FOOT_$(TERRAIN)"),
                        row(EC_ATTACK, 0.3, -1, "AS3D_A_BOW"),
                        row(EC_VICTORY, 0.1, -1, "AS3D_A_CHEER"),
                    ],
                },
                ModelSounds {
                    model: "warrior".into(),
                    weapon: None,
                    material: None,
                    triggers: vec![row(EC_DEATH, 0.1, -1, "AS3D_W_IMPACT_$(WEAPON)")],
                },
            ],
            pitch_2d: vec![],
        })
    }

    fn fig(
        unit: (i8, i32),
        index: u8,
        model: &str,
        x: f32,
        clip: Option<(u32, f32)>,
    ) -> FigureClip {
        FigureClip {
            unit,
            index,
            model: LogicalId::new(format!("md:{model}")),
            position: Vec3::new(x, 0.0, 0.0),
            clip,
            members: 3,
        }
    }

    fn grass(_: Vec2) -> &'static str {
        "DIRT"
    }

    fn scripts(p: &[UnitPlay]) -> Vec<&str> {
        p.iter().map(|u| u.script.as_str()).collect()
    }

    #[test]
    fn a_row_sounds_when_the_clip_of_a_figure_passes_it_and_only_for_its_tier() {
        let mut s = UnitSound::new(book());
        // First sight: nothing (it came into view mid-clip), but the files of its rows are fetched ahead.
        assert!(
            s.step(
                0,
                &[fig((0, 1), 0, "archer", 10.0, Some((1000, 0.4)))],
                &grass
            )
            .is_empty()
        );
        let mut pre = s.take_preloads();
        pre.sort();
        assert_eq!(
            pre,
            [
                "AS3D_A_BOW",
                "AS3D_A_CHEER",
                "AS3D_A_FOOT_DIRT",
                "AS3D_A_IDLE_DIRT",
                "AS3D_A_T1"
            ]
        );
        assert!(s.take_preloads().is_empty());
        let p = s.step(
            16,
            &[fig((0, 1), 0, "archer", 10.0, Some((1000, 0.8)))],
            &grass,
        );
        // The tier-1 row is for a squad of 4 to 8: a squad of three hears only its tier-0 row.
        assert_eq!(scripts(&p), ["AS3D_A_IDLE_DIRT"]);
        assert_eq!(p[0].position, [10.0, 0.0, 0.0]);
        assert!(
            s.step(
                32,
                &[fig((0, 1), 0, "archer", 10.0, Some((1000, 0.9)))],
                &grass
            )
            .is_empty()
        );
        // The loop wraps past 0.5 again.
        let again = s.step(
            48,
            &[fig((0, 1), 0, "archer", 10.0, Some((1000, 0.6)))],
            &grass,
        );
        assert_eq!(scripts(&again), ["AS3D_A_IDLE_DIRT"]);
    }

    #[test]
    fn each_member_plays_its_own_rows_and_a_new_clip_starts_from_its_start() {
        let mut s = UnitSound::new(book());
        let squad = |clip| {
            vec![
                fig((0, 1), 0, "archer", 0.0, clip),
                fig((0, 1), 1, "archer", 5.0, clip),
                fig((0, 1), 2, "archer", 9.0, clip),
            ]
        };
        s.step(0, &squad(Some((1000, 0.1))), &grass);
        // Fortify came in (another ec): its rows from 0. No row of 1500 in the book: silent; back to idle from 0..0.55.
        assert!(s.step(10, &squad(Some((1500, 0.2))), &grass).is_empty());
        let p = s.step(20, &squad(Some((1000, 0.55))), &grass);
        assert_eq!(p.len(), 3, "three archers, three sounds: {p:?}");
    }

    #[test]
    fn a_move_plays_the_run_rows_at_the_new_plot_timed_from_the_cue() {
        let mut s = UnitSound::new(book());
        s.step(0, &[fig((0, 1), 0, "archer", 0.0, None)], &grass);
        let cue = UnitCue {
            owner: 0,
            id: 1,
            ec: EC_RUN,
            x: 3,
            y: 2,
            opponent: None,
        };
        assert_eq!(UnitCue::parse(&cue.format()), Some(cue));
        s.cue(1000, &cue, &grass);
        assert!(
            s.step(1100, &[], &grass).is_empty(),
            "the first step is at 0.2 s"
        );
        let p = s.step(1250, &[], &grass);
        assert_eq!(scripts(&p), ["AS3D_A_FOOT_DIRT"]);
        let centre = render::hexgrid::cell_center(3, 2);
        assert!((Vec2::new(p[0].position[0], p[0].position[2]) - centre).length() < 1e-3);
        assert_eq!(scripts(&s.step(1700, &[], &grass)), ["AS3D_A_FOOT_DIRT"]);
        // A unit never seen near the listener is not heard.
        s.cue(2000, &UnitCue { id: 9, ..cue }, &grass);
        assert!(s.step(5000, &[], &grass).is_empty());
    }

    #[test]
    fn a_death_takes_the_killers_weapon_and_the_killer_cheers() {
        let mut s = UnitSound::new(book());
        s.step(
            0,
            &[
                fig((0, 1), 0, "archer", 0.0, None),
                fig((1, 7), 0, "warrior", 64.0, None),
            ],
            &grass,
        );
        s.cue(
            100,
            &UnitCue {
                owner: 0,
                id: 1,
                ec: EC_ATTACK,
                x: 1,
                y: 0,
                opponent: Some((1, 7)),
            },
            &grass,
        );
        s.cue(
            200,
            &UnitCue {
                owner: 1,
                id: 7,
                ec: EC_DEATH,
                x: 1,
                y: 0,
                opponent: None,
            },
            &grass,
        );
        let mut heard = Vec::new();
        for t in (200..3000).step_by(50) {
            heard.extend(s.step(t, &[], &grass).into_iter().map(|p| p.script));
        }
        assert_eq!(heard, ["AS3D_A_BOW", "AS3D_W_IMPACT_ARROW", "AS3D_A_CHEER"]);
    }

    #[test]
    fn a_cue_that_is_not_a_cue_is_refused() {
        assert_eq!(UnitCue::parse("1400:0:1"), None);
        assert_eq!(UnitCue::parse("x:0:1:2:3:-1:-1"), None);
        assert_eq!(UnitCue::parse("1400:0:1:2:3:-1:-1").unwrap().opponent, None);
    }
}
