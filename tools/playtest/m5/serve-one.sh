#!/bin/bash
# One real-core server from the WSL disk (the scenario `m5-select-hotkeys` and other single-server checks):
#   bash tools/playtest/m5/serve-one.sh <dir> <port> <web bundle dir on a Windows drive (/mnt/<drive>/...)> [gamedata dir name, default gamedata-plain]
#   stop: bash serve-one.sh <dir> <port> stop
# <dir> (e.g. ~/m5-sel) holds civ5-server (a release build of crates/server with the real core), assets/ and the game data
# dir; the web bundle is copied to <dir>/web, the state <dir>/state-<port> is wiped on every start. Admin port: port + 1.
N=$1
P=$2
for pid in $(pgrep -f "listen 127.0.0.1:$P"); do kill "$pid" 2>/dev/null; done
[ "$3" = "stop" ] && exit 0
GD=${4:-gamedata-plain}
sleep 2
rm -rf "$N/web" && cp -r "$3" "$N/web"
rm -rf "$N/state-$P" && mkdir -p "$N/state-$P"
cd "$N" || exit 1
setsid nohup "$N/civ5-server" --dev --core real --listen 127.0.0.1:$P --admin-port $((P + 1)) --state-dir "$N/state-$P" \
  --web-root "$N/web" --assets-root "$N/assets" --gamedata-dir "$N/$GD" > "$N/state-$P/server.log" 2>&1 < /dev/null &
sleep 20
tail -n 2 "$N/state-$P/server.log" | cut -c1-140
