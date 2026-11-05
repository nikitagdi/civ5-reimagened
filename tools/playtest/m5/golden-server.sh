# Usage: bash tools/playtest/m5/golden-server.sh <state dir made by make-golden-state.ps1, as a WSL path>
STATE_SRC="${1:?pass the state dir of make-golden-state.ps1}"
export PATH=$HOME/.cargo/bin:$PATH
pkill -f 'civ5-server --dev --core real --listen 127.0.0.1:8850'
for i in $(seq 1 40); do pgrep -f 'civ5-server --dev --core real --listen 127.0.0.1:8850' > /dev/null || break; sleep 0.5; done
[ -f ~/m5-h/civ5-server-new ] && cp ~/m5-h/civ5-server-new ~/m5-h/civ5-server
rm -rf ~/m5-h/state && cp -r "$STATE_SRC" ~/m5-h/state
cd ~/m5-h
setsid nohup ./civ5-server --dev --core real --listen 127.0.0.1:8850 --admin-port 8851 --state-dir ~/m5-h/state --web-root ~/m5-h/web --assets-root ~/m5-h/assets --gamedata-dir ~/m5-h/gamedata > ~/m5-h/server.log 2>&1 < /dev/null &
echo $! > ~/m5-h/server.pid
sleep 8
grep -c 'lobby is open' ~/m5-h/server.log