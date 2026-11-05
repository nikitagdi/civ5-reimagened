param([int]$Port = 8860, [string]$GameData = "gamedata-trade", [string]$State = "state", [switch]$Fresh, [switch]$Stop, [string]$Web = "web")
# M5 WO-I: the real-core server of WO-I from ~/m5-i in WSL (copies of the server build, web bundle, assets, game data: WSL is slow on /mnt/<drive>).
$d = if ($env:CIV5_WSL_DISTRO) { $env:CIV5_WSL_DISTRO } else { "Ubuntu-24.04" }
function W([string]$p) { $p = (Resolve-Path $p).Path; "/mnt/" + $p.Substring(0, 1).ToLower() + $p.Substring(2).Replace("\", "/") }
$sh = @"
export PATH=`$HOME/.cargo/bin:`$PATH
cd ~/m5-i
P=`$(cat $State-$Port.pid 2>/dev/null); [ -n "`$P" ] && kill `$P 2>/dev/null; sleep 0.5
[ -n "`$P" ] && kill -9 `$P 2>/dev/null
rm -f $State-$Port.pid
"@
if ($Stop) { $sh += "`necho stopped`n"; $sf = Join-Path $env:TEMP "woi-stop-$Port.sh"; [IO.File]::WriteAllText($sf, ($sh -replace "`r","")); wsl -d $d -- bash -lc "bash '$(W $sf)'"; return }
$sh += "`n"; if ($Fresh) { $sh += "rm -rf ~/m5-i/$State-$Port`n" }
$sh += @"
mkdir -p ~/m5-i/$State-$Port
export GAMECORE_LIB_DIR_x86_64=`$HOME/m5-i/gamecore/linux LEKMOD_ROOT=`$HOME/m5-i/lek
setsid nohup ~/m5-i/target/release/civ5-server --dev --core real --listen 127.0.0.1:$Port --admin-port $($Port + 1) --state-dir ~/m5-i/$State-$Port --web-root ~/m5-i/$Web --assets-root ~/m5-i/assets --gamedata-dir ~/m5-i/$GameData > ~/m5-i/$State-$Port.log 2>&1 < /dev/null &
echo `$! > ~/m5-i/$State-$Port.pid
"@
$f = Join-Path $env:TEMP "woi-launch-$Port.sh"
[IO.File]::WriteAllText($f, ($sh -replace "`r",""))
wsl -d $d -- bash -lc "bash '$(W $f)'"
$t0 = Get-Date
while (((Get-Date) - $t0).TotalSeconds -lt 120) {
  Start-Sleep -Milliseconds 700
  $txt = (& wsl -d $d -- bash -lc "cat ~/m5-i/$State-$Port.log") -join "`n"
  if ($txt -match "failed to bind|panicked|Error:") { $txt; throw "server failed" }
  if ($txt -match "lobby is open") { "http://127.0.0.1:$Port"; return }
}
throw "server not ready"
