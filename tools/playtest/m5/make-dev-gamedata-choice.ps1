# b4/fix-choices-city (live report 2026-10-30): a DEV copy of the game data for the scenario `choices-city`. Its lekmod_policies.lua
# gives the human on turn 3 +40 culture (a policy to choose) and +30 faith (a pantheon to found), and on turn 5 a Great Prophet in the
# capital (a religion to found with the unit's button: the religion popup). Never shipped; the server runs with --gamedata-dir of the copy:
#   .\tools\playtest\m5\make-dev-gamedata-choice.ps1 -Src <gamedata-plain> -Out <gamedata-choice>
# The scenario leaves the page flag `manual` unset: the original UI must leave the choices to the human by itself.
param([Parameter(Mandatory)][string]$Src, [Parameter(Mandatory)][string]$Out)
$ErrorActionPreference = "Stop"
Copy-Item -Recurse $Src $Out
$rel = "assets/dlc/lekmod_v34.15/lua/lekmod_policies.lua"
$f = Join-Path $Out $rel
$grant = @(
  "",
  "GameEvents.PlayerDoTurn.Add(function(player_id)",
  "    local player = Players[player_id]",
  "    if player and player:IsHuman() and Game.GetGameTurn() == 3 then",
  "        player:ChangeFaith(30)",
  "        player:ChangeJONSCulture(40)",
  "    end",
  "    if player and player:IsHuman() and Game.GetGameTurn() == 5 then",
  "        local cap = player:GetCapitalCity()",
  "        if cap then player:InitUnit(GameInfoTypes.UNIT_PROPHET, cap:GetX(), cap:GetY()) end",
  "    end",
  "end)"
)
Add-Content $f $grant -Encoding utf8
$new = (Get-FileHash $f -Algorithm SHA256).Hash.ToLower()
$size = (Get-Item $f).Length
$manifest = Join-Path $Out "manifest.sha256"
$old = (Select-String "lekmod_policies" -Path $manifest -SimpleMatch | ForEach-Object Line).Split(" ")[0]
$lock = Join-Path $Out "gamedata.lock"
(Get-Content $lock -Raw).Replace($old, $new) | Set-Content $lock -NoNewline -Encoding utf8
$idx = Join-Path $Out "vfs-index.json"
$pattern = '("path":\s*"' + [regex]::Escape($rel) + '",\s*"id":\s*"[^"]*",\s*"size":\s*)\d+'
[regex]::Replace((Get-Content $idx -Raw), $pattern, ('${1}' + $size)) | Set-Content $idx -NoNewline -Encoding utf8
$lines = Get-Content $manifest | ForEach-Object {
  $c = $_ -split "\s+", 2
  if ($c[1] -eq $rel) { "$new  $($c[1])" }
  elseif ($c[1] -in "gamedata.lock", "vfs-index.json") { "$((Get-FileHash (Join-Path $Out $c[1]) -Algorithm SHA256).Hash.ToLower())  $($c[1])" }
  else { $_ }
}
[IO.File]::WriteAllText($manifest, (($lines -join "`n") + "`n"))
"dev game data: $Out"
