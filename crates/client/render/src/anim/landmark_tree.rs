//! The animation of landmarks: the decision tree of a model (`.ddtxml`,
//! `ModelBlob.state_clips`), the clips of a feature (`ec` 100 idle, 101 while a city works the
//! plot), and what the other things that play in place ask for (wonders, models of the path and of
//! effects: no machine, the steady clip of the state).
//!
//! A row of the tree is: in state `state` play `clip`, `repeat` times, then wait `pause` seconds
//! ([`asset_format::StateClip`]). A model has one row per variant of the state (a herd has four idle
//! clips, `ec` 1000..1003): a landmark takes one of them, by a number from its own target, and
//! keeps it. A row with no repeat and no pause is a loop of its clip ([`Cycle::of`] is `None`).

use asset_format::{ClipKey as RowClip, StateClip};

use super::data::{ClipEntry, ClipTable, EC_IDLE_A, ModelAnim};
use super::{AnimState, LandmarkState};

/// How a row of the tree plays its clip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cycle {
    /// Times the clip plays in a round (at least one).
    pub repeat: u16,
    /// Seconds the last pose is held after the repeats.
    pub pause: f32,
}

impl Cycle {
    /// The cycle of a row; `None` when the row is a plain loop.
    pub fn of(row: &StateClip) -> Option<Self> {
        let pause = if row.pause.is_finite() {
            row.pause.max(0.0)
        } else {
            0.0
        };
        (row.repeat > 1 || pause > 0.0).then_some(Self {
            repeat: row.repeat.max(1),
            pause,
        })
    }

    /// The seconds of the clip that `elapsed` seconds from the start of the first round show: the
    /// clip `repeat` times, then its last pose for `pause` seconds, and again.
    pub fn time(&self, elapsed: f32, duration: f32) -> f32 {
        if duration.is_nan() || duration <= 0.0 || !elapsed.is_finite() {
            return 0.0;
        }
        let play = duration * f32::from(self.repeat);
        let phase = elapsed.rem_euclid(play + self.pause);
        if phase < play {
            phase % duration
        } else {
            duration
        }
    }
}

/// The state of the animation of a landmark: it only asks for the idle action, the rest is the
/// state of the landmark and a worked feature.
pub fn anim_state(landmark: LandmarkState, worked: bool) -> AnimState {
    AnimState {
        action: EC_IDLE_A,
        landmark,
        worked,
    }
}

/// The rows of the tree for `state`, in the order of the file.
pub fn rows_for<'a>(
    model: &'a ModelAnim,
    state: LandmarkState,
) -> impl Iterator<Item = &'a StateClip> + 'a {
    model
        .state_clips
        .iter()
        .filter(move |r| r.state == state.name())
}

fn entry_of<'a>(clips: &'a ClipTable, clip: &RowClip) -> Option<&'a ClipEntry> {
    match clip {
        RowClip::Ec(ec) => clips.by_ec(*ec),
        RowClip::Name(n) => clips.entries().iter().find(|c| &c.name == n),
    }
}

/// The row a landmark plays in `state` and the clip it names: of the rows whose clip the model has,
/// the one that `seed` picks (the same for the same landmark, spread over the landmarks).
pub fn pick_row(
    model: &ModelAnim,
    state: LandmarkState,
    seed: u64,
) -> Option<(&StateClip, &ClipEntry)> {
    let usable: Vec<(&StateClip, &ClipEntry)> = rows_for(model, state)
        .filter_map(|r| Some((r, entry_of(&model.clips, &r.clip)?)))
        .collect();
    if usable.is_empty() {
        return None;
    }
    // The generator of the service mixes the seed again for its phase; here only its low bits.
    let at = ((seed >> 17) ^ seed) as usize % usable.len();
    Some(usable[at])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anim::data::fixtures;
    use asset_format::ClipKey;

    fn row(repeat: u16, pause: f32) -> StateClip {
        StateClip {
            state: "Constructed".into(),
            clip: ClipKey::Ec(1000),
            repeat,
            pause,
        }
    }

    #[test]
    fn a_row_with_no_repeat_and_no_pause_is_a_loop() {
        assert_eq!(Cycle::of(&row(0, 0.0)), None);
        assert_eq!(Cycle::of(&row(1, 0.0)), None);
        assert_eq!(Cycle::of(&row(0, f32::NAN)), None);
        assert_eq!(
            Cycle::of(&row(3, 0.0)),
            Some(Cycle {
                repeat: 3,
                pause: 0.0
            })
        );
        assert_eq!(
            Cycle::of(&row(0, 2.0)),
            Some(Cycle {
                repeat: 1,
                pause: 2.0
            })
        );
    }

    #[test]
    fn the_clip_plays_its_repeats_and_then_holds_the_last_pose() {
        let c = Cycle {
            repeat: 2,
            pause: 3.0,
        };
        // A clip of 1 s: seconds 0..2 are two plays, 2..5 the hold, and the round begins again.
        assert!((c.time(0.25, 1.0) - 0.25).abs() < 1e-6);
        assert!((c.time(1.25, 1.0) - 0.25).abs() < 1e-6);
        assert_eq!(c.time(2.5, 1.0), 1.0);
        assert_eq!(c.time(4.99, 1.0), 1.0);
        assert!((c.time(5.25, 1.0) - 0.25).abs() < 1e-5);
        assert_eq!(
            c.time(-1.0, 1.0),
            1.0,
            "a time before the start wraps into the hold"
        );
        assert_eq!(c.time(1.0, 0.0), 0.0);
    }

    #[test]
    fn the_rows_of_a_state_are_variants_and_the_seed_picks_one() {
        let (m, pack) = fixtures::model("herd", 3);
        let mut model = ModelAnim::new(&m, Some(pack), None).unwrap();
        // The fixture has the clips of ec 1000 and 1001.
        model.state_clips = (1000..=1001)
            .map(|ec| StateClip {
                state: "Constructed".into(),
                clip: ClipKey::Ec(ec),
                repeat: 0,
                pause: 0.0,
            })
            .collect();
        let usable = rows_for(&model, LandmarkState::Constructed)
            .filter(|r| match &r.clip {
                ClipKey::Ec(ec) => model.clips.by_ec(*ec).is_some(),
                ClipKey::Name(_) => false,
            })
            .count();
        assert!(usable >= 1);
        let a = pick_row(&model, LandmarkState::Constructed, 12345).unwrap();
        let b = pick_row(&model, LandmarkState::Constructed, 12345).unwrap();
        assert_eq!(a.0, b.0, "the same seed, the same row");
        assert!(pick_row(&model, LandmarkState::Pillaged, 1).is_none());
    }

    #[test]
    fn a_landmark_asks_for_idle_and_its_state() {
        let s = anim_state(LandmarkState::Pillaged, true);
        assert_eq!(s.action, EC_IDLE_A);
        assert_eq!(s.landmark, LandmarkState::Pillaged);
        assert!(s.worked);
    }
}
