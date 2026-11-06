//! Sound of the original UI (U-D7): button clicks of the layer and `Events.AudioPlay2DSound` of the Lua, played through
//! `audio::UiAudio` over the platform's sound output (Web Audio in the browser).
//!
//! `UiSound` fetches the sound table (`ui:snd`) when the original UI starts, builds the engine, then each frame: feeds
//! the queued button events and the requested scripts, fetches the sound files the engine asks for (`ui:au:*`, once
//! each) and hands the bytes back. Nothing sounds before the first user gesture (the browser's rule); a call made just
//! before it is replayed by the engine if it is still fresh.

use crate::unit_sound::{UnitCue, UnitSound};
use crate::world_sound::{WorldInput, WorldSound};
use assets::{AssetHandle, AssetLoader, AssetPoll, LogicalId};
use audio::{ButtonEvent, ButtonFlags, ButtonSoundRules, PlayOutcome, UiAudio};
use audio::{Listener, NotificationQueue, ScapeCommand, SoundscapeMixer, UnitSoundBook};
use audio::{MusicCiv, MusicCmd, MusicDirector, MusicEvent, MusicScene, MusicView};
use platform::{AudioOutError, AudioOutput, Pcm, SampleId, VoiceId, VoiceParams};
use std::cell::Cell;
use std::collections::VecDeque;
use std::rc::Rc;

type Engine = UiAudio<Counted>;

/// The output with a count of the voices it started (the played-sounds counter).
struct Counted {
    inner: Box<dyn AudioOutput>,
    played: Rc<Cell<u64>>,
}

impl AudioOutput for Counted {
    fn load_pcm(&mut self, s: SampleId, pcm: &Pcm) -> Result<(), AudioOutError> {
        self.inner.load_pcm(s, pcm)
    }
    fn load_encoded(&mut self, s: SampleId, b: &[u8]) -> Result<(), AudioOutError> {
        self.inner.load_encoded(s, b)
    }
    fn is_loaded(&self, s: SampleId) -> bool {
        self.inner.is_loaded(s)
    }
    fn play(&mut self, s: SampleId, gain: f32, looped: bool) -> Option<VoiceId> {
        self.play_with(s, gain, looped, VoiceParams::default())
    }
    fn play_with(
        &mut self,
        s: SampleId,
        gain: f32,
        looped: bool,
        params: VoiceParams,
    ) -> Option<VoiceId> {
        let v = self.inner.play_with(s, gain, looped, params);
        if v.is_some() {
            self.played.set(self.played.get() + 1);
        }
        v
    }
    fn set_voice_pan(&mut self, v: VoiceId, pan: f32) {
        self.inner.set_voice_pan(v, pan);
    }
    fn stop(&mut self, v: VoiceId) {
        self.inner.stop(v);
    }
    fn fade_out(&mut self, v: VoiceId, secs: f32) {
        self.inner.fade_out(v, secs);
    }
    fn set_master_gain(&mut self, g: f32) {
        self.inner.set_master_gain(g);
    }
    fn is_running(&self) -> bool {
        self.inner.is_running()
    }
    fn resume(&mut self) {
        self.inner.resume();
    }
    fn is_playing(&self, v: VoiceId) -> bool {
        self.inner.is_playing(v)
    }
    fn set_voice_gain(&mut self, v: VoiceId, g: f32) {
        self.inner.set_voice_gain(v, g);
    }
    fn can_stream(&self) -> bool {
        self.inner.can_stream()
    }
    fn play_stream(&mut self, url: &str, gain: f32, looped: bool) -> Option<VoiceId> {
        let v = self.inner.play_stream(url, gain, looped);
        if v.is_some() {
            self.played.set(self.played.get() + 1);
        }
        v
    }
    fn pause(&mut self, v: VoiceId) {
        self.inner.pause(v);
    }
    fn unpause(&mut self, v: VoiceId) -> bool {
        self.inner.unpause(v)
    }
}

/// Sound files fetched ahead of a call are requested while fewer than this many fetches are under way.
const MAX_FETCHES_AHEAD: usize = 3;

/// More button events than this in one frame are dropped (a runaway tree must not flood the output).
const MAX_EVENTS_PER_FRAME: usize = 16;

/// Counters for `__civ5stats.audio.*` and the logs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SoundStats {
    /// Button events taken from the layer, ever.
    pub button_events: u64,
    /// Scripts the Lua asked for (`Events.AudioPlay2DSound`), ever.
    pub lua_scripts: u64,
    /// Sounds that started (a voice on the output): the played-sounds counter.
    pub started: u64,
    /// Sounds that wait for a file, a delay or the first gesture.
    pub waiting: u64,
    /// Calls the rules said no to (silent script, gap, limit, chance, muted).
    pub skipped: u64,
    /// Calls with no script or no file in the table.
    pub missing: u64,
    pub files_loaded: u64,
    pub files_failed: u64,
    /// Events that came before the table did.
    pub dropped: u64,
    /// World ambience beds that sound or wait for their file now, and random sounds of the soundscapes played, ever.
    pub world_beds: u64,
    pub world_sounds: u64,
    /// Sounds of the units started, ever, and the cues of their actions taken.
    pub unit_sounds: u64,
    pub unit_cues: u64,
    pub unlocked: bool,
    pub running: bool,
    pub ready: bool,
}

enum Stage {
    /// The output waits for the table to be requested.
    Idle(Box<dyn AudioOutput>),
    Table(Box<dyn AudioOutput>, AssetHandle),
    Ready(Box<Engine>),
    Off,
}

/// The world soundscape table (`ui:scape`): fetched once the first game frame wants it; a set without it (or a table
/// that does not decode) leaves the map quiet.
enum ScapeStage {
    Idle,
    Loading(AssetHandle),
    Ready(Box<WorldSound>),
    Off,
}

/// The unit sound table (`ui:snd3d`): fetched with the soundscape table; a set without it leaves the units quiet.
enum UnitStage {
    Idle,
    Loading(AssetHandle),
    Ready(Box<UnitSound>),
    Off,
}

/// The figures of the units this far from the camera's focus (world units) are watched for their sounds: past the
/// cutoff of any shipped unit script (6 tiles, 9 for a few) and the listener's lift.
pub const UNIT_HEAR_RADIUS: f32 = 10.0 * audio::HEX_UNITS;

pub struct UiSound {
    stage: Stage,
    scape: ScapeStage,
    units: UnitStage,
    /// The cues of the units' actions, until the next look at the world plays them.
    cues: Vec<UnitCue>,
    /// The notification sounds, one after another (re-b15 ITEM 5 P5.1).
    notifications: NotificationQueue,
    /// The spoken line of the last popup (a new popup's line ends it).
    popup_voice: Option<String>,
    profile: assets::PlatformProfile,
    /// The volume knobs of the options menu in the order of `exe_api::m4_wog::KNOBS` (music, SFX, speech, ambience),
    /// each the gain of its bus; the master gain stays 1.0 (the exe has no master option).
    knobs: [f32; 4],
    /// What music plays (re-b15 ITEM 3), the game as it last read it, and when.
    director: MusicDirector,
    music_view: Option<MusicView>,
    music_view_ms: Option<u64>,
    music_last_ms: Option<u64>,
    /// Reads of the game in a row that found the leader screen down while the music follows a leader.
    leader_down_reads: u8,
    files: Vec<(String, AssetHandle)>,
    /// Files to fetch ahead of a call, not yet requested.
    ahead: VecDeque<String>,
    gesture: bool,
    /// The city ambience script playing (stopped when the city screen closes).
    ambience: Option<String>,
    was_running: bool,
    stats: SoundStats,
    played: Rc<Cell<u64>>,
    /// Sounds logged so far (the first few only).
    logged: u32,
    /// The sounds of the mods (A2), decoded once.
    mod_sounds: audio::ModSounds,
    /// A mod silences the world's soundscape while it owns the screen.
    world_muted: bool,
}

impl UiSound {
    /// `knobs`: the volume knobs (0 to 1) in the order of `exe_api::m4_wog::KNOBS`, as the player saved them.
    pub fn new(out: Box<dyn AudioOutput>, knobs: [f32; 4]) -> UiSound {
        UiSound {
            stage: Stage::Idle(out),
            scape: ScapeStage::Idle,
            units: UnitStage::Idle,
            cues: Vec::new(),
            notifications: NotificationQueue::default(),
            popup_voice: None,
            profile: ui_client::ui_profile(),
            knobs: knobs.map(|k| k.clamp(0.0, 1.0)),
            director: MusicDirector::new(0x003a_51c5_u64),
            music_view: None,
            music_view_ms: None,
            music_last_ms: None,
            leader_down_reads: 0,
            files: Vec::new(),
            ahead: VecDeque::new(),
            gesture: false,
            ambience: None,
            was_running: false,
            stats: SoundStats::default(),
            played: Rc::new(Cell::new(0)),
            logged: 0,
            mod_sounds: audio::ModSounds::new(),
            world_muted: false,
        }
    }

    /// Plays a sound of a mod (`wav` is the file of its pack) at `gain` times the SFX knob and playback `rate`. The
    /// output is the one the UI sounds use, but this does not wait for the UI's sound table: a mod's sound plays
    /// as soon as the output runs. Before that (a browser starts after the first gesture) the sound is dropped.
    pub fn play_mod_sound(&mut self, key: &str, wav: &[u8], gain: f32, rate: f32) {
        let gain = gain * self.knob("SFX");
        let out: &mut dyn AudioOutput = match &mut self.stage {
            Stage::Idle(o) | Stage::Table(o, _) => &mut **o,
            Stage::Ready(a) => a.output_mut(),
            Stage::Off => return,
        };
        if let Err(e) = self.mod_sounds.play(out, key, wav, gain, rate) {
            tracing::warn!(sound = key, "mod sound not played: {e}");
        }
    }

    /// A mod silences (`true`) or gives back the strategic soundscape of the world.
    pub fn set_world_muted(&mut self, muted: bool) {
        self.world_muted = muted;
    }

    /// A volume knob of the options menu moved (`"MUSIC"`, `"SFX"`, `"SPEECH"`, `"AMBIENCE"`, 0..=1): only the sounds
    /// of its bus change (WO-G, SIMP-exe-392; re-b15 ITEM 8 P8.1).
    pub fn set_knob(&mut self, knob: &str, value: f32) {
        let Some(i) = exe_api::m4_wog::KNOBS.iter().position(|k| *k == knob) else {
            return;
        };
        self.knobs[i] = value.clamp(0.0, 1.0);
        if let Stage::Ready(a) = &mut self.stage {
            a.set_knob(knob, self.knobs[i]);
        }
    }

    /// A volume knob now (0..=1); 0 for an unknown name.
    pub fn knob(&self, knob: &str) -> f32 {
        exe_api::m4_wog::KNOBS
            .iter()
            .position(|k| *k == knob)
            .map_or(0.0, |i| self.knobs[i])
    }

    /// The SFX knob now (0..=1): the `volume` counter of the stats.
    pub fn volume(&self) -> f32 {
        self.knob("SFX")
    }

    pub fn stats(&self) -> SoundStats {
        let beds = match &self.stage {
            Stage::Ready(a) => a.bed_count() as u64,
            _ => 0,
        };
        SoundStats {
            started: self.played.get(),
            world_beds: beds,
            ..self.stats
        }
    }

    /// A pointer press or a key press: the browser lets the output start now.
    pub fn gesture(&mut self, now_ms: u64) {
        self.gesture = true;
        self.stats.unlocked = true;
        match &mut self.stage {
            Stage::Idle(out) | Stage::Table(out, _) => out.resume(),
            Stage::Ready(a) => a.unlock(now_ms),
            Stage::Off => {}
        }
    }

    /// The world soundscapes for this frame (item 82): `input` is the map under the camera while a game is on the
    /// screen, `None` when it is not (no game, the loading screen). With no input, or with the volume at zero, every
    /// bed ends. Call it before [`Self::update`], which fetches the files the beds and sounds ask for.
    pub fn world(&mut self, now_ms: u64, loader: &mut AssetLoader, input: Option<&WorldInput<'_>>) {
        let live = input.is_some() && self.knob("AMBIENCE") > 0.0 && !self.world_muted;
        self.scape = match std::mem::replace(&mut self.scape, ScapeStage::Off) {
            ScapeStage::Idle if live && matches!(self.stage, Stage::Ready(_)) => {
                let h = loader.request(
                    LogicalId::new(asset_format::SOUNDSCAPE_TABLE_ID),
                    &self.profile,
                );
                ScapeStage::Loading(h)
            }
            ScapeStage::Loading(h) => match loader.poll(h) {
                AssetPoll::Pending => ScapeStage::Loading(h),
                AssetPoll::Failed(e) => {
                    // (an older set has no such table: the map stays quiet)
                    tracing::info!(
                        "world sounds are off: {}: {e}",
                        asset_format::SOUNDSCAPE_TABLE_ID
                    );
                    ScapeStage::Off
                }
                AssetPoll::Ready(bytes) => match asset_format::SoundscapeTable::decode(&bytes) {
                    Ok(table) => {
                        let mixer = SoundscapeMixer::new(&table, 0x005c_a9e5_u64);
                        tracing::info!(soundscapes = mixer.len(), "world sounds are ready");
                        ScapeStage::Ready(Box::new(WorldSound::new(mixer)))
                    }
                    Err(e) => {
                        tracing::warn!("world sounds are off: the table: {e}");
                        ScapeStage::Off
                    }
                },
            },
            other => other,
        };
        self.units = match std::mem::replace(&mut self.units, UnitStage::Off) {
            UnitStage::Idle if live && matches!(self.stage, Stage::Ready(_)) => {
                UnitStage::Loading(loader.request(
                    LogicalId::new(asset_format::SOUND3D_TABLE_ID),
                    &self.profile,
                ))
            }
            UnitStage::Loading(h) => match loader.poll(h) {
                AssetPoll::Pending => UnitStage::Loading(h),
                AssetPoll::Failed(e) => {
                    // (an older set has no such table: the units stay quiet)
                    tracing::info!(
                        "unit sounds are off: {}: {e}",
                        asset_format::SOUND3D_TABLE_ID
                    );
                    UnitStage::Off
                }
                AssetPoll::Ready(bytes) => {
                    match (asset_format::Sound3DTable::decode(&bytes), &mut self.stage) {
                        (Ok(table), Stage::Ready(a)) => {
                            a.attach_3d(&table);
                            let units = UnitSound::new(UnitSoundBook::new(&table));
                            tracing::info!(
                                scripts = table.scripts.len(),
                                models = units.models(),
                                "unit sounds are ready"
                            );
                            UnitStage::Ready(Box::new(units))
                        }
                        (Err(e), _) => {
                            tracing::warn!("unit sounds are off: the table: {e}");
                            UnitStage::Off
                        }
                        _ => UnitStage::Off,
                    }
                }
            },
            other => other,
        };
        let Stage::Ready(a) = &mut self.stage else {
            return;
        };
        if let UnitStage::Ready(u) = &mut self.units {
            match input {
                Some(i) if live => {
                    let shape = i.snapshot.shape();
                    let terrain = |p: render::glam::Vec2| -> &'static str {
                        let (x, y) = render::hexgrid::nearest_cell(p);
                        shape
                            .normalize(x, y)
                            .and_then(|(x, y)| i.snapshot.cell(x, y))
                            .map(|c| {
                                let h = crate::world_sound::hex_sound(c);
                                audio::terrain_tag(h.terrain, h.feature)
                            })
                            .unwrap_or("DIRT")
                    };
                    for cue in self.cues.drain(..) {
                        u.cue(now_ms, &cue, &terrain);
                    }
                    let plays = u.step(now_ms, i.figures, &terrain);
                    for s in u.take_preloads() {
                        a.preload(now_ms, &s);
                    }
                    // SIMPLIFIED(SIMP-exe-1201): the listener stands on the ground of the hexes (height 0), not on the
                    // terrain under the focus.
                    let listener = Listener::at(i.focus.x, i.focus.y, 0.0, i.height);
                    for p in plays {
                        // SIMPLIFIED(SIMP-exe-1201): the tracked unit loops (one per script and unit, stopped 0.07 s after
                        // their last trigger with a 0.5 s fade) are not played.
                        if a.is_looping(&p.script) {
                            continue;
                        }
                        if matches!(
                            a.play_3d(now_ms, &p.script, p.position, &listener),
                            PlayOutcome::Started(_)
                        ) {
                            self.stats.unit_sounds += 1;
                        }
                    }
                }
                _ => {
                    self.cues.clear();
                    u.clear();
                }
            }
        }
        let ScapeStage::Ready(w) = &mut self.scape else {
            return;
        };
        let cmds = match input {
            Some(i) if live => w.step(now_ms, i, &mut |s| a.is_sounding(s)),
            _ => {
                a.end_all_beds();
                w.stop()
            }
        };
        for c in cmds {
            match c {
                ScapeCommand::Bed { script, level, pan } => {
                    a.set_bed_panned(now_ms, &script, level, pan)
                }
                ScapeCommand::EndBed { script } => a.end_bed(&script),
                ScapeCommand::Element { script, level, pan } => {
                    if matches!(
                        a.play_2d_panned(now_ms, &script, level, pan),
                        PlayOutcome::Started(_)
                    ) {
                        self.stats.world_sounds += 1;
                    }
                }
            }
        }
    }

    /// One frame: `buttons` are the events taken from the layer, `scripts` the sounds the Lua asked for.
    pub fn update(
        &mut self,
        now_ms: u64,
        loader: &mut AssetLoader,
        buttons: Vec<(ButtonEvent, ButtonFlags)>,
        scripts: Vec<String>,
    ) {
        self.stats.button_events += buttons.len() as u64;
        self.stats.lua_scripts += scripts.len() as u64;
        self.stage = match std::mem::replace(&mut self.stage, Stage::Off) {
            Stage::Idle(out) => {
                let h = loader.request(LogicalId::new(asset_format::AUDIO_TABLE_ID), &self.profile);
                Stage::Table(out, h)
            }
            Stage::Table(out, h) => match loader.poll(h) {
                AssetPoll::Pending => Stage::Table(out, h),
                AssetPoll::Failed(e) => {
                    tracing::warn!("ui sounds are off: {}: {e}", asset_format::AUDIO_TABLE_ID);
                    Stage::Off
                }
                AssetPoll::Ready(bytes) => match asset_format::AudioTable::decode(&bytes) {
                    Ok(table) => {
                        let counted = Counted {
                            inner: out,
                            played: Rc::clone(&self.played),
                        };
                        let mut a = UiAudio::new(table, counted, 0x5eed_u64);
                        a.set_master_volume(1.0);
                        for (k, v) in exe_api::m4_wog::KNOBS.iter().zip(self.knobs) {
                            a.set_knob(k, v);
                        }
                        // SIMPLIFIED(SIMP-uiau-1): the provisional click rule (the exe's rules are not read).
                        a.set_button_rules(ButtonSoundRules::provisional());
                        // The files of the click are fetched now, so the first click is heard.
                        a.preload_rules(now_ms);
                        // SIMPLIFIED(SIMP-exe-680): so are those of the notifications, popups and events: they are rare and a file first asked
                        // for at the call comes too late (a barbarian camp found made no sound).
                        a.preload_links(now_ms);
                        if self.gesture {
                            a.unlock(now_ms);
                        }
                        tracing::info!(
                            scripts = a.table().scripts.len(),
                            knobs = ?self.knobs,
                            "ui sounds are ready"
                        );
                        self.stats.ready = true;
                        Stage::Ready(Box::new(a))
                    }
                    Err(e) => {
                        tracing::warn!("ui sounds are off: the table: {e}");
                        Stage::Off
                    }
                },
            },
            ready @ (Stage::Ready(_) | Stage::Off) => ready,
        };
        let Stage::Ready(a) = &mut self.stage else {
            self.stats.dropped += (buttons.len() + scripts.len()) as u64;
            return;
        };
        let running = a.output().is_running();
        if running && !self.was_running && self.gesture {
            // The output started after the press: what waited for it plays now.
            a.unlock(now_ms);
        }
        self.was_running = running;
        self.stats.running = running;
        let mut outcomes = Vec::new();
        for (ev, flags) in buttons.into_iter().take(MAX_EVENTS_PER_FRAME) {
            outcomes.push(("button", a.on_button(now_ms, ev, flags)));
        }
        // Files to fetch ahead are not calls: they are all taken, however many come in one frame; so are the cues of the
        // units (the next look at the world plays them) and the notifications (they wait in their queue).
        let (hints, scripts): (Vec<String>, Vec<String>) = scripts
            .into_iter()
            .partition(|s| s.starts_with(crate::m2::PRELOAD_PREFIX));
        let (cues, scripts): (Vec<String>, Vec<String>) = scripts
            .into_iter()
            .partition(|s| s.starts_with(crate::m2::UNIT_CUE_PREFIX));
        for c in &cues {
            if let Some(cue) = c
                .strip_prefix(crate::m2::UNIT_CUE_PREFIX)
                .and_then(UnitCue::parse)
            {
                self.stats.unit_cues += 1;
                if matches!(self.units, UnitStage::Ready(_)) {
                    self.cues.push(cue);
                }
            }
        }
        let (notes, scripts): (Vec<String>, Vec<String>) = scripts
            .into_iter()
            .partition(|s| s.starts_with(crate::m2::NOTIFICATION_SOUND_PREFIX));
        for n in &notes {
            if let Some(kind) = n.strip_prefix(crate::m2::NOTIFICATION_SOUND_PREFIX) {
                self.notifications.push(kind);
            }
        }
        for h in &hints {
            if let Some(script) = h.strip_prefix(crate::m2::PRELOAD_PREFIX) {
                a.preload(now_ms, script);
            }
        }
        for s in scripts.iter().take(MAX_EVENTS_PER_FRAME) {
            if let Some(line) = s.strip_prefix(crate::m2::VOICE_PREFIX) {
                let Some((delay, script)) = line.split_once(':') else {
                    continue;
                };
                let delay = delay.parse::<u64>().unwrap_or(0);
                outcomes.push((
                    "voice",
                    voice(&mut self.popup_voice, a, now_ms, script, delay),
                ));
            } else if let Some(minor) = s.strip_prefix(crate::m2::MINOR_CIV_PREFIX) {
                let script = a
                    .table()
                    .link(asset_format::LinkTable::MinorCiv, minor)
                    .map(str::to_owned);
                if let Some(script) = script {
                    outcomes.push(("voice", voice(&mut self.popup_voice, a, now_ms, &script, 0)));
                }
            } else if let Some(kind) = s.strip_prefix(crate::m2::POPUP_SOUND_PREFIX) {
                outcomes.push(("popup", a.play_popup(now_ms, kind)));
            } else if let Some(name) = s.strip_prefix(crate::m2::EVENT_SOUND_PREFIX) {
                outcomes.push(("event", a.play_event(now_ms, name)));
            } else if let Some(script) = s.strip_prefix(crate::m2::AMBIENCE_START_PREFIX) {
                if let Some(old) = self.ambience.take() {
                    a.stop_script(&old);
                }
                self.ambience = Some(script.to_owned());
                outcomes.push(("ambience", a.play_2d(now_ms, script)));
            } else if s == crate::m2::AMBIENCE_STOP {
                if let Some(old) = self.ambience.take() {
                    a.stop_script(&old);
                }
            } else {
                outcomes.push(("lua", a.play_2d(now_ms, s)));
            }
        }
        // The next notification sound, once a whole second, when the one before has ended (re-b15 ITEM 5 P5.1).
        if let Some(kind) = self.notifications.poll(now_ms, |s| a.is_sounding(s)) {
            let script = a.notification_script(&kind).map(str::to_owned);
            let o = a.play_notification(now_ms, &kind);
            let sounds = matches!(
                o,
                PlayOutcome::Started(_) | PlayOutcome::Pending | PlayOutcome::Delayed
            );
            self.notifications.playing(script.filter(|_| sounds));
            outcomes.push(("notification", o));
        }
        a.tick(now_ms);
        // SIMPLIFIED(SIMP-exe-974): sound files are fetched by priority and a failed fetch is asked for again.
        // Files a call waits for go out at once; the ones fetched ahead wait their turn, a few at a time, so they do not
        // crowd out a file a call needs (or the terrain) on the browser's few connections.
        let (urgent, ahead) = a.take_wanted();
        for f in urgent {
            if let Some(i) = self.ahead.iter().position(|n| *n == f) {
                self.ahead.remove(i);
            }
            if !self.files.iter().any(|(n, _)| *n == f) {
                let h = loader.request(LogicalId::new(f.clone()), &self.profile);
                self.files.push((f, h));
            }
        }
        self.ahead.extend(ahead);
        let mut in_flight = self.files.len();
        while in_flight < MAX_FETCHES_AHEAD
            && let Some(f) = self.ahead.pop_front()
        {
            let h = loader.request(LogicalId::new(f.clone()), &self.profile);
            self.files.push((f, h));
            in_flight += 1;
        }
        let mut done = Vec::new();
        self.files.retain(|(name, h)| match loader.poll(*h) {
            AssetPoll::Pending => true,
            AssetPoll::Ready(bytes) => {
                done.push((name.clone(), Some(bytes)));
                false
            }
            AssetPoll::Failed(e) => {
                tracing::warn!("sound file {name}: {e}");
                done.push((name.clone(), None));
                false
            }
        });
        for (name, bytes) in done {
            match bytes {
                Some(b) => match a.on_file_loaded(now_ms, &name, &b) {
                    Ok(()) => self.stats.files_loaded += 1,
                    Err(e) => {
                        self.stats.files_failed += 1;
                        tracing::warn!("sound file {name}: {e}");
                    }
                },
                None => {
                    self.stats.files_failed += 1;
                    // (a busy server or a dropped connection must not silence the sound for the rest of the session)
                    a.on_file_failed(&name);
                }
            }
        }
        for (src, o) in outcomes {
            match o {
                PlayOutcome::Started(_) => {
                    if self.logged < 8 {
                        self.logged += 1;
                        tracing::info!(source = src, "ui sound started");
                    }
                }
                PlayOutcome::Delayed | PlayOutcome::Pending | PlayOutcome::WaitingForGesture => {
                    self.stats.waiting += 1
                }
                PlayOutcome::UnknownScript | PlayOutcome::NoFile => self.stats.missing += 1,
                _ => self.stats.skipped += 1,
            }
        }
    }
}

/// A popup's spoken line: the line of the popup before ends (`StopAllPopupSounds`), its files are asked for now, it
/// starts after `delay_ms`.
fn voice(
    popup_voice: &mut Option<String>,
    a: &mut Engine,
    now_ms: u64,
    script: &str,
    delay_ms: u64,
) -> PlayOutcome {
    if let Some(old) = popup_voice.take() {
        a.stop_script(&old);
    }
    a.preload(now_ms, script);
    *popup_voice = Some(script.to_owned());
    a.play_2d_after(now_ms, script, delay_ms)
}

/// The game as the music sees it, from what the exe API read.
pub fn music_view(st: &exe_api::music::MusicState) -> MusicView {
    MusicView {
        active: st.active,
        human: st.human,
        era: st.era,
        // (a table without the Renaissance: no borrowing)
        start_era: if st.start_era < 0 {
            i32::MAX
        } else {
            st.start_era
        },
        civs: st
            .players
            .iter()
            .map(|p| MusicCiv {
                id: p.id,
                leader: p.leader.clone(),
                tag: audio::music::soundtrack_tag(Some(&p.soundtrack), &p.civ_type),
                met: p.met,
                at_war: p.at_war,
                major_alive: p.major_alive,
            })
            .collect(),
    }
}

fn music_event(e: exe_api::music::MusicEvent) -> MusicEvent {
    match e {
        exe_api::music::MusicEvent::LeaderView(p) => MusicEvent::LeaderView(p),
        exe_api::music::MusicEvent::LeaveLeaderView => MusicEvent::LeaveLeaderView,
        exe_api::music::MusicEvent::GameOver => MusicEvent::GameOver,
    }
}

impl UiSound {
    /// The music for this frame (re-b15 ITEM 3): `scene` is where the player is, `state` reads the game (called at most
    /// once a second, and at once when an event came), `events` the music's events since the last frame. Call it after
    /// [`Self::update`]. A song streams from its URL (`AssetLoader::url_of`); nothing here waits on the network.
    pub fn music(
        &mut self,
        now_ms: u64,
        loader: &AssetLoader,
        scene: MusicScene,
        state: &dyn Fn() -> Option<exe_api::music::MusicState>,
        events: Vec<exe_api::music::MusicEvent>,
    ) {
        let Stage::Ready(a) = &mut self.stage else {
            return;
        };
        let dt = self.music_last_ms.map_or(0, |t| now_ms.saturating_sub(t));
        self.music_last_ms = Some(now_ms);
        let mut events: Vec<MusicEvent> = events.into_iter().map(music_event).collect();
        if scene == MusicScene::Game {
            let stale = self
                .music_view_ms
                .is_none_or(|t| now_ms.saturating_sub(t) >= audio::music::UPDATE_MS);
            if stale || !events.is_empty() {
                let st = state();
                // A leader screen that closed without LeavingLeaderViewMode (seen down twice, a second apart) is left too,
                // or his theme would come back as each song ends.
                let down = st.as_ref().is_some_and(|s| s.leader_up == Some(false));
                self.leader_down_reads = if down && self.director.leader_view().is_some() {
                    self.leader_down_reads + 1
                } else {
                    0
                };
                if self.leader_down_reads >= 2 {
                    self.leader_down_reads = 0;
                    events.push(MusicEvent::LeaveLeaderView);
                }
                self.music_view = st.map(|s| music_view(&s));
                self.music_view_ms = Some(now_ms);
            }
        } else {
            self.music_view = None;
            self.music_view_ms = None;
        }
        let playing = a.music_playing();
        let cmds = {
            let a: &UiAudio<Counted> = a;
            let has = |n: &str| a.has_script(n);
            self.director
                .update(dt, scene, self.music_view.as_ref(), &events, playing, &has)
        };
        for c in cmds {
            let MusicCmd::Play { script, mode } = c;
            if !a.play_music(now_ms, &script, mode) {
                self.stats.missing += 1;
            }
        }
        let profile = &self.profile;
        a.music_tick(now_ms, &|file| {
            loader.url_of(&LogicalId::new(file.to_owned()), profile)
        });
    }
}

impl SoundStats {
    pub fn as_pairs(&self) -> Vec<(String, f64)> {
        let n = |k: &str, v: f64| (k.to_owned(), v);
        vec![
            n("button_events", self.button_events as f64),
            n("lua_scripts", self.lua_scripts as f64),
            n("started", self.started as f64),
            n("waiting", self.waiting as f64),
            n("skipped", self.skipped as f64),
            n("missing", self.missing as f64),
            n("files_loaded", self.files_loaded as f64),
            n("files_failed", self.files_failed as f64),
            n("dropped", self.dropped as f64),
            n("world_beds", self.world_beds as f64),
            n("world_sounds", self.world_sounds as f64),
            n("unit_sounds", self.unit_sounds as f64),
            n("unit_cues", self.unit_cues as f64),
            n("unlocked", f64::from(u8::from(self.unlocked))),
            n("running", f64::from(u8::from(self.running))),
            n("ready", f64::from(u8::from(self.ready))),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use exe_api::music::{MusicPlayer, MusicState};

    #[test]
    fn the_game_state_becomes_the_music_view_with_the_soundtrack_tags() {
        let st = MusicState {
            active: 0,
            human: true,
            era: 2,
            start_era: -1,
            leader_up: None,
            players: vec![
                MusicPlayer {
                    id: 0,
                    leader: "LEADER_KAMEHAMEHA".into(),
                    civ_type: "CIVILIZATION_POLYNESIA".into(),
                    soundtrack: "Aztec".into(),
                    met: true,
                    at_war: false,
                    major_alive: true,
                },
                MusicPlayer {
                    id: 1,
                    leader: "LEADER_AUGUSTUS".into(),
                    civ_type: "CIVILIZATION_ROME".into(),
                    at_war: true,
                    ..MusicPlayer::default()
                },
            ],
        };
        let v = music_view(&st);
        assert_eq!(v.civs[0].tag, "AZTEC");
        assert_eq!(v.civs[1].tag, "ROME");
        assert!(v.civs[1].at_war);
        assert_eq!(
            v.start_era,
            i32::MAX,
            "no Renaissance in the table: nothing is borrowed"
        );
    }

    #[test]
    fn each_knob_is_kept_by_its_name() {
        let mut s = UiSound::new(
            Box::new(platform::NullOutput::default()),
            [0.1, 0.2, 0.3, 0.4],
        );
        assert_eq!(s.knob("MUSIC"), 0.1);
        s.set_knob("AMBIENCE", 2.0);
        assert_eq!(s.knob("AMBIENCE"), 1.0);
        s.set_knob("MASTER", 0.0);
        assert_eq!(s.volume(), 0.2, "the SFX knob");
    }
}
