//! Sound output. The logic of scripts, volumes and repeats lives in the
//! `audio` crate; an implementation of this trait only plays what it is given.

/// Decoded sound: interleaved 16-bit samples.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pcm {
    pub rate: u32,
    pub channels: u16,
    pub samples: Vec<i16>,
}

impl Pcm {
    /// Frames per channel.
    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels.max(1))
    }

    pub fn duration_ms(&self) -> u64 {
        if self.rate == 0 {
            return 0;
        }
        self.frames() as u64 * 1000 / u64::from(self.rate)
    }
}

/// Handle of a loaded sound. The caller chooses it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SampleId(pub u32);

/// Handle of a playing voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VoiceId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AudioOutError {
    /// The backend cannot decode this encoding (mp3, ogg).
    #[error("this output cannot decode {0}")]
    Unsupported(&'static str),
    #[error("audio output failed: {0}")]
    Failed(String),
}

/// How a voice is played beyond its gain: the playback rate (a pitch change resamples, `2^(n/12)` for `n` semitones)
/// and the stereo position (-1 left, 0 middle, 1 right).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VoiceParams {
    pub rate: f32,
    pub pan: f32,
}

impl Default for VoiceParams {
    fn default() -> Self {
        Self {
            rate: 1.0,
            pan: 0.0,
        }
    }
}

pub trait AudioOutput {
    /// Takes decoded samples under `sample`; an earlier sound with that id is replaced.
    fn load_pcm(&mut self, sample: SampleId, pcm: &Pcm) -> Result<(), AudioOutError>;
    /// Takes encoded bytes (mp3, ogg). The load may finish later; `play` before that returns `None`.
    fn load_encoded(&mut self, sample: SampleId, bytes: &[u8]) -> Result<(), AudioOutError>;
    fn is_loaded(&self, sample: SampleId) -> bool;
    /// `gain` is linear, 0.0 to 1.0 or more. `None`: not loaded, or the output is not running.
    fn play(&mut self, sample: SampleId, gain: f32, looped: bool) -> Option<VoiceId>;
    /// [`Self::play`] at a playback rate and a stereo position. An output that cannot do them plays it plainly.
    fn play_with(
        &mut self,
        sample: SampleId,
        gain: f32,
        looped: bool,
        _params: VoiceParams,
    ) -> Option<VoiceId> {
        self.play(sample, gain, looped)
    }
    /// Moves a voice that is sounding in the stereo field (a world ambience heard from the left). An output that
    /// cannot do it keeps the voice where it started.
    fn set_voice_pan(&mut self, _voice: VoiceId, _pan: f32) {}
    fn stop(&mut self, voice: VoiceId);
    /// Changes the gain of a voice that is sounding (a world ambience that fades with the camera's distance). An
    /// output that cannot do it keeps the gain the voice started with.
    fn set_voice_gain(&mut self, _voice: VoiceId, _gain: f32) {}
    /// Lets a voice die away over `secs` and then stops it (the exe's one-second fade of a world ambience that is
    /// left). It no longer counts as sounding. An output that cannot fade stops it at once.
    fn fade_out(&mut self, voice: VoiceId, _secs: f32) {
        self.stop(voice);
    }
    fn set_master_gain(&mut self, gain: f32);
    /// The output may make sound. A browser allows it only after a user gesture.
    fn is_running(&self) -> bool;
    /// Called from a user gesture handler; starts a suspended output.
    fn resume(&mut self);
    /// Whether a voice is still sounding.
    fn is_playing(&self, voice: VoiceId) -> bool;
    /// The output plays a long file from its URL while it downloads ([`Self::play_stream`]: the music). Without it the
    /// engine fetches the whole file and plays it as a sample.
    fn can_stream(&self) -> bool {
        false
    }
    /// Starts the file at `url` while it downloads (an `<audio>` element in a browser), at `gain`. `None`: the output
    /// cannot stream or is not running. A stream that fails to load ends (no longer [`Self::is_playing`]).
    fn play_stream(&mut self, _url: &str, _gain: f32, _looped: bool) -> Option<VoiceId> {
        None
    }
    /// Holds a voice where it is (the music of the playlist while a leader speaks); it still counts as sounding. An
    /// output that cannot pause stops it.
    fn pause(&mut self, voice: VoiceId) {
        self.stop(voice);
    }
    /// Goes on with a paused voice; `false` when the output cannot (the voice is gone).
    fn unpause(&mut self, _voice: VoiceId) -> bool {
        false
    }
}

/// A recording output for tests and for platforms without sound.
#[derive(Debug, Default)]
pub struct NullOutput {
    pub running: bool,
    pub master: f32,
    pub loaded: Vec<SampleId>,
    /// `(voice, sample, gain, looped)` in call order.
    pub played: Vec<(VoiceId, SampleId, f32, bool)>,
    pub stopped: Vec<VoiceId>,
    /// `(voice, gain)` of the `set_voice_gain` calls, in call order.
    pub gain_sets: Vec<(VoiceId, f32)>,
    /// `(voice, seconds)` of the `fade_out` calls, in call order.
    pub faded: Vec<(VoiceId, f32)>,
    /// `(voice, params)` of the voices started with `play_with`, in call order.
    pub params: Vec<(VoiceId, VoiceParams)>,
    /// `(voice, pan)` of the `set_voice_pan` calls, in call order.
    pub pan_sets: Vec<(VoiceId, f32)>,
    next_voice: u64,
    /// Voices that count as still sounding.
    pub live: Vec<VoiceId>,
    /// `(voice, url, gain, looped)` of the `play_stream` calls, in call order (only with `streams_on`).
    pub streams: Vec<(VoiceId, String, f32, bool)>,
    /// The output plays streams ([`AudioOutput::can_stream`]); off by default.
    pub streams_on: bool,
    /// Voices paused now.
    pub paused: Vec<VoiceId>,
}

impl NullOutput {
    pub fn running() -> Self {
        Self {
            running: true,
            master: 1.0,
            ..Self::default()
        }
    }
}

impl AudioOutput for NullOutput {
    fn load_pcm(&mut self, sample: SampleId, _pcm: &Pcm) -> Result<(), AudioOutError> {
        if !self.loaded.contains(&sample) {
            self.loaded.push(sample);
        }
        Ok(())
    }

    fn load_encoded(&mut self, _sample: SampleId, _bytes: &[u8]) -> Result<(), AudioOutError> {
        Err(AudioOutError::Unsupported("mp3/ogg"))
    }

    fn is_loaded(&self, sample: SampleId) -> bool {
        self.loaded.contains(&sample)
    }

    fn play(&mut self, sample: SampleId, gain: f32, looped: bool) -> Option<VoiceId> {
        if !self.running || !self.loaded.contains(&sample) {
            return None;
        }
        self.next_voice += 1;
        let v = VoiceId(self.next_voice);
        self.played.push((v, sample, gain, looped));
        self.live.push(v);
        Some(v)
    }

    fn play_with(
        &mut self,
        sample: SampleId,
        gain: f32,
        looped: bool,
        params: VoiceParams,
    ) -> Option<VoiceId> {
        let v = self.play(sample, gain, looped)?;
        self.params.push((v, params));
        Some(v)
    }

    fn set_voice_pan(&mut self, voice: VoiceId, pan: f32) {
        self.pan_sets.push((voice, pan));
    }

    fn stop(&mut self, voice: VoiceId) {
        self.stopped.push(voice);
        self.live.retain(|v| *v != voice);
        self.paused.retain(|v| *v != voice);
    }

    fn set_voice_gain(&mut self, voice: VoiceId, gain: f32) {
        self.gain_sets.push((voice, gain));
    }

    fn fade_out(&mut self, voice: VoiceId, secs: f32) {
        self.faded.push((voice, secs));
        self.live.retain(|v| *v != voice);
        self.paused.retain(|v| *v != voice);
    }

    fn set_master_gain(&mut self, gain: f32) {
        self.master = gain;
    }

    fn is_running(&self) -> bool {
        self.running
    }

    fn resume(&mut self) {
        self.running = true;
    }

    fn is_playing(&self, voice: VoiceId) -> bool {
        self.live.contains(&voice)
    }

    fn can_stream(&self) -> bool {
        self.streams_on
    }

    fn play_stream(&mut self, url: &str, gain: f32, looped: bool) -> Option<VoiceId> {
        if !self.running || !self.streams_on {
            return None;
        }
        self.next_voice += 1;
        let v = VoiceId(self.next_voice);
        self.streams.push((v, url.to_owned(), gain, looped));
        self.live.push(v);
        Some(v)
    }

    fn pause(&mut self, voice: VoiceId) {
        if self.live.contains(&voice) && !self.paused.contains(&voice) {
            self.paused.push(voice);
        }
    }

    fn unpause(&mut self, voice: VoiceId) -> bool {
        let was = self.paused.contains(&voice);
        self.paused.retain(|v| *v != voice);
        was && self.live.contains(&voice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_duration() {
        let p = Pcm {
            rate: 22_050,
            channels: 2,
            samples: vec![0; 22_050 * 2],
        };
        assert_eq!((p.frames(), p.duration_ms()), (22_050, 1000));
    }

    #[test]
    fn null_output_records_and_needs_a_gesture() {
        let mut o = NullOutput::default();
        let pcm = Pcm {
            rate: 8000,
            channels: 1,
            samples: vec![0; 8],
        };
        o.load_pcm(SampleId(1), &pcm).unwrap();
        assert_eq!(o.play(SampleId(1), 1.0, false), None, "not running yet");
        o.resume();
        let v = o.play(SampleId(1), 0.5, false).unwrap();
        assert!(o.is_playing(v));
        o.set_voice_gain(v, 0.25);
        assert_eq!(o.gain_sets, [(v, 0.25)]);
        o.stop(v);
        assert!(!o.is_playing(v));
        assert_eq!(o.play(SampleId(2), 1.0, false), None, "not loaded");
        assert!(matches!(
            o.load_encoded(SampleId(3), b"x"),
            Err(AudioOutError::Unsupported(_))
        ));
    }
}
