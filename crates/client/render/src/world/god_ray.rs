//! The god ray: the vertical beam of light over the selected unit (item 31).
//!
//! The exe draws it, not the DLL: `cvMovementPathVisSystem::OnHexHighlight` (0x088c0178) answers the
//! event `UnitHexHighlight` (the Lua `UnitSelectionChanged` of a unit of the local player, and every
//! move of the selected unit) by putting the instance of `FXM_God_Ray.fxsxml` on the hex and starting
//! its clock (re-b10-answers ITEM 3):
//!
//! - the place is the hex's world location with `z = max(z, 10.1167)` (`m_fMinHeight`, 0x4121de22);
//!   the matrix is the identity with that translation: no scale, no turn;
//! - the clock `m_fHighlightTime` restarts at every event, its period `m_fHighlightPeriod` is 1.0 s
//!   (`fScrollTime` of the art define is 1.0 too); the scrolling of the texture is the shader's;
//! - it is shown while a unit is selected and hidden when the selection clears.
//!
//! The model is an effect model of `FXShader` (`FX_LightBeam_shader_01`) and is drawn by the unit draw, after the
//! opaque calls, with the exe's "God Ray" command set (re-b11-answers.txt): the blend of
//! `BlendMode::GodRay` (colour `ONE`/`SRCALPHA`, alpha `ZERO`/`SRCALPHA`), the face culling of the outside of
//! the funnel and no test against the world (the exe clears the depth first), by the pixel shader `FXPS_CODE3`
//! of three layers (`fx.wgsl`, `fx_ps_code3`; squared texture decode, the layers put over each other,
//! `sqrt` on the output). The vertex entry `fx_god_ray_vs` takes the clock from the instance: the time since
//! the highlight event, unwrapped, as in the exe (`fElapsedTimeR` = `fScrollCoordR` = `m_fHighlightTime`).
//!
//! Gamma: the exe writes with `SRGBWRITEENABLE` off, so the `sqrt` of the shader is the gamma encode and the
//! blend runs on gamma values. Our world target is `Rgba8Unorm` and holds the same gamma-space values
//! (`Present` copies it as it is, and linearises only for a surface that encodes), so the shader's output goes
//! into the blend unchanged and the beam is the same number on screen as in the original; nothing is added.
//!
//! This module is the pure part: where the instance stands and since when. An install whose assets lack the
//! model draws nothing (the model is simply never given to the draw).
//!
//! SIMPLIFIED(SIMP-exe-1028): the variant of the pixel shader (`FXPS_CODE3`, from the three layers of the
//! material), the mapping of the two per-frame constants to the exe's dynamic block (by register order) and the
//! sampler states (repeat, linear, mips; the exe's were not decoded) are inferred; the depth clear plus
//! `LEQUAL` write of the exe is `Always` without a write here (the same picture for a convex one-sided
//! funnel), and the beam is also drawn in the reflection pass, which the exe may not do.
//! SIMPLIFIED(SIMP-exe-1029): the hex is the one of the UI's selected unit (the ring's); the clock restarts when
//! it changes or when the selection returns after a clear; there is no fog or zoom culling (the exe has none).
//! The exe draws the beam in the pass of two rects that contains its world position; one pass here.

use asset_format::LogicalId;
use glam::Vec2;

use crate::hexgrid::cell_center;
use crate::static_model::StaticInstance;
use crate::world::objects::instance::ObjectInstance;

/// The id of the model in the asset sets (`b2-misc`, `fxm_god_ray.fxsxml`).
pub const MODEL: &str = "md:fxm_god_ray";

/// The lowest height of the beam's foot: `cvMovementPathVisSystem::m_fMinHeight`.
// re: cvMovementPathVisSystem ctor 0x088bf38e (0x4121de22), clamp in OnHexHighlight 0x088c0178
pub const MIN_HEIGHT: f32 = 10.1167;

/// `m_fHighlightPeriod`: `fScrollCoordR` is the time since the event over this (1.0, so the same number).
// re: cvMovementPathVisSystem ctor 0x088bf38e (1.0)
pub const PERIOD: f32 = 1.0;

/// The id of the model as the draw knows it.
pub fn model() -> LogicalId {
    LogicalId::new(MODEL)
}

/// The height of the foot of the beam on a hex whose ground is `ground` high.
pub fn foot_height(ground: f32) -> f32 {
    ground.max(MIN_HEIGHT)
}

/// The colour bytes of the instance that carry the moment `started` of the event to the vertex shader: the
/// four bytes of the f32, little endian (`fx_god_ray_timing` puts them together again).
pub fn start_bytes(started: f32) -> [u8; 4] {
    started.to_le_bytes()
}

/// `fElapsedTimeR` the shader computes at the time `now` for the bytes of [`start_bytes`]: the seconds since the
/// event, not wrapped (the exe adds the frame time to `m_fHighlightTime` without a limit).
pub fn shader_elapsed(bytes: [u8; 4], now: f32) -> f32 {
    (now - f32::from_le_bytes(bytes)).max(0.0)
}

/// Where the beam stands and since when.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct GodRay {
    cell: Option<(i32, i32)>,
    /// The moment of the last highlight event (the clock `m_fHighlightTime` is 0 then).
    started: f32,
}

impl GodRay {
    /// Follows the selected unit's hex at the time `now`. A new hex (a selection, a move of the unit) is a
    /// highlight event: the clock restarts. The same hex again does nothing; no selection hides the beam.
    pub fn update(&mut self, selected: Option<(i32, i32)>, now: f32) {
        if selected != self.cell {
            self.cell = selected;
            self.started = now;
        }
    }

    /// The hex the beam stands on; `None` while no unit is selected.
    pub fn cell(&self) -> Option<(i32, i32)> {
        self.cell
    }

    /// Seconds since the last highlight event (`m_fHighlightTime`; it grows without a wrap).
    pub fn age(&self, now: f32) -> f32 {
        (now - self.started).max(0.0)
    }

    /// The model the beam needs, while it is shown.
    pub fn wanted(&self) -> Vec<LogicalId> {
        if self.cell.is_some() {
            vec![model()]
        } else {
            Vec::new()
        }
    }

    /// The instance of the beam, `ground` giving the relief's height at `(x, z)` (our basis): the hex's
    /// centre at `max(height, MIN_HEIGHT)`, no turn and no scale, the clock of the shader started at the
    /// last event.
    pub fn instance(&self, ground: &dyn Fn(Vec2) -> f32) -> Option<ObjectInstance> {
        let cell = self.cell?;
        let at = cell_center(cell.0, cell.1);
        let mut o = ObjectInstance::from_static(&StaticInstance::new(
            [at.x, foot_height(ground(at)), at.y],
            0.0,
            1.0,
        ));
        // The moment of the event, for `fx_god_ray_vs` (the alpha byte is part of it; nothing reads it).
        o.color = start_bytes(self.started);
        Some(o)
    }
}

#[cfg(test)]
pub(crate) mod reference {
    // ---- the maths of FXPS_CODE3 (re-b11 3) on the CPU: the reference of the shader ----

    /// The texels one layer reads: the scroll map `s` (UVScrollingMap, unorm), the base at the moved uv,
    /// the alpha map at the uv (`.w`) and at the scroll coordinate (`.x`).
    #[derive(Clone, Copy)]
    pub(crate) struct Layer {
        pub scroll: [f32; 4],
        pub base: [f32; 4],
        pub alpha_uv_w: f32,
        pub alpha_scroll_x: f32,
    }

    /// The velocity of the layer in uv units a second, from its scroll map.
    pub(crate) fn velocity(s: [f32; 4]) -> [f32; 2] {
        let near = |v: f32| (0.498..=0.502).contains(&v);
        let dx = if near(s[0]) { 0.0 } else { 1.0 - 2.0 * s[0] };
        let dy = if near(s[1]) { 0.0 } else { 2.0 * s[1] - 1.0 };
        [s[2] * dx, s[3] * dy]
    }

    /// `(premultiplied colour, transmittance)` of a layer: the base is squared in all four channels, the
    /// alpha is `alpha_scroll.x^2 * alpha_uv.w^2 * base.a^2`.
    pub(crate) fn layer_out(l: &Layer) -> ([f32; 3], f32) {
        let b = l.base.map(|v| v * v);
        let a = (l.alpha_scroll_x * l.alpha_scroll_x) * (l.alpha_uv_w * l.alpha_uv_w) * b[3];
        ([b[0] * a, b[1] * a, b[2] * a], 1.0 - a)
    }

    /// The output of `FXPS_CODE3` for three layers: `sqrt(c2 + t2 * (c1 + t1 * c0))` and `sqrt(t0 t1 t2)`.
    pub(crate) fn shade(layers: &[Layer; 3]) -> [f32; 4] {
        let o = layers.map(|l| layer_out(&l));
        let mut c = [0.0f32; 3];
        for (i, k) in c.iter_mut().enumerate() {
            *k = o[2].0[i] + o[2].1 * (o[1].0[i] + o[1].1 * o[0].0[i]);
        }
        let t = o[0].1 * o[1].1 * o[2].1;
        [c[0].sqrt(), c[1].sqrt(), c[2].sqrt(), t.sqrt()]
    }
}

#[cfg(test)]
mod tests {
    use super::reference::*;
    use super::*;

    fn flat(h: f32) -> impl Fn(Vec2) -> f32 {
        move |_| h
    }

    #[test]
    fn nothing_is_shown_until_a_unit_is_selected() {
        let mut g = GodRay::default();
        assert!(g.cell().is_none() && g.wanted().is_empty());
        assert!(g.instance(&flat(0.0)).is_none());
        g.update(Some((3, 4)), 5.0);
        assert_eq!(g.cell(), Some((3, 4)));
        assert_eq!(g.wanted(), [model()]);
        assert!(g.instance(&flat(0.0)).is_some());
        g.update(None, 6.0);
        assert!(g.cell().is_none() && g.wanted().is_empty());
        assert!(g.instance(&flat(0.0)).is_none());
    }

    #[test]
    fn the_foot_is_the_hex_centre_at_least_10_1167_high_without_scale_or_turn() {
        let mut g = GodRay::default();
        g.update(Some((3, 4)), 0.0);
        let c = cell_center(3, 4);
        // Water level: the clamp lifts the foot.
        let o = g.instance(&flat(0.0)).unwrap();
        let p = o.transform_point([0.0, 0.0, 0.0]);
        assert!((p[0] - c.x).abs() < 1e-4 && (p[2] - c.y).abs() < 1e-4);
        assert!((p[1] - 10.1167).abs() < 1e-4, "{p:?}");
        // A hill stands above the clamp and is followed.
        let o = g.instance(&flat(42.5)).unwrap();
        assert!((o.transform_point([0.0, 0.0, 0.0])[1] - 42.5).abs() < 1e-4);
        // The matrix is the identity with the translation: a point of the model keeps its offsets.
        let q = o.transform_point([7.0, 357.0, -3.0]);
        assert!((q[0] - c.x - 7.0).abs() < 1e-4);
        assert!((q[1] - 42.5 - 357.0).abs() < 1e-3);
        assert!((q[2] - c.y + 3.0).abs() < 1e-4);
        assert_eq!(foot_height(-5.0), MIN_HEIGHT);
        assert_eq!(foot_height(20.0), 20.0);
    }

    #[test]
    fn the_beam_follows_the_unit_to_its_new_hex() {
        let mut g = GodRay::default();
        g.update(Some((3, 4)), 0.0);
        g.update(Some((4, 4)), 0.4);
        let c = cell_center(4, 4);
        let p = g.instance(&flat(20.0)).unwrap().transform_point([0.0; 3]);
        assert!((p[0] - c.x).abs() < 1e-4 && (p[2] - c.y).abs() < 1e-4);
    }

    #[test]
    fn the_clock_restarts_at_every_new_hex_and_not_on_the_same_one() {
        let mut g = GodRay::default();
        g.update(Some((1, 1)), 10.25);
        assert!(g.age(10.25).abs() < 1e-6);
        assert!((g.age(10.75) - 0.5).abs() < 1e-6);
        // The same hex again: no event, the clock runs on and does not wrap (the exe adds the frame time
        // to `m_fHighlightTime` without a limit).
        g.update(Some((1, 1)), 11.5);
        assert!((g.age(11.5) - 1.25).abs() < 1e-5);
        assert!((g.age(70.25) - 60.0).abs() < 1e-3);
        // A move: the clock restarts.
        g.update(Some((2, 1)), 11.5);
        assert!(g.age(11.5).abs() < 1e-6);
        // Deselect and select the same hex again: an event too.
        g.update(None, 12.0);
        g.update(Some((2, 1)), 12.3);
        assert!(g.age(12.3).abs() < 1e-6);
    }

    /// The instance carries the moment of the event as the bytes of the f32; the vertex shader puts them
    /// together again and the elapsed time is the plain difference, for any time of the session.
    #[test]
    fn the_instance_carries_the_start_time_and_the_elapsed_time_is_not_wrapped() {
        let mut g = GodRay::default();
        g.update(Some((0, 0)), 0.25);
        let o = g.instance(&flat(0.0)).unwrap();
        assert_eq!(o.color, 0.25f32.to_le_bytes());
        for started in [0.0f32, 0.25, 1.0, 7.77, 123.456, 86_400.5] {
            let bytes = start_bytes(started);
            assert_eq!(f32::from_le_bytes(bytes), started);
            for dt in [0.0f32, 0.25, 1.0, 2.5, 90.0] {
                let got = shader_elapsed(bytes, started + dt);
                assert!(
                    (got - dt).abs() <= started.max(1.0) * 1e-6 * 4.0,
                    "{started} +{dt}: {got}"
                );
            }
        }
        // Before the event (a clock that stepped back) the elapsed time is 0, never negative.
        assert_eq!(shader_elapsed(start_bytes(5.0), 4.0), 0.0);
    }

    fn unorm(b: [u8; 4]) -> [f32; 4] {
        b.map(|v| f32::from(v) / 255.0)
    }

    /// The scroll maps of the shipped art (constant 4x4 textures) give the velocities of re-b11 3.1.
    #[test]
    fn the_scroll_maps_of_the_art_give_the_velocities_of_re_b11() {
        let uv01 = velocity(unorm([190, 255, 8, 0]));
        assert!(
            (uv01[0] + 0.01537).abs() < 1e-4 && uv01[1] == 0.0,
            "{uv01:?}"
        );
        let uv03 = velocity(unorm([136, 0, 8, 0]));
        assert!(
            (uv03[0] + 0.00209).abs() < 1e-4 && uv03[1] == 0.0,
            "{uv03:?}"
        );
        let uv02 = velocity(unorm([0, 0, 5, 205]));
        assert!(
            (uv02[0] - 0.01961).abs() < 1e-4 && (uv02[1] + 0.80392).abs() < 1e-4,
            "{uv02:?}"
        );
        // The dead zone around 0.5 is no motion.
        assert_eq!(velocity([0.5, 0.5, 1.0, 1.0]), [0.0, 0.0]);
    }

    /// Known texels: the typical texel and the brightest one of re-b11 5 (the numbers there are hand
    /// computed from the means of the textures, so within a few percent), and the exact squares.
    #[test]
    fn the_shader_maths_gives_the_faint_warm_beam_of_re_b11() {
        // alpha_01: R = 126 everywhere, .w at the texel; base_01 / base_02 means.
        let al = 126.0 / 255.0;
        let layer = |base: [u8; 4], w: u8| Layer {
            scroll: [0.0; 4],
            base: unorm(base),
            alpha_uv_w: f32::from(w) / 255.0,
            alpha_scroll_x: al,
        };
        // Typical texel (Aw = 0.33): out.rgb about 0.08, T about 0.99 (sqrt 0.997).
        let typical = shade(&[
            layer([242, 236, 155, 47], 84),
            layer([241, 235, 149, 88], 84),
            layer([241, 235, 149, 88], 84),
        ]);
        assert!((0.05..0.12).contains(&typical[0]), "{typical:?}");
        assert!(
            typical[0] > typical[1] && typical[1] > typical[2] && typical[2] < 0.7 * typical[0],
            "warm: {typical:?}"
        );
        assert!((typical[3] - 0.997).abs() < 0.004, "{typical:?}");
        // Brightest texel (Aw = 1, base_02 alpha 0.67): about 0.5.
        let bright = shade(&[
            layer([242, 236, 155, 47], 253),
            layer([241, 235, 149, 171], 253),
            layer([241, 235, 149, 171], 253),
        ]);
        assert!((0.3..0.6).contains(&bright[0]), "{bright:?}");
        // The exact value of one layer: a = (126/255)^2 * 1 * (alpha * alpha)^2 ... checked by hand.
        let one = Layer {
            scroll: [0.0; 4],
            base: [1.0, 0.5, 0.0, 1.0],
            alpha_uv_w: 1.0,
            alpha_scroll_x: 0.5,
        };
        let (c, t) = layer_out(&one);
        // base squared = (1, 0.25, 0, 1); a = 0.25 * 1 * 1 = 0.25.
        assert!((c[0] - 0.25).abs() < 1e-6 && (c[1] - 0.0625).abs() < 1e-6 && c[2] == 0.0);
        assert!((t - 0.75).abs() < 1e-6);
        // Two empty layers over a full one: the colour and what is left are those of the one.
        let empty = Layer {
            alpha_uv_w: 0.0,
            ..one
        };
        let s = shade(&[one, empty, empty]);
        assert!(
            (s[0] - 0.5).abs() < 1e-6 && (s[3] - 0.75f32.sqrt()).abs() < 1e-6,
            "{s:?}"
        );
        // The layer 2 is over the layer 1 over the layer 0: with the colour of layer 0 only, it is
        // dimmed by the transmittances of both above it.
        let dark = Layer {
            base: [0.0, 0.0, 0.0, 1.0],
            ..one
        };
        let s = shade(&[one, dark, dark]);
        assert!(
            (s[0] - (0.25f32 * 0.75 * 0.75).sqrt()).abs() < 1e-6,
            "{s:?}"
        );
    }
}
