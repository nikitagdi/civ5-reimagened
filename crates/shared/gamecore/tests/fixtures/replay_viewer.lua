-- The data the end-game ReplayViewer collects on show (GenerateReplayInfoFromCurrentGame, Lekmod ReplayViewer.lua).
function ReplayProbe()
  local log = {}
  local function step(name, f)
    local ok, err = pcall(f)
    log[#log + 1] = name .. "=" .. (ok and "ok" or tostring(err))
    if TraceStep then TraceStep(name) end
  end
  step("GetStartEra", function() return Game.GetStartEra() end)
  step("GetGameSpeedType", function() return Game.GetGameSpeedType() end)
  step("GetCalendar", function() return Game.GetCalendar() end)
  step("GetStartTurn", function() return Game.GetStartTurn() end)
  step("GetStartYear", function() return Game.GetStartYear() end)
  step("GetGridSize", function() return Map.GetGridSize() end)
  -- The history getters create the data set they look up: a player nothing was recorded for has an empty one. Reading it
  -- used to abort the process (lua_createtable with size() - 1 = -1).
  for i = 0, 63 do
    local pl = Players[i]
    if pl ~= nil and pl:IsEverAlive() then pl:GetScoreHistory(0) end
  end
  local sets, empty = 0, 0
  for i = 0, 63 do
    local pl = Players[i]
    if pl ~= nil and pl:IsEverAlive() then
      step("GetReplayData" .. i, function()
        local d = pl:GetReplayData()
        for ds, v in pairs(d) do
          sets = sets + 1
          local n = 0
          for _ in pairs(v) do n = n + 1 end
          if n == 0 then empty = empty + 1 end
        end
      end)
      step("Info" .. i, function()
        local _ = pl:GetPlayerColor(), pl:GetHandicapType(), pl:GetName(), pl:GetCivilizationDescription(),
          pl:GetCivilizationShortDescription(), pl:GetCivilizationAdjective()
      end)
    end
  end
  -- The deal lists of the Diplomatic Overview: the count, and every deal loads into the scratch deal with the player in it.
  step("Deals", function()
    local cur, hist = 0, 0
    for i = 0, 63 do
      local pl = Players[i]
      if pl ~= nil and pl:IsEverAlive() and not pl:IsMinorCiv() and not pl:IsBarbarian() then
        for _, kind in ipairs({ "Current", "Historic" }) do
          local n = _G["GcUiGetNum" .. kind .. "Deals"](i)
          assert(type(n) == "number" and n >= 0)
          for k = 0, n - 1 do
            _G["GcUiLoad" .. kind .. "Deal"](i, k)
            local d = GcUiGetScratchDeal()
            assert(d:GetFromPlayer() == i or d:GetToPlayer() == i, kind .. " deal " .. k .. " of " .. i)
          end
          if kind == "Current" then cur = cur + n else hist = hist + n end
        end
        _G["GcUiLoadCurrentDeal"](i, 9999)
        _G["GcUiLoadHistoricDeal"](i, -1)
      end
    end
    log[#log + 1] = "deals=" .. cur .. "/" .. hist
  end)
  step("GetReplayMessages", function()
    local m = Game.GetReplayMessages()
    log[#log + 1] = "messages=" .. #m
  end)
  step("Plots", function()
    for i = 0, Map.GetNumPlots() - 1 do
      local p = Map.GetPlotByIndex(i)
      local _ = p:GetTerrainType(), p:GetFeatureType()
    end
  end)
  return sets, empty, table.concat(log, "\n")
end

-- Every read the end-game replay makes of a finished game, in one pass (our own stand-in for the screen's collector): the
-- start info, each player's names and types looked up in GameInfo, the replay data sets regrouped per turn, the replay
-- messages with all their fields, and the terrain and feature of every plot.
function ReplayFull()
  local w, h = Map.GetGridSize()
  local info = {
    Era = Game.GetStartEra(), Speed = Game.GetGameSpeedType(), Calendar = Game.GetCalendar(),
    First = Game.GetStartTurn(), Last = Game.GetGameTurn(), Year = Game.GetStartYear(), W = w, H = h,
  }
  local index = { [-1] = -1 }
  local players = {}
  for i = 0, GameDefines.MAX_CIV_PLAYERS - 1 do
    local pl = Players[i]
    if pl and pl:IsEverAlive() then
      local byTurn = {}
      for set, values in pairs(pl:GetReplayData()) do
        for t, v in pairs(values) do
          byTurn[t] = byTurn[t] or {}
          byTurn[t][set] = v
        end
      end
      players[#players + 1] = {
        Civ = GameInfo.Civilizations[pl:GetCivilizationType()].Type,
        Leader = GameInfo.Leaders[pl:GetLeaderType()].Type,
        Color = GameInfo.PlayerColors[pl:GetPlayerColor()].Type,
        Handicap = GameInfo.HandicapInfos[pl:GetHandicapType()].Type,
        Name = pl:GetName(), Desc = pl:GetCivilizationDescription(),
        Short = pl:GetCivilizationShortDescription(), Adj = pl:GetCivilizationAdjective(),
        Scores = byTurn,
      }
      index[i] = #players
    end
  end
  info.Players = players
  info.Active = index[Game.GetActivePlayer()]
  local msgs = {}
  for _, m in ipairs(Game.GetReplayMessages()) do
    msgs[#msgs + 1] = { m.Turn, m.Text, m.Type, index[m.Player], m.Plots, m.Timestamp }
  end
  info.Messages = msgs
  local plots = {}
  for i = 0, Map.GetNumPlots() - 1 do
    local p = Map.GetPlotByIndex(i)
    plots[i + 1] = { p:GetTerrainType(), p:GetFeatureType() }
  end
  info.Plots = plots
  return info
end
