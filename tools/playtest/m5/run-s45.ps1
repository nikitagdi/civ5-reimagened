# M5 WO-I: S4 + S5 from scratch (the nicks g4/g5 register on first login, so the state is imported again for each run):
# stop the server of the port, import both saves, start it, run the scenario. Needs target\debug\import_c5sv.exe and ui_flip.exe.
param([int]$Port = 8862, [string]$Captures = "S5,S4")
$ErrorActionPreference = "Stop"
$here = $PSScriptRoot
& powershell -ExecutionPolicy Bypass -File "$here\woi-server.ps1" -Port $Port -Stop | Out-Null
pwsh -NoProfile -File "$here\import-golden.ps1" -Scene S5-trade-ai -Nick g5 -Game 1 -State "state-$Port" -Fresh | Select-Object -Last 1
pwsh -NoProfile -File "$here\import-golden.ps1" -Scene S4-tech-tree -Nick g4 -Game 2 -State "state-$Port" | Select-Object -Last 1
& powershell -ExecutionPolicy Bypass -File "$here\woi-server.ps1" -Port $Port -GameData gamedata-b2 -State state
Set-Location (Resolve-Path "$here\..\..\..")
node tools/playtest/run.mjs --attach "http://127.0.0.1:$Port" --scenario m5-goldens-s45 --golden-captures $Captures --out target/playtest
