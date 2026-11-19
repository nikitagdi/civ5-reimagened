-- Acceptance fixture for live UI checks: a mid-game state for seat 0, run once by the server at the first human phase
-- when it is started with CIV5_TEST_LUA=<this file> (see crates/server/server/src/worker/mod.rs). It uses only the
-- game's own Lua API (the bindings the screen scripts read): every section is guarded so that one refusal leaves the
-- others in place, and the chunk ends with an error naming what failed (the server logs it).
local p = Players[0]
local team = Teams[p:GetTeam()]

local failed = {}
local function step(name, f)
  local ok, err = pcall(f)
  if not ok then failed[#failed + 1] = name .. ": " .. tostring(err) end
end

-- The capital where the first settler stands, and three more cities around it (trade routes, city list, economy).
local capital
step("cities", function()
  local settler
  for u in p:Units() do
    if u:IsFound() then settler = u break end
  end
  local x, y = settler:GetX(), settler:GetY()
  capital = p:InitCity(x, y)
  settler:Kill(true, -1)
  local made = 0
  for _, d in ipairs({{5, 0}, {-5, 0}, {2, 5}, {-2, -5}, {3, -5}, {-3, 5}, {6, 3}, {-6, -3}}) do
    if made >= 3 then break end
    local plot = Map.GetPlot(x + d[1], y + d[2])
    if plot and not plot:IsWater() and not plot:IsMountain() and plot:GetOwner() == -1 and not plot:IsImpassable() then
      local city = p:InitCity(plot:GetX(), plot:GetY())
      if city then
        city:SetPopulation(4, true)
        made = made + 1
      end
    end
  end
  capital:SetPopulation(9, true)
end)

-- Everyone met (diplomacy, deals, leaders, city-states, the ribbon).
step("meet", function()
  for id = 0, GameDefines.MAX_CIV_PLAYERS - 1 do
    local q = Players[id]
    if id ~= 0 and q and q:IsAlive() then team:Meet(q:GetTeam(), true) end
  end
end)

-- Techs of the first four eras (spies, archaeology later; trade units, great works, the World Congress's printing press).
step("techs", function()
  local eras = {ERA_ANCIENT = true, ERA_CLASSICAL = true, ERA_MEDIEVAL = true, ERA_RENAISSANCE = true}
  for t in GameInfo.Technologies() do
    if eras[t.Era] then team:SetHasTech(t.ID, true) end
  end
end)

-- Treasury, culture and faith to spend; a few policies adopted.
step("yields", function()
  p:ChangeGold(3000)
  p:ChangeJONSCulture(4000)
  p:ChangeFaith(800)
end)
step("policies", function()
  for _, branch in ipairs({"POLICY_BRANCH_TRADITION", "POLICY_BRANCH_LIBERTY"}) do
    local b = GameInfo.Policy_Branches and GameInfoTypes[branch]
    if b then p:SetPolicyBranchUnlocked(b, true, false) end
  end
  for _, name in ipairs({"POLICY_TRADITION", "POLICY_ARISTOCRACY", "POLICY_LIBERTY"}) do
    local id = GameInfoTypes[name]
    if id then p:SetHasPolicy(id, true) end
  end
end)

-- Strategic resources to show in the top strip and the unit costs (horses, iron).
step("resources", function()
  for _, name in ipairs({"RESOURCE_HORSE", "RESOURCE_IRON"}) do
    local id = GameInfoTypes[name]
    if id then p:ChangeNumResourceTotal(id, 4) end
  end
end)

-- A pantheon and a religion founded in the capital.
step("religion", function()
  local pantheon, founder, follower
  for b in GameInfo.Beliefs() do
    if b.Pantheon and not pantheon then pantheon = b.ID end
    if b.Founder and not founder then founder = b.ID end
    if b.Follower and not follower then follower = b.ID end
  end
  Game.FoundPantheon(0, pantheon)
  local religion
  for r in GameInfo.Religions() do
    if r.Type ~= "RELIGION_PANTHEON" and not religion then religion = r.ID end
  end
  Game.FoundReligion(0, religion, nil, founder, follower, -1, -1, capital)
end)

-- Units with something to do: a worker (many actions), a caravan, great people, an archaeologist, a damaged warrior.
step("units", function()
  local x, y = capital:GetX(), capital:GetY()
  for _, name in ipairs({"UNIT_WORKER", "UNIT_CARAVAN", "UNIT_CARGO_SHIP", "UNIT_GREAT_SCIENTIST", "UNIT_ARTIST",
      "UNIT_WRITER", "UNIT_ARCHAEOLOGIST", "UNIT_KNIGHT", "UNIT_CROSSBOWMAN"}) do
    local id = GameInfoTypes[name]
    if id then p:InitUnit(id, x, y) end
  end
  for u in p:Units() do
    if u:GetUnitType() == GameInfoTypes.UNIT_WARRIOR then
      u:ChangeDamage(45)
      u:ChangeExperience(30)
    end
  end
end)
if #failed > 0 then error("mid-game fixture: " .. table.concat(failed, "; ")) end
