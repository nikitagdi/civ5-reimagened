//! The render scale: the 3D scene is drawn at `scale` times the size of the screen and `Present`
//! stretches it over the surface (linear filter); the UI passes draw on the surface itself and stay
//! sharp. No GPU: the limits, the snapping of a value and the size arithmetic, so the shell, the
//! settings window and the targets agree.

// SIMPLIFIED(SIMP-exe-1112): all the targets that follow the screen are scaled together; the original has no
// render scale.
/// The smallest scale the settings offer.
pub const MIN: f32 = 0.5;
/// Full size; also the default of a desktop.
pub const MAX: f32 = 1.0;
/// One notch of the slider.
pub const STEP: f32 = 0.05;
/// The default of a touch device (a phone's pixel ratio is 2.5 to 3).
pub const DEFAULT_TOUCH: f32 = 0.75;

/// A value of the setting made valid: inside [`MIN`]..=[`MAX`], on a multiple of [`STEP`]. Not a
/// number (or infinite) gives full size.
pub fn snap(value: f32) -> f32 {
    if !value.is_finite() {
        return MAX;
    }
    percent_to_scale(scale_to_percent(value.clamp(MIN, MAX)))
}

/// The scale as a whole percent on the step (`0.75` is `75`): what the key-value store keeps.
pub fn scale_to_percent(scale: f32) -> u32 {
    let step = (STEP * 100.0).round() as u32;
    let p = (scale * 100.0).round().max(0.0) as u32;
    ((p + step / 2) / step * step).clamp((MIN * 100.0) as u32, (MAX * 100.0) as u32)
}

/// The scale of a stored percent, made valid.
pub fn percent_to_scale(percent: u32) -> f32 {
    let step = (STEP * 100.0).round() as u32;
    let p = ((percent + step / 2) / step * step).clamp((MIN * 100.0) as u32, (MAX * 100.0) as u32);
    p as f32 / 100.0
}

/// The default for a device: [`DEFAULT_TOUCH`] on a touch device, else full size.
pub fn default_for(touch: bool) -> f32 {
    if touch { DEFAULT_TOUCH } else { MAX }
}

/// Size of the scene for a surface of `screen` pixels: at least one pixel each way, and never more
/// than the screen.
pub fn scene_size(screen: (u32, u32), scale: f32) -> (u32, u32) {
    let scale = snap(scale);
    let f = |v: u32| (((f64::from(v) * f64::from(scale)).round() as u32).max(1)).min(v.max(1));
    (f(screen.0), f(screen.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_snaps_to_the_step_inside_the_limits() {
        assert_eq!(snap(0.75), 0.75);
        assert_eq!(snap(0.77), 0.75);
        assert_eq!(snap(0.78), 0.8);
        assert_eq!(snap(0.1), MIN);
        assert_eq!(snap(3.0), MAX);
        assert_eq!(snap(f32::NAN), MAX);
        assert_eq!(snap(f32::INFINITY), MAX);
    }

    #[test]
    fn percent_and_scale_go_both_ways() {
        for p in (50..=100).step_by(5) {
            assert_eq!(scale_to_percent(percent_to_scale(p)), p);
        }
        assert_eq!(percent_to_scale(0), MIN);
        assert_eq!(percent_to_scale(999), MAX);
        assert_eq!(percent_to_scale(72), 0.7);
    }

    #[test]
    fn a_touch_device_starts_smaller_than_a_desktop() {
        assert_eq!(default_for(false), 1.0);
        assert_eq!(default_for(true), 0.75);
    }

    #[test]
    fn the_scene_is_the_screen_times_the_scale() {
        assert_eq!(scene_size((2400, 1080), 1.0), (2400, 1080));
        assert_eq!(scene_size((2400, 1080), 0.75), (1800, 810));
        assert_eq!(scene_size((2400, 1080), 0.5), (1200, 540));
        // Out-of-range values are snapped first.
        assert_eq!(scene_size((2400, 1080), 0.1), (1200, 540));
        assert_eq!(scene_size((2400, 1080), 9.0), (2400, 1080));
    }

    #[test]
    fn the_scene_keeps_a_pixel_and_never_exceeds_the_screen() {
        assert_eq!(scene_size((1, 1), 0.5), (1, 1));
        assert_eq!(scene_size((3, 1), 0.5), (2, 1));
        assert_eq!(scene_size((0, 0), 0.5), (1, 1));
    }
}
