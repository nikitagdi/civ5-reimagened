# M3 WO-B: a DEV copy of the game data whose lekmod_policies.lua grants the human player +30 faith and +25 culture on
# turn 3 (GameEvents.PlayerDoTurn). The scenario `m3-tech-policy-religion` needs it: a pantheon costs 10 faith and the
# first policy 25 culture, which a short game does not collect. Never shipped; the server must run with -GameData of the copy:
#   .\tools\playtest\m3\make-dev-gamedata.ps1 -Src <assets-out\gamedata-b2> -Out D:\m3\gamedata
#   .\tools\playtest\m2\server.ps1 -Port 8720 -GameData D:\m3\gamedata ...
# The page flag &manual=1 (the scenario sets it) turns the auto choices of the human off.
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
  "        player:ChangeJONSCulture(25)",
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
