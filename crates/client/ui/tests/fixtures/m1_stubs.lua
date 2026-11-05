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

Mouse = Mouse or { eLClick = 0, eLCLick = 0, eLDblClick = 1, eRClick = 2, eMClick = 3, eMouseEnter = 5, eMouseExit = 6 }
MouseEvents = MouseEvents or { LButtonDown = 513, LButtonUp = 514, RButtonDown = 516, RButtonUp = 517, MouseMove = 512, MouseWheel = 522 }
KeyEvents = KeyEvents or { KeyDown = 256, KeyUp = 257 }
Keys = Keys or enum{ VK_RETURN = 13, VK_ESCAPE = 27, VK_TAB = 9, VK_SHIFT = 16, VK_CONTROL = 17 }

-- Events: Add keeps the handlers; __fire_event runs them, as the game core does.
local handlers = {}
Events = setmetatable({}, { __index = function(t, name)
	local ev = {
		Add = function(f) handlers[name] = handlers[name] or {}; table.insert(handlers[name], f) end,
		Remove = function() end,
		Call = function(...) for _, f in ipairs(handlers[name] or {}) do f(...) end end,
	}
	setmetatable(ev, { __call = function(self, ...) return self.Call(...) end })
	rawset(t, name, ev)
	return ev
end })
function __fire_event(name, ...)
	for _, f in ipairs(handlers[name] or {}) do f(...) end
end
-- The game core announces one notification right after the game starts, so the panel has something to show.
local fire_event = __fire_event
function __fire_event(name, ...)
	fire_event(name, ...)
	if name == "SequenceGameInitComplete" then
		fire_event("NotificationAdded", 1, NotificationTypes.NOTIFICATION_GENERIC,
			"Your Warrior has finished exploring the nearby land.", "Exploration finished", 0, 0, 0)
	end
end
LuaEvents = Events

ContentType = ContentType or enum{ GAMEPLAY = 1, SCENARIO = 2 }
ContentManager = ContentManager or api{ IsActive = function() return false end }
DB = api{ Query = function() return function() return nil end end }
Locale = Locale or api{
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
-- The exe API of `exe-api` (OptionsManager, Modding, ContentManager, Mouse, Keys, ...) is put in before these stubs when the
-- client runs them; each line below only fills in what is still missing, so the tests of `ui`, which have no `exe-api`, keep working.
PreGame = PreGame or api{ IsMultiplayerGame = function() return false end, IsHotSeatGame = function() return false end }
TradeableItems = enum()
Map = api{ GetPlot = function() return api() end, GetNumPlots = function() return 0 end }
UI = api{ GetScratchDeal = function() return api() end }
Players = setmetatable({}, { __index = function() return api() end })
Teams = setmetatable({}, { __index = function() return api() end })
OptionsManager = OptionsManager or api()
Network = api()
Steam = Steam or api()
Matchmaking = Matchmaking or api()
InterfaceDirective = enum()
InterfaceModeTypes = enum()
ButtonPopupTypes = enum()
NotificationTypes = enum()
EndTurnBlockingTypes = enum()
ActionSubTypes = enum()
Modding = Modding or api()
Controls = Controls or {}

function Color(r, g, b, a) return { x = r, y = g, z = b, w = a } end
if getmetatable(PreGame) == nil then setmetatable(PreGame, getmetatable(api())) end
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
InStrategicView = InStrategicView or function() return false end
ContentManager.GetAllPackageIDs = rawget(ContentManager, "GetAllPackageIDs") or function() return {} end
Modding.GetActivatedMods = rawget(Modding, "GetActivatedMods") or function() return {} end
MapModData = MapModData or {}
Modding.OpenUserData = rawget(Modding, "OpenUserData") or function()
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
GameEvents = setmetatable({}, { __index = function(t, k)
	local ev = { Add = function() end, Remove = function() end }
	rawset(t, k, ev)
	return ev
end })
IsGameCoreBusy = function() return false end
