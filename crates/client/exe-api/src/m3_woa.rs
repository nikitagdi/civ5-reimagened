//! Contexts of milestone M3, work order A: K8 (city screen) and K18 (popup host and event popups).
//! The names are bare Lua file names; `m2_contexts()` appends them to the M2 list.

/// K8: the city view, the (stubbed in EUI) production popup, city naming and the plot purchase.
pub const K8: &[&str] = &[
    "CityView",
    "ProductionPopup",
    "SetCityName",
    "CityPurchasePopup",
];

/// K18: the popup host (it includes `PopupsGeneric/*Popup.lua`) and the event popups.
pub const K18: &[&str] = &[
    "GenericPopup",
    "TextPopup",
    "NewEraPopup",
    "NaturalWonderPopup",
    "GoodyHutPopup",
    "BarbarianCampPopup",
    "GoldenAgePopup",
    "GreatPersonRewardPopup",
    "WonderPopup",
    "ChooseFreeItem",
    "ChooseMayaBonus",
    "SetUnitName",
    "ChooseGoodyHutReward",
    "ChooseAdmiralNewPort",
];
