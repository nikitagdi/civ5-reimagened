// Fragment of the class mask pass: writes the number of the class of what
// the call draws, 1 land ... 10 particles (`frame::mask::class`). The class is the value of the
// pipeline-overridable constant, so one text gives a pipeline per class. The vertex stage is the
// subsystem's own (the same vertex shader as in its normal pass).
//
// The bits 0x40 (fog clouds) and 0x80 (overlay) are set by `world-overlays` from `FowMask` and
// `ScreenOverlay` with their own fragment functions.

override MASK_CLASS: u32 = 0u;

@fragment
fn fs_mask() -> @location(0) u32 {
    return MASK_CLASS;
}
