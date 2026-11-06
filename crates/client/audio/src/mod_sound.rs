//! Sounds of mods (A2): WAV files of a mod's asset pack, decoded once and played through the platform's output.
//!
//! The engine decodes WAV only (`wav::decode`: PCM 8 and 16 bit, IMA ADPCM); a pack carries WAV. Each sound is
//! keyed by a string the caller chooses (the mod and the sound id); the first play decodes and loads it under a
//! `SampleId` of its own range (far from the ids the UI engine uses), the next plays reuse the sample. A file that
//! does not decode is remembered as bad and not tried again.

use std::collections::{BTreeMap, BTreeSet};

use platform::{AudioOutput, SampleId, VoiceId, VoiceParams};

use crate::wav;

/// The first sample id of mod sounds.
pub const FIRST_SAMPLE: u32 = 0x4000_0000;

/// Why a mod sound did not play.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModSoundError {
    #[error("the sound is not a WAV the engine can read: {0}")]
    Decode(String),
    #[error("the sound output refused it: {0}")]
    Output(String),
}

/// The decoded sounds of the mods and the ids they were loaded under.
#[derive(Debug, Default)]
pub struct ModSounds {
    samples: BTreeMap<String, SampleId>,
    bad: BTreeSet<String>,
    next: u32,
}

impl ModSounds {
    pub fn new() -> Self {
        Self::default()
    }

    /// How many sounds are loaded.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Plays the sound `key` whose file is `bytes` at `gain` (linear) and playback `rate`. `Ok(None)`: the output did not
    /// start a voice (it is not running yet: a browser starts after the first gesture). A file that failed
    /// before answers the same error without decoding again.
    pub fn play(
        &mut self,
        out: &mut dyn AudioOutput,
        key: &str,
        bytes: &[u8],
        gain: f32,
        rate: f32,
    ) -> Result<Option<VoiceId>, ModSoundError> {
        if self.bad.contains(key) {
            return Err(ModSoundError::Decode("failed before".into()));
        }
        let sample = match self.samples.get(key) {
            Some(s) => *s,
            None => {
                let pcm = wav::decode(bytes).map_err(|e| {
                    self.bad.insert(key.to_owned());
                    ModSoundError::Decode(e.to_string())
                })?;
                let id = SampleId(FIRST_SAMPLE + self.next);
                out.load_pcm(id, &pcm).map_err(|e| {
                    self.bad.insert(key.to_owned());
                    ModSoundError::Output(e.to_string())
                })?;
                self.next += 1;
                self.samples.insert(key.to_owned(), id);
                id
            }
        };
        Ok(out.play_with(
            sample,
            gain,
            false,
            VoiceParams {
                rate: rate.max(0.01),
                pan: 0.0,
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform::NullOutput;

    fn tone() -> Vec<u8> {
        crate::wav::tests::pcm16(22_050, &[0, 1000, -1000, 500])
    }

    #[test]
    fn a_sound_is_decoded_once_and_played_with_its_gain_and_rate() {
        let mut out = NullOutput::running();
        let mut s = ModSounds::new();
        let v = s.play(&mut out, "1:ping", &tone(), 0.5, 1.5).unwrap();
        assert!(v.is_some());
        assert_eq!(out.loaded, [SampleId(FIRST_SAMPLE)]);
        s.play(&mut out, "1:ping", &tone(), 1.0, 1.0).unwrap();
        s.play(&mut out, "1:pong", &tone(), 1.0, 1.0).unwrap();
        assert_eq!(
            out.loaded,
            [SampleId(FIRST_SAMPLE), SampleId(FIRST_SAMPLE + 1)]
        );
        assert_eq!(out.played.len(), 3);
        assert_eq!(out.played[0].2, 0.5);
        assert_eq!(out.params[0].1.rate, 1.5);
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn a_sound_the_output_is_not_ready_for_is_loaded_and_plays_later() {
        let mut out = NullOutput::default(); // not running
        let mut s = ModSounds::new();
        assert_eq!(s.play(&mut out, "1:ping", &tone(), 1.0, 1.0), Ok(None));
        out.running = true;
        assert!(
            s.play(&mut out, "1:ping", &tone(), 1.0, 1.0)
                .unwrap()
                .is_some()
        );
        assert_eq!(out.loaded.len(), 1, "not decoded again");
    }

    #[test]
    fn a_file_that_is_not_wav_fails_once_and_stays_failed() {
        let mut out = NullOutput::running();
        let mut s = ModSounds::new();
        let e = s
            .play(&mut out, "1:bad", b"OggS....", 1.0, 1.0)
            .unwrap_err();
        assert!(matches!(e, ModSoundError::Decode(_)));
        // Even a good file under the same key is not tried: the key names the pack's file, which does not change.
        assert!(s.play(&mut out, "1:bad", &tone(), 1.0, 1.0).is_err());
        assert!(out.loaded.is_empty());
        assert!(s.is_empty());
    }
}
