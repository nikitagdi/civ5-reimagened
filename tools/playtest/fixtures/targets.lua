-- Acceptance fixture for the map targeting mode (`ui2-targets`): run once by the server at the first human phase when
-- it is started with CIV5_TEST_LUA=<this file>. Seat 0 gets its capital where the first settler stands, a
-- crossbowman in it and, two plots away on land the city sees, a barbarian warrior to shoot at. Only the game's own
-- Lua API; the chunk ends with an error naming what failed (the server logs it).
local p = Players[0]
local failed = {}
local function step(name, f)
  local ok, err = pcall(f)
  if not ok then failed[#failed + 1] = name .. ": " .. tostring(err) end
end

local capital
step("capital", function()
  local settler
  for u in p:Units() do
    if u:IsFound() then settler = u break end
  end
  capital = p:InitCity(settler:GetX(), settler:GetY())
  settler:Kill(true, -1)
  capital:SetPopulation(3, true)
end)

step("archery", function()
  local team = Teams[p:GetTeam()]
  for _, name in ipairs({"TECH_ARCHERY", "TECH_BRONZE_WORKING", "TECH_MATHEMATICS", "TECH_MACHINERY"}) do
    local id = GameInfoTypes[name]
    if id then team:SetHasTech(id, true) end
  end
end)

step("units", function()
  local x, y = capital:GetX(), capital:GetY()
  p:InitUnit(GameInfoTypes.UNIT_CROSSBOWMAN, x, y)
  local placed = false
  for dx = -2, 2 do
    for dy = -2, 2 do
      local plot = Map.PlotXYWithRangeCheck(x, y, dx, dy, 2)
      if not placed and plot and Map.PlotDistance(x, y, plot:GetX(), plot:GetY()) == 2 and not plot:IsWater()
          and not plot:IsMountain() and not plot:IsImpassable() and plot:GetNumUnits() == 0
          and plot:IsVisible(p:GetTeam(), false) and capital:Plot():CanSeePlot(plot, p:GetTeam(), 1, -1) then
        local barb = Players[GameDefines.BARBARIAN_PLAYER or 63]:InitUnit(GameInfoTypes.UNIT_WARRIOR, plot:GetX(), plot:GetY())
        -- No moves this turn: the barbarians' turn starts with the humans' and would walk it into the fog.
        barb:SetMoves(0)
        placed = true
      end
    end
  end
  assert(placed, "no plot for the barbarian")
end)
if #failed > 0 then error("targets fixture: " .. table.concat(failed, "; ")) end
