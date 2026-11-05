//! The automaton of a figure: the states, the transitions with their
//! conditions and the path options of `C5FM` v2, stepped 30 times a second, one instance and one
//! generator per figure.
//!
//! **Step.** The clock of the machine is a tick count (`tick / 30` seconds), not frames: a frame
//! that spans several ticks runs them one after the other, a frame that spans none runs none, so 30
//! and 144 frames a second give the same sequence (WA-D6). Everything the machine draws (the
//! weights, `RandomRoll`) is drawn on its ticks, from the stream of the figure.
//!
//! **Transitions.** The transitions of the current state are walked in the order of the file; the
//! conditions of one are split into groups by `Or`; a transition fires when every check of some
//! group holds (`CheckPathOption`, `RandomRoll`, `CheckStateTime`, `EcComplete`). The `Event*`
//! conditions are actions: those of the group that held run when it fires (`EventSetPathOption`,
//! `EventRandomPathOption`). A transition with no conditions fires at once. That is our reading of
//! the data. One number is rolled per tick and every `RandomRoll` of that tick compares with it
//! (`roll < value`, or `value <= roll` inverted); `EcComplete` holds once the time in the state
//! passes the length of the model's clip of the condition's code.
// re: FFiniteStateMachineInstance vf_10 0x087fbcd2 (roll = one FRandom draw per update),
// FFSMConditionRandomRoll::evaluate 0x08dadb4c, FFSMConditionECComplete::evaluate 0x08cf12cc
//! SIMPLIFIED(SIMP-049): the streams of the dice are ours, one per figure; the weights and
//! probabilities are the file's.
//!
//! **A machine without conditions** (the weights `next` of `C5FM` v1, or v2 with nothing but empty
//! transitions): the chooser states pick by weight; a standing action that some state carries
//! enters that state, the idle request leaves it for the default state; a clip that plays once (a
//! fidget) hands back to the default state when it is done.
//!
//! **A request nothing in the machine carries** (a model whose `C5FM` has no state for 1910..=1960
//! but whose clips do): the clip of that `ec` loops in place of the machine until the request goes
//! back to idle.
//!
//! No state with an action of attack, death, run, victory or capture is ever entered: the service
//! only passes the three standing actions (see `action_is_standing`), and nothing else writes
//! path option 4. SIMP-010 stays here: there are no clips of moving and attacking.

use asset_format::{FsmCondition, Transition};

use super::AnimRng;
use super::data::ClipTable;
use super::graph::{GraphProgram, MachineDef, PATH_ACTION, PathOptions};

/// Steps of the machine a second (WA-D6).
// SIMPLIFIED(SIMP-exe-1270): the original updates a machine once per drawn frame with the frame's dt and
// only for a unit inside the view rectangle (+64), so the machine of a unit off screen is frozen
// (FUN_08936490 from cvUnitVisSystem::DrawUnits 0x08934358); ours ticks every machine at 30 Hz.
pub const TICK_HZ: f32 = 30.0;
/// Most ticks run in one call to catch up with the clock; a longer gap (a hidden tab, a frame of
/// over 400 ms) jumps. Every frame above 30 ms runs more than one tick per figure, so a long
/// frame must not make the next one longer still.
pub const MAX_CATCH_UP: u64 = 12;
/// States passed through in one tick (choosers and empty states).
const MAX_CHAIN: usize = 8;

/// The seconds of a tick.
pub fn tick_time(tick: u64) -> f32 {
    tick as f32 / TICK_HZ
}

/// The tick a time falls in.
pub fn tick_of(time_s: f32) -> u64 {
    if time_s.is_finite() && time_s > 0.0 {
        (f64::from(time_s) * f64::from(TICK_HZ)).floor() as u64
    } else {
        0
    }
}

/// How a `method` of the file compares the left side (the path option, the time in the state) with
/// the value: `FCompareValues::Method`, 1 equal, 2 less, 3 greater, 4 less or equal, 5 greater or
/// equal, 6 not equal; any other code never holds.
// re: FFSMConditionCheckPathOption::evaluate 0x08dae582, FFSMConditionCheckStateTime::evaluate
// 0x08dae3ac, FCompareValues.h:6-11 (RE-B18 item 2 F6)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compare {
    Eq,
    Lt,
    Gt,
    Le,
    Ge,
    Ne,
}

impl Compare {
    pub fn from_method(method: u8) -> Option<Self> {
        Some(match method {
            1 => Self::Eq,
            2 => Self::Lt,
            3 => Self::Gt,
            4 => Self::Le,
            5 => Self::Ge,
            6 => Self::Ne,
            _ => return None,
        })
    }

    fn holds<T: PartialOrd>(self, a: T, b: T) -> bool {
        match self {
            Self::Eq => a == b,
            Self::Lt => a < b,
            Self::Gt => a > b,
            Self::Le => a <= b,
            Self::Ge => a >= b,
            Self::Ne => a != b,
        }
    }
}

/// `left <method> value`; false for a code that is no method.
fn compare<T: PartialOrd>(method: u8, left: T, value: T) -> bool {
    Compare::from_method(method).is_some_and(|m| m.holds(left, value))
}

/// A clip the machine has chosen to play.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlaySpec {
    pub clip: u16,
    pub ec: u32,
    pub duration: f32,
    pub looping: bool,
    /// When it began, in seconds of the machine clock (before the tick for a phase).
    pub start_s: f32,
    /// True when the clip crossfades from the one before; false for the first of a figure and
    /// for a hard cut.
    pub fade: bool,
    /// The crossfade: seconds, and the S-curve of the graph (`Curve3`) or a straight line.
    pub blend_s: f32,
    pub s_curve: bool,
}

#[derive(Debug, Clone, Copy)]
struct Cur {
    /// Index in the program of the state; `usize::MAX` for a clip played in place of the machine.
    idx: usize,
    spec: PlaySpec,
}

/// The automaton of one figure.
#[derive(Debug, Clone)]
pub struct Fsm {
    state: u16,
    entered_tick: u64,
    tick: u64,
    opts: PathOptions,
    rng: AnimRng,
    cur: Option<Cur>,
    pending: Option<PlaySpec>,
    /// The roll of the tick (drawn at its first `RandomRoll`, so a quiet tick draws nothing), and
    /// the tick it is for.
    roll: Option<(u64, f32)>,
    /// The last states entered, for the tests and the report (state, tick), oldest first.
    log: Vec<(u16, u64)>,
}

impl Fsm {
    /// A machine that starts at `tick`, with the stream of `seed`. [`Self::start`] enters the
    /// default state.
    pub fn new(seed: u64, tick: u64) -> Self {
        // Nothing is asked of a new figure: idle.
        let mut opts = PathOptions::default();
        opts.set(PATH_ACTION, 1000);
        Self {
            state: 0,
            entered_tick: tick,
            tick,
            opts,
            rng: AnimRng::new(seed),
            cur: None,
            pending: None,
            roll: None,
            log: Vec::new(),
        }
    }

    pub fn tick(&self) -> u64 {
        self.tick
    }

    pub fn state(&self) -> u16 {
        self.state
    }

    pub fn options(&self) -> &PathOptions {
        &self.opts
    }

    /// The states entered so far, with their ticks (capped; for tests).
    pub fn log(&self) -> &[(u16, u64)] {
        &self.log
    }

    /// The `ec` of the clip it plays now.
    pub fn playing_ec(&self) -> Option<u32> {
        self.cur.map(|c| c.spec.ec)
    }

    /// Takes the clip the last step chose, if it chose one.
    pub fn take_change(&mut self) -> Option<PlaySpec> {
        self.pending.take()
    }

    /// Jumps the clock to `tick` without running the ticks in between (a long gap, a clock that
    /// went back).
    pub fn jump_to(&mut self, tick: u64) {
        self.tick = tick;
        if self.entered_tick > tick {
            self.entered_tick = tick;
        }
    }

    /// Enters the default state and goes through the choosers; the first clip begins `phase_u` of
    /// its length ago (WA-D11).
    pub fn start(&mut self, def: &MachineDef, clips: &ClipTable, phase_u: f32) {
        let t = tick_time(self.tick);
        self.enter(def, clips, def.default, t, Some(phase_u));
        self.resolve_choosers(def, clips, t);
    }

    /// Writes the requested action (path option 4). The machine reacts on its next tick.
    pub fn request(&mut self, action: u32) {
        self.opts.set(PATH_ACTION, action as i32);
    }

    /// The requested action.
    pub fn requested(&self) -> u32 {
        self.opts.get(PATH_ACTION).max(0) as u32
    }

    // ---- one tick ----

    /// Runs one tick (the one after `self.tick`).
    pub fn step(&mut self, def: &MachineDef, clips: &ClipTable) {
        self.tick += 1;
        let t = tick_time(self.tick);
        if def.states.is_empty() {
            return;
        }
        if self.direct_request(def, clips, t) {
            return;
        }
        if def.is_legacy() {
            self.legacy_rules(def, clips, t);
        } else {
            self.fire_transitions(def, clips, t);
        }
        self.resolve_choosers(def, clips, t);
        self.advance_clip(def, clips, t);
    }

    fn state_def<'a>(&self, def: &'a MachineDef) -> Option<&'a super::graph::StateDef> {
        def.states.get(usize::from(self.state))
    }

    /// The request that no state carries: its clip plays in place of the machine. True while it
    /// does.
    fn direct_request(&mut self, def: &MachineDef, clips: &ClipTable, t: f32) -> bool {
        let action = self.requested();
        let direct = action != 1000
            && def.state_for_action(action).is_none()
            && clips.by_ec(action).is_some();
        let in_direct = self.cur.is_some_and(|c| c.idx == usize::MAX);
        if direct {
            if !in_direct {
                let c = clips.by_ec(action).expect("checked");
                let spec = PlaySpec {
                    clip: c.index,
                    ec: c.ec,
                    duration: c.duration,
                    looping: true,
                    start_s: t,
                    fade: self.cur.is_some(),
                    blend_s: super::graph::CROSSFADE_S,
                    s_curve: true,
                };
                self.cur = Some(Cur {
                    idx: usize::MAX,
                    spec,
                });
                self.pending = Some(spec);
            }
            return true;
        }
        if in_direct {
            // Back to the machine: the default state again.
            self.cur = None;
            self.enter(def, clips, def.default, t, None);
            self.resolve_choosers(def, clips, t);
        }
        false
    }

    /// Rules of a machine with no conditions at all (see the module text).
    fn legacy_rules(&mut self, def: &MachineDef, clips: &ClipTable, t: f32) {
        let action = self.requested();
        let Some(cur_state) = self.state_def(def) else {
            return;
        };
        let carried = cur_state.action;
        if action != 1000 {
            if carried != action
                && let Some((i, _)) = def.state_for_action(action)
            {
                self.enter(def, clips, i as u16, t, None);
            }
        } else if carried != 1000 && carried != 0 {
            self.enter(def, clips, def.default, t, None);
        }
    }

    fn fire_transitions(&mut self, def: &MachineDef, clips: &ClipTable, t: f32) {
        for _ in 0..MAX_CHAIN {
            let Some(st) = self.state_def(def) else {
                return;
            };
            let mut hit = None;
            for tr in &st.transitions {
                if let Some(events) = self.eval_transition(def, tr, t) {
                    hit = Some((tr.to, events));
                    break;
                }
            }
            let Some((to, events)) = hit else {
                return;
            };
            self.run_events(&events);
            if usize::from(to) >= def.states.len() {
                return;
            }
            self.enter(def, clips, to, t, None);
            // A state with something to play stays for this tick; a pass-through goes on.
            if !self.state_def(def).is_some_and(|s| s.program.is_empty()) {
                return;
            }
        }
    }

    /// The `Event*` conditions of the group that held, or `None` if no group holds.
    fn eval_transition<'a>(
        &mut self,
        def: &MachineDef,
        tr: &'a Transition,
        t: f32,
    ) -> Option<Vec<&'a FsmCondition>> {
        if tr.conditions.is_empty() {
            return Some(Vec::new());
        }
        for group in tr.conditions.split(|c| matches!(c, FsmCondition::Or)) {
            let mut events = Vec::new();
            let mut ok = true;
            for c in group {
                match c {
                    FsmCondition::EventSetPathOption { .. }
                    | FsmCondition::EventRandomPathOption { .. } => events.push(c),
                    FsmCondition::Or => {}
                    check => {
                        if !self.check(def, check, t) {
                            ok = false;
                            break;
                        }
                    }
                }
            }
            if ok {
                return Some(events);
            }
        }
        None
    }

    fn check(&mut self, def: &MachineDef, c: &FsmCondition, t: f32) -> bool {
        match *c {
            FsmCondition::CheckPathOption {
                method,
                path,
                value,
            } => compare(method, self.opts.get(path), value),
            FsmCondition::RandomRoll { value, invert } => {
                let roll = self.roll_of_tick();
                if invert { value <= roll } else { roll < value }
            }
            // Events and the marker are handled by the caller.
            ref c => self.check_pure(def, c, t).unwrap_or(true),
        }
    }

    /// The number rolled for this tick: the same for every `RandomRoll` of it.
    fn roll_of_tick(&mut self) -> f32 {
        match self.roll {
            Some((k, r)) if k == self.tick => r,
            _ => {
                let r = self.rng.unit();
                self.roll = Some((self.tick, r));
                r
            }
        }
    }

    /// `EcComplete`: the time in the state is past the length of the model's clip of the
    /// condition's `ec` (0 without one: the first tick after the entry); the file's `value` is
    /// not read.
    // re: FFSMConditionECComplete::evaluate 0x08cf12cc (stateTime > m_fDuration), the duration set
    // per asset by FGrannyAssetInstance::AddStateMachineInstance 0x08ccd362 (RB19 item 2)
    fn ec_complete(&self, def: &MachineDef, ec: u32, t: f32) -> bool {
        let elapsed = t - tick_time(self.entered_tick);
        elapsed > def.ec_duration(ec)
    }

    fn run_events(&mut self, events: &[&FsmCondition]) {
        for e in events {
            match **e {
                FsmCondition::EventSetPathOption { path, value } => self.opts.set(path, value),
                FsmCondition::EventRandomPathOption {
                    path,
                    value,
                    use_weights,
                    ref options,
                } => {
                    let v = pick_option(&mut self.rng, options, use_weights, value);
                    self.opts.set(path, v);
                }
                _ => {}
            }
        }
    }

    /// A state that plays nothing passes on: its transitions (already tried when it was entered
    /// by one), then the weights `next`.
    fn resolve_choosers(&mut self, def: &MachineDef, clips: &ClipTable, t: f32) {
        for _ in 0..MAX_CHAIN {
            let Some(st) = self.state_def(def) else {
                return;
            };
            if !st.program.is_empty() {
                return;
            }
            let from = self.state;
            let mut moved = false;
            if !def.is_legacy() {
                self.fire_transitions_once(def, clips, t);
                moved = self.state != from;
            }
            if !moved && let Some(to) = def.pick_next(self.state, self.rng.unit()) {
                self.enter(def, clips, to, t, None);
                moved = true;
            }
            if !moved {
                return;
            }
        }
    }

    fn fire_transitions_once(&mut self, def: &MachineDef, clips: &ClipTable, t: f32) {
        let Some(st) = self.state_def(def) else {
            return;
        };
        for tr in &st.transitions {
            if let Some(events) = self.eval_transition(def, tr, t) {
                self.run_events(&events);
                if usize::from(tr.to) < def.states.len() {
                    self.enter(def, clips, tr.to, t, None);
                }
                return;
            }
        }
    }

    /// Enters `to`: starts the first clip of its program. A state without a program leaves the
    /// clip that plays as it is.
    fn enter(&mut self, def: &MachineDef, clips: &ClipTable, to: u16, t: f32, phase: Option<f32>) {
        let prev = self.cur;
        self.state = to;
        self.entered_tick = self.tick;
        // The log is a test aid: kept in tests only (13k machines at 4096 entries are megabytes).
        #[cfg(test)]
        if self.log.len() < 4096 {
            self.log.push((to, self.tick));
        }
        let Some(st) = def.states.get(usize::from(to)) else {
            return;
        };
        let program = &st.program;
        if program.is_empty() {
            return;
        }
        let first = if program.cycle_from.is_some() {
            program.clips.first().map(|_| 0)
        } else {
            self.select_index(program)
        };
        let Some(idx) = first else {
            return;
        };
        self.play(program, clips, def, idx, t, phase, prev, true);
    }

    fn select_index(&self, program: &GraphProgram) -> Option<usize> {
        let want = program.select(&self.opts)?;
        program.clips.iter().position(|c| std::ptr::eq(c, want))
    }

    /// Starts clip `idx` of `program` at `t`. At a state's `entry` the graph's crossfade runs from
    /// the clip before; a change of selection inside the state is a hard cut. A clip that keeps
    /// the phase (`cont`) goes on from where the same clip of the state before was; `phase` starts
    /// the first clip of a figure part-way.
    #[allow(clippy::too_many_arguments)]
    fn play(
        &mut self,
        program: &GraphProgram,
        clips: &ClipTable,
        def: &MachineDef,
        idx: usize,
        t: f32,
        phase: Option<f32>,
        prev: Option<Cur>,
        entry: bool,
    ) {
        let Some(g) = program.clips.get(idx) else {
            return;
        };
        let duration = clips
            .by_index(g.clip)
            .map(|c| c.duration)
            .unwrap_or_default();
        let mut start = t;
        let mut same = false;
        if let Some(u) = phase {
            start = t - u.clamp(0.0, 1.0) * duration;
        } else if g.cont
            && let Some(p) = prev
            && p.spec.clip == g.clip
        {
            // re: FGrannyStateMachineInstance::on_leave 0x08cb763a, SetContinuationOffset
            // 0x08cb769e (the clock of the same animation carries over)
            start = p.spec.start_s;
            same = true;
        }
        let blend_s = g
            .crossfade_ec
            .map_or(g.crossfade_s, |ec| def.ec_duration(ec));
        let spec = PlaySpec {
            clip: g.clip,
            ec: g.ec,
            duration,
            looping: g.looping,
            start_s: start,
            fade: prev.is_some() && entry && blend_s > 0.0 && !same,
            blend_s,
            s_curve: g.s_curve,
        };
        self.cur = Some(Cur { idx, spec });
        self.pending = Some(spec);
    }

    /// The clip of the current state when its selections change (a path option written by an
    /// event) or at its end: the selection's clip, cut in; the next clip of a sequence of
    /// `C5FM` v1; a machine without conditions goes back to its default state after a clip that
    /// plays once.
    fn advance_clip(&mut self, def: &MachineDef, clips: &ClipTable, t: f32) {
        let Some(cur) = self.cur else {
            return;
        };
        if cur.idx == usize::MAX {
            return;
        }
        let Some(st) = self.state_def(def) else {
            return;
        };
        let program = &st.program;
        if program.is_empty() {
            return;
        }
        let done = t - cur.spec.start_s >= cur.spec.duration;
        if let Some(from) = program.cycle_from {
            if done {
                let next = if cur.idx + 1 < program.clips.len() {
                    cur.idx + 1
                } else {
                    usize::from(from)
                };
                let begin = if cur.spec.duration > 0.0 {
                    (cur.spec.start_s + cur.spec.duration).min(t)
                } else {
                    t
                };
                self.play(program, clips, def, next, begin, None, Some(cur), false);
            }
            return;
        }
        // The selections of a graph are read every update (granny::SampleDAG_Selection).
        if let Some(i) = self.select_index(program)
            && i != cur.idx
        {
            self.play(program, clips, def, i, t, None, Some(cur), false);
            return;
        }
        if !cur.spec.looping && done && def.is_legacy() {
            self.enter(def, clips, def.default, t, None);
            self.resolve_choosers(def, clips, t);
        }
    }
}
/// Ticks a machine sleeps at most before it looks again (ten minutes of game time): a time check
/// further away than that is only re-planned, never missed.
pub const MAX_SLEEP: u64 = 30 * 600;

// ---- sleeping (B3 packet F) ----
//
// A tick of the machine does nothing but count when no transition fires, no check rolls the dice,
// no clip ends and nothing was asked: `quiet_at` says that for a tick without changing anything,
// by the same walk as `step`. Between two ticks where one of the time checks of the state turns
// (`CheckStateTime`, `EcComplete`, the end of the clip), every check has the same value, so every
// tick there is as quiet as the first; `next_wake` is the first tick that is not. The service
// skips the quiet ticks (`jump_to`), which is exactly what stepping them would do.
impl Fsm {
    /// The first tick after `self.tick` whose step may do more than count (or `MAX_SLEEP` ticks
    /// on, to look again). Valid until the machine steps or is asked something.
    pub fn next_wake(&self, def: &MachineDef, clips: &ClipTable) -> u64 {
        let k0 = self.tick + 1;
        if !self.quiet_at(def, clips, k0) {
            return k0;
        }
        let mut turns: Vec<u64> = Vec::new();
        self.time_turns(def, k0, &mut turns);
        turns.sort_unstable();
        turns.dedup();
        for k in turns {
            if !self.quiet_at(def, clips, k) {
                return k;
            }
        }
        k0 + MAX_SLEEP
    }

    /// The ticks after `k0` (and within `MAX_SLEEP`) where a time check of the state, or the end
    /// of the clip, changes its value.
    fn time_turns(&self, def: &MachineDef, k0: u64, out: &mut Vec<u64>) {
        if let Some(cur) = self.cur {
            let (start, duration) = (cur.spec.start_s, cur.spec.duration);
            push_turn(out, k0, |k| tick_time(k) - start >= duration);
        }
        if def.is_legacy() {
            return;
        }
        let Some(st) = self.state_def(def) else {
            return;
        };
        for tr in &st.transitions {
            for c in &tr.conditions {
                if matches!(
                    c,
                    FsmCondition::CheckStateTime { .. } | FsmCondition::EcComplete { .. }
                ) {
                    push_turn(out, k0, |k| {
                        self.check_pure(def, c, tick_time(k)).unwrap_or(false)
                    });
                }
            }
        }
    }

    /// `check` without the dice: `None` for a roll.
    fn check_pure(&self, def: &MachineDef, c: &FsmCondition, t: f32) -> Option<bool> {
        Some(match *c {
            FsmCondition::CheckPathOption {
                method,
                path,
                value,
            } => compare(method, self.opts.get(path), value),
            FsmCondition::RandomRoll { .. } => return None,
            FsmCondition::CheckStateTime { method, value } => {
                let elapsed = t - tick_time(self.entered_tick);
                compare(method, elapsed, value)
            }
            FsmCondition::EcComplete { ec, .. } => self.ec_complete(def, ec, t),
            _ => true,
        })
    }

    /// False when no group of the transition holds at `t` and none rolls the dice on the way.
    fn transition_stirs(&self, def: &MachineDef, tr: &Transition, t: f32) -> bool {
        if tr.conditions.is_empty() {
            return true;
        }
        for group in tr.conditions.split(|c| matches!(c, FsmCondition::Or)) {
            let mut ok = true;
            for c in group {
                match c {
                    FsmCondition::EventSetPathOption { .. }
                    | FsmCondition::EventRandomPathOption { .. }
                    | FsmCondition::Or => {}
                    check => match self.check_pure(def, check, t) {
                        None => return true,
                        Some(false) => {
                            ok = false;
                            break;
                        }
                        Some(true) => {}
                    },
                }
            }
            if ok {
                return true;
            }
        }
        false
    }

    /// True when the step of tick `k` (from `self.tick == k - 1`) would only count: the walk of
    /// [`Self::step`], with nothing changed.
    pub fn quiet_at(&self, def: &MachineDef, clips: &ClipTable, k: u64) -> bool {
        let t = tick_time(k);
        if def.states.is_empty() {
            return true;
        }
        // direct_request
        let action = self.requested();
        let direct = action != 1000
            && def.state_for_action(action).is_none()
            && clips.by_ec(action).is_some();
        let in_direct = self.cur.is_some_and(|c| c.idx == usize::MAX);
        if direct {
            return in_direct;
        }
        if in_direct {
            return false;
        }
        let st = self.state_def(def);
        if def.is_legacy() {
            if let Some(st) = st {
                let carried = st.action;
                if action != 1000 {
                    if carried != action && def.state_for_action(action).is_some() {
                        return false;
                    }
                } else if carried != 1000 && carried != 0 {
                    return false;
                }
            }
        } else if let Some(st) = st
            && st
                .transitions
                .iter()
                .any(|tr| self.transition_stirs(def, tr, t))
        {
            return false;
        }
        let Some(st) = st else {
            return true;
        };
        // resolve_choosers: a state that plays nothing rolls for its next one every tick.
        let program = &st.program;
        if program.is_empty() {
            return false;
        }
        // advance_clip
        let Some(cur) = self.cur else {
            return true;
        };
        if cur.idx == usize::MAX {
            return true;
        }
        let done = t - cur.spec.start_s >= cur.spec.duration;
        if program.cycle_from.is_some() {
            return !done;
        }
        if self.select_index(program).is_some_and(|i| i != cur.idx) {
            return false;
        }
        cur.spec.looping || !(done && def.is_legacy())
    }
}

/// Adds the first tick after `k0` (within `MAX_SLEEP`) where `pred` differs from its value at
/// `k0`. Every time check is monotone in the tick (`tick_time` is, and so are the subtraction and
/// the comparison in f32), so it turns at most once and a bisection finds the exact tick.
fn push_turn(out: &mut Vec<u64>, k0: u64, pred: impl Fn(u64) -> bool) {
    let limit = k0 + MAX_SLEEP;
    let p0 = pred(k0);
    if pred(limit) == p0 {
        return;
    }
    // pred(lo) == p0, pred(hi) != p0.
    let (mut lo, mut hi) = (k0, limit);
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if pred(mid) == p0 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    out.push(hi);
}

/// The value `EventRandomPathOption` writes: one draw `u` in [0, 1); without weights `value * u`
/// (the list is not read; our options are whole numbers, so it is the floor), with weights the
/// first entry whose running sum of weights reaches `u` (the weights are not scaled to 1), 0 past
/// the last.
// re: FFSMEventRandomPathOption::fire_event (vf at 0x08dadf98)
pub fn pick_option(
    rng: &mut AnimRng,
    options: &[(i32, f32)],
    use_weights: bool,
    value: i32,
) -> i32 {
    let u = rng.unit();
    if !use_weights {
        return (value as f32 * u).floor() as i32;
    }
    let mut sum = 0.0f32;
    for &(v, w) in options {
        sum += w;
        if u <= sum {
            return v;
        }
    }
    0
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::data::{ModelAnim, fixtures};
    use asset_format::{FsmState, IdleFsm};

    fn machine_of(fsm: &IdleFsm) -> (MachineDef, ModelAnim) {
        let (m, pack) = fixtures::model("warrior", 3);
        let a = ModelAnim::new(&m, Some(pack), Some(fsm)).unwrap();
        (a.machine.clone().unwrap(), a)
    }

    /// Runs `ticks` ticks of a figure; the log is its sequence of states.
    fn run(fsm: &IdleFsm, seed: u64, ticks: u64, action_at: &[(u64, u32)]) -> Fsm {
        let (def, a) = machine_of(fsm);
        let mut f = Fsm::new(seed, 0);
        f.start(&def, &a.clips, 0.0);
        for k in 1..=ticks {
            if let Some(&(_, act)) = action_at.iter().rev().find(|(at, _)| *at < k) {
                f.request(act);
            }
            f.step(&def, &a.clips);
        }
        f
    }

    #[test]
    fn a_tick_is_a_thirtieth_of_a_second() {
        assert_eq!(tick_of(1.0), 30);
        assert_eq!(tick_of(0.0333), 0);
        assert_eq!(tick_of(-1.0), 0);
        assert!((tick_time(45) - 1.5).abs() < 1e-6);
    }

    #[test]
    fn the_methods_are_those_of_fcomparevalues() {
        // "The action asked is not fortify": method 6 on 1500.
        assert!(compare(6, 1000, 1500) && !compare(6, 1500, 1500));
        assert!(compare(1, 3, 3) && !compare(1, 3, 4));
        assert!(compare(2, 3, 4) && !compare(2, 4, 4));
        assert!(compare(3, 0.2, 0.15) && !compare(3, 0.1, 0.15));
        assert!(compare(4, 4, 4) && !compare(4, 5, 4));
        assert!(compare(5, 0.15, 0.15) && !compare(5, 0.1, 0.15));
        assert!(!compare(0, 1, 1) && !compare(7, 1, 1));
    }

    #[test]
    fn a_random_path_option_follows_the_running_sum_of_its_weights() {
        let mut rng = AnimRng::new(0xFEED);
        let opts = [(1, 0.4), (0, 0.6)];
        let n = 10_000;
        let ones = (0..n)
            .filter(|_| pick_option(&mut rng, &opts, true, 0) == 1)
            .count();
        let share = ones as f64 / f64::from(n);
        assert!((share - 0.4).abs() < 0.02, "{share}");
        // Weights that do not reach 1: past them the option is 0.
        let short = [(5, 0.25)];
        let zeros = (0..n)
            .filter(|_| pick_option(&mut rng, &short, true, 9) == 0)
            .count() as f64
            / f64::from(n);
        assert!((zeros - 0.75).abs() < 0.02, "{zeros}");
        // Without weights: value times the draw, the list unread.
        for _ in 0..100 {
            let v = pick_option(&mut rng, &opts, false, 4);
            assert!((0..4).contains(&v));
        }
        assert_eq!(pick_option(&mut rng, &[], false, 1), 0);
    }

    #[test]
    fn every_random_roll_of_a_tick_compares_with_the_same_number() {
        let (def, _) = machine_of(&fixtures::omni_machine_v2());
        let mut f = Fsm::new(11, 0);
        f.tick = 5;
        let a = f.roll_of_tick();
        assert_eq!(f.roll_of_tick(), a);
        let roll = |v: f32, invert: bool| FsmCondition::RandomRoll { value: v, invert };
        // Below and not below one number: exactly one of the pair holds.
        let lo = f.check(&def, &roll(a, false), 0.0);
        let hi = f.check(&def, &roll(a, true), 0.0);
        assert!(!lo && hi);
        f.tick = 6;
        assert_ne!(f.roll_of_tick(), a, "a new tick, a new roll");
    }

    /// RB19 item 2: `ECComplete` compares the time in the state with the length of the model's
    /// clip of the condition's own code (not the clip that plays, no 0.98), strictly; a code the
    /// model lacks holds at once.
    #[test]
    fn ec_complete_holds_past_the_length_of_the_clip_of_its_own_code() {
        let (def, _) = machine_of(&fixtures::omni_machine_v2());
        // The fixture's FidgetA (1040) lasts 1 s, IdleB_to_A (2020) 0.5 s.
        let f = Fsm::new(1, 0);
        let ec = |ec, value| FsmCondition::EcComplete { ec, value };
        assert!(!f.check_pure(&def, &ec(1040, None), 1.0).unwrap());
        assert!(f.check_pure(&def, &ec(1040, None), 1.01).unwrap());
        assert!(
            !f.check_pure(&def, &ec(2020, Some(0.1)), 0.4).unwrap(),
            "value unread"
        );
        assert!(f.check_pure(&def, &ec(2020, None), 0.51).unwrap());
        assert!(
            f.check_pure(&def, &ec(1580, None), 0.01).unwrap(),
            "no clip: at once"
        );
        let mut g = f.clone();
        g.entered_tick = 30;
        assert!(
            !g.check_pure(&def, &ec(1040, None), 1.5).unwrap(),
            "counted from the state entry"
        );
    }
    /// The entry of a graph crossfades over its own `BlendTime` on the S-curve (ENTER_FORTIFY:
    /// 0.15 s); FORTIFY_GRAPH has no blend: the held pose cuts in when the entry is done.
    #[test]
    fn a_state_crossfades_by_the_blend_time_of_its_graph() {
        let (mut m, mut pack) = fixtures::model("warrior", 3);
        for (i, ec) in [1500u32, 1520].into_iter().enumerate() {
            pack.clips.push(fixtures::swing_clip("F", 0.5, 3, 10.0));
            m.clips.push(asset_format::ClipRef {
                name: format!("F{ec}"),
                ec,
                index: 6 + i as u16,
                events: vec![],
                legacy_blob: None,
            });
        }
        let a = ModelAnim::new(&m, Some(pack), Some(&fixtures::omni_machine_v2())).unwrap();
        let def = a.machine.clone().unwrap();
        let mut f = Fsm::new(2, 0);
        f.start(&def, &a.clips, 0.0);
        f.take_change();
        f.request(1500);
        let mut changes = Vec::new();
        for _ in 0..60 {
            f.step(&def, &a.clips);
            if let Some(c) = f.take_change() {
                changes.push(c);
            }
        }
        let entry = changes.iter().find(|c| c.ec == 1500).expect("the entry");
        assert!(entry.fade && entry.s_curve && (entry.blend_s - 0.15).abs() < 1e-6);
        let held = changes
            .iter()
            .find(|c| c.ec == 1520)
            .expect("the held pose");
        assert!(!held.fade && held.looping);
        // The entry lasts its clip: the pose comes 0.5 s (and one tick) after it.
        assert!(held.start_s > entry.start_s + 0.5 && held.start_s < entry.start_s + 0.6);
    }
    #[test]
    fn a_machine_of_conditions_walks_idle_fidget_idle() {
        let fsm = fixtures::omni_machine_v2();
        let f = run(&fsm, 3, 30 * 120, &[]);
        let states: Vec<u16> = f.log().iter().map(|&(s, _)| s).collect();
        // The chooser, then idle or fidget, then back by the conditions, several times.
        assert!(states.len() > 6, "{states:?}");
        assert!(states.contains(&1) && states.contains(&2), "{states:?}");
        // Never fortify without being asked.
        assert!(!states.contains(&3) && !states.contains(&4), "{states:?}");
    }

    #[test]
    fn requests_enter_and_leave_the_standing_states() {
        let fsm = fixtures::omni_machine_v2();
        let f = run(&fsm, 5, 30 * 30, &[(60, 1500)]);
        assert_eq!(f.state(), 3, "fortify");
        let f = run(&fsm, 5, 30 * 30, &[(60, 1500), (300, 1000)]);
        assert_ne!(f.state(), 3, "idle again leaves fortify");
    }

    #[test]
    fn the_sequence_is_the_same_at_any_frame_rate() {
        let fsm = fixtures::omni_machine_v2();
        let (def, a) = machine_of(&fsm);
        let log_at = |fps: f32| {
            let mut f = Fsm::new(11, 0);
            f.start(&def, &a.clips, 0.25);
            let frames = (60.0 * fps) as u32;
            for i in 1..=frames {
                let target = tick_of(i as f32 / fps);
                while f.tick() < target {
                    f.step(&def, &a.clips);
                }
            }
            let upto = tick_of(60.0);
            f.log()
                .iter()
                .copied()
                .filter(|&(_, k)| k <= upto)
                .collect::<Vec<_>>()
        };
        let slow = log_at(30.0);
        let fast = log_at(144.0);
        assert!(slow.len() > 4);
        assert_eq!(slow, fast);
    }

    #[test]
    fn no_state_of_move_or_attack_is_entered_whatever_is_asked() {
        // AC-R4.2: ten minutes of game time, every action thrown at the machine.
        for fsm in [fixtures::omni_machine(), fixtures::omni_machine_v2()] {
            let (def, a) = machine_of(&fsm);
            assert!(def.has_no_forbidden_state());
            let mut rng = AnimRng::new(42);
            let mut f = Fsm::new(7, 0);
            f.start(&def, &a.clips, 0.0);
            for k in 1..=(30 * 600u64) {
                if k % 90 == 0 {
                    let all = [1000, 1500, 1910, 1920, 1950, 1960];
                    f.request(all[(rng.next_u64() % all.len() as u64) as usize]);
                }
                f.step(&def, &a.clips);
                if let Some(ec) = f.playing_ec() {
                    assert!(!super::super::data::EC_NEVER_SAMPLES.contains(&ec), "{ec}");
                }
                let st = &def.states[usize::from(f.state())];
                assert!(!super::super::data::EC_NEVER.contains(&st.action));
            }
        }
    }

    #[test]
    fn a_request_no_state_carries_plays_its_own_clip() {
        let (mut m, mut pack) = fixtures::model("worker", 3);
        pack.clips.push(fixtures::swing_clip("Work", 1.0, 3, 40.0));
        m.clips.push(asset_format::ClipRef {
            name: "Work".into(),
            ec: 1930,
            index: 6,
            events: vec![],
            legacy_blob: None,
        });
        let a = ModelAnim::new(&m, Some(pack), Some(&fixtures::omni_machine())).unwrap();
        let def = a.machine.clone().unwrap();
        let mut f = Fsm::new(1, 0);
        f.start(&def, &a.clips, 0.0);
        f.request(1930);
        f.step(&def, &a.clips);
        assert_eq!(f.playing_ec(), Some(1930));
        f.request(1000);
        f.step(&def, &a.clips);
        assert_ne!(f.playing_ec(), Some(1930));
    }

    /// The mobile SAM raised its launcher over and over (live report 2026-11-03): the entry of
    /// fortify plays once, then the held pose loops; the entry never comes back while fortified.
    #[test]
    fn the_fortify_entry_plays_once_then_the_held_pose_loops() {
        let (mut m, mut pack) = fixtures::model("warrior", 3);
        for (i, ec) in [1500u32, 1520, 1540].into_iter().enumerate() {
            pack.clips.push(fixtures::swing_clip("F", 0.5, 3, 10.0));
            m.clips.push(asset_format::ClipRef {
                name: format!("F{ec}"),
                ec,
                index: 6 + i as u16,
                events: vec![],
                legacy_blob: None,
            });
        }
        let a = ModelAnim::new(&m, Some(pack), Some(&fixtures::omni_machine_v2())).unwrap();
        let def = a.machine.clone().unwrap();
        let mut f = Fsm::new(1, 0);
        f.start(&def, &a.clips, 0.0);
        f.request(1500);
        let mut seen = Vec::new();
        let mut starts = 0;
        // Twenty seconds fortified.
        for _ in 0..600 {
            f.step(&def, &a.clips);
            if let Some(c) = f.take_change()
                && c.ec == 1500
            {
                starts += 1;
            }
            if let Some(ec) = f.playing_ec()
                && seen.last() != Some(&ec)
            {
                seen.push(ec);
            }
        }
        let tail: Vec<u32> = seen.iter().copied().skip_while(|&e| e != 1500).collect();
        assert_eq!(tail, [1500, 1520], "{seen:?}");
        assert_eq!(starts, 1, "the entry starts once");
        let st = &def.states[usize::from(f.state())];
        assert_eq!(st.name, "FORTIFY_LOOP");
        assert!(f.cur.is_some_and(|c| c.spec.looping && c.spec.ec == 1520));
    }

    /// Packet F: a machine that sleeps through its quiet ticks (`next_wake`, `jump_to`) is the
    /// machine that steps every tick: the same clips at the same ticks with the same starts, the
    /// same states, the same dice, over ten minutes of frames of 1 to 6 ticks with requests
    /// between them.
    #[test]
    fn a_sleeping_machine_is_the_machine_that_steps_every_tick() {
        let (mut m, mut pack) = fixtures::model("warrior", 3);
        for (i, ec) in [1500u32, 1520, 1540, 1930].into_iter().enumerate() {
            pack.clips
                .push(fixtures::swing_clip("F", 0.5 + i as f32 * 0.3, 3, 10.0));
            m.clips.push(asset_format::ClipRef {
                name: format!("F{ec}"),
                ec,
                index: 6 + i as u16,
                events: vec![],
                legacy_blob: None,
            });
        }
        let mut slept = 0u64;
        let mut stepped = 0u64;
        for fsm in [fixtures::omni_machine(), fixtures::omni_machine_v2()] {
            let a = ModelAnim::new(&m, Some(pack.clone()), Some(&fsm)).unwrap();
            let def = a.machine.clone().unwrap();
            for seed in 0..6u64 {
                let mut script = AnimRng::new(seed * 7919 + 1);
                let mut every = Fsm::new(seed, 0);
                let mut sleeper = Fsm::new(seed, 0);
                every.start(&def, &a.clips, 0.3);
                sleeper.start(&def, &a.clips, 0.3);
                assert_eq!(every.take_change(), sleeper.take_change());
                let (mut log_a, mut log_b) = (Vec::new(), Vec::new());
                let mut tick = 0u64;
                while tick < 30 * 600 {
                    if script.next_u64().is_multiple_of(40) {
                        let all = [1000, 1000, 1500, 1910, 1930, 1960];
                        let act = all[(script.next_u64() % all.len() as u64) as usize];
                        every.request(act);
                        sleeper.request(act);
                    }
                    tick += 1 + script.next_u64() % 6;
                    while every.tick() < tick {
                        every.step(&def, &a.clips);
                        stepped += 1;
                        if let Some(c) = every.take_change() {
                            log_a.push((every.tick(), c));
                        }
                    }
                    while sleeper.tick() < tick {
                        let w = sleeper.next_wake(&def, &a.clips);
                        assert!(w > sleeper.tick());
                        if w > tick {
                            slept += tick - sleeper.tick();
                            sleeper.jump_to(tick);
                            break;
                        }
                        slept += w - 1 - sleeper.tick();
                        sleeper.jump_to(w - 1);
                        sleeper.step(&def, &a.clips);
                        if let Some(c) = sleeper.take_change() {
                            log_b.push((sleeper.tick(), c));
                        }
                    }
                    assert_eq!(format!("{every:?}"), format!("{sleeper:?}"), "tick {tick}");
                }
                assert!(log_a.len() > 10, "{}", log_a.len());
                assert_eq!(log_a, log_b);
            }
        }
        // The point of it: most ticks are slept through.
        assert!(slept * 2 > stepped, "slept {slept} of {stepped}");
    }

    /// A worker asked nothing stands in its idle family for two minutes: never its sleep (1500..)
    /// nor a work clip (live report 2026-11-03: workers knelt with no task).
    #[test]
    fn a_worker_with_no_task_stays_in_idle() {
        let (mut m, mut pack) = fixtures::model("worker", 3);
        for (i, ec) in [1500u32, 1520, 1580, 1910, 1930].into_iter().enumerate() {
            pack.clips.push(fixtures::swing_clip("W", 1.0, 3, 10.0));
            m.clips.push(asset_format::ClipRef {
                name: format!("W{ec}"),
                ec,
                index: 6 + i as u16,
                events: vec![],
                legacy_blob: None,
            });
        }
        let a = ModelAnim::new(&m, Some(pack), Some(&fixtures::omni_machine_v2())).unwrap();
        let def = a.machine.clone().unwrap();
        for seed in 0..8 {
            let mut f = Fsm::new(seed, 0);
            f.start(&def, &a.clips, 0.0);
            for _ in 0..(30 * 120) {
                f.step(&def, &a.clips);
                let ec = f.playing_ec().unwrap();
                assert!(
                    [1000, 1020, 1040, 2000, 2020, 2040].contains(&ec),
                    "{seed}: {ec}"
                );
            }
        }
    }

    /// The machines of the real build (`CIV5_B2_ASSETS`, a folder of blobs by hash with the file
    /// `manifest`; skipped without it): asked nothing, every unit model plays only its idle family;
    /// asked to fortify, the entry 1500 starts once and the unit ends in the held pose; asked idle
    /// again, it gets up (1580) and stands. Before the fix the `AIM_` graphs of the mechanized
    /// machine (the mobile SAM) looped 1500, and the method codes left the chooser of
    /// `1_omni_state_01` with nothing to play.
    #[test]
    fn every_real_machine_idles_unasked_and_plays_the_fortify_entry_once() {
        use asset_format::{LogicalId, Manifest, ModelBlobV2};
        let Some(dir) = std::env::var_os("CIV5_B2_ASSETS") else {
            eprintln!("every_real_machine_...: CIV5_B2_ASSETS is not set; skipped");
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        let line = std::fs::read_to_string(dir.join("manifest")).expect("manifest");
        let manifest = Manifest::decode(
            &std::fs::read(dir.join(line.lines().next().expect("hash").trim())).expect("blob"),
        )
        .expect("manifest");
        let blob = |id: &LogicalId| -> Option<Vec<u8>> {
            let v = manifest.variants(id).first()?;
            std::fs::read(dir.join(v.hash.to_string())).ok()
        };
        let idle_family = [1000, 1020, 1040, 2000, 2020, 2040];
        let (mut checked, mut fortified) = (0, 0);
        for (id, _) in manifest.iter() {
            if !id.as_str().starts_with("md:") {
                continue;
            }
            let Some(model) = blob(id).and_then(|b| ModelBlobV2::decode(&b).ok()) else {
                continue;
            };
            let Some(fsm) = model.fsm.as_ref().and_then(&blob) else {
                continue;
            };
            let fsm = IdleFsm::decode(&fsm).expect("fsm");
            let pack = model
                .anim_pack
                .as_ref()
                .and_then(&blob)
                .and_then(|b| asset_format::AnimPack::decode(&b).ok());
            let Ok(a) = ModelAnim::new(&model, pack, Some(&fsm)) else {
                continue;
            };
            let Some(def) = a.machine.clone() else {
                continue;
            };
            let idle_ec = a.clips.idle().map(|c| c.ec);
            let has_fortify = a.clips.by_ec(1500).is_some() && a.clips.by_ec(1520).is_some();
            checked += 1;
            // Idle for a minute, fortified for a minute, idle again for half of one.
            let mut f = Fsm::new(3, 0);
            f.start(&def, &a.clips, 0.0);
            let (mut entries, mut seen_up) = (0, false);
            for k in 1..=(30 * 150u64) {
                if k == 30 * 60 {
                    f.request(1500);
                }
                if k == 30 * 120 {
                    f.request(1000);
                }
                f.step(&def, &a.clips);
                let change = f.take_change();
                let ec = f.playing_ec();
                if k < 30 * 60 {
                    assert!(
                        ec.is_some_and(|e| idle_family.contains(&e) || Some(e) == idle_ec),
                        "{}: idle plays {ec:?}",
                        id.as_str()
                    );
                }
                if change.is_some_and(|c| c.ec == 1500) {
                    entries += 1;
                }
                if k == 30 * 120 - 1 && has_fortify {
                    assert_eq!(ec, Some(1520), "{}: the held pose", id.as_str());
                    fortified += 1;
                }
                seen_up |= k > 30 * 120 && ec == Some(1580);
            }
            if has_fortify {
                assert_eq!(entries, 1, "{}: the entry starts once", id.as_str());
                assert!(seen_up, "{}: it gets up", id.as_str());
                assert!(
                    f.playing_ec().is_some_and(|e| idle_family.contains(&e)),
                    "{}: idle again",
                    id.as_str()
                );
            }
        }
        eprintln!("{checked} machines, {fortified} with the fortify family");
        assert!(checked > 100 && fortified > 50, "{checked} {fortified}");
    }

    #[test]
    fn a_state_that_plays_nothing_is_passed_through() {
        let fsm = IdleFsm {
            name: "m".into(),
            default: 0,
            states: vec![
                FsmState {
                    name: "CHOOSER".into(),
                    action: 0,
                    asset: String::new(),
                    next: vec![(1, 1.0)],
                    transitions: vec![],
                },
                FsmState {
                    name: "IDLE".into(),
                    action: 1000,
                    asset: "IDLE_GRAPH.dge".into(),
                    next: vec![],
                    transitions: vec![],
                },
            ],
        };
        let f = run(&fsm, 1, 3, &[]);
        assert_eq!(f.state(), 1);
        assert_eq!(f.playing_ec(), Some(1000));
    }
}
