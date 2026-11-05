//! The sounds the exe plays for the game beyond the tables (re-b15 ITEM 4, ITEM 5): the cues of the units' actions the
//! port does not animate, and the spoken lines of the popups (`cvAudioSimMainGame::OnButtonPopupDisplayShow`).

/// `ec` of the clips the cues of [`UnitClipCue`] stand for (the `Animation` codes of the units' `.fxsxml`).
pub const EC_RUN: u32 = 1400;
pub const EC_ATTACK: u32 = 1100;
pub const EC_ATTACK_CITY: u32 = 1160;
pub const EC_DEATH: u32 = 1200;

/// An action of a unit the exe animates and the port does not: the sound rows of the clip `ec` play for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnitClipCue {
    pub owner: i32,
    pub id: i32,
    pub ec: u32,
    /// The plot it happens on (a move: the plot it went to).
    pub x: i32,
    pub y: i32,
    /// The other side of a fight.
    pub opponent: Option<(i32, i32)>,
}

/// The wait of the era and tech lines after their popup (`+0xa74` = 1.0 s); a wonder's line has none.
pub const VOICE_DELAY_MS: u32 = 1000;

/// The popups of a city-state that speak its greeting instead of their table sound (types `0x24..=0x28` and
/// `0x3d..=0x3f` of the exe, by name).
pub fn is_city_state_popup(name: &str) -> bool {
    matches!(
        name,
        "BUTTONPOPUP_MINOR_GOLD"
            | "BUTTONPOPUP_MINOR_CIV_QUEST"
            | "BUTTONPOPUP_MINOR_CIV_QUEST_COMPLETED"
            | "BUTTONPOPUP_MINOR_CIV_INTRUSION"
            | "BUTTONPOPUP_MINOR_CIV_GREETING_AND_WAR"
            | "BUTTONPOPUP_CITY_STATE_GREETING"
            | "BUTTONPOPUP_CITY_STATE_MESSAGE"
            | "BUTTONPOPUP_CITY_STATE_DIPLO"
    )
}

/// What the spoken line of a popup is read from: the popup's data slot (`iData1..3`) and the delay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceSource {
    /// `Era_NewEraVOs` of the era in `iData1`.
    Era,
    /// `Technologies.AudioIntro` (`AudioIntroHeader` in a multiplayer game) of the tech in `iData3`.
    Tech,
    /// `Buildings.WonderSplashAudio` of the building in `iData1`.
    Wonder,
}

/// The line a popup type speaks, if any.
pub fn voice_source(name: &str) -> Option<(VoiceSource, u32)> {
    match name {
        "BUTTONPOPUP_NEW_ERA" => Some((VoiceSource::Era, VOICE_DELAY_MS)),
        "BUTTONPOPUP_TECH_AWARD" => Some((VoiceSource::Tech, VOICE_DELAY_MS)),
        "BUTTONPOPUP_WONDER_COMPLETED_ACTIVE_PLAYER" => Some((VoiceSource::Wonder, 0)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_era_tech_and_wonder_popups_speak_and_the_city_states_greet() {
        assert_eq!(
            voice_source("BUTTONPOPUP_NEW_ERA"),
            Some((VoiceSource::Era, 1000))
        );
        assert_eq!(
            voice_source("BUTTONPOPUP_TECH_AWARD"),
            Some((VoiceSource::Tech, 1000))
        );
        assert_eq!(
            voice_source("BUTTONPOPUP_WONDER_COMPLETED_ACTIVE_PLAYER"),
            Some((VoiceSource::Wonder, 0))
        );
        assert_eq!(voice_source("BUTTONPOPUP_BARBARIAN_CAMP_REWARD"), None);
        assert!(is_city_state_popup("BUTTONPOPUP_CITY_STATE_GREETING"));
        assert!(is_city_state_popup("BUTTONPOPUP_MINOR_GOLD"));
        assert!(!is_city_state_popup("BUTTONPOPUP_BARBARIAN_RANSOM"));
    }
}
