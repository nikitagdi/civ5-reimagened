//! World soundscapes: the ambient loops of the map (SIMP-exe-1025 .. 1027). A pure mixer, no output and no clock of its
//! own.
//!
//! The exe samples the hexes around the camera once a second (and when the camera moves), gives each one sound id
//! (improvement, then feature, then terrain; the fog loop for a hex never seen), keeps one soundscape instance per id and
//! sets its volume from the distance of the nearest hex that asked for it: full up to the soundscape's `min` distance,
//! linear down to silence at its `max` (`re-b10-answers.txt`, item 2). In its weighted mode the rectangle is cut in four
//! at the camera's hex, each quarter asks for its dominant sound only (improvements do not count), and the camera's own
//! hex asks for its sound and for the river's.
//!
//! [`SoundscapeMixer::mix`] does that for one look at the map and gives the levels; [`SoundscapeMixer::update`] turns the
//! levels into what the output must do: keep a bed at a level, end it, play a random element of a soundscape now and
//! then. A level comes with a place in the stereo field: a forest to the west sounds on the left (re-b15 ITEM 7 P7.2).

use std::collections::HashMap;

use asset_format::{ScapeBindingKind, SoundscapeTable};

/// The loop of a hex nobody has seen (a literal in the exe).
pub const FOG_OF_WAR: &str = "ASSS_AMBIENCE_FOG_OF_WAR";
/// The loop added to a hex a river runs along (a literal in the exe).
pub const RIVER: &str = "ASSS_AMBIENCE_RIVER";

/// `GetSoundIDFromPlot` flags of the exe.
const FLAG_FOG: u8 = 1;
const FLAG_IMPROVEMENT: u8 = 2;
const FLAG_FEATURE: u8 = 4;
const FLAG_TERRAIN: u8 = 8;
/// The quarters of the rectangle count terrain and feature only.
const QUARTER_FLAGS: u8 = FLAG_FEATURE | FLAG_TERRAIN;
const ALL_FLAGS: u8 = 0xff;

/// A level below this is silence (the exe's 0.005 sliver).
const SLIVER: f32 = 0.005;
/// Height of a hex row in hex widths (`ROW_STEP / HEX_WIDTH`).
const ROW: f32 = 0.866_025_4;

/// What one hex gives the mix. Names are the `Type` of the gameplay rows (`TERRAIN_GRASS`, `FEATURE_FOREST`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HexSound {
    /// The hex was never seen (fog of war, black): the fog loop, nothing else.
    pub unexplored: bool,
    /// A river runs along the hex.
    pub river: bool,
    pub terrain: Option<&'static str>,
    pub feature: Option<&'static str>,
    pub improvement: Option<&'static str>,
}

/// Where the mix listens: the camera's hex and the rectangle around it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub cx: i32,
    pub cy: i32,
    /// Half the width and height of the rectangle, in hexes.
    pub half_w: i32,
    pub half_h: i32,
    /// How high the listener is above the hexes, in hexes: it counts in every distance.
    pub lift: f32,
}

/// `fCameraZFactor` and `fMaxCameraZ` (4 tiles of 64) of `AudioParams.xml`: the listener's lift.
const CAMERA_Z_FACTOR: f32 = 0.55;
const MAX_LIFT_HEXES: f32 = 4.0;
const HEX_WIDTH: f32 = 64.0;

impl View {
    /// The view of a camera at hex `(cx, cy)` and `height` above the ground (world units, 260 closest to 1083 farthest).
    ///
    /// SIMPLIFIED(SIMP-exe-1025): the exe's `CalculateGridRect` table (a rectangle per zoom level) is not read; the
    /// rectangle grows with the height about as the camera's footprint does. The listener's lift is the exe's
    /// (`UpdateListenerPosition`, re-b15 ITEM 7 P7.3): `clamp(0.55 * height, 0, 256)` world units, in hexes; the
    /// terrain height under the focus that the exe adds is left out (the hexes and the camera's height both count from
    /// the ground here).
    pub fn at(cx: i32, cy: i32, height: f32) -> View {
        let h = if height.is_finite() { height } else { 400.0 };
        View {
            cx,
            cy,
            half_w: ((h / 51.0).ceil() as i32).clamp(3, 24),
            half_h: ((h / 90.0).ceil() as i32).clamp(2, 16),
            lift: (CAMERA_Z_FACTOR * h / HEX_WIDTH).clamp(0.0, MAX_LIFT_HEXES),
        }
    }
}

/// A soundscape heard at `level` (0..1, the fade of its nearest hex) from `pan` (-1 left .. 1 right).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Heard {
    /// Index of the soundscape in the table.
    pub scape: usize,
    pub level: f32,
    pub pan: f32,
}

/// What the output must do.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Keep the looping bed `script` at `level` of its volume and at `pan` (start it if it is not sounding).
    Bed {
        script: String,
        level: f32,
        pan: f32,
    },
    /// The bed ends.
    EndBed { script: String },
    /// Play `script` once at `level` of its volume and at `pan`.
    Element {
        script: String,
        level: f32,
        pan: f32,
    },
}

#[derive(Debug)]
struct Random {
    scripts: Vec<String>,
    min_gap_ms: u64,
    max_gap_ms: u64,
    /// When it may sound next; `None` while the soundscape is not heard.
    next_ms: Option<u64>,
    /// The script it played last, while that may still sound: its wait starts when the sound ends.
    playing: Option<String>,
}

#[derive(Debug)]
struct Scape {
    id: String,
    min_distance: f32,
    max_distance: f32,
    /// The soundscape's own volume, 0..1.
    volume: f32,
    /// The weight of a hex of this loop in the dominant sound of a quarter.
    ///
    /// SIMPLIFIED(SIMP-exe-1025): the exe weighs a hex by a field of the script whose source in the XML is not settled
    /// (RE open question O3); `fPriority` (1 when absent) is taken.
    priority: f32,
    beds: Vec<String>,
    randoms: Vec<Random>,
    active: bool,
    /// The level heard times the volume, as of the last look: what the random elements play at.
    share: f32,
    /// The stereo position of the last look.
    pan: f32,
}

pub struct SoundscapeMixer {
    scapes: Vec<Scape>,
    terrain: HashMap<String, usize>,
    feature: HashMap<String, usize>,
    improvement: HashMap<String, usize>,
    fog: Option<usize>,
    river: Option<usize>,
    rng: u64,
}

impl SoundscapeMixer {
    pub fn new(table: &SoundscapeTable, seed: u64) -> Self {
        let scapes: Vec<Scape> = table
            .scapes
            .iter()
            .map(|s| Scape {
                id: s.id.clone(),
                min_distance: s.min_distance,
                max_distance: s.max_distance,
                volume: (f32::from(s.min_volume) + f32::from(s.max_volume)) / 200.0,
                priority: if s.priority > 0.0 { s.priority } else { 1.0 },
                beds: s
                    .elements
                    .iter()
                    .filter(|e| e.bed)
                    .filter_map(|e| e.scripts.first().cloned())
                    .collect(),
                randoms: s
                    .elements
                    .iter()
                    .filter(|e| !e.bed && !e.scripts.is_empty())
                    .map(|e| Random {
                        scripts: e.scripts.clone(),
                        min_gap_ms: u64::from(e.min_gap_ms),
                        max_gap_ms: u64::from(e.max_gap_ms.max(e.min_gap_ms)),
                        next_ms: None,
                        playing: None,
                    })
                    .collect(),
                active: false,
                share: 0.0,
                pan: 0.0,
            })
            .collect();
        let index: HashMap<&str, usize> = scapes
            .iter()
            .enumerate()
            .map(|(i, s)| (s.id.as_str(), i))
            .collect();
        let mut terrain = HashMap::new();
        let mut feature = HashMap::new();
        let mut improvement = HashMap::new();
        for b in &table.bindings {
            // A row that names a soundscape the file does not define (`ASSS_AMBIENCE_TUNDRA`) is silent: the exe's lookup
            // of an unknown name is not settled either (RE open question O6).
            let Some(&i) = index.get(b.scape.as_str()) else {
                continue;
            };
            let map = match b.kind {
                ScapeBindingKind::Terrain => &mut terrain,
                ScapeBindingKind::Feature => &mut feature,
                ScapeBindingKind::Improvement => &mut improvement,
            };
            map.insert(b.type_name.clone(), i);
        }
        let fog = index.get(FOG_OF_WAR).copied();
        let river = index.get(RIVER).copied();
        Self {
            scapes,
            terrain,
            feature,
            improvement,
            fog,
            river,
            rng: seed | 1,
        }
    }

    /// Number of soundscapes.
    pub fn len(&self) -> usize {
        self.scapes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.scapes.is_empty()
    }

    /// Id of the soundscape at `index` of [`Heard::scape`].
    pub fn id(&self, index: usize) -> Option<&str> {
        self.scapes.get(index).map(|s| s.id.as_str())
    }

    /// The soundscape a hex asks for under `flags` (`GetSoundIDFromPlot`).
    fn scape_of(&self, hex: &HexSound, flags: u8) -> Option<usize> {
        if hex.unexplored {
            // Never seen: the fog loop where the flags ask for it, else the hex is not revealed and gives nothing.
            return if flags & FLAG_FOG != 0 {
                self.fog
            } else {
                None
            };
        }
        let by = |map: &HashMap<String, usize>, name: Option<&str>| -> Option<usize> {
            map.get(name?).copied()
        };
        if flags & FLAG_IMPROVEMENT != 0
            && let Some(i) = by(&self.improvement, hex.improvement)
        {
            return Some(i);
        }
        if flags & FLAG_FEATURE != 0
            && let Some(i) = by(&self.feature, hex.feature)
        {
            return Some(i);
        }
        if flags & FLAG_TERRAIN != 0 {
            return by(&self.terrain, hex.terrain);
        }
        None
    }

    /// Volume of a soundscape heard from `distance` hexes: whole up to its minimum distance, linear down to silence
    /// at its maximum (the exe cuts at the maximum and drops a sliver; the slope between is the engine's, assumed linear).
    ///
    /// SIMPLIFIED(SIMP-exe-1025): the fade shape inside the engine is not traced (RE open question O7).
    fn fade(&self, scape: usize, distance: f32) -> f32 {
        let s = &self.scapes[scape];
        if distance >= s.max_distance {
            return 0.0;
        }
        if distance <= s.min_distance {
            return 1.0;
        }
        let level = 1.0 - (distance - s.min_distance) / (s.max_distance - s.min_distance);
        if level < SLIVER { 0.0 } else { level }
    }

    /// One look at the map. `plot(x, y)` is the hex at those coordinates (unwrapped; the caller wraps), `None` off the
    /// map. Only the weighted mode of the exe is done: four quarters around the camera's hex, each with its dominant
    /// sound heard from every hex of it that asks for it, the river from every revealed river hex of a quarter that has
    /// a dominant sound (re-b15 ITEM 7 P7.1), and the camera's hex with all its flags only when no quarter's dominant
    /// hexes included it.
    ///
    /// SIMPLIFIED(SIMP-exe-1025): the uniform mode (every hex of the rectangle asks for its own sound) is not done; which
    /// mode the exe uses by default is not traced (RE open question O5). Hexes are offset coordinates, the exe's are
    /// axial; the quarters meet on the camera's row and column as theirs do. The stereo position of a hex is the cosine
    /// of its bearing to the east (the camera looks north); a soundscape's left and right are the loudest of its hexes on
    /// each side (re-b15 P7.2; Miles' own pan law is not traced).
    pub fn mix(
        &self,
        view: &View,
        mut plot: impl FnMut(i32, i32) -> Option<HexSound>,
    ) -> Vec<Heard> {
        let n = self.scapes.len();
        if n == 0 {
            return Vec::new();
        }
        // The loudest level of each soundscape on the left and on the right.
        let mut left = vec![0.0f32; n];
        let mut right = vec![0.0f32; n];
        let (cx, cy) = (view.cx, view.cy);
        let (x0, x1) = (cx - view.half_w, cx + view.half_w);
        let (y0, y1) = (cy - view.half_h, cy + view.half_h);
        let (lx, ly) = hex_world(cx, cy);
        let hear = |i: usize, x: i32, y: i32, left: &mut [f32], right: &mut [f32]| {
            let (wx, wy) = hex_world(x, y);
            let dx = wx - lx;
            let d = (dx.powi(2) + (wy - ly).powi(2) + view.lift.powi(2)).sqrt();
            let level = self.fade(i, d);
            if level <= 0.0 {
                return;
            }
            let pan = if d > f32::EPSILON { dx / d } else { 0.0 };
            left[i] = left[i].max(level * (1.0 - pan).min(1.0));
            right[i] = right[i].max(level * (1.0 + pan).min(1.0));
        };
        // The quarters, as the exe cuts them: (x from, x to, y from, y to), edges in both.
        let quarters = [
            (x0, cx, cy, y1),
            (cx, x1, cy, y1),
            (cx, x1, y0, cy),
            (x0, cx, y0, cy),
        ];
        let mut weight = vec![0.0f32; n];
        let mut centre_hit = false;
        for (qx0, qx1, qy0, qy1) in quarters {
            weight.iter_mut().for_each(|w| *w = 0.0);
            for y in qy0..=qy1 {
                for x in qx0..=qx1 {
                    let Some(hex) = plot(x, y) else { continue };
                    if let Some(i) = self.scape_of(&hex, QUARTER_FLAGS) {
                        weight[i] += self.scapes[i].priority;
                    }
                }
            }
            // The dominant sound: the most weight, the first on a tie, none if no hex asked.
            let mut dominant = None;
            let mut top = 0.0f32;
            for (i, &w) in weight.iter().enumerate() {
                if w > top {
                    top = w;
                    dominant = Some(i);
                }
            }
            let Some(d) = dominant else { continue };
            for y in qy0..=qy1 {
                for x in qx0..=qx1 {
                    let Some(hex) = plot(x, y) else { continue };
                    if self.scape_of(&hex, QUARTER_FLAGS) == Some(d) {
                        hear(d, x, y, &mut left, &mut right);
                        if (x, y) == (cx, cy) {
                            centre_hit = true;
                        }
                    }
                    if hex.river
                        && !hex.unexplored
                        && let Some(r) = self.river
                    {
                        hear(r, x, y, &mut left, &mut right);
                    }
                }
            }
        }
        if !centre_hit && let Some(hex) = plot(cx, cy) {
            if let Some(i) = self.scape_of(&hex, ALL_FLAGS) {
                hear(i, cx, cy, &mut left, &mut right);
            }
            if hex.river
                && !hex.unexplored
                && let Some(r) = self.river
            {
                hear(r, cx, cy, &mut left, &mut right);
            }
        }
        (0..n)
            .filter_map(|scape| {
                let level = left[scape].max(right[scape]);
                (level > 0.0).then(|| Heard {
                    scape,
                    level,
                    pan: ((right[scape] - left[scape]) / level).clamp(-1.0, 1.0),
                })
            })
            .collect()
    }

    fn rand(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        if hi <= lo {
            return lo;
        }
        lo + self.rand() % (hi - lo + 1)
    }

    /// What the output does for what is heard now. Call it with every look ([`Self::mix`]) and about once a second,
    /// for the random elements go by `now_ms`. `sounding` says whether a script the mixer asked for still sounds.
    ///
    /// SIMPLIFIED(SIMP-exe-1026): the exe's engine layer (`FSoundScape`: the fades) is not traced. A bed sounds at its
    /// script's volume times the soundscape's volume times the level; a random element is played with the same share
    /// after a wait drawn from its `iMin/MaxTimeMustNotPlayAgain`; of an element that lists several scripts one is
    /// picked each time.
    pub fn update(
        &mut self,
        now_ms: u64,
        heard: &[Heard],
        sounding: &mut dyn FnMut(&str) -> bool,
    ) -> Vec<Command> {
        let mut out = Vec::new();
        for i in 0..self.scapes.len() {
            let found = heard.iter().find(|h| h.scape == i).copied();
            match found {
                Some(h) => {
                    let share = h.level * self.scapes[i].volume;
                    self.scapes[i].active = true;
                    self.scapes[i].share = share;
                    self.scapes[i].pan = h.pan;
                    for script in self.scapes[i].beds.clone() {
                        out.push(Command::Bed {
                            script,
                            level: share,
                            pan: h.pan,
                        });
                    }
                }
                None if self.scapes[i].active => {
                    self.scapes[i].active = false;
                    self.scapes[i].share = 0.0;
                    for script in self.scapes[i].beds.clone() {
                        out.push(Command::EndBed { script });
                    }
                    for r in &mut self.scapes[i].randoms {
                        r.next_ms = None;
                        r.playing = None;
                    }
                }
                None => {}
            }
        }
        out.extend(self.elements(now_ms, sounding));
        out
    }

    /// The random elements of the soundscapes that are heard, for `now_ms`. The exe's engine counts them every frame
    /// (`FSoundScape::Update`), not once a second like the look at the map, so the caller asks every frame.
    ///
    /// An element waits a random time in its `iMin/MaxTimeMustNotPlayAgain` before its first sound, and again after
    /// every sound counted from the END of that sound: the exe does not count the element's timer down while its sound
    /// plays (re-b15 ITEM 7 P7.4), so the period is the sound's length plus the wait. One script of its list is picked
    /// each time; `sounding` says whether the one played still sounds.
    pub fn elements(
        &mut self,
        now_ms: u64,
        sounding: &mut dyn FnMut(&str) -> bool,
    ) -> Vec<Command> {
        let mut out = Vec::new();
        for i in 0..self.scapes.len() {
            if !self.scapes[i].active {
                continue;
            }
            let (share, pan) = (self.scapes[i].share, self.scapes[i].pan);
            for r in 0..self.scapes[i].randoms.len() {
                let (min, max) = {
                    let e = &self.scapes[i].randoms[r];
                    (e.min_gap_ms, e.max_gap_ms)
                };
                if let Some(s) = self.scapes[i].randoms[r].playing.clone() {
                    if sounding(&s) {
                        continue;
                    }
                    // It ended: the wait starts now.
                    let wait = self.range(min, max);
                    let e = &mut self.scapes[i].randoms[r];
                    e.playing = None;
                    e.next_ms = Some(now_ms + wait);
                    continue;
                }
                match self.scapes[i].randoms[r].next_ms {
                    None => {
                        let wait = self.range(min, max);
                        self.scapes[i].randoms[r].next_ms = Some(now_ms + wait);
                    }
                    Some(t) if now_ms >= t => {
                        let count = self.scapes[i].randoms[r].scripts.len() as u64;
                        let pick = self.range(0, count - 1) as usize;
                        let script = self.scapes[i].randoms[r].scripts[pick].clone();
                        out.push(Command::Element {
                            script: script.clone(),
                            level: share,
                            pan,
                        });
                        let e = &mut self.scapes[i].randoms[r];
                        e.playing = Some(script);
                        e.next_ms = None;
                    }
                    Some(_) => {}
                }
            }
        }
        out
    }
    /// Nothing is heard any more (the game is left): every bed that sounds ends.
    pub fn stop_all(&mut self) -> Vec<Command> {
        self.update(0, &[], &mut |_| false)
    }
}

/// A hex in offset coordinates as a point, in hex widths (odd rows shifted half a hex).
fn hex_world(x: i32, y: i32) -> (f32, f32) {
    (x as f32 + 0.5 * y.rem_euclid(2) as f32, y as f32 * ROW)
}

#[cfg(test)]
mod tests {
    use asset_format::{ScapeBinding, ScapeElement, Soundscape};

    use super::*;

    fn scape(
        id: &str,
        priority: f32,
        bed: &str,
        random: Option<(&[&str], u32, u32)>,
    ) -> Soundscape {
        let mut elements = vec![ScapeElement {
            scripts: vec![bed.into()],
            bed: true,
            min_gap_ms: 0,
            max_gap_ms: 0,
        }];
        if let Some((scripts, min, max)) = random {
            elements.push(ScapeElement {
                scripts: scripts.iter().map(|s| (*s).into()).collect(),
                bed: false,
                min_gap_ms: min,
                max_gap_ms: max,
            });
        }
        Soundscape {
            id: id.into(),
            min_distance: 3.0,
            max_distance: 5.0,
            min_volume: 40,
            max_volume: 40,
            priority,
            elements,
        }
    }

    fn bind(kind: ScapeBindingKind, ty: &str, scape: &str) -> ScapeBinding {
        ScapeBinding {
            kind,
            type_name: ty.into(),
            scape: scape.into(),
        }
    }

    /// Ids in the order of the table (sorted): DESERT 0, FOG 1, FOREST 2, GRASSLANDS 3, RIVER 4.
    fn mixer() -> SoundscapeMixer {
        mixer_seeded(9)
    }

    fn mixer_seeded(seed: u64) -> SoundscapeMixer {
        let t = SoundscapeTable {
            scapes: vec![
                scape("ASSS_AMBIENCE_DESERT", 1.0, "BED_DESERT", None),
                scape("ASSS_AMBIENCE_FOG_OF_WAR", 1.0, "BED_FOG", None),
                scape(
                    "ASSS_AMBIENCE_FOREST",
                    6.0,
                    "BED_FOREST",
                    Some((&["OWL", "BIRD"], 4000, 12000)),
                ),
                scape("ASSS_AMBIENCE_GRASSLANDS", 1.0, "BED_GRASS", None),
                scape("ASSS_AMBIENCE_RIVER", 1.0, "BED_RIVER", None),
            ],
            bindings: vec![
                bind(
                    ScapeBindingKind::Terrain,
                    "TERRAIN_DESERT",
                    "ASSS_AMBIENCE_DESERT",
                ),
                bind(
                    ScapeBindingKind::Terrain,
                    "TERRAIN_GRASS",
                    "ASSS_AMBIENCE_GRASSLANDS",
                ),
                // The dangling name of the tundra rows.
                bind(
                    ScapeBindingKind::Terrain,
                    "TERRAIN_TUNDRA",
                    "ASSS_AMBIENCE_TUNDRA",
                ),
                bind(
                    ScapeBindingKind::Feature,
                    "FEATURE_FOREST",
                    "ASSS_AMBIENCE_FOREST",
                ),
                bind(
                    ScapeBindingKind::Improvement,
                    "IMPROVEMENT_FARM",
                    "ASSS_AMBIENCE_DESERT",
                ),
            ],
        };
        SoundscapeMixer::new(&t, seed)
    }

    fn grass() -> HexSound {
        HexSound {
            terrain: Some("TERRAIN_GRASS"),
            ..HexSound::default()
        }
    }

    const D: usize = 0;
    const FOG: usize = 1;
    const F: usize = 2;
    const G: usize = 3;
    const R: usize = 4;

    fn level(heard: &[Heard], scape: usize) -> Option<f32> {
        heard.iter().find(|h| h.scape == scape).map(|h| h.level)
    }

    #[test]
    fn the_view_grows_with_the_camera_height_and_stays_in_bounds() {
        let near = View::at(0, 0, 260.0);
        let mid = View::at(0, 0, 424.0);
        let far = View::at(0, 0, 1083.0);
        assert!(near.half_w < mid.half_w && mid.half_w < far.half_w);
        assert!(near.half_h < mid.half_h && mid.half_h < far.half_h);
        assert!(near.lift < mid.lift && far.lift <= 4.0);
        // The exe's lift: 0.55 of the height in world units, at most 4 hexes (reached at a height of about 465).
        assert!(
            (near.lift - 0.55 * 260.0 / 64.0).abs() < 1e-5,
            "{}",
            near.lift
        );
        assert_eq!(View::at(0, 0, 470.0).lift, 4.0);
        let crazy = View::at(0, 0, 1.0e9);
        assert_eq!((crazy.half_w, crazy.half_h), (24, 16));
        assert_eq!(
            View::at(0, 0, f32::NAN).half_w,
            View::at(0, 0, 400.0).half_w
        );
    }

    #[test]
    fn improvement_then_feature_then_terrain() {
        let m = mixer();
        let hex = HexSound {
            terrain: Some("TERRAIN_GRASS"),
            feature: Some("FEATURE_FOREST"),
            improvement: Some("IMPROVEMENT_FARM"),
            ..HexSound::default()
        };
        assert_eq!(m.scape_of(&hex, ALL_FLAGS), Some(D));
        // The quarters leave improvements out.
        assert_eq!(m.scape_of(&hex, QUARTER_FLAGS), Some(F));
        let no_improvement = HexSound {
            improvement: None,
            ..hex
        };
        assert_eq!(m.scape_of(&no_improvement, ALL_FLAGS), Some(F));
        // A feature or improvement the table does not bind falls through to the terrain.
        let plain = HexSound {
            feature: Some("FEATURE_ICE"),
            improvement: Some("IMPROVEMENT_ROAD"),
            ..hex
        };
        assert_eq!(m.scape_of(&plain, ALL_FLAGS), Some(G));
    }

    #[test]
    fn a_hex_never_seen_gives_the_fog_loop_to_the_centre_only() {
        let m = mixer();
        let fog = HexSound {
            unexplored: true,
            terrain: Some("TERRAIN_GRASS"),
            ..HexSound::default()
        };
        assert_eq!(m.scape_of(&fog, ALL_FLAGS), Some(FOG));
        assert_eq!(m.scape_of(&fog, QUARTER_FLAGS), None);
        // A map of black hexes: only the camera's hex is heard.
        let heard = m.mix(&View::at(5, 5, 424.0), |_, _| Some(fog));
        assert_eq!(heard.len(), 1);
        assert_eq!(heard[0].scape, FOG);
        assert!(heard[0].level > 0.0 && heard[0].pan == 0.0, "{heard:?}");
    }

    #[test]
    fn a_name_the_file_does_not_define_is_silent() {
        let m = mixer();
        let tundra = HexSound {
            terrain: Some("TERRAIN_TUNDRA"),
            ..HexSound::default()
        };
        assert_eq!(m.scape_of(&tundra, ALL_FLAGS), None);
        assert!(
            m.mix(&View::at(5, 5, 424.0), |_, _| Some(tundra))
                .is_empty()
        );
    }

    #[test]
    fn the_dominant_sound_of_a_quarter_is_the_one_with_the_most_weight() {
        let m = mixer();
        let forest = HexSound {
            feature: Some("FEATURE_FOREST"),
            ..grass()
        };
        // Two forests (2 * 6) beat seven grass hexes (7) in the nine of the north-east quarter; the others are grass.
        let view = View {
            cx: 10,
            cy: 10,
            half_w: 2,
            half_h: 2,
            lift: 0.0,
        };
        let heard = m.mix(&view, |x, y| {
            Some(if (x, y) == (11, 11) || (x, y) == (12, 11) {
                forest
            } else {
                grass()
            })
        });
        // (the nearest forest is under 2 hexes from the camera: heard whole)
        assert_eq!(level(&heard, F), Some(1.0));
        assert!(level(&heard, G).is_some(), "the other quarters are grass");
        // One forest hex (6) against eight grass hexes (8): grass wins, the forest is not heard.
        let one = m.mix(&view, |x, y| {
            Some(if (x, y) == (11, 11) { forest } else { grass() })
        });
        assert_eq!(level(&one, F), None);
    }

    #[test]
    fn the_level_is_the_fade_of_the_nearest_hex() {
        let m = mixer();
        let desert = HexSound {
            terrain: Some("TERRAIN_DESERT"),
            ..HexSound::default()
        };
        let view = View {
            cx: 10,
            cy: 10,
            half_w: 9,
            half_h: 4,
            lift: 0.0,
        };
        // The desert starts 4 hexes east of the camera on its row: halfway between 3 (whole) and 5 (silent).
        let at = |from: i32| m.mix(&view, |x, y| (x >= from && y >= 10).then_some(desert));
        let heard = at(14);
        assert!((level(&heard, D).unwrap() - 0.5).abs() < 1e-4, "{heard:?}");
        // 3 hexes or nearer is whole; 5 or farther is cut hard.
        assert_eq!(level(&at(13), D), Some(1.0));
        assert_eq!(level(&at(15), D), None);
        assert_eq!(level(&at(19), D), None);
    }

    #[test]
    fn the_listener_lift_counts_in_the_distance() {
        let m = mixer();
        let desert = HexSound {
            terrain: Some("TERRAIN_DESERT"),
            ..HexSound::default()
        };
        let view = |lift| View {
            cx: 10,
            cy: 10,
            half_w: 9,
            half_h: 4,
            lift,
        };
        let on_row = |v: &View| m.mix(v, |x, y| (x >= 13 && y == 10).then_some(desert));
        let low = level(&on_row(&view(0.0)), D).unwrap();
        let high = level(&on_row(&view(2.0)), D).unwrap();
        assert_eq!(low, 1.0);
        assert!(high < 1.0 && high > 0.0, "{high}");
    }

    #[test]
    fn a_river_is_heard_from_every_revealed_river_hex_of_a_quarter_but_not_from_the_fog() {
        let m = mixer();
        let river = HexSound {
            river: true,
            ..grass()
        };
        let view = View {
            cx: 10,
            cy: 10,
            half_w: 6,
            half_h: 4,
            lift: 0.0,
        };
        // A river four hexes east of the camera (re-b15 P7.1): heard, half way between 3 and 5, from the right.
        let away = m.mix(&view, |x, y| {
            Some(if (x, y) == (14, 10) { river } else { grass() })
        });
        let r = away.iter().find(|h| h.scape == R).copied().unwrap();
        assert!((r.level - 0.5).abs() < 1e-4 && r.pan > 0.99, "{r:?}");
        let here = m.mix(&view, |x, y| {
            Some(if (x, y) == (10, 10) { river } else { grass() })
        });
        assert_eq!(level(&here, R), Some(1.0));
        assert!(level(&here, G).is_some());
        let blind = HexSound {
            unexplored: true,
            river: true,
            ..grass()
        };
        let fog = m.mix(&view, |_, _| Some(blind));
        assert_eq!(level(&fog, R), None);
    }

    #[test]
    fn the_camera_hex_asks_with_all_its_flags_only_when_no_quarter_heard_it() {
        let m = mixer();
        // A farm (bound to the desert loop) under the camera in a sea of grass: the quarters hear grass from the camera's
        // hex, so its improvement is not asked for.
        let farm = HexSound {
            improvement: Some("IMPROVEMENT_FARM"),
            ..grass()
        };
        let view = View::at(10, 10, 300.0);
        let heard = m.mix(&view, |x, y| {
            Some(if (x, y) == (10, 10) { farm } else { grass() })
        });
        assert_eq!(level(&heard, D), None, "{heard:?}");
        // A forest hex under the camera that no quarter's dominant sound covers: the centre asks for it.
        let forest = HexSound {
            feature: Some("FEATURE_FOREST"),
            improvement: Some("IMPROVEMENT_FARM"),
            ..grass()
        };
        let alone = m.mix(&view, |x, y| {
            Some(if (x, y) == (10, 10) { forest } else { grass() })
        });
        assert!(
            level(&alone, D).is_some(),
            "the centre's improvement: {alone:?}"
        );
    }

    #[test]
    fn a_soundscape_to_the_west_sounds_on_the_left() {
        let m = mixer();
        let desert = HexSound {
            terrain: Some("TERRAIN_DESERT"),
            ..HexSound::default()
        };
        let view = View {
            cx: 10,
            cy: 10,
            half_w: 9,
            half_h: 4,
            lift: 1.0,
        };
        let west = m.mix(&view, |x, _| (x <= 7).then_some(desert));
        let d = west.iter().find(|h| h.scape == D).copied().unwrap();
        assert!(d.pan < -0.5 && d.level > 0.0, "{d:?}");
        // All around: in the middle.
        let around = m.mix(&view, |_, _| Some(desert));
        assert_eq!(around.iter().find(|h| h.scape == D).unwrap().pan, 0.0);
    }
    #[test]
    fn hexes_off_the_map_ask_for_nothing() {
        let m = mixer();
        let view = View {
            lift: 0.0,
            ..View::at(0, 0, 424.0)
        };
        let heard = m.mix(&view, |x, y| (x >= 0 && y >= 0).then(grass));
        assert_eq!(
            heard,
            [Heard {
                scape: G,
                level: 1.0,
                pan: 0.0,
            }]
        );
        assert!(m.mix(&View::at(0, 0, 424.0), |_, _| None).is_empty());
    }

    #[test]
    fn beds_follow_the_level_and_end_when_it_is_gone() {
        let mut m = mixer();
        let cmds = m.update(
            0,
            &[Heard {
                scape: G,
                level: 0.5,
                pan: -0.25,
            }],
            &mut |_| false,
        );
        // 0.5 heard of a soundscape of volume 0.4, a little to the left.
        assert_eq!(
            cmds,
            [Command::Bed {
                script: "BED_GRASS".into(),
                level: 0.2,
                pan: -0.25,
            }]
        );
        let cmds = m.update(
            1000,
            &[Heard {
                scape: G,
                level: 1.0,
                pan: 0.0,
            }],
            &mut |_| false,
        );
        assert_eq!(
            cmds,
            [Command::Bed {
                script: "BED_GRASS".into(),
                level: 0.4,
                pan: 0.0,
            }]
        );
        let cmds = m.update(2000, &[], &mut |_| false);
        assert_eq!(
            cmds,
            [Command::EndBed {
                script: "BED_GRASS".into()
            }]
        );
        // Nothing heard, nothing active: nothing to do.
        assert!(m.update(3000, &[], &mut |_| false).is_empty());
    }

    #[test]
    fn a_random_element_sounds_after_its_wait_and_again() {
        let mut m = mixer();
        let heard = [Heard {
            scape: F,
            level: 1.0,
            pan: 0.0,
        }];
        let mut elements = Vec::new();
        for s in 0..60 {
            for c in m.update(s * 1000, &heard, &mut |_| false) {
                if let Command::Element { script, level, .. } = c {
                    assert!(script == "OWL" || script == "BIRD");
                    assert_eq!(level, 0.4);
                    elements.push(s);
                }
            }
        }
        // The first draws up to 12 s, the next waits 4 to 12 s: several in a minute, never two in a second.
        assert!(elements.len() >= 4, "{elements:?}");
        assert!(
            elements.windows(2).all(|w| w[1] - w[0] >= 4),
            "{elements:?}"
        );
        // Leaving and coming back draws the first wait again, and nothing sounds the moment it is heard.
        m.update(70_000, &[], &mut |_| false);
        let back = m.update(71_000, &heard, &mut |_| false);
        assert!(
            back.iter().all(|c| matches!(c, Command::Bed { .. })),
            "{back:?}"
        );
    }

    #[test]
    fn four_quarters_and_the_river_give_at_most_six_scapes_one_instance_per_id() {
        let m = mixer();
        let forest = HexSound {
            feature: Some("FEATURE_FOREST"),
            ..grass()
        };
        let desert = HexSound {
            terrain: Some("TERRAIN_DESERT"),
            ..HexSound::default()
        };
        let river = HexSound {
            river: true,
            ..grass()
        };
        let view = View {
            cx: 10,
            cy: 10,
            half_w: 3,
            half_h: 3,
            lift: 0.0,
        };
        // The south-west quarter is desert, the south-east forest, the north grass; the camera's hex has a river. The
        // row and column through the camera belong to two quarters each and are grass.
        let heard = m.mix(&view, |x, y| {
            Some(if (x, y) == (10, 10) {
                river
            } else if x < 10 && y > 10 {
                desert
            } else if x > 10 && y > 10 {
                forest
            } else {
                grass()
            })
        });
        let mut ids: Vec<usize> = heard.iter().map(|h| h.scape).collect();
        ids.sort_unstable();
        assert_eq!(ids, [D, F, G, R], "one instance per id: {heard:?}");
        assert!(heard.len() <= 6);
    }

    #[test]
    fn a_random_element_waits_four_to_twelve_seconds_from_the_end_of_its_sound() {
        let heard = [Heard {
            scape: F,
            level: 1.0,
            pan: 0.0,
        }];
        for seed in 1..=20 {
            let mut m = mixer_seeded(seed);
            m.update(0, &heard, &mut |_| false);
            let mut starts: Vec<u64> = Vec::new();
            // Every frame (100 ms) for ten minutes; each sound lasts two seconds (re-b15 P7.4: the wait counts from its
            // end, the timer stands while it plays).
            for t in (100..600_000).step_by(100) {
                let last = starts.last().copied();
                let mut sounding = |_: &str| last.is_some_and(|s| t < s + 2000);
                for c in m.elements(t, &mut sounding) {
                    assert!(matches!(c, Command::Element { .. }));
                    starts.push(t);
                }
            }
            assert!(starts.len() >= 40, "{seed}: {}", starts.len());
            // (the shipped forest: 4000..12000 ms; a frame later at most)
            assert!(
                (4000..=12_100).contains(&starts[0]),
                "{seed}: first at {}",
                starts[0]
            );
            for w in starts.windows(2) {
                assert!(
                    (6000..=14_200).contains(&(w[1] - w[0])),
                    "{seed}: gap {}",
                    w[1] - w[0]
                );
            }
        }
    }
    #[test]
    fn a_soundscape_that_is_not_heard_plays_no_element_and_waits_again_on_return() {
        let mut m = mixer();
        let heard = [Heard {
            scape: F,
            level: 0.5,
            pan: 0.0,
        }];
        m.update(0, &heard, &mut |_| false);
        assert!(
            m.elements(1000, &mut |_| false).is_empty(),
            "the first gap is not over"
        );
        m.update(2000, &[], &mut |_| false);
        assert!(
            m.elements(60_000, &mut |_| false).is_empty(),
            "left: silent"
        );
        m.update(61_000, &heard, &mut |_| false);
        assert!(
            m.elements(62_000, &mut |_| false).is_empty(),
            "back: waits its gap again"
        );
    }

    #[test]
    fn stop_all_ends_every_bed_that_sounds() {
        let mut m = mixer();
        m.update(
            0,
            &[
                Heard {
                    scape: G,
                    level: 1.0,
                    pan: 0.0,
                },
                Heard {
                    scape: F,
                    level: 0.3,
                    pan: 0.0,
                },
            ],
            &mut |_| false,
        );
        let mut ended: Vec<String> = m
            .stop_all()
            .into_iter()
            .filter_map(|c| match c {
                Command::EndBed { script } => Some(script),
                _ => None,
            })
            .collect();
        ended.sort();
        assert_eq!(ended, ["BED_FOREST", "BED_GRASS"]);
        assert!(m.stop_all().is_empty());
    }

    #[test]
    fn an_empty_table_is_silent() {
        let mut m = SoundscapeMixer::new(&SoundscapeTable::default(), 1);
        assert!(m.is_empty());
        assert!(
            m.mix(&View::at(3, 3, 424.0), |_, _| Some(grass()))
                .is_empty()
        );
        assert!(m.update(0, &[], &mut |_| false).is_empty());
    }
}
