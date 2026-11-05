# M5 WO-H: a server state dir with the games of the golden saves (S1..S5 of assets-out/goldens-b4-ui), as sleeping games:
#   game 1 (nick m5a): the save of S1, S2, S5 (turn 25 of the original's MP game, Ethiopia vs Macedonia)
#   game 2 (nick m5b): the save of S3, S4 (turn 5)
# Logging in as the nick wakes the game from the save (the B2 civ5save import: `tools/save-import`, the python extractor of
# Lekmod `headless/tools`). Run the server on the result (tools/playtest/m5/golden-server.ps1 copies it into WSL and starts it).
#   .\tools\playtest\m5\make-golden-state.ps1 -Out target\m5\state -Ref <any .c5sv of the server build>
param(
  [Parameter(Mandatory)][string]$Out,
  [Parameter(Mandatory)][string]$Ref,   # a .c5sv of the same server build + game data (its header names the core build)
  [string]$Goldens = $env:CIV5_GOLDEN_DIR,                                   # the golden scenes (saves of the original)
  [string]$Civsim = $(if ($env:LEKMOD_ROOT) { Join-Path $env:LEKMOD_ROOT "headless\tools" } else { "" })
)
if (-not $Goldens -or -not $Civsim) { throw "pass -Goldens and -Civsim (or set CIV5_GOLDEN_DIR and LEKMOD_ROOT)" }
$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
$env:CARGO_TARGET_DIR = Join-Path $root "target\native"
Push-Location $root
cargo build -p save-import 2>&1 | Select-Object -Last 2
Pop-Location
$exe = Join-Path $env:CARGO_TARGET_DIR "debug\save-import.exe"
if (Test-Path $Out) { Remove-Item -Recurse -Force $Out }
$common = @("--state", $Out, "--civ", "CIVILIZATION_ETHIOPIA", "--ai", "CIVILIZATION_MACEDONIA", "--world", "WORLDSIZE_DUEL", "--map-script", "Assets\Maps\Pangaea.lua", "--speed", "GAMESPEED_QUICK", "--handicap", "HANDICAP_PRINCE", "--build-id-from", $Ref, "--civsim", $Civsim)
& $exe --save "$Goldens\S1-map-flags-banners.Civ5Save" --nick m5a --game 1 @common
& $exe --save "$Goldens\S3-city-screen.Civ5Save" --nick m5b --game 2 @common
