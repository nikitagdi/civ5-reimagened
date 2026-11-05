-- The game the end-game replay viewer reads on show (after the M1 stubs): two players, a small map, the replay data sets.
local sets = { "REPLAYDATASET_SCORE", "REPLAYDATASET_CITYCOUNT", "REPLAYDATASET_TOTALGOLD" }
local rows = {}
for i, s in ipairs(sets) do rows[i] = { ID = i - 1, Type = s, Description = "TXT_" .. s } end
GameInfo.ReplayDataSets = setmetatable({}, {
	__call = function() local i = 0; return function() i = i + 1; return rows[i] end end,
	__index = function(_, k) if type(k) == "number" then return rows[k + 1] end return rows[1] end })
GameDefines.MAX_CIV_PLAYERS = 4
Game.GetStartEra = function() return 0 end
Game.GetGameSpeedType = function() return 0 end
Game.GetCalendar = function() return 0 end
Game.GetStartTurn = function() return 0 end
Game.GetStartYear = function() return -4000 end
Game.GetGameTurn = function() return 143 end
Game.GetReplayMessages = function()
	local m = {}
	for i = 1, 40 do m[i] = { Turn = i * 3, Text = "message " .. i, Type = 0, Player = i % 2, Plots = { { X = 1, Y = 2 } }, Timestamp = i } end
	return m
end
Map.GetGridSize = function() return 40, 24 end
Map.GetNumPlots = function() return 40 * 24 end
Map.GetPlotByIndex = function(i)
	return { GetTerrainType = function() return i % 4 end, GetFeatureType = function() return -1 end }
end
local function player(id)
	local data = {}
	for _, s in ipairs(sets) do data[s] = {}; for t = 0, 143 do data[s][t] = t * (id + 1) end end
	return {
		IsEverAlive = function() return true end, GetID = function() return id end,
		GetCivilizationType = function() return 0 end, GetLeaderType = function() return 0 end,
		GetPlayerColor = function() return 0 end, GetHandicapType = function() return 0 end,
		GetName = function() return "Leader" .. id end, GetCivilizationDescription = function() return "Civ" .. id end,
		GetCivilizationShortDescription = function() return "Civ" .. id end, GetCivilizationAdjective = function() return "Civish" end,
		GetReplayData = function() return data end,
	}
end
local ps = { [0] = player(0), [1] = player(1), [2] = player(2), [3] = player(3) }
Players = setmetatable(ps, { __index = function() return nil end })
GameInfo.PlayerColors = setmetatable({}, { __index = function(_, k)
	return { ID = 0, Type = "PLAYERCOLOR_" .. tostring(k), PrimaryColor = "COLOR_P" .. tostring(k), SecondaryColor = "COLOR_S" .. tostring(k) }
end })
GameInfo.Colors = setmetatable({}, { __index = function(_, k)
	local n = #tostring(k)
	return { Type = k, Red = (n * 37 % 100) / 100, Green = (n * 53 % 100) / 100, Blue = (n * 71 % 100) / 100, Alpha = 1 }
end })
