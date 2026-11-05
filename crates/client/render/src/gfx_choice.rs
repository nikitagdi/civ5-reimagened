//! The graphics choices of the player that come as a set: the multisampling of the main pass and the
//! presets that set the render scale, the shadow quality, the multisampling and the terrain detail
//! together. No GPU: the
//! shell, the settings window and the performance report agree on the numbers here.

use crate::render_scale;
use crate::shadow::ShadowQuality;
use crate::terrain_detail::TerrainDetail;

// SIMPLIFIED(SIMP-exe-1130): no 2x. WebGPU guarantees 1 and 4 samples for the formats of the main pass
// and the pipelines are made for one count (`RenderSettings::effective_msaa`), so the window offers
// Off and 4x only.
/// The multisampling of the main pass the player picks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Msaa {
    Off,
    #[default]
    X4,
}

impl Msaa {
    pub const ALL: [Msaa; 2] = [Self::Off, Self::X4];

    /// The default of a device: off on a touch device, else 4x.
    pub fn default_for(touch: bool) -> Self {
        if touch { Self::Off } else { Self::X4 }
    }

    /// The number the key-value store keeps: the samples (1 or 4).
    pub fn samples(self) -> u32 {
        match self {
            Self::Off => 1,
            Self::X4 => 4,
        }
    }

    /// The choice of a stored number: 0 and 1 are off, every count from 2 up is 4x (what
    /// `RenderSettings::effective_msaa` makes of it).
    pub fn from_samples(samples: u32) -> Self {
        if samples >= 2 { Self::X4 } else { Self::Off }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::X4 => "4x",
        }
    }
}

/// A named set of the three choices. Not stored: the window shows the preset whose values the
/// choices have, and "Custom" when none does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GfxPreset {
    Low,
    Medium,
    High,
}

/// The choices as the renderer takes them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GfxValues {
    pub render_scale: f32,
    pub shadow_quality: ShadowQuality,
    pub msaa: Msaa,
    pub terrain_detail: TerrainDetail,
}

impl GfxPreset {
    pub const ALL: [GfxPreset; 3] = [Self::Low, Self::Medium, Self::High];

    /// The default of a device: `Low` on a touch device, else `High`.
    pub fn default_for(touch: bool) -> Self {
        if touch { Self::Low } else { Self::High }
    }

    pub fn values(self) -> GfxValues {
        let (pct, shadow_quality, msaa, terrain_detail) = match self {
            Self::Low => (60, ShadowQuality::Low, Msaa::Off, TerrainDetail::Low),
            Self::Medium => (80, ShadowQuality::Medium, Msaa::Off, TerrainDetail::Medium),
            Self::High => (100, ShadowQuality::High, Msaa::X4, TerrainDetail::High),
        };
        GfxValues {
            render_scale: render_scale::percent_to_scale(pct),
            shadow_quality,
            msaa,
            terrain_detail,
        }
    }

    /// The preset whose values these are, if there is one.
    pub fn of(values: &GfxValues) -> Option<GfxPreset> {
        Self::ALL.into_iter().find(|p| {
            let v = p.values();
            render_scale::snap(values.render_scale) == v.render_scale
                && values.shadow_quality == v.shadow_quality
                && values.msaa == v.msaa
                && values.terrain_detail == v.terrain_detail
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
        }
    }

    /// 0 for none (custom), else 1 Low, 2 Medium, 3 High: what the performance counter keeps.
    pub fn index_of(preset: Option<GfxPreset>) -> u32 {
        match preset {
            None => 0,
            Some(Self::Low) => 1,
            Some(Self::Medium) => 2,
            Some(Self::High) => 3,
        }
    }

    /// The name of an index of [`Self::index_of`]: `Custom` for 0 and for what is none.
    pub fn name_of_index(index: u32) -> &'static str {
        match index {
            1 => Self::Low.name(),
            2 => Self::Medium.name(),
            3 => Self::High.name(),
            _ => "Custom",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phone_defaults_to_low_and_a_desktop_to_high() {
        assert_eq!(GfxPreset::default_for(true), GfxPreset::Low);
        assert_eq!(GfxPreset::default_for(false), GfxPreset::High);
        let low = GfxPreset::Low.values();
        assert_eq!(
            (low.render_scale, low.shadow_quality, low.msaa),
            (0.6, ShadowQuality::Low, Msaa::Off)
        );
        assert_eq!(Msaa::default_for(true), Msaa::Off);
        assert_eq!(Msaa::default_for(false), Msaa::X4);
        assert_eq!(GfxPreset::High.values().render_scale, 1.0);
    }

    #[test]
    fn the_values_of_a_preset_name_it_and_any_change_makes_it_custom() {
        for p in GfxPreset::ALL {
            assert_eq!(GfxPreset::of(&p.values()), Some(p));
        }
        let mut v = GfxPreset::High.values();
        v.render_scale = 0.95;
        assert_eq!(GfxPreset::of(&v), None);
        let mut v = GfxPreset::Low.values();
        v.shadow_quality = ShadowQuality::Off;
        assert_eq!(GfxPreset::of(&v), None);
        let mut v = GfxPreset::Medium.values();
        v.msaa = Msaa::X4;
        assert_eq!(GfxPreset::of(&v), None);
        let mut v = GfxPreset::High.values();
        v.terrain_detail = TerrainDetail::Medium;
        assert_eq!(
            GfxPreset::of(&v),
            None,
            "the terrain detail alone makes it Custom"
        );
        for p in GfxPreset::ALL {
            let want = match p {
                GfxPreset::Low => TerrainDetail::Low,
                GfxPreset::Medium => TerrainDetail::Medium,
                GfxPreset::High => TerrainDetail::High,
            };
            assert_eq!(p.values().terrain_detail, want);
        }
    }

    #[test]
    fn the_presets_differ_from_each_other_and_the_index_round_trips() {
        let all: Vec<_> = GfxPreset::ALL.iter().map(|p| p.values()).collect();
        assert!(all[0] != all[1] && all[1] != all[2] && all[0] != all[2]);
        for p in GfxPreset::ALL {
            let i = GfxPreset::index_of(Some(p));
            assert_eq!(GfxPreset::name_of_index(i), p.name());
        }
        assert_eq!(
            GfxPreset::name_of_index(GfxPreset::index_of(None)),
            "Custom"
        );
    }

    #[test]
    fn msaa_keeps_its_samples_and_reads_the_old_counts() {
        for m in Msaa::ALL {
            assert_eq!(Msaa::from_samples(m.samples()), m);
        }
        assert_eq!(Msaa::from_samples(0), Msaa::Off);
        assert_eq!(Msaa::from_samples(2), Msaa::X4);
        assert_eq!(Msaa::from_samples(8), Msaa::X4);
    }
}
