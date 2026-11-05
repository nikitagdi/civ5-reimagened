# M2 (WO-6) playtest server: the real-core civ5-server in WSL, serving a web bundle and an asset dir.
#   .\tools\playtest\m2\server.ps1 -Port 8300 -Web <web bundle> -Assets <asset dir> -State <state dir> -GameData <export-gamedata dir> -GameCore <gamecore archives>   (stop: add -Stop)
# The defaults of -GameData, -GameCore and -Lekmod are CIV5_GAMEDATA, CIV5_GAMECORE_LIB (the folder with linux/libgamecore.a)
# and LEKMOD_ROOT.
# Builds civ5-server in WSL if needed (release, target $HOME/target-wo6, real libgamecore from -GameCore/linux), starts it in
# the background, waits for /healthz and prints the URL. State dir = the games/saves of this run (-Fresh wipes it first).
# The page: <url>/?test=1&nick=NICK&autostart=1&ai=2&size=small&seed=7&mode=simultaneous  (dev password: dev).
# The server log: <State>\server.log, the pid: <State>\server.pid (WSL pid).
param(
  [int]$Port = 8300,
  [Parameter(Mandatory)][string]$Web,
  [Parameter(Mandatory)][string]$Assets,
  [Parameter(Mandatory)][string]$State,
  [string]$GameData = $env:CIV5_GAMEDATA,
  [string]$GameCore = $env:CIV5_GAMECORE_LIB,
  [string]$Core = "real",
  [string]$Lekmod = $env:LEKMOD_ROOT, # optional: the gamecore checkout (LEKMOD_ROOT of the build)
  [string]$TargetName = "target-wo6", # WSL cargo target dir name under $HOME (use another for another -Source)
  [string]$Source = "",          # the repo to build the server from (default: this worktree)
  [switch]$Fresh,                # wipe the state dir first
  [switch]$Rebuild,              # run cargo build even if the binary exists
  [switch]$Stop
)
$ErrorActionPreference = "Stop"
$distro = if ($env:CIV5_WSL_DISTRO) { $env:CIV5_WSL_DISTRO } else { "Ubuntu-24.04" }
if ($Core -eq "real" -and (-not $GameData -or -not $GameCore)) { throw "the real core needs -GameData and -GameCore (or CIV5_GAMEDATA and CIV5_GAMECORE_LIB)" }
if (-not $Source) { $Source = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path }
function W([string]$p) { $p = (Resolve-Path $p).Path; "/mnt/" + $p.Substring(0, 1).ToLower() + $p.Substring(2).Replace("\", "/") }
function RunWsl([string]$cmd) { & wsl -d $distro -- bash -lc $cmd; if ($LASTEXITCODE -ne 0) { throw "wsl failed ($LASTEXITCODE): $cmd" } }

New-Item -ItemType Directory -Force $State | Out-Null
$pidFile = Join-Path $State "server.pid"
if (Test-Path $pidFile) {
  $old = (Get-Content $pidFile -Raw).Trim()
  if ($old) { & wsl -d $distro -- bash -lc "kill $old 2>/dev/null; sleep 0.5; kill -9 $old 2>/dev/null; true" | Out-Null }
  Remove-Item $pidFile -Force
}
if ($Stop) { "stopped (port $Port)"; return }

$tbin = "`$HOME/$TargetName/release/civ5-server"
$env_ = 'export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0 WASI_SDK=${WASI_SDK:-$HOME/wasi-sdk-33}'
$env_ += " CARGO_TARGET_DIR=`$HOME/$TargetName"
if ($Core -eq "real") { $env_ += " GAMECORE_LIB_DIR_x86_64=" + (W (Join-Path $GameCore "linux")) }
if ($Lekmod) { $env_ += " LEKMOD_ROOT=" + (W $Lekmod) }
$src = W $Source
$have = "$(& wsl -d $distro -- bash -lc "test -x $tbin && echo yes")".Trim()
if ($Rebuild -or $have -ne "yes") {
  Write-Host "building civ5-server in WSL ($src) ..."
  # A script file: wsl.exe would expand $PATH (with the spaces of the Windows PATH) in a -lc string.
  $bsh = Join-Path $State "build.sh"
  [IO.File]::WriteAllText($bsh, "$env_`ncd '$src'`ncargo build --release -p server --bin civ5-server 2>&1 | tail -n 3`n")
  RunWsl "bash '$(W $bsh)'"
}
if ($Fresh) { Remove-Item (Join-Path $State "*") -Recurse -Force -ErrorAction SilentlyContinue }
New-Item -ItemType Directory -Force $State | Out-Null
$flags = "--dev --core $Core --listen 127.0.0.1:$Port --admin-port $($Port + 1) --state-dir '$(W $State)' --web-root '$(W $Web)' --assets-root '$(W $Assets)'"
if ($Core -eq "real") { $flags += " --gamedata-dir '$(W $GameData)'" }
$log = "$(W $State)/server.log"
$pf = "$(W $State)/server.pid"
$launch = Join-Path $State "launch.sh"
$sh = "$env_`ncd '$src'`nsetsid nohup $tbin $flags > '$log' 2>&1 < /dev/null &`necho `$! > '$pf'`n"
[IO.File]::WriteAllText($launch, $sh)
RunWsl "bash '$(W $launch)'"
$url = "http://127.0.0.1:$Port"
$logWin = Join-Path $State "server.log"
$t0 = Get-Date
$ready = $false
while (((Get-Date) - $t0).TotalSeconds -lt 180) {
  Start-Sleep -Milliseconds 500
  $txt = if (Test-Path $logWin) { Get-Content $logWin -Raw } else { "" }
  # Our own server only: another program may already own the port (the healthz of that one would answer).
  if ($txt -match "failed to bind|panicked|Error:") { Get-Content $logWin -Tail 15; throw "server failed to start (log above): $logWin" }
  if ($txt -match "lobby is open") { $ready = $true; break }
}
if (-not $ready) { throw "server not ready in 180 s: $logWin" }
try { $null = Invoke-WebRequest "$url/healthz" -UseBasicParsing -TimeoutSec 5 } catch { throw "server up but $url/healthz does not answer: $_" }
"$url/?test=1&nick=NICK&autostart=1&ai=2&size=small&seed=7&mode=simultaneous"
