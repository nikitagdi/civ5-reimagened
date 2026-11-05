//! The engine's part of the 3D sounds (re-b15 ITEM 1 E4, ITEM 4): a script played at a point of the world, heard from
//! the listener with the original's linear falloff between its `fMinDistance` and `fCutoffDistance` and placed in the
//! stereo field by its bearing.

use asset_format::{DEFAULT_CUTOFF_DISTANCE, DEFAULT_MIN_DISTANCE, Sound3DTable, SoundScript};
use platform::AudioOutput;

use super::{Call, PlayOutcome, UiAudio, key};

/// `f3dDistanceMultiplier` of `AudioParams.xml`: world units per tile of the scripts' distances (the port's hex width).
pub const HEX_UNITS: f32 = 64.0;
/// `fCameraZFactor` and the top of the lift (`fMaxCameraZ` 4 tiles) of `AudioParams.xml`.
const CAMERA_Z_FACTOR: f32 = 0.55;
const MAX_LIFT: f32 = 4.0 * HEX_UNITS;

/// Where the sounds are heard from: the ground point on the camera's axis, lifted by the camera's height (re-b15 ITEM 7
/// P7.3, `UpdateListenerPosition`). Coordinates are the renderer's: X east, Y up, Z south.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Listener {
    pub pos: [f32; 3],
}

impl Listener {
    /// The listener of a camera looking at `(x, z)` on ground at height `ground`, its eye `height` above it:
    /// `ground + clamp(0.55 * height, 0, 256)`.
    pub fn at(x: f32, z: f32, ground: f32, height: f32) -> Listener {
        let lift = if height.is_finite() {
            (CAMERA_Z_FACTOR * height).clamp(0.0, MAX_LIFT)
        } else {
            0.0
        };
        Listener {
            pos: [x, ground + lift, z],
        }
    }

    /// The level (0..=1) and the stereo position (-1 left .. 1 right) of a sound at `at` whose script is whole up to
    /// `min` and silent from `cutoff` (world units); `None` beyond the cutoff (the exe never starts such a sound,
    /// `IsSoundCloseEnoughToBeHeard`).
    ///
    /// SIMPLIFIED(SIMP-exe-1200): the falloff is the linear one `AudioParams.xml` names for Civ5
    /// (`iFalloffFunctionIndex` 1); the stereo position is the cosine of the angle between the sound and the
    /// listener's right (the camera looks north, so east is right), not Miles' speaker levels (`fSpeakerFalloffPower`
    /// 5, not traced).
    pub fn hear(&self, at: [f32; 3], min: f32, cutoff: f32) -> Option<(f32, f32)> {
        let d = [
            at[0] - self.pos[0],
            at[1] - self.pos[1],
            at[2] - self.pos[2],
        ];
        let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        if !dist.is_finite() {
            return None;
        }
        let level = if dist <= min {
            1.0
        } else if dist >= cutoff {
            return None;
        } else {
            1.0 - (dist - min) / (cutoff - min)
        };
        // (the exe drops a level below 0.001 at the start)
        if level < 0.001 {
            return None;
        }
        let pan = if dist > f32::EPSILON {
            (d[0] / dist).clamp(-1.0, 1.0)
        } else {
            0.0
        };
        Some((level, pan))
    }
}

impl<O: AudioOutput> UiAudio<O> {
    /// Takes the 3D scripts of the unit sound table (`ui:snd3d`) beside the 2D ones, with their distances and pitch
    /// ranges, and the pitch ranges of the 2D scripts. Their sounds are in the 2D table's definitions.
    pub fn attach_3d(&mut self, table: &Sound3DTable) {
        for s in &table.scripts {
            let mut sc = SoundScript::new(&s.name, &s.sound, s.kind);
            sc.min_volume = s.min_volume;
            sc.max_volume = s.max_volume;
            sc.looping = s.looping;
            sc.dont_trigger_duplicates = s.dont_trigger_duplicates;
            sc.max_concurrent = s.max_concurrent;
            sc.min_delay_ms = s.min_delay_ms;
            sc.max_delay_ms = s.max_delay_ms;
            sc.min_gap_ms = s.min_gap_ms;
            sc.max_gap_ms = s.max_gap_ms;
            sc.chance_percent = s.chance_percent;
            let k = key(&s.name);
            self.scripts.insert(k.clone(), sc);
            self.positional
                .insert(k, (s.min_distance, s.cutoff_distance));
            self.set_pitch_range(&s.name, s.pitch_down, s.pitch_up);
        }
        for p in &table.pitch_2d {
            self.set_pitch_range(&p.script, p.down, p.up);
        }
    }

    /// Whether a script loops (`bLooping`).
    pub fn is_looping(&self, script: &str) -> bool {
        self.scripts.get(&key(script)).is_some_and(|s| s.looping)
    }

    /// Plays a 3D script at the world point `at` (X east, Y up, Z south) as `listener` hears it: the level of its
    /// distance scales its volume, its bearing places it left or right. A sound out of reach is not started; one that
    /// comes before the first gesture is dropped (a step is of no use later).
    pub fn play_3d(
        &mut self,
        now_ms: u64,
        script: &str,
        at: [f32; 3],
        listener: &Listener,
    ) -> PlayOutcome {
        let k = key(script);
        let (min, cutoff) = self
            .positional
            .get(&k)
            .copied()
            .unwrap_or((DEFAULT_MIN_DISTANCE, DEFAULT_CUTOFF_DISTANCE));
        let Some((level, pan)) = listener.hear(at, min * HEX_UNITS, cutoff * HEX_UNITS) else {
            return PlayOutcome::OutOfRange;
        };
        if !self.out.is_running() {
            return PlayOutcome::WaitingForGesture;
        }
        self.play_2d_inner(
            now_ms,
            script,
            Call {
                scale: level,
                pan,
                ..Call::PLAIN
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use asset_format::{AudioTable, PitchRange, Script3D, SoundDef, SoundKind, SoundLoad};
    use platform::NullOutput;

    use super::*;
    use crate::engine::MAX_SAME_SOUNDS;
    use crate::wav::tests::pcm16;

    fn engine() -> UiAudio<NullOutput> {
        let def = |id: &str, files: &[&str]| SoundDef {
            id: id.into(),
            filename: id.into(),
            load: SoundLoad::DynamicRes,
            one_variation: false,
            files: files.iter().map(|f| (*f).to_owned()).collect(),
        };
        let table = AudioTable {
            scripts: vec![SoundScript::new("AS2D_P", "SND_V", SoundKind::Sfx)],
            sounds: vec![
                def("SND_FOOT", &["ui:au:foot.wav"]),
                def(
                    "SND_V",
                    &[
                        "ui:au:v0.wav",
                        "ui:au:v1.wav",
                        "ui:au:v2.wav",
                        "ui:au:v3.wav",
                    ],
                ),
            ],
            links: vec![],
        };
        let mut a = UiAudio::new(table, NullOutput::running(), 11);
        let mut foot = Script3D::new("AS3D_FOOT", "SND_FOOT");
        foot.pitch_down = -4;
        foot.pitch_up = 3;
        foot.min_volume = 50;
        foot.max_volume = 50;
        a.attach_3d(&Sound3DTable {
            scripts: vec![foot],
            models: vec![],
            pitch_2d: vec![PitchRange {
                script: "AS2D_P".into(),
                down: -2,
                up: 2,
            }],
        });
        a.preload(0, "AS3D_FOOT");
        a.preload(0, "AS2D_P");
        let (_, ahead) = a.take_wanted();
        for f in ahead {
            a.on_file_loaded(0, &f, &pcm16(8000, &[0; 80])).unwrap();
        }
        a
    }

    #[test]
    fn the_listener_is_lifted_by_the_camera_height_up_to_four_tiles() {
        assert_eq!(Listener::at(0.0, 0.0, 10.0, 100.0).pos[1], 10.0 + 55.0);
        assert_eq!(Listener::at(0.0, 0.0, 0.0, 1000.0).pos[1], 256.0);
        assert_eq!(Listener::at(0.0, 0.0, 0.0, -5.0).pos[1], 0.0);
    }

    #[test]
    fn a_sound_falls_off_linearly_between_its_distances_and_is_cut_beyond() {
        let l = Listener { pos: [0.0; 3] };
        assert_eq!(l.hear([50.0, 0.0, 0.0], 96.0, 384.0), Some((1.0, 1.0)));
        let (half, pan) = l.hear([-240.0, 0.0, 0.0], 96.0, 384.0).unwrap();
        assert!((half - 0.5).abs() < 1e-6 && pan == -1.0, "{half} {pan}");
        assert_eq!(l.hear([0.0, 0.0, 384.0], 96.0, 384.0), None);
        // Straight ahead (north) is in the middle.
        assert_eq!(l.hear([0.0, 0.0, -100.0], 96.0, 384.0).unwrap().1, 0.0);
    }

    #[test]
    fn a_3d_script_plays_at_the_level_of_its_distance_and_its_bearing() {
        let mut a = engine();
        let l = Listener { pos: [0.0; 3] };
        // 1.5 tiles whole, 6 silent: 3.75 tiles is half way.
        let o = a.play_3d(10, "AS3D_FOOT", [3.75 * HEX_UNITS, 0.0, 0.0], &l);
        assert!(matches!(o, PlayOutcome::Started(_)), "{o:?}");
        let (_, _, gain, _) = a.output().played[0];
        assert!(
            (gain - 0.25).abs() < 1e-5,
            "0.5 of the script times 0.5 heard: {gain}"
        );
        assert_eq!(a.output().params[0].1.pan, 1.0, "to the east is right");
        assert_eq!(
            a.play_3d(20, "AS3D_FOOT", [7.0 * HEX_UNITS, 0.0, 0.0], &l),
            PlayOutcome::OutOfRange
        );
    }

    #[test]
    fn every_play_draws_a_pitch_in_the_range_of_the_script() {
        let mut a = engine();
        let l = Listener { pos: [0.0; 3] };
        let mut rates = std::collections::BTreeSet::new();
        for t in 0..200u64 {
            a.output_mut().live.clear();
            a.play_3d(t, "AS3D_FOOT", [0.0; 3], &l);
            a.play_2d(t, "AS2D_P");
        }
        for (_, p) in &a.output().params {
            let n = (p.rate.log2() * 12.0).round() as i32;
            assert!((-4..=3).contains(&n), "{n}");
            assert!((p.rate - 2f32.powf(n as f32 / 12.0)).abs() < 1e-5);
            rates.insert(n);
        }
        assert_eq!(rates.len(), 8, "every step of -4..=3 is reached: {rates:?}");
    }

    #[test]
    fn variations_come_from_a_shuffle_bag_each_once_per_round() {
        let mut a = engine();
        for t in 0..12u64 {
            a.output_mut().live.clear();
            assert!(matches!(a.play_2d(t, "AS2D_P"), PlayOutcome::Started(_)));
        }
        let samples: Vec<u32> = a.output().played.iter().map(|p| p.1.0).collect();
        for round in samples.chunks(4) {
            let mut r = round.to_vec();
            r.sort_unstable();
            r.dedup();
            assert_eq!(r.len(), 4, "each of the four once in a round: {samples:?}");
        }
    }

    #[test]
    fn no_more_than_ten_copies_of_a_script_sound_at_once() {
        let mut a = engine();
        let l = Listener { pos: [0.0; 3] };
        for t in 0..MAX_SAME_SOUNDS as u64 {
            assert!(matches!(
                a.play_3d(t, "AS3D_FOOT", [0.0; 3], &l),
                PlayOutcome::Started(_)
            ));
        }
        assert_eq!(
            a.play_3d(50, "AS3D_FOOT", [0.0; 3], &l),
            PlayOutcome::Suppressed(crate::Suppressed::Limit)
        );
        // One ends: the next is let through.
        let v = a.output().live[0];
        a.stop(v);
        assert!(matches!(
            a.play_3d(60, "AS3D_FOOT", [0.0; 3], &l),
            PlayOutcome::Started(_)
        ));
    }

    #[test]
    fn a_voice_starts_after_its_delay_and_counts_as_sounding_while_it_waits() {
        let mut a = engine();
        assert_eq!(a.play_2d_after(0, "AS2D_P", 1000), PlayOutcome::Delayed);
        assert!(a.is_sounding("as2d_p"), "waiting for its second");
        a.tick(900);
        assert!(a.output().played.is_empty());
        a.tick(1000);
        assert_eq!(a.output().played.len(), 1);
        assert!(a.is_sounding("AS2D_P"));
        let v = a.output().live[0];
        a.stop(v);
        assert!(!a.is_sounding("AS2D_P"));
    }

    #[test]
    fn a_3d_sound_before_the_gesture_is_dropped_not_kept() {
        let mut a = engine();
        a.output_mut().running = false;
        let l = Listener { pos: [0.0; 3] };
        assert_eq!(
            a.play_3d(0, "AS3D_FOOT", [0.0; 3], &l),
            PlayOutcome::WaitingForGesture
        );
        a.unlock(10);
        assert!(a.output().played.is_empty());
        assert!(a.has_script("as3d_foot") && !a.has_script("AS3D_NOPE"));
    }
}
