local function color(key)
    local c = GameInfo.Colors[key]
    return math.floor(c.Red * 255) * 16777216 + math.floor(c.Green * 255) * 65536 + math.floor(c.Blue * 255) * 256 + 255
end

-- MilitaryOverview's status column: its order and its texts; an automated unit reads as automated building (a worker
-- at work) or exploring.
local function status(u, player)
    if u:IsEmbarked() then return "embarked", L"TXT_KEY_UNIT_STATUS_EMBARKED" end
    if u:IsGarrisoned() then return "garrison", L"TXT_KEY_MISSION_GARRISON" end
    if u:IsAutomated() then return "automated", L(u:IsWork() and "TXT_KEY_ACTION_AUTOMATE_BUILD" or "TXT_KEY_ACTION_AUTOMATE_EXPLORE") end
    local activity = u:GetActivityType()
    if activity == ActivityTypes.ACTIVITY_HEAL then return "healing", L"TXT_KEY_MISSION_HEAL" end
    if activity == ActivityTypes.ACTIVITY_SENTRY then return "sentry", L"TXT_KEY_MISSION_ALERT" end
    if u:GetFortifyTurns() > 0 then return "fortified", L"TXT_KEY_UNIT_STATUS_FORTIFIED" end
    if activity == ActivityTypes.ACTIVITY_SLEEP then return "sleeping", L"TXT_KEY_MISSION_SLEEP" end
    local build = u:GetBuildType()
    if build >= 0 then
        local info = GameInfo.Builds[build]
        local turns = u:GetPlot():GetBuildTurnsLeft(build, player, 0, 0)
        return "building", L(info.Description), turns > 0 and turns < 4000 and turns or nil
    end
    return "none", ""
end

return {
    data = function(player)
        local p = assert(Players[player], "player is unavailable")
        local civ = GameInfo.Civilizations[p:GetCivilizationType()]
        local colors = GameInfo.PlayerColors[civ.DefaultPlayerColor]
        local rows, counts, used = array(), { military = 0, civilian = 0 }, 0
        for u in p:Units() do
            local info = GameInfo.Units[u:GetUnitType()]
            local combat = u:IsCombatUnit() or u:NukeDamageLevel() >= 0
            local state, text, turns = status(u, player)
            local hp, max_hp = u:GetCurrHitPoints(), u:GetMaxHitPoints()
            local row = {
                id = u:GetID(), unit_key = key_unit(player, u:GetID()), name = u:GetName(), icon = "unit:" .. info.Type,
                military = combat, status = state, status_text = text, build_turns = turns,
                moves = u:MovesLeft() / GameDefines.MOVE_DENOMINATOR, max_moves = u:MaxMoves() / GameDefines.MOVE_DENOMINATOR,
                strength = u:GetBaseCombatStrength(), ranged = u:GetBaseRangedCombatStrength(), can_promote = u:IsPromotionReady(),
            }
            if hp < max_hp then row.health_pct = math.floor(100 * hp / max_hp) end
            rows[#rows + 1] = row
            local group = combat and "military" or "civilian"
            counts[group] = counts[group] + 1
            if not u:IsTrade() then used = used + 1 end
        end
        table.sort(rows, function(a, b) return a.id < b.id end)
        local limit, over = p:GetNumUnitsSupplied(), p:GetNumUnitsOutOfSupply()
        return {
            player = { name = L(GameInfo.Leaders[p:GetLeaderType()].Description), civ_name = L(civ.ShortDescription), player = player, icon = "civ:" .. civ.Type, color_bg = color(colors.PrimaryColor), color_fg = color(colors.SecondaryColor) },
            supply = { difficulty = p:GetNumUnitsSuppliedByHandicap(), cities = p:GetNumUnitsSuppliedByCities(), population = p:GetNumUnitsSuppliedByPopulation(), limit = limit, used = used, remaining = math.max(0, limit - used), over = over, production_penalty_pct = p:GetUnitProductionMaintenanceMod() },
            great_general = { progress = p:GetCombatExperience(), threshold = p:GreatGeneralThreshold(), icon = "unit:UNIT_GREAT_GENERAL" },
            great_admiral = { progress = p:GetNavalCombatExperience(), threshold = p:GreatAdmiralThreshold(), icon = "unit:UNIT_GREAT_ADMIRAL" },
            counts = counts, units = rows,
        }
    end,
    actions = {},
}
