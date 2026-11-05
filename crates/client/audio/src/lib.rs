//! UI sounds: the 2D sound scripts of the original and the rules they carry,
//! over a [`platform::AudioOutput`]. No platform code and no clock here.

mod button;
mod engine;
pub mod music;
mod notify;
mod soundscape;
mod unit;
mod wav;

pub use button::{ButtonEvent, ButtonFlags, ButtonSoundRules, ControlKind};
pub use engine::{
    AudioError, Category, DUCK_FADE_MS, HEX_UNITS, Listener, MAX_SAME_SOUNDS, MAX_WAIT_MS,
    MIN_BUS_GAIN, PlayOutcome, Suppressed, UNDUCK_FADE_MS, UiAudio,
};
pub use music::{MusicCiv, MusicCmd, MusicDirector, MusicEvent, MusicMode, MusicScene, MusicView};
pub use notify::NotificationQueue;
pub use soundscape::{
    Command as ScapeCommand, FOG_OF_WAR, Heard, HexSound, RIVER, SoundscapeMixer, View as ScapeView,
};
pub use unit::{
    Macros, UnitSoundBook, resolve as resolve_unit_sound, terrain_tag, tier_fits, tier_of,
};
pub use wav::{WavError, decode as decode_wav};
