-- The M1 stand-ins that exe-api does not give: the DLL's API (Players, Game, Map, GameInfo, DB, ...) and
-- the exe globals of WO-5 (options, content, enums). A file of ours, derived from ui/tests/fixtures/m1_stubs.lua.
-- Stand-ins for the exe API and the game core, for the M1 run of EUI (a file of ours, not the
-- original's). Only what TopPanel, NotificationPanel and the core of EUI call; a missing
-- function is nil, as in the exe.

-- A table whose every field is a function returning nil, with a few numeric constants.
local function api(fields)
	return setmetatable(fields or {}, { __index = function(t, k)
		local f = function() return nil end
		rawset(t, k, f)
		return f
	end })
end

-- A table of constants: any unknown name is a distinct number.
local function enum(fields)
	local n = 0
	return setmetatable(fields or {}, { __index = function(t, k)
		if type(k) ~= "string" then return nil end
		n = n + 1
		rawset(t, k, 1000 + n)
		return 1000 + n
	end })
end

Mouse = { eLClick = 1, eRClick = 2, eLDblClick = 3, eMClick = 4, eMouseEnter = 6, eMouseExit = 7, eMouseWheel = 8 }
MouseEvents = { LButtonDown = 513, LButtonUp = 514, RButtonDown = 516, RButtonUp = 517, MouseMove = 512, MouseWheel = 522 }
KeyEvents = { KeyDown = 256, KeyUp = 257 }
Keys = enum{ VK_RETURN = 13, VK_ESCAPE = 27, VK_TAB = 9, VK_SHIFT = 16, VK_CONTROL = 17 }

-- (Events, LuaEvents, GameEvents, UI and Network come from exe-api.)

ContentType = enum{ GAMEPLAY = 1, SCENARIO = 2 }
ContentManager = api{ IsActive = function() return false end }
DB = api{ Query = function() return function() return nil end end }
Locale = api{
	ConvertTextKey = function(k, ...) return tostring(k) end,
	Lookup = function(k) return tostring(k) end,
	ToUpper = function(s) return string.upper(tostring(s)) end,
	ToLower = function(s) return string.lower(tostring(s)) end,
	Compare = function(a, b) if a < b then return -1 elseif a > b then return 1 end return 0 end,
	IsNilOrWhitespace = function(s) return s == nil or s:match("^%s*$") ~= nil end,
}
Game = api{ GetActivePlayer = function() return 0 end, GetActiveTeam = function() return 0 end, GetGameTurn = function() return 1 end }
GameDefines = setmetatable({ MAXIMUM_ACQUIRE_PLOT_DISTANCE = 5 }, { __index = function() return 0 end })
GameInfo = setmetatable({}, { __index = function(t, k)
	local tbl = setmetatable({}, { __call = function() return function() return nil end end,
		__index = function(_, key) return { ID = -1, Type = tostring(key) } end })
	rawset(t, k, tbl)
	return tbl
end })
GameInfoTypes = setmetatable({}, { __index = function() return -1 end })
PreGame = api{ IsMultiplayerGame = function() return false end, IsHotSeatGame = function() return false end }
TradeableItems = enum()
Map = api{ GetPlot = function() return api() end, GetNumPlots = function() return 0 end }
Players = setmetatable({}, { __index = function() return api() end })
Teams = setmetatable({}, { __index = function() return api() end })
OptionsManager = api()
Steam = api()
Matchmaking = api()
InterfaceDirective = enum()
InterfaceModeTypes = enum()
ButtonPopupTypes = enum()
NotificationTypes = enum()
EndTurnBlockingTypes = enum()
ActionSubTypes = enum()
Modding = api()
Controls = Controls or {}

function Color(r, g, b, a) return { x = r, y = g, z = b, w = a } end
PreGame.GetCivilization = function() return -1 end
PreGame.GetCivilizationColor = function() return -1 end
for _, name in ipairs{ "DisputeLevelTypes", "GameOptionTypes", "MajorCivApproachTypes", "ThreatTypes", "YieldTypes",
	"DomainTypes", "CombatPredictionTypes", "FlowDirectionTypes", "GameplayGameStateTypes", "PlotTypes", "TerrainTypes",
	"FeatureTypes", "ResourceUsageTypes", "CityAIFocusTypes", "OrderTypes", "CommandTypes", "MissionTypes", "ActivityTypes",
	"DirectionTypes", "FogOfWarModeTypes", "LeaderheadAnimationTypes", "PublicOpinionTypes", "GenericWorldAnchorTypes",
	"PolicyBranchTypes", "TaskTypes", "TechTypes", "UnitMoveRate", "ResourceClassTypes", "FaithPurchaseTypes",
	"ReligionTypes", "MinorCivTraitTypes", "MinorCivQuestTypes", "CitySizeTypes", "BuildingClassTypes", "PlayerTypes" } do
	if _G[name] == nil then _G[name] = enum() end
end
PreGame.GetGameSpeed = function() return 0 end
InStrategicView = function() return false end
ContentManager.GetAllPackageIDs = function() return {} end
Modding.GetActivatedMods = function() return {} end
MapModData = {}
Modding.OpenUserData = function()
	local values = {}
	return { GetValue = function(k) return values[k] end, SetValue = function(k, v) values[k] = v end }
end
-- A player/team/city object: every method answers 0 (names, descriptions and tool tip strings answer "";
-- the predicates `Is*`, `Has*` and `Can*` answer false, as 0 is true in Lua and `Game.IsOption` would switch every game option on).
local function numbers()
	return setmetatable({}, { __index = function(t, k)
		local v = (type(k) == "string" and (k:find("Name") or k:find("Description") or k:find("TTString"))) and "" or 0
		if type(k) == "string" and (k:find("^Is%u") or k:find("^Has%u") or k:find("^Can%u")) then v = false end
		local f = function() return v end
		if k == "Cities" or k == "Units" or k == "GetPlots" then
			f = function() return function() return nil end end
		end
		if k == "GetEspionageSpies" then f = function() return {} end end
		rawset(t, k, f)
		return f
	end })
end
Players = setmetatable({}, { __index = function(t, k) local p = numbers(); rawset(t, k, p); return p end })
Teams = setmetatable({}, { __index = function(t, k) local p = numbers(); rawset(t, k, p); return p end })
Game.GetActivePlayer = function() return 0 end
Game = numbers()
Game.GetGameTurn = function() return 1 end
IsGameCoreBusy = function() return false end
