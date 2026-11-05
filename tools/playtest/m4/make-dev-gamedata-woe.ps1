# M4 WO-E: a DEV copy of the game data for the scenario `m4-congress-spies-routes`. Never shipped; the server must run with
# -GameData of the copy:
#   .\tools\playtest\m4\make-dev-gamedata-woe.ps1 -Src <assets-out\gamedata-b2> -Out <out-dir>\gamedata
#   .\tools\playtest\m2\server.ps1 -Port 8820 -GameData <out-dir>\gamedata ...
# What it changes (so the congress, the spies and a caravan do not take 100 turns):
#  - lekmod_policies.lua gets a block that on turn 3 (PlayerDoTurn of the human): gives every major civ's team Printing Press
#    (World Congress + Renaissance, which grants the spies) and Animal Husbandry (Caravan), makes all majors meet, reveals the
#    other capitals to the human, founds a city of another civ 4-6 tiles from the human's capital (a trade route needs a city of
#    another team in range; a spy can only move into a revealed city) and gives the human a Caravan in the capital.
# The database is NOT changed (the replica reads its own gameplay.db from the asset set, a different one is refused as "another
# gamecore build"), so the first congress session comes 24 turns (standard speed) after the founding.
param([Parameter(Mandatory)][string]$Src, [Parameter(Mandatory)][string]$Out)
$ErrorActionPreference = "Stop"
Copy-Item -Recurse $Src $Out
$rel = "assets/dlc/lekmod_v34.15/lua/lekmod_policies.lua"
$f = Join-Path $Out $rel
$lock = Join-Path $Out "gamedata.lock"
$lockText = Get-Content $lock -Raw
$oldLua = (Select-String "lekmod_policies" -Path (Join-Path $Out "manifest.sha256") -SimpleMatch | ForEach-Object Line).Split(" ")[0]
$block = @'

-- WO-E dev block (make-dev-gamedata-woe.ps1)
GameEvents.PlayerDoTurn.Add(function(player_id)
    local player = Players[player_id]
    if not (player and player:IsHuman() and Game.GetGameTurn() == 3) then return end
    local pt = GameInfoTypes.TECH_PRINTING_PRESS
    local ah = GameInfoTypes.TECH_ANIMAL_HUSBANDRY
    local teams, others = {}, {}
    for i = 0, GameDefines.MAX_MAJOR_CIVS - 1 do
        local p = Players[i]
        if p and p:IsAlive() then
            teams[#teams + 1] = p:GetTeam()
            if i ~= player_id then others[#others + 1] = i end
        end
    end
    for _, t in ipairs(teams) do
        local team = Teams[t]
        team:SetHasTech(pt, true)
        team:SetHasTech(ah, true)
        if team:GetCurrentEra() < GameInfoTypes.ERA_RENAISSANCE then team:SetCurrentEra(GameInfoTypes.ERA_RENAISSANCE) end
        for _, o in ipairs(teams) do
            if o ~= t then team:Meet(o, true) end
        end
    end
    local myteam = player:GetTeam()
    -- the capitals of the others are known to the human (a spy may move into a revealed city only)
    for _, o in ipairs(others) do
        local cc = Players[o]:GetCapitalCity()
        if cc then cc:Plot():SetRevealed(myteam, true) end
    end
    local cap = player:GetCapitalCity()
    if not cap then return end
    local cx, cy = cap:GetX(), cap:GetY()
    -- a city of the first other civ 4-6 tiles from the capital: a trade destination in range (a route needs a city of another
    -- team) and a city to send a spy to; revealed to the human
    local owner = others[1]
    local placed = false
    for r = 4, 6 do
        for dx = -r, r do
            for dy = -r, r do
                local x, y = cx + dx, cy + dy
                local plot = Map.GetPlot(x, y)
                if owner and not placed and plot and not plot:IsWater() and not plot:IsMountain() and not plot:IsCity()
                    and not plot:IsUnit() and plot:GetOwner() == -1 and Map.PlotDistance(cx, cy, x, y) >= 4 then
                    local c = Players[owner]:InitCity(x, y)
                    if c then
                        placed = true
                        plot:SetRevealed(myteam, true)
                    end
                end
            end
        end
    end
    player:InitUnit(GameInfoTypes.UNIT_CARAVAN, cx, cy)
end)
'@
Add-Content $f $block -Encoding utf8
$newLua = (Get-FileHash $f -Algorithm SHA256).Hash.ToLower()
$size = (Get-Item $f).Length
$lockText = $lockText.Replace($oldLua, $newLua)
Set-Content $lock $lockText -NoNewline -Encoding utf8
$idx = Join-Path $Out "vfs-index.json"
$pattern = '("path":\s*"' + [regex]::Escape($rel) + '",\s*"id":\s*"[^"]*",\s*"size":\s*)\d+'
[regex]::Replace((Get-Content $idx -Raw), $pattern, ('${1}' + $size)) | Set-Content $idx -NoNewline -Encoding utf8
$manifest = Join-Path $Out "manifest.sha256"
$lines = Get-Content $manifest | ForEach-Object {
  $c = $_ -split "\s+", 2
  if ($c[1] -eq $rel) { "$newLua  $($c[1])" }
  elseif ($c[1] -in "gamedata.lock", "vfs-index.json") { "$((Get-FileHash (Join-Path $Out $c[1]) -Algorithm SHA256).Hash.ToLower())  $($c[1])" }
  else { $_ }
}
[IO.File]::WriteAllText($manifest, (($lines -join "`n") + "`n"))
"dev game data: $Out"
