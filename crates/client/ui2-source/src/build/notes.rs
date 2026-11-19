//! The root `notes`: the seat's notifications, minus the ones dismissed in the core or here, with a category.
//!
//! The core gives a notification's type as the hash of its `NotificationTypes` name. The category comes from the
//! table below (name -> category), hashed at compile time with the core's own function.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::read::{Notification, ReadError};
use ui2_model::r#gen::{Note, NoteCat, NoteGo, Plot};
use ui2_model::{Key, KeyedList};

use crate::CountingRead;

/// The hash the core gives a type name (`FString::Hash`): CRC-32 (reflected, polynomial `0xEDB88320`, start
/// `0xFFFFFFFF`) without the final inversion.
#[must_use]
pub const fn type_hash(name: &str) -> u32 {
    let b = name.as_bytes();
    let mut crc: u32 = 0xFFFF_FFFF;
    let mut i = 0;
    while i < b.len() {
        crc ^= b[i] as u32;
        let mut k = 0;
        while k < 8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
            k += 1;
        }
        i += 1;
    }
    crc
}

use NoteCat::{Diplomacy, GreatPeople, Growth, Military, Policy, Production, Religion, Research};

// SIMPLIFIED(SIMP-242): the category of a notification is this client's own table of the core's notification
// types; a type not listed here (a new or a mod's type) is shown as "other".
/// Notification types by category. A name not listed is `other`.
pub const CATEGORIES: &[(&str, NoteCat)] = &[
    ("NOTIFICATION_CITY_GROWTH", Growth),
    ("NOTIFICATION_STARVING", Growth),
    ("NOTIFICATION_CITY_TILE", Growth),
    ("NOTIFICATION_BUY_TILE", Growth),
    ("NOTIFICATION_GREAT_PERSON_ACTIVE_PLAYER", GreatPeople),
    ("NOTIFICATION_FREE_GREAT_PERSON", GreatPeople),
    ("NOTIFICATION_FAITH_GREAT_PERSON", GreatPeople),
    ("NOTIFICATION_MAYA_LONG_COUNT", GreatPeople),
    (
        "NOTIFICATION_GREAT_WORK_COMPLETED_ACTIVE_PLAYER",
        GreatPeople,
    ),
    ("NOTIFICATION_MET_MINOR", Diplomacy),
    ("NOTIFICATION_MINOR", Diplomacy),
    ("NOTIFICATION_MINOR_QUEST", Diplomacy),
    ("NOTIFICATION_MINOR_BUYOUT", Diplomacy),
    ("NOTIFICATION_DEMAND_RESOURCE", Diplomacy),
    ("NOTIFICATION_REQUEST_RESOURCE", Diplomacy),
    ("NOTIFICATION_PEACE_ACTIVE_PLAYER", Diplomacy),
    ("NOTIFICATION_PEACE", Diplomacy),
    ("NOTIFICATION_DIPLO_VOTE", Diplomacy),
    ("NOTIFICATION_DIPLOMACY_DECLARATION", Diplomacy),
    ("NOTIFICATION_DEAL_EXPIRED_GPT", Diplomacy),
    ("NOTIFICATION_DEAL_EXPIRED_RESOURCE", Diplomacy),
    ("NOTIFICATION_DEAL_EXPIRED_OPEN_BORDERS", Diplomacy),
    ("NOTIFICATION_DEAL_EXPIRED_DEFENSIVE_PACT", Diplomacy),
    ("NOTIFICATION_DEAL_EXPIRED_RESEARCH_AGREEMENT", Diplomacy),
    ("NOTIFICATION_DEAL_EXPIRED_TRADE_AGREEMENT", Diplomacy),
    ("NOTIFICATION_PLAYER_DEAL", Diplomacy),
    ("NOTIFICATION_PLAYER_DEAL_RECEIVED", Diplomacy),
    ("NOTIFICATION_PLAYER_DEAL_RESOLVED", Diplomacy),
    ("NOTIFICATION_DENUNCIATION_EXPIRED", Diplomacy),
    ("NOTIFICATION_FRIENDSHIP_EXPIRED", Diplomacy),
    ("NOTIFICATION_EXPANSION_PROMISE_EXPIRED", Diplomacy),
    ("NOTIFICATION_BORDER_PROMISE_EXPIRED", Diplomacy),
    ("NOTIFICATION_LIBERATED_MAJOR_CITY", Diplomacy),
    ("NOTIFICATION_RESURRECTED_MAJOR_CIV", Diplomacy),
    ("NOTIFICATION_DIPLOMAT_EJECTED", Diplomacy),
    ("NOTIFICATION_LEAGUE_CALL_FOR_PROPOSALS", Diplomacy),
    ("NOTIFICATION_LEAGUE_CALL_FOR_VOTES", Diplomacy),
    ("NOTIFICATION_LEAGUE_VOTING_DONE", Diplomacy),
    ("NOTIFICATION_LEAGUE_VOTING_SOON", Diplomacy),
    ("NOTIFICATION_SPY_CREATED_ACTIVE_PLAYER", Diplomacy),
    ("NOTIFICATION_SPY_EVICTED", Diplomacy),
    ("NOTIFICATION_SPY_REPLACEMENT", Diplomacy),
    ("NOTIFICATION_SPY_PROMOTION", Diplomacy),
    ("NOTIFICATION_SPY_RIG_ELECTION_SUCCESS", Diplomacy),
    ("NOTIFICATION_SPY_RIG_ELECTION_FAILURE", Diplomacy),
    ("NOTIFICATION_SPY_RIG_ELECTION_ALERT", Diplomacy),
    ("NOTIFICATION_SPY_YOU_STAGE_COUP_SUCCESS", Diplomacy),
    ("NOTIFICATION_SPY_YOU_STAGE_COUP_FAILURE", Diplomacy),
    ("NOTIFICATION_SPY_STAGE_COUP_SUCCESS", Diplomacy),
    ("NOTIFICATION_SPY_STAGE_COUP_FAILURE", Diplomacy),
    ("NOTIFICATION_INTRIGUE_DECEPTION", Diplomacy),
    ("NOTIFICATION_INTRIGUE_CONSTRUCTING_WONDER", Diplomacy),
    ("NOTIFICATION_ENEMY_IN_TERRITORY", Military),
    ("NOTIFICATION_CITY_RANGE_ATTACK", Military),
    ("NOTIFICATION_BARBARIAN", Military),
    ("NOTIFICATION_WAR_ACTIVE_PLAYER", Military),
    ("NOTIFICATION_WAR", Military),
    ("NOTIFICATION_UNIT_DIED", Military),
    ("NOTIFICATION_UNIT_PROMOTION", Military),
    ("NOTIFICATION_CITY_LOST", Military),
    ("NOTIFICATION_CAPITAL_LOST_ACTIVE_PLAYER", Military),
    ("NOTIFICATION_CAPITAL_LOST", Military),
    ("NOTIFICATION_CAPITAL_RECOVERED", Military),
    ("NOTIFICATION_PLAYER_KILLED", Military),
    ("NOTIFICATION_REBELS", Military),
    ("NOTIFICATION_CITY_REVOLT_POSSIBLE", Military),
    ("NOTIFICATION_CITY_REVOLT", Military),
    (
        "NOTIFICATION_INTERNATIONAL_TRADE_UNIT_PLUNDERED_TRADER",
        Military,
    ),
    (
        "NOTIFICATION_INTERNATIONAL_TRADE_UNIT_PLUNDERED_TRADEE",
        Military,
    ),
    ("NOTIFICATION_INTRIGUE_BUILDING_SNEAK_ATTACK_ARMY", Military),
    (
        "NOTIFICATION_INTRIGUE_BUILDING_SNEAK_ATTACK_AMPHIBIOUS",
        Military,
    ),
    (
        "NOTIFICATION_INTRIGUE_SNEAK_ATTACK_ARMY_AGAINST_KNOWN_CITY_UNKNOWN",
        Military,
    ),
    (
        "NOTIFICATION_INTRIGUE_SNEAK_ATTACK_ARMY_AGAINST_KNOWN_CITY_KNOWN",
        Military,
    ),
    (
        "NOTIFICATION_INTRIGUE_SNEAK_ATTACK_ARMY_AGAINST_YOU_CITY_UNKNOWN",
        Military,
    ),
    (
        "NOTIFICATION_INTRIGUE_SNEAK_ATTACK_ARMY_AGAINST_YOU_CITY_KNOWN",
        Military,
    ),
    (
        "NOTIFICATION_INTRIGUE_SNEAK_ATTACK_ARMY_AGAINST_UNKNOWN",
        Military,
    ),
    (
        "NOTIFICATION_INTRIGUE_SNEAK_ATTACK_AMPHIB_AGAINST_KNOWN_CITY_UNKNOWN",
        Military,
    ),
    (
        "NOTIFICATION_INTRIGUE_SNEAK_ATTACK_AMPHIB_AGAINST_KNOWN_CITY_KNOWN",
        Military,
    ),
    (
        "NOTIFICATION_INTRIGUE_SNEAK_ATTACK_AMPHIB_AGAINST_YOU_CITY_UNKNOWN",
        Military,
    ),
    (
        "NOTIFICATION_INTRIGUE_SNEAK_ATTACK_AMPHIB_AGAINST_YOU_CITY_KNOWN",
        Military,
    ),
    (
        "NOTIFICATION_INTRIGUE_SNEAK_ATTACK_AMPHIB_AGAINST_UNKNOWN",
        Military,
    ),
    ("NOTIFICATION_PRODUCTION", Production),
    ("NOTIFICATION_WONDER_COMPLETED_ACTIVE_PLAYER", Production),
    ("NOTIFICATION_WONDER_COMPLETED", Production),
    ("NOTIFICATION_WONDER_BEATEN", Production),
    ("NOTIFICATION_PROJECT_COMPLETED", Production),
    ("NOTIFICATION_TRADE_ROUTE", Production),
    ("NOTIFICATION_TRADE_ROUTE_BROKEN", Production),
    ("NOTIFICATION_TECH", Research),
    ("NOTIFICATION_FREE_TECH", Research),
    ("NOTIFICATION_TECH_AWARD", Research),
    ("NOTIFICATION_SPY_STOLE_TECH", Research),
    ("NOTIFICATION_SPY_CANT_STEAL_TECH", Research),
    ("NOTIFICATION_OTHER_PLAYER_NEW_ERA", Research),
    ("NOTIFICATION_POLICY", Policy),
    ("NOTIFICATION_FREE_POLICY", Policy),
    ("NOTIFICATION_GOLDEN_AGE_BEGUN_ACTIVE_PLAYER", Policy),
    ("NOTIFICATION_GOLDEN_AGE_ENDED_ACTIVE_PLAYER", Policy),
    ("NOTIFICATION_CHOOSE_IDEOLOGY", Policy),
    ("NOTIFICATION_IDEOLOGY_CHOSEN", Policy),
    ("NOTIFICATION_RELIGION_RACE", Religion),
    ("NOTIFICATION_RELIGION_FOUNDED_ACTIVE_PLAYER", Religion),
    ("NOTIFICATION_RELIGION_FOUNDED", Religion),
    ("NOTIFICATION_PANTHEON_FOUNDED_ACTIVE_PLAYER", Religion),
    ("NOTIFICATION_PANTHEON_FOUNDED", Religion),
    ("NOTIFICATION_FOUND_PANTHEON", Religion),
    ("NOTIFICATION_FOUND_RELIGION", Religion),
    ("NOTIFICATION_ENHANCE_RELIGION", Religion),
    ("NOTIFICATION_RELIGION_ENHANCED_ACTIVE_PLAYER", Religion),
    ("NOTIFICATION_RELIGION_ENHANCED", Religion),
    ("NOTIFICATION_ADD_REFORMATION_BELIEF", Religion),
    (
        "NOTIFICATION_REFORMATION_BELIEF_ADDED_ACTIVE_PLAYER",
        Religion,
    ),
    ("NOTIFICATION_REFORMATION_BELIEF_ADDED", Religion),
    ("NOTIFICATION_RELIGION_SPREAD_NATURAL", Religion),
    ("NOTIFICATION_RELIGION_ERROR", Religion),
    ("NOTIFICATION_CAN_BUILD_MISSIONARY", Religion),
    ("NOTIFICATION_AUTOMATIC_FAITH_PURCHASE_STOPPED", Religion),
];

const N: usize = CATEGORIES.len();

/// [`CATEGORIES`] by hash, computed once at compile time.
const HASHED: [(u32, NoteCat); N] = {
    let mut out = [(0_u32, NoteCat::Other); N];
    let mut i = 0;
    while i < N {
        out[i] = (type_hash(CATEGORIES[i].0), CATEGORIES[i].1);
        i += 1;
    }
    out
};

/// The category of a notification type hash; `other` for a type not in [`CATEGORIES`].
#[must_use]
pub fn category(kind: u32) -> NoteCat {
    HASHED
        .iter()
        .find(|(h, _)| *h == kind)
        .map_or(NoteCat::Other, |(_, c)| *c)
}

/// What a click on a notification of each type does: the cases of the core's `CvNotifications::Activate`. A type
/// not listed takes the core's default, a look at its plot.
pub const ACTIONS: &[(&str, NoteGo)] = &[
    ("NOTIFICATION_WONDER_COMPLETED_ACTIVE_PLAYER", NoteGo::Look),
    (
        "NOTIFICATION_GREAT_WORK_COMPLETED_ACTIVE_PLAYER",
        NoteGo::GreatWork,
    ),
    ("NOTIFICATION_BUY_TILE", NoteGo::Capital),
    ("NOTIFICATION_TECH", NoteGo::ChooseTech),
    ("NOTIFICATION_FREE_TECH", NoteGo::ChooseTech),
    ("NOTIFICATION_TECH_AWARD", NoteGo::TechAward),
    ("NOTIFICATION_POLICY", NoteGo::Policies),
    ("NOTIFICATION_FREE_POLICY", NoteGo::Policies),
    ("NOTIFICATION_DIPLO_VOTE", NoteGo::DiploVote),
    ("NOTIFICATION_MINOR_QUEST", NoteGo::CityState),
    ("NOTIFICATION_PRODUCTION", NoteGo::Production),
    ("NOTIFICATION_UNIT_PROMOTION", NoteGo::Unit),
    ("NOTIFICATION_PLAYER_DEAL", NoteGo::Deal),
    ("NOTIFICATION_PLAYER_DEAL_RECEIVED", NoteGo::Deal),
    ("NOTIFICATION_FREE_GREAT_PERSON", NoteGo::FreeGreatPerson),
    ("NOTIFICATION_FOUND_PANTHEON", NoteGo::Pantheon),
    ("NOTIFICATION_ADD_REFORMATION_BELIEF", NoteGo::Reformation),
    ("NOTIFICATION_FOUND_RELIGION", NoteGo::FoundReligion),
    ("NOTIFICATION_ENHANCE_RELIGION", NoteGo::EnhanceReligion),
    ("NOTIFICATION_SPY_CREATED_ACTIVE_PLAYER", NoteGo::Espionage),
    ("NOTIFICATION_SPY_EVICTED", NoteGo::Espionage),
    ("NOTIFICATION_SPY_PROMOTION", NoteGo::Espionage),
    ("NOTIFICATION_SPY_STOLE_TECH", NoteGo::StealTech),
    ("NOTIFICATION_MAYA_LONG_COUNT", NoteGo::MayaBonus),
    ("NOTIFICATION_FAITH_GREAT_PERSON", NoteGo::FaithGreatPerson),
    // SIMPLIFIED(SIMP-448): the core opens a conversation with the AI the note names for these
    // (DoBeginDiploWithHumanEspionageResult, DoBeginDiploWithHumanInDiscuss); a click opens that AI's leader window
    // with its usual greeting instead, without the spy confrontation's answers or the offer to share the intrigue.
    // The wonder above has no server popup to show again, so its click looks at the wonder's city instead of the
    // original's splash.
    ("NOTIFICATION_TECH_STOLEN_SPY_IDENTIFIED", NoteGo::Leader),
    ("NOTIFICATION_SPY_KILLED_A_SPY", NoteGo::Leader),
    ("NOTIFICATION_INTRIGUE_DECEPTION", NoteGo::Leader),
    (
        "NOTIFICATION_INTRIGUE_SNEAK_ATTACK_ARMY_AGAINST_KNOWN_CITY_KNOWN",
        NoteGo::Leader,
    ),
    (
        "NOTIFICATION_INTRIGUE_SNEAK_ATTACK_ARMY_AGAINST_KNOWN_CITY_UNKNOWN",
        NoteGo::Leader,
    ),
    (
        "NOTIFICATION_INTRIGUE_SNEAK_ATTACK_AMPHIB_AGAINST_KNOWN_CITY_UNKNOWN",
        NoteGo::Leader,
    ),
    (
        "NOTIFICATION_INTRIGUE_SNEAK_ATTACK_AMPHIB_AGAINST_KNOWN_CITY_KNOWN",
        NoteGo::Leader,
    ),
    ("NOTIFICATION_LEAGUE_CALL_FOR_PROPOSALS", NoteGo::League),
    ("NOTIFICATION_LEAGUE_CALL_FOR_VOTES", NoteGo::League),
    ("NOTIFICATION_LEAGUE_VOTING_DONE", NoteGo::League),
    ("NOTIFICATION_LEAGUE_VOTING_SOON", NoteGo::League),
    ("NOTIFICATION_IDEOLOGY_CHOSEN", NoteGo::Culture),
    (
        "NOTIFICATION_CULTURE_VICTORY_SOMEONE_INFLUENTIAL",
        NoteGo::Culture,
    ),
    ("NOTIFICATION_CULTURE_VICTORY_WITHIN_TWO", NoteGo::Culture),
    (
        "NOTIFICATION_CULTURE_VICTORY_WITHIN_TWO_ACTIVE_PLAYER",
        NoteGo::Culture,
    ),
    ("NOTIFICATION_CULTURE_VICTORY_WITHIN_ONE", NoteGo::Culture),
    (
        "NOTIFICATION_CULTURE_VICTORY_WITHIN_ONE_ACTIVE_PLAYER",
        NoteGo::Culture,
    ),
    (
        "NOTIFICATION_CULTURE_VICTORY_NO_LONGER_INFLUENTIAL",
        NoteGo::Culture,
    ),
    ("NOTIFICATION_CHOOSE_ARCHAEOLOGY", NoteGo::Archaeology),
    ("NOTIFICATION_CHOOSE_IDEOLOGY", NoteGo::Ideology),
    (
        "NOTIFICATION_LEAGUE_PROJECT_COMPLETE",
        NoteGo::LeagueProject,
    ),
];

/// The types the player may not dismiss (the core's `MayUserDismiss`): they wait for a choice.
// SIMPLIFIED(SIMP-449): a policy notification is never dismissable here; the core lets it go when the game has the
// policy saving option, which this client does not read.
pub const KEPT: &[&str] = &[
    "NOTIFICATION_DIPLO_VOTE",
    "NOTIFICATION_PRODUCTION",
    "NOTIFICATION_TECH",
    "NOTIFICATION_FREE_TECH",
    "NOTIFICATION_FREE_POLICY",
    "NOTIFICATION_FREE_GREAT_PERSON",
    "NOTIFICATION_FOUND_PANTHEON",
    "NOTIFICATION_FOUND_RELIGION",
    "NOTIFICATION_ENHANCE_RELIGION",
    "NOTIFICATION_SPY_STOLE_TECH",
    "NOTIFICATION_MAYA_LONG_COUNT",
    "NOTIFICATION_FAITH_GREAT_PERSON",
    "NOTIFICATION_ADD_REFORMATION_BELIEF",
    "NOTIFICATION_LEAGUE_CALL_FOR_PROPOSALS",
    "NOTIFICATION_CHOOSE_ARCHAEOLOGY",
    "NOTIFICATION_LEAGUE_CALL_FOR_VOTES",
    "NOTIFICATION_CHOOSE_IDEOLOGY",
    "NOTIFICATION_MP_IRR_PROPOSAL",
    "NOTIFICATION_MP_CC_PROPOSAL",
    "NOTIFICATION_MP_SCRAP_PROPOSAL",
    "NOTIFICATION_MP_REMAP_PROPOSAL",
    "NOTIFICATION_POLICY",
];

const NA: usize = ACTIONS.len();
const ACTIONS_HASHED: [(u32, NoteGo); NA] = {
    let mut out = [(0_u32, NoteGo::Look); NA];
    let mut i = 0;
    while i < NA {
        out[i] = (type_hash(ACTIONS[i].0), ACTIONS[i].1);
        i += 1;
    }
    out
};

const NK: usize = KEPT.len();
const KEPT_HASHED: [u32; NK] = {
    let mut out = [0_u32; NK];
    let mut i = 0;
    while i < NK {
        out[i] = type_hash(KEPT[i]);
        i += 1;
    }
    out
};

/// What a click on a notification of this type hash does.
#[must_use]
pub fn action(kind: u32) -> NoteGo {
    ACTIONS_HASHED
        .iter()
        .find(|(h, _)| *h == kind)
        .map_or(NoteGo::Look, |(_, g)| *g)
}

/// Whether the player may dismiss a notification of this type hash.
#[must_use]
pub fn dismissable(kind: u32) -> bool {
    !KEPT_HASHED.contains(&kind)
}

/// `key<note>`: the notification's id.
#[must_use]
pub fn note_key(id: i32) -> Key {
    Key::from_i32(id)
}

/// The notifications to show, in the core's order (oldest first): not dismissed in the core, not dismissed here.
#[must_use]
pub fn rows(notes: &[Notification], dismissed: &BTreeSet<Key>) -> KeyedList<Note> {
    let mut out = KeyedList::new();
    for n in notes {
        let id = note_key(n.id);
        if n.dismissed || dismissed.contains(&id) {
            continue;
        }
        let go = action(n.kind);
        // A deal's note carries the other player as its x (OpenPlayerDealScreen(x)): it has no plot.
        let (plot, data1) = if go == NoteGo::Deal {
            (None, i32::from(n.x))
        } else {
            (n.plot().map(|(x, y)| Plot { x, y }), n.data1)
        };
        // The core's ids are unique; a repeated one keeps its first row.
        let _ = out.try_push(Note {
            id,
            category: category(n.kind),
            message: Arc::from(n.message.as_str()),
            summary: Arc::from(n.summary.as_str()),
            turn: n.turn,
            plot,
            go,
            data1,
            data2: n.data2,
            dismissable: dismissable(n.kind),
        });
    }
    out
}

/// The ids of the notifications the core still has (the ones it dismissed itself are gone for good): a local
/// dismissal of any other note can be forgotten.
#[must_use]
pub fn live_ids(notes: &[Notification]) -> BTreeSet<Key> {
    notes
        .iter()
        .filter(|n| !n.dismissed)
        .map(|n| note_key(n.id))
        .collect()
}

/// `notes` from `Q_NOTIFICATIONS` and the locally dismissed set, with the [`live_ids`] of the read.
pub(crate) fn notes(
    read: &mut CountingRead<'_, '_>,
    dismissed: &BTreeSet<Key>,
) -> Result<(KeyedList<Note>, BTreeSet<Key>), ReadError> {
    let notes = read.notifications()?;
    Ok((rows(&notes, dismissed), live_ids(&notes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hash_is_the_cores() {
        // The values the core's header gives these types.
        for (name, hash) in [
            ("NOTIFICATION_GENERIC", 0xBE97_25C6),
            ("NOTIFICATION_TECH", 0x5ACF_A6F7),
            ("NOTIFICATION_POLICY", 0x252B_FE28),
            ("NOTIFICATION_PRODUCTION", 0x7701_4C62),
            ("NOTIFICATION_CITY_GROWTH", 0xCD6F_572E),
            ("NOTIFICATION_BARBARIAN", 0x70CB_3D66),
            ("NOTIFICATION_GREAT_PERSON_ACTIVE_PLAYER", 0x83CD_0016),
            ("NOTIFICATION_DIPLOMACY_DECLARATION", 0x4EA0_0B5D),
            ("NOTIFICATION_DEAL_EXPIRED_OPEN_BORDERS", 0x008A_2ABC),
            ("NOTIFICATION_RELIGION_FOUNDED_ACTIVE_PLAYER", 0x0CE3_37A4),
        ] {
            assert_eq!(type_hash(name), hash, "{name}");
        }
    }

    #[test]
    fn the_table_has_no_two_names_with_one_hash() {
        let mut seen = std::collections::BTreeMap::new();
        for (name, _) in CATEGORIES {
            if let Some(other) = seen.insert(type_hash(name), *name) {
                panic!("{name} and {other} hash alike");
            }
        }
    }

    #[test]
    fn categories_by_hash() {
        assert_eq!(category(0xCD6F_572E), NoteCat::Growth);
        assert_eq!(category(0x83CD_0016), NoteCat::GreatPeople);
        assert_eq!(category(0x4EA0_0B5D), NoteCat::Diplomacy);
        assert_eq!(category(0x70CB_3D66), NoteCat::Military);
        assert_eq!(category(0xBE97_25C6), NoteCat::Other, "generic");
        assert_eq!(category(12345), NoteCat::Other, "unknown");
    }

    #[test]
    fn a_click_does_what_the_cores_activate_does_for_the_type() {
        assert_eq!(action(0x5ACF_A6F7), NoteGo::ChooseTech, "tech");
        assert_eq!(action(0x252B_FE28), NoteGo::Policies, "policy");
        assert_eq!(action(0x7701_4C62), NoteGo::Production, "production");
        assert_eq!(action(0x70CB_3D66), NoteGo::Look, "barbarian: the default");
        assert_eq!(
            action(type_hash("NOTIFICATION_UNIT_PROMOTION")),
            NoteGo::Unit
        );
        assert_eq!(
            action(type_hash("NOTIFICATION_FOUND_PANTHEON")),
            NoteGo::Pantheon
        );
        assert_eq!(action(12345), NoteGo::Look, "unknown: the default");
        let mut seen = std::collections::BTreeMap::new();
        for (name, _) in ACTIONS {
            if let Some(other) = seen.insert(type_hash(name), *name) {
                panic!("{name} and {other} hash alike");
            }
        }
    }

    #[test]
    fn the_notes_waiting_for_a_choice_may_not_be_dismissed() {
        assert!(!dismissable(0x5ACF_A6F7), "tech");
        assert!(!dismissable(0x7701_4C62), "production");
        assert!(!dismissable(0x252B_FE28), "policy");
        assert!(dismissable(0xCD6F_572E), "growth");
        assert!(dismissable(0x70CB_3D66), "barbarian");
        assert!(dismissable(12345), "unknown");
    }
}
