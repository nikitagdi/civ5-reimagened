//! The engine's part of the world soundscapes: looping beds whose gain follows the camera, and sounds played at a
//! share of their volume (`SoundscapeMixer` decides which, this only plays).

use platform::{AudioOutput, VoiceId};

use super::{Call, PlayOutcome, Queued, UiAudio, key};

/// A world ambience bed that sounds (or waits for its file).
#[derive(Debug, Default)]
pub(super) struct Bed {
    pub(super) voice: Option<VoiceId>,
    /// The volume of the script without the share: the gain of the voice is `base * level`.
    pub(super) base: f32,
    /// Where it sounds in the stereo field (-1 left .. 1 right).
    pub(super) pan: f32,
}

/// A bed this far from silent is not worth a voice.
const MIN_LEVEL: f32 = 0.005;

/// A bed that ends fades out over this long (`FAudioManager` `StopSound` with fade 1.0 in the exe).
pub const BED_FADE_OUT_S: f32 = 1.0;

impl<O: AudioOutput> UiAudio<O> {
    /// Plays a script at `scale` (0..=1) of its volume: a random sound of a world soundscape.
    pub fn play_2d_scaled(&mut self, now_ms: u64, script: &str, scale: f32) -> PlayOutcome {
        self.play_2d_panned(now_ms, script, scale, 0.0)
    }

    /// [`Self::play_2d_scaled`] at a place in the stereo field (-1 left .. 1 right).
    pub fn play_2d_panned(
        &mut self,
        now_ms: u64,
        script: &str,
        scale: f32,
        pan: f32,
    ) -> PlayOutcome {
        self.play_2d_inner(
            now_ms,
            script,
            Call {
                scale: scale.clamp(0.0, 1.0),
                pan: if pan.is_finite() {
                    pan.clamp(-1.0, 1.0)
                } else {
                    0.0
                },
                ..Call::PLAIN
            },
        )
    }

    /// Keeps the looping bed `script` at `level` (0..=1 of its volume): starts it, changes the gain of the voice that
    /// sounds, or ends it when the level is gone. Called again and again (once a second); a call that cannot start the
    /// bed yet (the output waits for a gesture, the file is on its way) is asked again by the next one.
    pub fn set_bed(&mut self, now_ms: u64, script: &str, level: f32) {
        self.set_bed_panned(now_ms, script, level, 0.0);
    }

    /// [`Self::set_bed`] at a place in the stereo field (-1 left .. 1 right) that follows the camera like the level.
    pub fn set_bed_panned(&mut self, now_ms: u64, script: &str, level: f32, pan: f32) {
        let pan = if pan.is_finite() {
            pan.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        let k = key(script);
        if level.is_nan() || level < MIN_LEVEL {
            self.end_bed(script);
            return;
        }
        let level = level.min(1.0);
        if let Some(b) = self.beds.get_mut(&k)
            && let Some(v) = b.voice
        {
            if self.out.is_playing(v) {
                let gain = b.base * level;
                let moved = (b.pan - pan).abs() > 0.01;
                if moved {
                    b.pan = pan;
                }
                // (the ambience slider applies on top: mixer.rs)
                self.set_free_gain(v, gain);
                if moved {
                    self.out.set_voice_pan(v, pan);
                }
                return;
            }
            // (the voice is gone: the output stopped it, e.g. a context that was reset)
            self.beds.remove(&k);
        }
        // A start under way (the file is coming, the delay is running) is not asked twice.
        if self.pending.iter().any(|q| q.script == k && q.bed)
            || self.delayed.iter().any(|q| q.script == k && q.bed)
        {
            return;
        }
        // Nothing waits for the first gesture: the next call finds the output running.
        if !self.out.is_running() {
            return;
        }
        self.beds.entry(k).or_default().pan = pan;
        let call = Call {
            scale: level,
            bed: true,
            pan,
            delay_ms: 0,
        };
        match self.play_2d_inner(now_ms, script, call) {
            PlayOutcome::Started(_) | PlayOutcome::Pending | PlayOutcome::Delayed => {}
            _ => {
                // Nothing sounds or waits: forget it, a later call tries again.
                self.beds.remove(&key(script));
            }
        }
    }

    /// The bed of `script` ends: it dies away over a second (the exe's `StopSound` fade of a soundscape that is left).
    pub fn end_bed(&mut self, script: &str) {
        if self.beds.remove(&key(script)).is_some() {
            self.fade_out_script(script, BED_FADE_OUT_S);
        } else {
            // (one that never started may wait for its file)
            self.pending.retain(|q| !(q.bed && q.script == key(script)));
            self.delayed.retain(|q| !(q.bed && q.script == key(script)));
        }
    }

    /// Every world bed ends (the game is left, the loading screen is up).
    pub fn end_all_beds(&mut self) {
        let names: Vec<String> = self.beds.keys().cloned().collect();
        for n in names {
            self.end_bed(&n);
        }
        self.pending.retain(|q| !q.bed);
        self.delayed.retain(|q| !q.bed);
    }

    /// How many beds sound or wait for their file.
    pub fn bed_count(&self) -> usize {
        self.beds.len()
    }

    /// `start` played a bed's file: its voice is kept.
    pub(super) fn bed_started(&mut self, q: &Queued, v: VoiceId) {
        match self.beds.get_mut(&q.script) {
            Some(b) => {
                b.voice = Some(v);
                b.base = q.gain / q.scale.max(f32::EPSILON);
            }
            // (it ended while the file came)
            None => self.out.stop(v),
        }
    }
}

#[cfg(test)]
mod tests {
    use asset_format::{AudioTable, SoundDef, SoundKind, SoundLoad, SoundScript};
    use platform::{NullOutput, VoiceId};

    use super::*;
    use crate::wav::tests::pcm16;

    fn bed_table() -> AudioTable {
        let mut bed = SoundScript::new("AS2D_BED", "SND_BED", SoundKind::Ambience);
        bed.min_volume = 80;
        bed.max_volume = 80;
        bed.looping = true;
        let owl = SoundScript::new("AS2D_OWL", "SND_OWL", SoundKind::Ambience);
        let def = |id: &str, f: &str| SoundDef {
            id: id.into(),
            filename: id.into(),
            load: SoundLoad::DynamicRes,
            one_variation: false,
            files: vec![f.into()],
        };
        AudioTable {
            scripts: vec![bed, owl],
            sounds: vec![
                def("SND_BED", "ui:au:bed.wav"),
                def("SND_OWL", "ui:au:owl.wav"),
            ],
            links: vec![],
        }
    }

    fn engine() -> UiAudio<NullOutput> {
        UiAudio::new(bed_table(), NullOutput::running(), 5)
    }

    fn feed(a: &mut UiAudio<NullOutput>, now: u64, file: &str) {
        a.on_file_loaded(now, file, &pcm16(8000, &[0; 80])).unwrap();
    }

    #[test]
    fn a_bed_starts_when_its_file_comes_and_follows_the_level() {
        let mut a = engine();
        a.set_bed(0, "AS2D_BED", 0.5);
        assert_eq!(a.take_wanted().0, ["ui:au:bed.wav"]);
        // Asked again while the file is on its way: not queued twice.
        a.set_bed(1000, "as2d_bed", 0.5);
        feed(&mut a, 1200, "ui:au:bed.wav");
        let played = a.output().played.clone();
        assert_eq!(played.len(), 1, "one voice");
        let (v, _, gain, looped) = played[0];
        assert!(looped, "a bed loops");
        assert!(
            (gain - 0.4).abs() < 1e-6,
            "0.8 of the script, half heard: {gain}"
        );
        // The level changes: the voice that sounds changes its gain, no new voice.
        a.set_bed(2200, "AS2D_BED", 0.25);
        assert_eq!(a.output().played.len(), 1);
        let (gv, g) = *a.output().gain_sets.last().unwrap();
        assert_eq!(gv, v);
        assert!((g - 0.2).abs() < 1e-6, "{g}");
        assert_eq!(a.bed_count(), 1);
    }

    #[test]
    fn no_level_or_a_left_game_ends_the_beds() {
        let mut a = engine();
        a.set_bed(0, "AS2D_BED", 1.0);
        feed(&mut a, 10, "ui:au:bed.wav");
        let v = a.output().played[0].0;
        a.set_bed(1000, "AS2D_BED", 0.0);
        // It is not cut off: it dies away over one second.
        assert_eq!(a.output().faded, [(v, BED_FADE_OUT_S)]);
        assert!(a.output().stopped.is_empty());
        assert_eq!(a.bed_count(), 0);
        // It comes back with its own voice.
        a.set_bed(2000, "AS2D_BED", 0.6);
        assert_eq!(a.output().played.len(), 2);
        a.end_all_beds();
        assert_eq!(a.output().faded.len(), 2);
        assert_eq!(a.bed_count(), 0);
        assert!(!a.output().is_playing(VoiceId(2)));
    }

    #[test]
    fn a_bed_that_ends_while_its_file_comes_never_sounds() {
        let mut a = engine();
        a.set_bed(0, "AS2D_BED", 1.0);
        a.end_bed("AS2D_BED");
        feed(&mut a, 10, "ui:au:bed.wav");
        assert!(a.output().played.is_empty());
        assert_eq!(a.bed_count(), 0);
    }

    #[test]
    fn a_bed_waits_for_the_output_and_a_voice_that_died_is_replaced() {
        let mut a = UiAudio::new(bed_table(), NullOutput::default(), 5);
        a.set_bed(0, "AS2D_BED", 1.0);
        assert!(
            a.take_wanted().0.is_empty(),
            "asks for nothing before the gesture"
        );
        assert_eq!(a.bed_count(), 0);
        a.unlock(10);
        a.set_bed(1000, "AS2D_BED", 1.0);
        feed(&mut a, 1010, "ui:au:bed.wav");
        assert_eq!(a.output().played.len(), 1);
        // The output lost the voice by itself.
        a.output_mut().live.clear();
        a.set_bed(2000, "AS2D_BED", 1.0);
        assert_eq!(a.output().played.len(), 2);
    }

    #[test]
    fn a_scaled_sound_plays_at_its_share_of_the_volume() {
        let mut a = engine();
        a.preload(0, "AS2D_OWL");
        feed(&mut a, 1, "ui:au:owl.wav");
        assert!(matches!(
            a.play_2d_scaled(10, "AS2D_OWL", 0.25),
            PlayOutcome::Started(_)
        ));
        assert!((a.output().played[0].2 - 0.25).abs() < 1e-6);
        assert!(!a.output().played[0].3, "a one-shot");
        // More than all of it is all of it.
        a.play_2d_scaled(20, "AS2D_OWL", 7.0);
        assert!((a.output().played[1].2 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn an_unknown_bed_script_leaves_no_trace() {
        let mut a = engine();
        a.set_bed(0, "AS2D_NOPE", 1.0);
        assert_eq!(a.bed_count(), 0);
        assert!(a.take_wanted().0.is_empty());
    }
}
