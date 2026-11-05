//! The volume buses (re-b15 ITEM 1 E1, ITEM 8): every sound's gain is its script volume times the knob of its bus, so
//! each slider of the options menu scales only its own sounds, at once and also for the voices that sound. The music
//! bus is ducked while a sound with `fTaperSoundtrackVolume` plays (E5).
//!
//! `AudioParams.xml` `SoundTypes`: `USER_VOLUME_MUSIC {GAME_MUSIC, GAME_MUSIC_STINGS}`, `USER_VOLUME_SFX {GAME_SFX,
//! GAME_INTERFACE}`, `USER_VOLUME_SPEECH {GAME_SPEECH, GAME_ADVISOR_SPEECH}`, `USER_VOLUME_AMBIENCE {GAME_AMBIENCE}`
//! under `MASTER`. The duck acts on the `GAME_MUSIC` knob only (the stings, which mute the music, are not ducked).

use std::collections::HashMap;

use asset_format::SoundKind;
use platform::{AudioOutput, VoiceId};

use super::{Category, UiAudio};

/// A sound whose bus gain is this low or lower is not started (`ShouldNewSoundPlay` 0x08d19662), unless it loops.
pub const MIN_BUS_GAIN: f32 = 0.01;
/// The music bus falls to the taper in this long (`DuckSoundtrackVolume(taper, 0.5)`).
pub const DUCK_FADE_MS: u64 = 500;
/// And comes back in this long when the last tapered sound ended (`UnDuckSoundtrackVolume` 0x08d2f888: 1.0 s).
pub const UNDUCK_FADE_MS: u64 = 1000;

impl Category {
    /// The bus of a volume knob of the options menu (`USER_VOLUME_<name>`, the names of `m4_wog::KNOBS`).
    pub fn of_knob(name: &str) -> Option<Self> {
        let n = name.strip_prefix("USER_VOLUME_").unwrap_or(name);
        match n.to_ascii_uppercase().as_str() {
            "MUSIC" => Some(Self::Music),
            "SFX" => Some(Self::Sfx),
            "SPEECH" => Some(Self::Speech),
            "AMBIENCE" => Some(Self::Ambience),
            _ => None,
        }
    }
}

/// A linear ramp of a level (the duck, a crossfade).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Ramp {
    pub(super) from: f32,
    pub(super) to: f32,
    pub(super) start_ms: u64,
    pub(super) len_ms: u64,
}

impl Ramp {
    pub(super) fn at(&self, now_ms: u64) -> f32 {
        if self.len_ms == 0 || now_ms >= self.start_ms + self.len_ms {
            return self.to;
        }
        let t = now_ms.saturating_sub(self.start_ms) as f32 / self.len_ms as f32;
        self.from + (self.to - self.from) * t
    }
}

#[derive(Debug, Clone, Copy)]
struct Tracked {
    kind: SoundKind,
    /// The gain without the bus: script volume times the share asked for.
    free: f32,
}

/// The voices the buses scale and the duck of the music.
#[derive(Debug)]
pub(super) struct Mixer {
    voices: HashMap<VoiceId, Tracked>,
    /// The sounds that duck the music now and their taper.
    duckers: Vec<(VoiceId, f32)>,
    /// The lowest taper since the duck began (`None`: not ducked).
    ducked_to: Option<f32>,
    ramp: Ramp,
    /// The duck level of the last tick (1.0: none).
    duck: f32,
}

impl Default for Mixer {
    fn default() -> Self {
        Self {
            voices: HashMap::new(),
            duckers: Vec::new(),
            ducked_to: None,
            ramp: Ramp {
                from: 1.0,
                to: 1.0,
                start_ms: 0,
                len_ms: 0,
            },
            duck: 1.0,
        }
    }
}

/// The taper of a script: `fTaperSoundtrackVolume` when present and not 1.0 (the exe stores it only then).
pub(super) fn taper(permille: Option<u16>) -> Option<f32> {
    permille
        .filter(|p| *p != 1000)
        .map(|p| f32::from(p) / 1000.0)
}

impl<O: AudioOutput> UiAudio<O> {
    /// A volume knob of the options menu moved (`SetVolumeKnobValue`): `"MUSIC"`, `"SFX"`, `"SPEECH"` or
    /// `"AMBIENCE"`, 0..=1. Only the sounds of that bus change. Unknown names are ignored.
    pub fn set_knob(&mut self, knob: &str, value: f32) {
        if let Some(c) = Category::of_knob(knob) {
            self.set_category_volume(c, value);
        }
    }

    /// The gain of the bus of a sound kind now: its knob, and the duck for `GAME_MUSIC`.
    pub fn bus_gain(&self, kind: SoundKind) -> f32 {
        let knob = self.category_volume(Category::of(kind));
        if kind == SoundKind::Music {
            knob * self.mixer.duck
        } else {
            knob
        }
    }

    /// [`Self::bus_gain`] of a script (a key); 1.0 for one the table lacks.
    pub(super) fn script_bus(&self, script: &str) -> f32 {
        self.scripts
            .get(script)
            .map_or(1.0, |s| self.bus_gain(s.kind))
    }

    /// The duck level of the music now (1.0: not ducked).
    pub fn music_duck(&self) -> f32 {
        self.mixer.duck
    }

    /// Sets the gain of every tracked voice of a bus again (a knob moved, the duck moved).
    pub(super) fn rebus(&mut self, c: Category) {
        let list: Vec<(VoiceId, Tracked)> = self
            .mixer
            .voices
            .iter()
            .filter(|(_, t)| Category::of(t.kind) == c)
            .map(|(v, t)| (*v, *t))
            .collect();
        for (v, t) in list {
            let g = t.free * self.bus_gain(t.kind);
            self.out.set_voice_gain(v, g);
        }
        self.music_rebus();
    }

    /// `start` started a voice of `script` (a key) at `free` (its gain without the bus): the bus follows it from now on,
    /// and a tapered script ducks the music.
    pub(super) fn voice_started(&mut self, now_ms: u64, script: &str, v: VoiceId, free: f32) {
        let Some(sc) = self.scripts.get(script) else {
            return;
        };
        let (kind, taper) = (sc.kind, taper(sc.taper_soundtrack_permille));
        self.mixer.voices.insert(v, Tracked { kind, free });
        if let Some(t) = taper {
            self.duck_by(now_ms, v, t);
        }
    }

    /// The gain of a voice the engine started changes (a world bed follows the camera): `free` is without the bus.
    pub(super) fn set_free_gain(&mut self, v: VoiceId, free: f32) {
        let kind = match self.mixer.voices.get_mut(&v) {
            Some(t) => {
                t.free = free;
                t.kind
            }
            None => SoundKind::Sfx,
        };
        let g = free * self.bus_gain(kind);
        self.out.set_voice_gain(v, g);
    }

    /// `FSoundBase::OnPlaybackActivated`: the music bus fades to `taper` (an absolute level) in 0.5 s; the lowest of the
    /// sounds that duck wins.
    fn duck_by(&mut self, now_ms: u64, v: VoiceId, taper: f32) {
        let m = &mut self.mixer;
        m.duckers.push((v, taper));
        let to = m.ducked_to.map_or(taper, |d| d.min(taper));
        if m.ducked_to != Some(to) {
            m.ducked_to = Some(to);
            m.ramp = Ramp {
                from: m.ramp.at(now_ms),
                to,
                start_ms: now_ms,
                len_ms: DUCK_FADE_MS,
            };
        }
    }

    /// Once a frame: forgets the voices that ended, lets the music come back when the last tapered sound ended
    /// (`OnPlaybackDeactivated`, 1.0 s), and moves the music bus along its ramp.
    pub(super) fn mixer_tick(&mut self, now_ms: u64) {
        let out = &self.out;
        self.mixer.voices.retain(|v, _| out.is_playing(*v));
        self.mixer.duckers.retain(|(v, _)| out.is_playing(*v));
        let m = &mut self.mixer;
        if m.duckers.is_empty() && m.ducked_to.take().is_some() {
            m.ramp = Ramp {
                from: m.ramp.at(now_ms),
                to: 1.0,
                start_ms: now_ms,
                len_ms: UNDUCK_FADE_MS,
            };
        }
        let d = m.ramp.at(now_ms);
        if (d - m.duck).abs() > 1e-4 {
            m.duck = d;
            self.rebus(Category::Music);
        }
    }

    /// Whether a sound of `kind` may start now: its bus is above [`MIN_BUS_GAIN`], or it loops (a loop follows the
    /// knob when it comes up again).
    pub(super) fn bus_allows(&self, kind: SoundKind, looping: bool) -> bool {
        looping || self.bus_gain(kind) > MIN_BUS_GAIN
    }
}

#[cfg(test)]
mod tests {
    use asset_format::{AudioTable, SoundDef, SoundKind, SoundLoad, SoundScript};
    use platform::NullOutput;

    use super::*;
    use crate::engine::PlayOutcome;
    use crate::wav::tests::pcm16;

    fn script(name: &str, kind: SoundKind, taper: Option<u16>) -> SoundScript {
        let mut s = SoundScript::new(name, "SND_A", kind);
        s.min_volume = 50;
        s.max_volume = 50;
        s.taper_soundtrack_permille = taper;
        s
    }

    fn engine() -> UiAudio<NullOutput> {
        let mut scripts = vec![
            script("AS2D_CLICK", SoundKind::Sfx, None),
            script("AS2D_BIRD", SoundKind::Ambience, None),
            script("AS2D_LINE", SoundKind::Speech, Some(850)),
            script("AS2D_QUOTE", SoundKind::MusicSting, Some(0)),
            script("AS2D_FLAT", SoundKind::Speech, Some(1000)),
            script("AS2D_SONG", SoundKind::Music, None),
        ];
        scripts.sort_by(|a, b| a.name.cmp(&b.name));
        let table = AudioTable {
            scripts,
            sounds: vec![SoundDef {
                id: "SND_A".into(),
                filename: "a".into(),
                load: SoundLoad::DynamicRes,
                one_variation: false,
                files: vec!["ui:au:a.wav".into()],
            }],
            links: vec![],
        };
        let mut a = UiAudio::new(table, NullOutput::running(), 3);
        a.preload(0, "AS2D_CLICK");
        a.on_file_loaded(0, "ui:au:a.wav", &pcm16(8000, &[0; 80]))
            .unwrap();
        a
    }

    fn started(o: PlayOutcome) -> VoiceId {
        match o {
            PlayOutcome::Started(v) => v,
            other => panic!("not started: {other:?}"),
        }
    }

    #[test]
    fn each_knob_names_its_own_bus() {
        assert_eq!(Category::of_knob("MUSIC"), Some(Category::Music));
        assert_eq!(Category::of_knob("USER_VOLUME_SFX"), Some(Category::Sfx));
        assert_eq!(Category::of_knob("speech"), Some(Category::Speech));
        assert_eq!(Category::of_knob("AMBIENCE"), Some(Category::Ambience));
        assert_eq!(Category::of_knob("MASTER"), None);
    }

    #[test]
    fn a_slider_scales_only_the_sounds_of_its_bus_and_the_ones_that_sound_already() {
        let mut a = engine();
        let click = started(a.play_2d(10, "AS2D_CLICK"));
        let bird = started(a.play_2d(10, "AS2D_BIRD"));
        a.output_mut().gain_sets.clear();
        a.set_knob("SFX", 0.4);
        // The click (SFX) goes to 0.5 * 0.4; the bird (ambience) is not touched; the master stays.
        assert_eq!(a.output().gain_sets, [(click, 0.2)]);
        assert_eq!(a.output().master, 1.0);
        a.set_knob("AMBIENCE", 0.5);
        assert_eq!(*a.output().gain_sets.last().unwrap(), (bird, 0.25));
        // A new click starts at its bus gain.
        a.play_2d(20, "AS2D_CLICK");
        assert!((a.output().played.last().unwrap().2 - 0.2).abs() < 1e-6);
        assert_eq!(a.category_volume(Category::Music), 1.0);
    }

    #[test]
    fn a_sound_on_a_silent_bus_does_not_start() {
        let mut a = engine();
        a.set_knob("SPEECH", 0.005);
        assert_eq!(a.play_2d(0, "AS2D_LINE"), PlayOutcome::Muted);
        a.set_knob("SPEECH", 0.02);
        assert!(matches!(a.play_2d(0, "AS2D_LINE"), PlayOutcome::Started(_)));
    }

    /// The music level at `now` after a tick.
    fn duck_at(a: &mut UiAudio<NullOutput>, now: u64) -> f32 {
        a.tick(now);
        a.music_duck()
    }

    #[test]
    fn a_speech_line_ducks_the_music_in_half_a_second_and_it_comes_back_in_one() {
        let mut a = engine();
        let line = started(a.play_2d(1000, "AS2D_LINE"));
        assert!((duck_at(&mut a, 1000) - 1.0).abs() < 1e-6);
        assert!(
            (duck_at(&mut a, 1250) - 0.925).abs() < 1e-4,
            "half way to 0.85"
        );
        assert!((duck_at(&mut a, 1500) - 0.85).abs() < 1e-6);
        assert!(
            (duck_at(&mut a, 5000) - 0.85).abs() < 1e-6,
            "held while it plays"
        );
        assert!((a.bus_gain(SoundKind::Music) - 0.85).abs() < 1e-6);
        assert_eq!(
            a.bus_gain(SoundKind::MusicSting),
            1.0,
            "the stings are not ducked"
        );
        a.stop(line);
        assert!((duck_at(&mut a, 6000) - 0.85).abs() < 1e-6);
        assert!(
            (duck_at(&mut a, 6500) - 0.925).abs() < 1e-4,
            "half way back"
        );
        assert!((duck_at(&mut a, 7000) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_wonder_quote_mutes_the_music_and_the_lowest_taper_wins() {
        let mut a = engine();
        let line = started(a.play_2d(0, "AS2D_LINE"));
        duck_at(&mut a, 600);
        let quote = started(a.play_2d(600, "AS2D_QUOTE"));
        assert!((duck_at(&mut a, 850) - 0.425).abs() < 1e-4);
        assert_eq!(duck_at(&mut a, 1100), 0.0);
        // The quote ends, the line goes on: the music stays down until the last one ends.
        a.stop(quote);
        assert_eq!(duck_at(&mut a, 2000), 0.0);
        a.stop(line);
        duck_at(&mut a, 3000);
        assert_eq!(duck_at(&mut a, 4000), 1.0);
    }

    #[test]
    fn a_taper_of_one_does_not_duck_and_the_music_voices_follow_the_duck() {
        let mut a = engine();
        a.play_2d(0, "AS2D_FLAT");
        assert_eq!(duck_at(&mut a, 1000), 1.0);
        let song = started(a.play_2d(0, "AS2D_SONG"));
        a.set_knob("MUSIC", 0.5);
        assert_eq!(*a.output().gain_sets.last().unwrap(), (song, 0.25));
        a.play_2d(1000, "AS2D_LINE");
        duck_at(&mut a, 1000);
        duck_at(&mut a, 1600);
        let (v, g) = *a.output().gain_sets.last().unwrap();
        assert_eq!(v, song);
        assert!((g - 0.5 * 0.5 * 0.85).abs() < 1e-6, "{g}");
    }
}
