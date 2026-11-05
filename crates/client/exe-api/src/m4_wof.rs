//! Contexts of milestone M4, work order F: K16 (culture, great works, archaeology) and K17 (overviews).
//! The names are bare Lua file names; `m2_contexts()` appends them to the M2 list.

/// K16: the culture overview (great works, tourism, swapping), the great work splash, the archaeology choice.
pub const K16: &[&str] = &[
    "CultureOverview",
    "GreatWorkPopup",
    "ChooseArchaeologyPopup",
];

/// K17: the economic, happiness, diplomatic (tabs), military, demographics and victory overviews, the notification log.
pub const K17: &[&str] = &[
    "NotificationLogPopup",
    "WhosWinningPopup",
    "EconomicOverview",
    "EconomicGeneralInfo",
    "HappinessInfo",
    "DiploOverview",
    "DiploCurrentDeals",
    "DiploRelationships",
    "DiploGlobalRelationships",
    "MilitaryOverview",
    "Demographics",
    "VictoryProgress",
];
