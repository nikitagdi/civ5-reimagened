# M4 WO-G: a DEV copy of the game data whose lekmod_policies.lua ends the game on a turn: on that turn (default 3) the
# human player's team wins a time victory (GameEvents.PlayerDoTurn -> Game.SetWinner). The scenario `m4-menus-endgame`
# needs it to see the end-game screen without playing to a real victory. Never shipped; the server must run with
# -GameData of the copy:
#   .\tools\playtest\m4\make-dev-gamedata.ps1 -Src <assets-out\gamedata-b2> -Out <out-dir>\gamedata [-Turn 3]
#   .\tools\playtest\m2\server.ps1 -Port 8840 -GameData <out-dir>\gamedata ...
# (same manifest/lock/index patching as tools/playtest/m3/make-dev-gamedata.ps1)
param([Parameter(Mandatory)][string]$Src, [Parameter(Mandatory)][string]$Out, [int]$Turn = 3)
$ErrorActionPreference = "Stop"
Copy-Item -Recurse $Src $Out
$rel = "assets/dlc/lekmod_v34.15/lua/lekmod_policies.lua"
$f = Join-Path $Out $rel
$hook = @(
  "",
  "-- DEV (M4 WO-G): the human's team wins a time victory on turn $Turn",
  "GameEvents.PlayerDoTurn.Add(function(player_id)",
  "    local ok, err = pcall(function()",
  "        local player = Players[player_id]",
  "        if player and player:IsHuman() and Game.GetGameTurn() >= $Turn and not M4_DEV_WON then",
  "            M4_DEV_WON = true",
  "            player:ChangeGold(500) -- a mark the playtest can see: the hook ran",
  "            Game.SetWinner(player:GetTeam(), GameInfoTypes.VICTORY_TIME)",
  "        end",
  "    end)",
  "    if not ok then print('M4 DEV victory hook: ' .. tostring(err)) end",
  "end)"
)
Add-Content $f $hook -Encoding utf8
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
