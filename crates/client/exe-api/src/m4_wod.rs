//! Contexts of milestone M4, work order D: K12 diplomacy in the MP form of the original (U-D6, no 3D leaders): the leader
//! screen without the 3D head, the discussion, the trade table, the city-state windows and the declare-war confirmation.
//! The names are bare Lua file names; `m2_contexts()` appends them to the list.

pub const K12: &[&str] = &[
    "LeaderHeadRoot",
    "DiscussionDialog",
    "DiploTrade",
    "TradeLogic",
    "CityStateGreetingPopup",
    "CityStateDiploPopup",
    "DeclareWarPopup",
    "DeclareWarMovePopup",
];
