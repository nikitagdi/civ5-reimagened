# M5 WO-J: the DEV game data of the scenario `m5-acceptance` (mode "ai", 50 turns against the AI). Never shipped; the server must run
# with -GameData of the copy:
#   .\tools\playtest\m5/make-dev-gamedata-woj.ps1 -Src <assets-out\gamedata-b2> -Out <dir>
# It is the M4 WO-E data (`m4/make-dev-gamedata-woe.ps1`: on turn 3 every major civ gets Printing Press + Animal Husbandry, all majors
# meet, a city of another civ stands 4-6 tiles from the human's capital, the human gets a Caravan and a spy slot) plus the M3 WO-B
# grant (turn 3: +30 faith for a pantheon, +40 culture for the first policy) and +200 gold for a deal. The mode "mp" runs on the plain
# game data (two humans would run the turn-3 block twice).
param([Parameter(Mandatory)][string]$Src, [Parameter(Mandatory)][string]$Out)
$ErrorActionPreference = "Stop"
& (Join-Path $PSScriptRoot "..\m4\make-dev-gamedata-woe.ps1") -Src $Src -Out $Out | Out-Null
$rel = "assets/dlc/lekmod_v34.15/lua/lekmod_policies.lua"
$f = Join-Path $Out $rel
$lock = Join-Path $Out "gamedata.lock"
$oldLua = (Select-String "lekmod_policies" -Path (Join-Path $Out "manifest.sha256") -SimpleMatch | ForEach-Object Line).Split(" ")[0]
$block = @'

-- WO-J dev block (m5/make-dev-gamedata-woj.ps1)
GameEvents.PlayerDoTurn.Add(function(player_id)
    local player = Players[player_id]
    if player and player:IsHuman() and Game.GetGameTurn() == 3 then
        player:ChangeFaith(30)
        player:ChangeJONSCulture(40)
        player:ChangeGold(200)
    end
end)
'@
Add-Content $f $block -Encoding utf8
$newLua = (Get-FileHash $f -Algorithm SHA256).Hash.ToLower()
$size = (Get-Item $f).Length
Set-Content $lock ((Get-Content $lock -Raw).Replace($oldLua, $newLua)) -NoNewline -Encoding utf8
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
