#!/bin/bash
# M5 WO-J: (re)starts the two real-core servers of the scenario `m5-acceptance` from the WSL disk (WSL is slow on /mnt/<drive>):
#   bash tools/playtest/m5/serve.sh <web bundle dir on a Windows drive (/mnt/<drive>/...)>      stop: bash serve.sh stop
# ~/m5-j/{civ5-server,assets,gamedata-dev,gamedata-plain} are copies (civ5-server = a release build of crates/server with the real
# core; the dev game data = tools/playtest/m5/make-dev-gamedata-woj.ps1). Port 8870: mode "ai" on the DEV game data; port 8872: mode "mp"
# on the plain game data (admin ports are +1). The states are wiped on every start.
N=$HOME/m5-j
for p in 8870 8872; do
  for pid in $(pgrep -f "listen 127.0.0.1:$p"); do kill "$pid" 2>/dev/null; done
done
[ "$1" = "stop" ] && exit 0
sleep 3
rm -rf "$N/web" && cp -r "$1" "$N/web"
cd "$N" || exit 1
for spec in "8870 gamedata-dev" "8872 gamedata-plain"; do
  set -- $spec
  rm -rf "$N/state-$1" && mkdir -p "$N/state-$1"
  setsid nohup "$N/civ5-server" --dev --core real --listen 127.0.0.1:$1 --admin-port $(($1 + 1)) --state-dir "$N/state-$1" \
    --web-root "$N/web" --assets-root "$N/assets" --gamedata-dir "$N/$2" > "$N/state-$1/server.log" 2>&1 < /dev/null &
done
sleep 25
for p in 8870 8872; do tail -n 1 "$N/state-$p/server.log" | cut -c1-100; done
