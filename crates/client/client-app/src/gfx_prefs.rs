//! The graphics choices of the player that are not keys of the original's ini: the render scale, the
//! shadow quality and the multisampling. They are choices of the device, not of the account: they live
//! in the device's own store (`localStorage` in the browser, a file on the desktop) under `civ5.gfx.*`,
//! which `account_sync` does not follow. A phone's low settings must not come to the PC through the
//! account. Nothing stored means the default of the device: a touch device starts on the Low preset
//! (60%, small shadows, no multisampling), a desktop on High.
//!
//! The values of the earlier version lived under `civ5.options.RenderScale` and `ShadowQuality`, which
//! the account carried to every device. They are not read: a device with no value of its own takes its
//! default, never what the account had.
//!
//! SIMPLIFIED(SIMP-exe-1110): the controls are in the interim Settings window, not in the original's
//! Options menu.

use platform::KeyValueStore;
use render::gfx_choice::{GfxPreset, GfxValues, Msaa};
use render::render_scale;
use render::shadow::ShadowQuality;
use render::terrain_detail::TerrainDetail;

/// The keys. The prefix is not `civ5.options.`, so the account does not carry them.
pub const RENDER_SCALE_KEY: &str = "civ5.gfx.RenderScale";
pub const SHADOW_QUALITY_KEY: &str = "civ5.gfx.ShadowQuality";
pub const MSAA_KEY: &str = "civ5.gfx.Msaa";
pub const TERRAIN_DETAIL_KEY: &str = "civ5.gfx.TerrainDetail";

/// What the renderer is told.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GfxPrefs {
    pub render_scale: f32,
    pub shadow_quality: ShadowQuality,
    pub msaa: Msaa,
    pub terrain_detail: TerrainDetail,
}

impl GfxPrefs {
    /// The stored choices; what is missing or not a number is the value of the default preset of
    /// the device.
    pub fn read(kv: &dyn KeyValueStore, touch: bool) -> Self {
        let number = |key: &str| kv.get(key).and_then(|s| s.trim().parse::<u32>().ok());
        let default = GfxPreset::default_for(touch).values();
        Self {
            render_scale: number(RENDER_SCALE_KEY)
                .map_or(default.render_scale, render_scale::percent_to_scale),
            shadow_quality: number(SHADOW_QUALITY_KEY)
                .and_then(ShadowQuality::from_index)
                .unwrap_or(default.shadow_quality),
            msaa: number(MSAA_KEY).map_or(default.msaa, Msaa::from_samples),
            terrain_detail: number(TERRAIN_DETAIL_KEY)
                .and_then(TerrainDetail::from_index)
                .unwrap_or(default.terrain_detail),
        }
    }

    pub fn values(&self) -> GfxValues {
        GfxValues {
            render_scale: self.render_scale,
            shadow_quality: self.shadow_quality,
            msaa: self.msaa,
            terrain_detail: self.terrain_detail,
        }
    }

    /// The preset these choices are, if they are one; `None` is "Custom".
    pub fn preset(&self) -> Option<GfxPreset> {
        GfxPreset::of(&self.values())
    }
}

/// Keeps the render scale (any value; snapped to the steps).
pub fn store_render_scale(kv: &dyn KeyValueStore, scale: f32) {
    let pct = render_scale::scale_to_percent(render_scale::snap(scale));
    kv.set(RENDER_SCALE_KEY, &pct.to_string());
}

pub fn store_shadow_quality(kv: &dyn KeyValueStore, quality: ShadowQuality) {
    kv.set(SHADOW_QUALITY_KEY, &quality.index().to_string());
}

pub fn store_msaa(kv: &dyn KeyValueStore, msaa: Msaa) {
    kv.set(MSAA_KEY, &msaa.samples().to_string());
}

pub fn store_terrain_detail(kv: &dyn KeyValueStore, detail: TerrainDetail) {
    kv.set(TERRAIN_DETAIL_KEY, &detail.index().to_string());
}

/// Keeps the choices of a preset together.
pub fn store_preset(kv: &dyn KeyValueStore, preset: GfxPreset) {
    let v = preset.values();
    store_terrain_detail(kv, v.terrain_detail);
    store_render_scale(kv, v.render_scale);
    store_shadow_quality(kv, v.shadow_quality);
    store_msaa(kv, v.msaa);
}

// SIMPLIFIED(SIMP-exe-1111): the user agent, not a pointer media query.
/// Whether the page runs on a phone or a tablet: the user agent says so, or it is an iPad that
/// calls itself a Mac (several touch points). A laptop with a touch screen is not one.
pub fn is_touch_platform(user_agent: &str, max_touch_points: u32) -> bool {
    let mobile = ["Android", "iPhone", "iPad", "iPod", "Mobile"]
        .iter()
        .any(|m| user_agent.contains(m));
    mobile || (user_agent.contains("Macintosh") && max_touch_points > 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use platform::MemoryStore;

    #[test]
    fn nothing_stored_is_the_default_preset_of_the_device() {
        let kv = MemoryStore::new();
        let desktop = GfxPrefs::read(&kv, false);
        assert_eq!(
            desktop,
            GfxPrefs {
                render_scale: 1.0,
                shadow_quality: ShadowQuality::High,
                msaa: Msaa::X4,
                terrain_detail: TerrainDetail::High
            }
        );
        assert_eq!(desktop.preset(), Some(GfxPreset::High));
        let phone = GfxPrefs::read(&kv, true);
        assert_eq!(
            phone,
            GfxPrefs {
                render_scale: 0.6,
                shadow_quality: ShadowQuality::Low,
                msaa: Msaa::Off,
                terrain_detail: TerrainDetail::Low
            }
        );
        assert_eq!(phone.preset(), Some(GfxPreset::Low));
    }

    #[test]
    fn a_choice_is_kept_and_wins_over_the_default() {
        let kv = MemoryStore::new();
        store_render_scale(&kv, 0.6);
        store_shadow_quality(&kv, ShadowQuality::Off);
        store_msaa(&kv, Msaa::X4);
        assert_eq!(kv.get(RENDER_SCALE_KEY).as_deref(), Some("60"));
        assert_eq!(kv.get(SHADOW_QUALITY_KEY).as_deref(), Some("0"));
        assert_eq!(kv.get(MSAA_KEY).as_deref(), Some("4"));
        for touch in [false, true] {
            let p = GfxPrefs::read(&kv, touch);
            assert_eq!(
                p,
                GfxPrefs {
                    render_scale: 0.6,
                    shadow_quality: ShadowQuality::Off,
                    msaa: Msaa::X4,
                    terrain_detail: TerrainDetail::from_index(
                        GfxPreset::default_for(touch)
                            .values()
                            .terrain_detail
                            .index()
                    )
                    .unwrap()
                }
            );
            assert_eq!(p.preset(), None, "a mix is Custom");
        }
    }

    #[test]
    fn bad_stored_values_fall_back_or_snap() {
        let kv = MemoryStore::new();
        kv.set(RENDER_SCALE_KEY, "lots");
        kv.set(SHADOW_QUALITY_KEY, "9");
        kv.set(MSAA_KEY, "x");
        let p = GfxPrefs::read(&kv, true);
        assert_eq!(p.render_scale, 0.6);
        assert_eq!(p.shadow_quality, ShadowQuality::Low);
        assert_eq!(p.msaa, Msaa::Off);
        kv.set(RENDER_SCALE_KEY, "12");
        assert_eq!(GfxPrefs::read(&kv, false).render_scale, 0.5);
        kv.set(RENDER_SCALE_KEY, "400");
        assert_eq!(GfxPrefs::read(&kv, false).render_scale, 1.0);
    }

    #[test]
    fn a_preset_stores_its_three_choices_and_any_single_change_makes_it_custom() {
        let kv = MemoryStore::new();
        for preset in GfxPreset::ALL {
            store_preset(&kv, preset);
            assert_eq!(GfxPrefs::read(&kv, false).preset(), Some(preset));
        }
        store_preset(&kv, GfxPreset::High);
        store_render_scale(&kv, 0.95);
        assert_eq!(GfxPrefs::read(&kv, false).preset(), None);
        store_preset(&kv, GfxPreset::Medium);
        store_msaa(&kv, Msaa::X4);
        assert_eq!(GfxPrefs::read(&kv, false).preset(), None);
        store_preset(&kv, GfxPreset::Low);
        store_shadow_quality(&kv, ShadowQuality::Medium);
        assert_eq!(GfxPrefs::read(&kv, false).preset(), None);
    }

    #[test]
    fn the_terrain_detail_is_kept_on_the_device_and_alone_it_makes_the_preset_custom() {
        let kv = MemoryStore::new();
        store_preset(&kv, GfxPreset::High);
        store_terrain_detail(&kv, TerrainDetail::Low);
        let p = GfxPrefs::read(&kv, false);
        assert_eq!(p.terrain_detail, TerrainDetail::Low);
        assert_eq!(p.preset(), None);
        assert_eq!(kv.get(TERRAIN_DETAIL_KEY).as_deref(), Some("0"));
        // The account's `civ5.options.TerrainDetail` (an earlier build) is not read.
        let kv = MemoryStore::new();
        kv.set("civ5.options.TerrainDetail", "0");
        assert_eq!(
            GfxPrefs::read(&kv, false).terrain_detail,
            TerrainDetail::High
        );
        store_preset(&kv, GfxPreset::Medium);
        assert_eq!(
            GfxPrefs::read(&kv, false).terrain_detail,
            TerrainDetail::Medium
        );
    }

    #[test]
    fn the_keys_stay_on_the_device_and_the_account_does_not_follow_them() {
        for key in [
            RENDER_SCALE_KEY,
            SHADOW_QUALITY_KEY,
            MSAA_KEY,
            TERRAIN_DETAIL_KEY,
        ] {
            assert!(crate::account_sync::account_key(key).is_none(), "{key}");
        }
    }

    #[test]
    fn what_the_account_carried_of_the_old_keys_is_not_read() {
        // A phone's Low choices came to the PC through `civ5.options.*`: now the PC takes its default.
        let kv = MemoryStore::new();
        kv.set("civ5.options.RenderScale", "50");
        kv.set("civ5.options.ShadowQuality", "1");
        let p = GfxPrefs::read(&kv, false);
        assert_eq!(p.preset(), Some(GfxPreset::High));
        assert_eq!(p.render_scale, 1.0);
    }

    #[test]
    fn phones_and_tablets_are_touch_platforms_and_laptops_are_not() {
        const PIXEL: &str = "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 \
                             (KHTML, like Gecko) Chrome/141.0.0.0 Mobile Safari/537.36";
        const IPHONE: &str =
            "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15";
        const MAC: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15";
        const WINDOWS: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                               (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";
        assert!(is_touch_platform(PIXEL, 5));
        assert!(is_touch_platform(IPHONE, 5));
        // iPadOS asks for the desktop page and shows a Mac with a touch screen.
        assert!(is_touch_platform(MAC, 5));
        assert!(!is_touch_platform(MAC, 0));
        assert!(
            !is_touch_platform(WINDOWS, 10),
            "a touch laptop is a laptop"
        );
    }
}
