//! The engine's part of the music (`FMusicManager::PlayMusic` 0x08d2f176): one song at a time, each change a 2 s
//! crossfade (the new song fades in, the old one fades out and ends, or is held to be resumed). A song is streamed from
//! its URL when the output can (a browser's `<audio>`), else its file is fetched whole like any other sound.

use std::collections::HashSet;

use asset_format::SoundKind;
use platform::{AudioOutput, VoiceId};

use super::mixer::Ramp;
use super::{FileState, UiAudio, key};
use crate::music::{CROSSFADE_S, MusicMode};

const FADE_MS: u64 = (CROSSFADE_S * 1000.0) as u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum After {
    Keep,
    Stop,
    Hold,
}

#[derive(Debug)]
struct Song {
    script: String,
    file: String,
    voice: Option<VoiceId>,
    /// The script volume drawn for this play (`PlayMusic` volume -1: the script's own).
    volume: f32,
    looped: bool,
    kind: SoundKind,
    ramp: Ramp,
    after: After,
    /// The gain sent last (`NAN`: send again).
    sent: f32,
}

/// The songs that sound, fade or are held.
#[derive(Debug, Default)]
pub(super) struct MusicVoices {
    current: Option<Song>,
    leaving: Vec<Song>,
    held: Vec<Song>,
    unknown_logged: HashSet<String>,
}

fn fade_in(now_ms: u64) -> Ramp {
    Ramp {
        from: 0.0,
        to: 1.0,
        start_ms: now_ms,
        len_ms: FADE_MS,
    }
}

impl<O: AudioOutput> UiAudio<O> {
    /// The sound table has this script (the music director counts the songs of a playlist with it).
    pub fn has_script(&self, name: &str) -> bool {
        self.scripts.contains_key(&key(name))
    }

    /// `FAudioGame::PlayMusic(script, volume -1, crossfade 2.0, mode)`. The same song that sounds goes on (nothing
    /// happens); `false` when the table has no such script or no file for it.
    pub fn play_music(&mut self, now_ms: u64, script: &str, mode: MusicMode) -> bool {
        let k = key(script);
        if let Some(c) = &self.music.current
            && c.script == k
            && c.voice.is_none_or(|v| self.out.is_playing(v))
        {
            return true;
        }
        let found = self.scripts.get(&k).cloned().and_then(|sc| {
            let files = self.table.sound(&sc.sound).map(|d| d.files.clone())?;
            (!files.is_empty()).then_some((sc, files))
        });
        // The old song leaves either way (the exe's crossfade runs before it knows the new one sounds).
        if let Some(mut old) = self.music.current.take() {
            old.after = if mode == MusicMode::HoldOld {
                After::Hold
            } else {
                After::Stop
            };
            old.ramp = Ramp {
                from: old.ramp.at(now_ms),
                to: 0.0,
                start_ms: now_ms,
                len_ms: FADE_MS,
            };
            if old.voice.is_some() {
                self.music.leaving.push(old);
            }
        }
        let Some((sc, files)) = found else {
            if self.music.unknown_logged.insert(k.clone()) {
                tracing::info!(script, "no such music in the sound table");
            }
            return false;
        };
        if mode == MusicMode::Resume
            && let Some(i) = self.music.held.iter().position(|s| s.script == k)
        {
            let mut s = self.music.held.remove(i);
            if s.voice.is_some_and(|v| self.out.unpause(v)) {
                s.after = After::Keep;
                s.ramp = fade_in(now_ms);
                s.sent = f32::NAN;
                self.music.current = Some(s);
                return true;
            }
            if let Some(v) = s.voice {
                self.out.stop(v);
            }
        }
        // One song is held at most: the one that leaves now replaces an older one.
        if mode == MusicMode::HoldOld {
            for s in self.music.held.drain(..) {
                if let Some(v) = s.voice {
                    self.out.stop(v);
                }
            }
        }
        let volume = self.range(u64::from(sc.min_volume), u64::from(sc.max_volume)) as f32 / 100.0;
        let file = files[self.range(0, files.len() as u64 - 1) as usize].clone();
        if !self.out.can_stream() && !self.files.contains_key(&file) {
            self.sample_for(&file).state = FileState::Wanted { since_ms: now_ms };
            self.ask_urgent(&file);
        }
        self.music.current = Some(Song {
            script: k,
            file,
            voice: None,
            volume,
            looped: sc.looping,
            kind: sc.kind,
            ramp: fade_in(now_ms),
            after: After::Keep,
            sent: f32::NAN,
        });
        true
    }

    /// The song asked for last still sounds, or waits to start (the director waits for its end before the gap).
    pub fn music_playing(&self) -> bool {
        self.music
            .current
            .as_ref()
            .is_some_and(|c| c.voice.is_none_or(|v| self.out.is_playing(v)))
    }

    /// The script of the song that sounds or waits to start (a key: lower case).
    pub fn current_music(&self) -> Option<&str> {
        self.music.current.as_ref().map(|c| c.script.as_str())
    }

    /// Once a frame: starts the song that waits (`url_of` gives the URL of a file to stream), moves the crossfades and
    /// ends or holds the songs that faded out. SIMPLIFIED(SIMP-exe-1194): the 2 s linear crossfade is stepped once a
    /// frame through `set_voice_gain` (the browser glides each step over 0.25 s), not by Miles' own fade; a song's 2 s
    /// starts when its stream starts, not when it was asked for.
    pub fn music_tick(&mut self, now_ms: u64, url_of: &dyn Fn(&str) -> Option<String>) {
        self.start_waiting_song(now_ms, url_of);
        let mut songs: Vec<Song> = self.music.current.take().into_iter().collect();
        let n_current = songs.len();
        songs.append(&mut self.music.leaving);
        let mut keep = Vec::new();
        for (i, mut s) in songs.into_iter().enumerate() {
            if let Some(v) = s.voice {
                let g = s.volume * s.ramp.at(now_ms) * self.bus_gain(s.kind);
                if s.sent.is_nan() || (g - s.sent).abs() > 1e-3 {
                    self.out.set_voice_gain(v, g);
                    s.sent = g;
                }
                if i >= n_current && now_ms >= s.ramp.start_ms + s.ramp.len_ms {
                    match s.after {
                        After::Hold => {
                            self.out.pause(v);
                            self.music.held.push(s);
                        }
                        _ => self.out.stop(v),
                    }
                    continue;
                }
            }
            keep.push((i < n_current, s));
        }
        for (is_current, s) in keep {
            if is_current {
                self.music.current = Some(s);
            } else {
                self.music.leaving.push(s);
            }
        }
    }

    fn start_waiting_song(&mut self, now_ms: u64, url_of: &dyn Fn(&str) -> Option<String>) {
        let Some(c) = &self.music.current else {
            return;
        };
        if c.voice.is_some() || !self.out.is_running() {
            return;
        }
        let (file, looped) = (c.file.clone(), c.looped);
        let voice = if self.out.can_stream() {
            match url_of(&file) {
                Some(url) => self.out.play_stream(&url, 0.0, looped),
                None => {
                    tracing::warn!(file, "the music file is not in the manifest");
                    self.music.current = None;
                    return;
                }
            }
        } else {
            if !self.files.contains_key(&file) {
                // (asked again after a failed fetch)
                self.sample_for(&file).state = FileState::Wanted { since_ms: now_ms };
                self.ask_urgent(&file);
            }
            let (sample, state) = {
                let e = &self.files[&file];
                (e.sample, e.state)
            };
            match state {
                FileState::Loaded if self.out.is_loaded(sample) => {
                    self.out.play(sample, 0.0, looped)
                }
                FileState::Failed => {
                    self.music.current = None;
                    return;
                }
                FileState::Loaded | FileState::Wanted { .. } => None,
            }
        };
        if let (Some(v), Some(c)) = (voice, self.music.current.as_mut()) {
            c.voice = Some(v);
            c.ramp = fade_in(now_ms);
            c.sent = f32::NAN;
        }
    }

    /// A knob or the duck moved: the songs take their gain again at the next tick.
    pub(super) fn music_rebus(&mut self) {
        for s in self
            .music
            .current
            .iter_mut()
            .chain(self.music.leaving.iter_mut())
        {
            s.sent = f32::NAN;
        }
    }
}

#[cfg(test)]
mod tests {
    use asset_format::{AudioTable, SoundDef, SoundKind, SoundLoad, SoundScript};
    use platform::NullOutput;

    use super::*;
    use crate::wav::tests::pcm16;

    fn song(name: &str, file: &str, vol: u16) -> (SoundScript, SoundDef) {
        let mut s = SoundScript::new(name, &format!("SND_{name}"), SoundKind::Music);
        s.min_volume = vol;
        s.max_volume = vol;
        s.is_music = true;
        let d = SoundDef {
            id: format!("SND_{name}"),
            filename: file.into(),
            load: SoundLoad::Streamed,
            one_variation: false,
            files: vec![format!("ui:au:{file}.ogg")],
        };
        (s, d)
    }

    fn engine(stream: bool) -> UiAudio<NullOutput> {
        let mut scripts = Vec::new();
        let mut sounds = Vec::new();
        for (n, f, v) in [
            ("SONG_A", "a", 40),
            ("SONG_B", "b", 40),
            ("AS2D_LEADER_MUSIC_X_PEACE", "x", 50),
        ] {
            let (s, d) = song(n, f, v);
            scripts.push(s);
            sounds.push(d);
        }
        let mut line = SoundScript::new("AS2D_LINE", "SND_LINE", SoundKind::Speech);
        line.taper_soundtrack_permille = Some(850);
        scripts.push(line);
        sounds.push(SoundDef {
            id: "SND_LINE".into(),
            filename: "line".into(),
            load: SoundLoad::DynamicRes,
            one_variation: false,
            files: vec!["ui:au:line.wav".into()],
        });
        scripts.sort_by(|a, b| a.name.cmp(&b.name));
        sounds.sort_by(|a, b| a.id.cmp(&b.id));
        let mut out = NullOutput::running();
        out.streams_on = stream;
        UiAudio::new(
            AudioTable {
                scripts,
                sounds,
                links: vec![],
            },
            out,
            9,
        )
    }

    fn url(file: &str) -> Option<String> {
        Some(format!("https://civ5.example/assets/{file}"))
    }

    fn gain_of(a: &UiAudio<NullOutput>, v: VoiceId) -> f32 {
        a.output()
            .gain_sets
            .iter()
            .rev()
            .find(|(x, _)| *x == v)
            .map(|(_, g)| *g)
            .unwrap()
    }

    #[test]
    fn a_song_streams_from_its_url_and_fades_in_over_two_seconds() {
        let mut a = engine(true);
        assert!(a.play_music(0, "song_a", MusicMode::New));
        assert!(a.music_playing(), "it waits to start: it counts as playing");
        a.music_tick(100, &url);
        let (v, u, g, looped) = a.output().streams[0].clone();
        assert_eq!(u, "https://civ5.example/assets/ui:au:a.ogg");
        assert_eq!((g, looped), (0.0, false));
        a.music_tick(1100, &url);
        assert!((gain_of(&a, v) - 0.2).abs() < 1e-6, "half way: 0.4 * 0.5");
        a.music_tick(2100, &url);
        assert!((gain_of(&a, v) - 0.4).abs() < 1e-6);
        assert!(
            a.take_wanted().0.is_empty(),
            "a stream is not fetched whole"
        );
    }

    #[test]
    fn a_new_song_crossfades_and_the_old_one_ends() {
        let mut a = engine(true);
        a.play_music(0, "SONG_A", MusicMode::New);
        a.music_tick(0, &url);
        a.music_tick(3000, &url);
        let old = a.output().streams[0].0;
        a.play_music(3000, "SONG_B", MusicMode::New);
        a.music_tick(3000, &url);
        let new = a.output().streams[1].0;
        a.music_tick(4000, &url);
        assert!((gain_of(&a, old) - 0.2).abs() < 1e-6);
        assert!((gain_of(&a, new) - 0.2).abs() < 1e-6);
        a.music_tick(5000, &url);
        assert_eq!(a.output().stopped, [old]);
        assert!(a.output().paused.is_empty());
        assert_eq!(a.current_music(), Some("song_b"));
        // The same song asked for again goes on.
        assert!(a.play_music(5000, "SONG_B", MusicMode::New));
        a.music_tick(5100, &url);
        assert_eq!(a.output().streams.len(), 2);
    }

    #[test]
    fn a_held_song_is_paused_and_comes_back_where_it_stopped() {
        let mut a = engine(true);
        a.play_music(0, "SONG_A", MusicMode::New);
        a.music_tick(0, &url);
        let song = a.output().streams[0].0;
        a.play_music(10_000, "AS2D_LEADER_MUSIC_X_PEACE", MusicMode::HoldOld);
        a.music_tick(10_000, &url);
        a.music_tick(12_000, &url);
        assert_eq!(a.output().paused, [song]);
        assert!(a.output().stopped.is_empty());
        a.play_music(20_000, "SONG_A", MusicMode::Resume);
        a.music_tick(20_000, &url);
        assert!(a.output().paused.is_empty(), "unpaused");
        assert_eq!(a.output().streams.len(), 2, "no new stream");
        a.music_tick(21_000, &url);
        assert!((gain_of(&a, song) - 0.2).abs() < 1e-6, "it fades in again");
        a.music_tick(22_000, &url);
        let theme = a.output().streams[1].0;
        assert_eq!(a.output().stopped, [theme]);
    }

    #[test]
    fn resume_with_nothing_held_starts_the_song_anew() {
        let mut a = engine(true);
        a.play_music(0, "SONG_A", MusicMode::Resume);
        a.music_tick(0, &url);
        assert_eq!(a.output().streams.len(), 1);
    }

    #[test]
    fn an_ended_song_is_not_playing_and_an_unknown_one_is_refused() {
        let mut a = engine(true);
        a.play_music(0, "SONG_A", MusicMode::New);
        a.music_tick(0, &url);
        let v = a.output().streams[0].0;
        a.output_mut().live.retain(|x| *x != v);
        assert!(!a.music_playing());
        assert!(!a.play_music(10, "SONG_NOPE", MusicMode::New));
        assert!(!a.music_playing());
        // A song whose file is not in the manifest gives up (the director then waits the gap and moves on).
        a.play_music(20, "SONG_B", MusicMode::New);
        a.music_tick(20, &|_| None);
        assert!(!a.music_playing());
    }

    #[test]
    fn without_streams_the_file_is_fetched_whole_and_played_as_a_sample() {
        let mut a = engine(false);
        a.play_music(0, "SONG_A", MusicMode::New);
        assert_eq!(a.take_wanted().0, ["ui:au:a.ogg"]);
        a.music_tick(10, &url);
        assert!(a.output().played.is_empty());
        a.on_file_loaded(50, "ui:au:a.ogg", &pcm16(8000, &[0; 80]))
            .unwrap();
        a.music_tick(60, &url);
        assert_eq!(a.output().played.len(), 1);
        assert_eq!(a.output().played[0].2, 0.0, "it starts silent and fades in");
    }

    #[test]
    fn the_music_slider_and_a_speech_line_set_the_gain_of_the_song() {
        let mut a = engine(true);
        a.play_music(0, "SONG_A", MusicMode::New);
        a.music_tick(0, &url);
        a.music_tick(2000, &url);
        let v = a.output().streams[0].0;
        a.set_knob("MUSIC", 0.5);
        a.music_tick(2100, &url);
        assert!((gain_of(&a, v) - 0.2).abs() < 1e-6);
        a.set_knob("SFX", 0.1);
        a.preload(2100, "AS2D_LINE");
        a.on_file_loaded(2100, "ui:au:line.wav", &pcm16(8000, &[0; 80]))
            .unwrap();
        a.play_2d(3000, "AS2D_LINE");
        a.tick(3000);
        a.tick(3500);
        a.music_tick(3500, &url);
        assert!((gain_of(&a, v) - 0.2 * 0.85).abs() < 1e-6);
    }
}
