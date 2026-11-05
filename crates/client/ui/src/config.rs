//! Settings of the UI core.

/// What the host gives `UiCore::new`.
#[derive(Debug, Clone)]
pub struct UiConfig {
    /// Size of the UI screen in logical pixels (physical size divided by the scale).
    pub screen_width: i32,
    pub screen_height: i32,
    /// Compare attribute names without case (`Anchorside`, `hidden`).
    /// `FXmlElement::GetFirstAttributeVal` is not recovered, so exact comparison is the default.
    pub attr_case_insensitive: bool,
    /// Physical pixels per UI pixel; the input position is divided by it.
    pub scale: f32,
    /// The game's option SmallUIAssets is on: a context reads <name>_small.xml when the file set has one (the exe's choice of the
    /// layout of the unit panel, the city screen, the tech and text popups; the EUI user of the goldens has it on).
    pub small_assets: bool,
}

impl Default for UiConfig {
    fn default() -> Self {
        UiConfig {
            screen_width: 1920,
            screen_height: 1080,
            attr_case_insensitive: false,
            scale: 1.0,
            small_assets: false,
        }
    }
}

impl UiConfig {
    /// UI scale for a physical height: height / 1080 rounded down to 0.25, at least 1 (AC-F6).
    pub fn scale_for_height(physical_height: f32) -> f32 {
        let s = (physical_height / 1080.0 * 4.0).floor() / 4.0;
        s.max(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_rounds_down_to_quarter_and_floors_at_one() {
        assert_eq!(UiConfig::scale_for_height(720.0), 1.0);
        assert_eq!(UiConfig::scale_for_height(1080.0), 1.0);
        assert_eq!(UiConfig::scale_for_height(1400.0), 1.25);
        assert_eq!(UiConfig::scale_for_height(2160.0), 2.0);
    }
}
