//! The program of a state's graph and the machine of a model.
//!
//! In the original a state of the automaton plays a blend graph (`.dge`). The graphs of the
//! install are decoded in [`super::dge`] (RB19); a state's program is the clips of its graph's
//! `to` side with the selections that pick them (path options), each with its loop mode (endless,
//! or once and the last frame held), whether it keeps the phase of the same clip of the state
//! before (`ContinueOffset`), and the entry crossfade of the graph (`BlendTime` seconds, S-curve
//! or linear; none: a hard cut).
//!
//! A clip the model lacks plays nothing: the figure keeps the pose it had (the original's source
//! without an animation). A graph none of whose clips the model has plays the idle clip of the
//! model and writes a line in the report; a graph the table does not know loops the clips of the
//! state's action (a line in the report).

use asset_format::{FsmState, IdleFsm, Transition};

use super::data::{ClipEntry, ClipTable};
use super::dge;

/// Seconds of cross-fade of a change that no graph times (the clip of a landmark, a request no
/// state carries).
pub const CROSSFADE_S: f32 = 0.25;

/// Path option 0: which idle (0 IdleA, 1 IdleB). `1_omni_state_01` sets it by weights.
pub const PATH_IDLE_VARIANT: u16 = 0;
/// Path option 4: the requested action (1000, 1500, 1910..1960; `anim` never asks the others).
pub const PATH_ACTION: u16 = 4;
/// Path options a machine keeps (the aim graphs read up to 8).
pub const PATH_COUNT: usize = 9;

/// The values of the path options of one automaton.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PathOptions {
    values: [i32; PATH_COUNT],
}

impl PathOptions {
    pub fn get(&self, path: u16) -> i32 {
        self.values.get(usize::from(path)).copied().unwrap_or(0)
    }

    pub fn set(&mut self, path: u16, value: i32) {
        if let Some(v) = self.values.get_mut(usize::from(path)) {
            *v = value;
        }
    }
}

/// Where a program came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphSource {
    /// The decoded `.dge` of the install ([`dge`]).
    Decoded,
    /// No graph of that name: the clips of the state's action, or the idle clip.
    Fallback,
}

/// One clip of a program.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphClip {
    /// Index in the pack.
    pub clip: u16,
    pub ec: u32,
    /// `(path, arm, arms)`: the selections that must pick this clip ([`dge::select_index`]);
    /// empty is "always".
    pub when: Vec<(u16, u16, u16)>,
    pub looping: bool,
    /// Keeps the phase of the same clip of the state before (`ContinueOffset`).
    pub cont: bool,
    /// The crossfade into the state: seconds (0: a hard cut), and the S-curve.
    pub crossfade_s: f32,
    pub s_curve: bool,
    /// The crossfade lasts the clip of this code (`BlendEC`).
    pub crossfade_ec: Option<u32>,
}

/// What a state plays.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphProgram {
    pub clips: Vec<GraphClip>,
    pub source: GraphSource,
    /// When the clips are a sequence, the index the sequence goes back to after its last clip
    /// (no decoded graph has one; kept for the machines of `C5FM` v1).
    pub cycle_from: Option<u16>,
}

impl GraphProgram {
    fn empty(source: GraphSource) -> Self {
        Self {
            clips: Vec::new(),
            source,
            cycle_from: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.clips.is_empty()
    }

    /// The clip the selections pick for the options, if the model has it.
    pub fn select(&self, opts: &PathOptions) -> Option<&GraphClip> {
        self.clips.iter().find(|c| {
            c.when.iter().all(|&(p, arm, arms)| {
                dge::select_index(opts.get(p) as f32, usize::from(arms)) == usize::from(arm)
            })
        })
    }
}

/// Why a line is in the report of the program building.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoteKind {
    /// The name of the graph is not in the table.
    UnknownGraph(String),
    /// The model has no clip with this `ec`.
    MissingClip(u32),
    /// The program had nothing to play and plays the idle clip of the model.
    IdleInstead,
    /// The model has no clip to fall back to either: the state plays nothing.
    NothingToPlay,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphNote {
    pub state: String,
    pub kind: NoteKind,
}

/// A state of the machine with its program.
#[derive(Debug, Clone, PartialEq)]
pub struct StateDef {
    pub name: String,
    /// The requested action that enters the state (path option 4).
    pub action: u32,
    /// The graph of the state as the file names it (`IDLE_GRAPH.dge`); empty for a dummy.
    pub graph: String,
    pub program: GraphProgram,
    /// The weighted targets of the chooser: `(state, weight)`.
    pub next: Vec<(u16, f32)>,
    pub transitions: Vec<Transition>,
}

/// The machine of a model: `C5FM` and the programs of its graphs.
#[derive(Debug, Clone, PartialEq)]
pub struct MachineDef {
    pub name: String,
    pub default: u16,
    pub states: Vec<StateDef>,
    /// The length of the model's clip of each code: what `ECComplete` compares the time in the
    /// state with (`FGrannyAssetInstance::AddStateMachineInstance` 0x08ccd362).
    pub ec_durations: std::collections::BTreeMap<u32, f32>,
}

/// The idle clip of the model as a program entry (the stand-in for a graph it has nothing of).
fn idle_clip(clips: &ClipTable) -> Option<GraphClip> {
    let c = clips.idle()?;
    Some(GraphClip {
        clip: c.index,
        ec: c.ec,
        when: Vec::new(),
        looping: true,
        cont: false,
        crossfade_s: CROSSFADE_S,
        s_curve: false,
        crossfade_ec: None,
    })
}

impl GraphProgram {
    /// The program of the graph named `asset` for a state that is entered by `action`, on a model
    /// with `clips`. Lines for the report go to `notes`.
    pub fn for_graph(
        state: &str,
        asset: &str,
        action: u32,
        clips: &ClipTable,
        notes: &mut Vec<GraphNote>,
    ) -> Self {
        let mut note = |kind| {
            notes.push(GraphNote {
                state: state.to_owned(),
                kind,
            })
        };
        if asset.is_empty() {
            return Self::empty(GraphSource::Decoded);
        }
        let mut program;
        let mut missing = None;
        if let Some(node) = dge::graph_of(asset) {
            program = Self::empty(GraphSource::Decoded);
            for leaf in dge::leaves(node) {
                let Some(c) = clips.by_ec(leaf.ec) else {
                    missing.get_or_insert(leaf.ec);
                    continue;
                };
                let (crossfade_s, s_curve) = leaf.blend.unwrap_or((0.0, false));
                program.clips.push(GraphClip {
                    clip: c.index,
                    ec: leaf.ec,
                    when: leaf.when,
                    looping: !leaf.once,
                    cont: leaf.cont,
                    crossfade_s,
                    s_curve,
                    crossfade_ec: leaf.blend_ec,
                });
            }
        } else {
            note(NoteKind::UnknownGraph(asset.to_owned()));
            program = Self::empty(GraphSource::Fallback);
            program.clips = clips
                .all_ec(action)
                .map(|c: &ClipEntry| GraphClip {
                    clip: c.index,
                    ec: c.ec,
                    when: Vec::new(),
                    looping: true,
                    cont: false,
                    crossfade_s: CROSSFADE_S,
                    s_curve: false,
                    crossfade_ec: None,
                })
                .collect();
            if program.clips.is_empty() {
                missing = Some(action);
            }
        }
        if let Some(ec) = missing {
            note(NoteKind::MissingClip(ec));
        }
        if program.clips.is_empty() {
            match idle_clip(clips) {
                Some(c) => {
                    program.clips = vec![c];
                    program.source = GraphSource::Fallback;
                    note(NoteKind::IdleInstead);
                }
                None => note(NoteKind::NothingToPlay),
            }
        }
        program
    }
}

impl MachineDef {
    /// Builds the machine of `fsm` for a model with `clips`; the lines for the report go to
    /// `notes`.
    pub fn build(fsm: &IdleFsm, clips: &ClipTable, notes: &mut Vec<GraphNote>) -> Self {
        let states = fsm
            .states
            .iter()
            .map(|s: &FsmState| StateDef {
                name: s.name.clone(),
                action: s.action,
                graph: s.asset.clone(),
                program: GraphProgram::for_graph(&s.name, &s.asset, s.action, clips, notes),
                next: s.next.clone(),
                transitions: s.transitions.clone(),
            })
            .collect();
        let mut ec_durations = std::collections::BTreeMap::new();
        for c in clips.entries() {
            ec_durations.entry(c.ec).or_insert(c.duration);
        }
        Self {
            name: fsm.name.clone(),
            default: fsm.default,
            states,
            ec_durations,
        }
    }

    /// The length of the model's clip of `ec`; 0 when it has none (`ECComplete` then holds one
    /// update after the entry).
    pub fn ec_duration(&self, ec: u32) -> f32 {
        self.ec_durations.get(&ec).copied().unwrap_or(0.0)
    }

    /// The state a request for `action` enters: the first one with that action that plays
    /// something.
    pub fn state_for_action(&self, action: u32) -> Option<(usize, &StateDef)> {
        if action == 0 {
            return None;
        }
        self.states
            .iter()
            .enumerate()
            .find(|(_, s)| s.action == action && !s.program.is_empty())
    }

    /// The states reachable from the default one by `next` and by the transitions.
    pub fn reachable(&self) -> Vec<u16> {
        let n = self.states.len();
        if n == 0 {
            return Vec::new();
        }
        let mut seen = vec![false; n];
        let mut stack = vec![self.default];
        while let Some(s) = stack.pop() {
            if usize::from(s) >= n || std::mem::replace(&mut seen[usize::from(s)], true) {
                continue;
            }
            let st = &self.states[usize::from(s)];
            stack.extend(st.next.iter().map(|&(t, _)| t));
            stack.extend(st.transitions.iter().map(|t| t.to));
        }
        (0..n as u16).filter(|&i| seen[usize::from(i)]).collect()
    }

    /// True if no transition of the machine has a condition: the machine is the weights `next` of
    /// `C5FM` v1 and the automaton goes by the rules of its legacy mode (`fsm.rs`).
    pub fn is_legacy(&self) -> bool {
        self.states
            .iter()
            .all(|s| s.transitions.iter().all(|t| t.conditions.is_empty()))
    }

    /// The target of a chooser state for the draw `u` in `[0, 1)`: weights are fractions of their
    /// sum. `None` for a state with no (or only zero) weights.
    pub fn pick_next(&self, state: u16, u: f32) -> Option<u16> {
        let st = self.states.get(usize::from(state))?;
        let total: f32 = st.next.iter().map(|&(_, w)| w).sum();
        if st.next.is_empty() || total <= 0.0 {
            return None;
        }
        let mut x = u.clamp(0.0, 0.999_999) * total;
        for &(to, w) in &st.next {
            if x < w {
                return Some(to);
            }
            x -= w;
        }
        st.next.last().map(|&(to, _)| to)
    }

    /// True if no state's action is one that must never be requested (attack, death, run, victory,
    /// capture; BUILD 5, AC-R4.2).
    pub fn has_no_forbidden_state(&self) -> bool {
        self.states
            .iter()
            .all(|s| !super::data::EC_NEVER.contains(&s.action))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::AnimRng;
    use crate::anim::data::{ModelAnim, fixtures};

    fn clips_with(ecs: &[u32]) -> ClipTable {
        ClipTable::new(
            ecs.iter()
                .enumerate()
                .map(|(i, &ec)| ClipEntry {
                    name: format!("c{ec}"),
                    ec,
                    index: i as u16,
                    duration: 1.0,
                })
                .collect(),
        )
    }

    fn program(graph: &str, action: u32, ecs: &[u32]) -> (GraphProgram, Vec<GraphNote>) {
        let mut notes = Vec::new();
        let p = GraphProgram::for_graph("S", graph, action, &clips_with(ecs), &mut notes);
        (p, notes)
    }

    fn ecs(p: &GraphProgram) -> Vec<(u32, bool)> {
        p.clips.iter().map(|c| (c.ec, c.looping)).collect()
    }

    fn with(pairs: &[(u16, i32)]) -> PathOptions {
        let mut o = PathOptions::default();
        for &(p, v) in pairs {
            o.set(p, v);
        }
        o
    }

    const ALL: [u32; 12] = [
        1000, 1020, 1040, 2000, 2020, 2040, 1500, 1520, 1540, 1560, 1580, 1620,
    ];

    /// IDLE_GRAPH: IdleA or IdleB by option 0, endless, a hard cut at the entry, no transition
    /// clip inside (TRANSITIONS_FOR_IDLES plays those).
    #[test]
    fn the_idle_graph_selects_a_or_b_and_cuts_in() {
        let (p, notes) = program("IDLE_GRAPH.dge", 1000, &ALL);
        assert!(notes.is_empty(), "{notes:?}");
        assert_eq!(p.source, GraphSource::Decoded);
        assert_eq!(ecs(&p), [(1000, true), (2000, true)]);
        assert_eq!(p.select(&with(&[(0, 0)])).unwrap().ec, 1000);
        assert_eq!(p.select(&with(&[(0, 1)])).unwrap().ec, 2000);
        // Option 0 past the inputs takes the last one (the Selection clamps).
        assert_eq!(p.select(&with(&[(0, 5)])).unwrap().ec, 2000);
        assert!(p.clips.iter().all(|c| c.crossfade_s == 0.0 && !c.cont));
        // The routing dummy keeps the phase of the idle it came from.
        let (p, _) = program("IDLE_OFFSET_CONTINUE_TRUE_GRAPH.dge", 1000, &ALL);
        assert!(p.clips.iter().all(|c| c.cont && c.looping));
    }

    /// TRANSITIONS_FOR_IDLES and FIDGETS_GRAPH: the clip of the variant once, S-curve 0.1 s and
    /// 0.3333 s.
    #[test]
    fn the_idle_transitions_and_fidgets_play_once_after_their_blend() {
        let (p, _) = program("TRANSITIONS_FOR_IDLES.dge", 0, &ALL);
        assert_eq!(ecs(&p), [(1020, false), (2020, false)]);
        let c = p.select(&with(&[(0, 1)])).unwrap();
        assert_eq!((c.ec, c.crossfade_s, c.s_curve), (2020, 0.1, true));
        let (p, _) = program("FIDGETS_GRAPH.dge", 1000, &ALL);
        assert_eq!(ecs(&p), [(1040, false), (2040, false)]);
        assert!((p.clips[0].crossfade_s - 0.3333).abs() < 1e-4);
        // The aim twin blends its transitions over 0.25 s.
        let (p, _) = program("AIM_TRANSITIONS_FOR_IDLES.dge", 0, &ALL);
        assert_eq!(p.clips[0].crossfade_s, 0.25);
    }

    /// The fortify family (RB19 1.2): ENTER_FORTIFY 1500 once (1520 when option 6 is 1) over
    /// 0.15 s; FORTIFY_GRAPH the held pose 1520 endless, phase kept, no blend; FORTIFY_FIDGET 1540
    /// once over 0.18 s; FORTIFY_LEAVE 1580 / 1560 / 1620 by option 1, once, over 0.2 s.
    #[test]
    fn the_fortify_graphs_are_those_of_the_install() {
        let (p, _) = program("ENTER_FORTIFY.dge", 0, &ALL);
        assert_eq!(ecs(&p), [(1500, false), (1520, false)]);
        let c = p.select(&with(&[(6, 0)])).unwrap();
        assert_eq!((c.ec, c.crossfade_s, c.s_curve), (1500, 0.15, true));
        assert_eq!(p.select(&with(&[(6, 1)])).unwrap().ec, 1520);
        let (p, _) = program("FORTIFY_GRAPH.dge", 1500, &ALL);
        assert_eq!(ecs(&p), [(1520, true)]);
        assert!(p.clips[0].cont && p.clips[0].crossfade_s == 0.0);
        let (p, _) = program("FORTIFY_FIDGET_GRAPH.dge", 0, &ALL);
        assert_eq!(ecs(&p), [(1540, false)]);
        assert_eq!(p.clips[0].crossfade_s, 0.18);
        let (p, _) = program("FORTIFY_LEAVE_GRAPH.dge", 0, &ALL);
        assert_eq!(ecs(&p), [(1580, false), (1560, false), (1620, false)]);
        assert_eq!(p.select(&with(&[(1, 0)])).unwrap().ec, 1580);
        assert_eq!(p.select(&with(&[(1, 1)])).unwrap().ec, 1560);
        assert_eq!(p.select(&with(&[(1, 2)])).unwrap().ec, 1620);
        assert_eq!(p.clips[0].crossfade_s, 0.2);
        // The aim twins: the body of the plain graph, their own blend times.
        let (p, _) = program("AIM_FORTIFY_LEAVE_GRAPH.dge", 0, &ALL);
        assert_eq!(p.clips[0].crossfade_s, 0.15);
        let (p, _) = program("AIM_FORTIFY_FIDGET_GRAPH.dge", 0, &ALL);
        assert_eq!(p.clips[0].crossfade_s, 0.25);
    }

    /// The civilian and worker sleep graphs and the worker's work.
    #[test]
    fn the_worker_graphs_select_their_clips() {
        let (p, _) = program("WORKER_ENTER_SLEEP.dge", 0, &ALL);
        assert_eq!(ecs(&p), [(1500, false)]);
        assert_eq!(p.clips[0].crossfade_s, 0.2);
        let (p, _) = program("WORKER_EXIT_SLEEP.dge", 0, &ALL);
        assert_eq!(ecs(&p), [(1580, false)]);
        let work = [1910, 1920, 1930, 1940, 1950, 1960];
        let (p, _) = program("WORKER_WORK.dge", 1900, &work);
        assert_eq!(p.select(&with(&[(2, 2)])).unwrap().ec, 1930);
        assert!(p.clips.iter().all(|c| c.looping && c.cont));
    }

    #[test]
    fn a_clip_the_model_lacks_plays_nothing_and_a_graph_of_nothing_plays_idle() {
        // No 1540: the fortify leave by option 1 = 1 picks nothing (the figure keeps its pose).
        let (p, notes) = program("FORTIFY_LEAVE_GRAPH.dge", 0, &[1000, 1520, 1580]);
        assert_eq!(ecs(&p), [(1580, false)]);
        assert!(p.select(&with(&[(1, 1)])).is_none());
        assert!(notes.iter().any(|n| n.kind == NoteKind::MissingClip(1560)));
        let (p, notes) = program("FORTIFY_GRAPH.dge", 1500, &[1000, 2000]);
        assert_eq!(ecs(&p), [(1000, true)]);
        assert!(notes.iter().any(|n| n.kind == NoteKind::MissingClip(1520)));
        assert!(notes.iter().any(|n| n.kind == NoteKind::IdleInstead));
        let (p, notes) = program("IDLE_GRAPH", 1000, &[]);
        assert!(p.is_empty());
        assert!(notes.iter().any(|n| n.kind == NoteKind::NothingToPlay));
    }

    #[test]
    fn an_unknown_graph_loops_the_clips_of_its_action() {
        let (p, notes) = program("MYSTERY_GRAPH.dge", 1234, &[1000, 1234]);
        assert_eq!(ecs(&p), [(1234, true)]);
        assert_eq!(p.source, GraphSource::Fallback);
        assert!(
            notes
                .iter()
                .any(|n| n.kind == NoteKind::UnknownGraph("MYSTERY_GRAPH.dge".into()))
        );
        let (p, notes) = program("MYSTERY_GRAPH.dge", 1234, &[1000]);
        assert_eq!(p.clips[0].ec, 1000);
        assert!(notes.iter().any(|n| n.kind == NoteKind::MissingClip(1234)));
    }

    #[test]
    fn a_dummy_state_has_an_empty_program_and_no_line() {
        let (p, notes) = program("", 0, &[1000]);
        assert!(p.is_empty() && notes.is_empty());
    }

    /// WAN1 test of the spec: on a machine shaped like `1_omni_state_01` every state reachable
    /// from the default one gets a program; the odd name is a line of the report.
    #[test]
    fn the_omni_machine_gets_a_program_for_every_reachable_state() {
        let (m, pack) = fixtures::model("warrior", 3);
        let fsm = fixtures::omni_machine();
        let a = ModelAnim::new(&m, Some(pack), Some(&fsm)).unwrap();
        let machine = a.machine.as_ref().unwrap();
        let reach = machine.reachable();
        assert!(reach.len() >= 3, "{reach:?}");
        for s in &reach {
            let st = &machine.states[usize::from(*s)];
            if !st.graph.is_empty() {
                assert!(!st.program.is_empty(), "{} has no program", st.name);
            }
        }
        assert!(machine.states[0].program.is_empty());
        assert_eq!(machine.states[1].program.clips[0].ec, 1000);
        assert!(machine.states[5].program.clips.iter().all(|c| c.cont));
        // FORTIFY has no clips on this model: it stands on idle, and the report says so.
        assert!(
            a.notes
                .iter()
                .any(|n| n.state == "FORTIFY_LOOP" && n.kind == NoteKind::MissingClip(1520))
        );
        assert!(
            a.notes
                .iter()
                .any(|n| n.state == "MYSTERY" && matches!(n.kind, NoteKind::UnknownGraph(_)))
        );
        assert_eq!(machine.state_for_action(1000).unwrap().1.name, "IDLE_LOOP");
        assert_eq!(
            machine.state_for_action(1500).unwrap().1.name,
            "FORTIFY_LOOP"
        );
        assert!(machine.state_for_action(0).is_none());
        assert!(machine.state_for_action(1100).is_none());
        assert!(machine.has_no_forbidden_state());
        assert_eq!(machine.ec_duration(1040), 1.0);
        assert_eq!(machine.ec_duration(1500), 0.0);
    }
    #[test]
    fn a_machine_with_an_attack_state_is_caught() {
        let (m, pack) = fixtures::model("warrior", 3);
        let mut fsm = fixtures::omni_machine();
        fsm.states[6].action = 1100;
        let a = ModelAnim::new(&m, Some(pack), Some(&fsm)).unwrap();
        assert!(!a.machine.unwrap().has_no_forbidden_state());
    }

    /// WAN1 test of the spec: the weights 0.4 / 0.6 of the chooser give those shares within 2 %
    /// over 10 000 draws.
    #[test]
    fn the_chooser_weights_hold_over_ten_thousand_draws() {
        let (m, pack) = fixtures::model("warrior", 3);
        let a = ModelAnim::new(&m, Some(pack), Some(&fixtures::omni_machine())).unwrap();
        let machine = a.machine.unwrap();
        let mut rng = AnimRng::new(0xC1F5);
        let mut to_idle = 0u32;
        const N: u32 = 10_000;
        for _ in 0..N {
            if machine.pick_next(0, rng.unit()) == Some(1) {
                to_idle += 1;
            }
        }
        let share = f64::from(to_idle) / f64::from(N);
        assert!((share - 0.4).abs() < 0.02, "{share}");
        // Edges: the first and the last draw, a state with no weights.
        assert_eq!(machine.pick_next(0, 0.0), Some(1));
        assert_eq!(machine.pick_next(0, 0.999_99), Some(2));
        assert_eq!(machine.pick_next(1, 0.5), None);
        assert_eq!(machine.pick_next(99, 0.5), None);
    }
}
