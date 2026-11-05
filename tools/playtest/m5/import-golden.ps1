# M5 WO-I (AC-S2): the saves of the original's golden scenes (assets-out/goldens-b4-ui/S4, S5: .Civ5Save of the MP game of the capture)
# as sleeping games of a server of ~/m5-i in WSL: `import_c5sv` (tools/golden/src/bin) makes the C5SV file of the core's import
# container, this script writes the game's `meta.json` and copies the game directory to ~/m5-i/<State>/games/<n>/. The server of that
# state dir is started after (tools/playtest/m5/woi-server.ps1 -State state-<port> without -Fresh); the human of the save sits under
# <Nick> (a nick the page logs in with, dev password), and a [Take] of that seat wakes the game from the save (scenario m5-goldens-ui).
#   .\tools\playtest\m5\import-golden.ps1 -Scene S5-trade-ai -Nick g5 -Game 1 -State state-8862 -Fresh
param(
  [Parameter(Mandatory)][string]$Scene,          # S4-tech-tree | S5-trade-ai | S1-map-flags-banners ... (the file name in goldens-b4-ui)
  [Parameter(Mandatory)][string]$Nick,
  [int]$Game = 1,
  [string]$State = "state-8862",
  [string]$Golden = $env:CIV5_GOLDEN_DIR,   # the golden scenes (saves of the original)
  [string]$Lekmod = $env:LEKMOD_ROOT,       # the gamecore checkout
  # the core build of the server that wakes the game (the `core_build` of any of its meta.json files)
  [string]$BuildId = "lekmod-portable/dev-fx40-e7caefa5/dll 1.0.0/guid 13c6ca53-50ff-4a66-941b/data d1eebd98ea0382f8",
  [string]$Human = "CIVILIZATION_ETHIOPIA",
  [string]$Ai = "CIVILIZATION_MACEDONIA",
  [int]$MinorCivs = 4,
  [switch]$Fresh
)
$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
$bin = Join-Path $root "target\debug\import_c5sv.exe"
if (-not (Test-Path $bin)) { throw "build it first: cargo build -p golden --bin import_c5sv" }
$out = Join-Path $root "target\import\$Scene"
Remove-Item $out -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $out | Out-Null
$line = & $bin --save (Join-Path $Golden "$Scene.Civ5Save") --civsim (Join-Path $Lekmod "headless\tools") --out $out `
  --build-id $BuildId --nick $Nick --game $Game --human $Human --ai $Ai --world-size WORLDSIZE_DUEL `
  --map-script "assets/maps/pangaea.lua" --handicap HANDICAP_PRINCE --speed GAMESPEED_QUICK --era ERA_ANCIENT --minor-civs $MinorCivs
if ($LASTEXITCODE -ne 0) { throw "import_c5sv failed" }
$turn = [int](([regex]::Match("$line", 'turn=(\d+)')).Groups[1].Value)
Remove-Item (Join-Path $out "extract") -Recurse -Force   # the pregame holds nicknames: nothing of it stays
$meta = [ordered]@{
  core_build = $BuildId; created_ms = [int64]1793363482234; finished_ms = $null; format = 1; host = $Nick; id = $Game; name = "golden $Scene"
  seats = @(
    [ordered]@{ civ = [ordered]@{ Civ = $Human }; kicked = $null; nick = $Nick; player = 0 },
    [ordered]@{ civ = [ordered]@{ Civ = $Ai }; kicked = $null; nick = $null; player = 1 })
  seed = 0
  settings = [ordered]@{
    auto_choices = [ordered]@{ archaeology = $true; captured_city = $true; goody_hut = $true; great_person = $true; ideology = $true; league = $true; policy = $true; promotion = $true; religion = $true; research = $true; steal_tech = $true }
    diplo_auto_decline = $true; game_options = @(); handicap = "HANDICAP_PRINCE"; map_options = @(); map_script = "assets/maps/pangaea.lua"; map_seed = $null
    minor_civs = $MinorCivs; name = "golden $Scene"; seed = 0; speed = "GAMESPEED_QUICK"; start_era = "ERA_ANCIENT"; timer = "Off"; turn_mode = "Simultaneous"; world_size = "WORLDSIZE_DUEL"
  }
  status = "Hibernating"; turn = $turn
}
[IO.File]::WriteAllText((Join-Path $out "meta.json"), ($meta | ConvertTo-Json -Depth 8) + "`n")
$d = "Ubuntu-24.04"
$src = "/mnt/" + $out.Substring(0, 1).ToLower() + $out.Substring(2).Replace("\", "/")
$wipe = if ($Fresh) { "rm -rf ~/m5-i/$State; " } else { "" }
$ids = "{`"format`": 1, `"next_game`": $($Game + 1), `"next_room`": 1}"
wsl -d $d -- bash -lc "$wipe mkdir -p ~/m5-i/$State/games/$Game && rm -rf ~/m5-i/$State/games/$Game/* && cp -r $src/. ~/m5-i/$State/games/$Game/ && [ -f ~/m5-i/$State/ids.json ] || echo '$ids' > ~/m5-i/$State/ids.json; ls -R ~/m5-i/$State/games/$Game | head -8"
"imported $Scene as game $Game (turn $turn), nick $Nick"
