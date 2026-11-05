//! The rules of a 2D sound script over a sound output.
//!
//! Time is a parameter (`now_ms`), randomness is a seeded generator, so the behaviour is
//! the same in a test and in a game.

use std::collections::{HashMap, HashSet};

use asset_format::{AudioTable, LinkTable, SoundKind, SoundScript};
use platform::{AudioOutError, AudioOutput, SampleId, VoiceId, VoiceParams};

use crate::button::{ButtonEvent, ButtonFlags, ButtonSoundRules};
use crate::wav;

mod mixer;
mod music;
mod positional;
mod scape;

pub use mixer::{DUCK_FADE_MS, MIN_BUS_GAIN, UNDUCK_FADE_MS};
pub use positional::{HEX_UNITS, Listener};

/// A call made before the first user gesture, or a sound that is still loading, is kept this
/// long; after that the moment has passed and the sound is dropped.
pub const MAX_WAIT_MS: u64 = 2000;
const PLACEHOLDER: &str = "AS2D_PLACEHOLDER_BLANK";
/// Rare scripts that are not in a link table but sound with an event (the "look here" effect of a ruin found).
const PRELOAD_EXTRA: [&str; 1] = ["AS2D_WORLD_RAY_OF_GOD"];
/// `iMaxNumSimultaneousSameSounds` of `AudioParams.xml`: the copies of one script that may sound at once when the script
/// sets no `iDontPlayMoreThan` of its own (re-b15 E3).
pub const MAX_SAME_SOUNDS: usize = 10;

/// Volume groups of the game settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    Sfx,
    Music,
    Speech,
    Ambience,
}

impl Category {
    pub fn of(kind: SoundKind) -> Self {
        match kind {
            SoundKind::Sfx | SoundKind::Test | SoundKind::Other => Self::Sfx,
            SoundKind::Music | SoundKind::MusicSting => Self::Music,
            SoundKind::Speech | SoundKind::AdvisorSpeech => Self::Speech,
            SoundKind::Ambience => Self::Ambience,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Suppressed {
    /// `iMinTimeMustNotPlayAgain` has not passed.
    Gap,
    /// `bDontTriggerDuplicates` and the script is still sounding.
    Duplicate,
    /// `iDontPlayMoreThan` copies are sounding.
    Limit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayOutcome {
    Started(VoiceId),
    /// Waits for `iMinTimeDelay`; starts in [`UiAudio::tick`].
    Delayed,
    /// Waits for the file; starts in [`UiAudio::on_file_loaded`] if it comes in time.
    Pending,
    /// Waits for the first user gesture.
    WaitingForGesture,
    /// The empty placeholder script.
    Silent,
    UnknownScript,
    /// The script names a sound with no file.
    NoFile,
    Muted,
    /// `iPercentChanceOfPlaying` said no.
    Chance,
    /// A 3D sound beyond its cutoff distance from the listener (never started, as in the exe).
    OutOfRange,
    Suppressed(Suppressed),
}

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error(transparent)]
    Wav(#[from] wav::WavError),
    #[error(transparent)]
    Output(#[from] AudioOutError),
    #[error("{0} is not a sound file this engine asked for")]
    UnknownFile(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileState {
    Wanted { since_ms: u64 },
    Loaded,
    Failed,
}

#[derive(Debug)]
struct FileEntry {
    sample: SampleId,
    state: FileState,
}

#[derive(Debug, Default)]
struct ScriptState {
    next_allowed_ms: u64,
    voices: Vec<VoiceId>,
}

#[derive(Debug, Clone)]
struct Queued {
    script: String,
    file: String,
    gain: f32,
    looped: bool,
    /// Start time for a delayed play; the request time for a pending one.
    at_ms: u64,
    /// The share of the script's volume the call asked for (a world ambience heard from afar).
    scale: f32,
    /// The call is a world ambience bed (set_bed): its voice is kept to change its gain.
    bed: bool,
    /// The playback rate (the pitch drawn for this play) and the stereo position.
    params: VoiceParams,
}

/// What a call asks beyond the script: a share of its volume, a bed, a stereo position, a wait before it starts.
#[derive(Debug, Clone, Copy)]
struct Call {
    scale: f32,
    bed: bool,
    pan: f32,
    delay_ms: u64,
}

impl Call {
    const PLAIN: Call = Call {
        scale: 1.0,
        bed: false,
        pan: 0.0,
        delay_ms: 0,
    };
}

pub struct UiAudio<O: AudioOutput> {
    out: O,
    scripts: HashMap<String, SoundScript>,
    table: AudioTable,
    files: HashMap<String, FileEntry>,
    next_sample: u32,
    state: HashMap<String, ScriptState>,
    delayed: Vec<Queued>,
    pending: Vec<Queued>,
    waiting: Vec<(String, u64)>,
    /// Files a call waits for now (fetched first).
    wanted: Vec<String>,
    /// Files fetched ahead (nothing waits for them yet; fetched after the urgent ones, a few at a time).
    background: Vec<String>,
    /// Files already named in `wanted` (a file is handed out as urgent once).
    urgent: HashSet<String>,
    /// Failed fetches per file (a file is asked for again after a failure, three times at most).
    failures: HashMap<String, u8>,
    category_gain: HashMap<Category, f32>,
    master: f32,
    muted: bool,
    rng: u64,
    unknown_logged: HashSet<String>,
    rules: ButtonSoundRules,
    /// The looping beds of the world soundscapes that sound, by script.
    beds: HashMap<String, scape::Bed>,
    /// `iPitchChangeDown`, `iPitchChangeUp` of the scripts that have a range (semitones), by script.
    pitch: HashMap<String, (i8, i8)>,
    /// The variations of each sound still to play in this round (re-b15 E6: each once per bag, then a new shuffle).
    bags: HashMap<String, Vec<usize>>,
    /// `fMinDistance`, `fCutoffDistance` (tiles) of the 3D scripts, by script.
    positional: HashMap<String, (f32, f32)>,
    /// The voices the volume buses follow and the duck of the music.
    mixer: mixer::Mixer,
    music: music::MusicVoices,
}

fn key(name: &str) -> String {
    name.to_ascii_lowercase()
}

impl<O: AudioOutput> UiAudio<O> {
    pub fn new(table: AudioTable, out: O, seed: u64) -> Self {
        let scripts = table
            .scripts
            .iter()
            .map(|s| (key(&s.name), s.clone()))
            .collect();
        Self {
            out,
            scripts,
            table,
            files: HashMap::new(),
            next_sample: 1,
            state: HashMap::new(),
            delayed: Vec::new(),
            pending: Vec::new(),
            waiting: Vec::new(),
            wanted: Vec::new(),
            background: Vec::new(),
            urgent: HashSet::new(),
            failures: HashMap::new(),
            category_gain: HashMap::new(),
            master: 1.0,
            muted: false,
            rng: seed | 1,
            unknown_logged: HashSet::new(),
            rules: ButtonSoundRules::none(),
            beds: HashMap::new(),
            pitch: HashMap::new(),
            bags: HashMap::new(),
            positional: HashMap::new(),
            mixer: mixer::Mixer::default(),
            music: music::MusicVoices::default(),
        }
    }

    /// The pitch range of a script (semitones, `down <= 0 <= up` in the files): each play draws a step in it.
    pub fn set_pitch_range(&mut self, script: &str, down: i8, up: i8) {
        let k = key(script);
        if down == 0 && up == 0 {
            self.pitch.remove(&k);
        } else {
            self.pitch.insert(k, (down.min(up), down.max(up)));
        }
    }

    /// The playback rate of one play of a script: `2^(n/12)` for a step `n` drawn in its pitch range (re-b15 E2: the exe
    /// resamples, `FSoundBase::SetPitchChange`).
    fn draw_rate(&mut self, k: &str) -> f32 {
        let Some(&(down, up)) = self.pitch.get(k) else {
            return 1.0;
        };
        let n = i64::from(down) + self.range(0, (i64::from(up) - i64::from(down)) as u64) as i64;
        2f32.powf(n as f32 / 12.0)
    }

    /// The next variation of a sound from its shuffle bag: every variation once, in a new random order each round.
    fn pick_variation(&mut self, sound: &str, count: usize) -> usize {
        if count <= 1 {
            return 0;
        }
        let mut bag = self.bags.remove(sound).unwrap_or_default();
        if bag.is_empty() || bag.iter().any(|&i| i >= count) {
            bag = (0..count).collect();
            // Fisher-Yates; the last element is the one played first.
            for i in (1..count).rev() {
                let j = self.range(0, i as u64) as usize;
                bag.swap(i, j);
            }
        }
        let pick = bag.pop().unwrap_or(0);
        self.bags.insert(sound.to_owned(), bag);
        pick
    }

    pub fn output(&self) -> &O {
        &self.out
    }

    pub fn output_mut(&mut self) -> &mut O {
        &mut self.out
    }

    pub fn table(&self) -> &AudioTable {
        &self.table
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

    /// Uniform in `lo..=hi`.
    fn range(&mut self, lo: u64, hi: u64) -> u64 {
        if hi <= lo {
            return lo;
        }
        lo + self.rand() % (hi - lo + 1)
    }

    /// The knob of a volume bus (0..=1): the voices of the bus that sound change too.
    pub fn set_category_volume(&mut self, c: Category, v: f32) {
        self.category_gain.insert(c, v.clamp(0.0, 1.0));
        self.rebus(c);
    }

    pub fn category_volume(&self, c: Category) -> f32 {
        self.category_gain.get(&c).copied().unwrap_or(1.0)
    }

    pub fn set_master_volume(&mut self, v: f32) {
        self.master = v.clamp(0.0, 1.0);
        self.out.set_master_gain(self.master);
    }

    pub fn set_muted(&mut self, m: bool) {
        self.muted = m;
    }

    pub fn set_button_rules(&mut self, rules: ButtonSoundRules) {
        self.rules = rules;
    }

    /// Files to fetch: manifest ids from `SoundDef.files`, the urgent ones first. Returned once per request.
    pub fn wanted_files(&mut self) -> Vec<String> {
        let mut all = std::mem::take(&mut self.wanted);
        all.extend(std::mem::take(&mut self.background));
        all
    }

    /// [`Self::wanted_files`] in two lists: `(urgent, background)`. A call waits for the urgent ones (the call is dropped
    /// after `MAX_WAIT_MS`); the background ones are fetched ahead of a call, so they can wait behind the urgent ones.
    pub fn take_wanted(&mut self) -> (Vec<String>, Vec<String>) {
        (
            std::mem::take(&mut self.wanted),
            std::mem::take(&mut self.background),
        )
    }

    fn ask_urgent(&mut self, file: &str) {
        if self.urgent.insert(file.to_owned()) {
            self.wanted.push(file.to_owned());
        }
    }

    /// Asks for the files of a script now, so the first call finds them loaded (nothing plays).
    pub fn preload(&mut self, now_ms: u64, script: &str) {
        let Some(sc) = self.scripts.get(&key(script)).cloned() else {
            return;
        };
        let files: Vec<String> = self
            .table
            .sound(&sc.sound)
            .map(|d| d.files.clone())
            .unwrap_or_default();
        for file in files {
            if !self.files.contains_key(&file) {
                self.sample_for(&file).state = FileState::Wanted { since_ms: now_ms };
                self.background.push(file);
            }
        }
    }

    /// [`Self::preload`] for the scripts of the notification, popup and event tables. Their sounds are rare (a barbarian
    /// camp found, an era begun): a file first asked for at the call arrives after `MAX_WAIT_MS` on a busy loader and the
    /// first sound is lost (live report 2026-10-31: no sound when a barbarian camp appears).
    pub fn preload_links(&mut self, now_ms: u64) {
        let names: Vec<String> = self
            .table
            .links
            .iter()
            .filter(|l| !matches!(l.table, LinkTable::Advisor | LinkTable::MinorCiv))
            .map(|l| l.script.clone())
            .collect();
        for n in names {
            self.preload(now_ms, &n);
        }
        for n in PRELOAD_EXTRA {
            self.preload(now_ms, n);
        }
    }

    /// [`Self::preload`] for the scripts the button rules name.
    pub fn preload_rules(&mut self, now_ms: u64) {
        let names: Vec<String> = self
            .rules
            .default
            .values()
            .chain(self.rules.per_kind.values())
            .cloned()
            .collect();
        for n in names {
            self.preload(now_ms, &n);
        }
    }

    /// The user made a gesture: start the output and play what waited for it.
    pub fn unlock(&mut self, now_ms: u64) {
        self.out.resume();
        let waiting = std::mem::take(&mut self.waiting);
        for (script, at) in waiting {
            if now_ms.saturating_sub(at) <= MAX_WAIT_MS {
                self.play_2d(now_ms, &script);
            }
        }
    }

    /// `Events.AudioPlay2DSound("AS2D_...")`.
    pub fn play_2d(&mut self, now_ms: u64, script: &str) -> PlayOutcome {
        self.play_2d_inner(now_ms, script, Call::PLAIN)
    }

    /// [`Self::play_2d`] that starts `delay_ms` from now (a voice-over a second after its popup).
    pub fn play_2d_after(&mut self, now_ms: u64, script: &str, delay_ms: u64) -> PlayOutcome {
        self.play_2d_inner(
            now_ms,
            script,
            Call {
                delay_ms,
                ..Call::PLAIN
            },
        )
    }

    /// Whether a script sounds, or waits to start (its delay, its file).
    pub fn is_sounding(&self, script: &str) -> bool {
        let k = key(script);
        self.state
            .get(&k)
            .is_some_and(|st| st.voices.iter().any(|v| self.out.is_playing(*v)))
            || self.delayed.iter().any(|q| q.script == k)
            || self.pending.iter().any(|q| q.script == k)
    }

    fn play_2d_inner(&mut self, now_ms: u64, script: &str, call: Call) -> PlayOutcome {
        let Call {
            scale,
            bed,
            pan,
            delay_ms,
        } = call;
        let k = key(script);
        let Some(sc) = self.scripts.get(&k).cloned() else {
            if self.unknown_logged.insert(k) {
                tracing::warn!(script, "unknown 2D sound script");
            }
            return PlayOutcome::UnknownScript;
        };
        if sc.name.eq_ignore_ascii_case(PLACEHOLDER) {
            return PlayOutcome::Silent;
        }
        if self.muted || !self.bus_allows(sc.kind, sc.looping) {
            return PlayOutcome::Muted;
        }
        if !self.out.is_running() {
            self.waiting
                .retain(|(_, at)| now_ms.saturating_sub(*at) <= MAX_WAIT_MS);
            self.waiting.push((script.to_owned(), now_ms));
            return PlayOutcome::WaitingForGesture;
        }
        let files: Vec<String> = self
            .table
            .sound(&sc.sound)
            .map(|d| d.files.clone())
            .unwrap_or_default();
        if files.is_empty() {
            return PlayOutcome::NoFile;
        }
        if sc.chance_percent < 100 && self.range(0, 99) >= u64::from(sc.chance_percent) {
            return PlayOutcome::Chance;
        }
        // Voices that stopped are forgotten before the rules count them.
        let mut st = self.state.remove(&k).unwrap_or_default();
        st.voices.retain(|v| self.out.is_playing(*v));
        let verdict = if now_ms < st.next_allowed_ms {
            Some(Suppressed::Gap)
        } else if sc.dont_trigger_duplicates && !st.voices.is_empty() {
            Some(Suppressed::Duplicate)
        } else if st.voices.len()
            >= if sc.max_concurrent > 0 {
                usize::from(sc.max_concurrent)
            } else {
                MAX_SAME_SOUNDS
            }
        {
            Some(Suppressed::Limit)
        } else {
            None
        };
        if let Some(v) = verdict {
            self.state.insert(k, st);
            return PlayOutcome::Suppressed(v);
        }
        let gap = self.range(u64::from(sc.min_gap_ms), u64::from(sc.max_gap_ms));
        st.next_allowed_ms = now_ms + gap;
        self.state.insert(k.clone(), st);
        let file = files[self.pick_variation(&sc.sound, files.len())].clone();
        let rate = self.draw_rate(&k);
        let volume = self.range(u64::from(sc.min_volume), u64::from(sc.max_volume)) as f32 / 100.0;
        // (without the bus: `start` applies it, so a voice follows its slider)
        let gain = volume * scale;
        let q = Queued {
            script: k,
            file,
            gain,
            looped: sc.looping,
            at_ms: now_ms,
            scale,
            bed,
            params: VoiceParams { rate, pan },
        };
        let delay = self.range(u64::from(sc.min_delay_ms), u64::from(sc.max_delay_ms)) + delay_ms;
        if delay > 0 {
            self.delayed.push(Queued {
                at_ms: now_ms + delay,
                ..q
            });
            return PlayOutcome::Delayed;
        }
        self.start(now_ms, q)
    }

    fn sample_for(&mut self, file: &str) -> &mut FileEntry {
        let next = &mut self.next_sample;
        self.files.entry(file.to_owned()).or_insert_with(|| {
            let sample = SampleId(*next);
            *next += 1;
            FileEntry {
                sample,
                state: FileState::Failed,
            }
        })
    }

    fn start(&mut self, now_ms: u64, q: Queued) -> PlayOutcome {
        let known = self.files.contains_key(&q.file);
        let e = self.sample_for(&q.file);
        let sample = e.sample;
        if !known {
            e.state = FileState::Wanted { since_ms: now_ms };
        }
        let state = self.files[&q.file].state;
        match state {
            // An encoded file (mp3, ogg) is decoded by the browser after its bytes came: until then the sample is not
            // there, and the call waits for it like for the file.
            FileState::Loaded if !self.out.is_loaded(sample) => {
                self.pending.push(Queued { at_ms: now_ms, ..q });
                PlayOutcome::Pending
            }
            FileState::Loaded => {
                match self.out.play_with(
                    sample,
                    q.gain * self.script_bus(&q.script),
                    q.looped,
                    q.params,
                ) {
                    Some(v) => {
                        self.voice_started(now_ms, &q.script, v, q.gain);
                        if q.bed {
                            self.bed_started(&q, v);
                        }
                        self.state.entry(q.script).or_default().voices.push(v);
                        PlayOutcome::Started(v)
                    }
                    None => PlayOutcome::WaitingForGesture,
                }
            }
            FileState::Wanted { .. } => {
                // A call waits for it now: it moves up from the background list.
                self.ask_urgent(&q.file.clone());
                self.pending.push(Queued { at_ms: now_ms, ..q });
                PlayOutcome::Pending
            }
            FileState::Failed => PlayOutcome::NoFile,
        }
    }

    /// The fetch of a file named by [`Self::wanted_files`] failed (network, server busy). The file is asked for again by
    /// the next call that needs it, up to three times in all; after that it counts as missing.
    pub fn on_file_failed(&mut self, file: &str) {
        let Some(e) = self.files.get(file) else {
            return;
        };
        if !matches!(e.state, FileState::Wanted { .. }) {
            return;
        }
        let n = self.failures.entry(file.to_owned()).or_default();
        *n += 1;
        if *n >= 3 {
            if let Some(e) = self.files.get_mut(file) {
                e.state = FileState::Failed;
            }
        } else {
            self.files.remove(file);
        }
        self.urgent.remove(file);
        self.pending.retain(|q| q.file != file);
    }

    /// Bytes of a file named by [`Self::wanted_files`] arrived.
    pub fn on_file_loaded(
        &mut self,
        now_ms: u64,
        file: &str,
        bytes: &[u8],
    ) -> Result<(), AudioError> {
        let Some(e) = self.files.get(file) else {
            return Err(AudioError::UnknownFile(file.to_owned()));
        };
        let sample = e.sample;
        let res = if bytes.starts_with(b"RIFF") {
            wav::decode(bytes)
                .map_err(AudioError::from)
                .and_then(|pcm| self.out.load_pcm(sample, &pcm).map_err(AudioError::from))
        } else {
            self.out
                .load_encoded(sample, bytes)
                .map_err(AudioError::from)
        };
        let st = if res.is_ok() {
            FileState::Loaded
        } else {
            FileState::Failed
        };
        if let Some(e) = self.files.get_mut(file) {
            e.state = st;
        }
        let waiting: Vec<Queued> = {
            let (mine, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending)
                .into_iter()
                .partition(|q| q.file == file);
            self.pending = rest;
            mine
        };
        if res.is_ok() {
            for q in waiting {
                if now_ms.saturating_sub(q.at_ms) <= MAX_WAIT_MS {
                    self.start(now_ms, q);
                }
            }
        }
        res
    }

    /// Starts delayed sounds that are due and drops pending ones that waited too long.
    pub fn tick(&mut self, now_ms: u64) {
        let due: Vec<Queued> = {
            let (due, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.delayed)
                .into_iter()
                .partition(|q| q.at_ms <= now_ms);
            self.delayed = rest;
            due
        };
        for q in due {
            self.start(now_ms, q);
        }
        // Calls that wait for the browser to decode their file start when it is done.
        let (ready, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|q| {
                self.files
                    .get(&q.file)
                    .is_some_and(|e| e.state == FileState::Loaded && self.out.is_loaded(e.sample))
            });
        self.pending = rest;
        for q in ready {
            if now_ms.saturating_sub(q.at_ms) <= MAX_WAIT_MS {
                self.start(now_ms, q);
            }
        }
        self.pending
            .retain(|q| now_ms.saturating_sub(q.at_ms) <= MAX_WAIT_MS);
        self.waiting
            .retain(|(_, at)| now_ms.saturating_sub(*at) <= MAX_WAIT_MS);
        self.mixer_tick(now_ms);
    }

    pub fn stop(&mut self, voice: VoiceId) {
        self.out.stop(voice);
    }

    /// Like [`Self::stop_script`], but what sounds dies away over `secs` instead of cutting off.
    pub fn fade_out_script(&mut self, script: &str, secs: f32) {
        let k = key(script);
        if let Some(st) = self.state.get_mut(&k) {
            for v in std::mem::take(&mut st.voices) {
                self.out.fade_out(v, secs);
            }
        }
        self.delayed.retain(|q| q.script != k);
        self.pending.retain(|q| q.script != k);
        self.waiting.retain(|(s, _)| key(s) != k);
    }

    /// Stops what a script started and forgets what waits to start it (a looping ambience that is left).
    pub fn stop_script(&mut self, script: &str) {
        let k = key(script);
        if let Some(st) = self.state.get_mut(&k) {
            for v in std::mem::take(&mut st.voices) {
                self.out.stop(v);
            }
        }
        self.delayed.retain(|q| q.script != k);
        self.pending.retain(|q| q.script != k);
        self.waiting.retain(|(s, _)| key(s) != k);
    }

    /// Script of a notification type (`NotificationSounds.xml`).
    pub fn notification_script(&self, id: &str) -> Option<&str> {
        self.table.link(LinkTable::Notification, id)
    }

    /// Script of a popup type (`ButtonPopupSounds.xml`).
    pub fn popup_script(&self, id: &str) -> Option<&str> {
        self.table.link(LinkTable::ButtonPopup, id)
    }

    pub fn play_notification(&mut self, now_ms: u64, id: &str) -> PlayOutcome {
        match self.notification_script(id).map(str::to_owned) {
            Some(s) => self.play_2d(now_ms, &s),
            None => PlayOutcome::UnknownScript,
        }
    }

    /// Script of an event (`EventSoundConnections.xml`: `CityCreated`, `UnitCreated`).
    pub fn event_script(&self, id: &str) -> Option<&str> {
        self.table.link(LinkTable::Event, id)
    }

    pub fn play_event(&mut self, now_ms: u64, id: &str) -> PlayOutcome {
        match self.event_script(id).map(str::to_owned) {
            Some(s) => self.play_2d(now_ms, &s),
            None => PlayOutcome::UnknownScript,
        }
    }

    pub fn play_popup(&mut self, now_ms: u64, id: &str) -> PlayOutcome {
        match self.popup_script(id).map(str::to_owned) {
            Some(s) => self.play_2d(now_ms, &s),
            None => PlayOutcome::UnknownScript,
        }
    }

    /// A control was clicked, hovered, ...; plays what the rules say (nothing without rules).
    pub fn on_button(&mut self, now_ms: u64, ev: ButtonEvent, flags: ButtonFlags) -> PlayOutcome {
        match self.rules.script_for(ev, flags).map(str::to_owned) {
            Some(s) => self.play_2d(now_ms, &s),
            None => PlayOutcome::Silent,
        }
    }
}

#[cfg(test)]
mod tests {
    use asset_format::{SoundDef, SoundLink, SoundLoad};
    use platform::{AudioOutError, AudioOutput, NullOutput, SampleId, VoiceId};

    use super::*;
    use crate::wav::tests::pcm16;

    fn script(name: &str, sound: &str) -> SoundScript {
        SoundScript::new(name, sound, SoundKind::Sfx)
    }

    fn def(id: &str, files: &[&str]) -> SoundDef {
        SoundDef {
            id: id.into(),
            filename: id.into(),
            load: SoundLoad::DynamicRes,
            one_variation: false,
            files: files.iter().map(|f| (*f).to_owned()).collect(),
        }
    }

    fn table(mut scripts: Vec<SoundScript>) -> AudioTable {
        scripts.sort_by(|a, b| a.name.cmp(&b.name));
        AudioTable {
            scripts,
            sounds: vec![
                def("SND_A", &["ui:au:a.wav"]),
                def("SND_V", &["ui:au:v0.wav", "ui:au:v1.wav", "ui:au:v2.wav"]),
            ],
            links: vec![
                SoundLink {
                    table: LinkTable::Notification,
                    index: 0,
                    key: "N_POLICY".into(),
                    script: "AS2D_A".into(),
                },
                SoundLink {
                    table: LinkTable::ButtonPopup,
                    index: 0,
                    key: "BP_X".into(),
                    script: "AS2D_A".into(),
                },
                SoundLink {
                    table: LinkTable::Event,
                    index: 0,
                    key: "CityCreated".into(),
                    script: "AS2D_A".into(),
                },
                SoundLink {
                    table: LinkTable::Advisor,
                    index: 0,
                    key: "ADV".into(),
                    script: "AS2D_V".into(),
                },
            ],
        }
    }

    fn engine(scripts: Vec<SoundScript>) -> UiAudio<NullOutput> {
        UiAudio::new(table(scripts), NullOutput::running(), 7)
    }

    fn feed(a: &mut UiAudio<NullOutput>, now: u64, files: &[&str]) {
        for f in files {
            a.on_file_loaded(now, f, &pcm16(8000, &[0; 80])).unwrap();
        }
    }

    #[test]
    fn plays_with_the_volume_of_the_script_and_the_category() {
        let mut s = script("AS2D_A", "SND_A");
        s.min_volume = 45;
        s.max_volume = 45;
        let mut a = engine(vec![s]);
        a.set_category_volume(Category::Sfx, 0.5);
        assert_eq!(a.play_2d(0, "as2d_a"), PlayOutcome::Pending);
        assert_eq!(a.wanted_files(), ["ui:au:a.wav"]);
        assert!(a.wanted_files().is_empty(), "asked once");
        feed(&mut a, 10, &["ui:au:a.wav"]);
        let played = &a.output().played;
        assert_eq!(played.len(), 1);
        assert!((played[0].2 - 0.225).abs() < 1e-6);
        // Loaded now: the next call starts at once.
        assert!(matches!(a.play_2d(5000, "AS2D_A"), PlayOutcome::Started(_)));
    }

    #[test]
    fn stop_script_cancels_what_waits_for_its_file() {
        let mut a = engine(vec![script("AS2D_A", "SND_A")]);
        assert_eq!(a.play_2d(0, "AS2D_A"), PlayOutcome::Pending);
        a.stop_script("as2d_a");
        feed(&mut a, 10, &["ui:au:a.wav"]);
        assert!(a.output().played.is_empty());
    }

    #[test]
    fn preload_asks_once_and_the_first_call_plays_at_once() {
        let mut a = engine(vec![script("AS2D_A", "SND_A")]);
        a.preload(0, "as2d_a");
        a.preload(1, "AS2D_A");
        a.preload(1, "AS2D_NOPE");
        assert_eq!(a.wanted_files(), ["ui:au:a.wav"]);
        assert!(a.output().played.is_empty());
        feed(&mut a, 50, &["ui:au:a.wav"]);
        assert!(a.output().played.is_empty(), "preload plays nothing");
        assert!(matches!(a.play_2d(9000, "AS2D_A"), PlayOutcome::Started(_)));
    }

    #[test]
    fn unknown_placeholder_and_missing_files() {
        let mut a = engine(vec![
            script("AS2D_PLACEHOLDER_BLANK", "SND_NONE"),
            script("AS2D_NOFILE", "SND_NONE"),
        ]);
        assert_eq!(a.play_2d(0, "AS2D_NOPE"), PlayOutcome::UnknownScript);
        assert_eq!(a.play_2d(0, "AS2D_PLACEHOLDER_BLANK"), PlayOutcome::Silent);
        assert_eq!(a.play_2d(0, "AS2D_NOFILE"), PlayOutcome::NoFile);
    }

    #[test]
    fn a_late_file_is_dropped_and_a_timely_one_plays() {
        let mut a = engine(vec![script("AS2D_A", "SND_A")]);
        assert_eq!(a.play_2d(0, "AS2D_A"), PlayOutcome::Pending);
        feed(&mut a, MAX_WAIT_MS + 1, &["ui:au:a.wav"]);
        assert!(a.output().played.is_empty(), "too late");
        assert!(matches!(
            a.play_2d(10_000, "AS2D_A"),
            PlayOutcome::Started(_)
        ));
    }

    #[test]
    fn duplicates_limit_and_gap() {
        let mut dup = script("AS2D_DUP", "SND_A");
        dup.dont_trigger_duplicates = true;
        let mut lim = script("AS2D_LIM", "SND_A");
        lim.max_concurrent = 2;
        let mut gap = script("AS2D_GAP", "SND_A");
        gap.min_gap_ms = 3000;
        gap.max_gap_ms = 3000;
        let mut a = engine(vec![dup, lim, gap]);
        a.play_2d(0, "AS2D_DUP");
        feed(&mut a, 1, &["ui:au:a.wav"]);
        // The file came in time: the first call started at 1 and still sounds.
        assert_eq!(a.output().played.len(), 1);
        assert_eq!(
            a.play_2d(3, "AS2D_DUP"),
            PlayOutcome::Suppressed(Suppressed::Duplicate)
        );
        // The voice ends: allowed again.
        let v = a.output().live[0];
        a.stop(v);
        assert!(matches!(a.play_2d(4, "AS2D_DUP"), PlayOutcome::Started(_)));

        assert!(matches!(a.play_2d(10, "AS2D_LIM"), PlayOutcome::Started(_)));
        assert!(matches!(a.play_2d(11, "AS2D_LIM"), PlayOutcome::Started(_)));
        assert_eq!(
            a.play_2d(12, "AS2D_LIM"),
            PlayOutcome::Suppressed(Suppressed::Limit)
        );

        assert!(matches!(
            a.play_2d(100, "AS2D_GAP"),
            PlayOutcome::Started(_)
        ));
        assert_eq!(
            a.play_2d(2000, "AS2D_GAP"),
            PlayOutcome::Suppressed(Suppressed::Gap)
        );
        assert!(matches!(
            a.play_2d(3100, "AS2D_GAP"),
            PlayOutcome::Started(_)
        ));
    }

    #[test]
    fn delay_starts_in_tick() {
        let mut d = script("AS2D_D", "SND_A");
        d.min_delay_ms = 500;
        d.max_delay_ms = 500;
        let mut a = engine(vec![d]);
        a.play_2d(0, "AS2D_D");
        // Not asked for the file yet? It is asked when the delay is over.
        assert_eq!(a.play_2d(1, "AS2D_D"), PlayOutcome::Delayed);
        a.tick(400);
        assert!(a.output().played.is_empty());
        a.tick(600);
        assert_eq!(a.wanted_files(), ["ui:au:a.wav"]);
        feed(&mut a, 650, &["ui:au:a.wav"]);
        assert_eq!(a.output().played.len(), 2);
    }

    #[test]
    fn chance_zero_never_plays_and_variations_are_all_reached() {
        let mut never = script("AS2D_NEVER", "SND_A");
        never.chance_percent = 0;
        let mut v = script("AS2D_V", "SND_V");
        v.min_volume = 10;
        v.max_volume = 90;
        let mut a = engine(vec![never, v]);
        assert_eq!(a.play_2d(0, "AS2D_NEVER"), PlayOutcome::Chance);
        let mut chosen = HashSet::new();
        for t in 0..60u64 {
            a.play_2d(t * 10, "AS2D_V");
        }
        for f in a.wanted_files() {
            chosen.insert(f.clone());
            feed(&mut a, 600, &[&f]);
        }
        assert_eq!(chosen.len(), 3, "every variation was chosen at least once");
        a.play_2d(5000, "AS2D_V");
        let gains: Vec<f32> = a.output().played.iter().map(|p| p.2).collect();
        assert!(gains.iter().all(|g| (0.1..=0.9).contains(g)));
        assert!(
            gains.iter().any(|g| *g != gains[0]),
            "volume varies in the range"
        );
    }

    #[test]
    fn the_same_seed_gives_the_same_choices() {
        let run = || {
            let mut v = script("AS2D_V", "SND_V");
            v.min_volume = 10;
            v.max_volume = 90;
            let mut a = UiAudio::new(table(vec![v]), NullOutput::running(), 99);
            for t in 0..20u64 {
                a.play_2d(t, "AS2D_V");
            }
            a.wanted_files()
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn nothing_sounds_before_the_gesture_and_late_calls_are_dropped() {
        let mut a = UiAudio::new(
            table(vec![script("AS2D_A", "SND_A")]),
            NullOutput::default(),
            1,
        );
        assert_eq!(a.play_2d(0, "AS2D_A"), PlayOutcome::WaitingForGesture);
        assert_eq!(a.play_2d(100, "AS2D_A"), PlayOutcome::WaitingForGesture);
        a.unlock(2050);
        assert!(a.output().is_running());
        // The first call is older than two seconds, the second is not: one request, pending.
        assert_eq!(a.wanted_files().len(), 1);
        assert!(a.output().played.is_empty());
    }

    #[test]
    fn mute_master_and_categories() {
        let mut a = engine(vec![script("AS2D_A", "SND_A")]);
        a.set_muted(true);
        assert_eq!(a.play_2d(0, "AS2D_A"), PlayOutcome::Muted);
        a.set_muted(false);
        a.set_master_volume(0.25);
        assert_eq!(a.output().master, 0.25);
        a.set_category_volume(Category::Music, 2.0);
        assert_eq!(a.category_volume(Category::Music), 1.0);
        assert_eq!(Category::of(SoundKind::AdvisorSpeech), Category::Speech);
        assert_eq!(Category::of(SoundKind::MusicSting), Category::Music);
    }

    #[test]
    fn preload_links_asks_for_notification_popup_and_event_files_but_not_the_advisor() {
        let mut a = engine(vec![script("AS2D_A", "SND_A"), script("AS2D_V", "SND_V")]);
        a.preload_links(0);
        assert_eq!(a.wanted_files(), ["ui:au:a.wav"]);
        feed(&mut a, 50, &["ui:au:a.wav"]);
        // Loaded long before the first call: it starts at once, however late in the session it comes.
        assert!(matches!(
            a.play_notification(60_000, "N_POLICY"),
            PlayOutcome::Started(_)
        ));
    }

    #[test]
    fn a_call_moves_its_file_up_from_the_background_and_a_failed_fetch_is_asked_again() {
        let mut a = engine(vec![script("AS2D_A", "SND_A")]);
        a.preload(0, "AS2D_A");
        // Asked ahead: nothing waits for it yet.
        let (urgent, ahead) = a.take_wanted();
        assert!(urgent.is_empty());
        assert_eq!(ahead, ["ui:au:a.wav"]);
        // A call comes while it is on the way: the file is urgent now (the loader moves it up), once.
        assert_eq!(a.play_2d(100, "AS2D_A"), PlayOutcome::Pending);
        assert_eq!(a.play_2d(120, "AS2D_A"), PlayOutcome::Pending);
        let (urgent, ahead) = a.take_wanted();
        assert_eq!((urgent, ahead), (vec!["ui:au:a.wav".to_owned()], vec![]));
        // The fetch failed (a busy server): the next call asks for the file again, it does not stay silent.
        a.on_file_failed("ui:au:a.wav");
        assert_eq!(a.play_2d(5000, "AS2D_A"), PlayOutcome::Pending);
        assert_eq!(a.take_wanted().0, ["ui:au:a.wav"]);
        feed(&mut a, 5100, &["ui:au:a.wav"]);
        assert_eq!(
            a.output().played.len(),
            1,
            "the call that came with the new fetch sounded"
        );
        assert!(matches!(a.play_2d(9000, "AS2D_A"), PlayOutcome::Started(_)));
    }

    #[test]
    fn a_file_that_keeps_failing_counts_as_missing() {
        let mut a = engine(vec![script("AS2D_A", "SND_A")]);
        for i in 0..3u64 {
            assert_eq!(a.play_2d(i * 10_000, "AS2D_A"), PlayOutcome::Pending);
            assert_eq!(a.take_wanted().0, ["ui:au:a.wav"], "try {i}");
            a.on_file_failed("ui:au:a.wav");
        }
        assert_eq!(a.play_2d(40_000, "AS2D_A"), PlayOutcome::NoFile);
        assert!(a.take_wanted().0.is_empty());
    }

    /// A sound output whose encoded files (mp3, ogg) are decoded later, as the browser does.
    #[derive(Default)]
    struct LateDecode {
        inner: NullOutput,
        decoded: bool,
    }

    impl AudioOutput for LateDecode {
        fn load_pcm(&mut self, s: SampleId, p: &platform::Pcm) -> Result<(), AudioOutError> {
            self.inner.load_pcm(s, p)
        }
        fn load_encoded(&mut self, _s: SampleId, _b: &[u8]) -> Result<(), AudioOutError> {
            Ok(())
        }
        fn is_loaded(&self, s: SampleId) -> bool {
            self.decoded || self.inner.is_loaded(s)
        }
        fn play(&mut self, s: SampleId, g: f32, l: bool) -> Option<VoiceId> {
            if !self.decoded {
                return None;
            }
            self.inner.running = true;
            self.inner.loaded.push(s);
            self.inner.play(s, g, l)
        }
        fn stop(&mut self, v: VoiceId) {
            self.inner.stop(v);
        }
        fn set_master_gain(&mut self, g: f32) {
            self.inner.set_master_gain(g);
        }
        fn is_running(&self) -> bool {
            true
        }
        fn resume(&mut self) {}
        fn is_playing(&self, v: VoiceId) -> bool {
            self.inner.is_playing(v)
        }
    }

    #[test]
    fn a_call_waits_for_the_browser_to_decode_an_encoded_file() {
        let mut a = UiAudio::new(
            table(vec![script("AS2D_A", "SND_A")]),
            LateDecode::default(),
            3,
        );
        assert_eq!(a.play_2d(0, "AS2D_A"), PlayOutcome::Pending);
        // The bytes are in (state loaded) but not decoded: the call is kept, not lost.
        a.on_file_loaded(100, "ui:au:a.wav", b"not a riff").unwrap();
        a.tick(200);
        assert!(a.output().inner.played.is_empty());
        a.output_mut().decoded = true;
        a.tick(300);
        assert_eq!(a.output().inner.played.len(), 1);
    }

    #[test]
    fn events_play_their_script() {
        let mut a = engine(vec![script("AS2D_A", "SND_A")]);
        assert_eq!(a.event_script("CityCreated"), Some("AS2D_A"));
        assert_eq!(a.play_event(0, "CityCreated"), PlayOutcome::Pending);
        assert_eq!(a.play_event(0, "Nope"), PlayOutcome::UnknownScript);
    }

    #[test]
    fn links_and_buttons() {
        let mut a = engine(vec![script("AS2D_A", "SND_A")]);
        assert_eq!(a.notification_script("N_POLICY"), Some("AS2D_A"));
        assert_eq!(a.play_popup(0, "BP_X"), PlayOutcome::Pending);
        assert_eq!(a.play_notification(0, "N_NONE"), PlayOutcome::UnknownScript);
        let flags = ButtonFlags {
            no_default_sound: false,
            disabled: false,
            kind: crate::button::ControlKind::Button,
        };
        assert_eq!(
            a.on_button(10, ButtonEvent::Click, flags),
            PlayOutcome::Silent,
            "no rules, no sound"
        );
        let mut r = ButtonSoundRules::none();
        r.default.insert(ButtonEvent::Click, "AS2D_A".into());
        a.set_button_rules(r);
        feed(&mut a, 20, &["ui:au:a.wav"]);
        assert!(matches!(
            a.on_button(5000, ButtonEvent::Click, flags),
            PlayOutcome::Started(_)
        ));
    }

    #[test]
    fn unknown_file_and_bad_bytes() {
        let mut a = engine(vec![script("AS2D_A", "SND_A")]);
        assert!(matches!(
            a.on_file_loaded(0, "ui:au:zzz.wav", b"x"),
            Err(AudioError::UnknownFile(_))
        ));
        a.play_2d(0, "AS2D_A");
        assert!(matches!(
            a.on_file_loaded(1, "ui:au:a.wav", b"RIFFxxxxWAVE"),
            Err(AudioError::Wav(_))
        ));
        // A failed file is not asked for again.
        assert_eq!(a.play_2d(9000, "AS2D_A"), PlayOutcome::NoFile);
    }
}
