//! Sound output on the desktop. SIMPLIFIED(SIMP-uiau-4): no device output yet; the proposal is
//! `cpal` for output (and `symphonia` for mp3/ogg speech), which needs the owner's approval
//!. Until then the recording output of `platform` stands in.

pub use platform::NullOutput as NativeAudio;
