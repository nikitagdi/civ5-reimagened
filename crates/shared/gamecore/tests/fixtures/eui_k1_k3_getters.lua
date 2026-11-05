-- The gamecore getters the original UI (EUI K1..K3: UnitFlagManager, CityBannerManager, TopPanel, UnitPanel, NotificationPanel, ...)
-- reads from a replica, taken from our inventory of the API the EUI calls (project notes, not in this repository; read-only names only).
-- GettersPass() calls each one nil-safely with pcall and returns calls, errors, missing (methods this DLL lacks), a sample of errors.
local NAMES = {
  Player = {
    "CalculateGoldRate", "CalculateGoldRateTimes100", "CalculateGrossGold", "CalculateGrossGoldTimes100",
    "CalculateInflatedCosts", "CalculateTotalYield", "CalculateUnitCost", "CalculateUnitSupply",
    "CanAdoptPolicy", "CanCommitVote", "CanEverResearch", "CanGetGoody", "CanMajorBullyGold",
    "CanMajorBullyUnit", "CanMajorBuyout", "CanMajorGiftTileImprovement", "CanMajorGiftTileImprovementAtPlot",
    "CanMajorStartProtection", "CanMajorWithdrawProtection", "CanRaze", "CanResearch", "CanResearchForFree",
    "CanSpyStageCoup", "CanTrain", "CanUnlockPolicyBranch", "CountNumBuildings",
    "DoesUnitPassFaithPurchaseCheck", "GetAlliedTurns", "GetAlly", "GetAnarchyNumTurns",
    "GetApproachTowardsUsGuess", "GetAttackBonusTurns", "GetAvailableSpyRelocationCities",
    "GetAvailableTenets", "GetBarbarianCombatBonus", "GetBeliefInPantheon", "GetBuildingClassCount",
    "GetBuildingGoldMaintenance", "GetBuildingOfClosestGreatWorkSlot", "GetBuildingProductionNeeded",
    "GetBuyoutCost", "GetCapitalCity", "GetCapitalUnhappinessMod", "GetCityByID",
    "GetCityConnectionGoldTimes100", "GetCityConnectionRouteGoldTimes100",
    "GetCityConnectionTradeRouteGoldModifier", "GetCityCountUnhappinessMod", "GetCityOfClosestGreatWorkSlot",
    "GetCivilizationAdjective", "GetCivilizationAdjectiveKey", "GetCivilizationDescription",
    "GetCivilizationDescriptionKey", "GetCivilizationShortDescription", "GetCivilizationShortDescriptionKey",
    "GetCivilizationType", "GetCombatBonusVsHigherTech", "GetCombatBonusVsLargerCiv", "GetCombatExperience",
    "GetCommitVoteDetails", "GetCoopWarAcceptedState", "GetCultureBombTimer", "GetCultureBonusTurns",
    "GetCulturePerTurnFromBonusTurns", "GetCulturePerTurnFromMinorCivs", "GetCulturePerTurnFromReligion",
    "GetCurrentCapitalFoodBonus", "GetCurrentEra", "GetCurrentOtherCityFoodBonus", "GetCurrentResearch",
    "GetCurrentScienceFriendshipBonusTimes100", "GetCurrentSpawnEstimate", "GetDealMyValue",
    "GetDenouncedPlayerCounter", "GetDominantPolicyBranchForTitle", "GetEndTurnBlockingNotificationIndex",
    "GetEndTurnBlockingType", "GetEspionageCityStatus", "GetEspionageSpies", "GetExcessHappiness",
    "GetExtraBuildingHappinessFromPolicies", "GetExtraHappinessPerCity", "GetExtraHappinessPerLuxury",
    "GetFaith", "GetFaithPerTurnFromCities", "GetFaithPerTurnFromMinorCivs", "GetFaithPerTurnFromReligion",
    "GetFaithPurchaseIndex", "GetFaithPurchaseType", "GetFirstReadyUnit",
    "GetFoundedReligionEnemyCityCombatMod", "GetFoundedReligionFriendlyCityCombatMod",
    "GetFriendshipFromGoldGift", "GetFriendshipFromUnitGift", "GetGarrisonedCityRangeStrikeModifier",
    "GetGiftTileImprovementCost", "GetGold", "GetGoldFromCitiesMinusTradeRoutesTimes100",
    "GetGoldFromCitiesTimes100", "GetGoldPerTurnFromDiplomacy", "GetGoldPerTurnFromReligion",
    "GetGoldPerTurnFromTraits", "GetGoldenAgeGreatArtistRateModifier",
    "GetGoldenAgeGreatMusicianRateModifier", "GetGoldenAgeGreatWriterRateModifier",
    "GetGoldenAgeProgressMeter", "GetGoldenAgeProgressThreshold", "GetGoldenAgeTourismModifier",
    "GetGoldenAgeTurns", "GetGreatArtistRateModifier", "GetGreatEngineerRateModifier",
    "GetGreatGeneralCombatBonus", "GetGreatMerchantRateModifier", "GetGreatMusicianRateModifier",
    "GetGreatPeopleRateModifier", "GetGreatScientistRateModifier", "GetGreatWorks",
    "GetGreatWriterRateModifier", "GetHandicapType", "GetHappiness", "GetHappinessFromBuildings",
    "GetHappinessFromCities", "GetHappinessFromLeagues", "GetHappinessFromLuxury",
    "GetHappinessFromMinorCivs", "GetHappinessFromNaturalWonders", "GetHappinessFromPolicies",
    "GetHappinessFromReligion", "GetHappinessFromResourceVariety", "GetHappinessFromResources",
    "GetHappinessFromTradeRoutes", "GetHappinessPerTradeRoute", "GetID", "GetImprovementGoldMaintenance",
    "GetIncomingUnitCountdown", "GetInfluenceLevel", "GetInfluenceOn", "GetInfluencePerTurn",
    "GetInfluenceSpyRankTooltip", "GetInfluenceTradeRouteScienceBonus", "GetInfluenceTrend",
    "GetInternationalTradeRouteBaseBonus", "GetInternationalTradeRouteDomainModifier",
    "GetInternationalTradeRouteExclusiveBonus", "GetInternationalTradeRouteGPTBonus",
    "GetInternationalTradeRouteOtherTraitBonus", "GetInternationalTradeRoutePlotToolTip",
    "GetInternationalTradeRoutePolicyBonus", "GetInternationalTradeRouteResourceTraitModifier",
    "GetInternationalTradeRouteRiverModifier", "GetInternationalTradeRouteScience",
    "GetInternationalTradeRouteTheirBuildingBonus", "GetInternationalTradeRouteTotal",
    "GetInternationalTradeRouteYourBuildingBonus", "GetIntrigueMessages", "GetJONSCulture",
    "GetJONSCultureEverGenerated", "GetJONSCulturePerTurnForFree", "GetJONSCulturePerTurnFromCities",
    "GetJONSCulturePerTurnFromExcessHappiness", "GetJONSCulturePerTurnFromTraits", "GetLateGamePolicyTree",
    "GetLeaderType", "GetLiberationPreviewString", "GetMajorBullyGoldDetails", "GetMajorBullyUnitDetails",
    "GetMayaCalendarLongString", "GetMayaCalendarString", "GetMilitaryMight",
    "GetMinimumFaithNextGreatProphet", "GetMinorCivBullyGoldAmount", "GetMinorCivContestValueForLeader",
    "GetMinorCivContestValueForPlayer", "GetMinorCivCurrentCultureBonus", "GetMinorCivCurrentFaithBonus",
    "GetMinorCivCurrentHappinessBonus", "GetMinorCivFriendshipAnchorWithMajor",
    "GetMinorCivFriendshipWithMajor", "GetMinorCivNumDisplayedQuestsForPlayer", "GetMinorCivTrait",
    "GetMinorCivType", "GetMinorCivUniqueUnit", "GetName", "GetNameKey", "GetNavalCombatExperience",
    "GetNegativeArchaeologyPoints", "GetNegativeReligiousConversionPoints", "GetNextDigCompletePlot",
    "GetNextPolicyCost", "GetNickName", "GetNotificationDismissed", "GetNotificationIndex",
    "GetNotificationStr", "GetNotificationSummaryStr", "GetNotificationTurn", "GetNumAvailableTradeUnits",
    "GetNumCities", "GetNumCivsInfluentialOn", "GetNumCivsToBeInfluentialOn", "GetNumFaithGreatPeople",
    "GetNumFreeGreatPeople", "GetNumFreePolicies", "GetNumFreeTechs", "GetNumFreeTenets",
    "GetNumGreatWorkSlots", "GetNumGreatWorks", "GetNumInternationalTradeRoutesAvailable",
    "GetNumInternationalTradeRoutesUsed", "GetNumMaintenanceFreeUnits", "GetNumMayaBoosts",
    "GetNumNotifications", "GetNumPlots", "GetNumPolicies", "GetNumPoliciesInBranch",
    "GetNumPolicyBranchesUnlocked", "GetNumResourceAvailable", "GetNumResourceTotal", "GetNumResourceUsed",
    "GetNumSpies", "GetNumTechDifference", "GetNumUnassignedSpies", "GetNumUnits", "GetNumUnitsOutOfSupply",
    "GetNumUnitsSupplied", "GetNumUnitsSuppliedByCities", "GetNumUnitsSuppliedByHandicap",
    "GetNumWorldWonders", "GetOpinionTable", "GetOthersGreatWorks", "GetPersonality",
    "GetPlayerBuildingClassHappiness", "GetPlayerColor", "GetPlayerColors",
    "GetPolicyBuildingClassYieldModifier", "GetPolicyCatchSpiesModifier",
    "GetPolicyEspionageCatchSpiesModifier", "GetPolicyEspionageModifier", "GetPolicyGreatArtistRateModifier",
    "GetPolicyGreatEngineerRateModifier", "GetPolicyGreatMerchantRateModifier",
    "GetPolicyGreatMusicianRateModifier", "GetPolicyGreatPeopleRateModifier",
    "GetPolicyGreatScientistRateModifier", "GetPolicyGreatWriterRateModifier", "GetPotentialAdmiralNewPort",
    "GetPotentialInternationalTradeRouteDestinations", "GetPotentialTradeUnitNewHomeCity",
    "GetProjectProductionNeeded", "GetPublicOpinionPreferredIdeology", "GetPublicOpinionTooltip",
    "GetPublicOpinionType", "GetPublicOpinionUnhappiness", "GetPublicOpinionUnhappinessTooltip",
    "GetQuestData1", "GetQuestData2", "GetQuestTurnsRemaining", "GetQueuePosition", "GetRecentIntrigueInfo",
    "GetRecommendedFoundCityPlots", "GetRecommendedWorkerPlots", "GetReplayData", "GetResearchCost",
    "GetResearchProgress", "GetResearchTurnsLeft", "GetResourceExport", "GetResourceImport", "GetScience",
    "GetScienceFromBudgetDeficitTimes100", "GetScienceFromCitiesTimes100", "GetScienceFromHappinessTimes100",
    "GetScienceFromOtherPlayersTimes100", "GetScienceFromResearchAgreementsTimes100", "GetScore",
    "GetScoreFromCities", "GetScoreFromFutureTech", "GetScoreFromGreatWorks", "GetScoreFromLand",
    "GetScoreFromPolicies", "GetScoreFromReligion", "GetScoreFromScenario1", "GetScoreFromScenario2",
    "GetScoreFromScenario3", "GetScoreFromScenario4", "GetScoreFromTechs", "GetScoreFromWonders",
    "GetStateReligionKey", "GetSwappableGreatArt", "GetSwappableGreatArtifact", "GetSwappableGreatMusic",
    "GetSwappableGreatWriting", "GetTeam", "GetTenet", "GetTotalFaithPerTurn", "GetTotalJONSCulturePerTurn",
    "GetTourism", "GetTourismModifierWith", "GetTourismModifierWithTooltip", "GetTradeRouteRange",
    "GetTradeRoutes", "GetTradeRoutesAvailable", "GetTradeRoutesToYou", "GetTradeToYouRoutesTTString",
    "GetTradeUnitType", "GetTradeYourRoutesTTString", "GetTraitCityStateCombatModifier",
    "GetTraitCityUnhappinessMod", "GetTraitGoldenAgeCombatModifier", "GetTraitGreatGeneralExtraBonus",
    "GetTurnLastPledgeBrokenByMajor", "GetTurnLastPledgedProtectionByMajor", "GetTurnsToInfluential",
    "GetUnhappiness", "GetUnhappinessForecast", "GetUnhappinessFromCapturedCityCount",
    "GetUnhappinessFromCityCount", "GetUnhappinessFromCityForUI", "GetUnhappinessFromCitySpecialists",
    "GetUnhappinessFromOccupiedCities", "GetUnhappinessFromPublicOpinion", "GetUnhappinessFromUnits",
    "GetUnhappinessMod", "GetUnitBaktun", "GetUnitByID", "GetUnitProductionMaintenanceMod",
    "GetUnitProductionNeeded", "GetWarmongerPreviewString", "GetWrittenArtifactCulture",
    "HasAvailableGreatWorkSlot", "HasPolicy", "HasReceivedNetTurnComplete", "HasRecentIntrigueAbout",
    "HasSpyEstablishedSurveillance", "IsAbleToAnnexCityStates", "IsAlive", "IsAllies", "IsAnarchy",
    "IsAskedToStopConverting", "IsAskedToStopDigging", "IsBarbarian", "IsBuildBlockedByFeature",
    "IsCanPurchaseAnyCity", "IsCapitalConnectedToCity", "IsConnected", "IsDenouncedPlayer",
    "IsDenouncingPlayer", "IsEmpireSuperUnhappy", "IsEmpireUnhappy", "IsEmpireVeryUnhappy", "IsEverAlive",
    "IsFreeMayaGreatPersonChoice", "IsFriendDeclaredWarOnUs", "IsFriendDenouncedUs", "IsFriends",
    "IsGoldenAge", "IsGoldenAgeCultureBonusDisabled", "IsHalfSpecialistUnhappiness", "IsHasLostCapital",
    "IsHuman", "IsMinorCiv", "IsMinorCivContestLeader", "IsMinorCivDisplayedQuestForPlayer",
    "IsMinorCivHasUniqueUnit", "IsMinorCivUnitSpawningDisabled", "IsMinorPermanentWar",
    "IsMinorWarQuestWithMajorActive", "IsMyDiplomatVisitingThem", "IsObserver", "IsPeaceBlocked",
    "IsPlayerHasOpenBorders", "IsPlayerHasOpenBordersAutomatically", "IsPolicyBlocked",
    "IsPolicyBranchBlocked", "IsPolicyBranchFinished", "IsPolicyBranchUnlocked", "IsProtectedByMajor",
    "IsProtectingMinor", "IsProxyWarActiveForMajor", "IsSimultaneousTurns", "IsSpyDiplomat",
    "IsSpySchmoozing", "IsStopSpyingMessageTooSoon", "IsThreateningBarbariansEventActiveForPlayer",
    "IsTraitBonusReligiousBelief", "IsTurnActive", "IsUsingMayaCalendar", "IsWillAcceptPeaceWithPlayer",
    "WasResurrectedThisTurnBy",
  },
  City = {
    "CanBuyPlotAt", "CanConstruct", "CanConstructTooltip", "CanMaintain", "CanRangeStrike",
    "CanRangeStrikeAt", "CanRangeStrikeNow", "CanTrain", "CanTrainTooltip", "CanWork",
    "GetAirStrikeDefenseDamage", "GetBaseTourism", "GetBaseYieldRate", "GetBaseYieldRateFromBuildings",
    "GetBaseYieldRateFromMisc", "GetBaseYieldRateFromReligion", "GetBaseYieldRateFromSpecialists",
    "GetBaseYieldRateFromTerrain", "GetBuildingEspionageModifier", "GetBuildingFaithPurchaseCost",
    "GetBuildingGlobalEspionageModifier", "GetBuildingGreatWork", "GetBuildingProductionTurnsLeft",
    "GetBuildingPurchaseCost", "GetBuyPlotCost", "GetBuyablePlotList", "GetCityIndexPlot",
    "GetCultureFromSpecialist", "GetCultureRateModifier", "GetCurrentProductionDifference",
    "GetCurrentProductionDifferenceTimes100", "GetDamage", "GetFaithBuildingTourism", "GetFaithPerTurn",
    "GetFaithPerTurnFromBuildings", "GetFaithPerTurnFromPolicies", "GetFaithPerTurnFromReligion",
    "GetFaithPerTurnFromTraits", "GetFaithPurchaseBuildingTooltip", "GetFaithPurchaseUnitTooltip",
    "GetFocusType", "GetFood", "GetFoodTimes100", "GetFoodTurnsLeft", "GetGarrisonedUnit",
    "GetGreatPeopleRateModifier", "GetHappiness", "GetID", "GetJONSCulturePerTurn",
    "GetJONSCulturePerTurnFromBuildings", "GetJONSCulturePerTurnFromGreatWorks",
    "GetJONSCulturePerTurnFromLeagues", "GetJONSCulturePerTurnFromPolicies",
    "GetJONSCulturePerTurnFromReligion", "GetJONSCulturePerTurnFromSpecialists",
    "GetJONSCulturePerTurnFromTraits", "GetJONSCultureStored", "GetJONSCultureThreshold", "GetLocalHappiness",
    "GetMaxHitPoints", "GetName", "GetNameKey", "GetNumCityPlots", "GetNumFollowers",
    "GetNumForcedWorkingPlots", "GetNumFreeBuilding", "GetNumGreatWorks",
    "GetNumSpecialistsAllowedByBuilding", "GetNumSpecialistsInBuilding", "GetNumTradeRoutesAddingPressure",
    "GetNumWorldWonders", "GetOrderFromQueue", "GetOrderQueueLength", "GetOriginalOwner", "GetOwner",
    "GetPressurePerTurn", "GetPreviousOwner", "GetProduction", "GetProductionBuilding",
    "GetProductionModifier", "GetProductionNameKey", "GetProductionNeeded", "GetProductionProcess",
    "GetProductionProject", "GetProductionTimes100", "GetProductionTurnsLeft", "GetProductionUnit",
    "GetProjectProductionTurnsLeft", "GetProjectPurchaseCost", "GetPurchaseBuildingTooltip",
    "GetPurchaseUnitTooltip", "GetRazingTurns", "GetReligionBuildingClassHappiness",
    "GetReligionCityRangeStrikeModifier", "GetReligiousMajority", "GetResistanceTurns", "GetResourceDemanded",
    "GetSellBuildingRefund", "GetSpecialistCount", "GetSpecialistGreatPersonProgress",
    "GetSpecialistUpgradeThreshold", "GetSpecialistYield", "GetStrengthValue", "GetTeam", "GetThemingBonus",
    "GetThemingTooltip", "GetTotalBaseBuildingMaintenance", "GetTotalSlotsTooltip", "GetTourismTooltip",
    "GetUnitFaithPurchaseCost", "GetUnitProductionTurnsLeft", "GetUnitPurchaseCost",
    "GetWeLoveTheKingDayCounter", "GetX", "GetY", "GetYieldModifierTooltip", "GetYieldRate",
    "GetYieldRateTimes100", "IsBlockaded", "IsBuildingSellable", "IsCanAddSpecialistToBuilding",
    "IsCanPurchase", "IsCapital", "IsFoodProduction", "IsForcedAvoidGrowth", "IsForcedWorkingPlot",
    "IsHasBuilding", "IsHasResourceLocal", "IsHolyCityForReligion", "IsNoAutoAssignSpecialists",
    "IsNoOccupiedUnhappiness", "IsOccupied", "IsOriginalMajorCapital", "IsPlotBlockaded", "IsProduction",
    "IsProductionProcess", "IsPuppet", "IsRazing", "IsResistance", "IsThemingBonusPossible", "IsWorkingPlot",
  },
  Unit = {
    "CanAirlift", "CanAirliftAt", "CanDisembarkOnto", "CanDistanceGift", "CanEmbarkOnto", "CanNuke",
    "CanNukeAt", "CanParadrop", "CanParadropAt", "CanPromote", "CanRangeStrike", "CanRangeStrikeAt",
    "CanRebaseAt", "GetActivityType", "GetAdjacentModifier", "GetAirStrikeDefenseDamage", "GetAttackModifier",
    "GetBaseCombatStrength", "GetBaseRangedCombatStrength", "GetBlastTourism", "GetBuildType",
    "GetCaptureChance", "GetCaptureUnitType", "GetCivilizationType", "GetCombatDamage",
    "GetConversionStrength", "GetCurrHitPoints", "GetDamage", "GetDeclareWarRangeStrike",
    "GetDefenseModifier", "GetDiscoverAmount", "GetDomainType", "GetDropRange", "GetEmbarkedUnitDefense",
    "GetExoticGoodsGoldAmount", "GetExoticGoodsXPAmount", "GetExperience", "GetExtraCombatPercent",
    "GetFireSupportUnit", "GetFortifyTurns", "GetFriendlyLandsAttackModifier", "GetFriendlyLandsModifier",
    "GetGarrisonedCity", "GetGivePoliciesCulture", "GetGoldenAgeTurns", "GetGreatGeneralCombatModifier",
    "GetGreatWorkSlotType", "GetHurryProduction", "GetID", "GetInterceptorCount", "GetLevel",
    "GetMajorityReligionAfterSpread", "GetMaxAttackStrength", "GetMaxDefenseStrength", "GetMaxHitPoints",
    "GetMaxRangedCombatStrength", "GetName", "GetNameKey", "GetNameNoDesc", "GetNearbyImprovementModifier",
    "GetNumEnemyUnitsAdjacent", "GetNumFollowersAfterSpread", "GetNumResourceNeededToUpgrade",
    "GetOutsideFriendlyLandsModifier", "GetOwner", "GetPlot", "GetRangeCombatDamage",
    "GetRangedAttackModifier", "GetReligion", "GetReverseGreatGeneralModifier", "GetScrapGold",
    "GetSpreadsLeft", "GetStrategicResourceCombatPenalty", "GetTeam", "GetTourismBlastStrength",
    "GetTradeGold", "GetTradeInfluence", "GetTransportUnit", "GetUnhappinessCombatPenalty",
    "GetUnitClassModifier", "GetUnitClassType", "GetUnitCombatType", "GetUnitType", "GetUpgradeUnitType",
    "GetX", "GetY", "HasCargo", "HasName", "IsActionRecommended", "IsAmphib", "IsAutomated", "IsBarbarian",
    "IsCargo", "IsCityAttackOnly", "IsCombatUnit", "IsDead", "IsDelayedDeath", "IsEmbarked",
    "IsEverFortifyable", "IsFriendlyUnitAdjacent", "IsGarrisoned", "IsGreatPerson", "IsHasPromotion",
    "IsHigherTechThan", "IsIgnoreGreatGeneralBenefit", "IsImmobile", "IsInvisible", "IsLargerCivThan",
    "IsNearGreatGeneral", "IsNearSapper", "IsPromotionReady", "IsRangeAttackIgnoreLOS",
    "IsRangeAttackOnlyInDomain", "IsRanged", "IsRangedSupportFire", "IsReadyToMove",
    "IsRiverCrossingNoPenalty", "IsStackedGreatGeneral", "IsTrade", "IsWork",
  },
  Plot = {
    "CalculateYield", "CanSeePlot", "GetArchaeologyArtifactEra", "GetArchaeologyArtifactPlayer1",
    "GetArchaeologyArtifactPlayer2", "GetArchaeologyArtifactType", "GetArchaeologyArtifactWork",
    "GetBuildProgress", "GetBuildTurnsLeft", "GetBuildTurnsTotal", "GetContinentArtType",
    "GetFeatureProduction", "GetFeatureType", "GetImprovementType", "GetLayerUnit",
    "GetNumFriendlyUnitsOfType", "GetNumLayerUnits", "GetNumResource", "GetNumUnits", "GetOwner",
    "GetPlotCity", "GetPlotIndex", "GetResourceType", "GetRevealedImprovementType", "GetRevealedOwner",
    "GetRevealedRouteType", "GetTeam", "GetTerrainType", "GetUnit", "GetVisibilityCount", "GetWorkingCity",
    "GetX", "GetY", "GetYield", "GetYieldWithBuild", "HasWrittenArtifact", "IsAdjacentTeam",
    "IsBuildRemovesFeature", "IsCity", "IsEnemyCity", "IsFighting", "IsFreshWater", "IsFriendlyTerritory",
    "IsHills", "IsLake", "IsMountain", "IsOpenGround", "IsOwned", "IsResourceConnectedByImprovement",
    "IsRevealed", "IsRiver", "IsRiverCrossingToPlot", "IsRoughGround", "IsTradeRoute", "IsVisible",
    "IsVisibleEnemyUnit", "IsWater",
  },
  Game = {
    "CanHandleAction", "CountHumanPlayersAlive", "GetActiveLeague", "GetActivePlayer", "GetActiveTeam",
    "GetAllowRClickMovementWhileScrolling", "GetArtifactName", "GetAvailableBonusBeliefs",
    "GetAvailableEnhancerBeliefs", "GetAvailableFollowerBeliefs", "GetAvailableFounderBeliefs",
    "GetAvailablePantheonBeliefs", "GetAvailableReformationBeliefs", "GetBeliefsInReligion",
    "GetBuildingYieldModifier", "GetCalendar", "GetCombatPrediction", "GetDealDuration",
    "GetElapsedGameTurns", "GetGameSpeedType", "GetGameState", "GetGameTurn", "GetGameTurnYear",
    "GetGreatWorkArtist", "GetGreatWorkClass", "GetGreatWorkController", "GetGreatWorkCreator",
    "GetGreatWorkCurrentThemingBonus", "GetGreatWorkEraShort", "GetGreatWorkName", "GetGreatWorkTooltip",
    "GetGreatWorkType", "GetHandicapType", "GetHolyCityForReligion", "GetLeague", "GetMaxTurns",
    "GetMinimumFaithNextPantheon", "GetNumActiveLeagues", "GetNumArchaeologySites", "GetNumCitiesFollowing",
    "GetNumCitiesPolicyCostMod", "GetNumCitiesTechCostMod", "GetNumFreePolicies", "GetNumGameTurnActive",
    "GetNumHiddenArchaeologySites", "GetNumLeaguesEverFounded", "GetNumReligionsStillToFound",
    "GetNumResourceRequiredForUnit", "GetNumVotesForTeam", "GetNumWorldWonders", "GetPeaceDuration",
    "GetReligionName", "GetReplayMessages", "GetResearchAgreementCost", "GetResourceUsageType", "GetStartEra",
    "GetStartTurn", "GetStartYear", "GetTimeString", "GetTurnString", "GetTurnsBetweenMinorCivElections",
    "GetTurnsUntilMinorCivElection", "GetVictory", "GetVoteCast", "GetVotesNeededForDiploVictory",
    "GetWinner", "GetWorldNumCitiesUnhappinessPercent", "IsBuildingRecommended", "IsGameMultiPlayer",
    "IsHotSeat", "IsNetworkMultiPlayer", "IsOption", "IsProcessingMessages", "IsProjectRecommended",
    "IsStaticTutorialActive", "IsTechRecommended", "IsUnitRecommended", "IsUnitedNationsActive",
  },
}

-- Not called by this pass, "Kind.Name".
-- 1. The DLL reads the argument unchecked (a great work, a belief list, a unit or city index the UI never passes) and the blind
--    arguments of this pass (0, 0) crash it.
-- (Player.GetMilitaryMight and Player.GetWrittenArtifactCulture, lazy caches the replica digest covers, run since FIX-0022 is in the build flag set.)
local SKIP = {
  ["Game.GetArtifactName"] = true, ["Game.GetBeliefsInReligion"] = true,
  ["Player.GetPotentialAdmiralNewPort"] = true, ["Player.GetPotentialInternationalTradeRouteDestinations"] = true,
  ["Player.GetPotentialTradeUnitNewHomeCity"] = true, ["Player.GetUnhappinessFromCityForUI"] = true,
  ["Unit.GetRangeCombatDamage"] = true, ["City.GetAirStrikeDefenseDamage"] = true, ["Player.GetReplayData"] = true,
}
local calls, errs, missing, sample = 0, 0, 0, {}

local ONLY = nil

local function pass(obj, kind, static)
  for _, name in ipairs(NAMES[kind]) do
    local f = obj[name]
    if ONLY and ONLY ~= kind .. "." .. name then f = false end
    -- The notification list exists for the seat only (the panel reads the active player's).
    if f and kind == "Player" and name:find("Notification") and obj:GetID() ~= Game.GetActivePlayer() then f = false end
    if SKIP[kind .. "." .. name] or (SKIP_EXTRA and SKIP_EXTRA[kind .. "." .. name]) then f = false end
    if f == nil then
      missing = missing + 1
    elseif f then
      calls = calls + 1
      if GetterTrace then GetterTrace(kind .. "." .. name) end
      local ok, err
      if static then ok, err = pcall(f, 0, 0) else ok, err = pcall(f, obj, 0, 0) end
      if not ok then
        errs = errs + 1
        if #sample < 6 then sample[#sample + 1] = kind .. "." .. name .. ": " .. tostring(err) end
      end
    end
  end
end

function GettersPass(only)
  ONLY = only
  calls, errs, missing, sample = 0, 0, 0, {}
  pass(Game, "Game", true)
  local seenPlots = 0
  for i = 0, 63 do
    local pl = Players[i]
    if pl ~= nil and pl:IsAlive() then
      pass(pl, "Player", false)
      local nc, nu = 0, 0
      for city in pl:Cities() do
        nc = nc + 1
        if nc <= 2 then
          pass(city, "City", false)
          local plot = city:Plot()
          if plot and seenPlots < 6 then seenPlots = seenPlots + 1; pass(plot, "Plot", false) end
        end
      end
      for unit in pl:Units() do
        nu = nu + 1
        if nu <= 3 then
          pass(unit, "Unit", false)
          local plot = unit:GetPlot()
          if plot and seenPlots < 6 then seenPlots = seenPlots + 1; pass(plot, "Plot", false) end
        end
      end
    end
  end
  return calls, errs, missing, table.concat(sample, " | ")
end

-- Living units and cities of every player (the setup announces each once).
function CountAlive()
  local units, cities = 0, 0
  for i = 0, 63 do
    local pl = Players[i]
    if pl ~= nil and pl:IsAlive() then
      for _ in pl:Units() do units = units + 1 end
      for _ in pl:Cities() do cities = cities + 1 end
    end
  end
  return units, cities
end

-- The first unit of a player: owner, id (nil when it has none).
function FirstUnit(owner)
  local pl = Players[owner]
  if pl == nil then return nil end
  for u in pl:Units() do return owner, u:GetID() end
  return nil
end

-- Keeps one object of the old game to prove the invalidation later.
function KeepOldUnit(owner)
  local pl = Players[owner]
  for u in pl:Units() do OldUnit = u; OldCity = nil; break end
  for c in pl:Cities() do OldCity = c; break end
  return OldUnit ~= nil
end

function OldUnitId() return OldUnit:GetID() end
function MyName() return Players[0]:GetName() end

-- Every "Kind.Name" the pass calls, one per line (for tests that look at them one by one).
function AllNames()
  local t = {}
  for kind, names in pairs(NAMES) do
    for _, n in ipairs(names) do t[#t + 1] = kind .. "." .. n end
  end
  table.sort(t)
  return table.concat(t, "\n")
end
