#!/usr/bin/env bash
# netem.sh: real packet loss and delay for the net-test stand. Run as root (WSL: `wsl -u root`).
# It touches only its own pair of veth devices in its own network namespace; nothing else on the machine.
#
#   netem.sh up                                   netns ntbots, veth ntb0 (root, 10.77.0.1/24) <-> ntb1 (netns, 10.77.0.2/24)
#   netem.sh set <rtt_ms> <jitter_ms> <loss_pct> [rate_kbit]
#   netem.sh blackhole on|off                     loss 100% (TCP stays up and retransmits)
#   netem.sh status                               the qdiscs now
#   netem.sh down                                 remove everything
#
# The server listens on 10.77.0.1; the bots run as `ip netns exec ntbots net-bot ...`. The loss is on each
# direction separately (3 % means 3 % each way). The admin API stays on 127.0.0.1 of the root and is not affected.
set -euo pipefail

NS=ntbots
ROOT_DEV=ntb0
NS_DEV=ntb1
ROOT_IP=10.77.0.1
NS_IP=10.77.0.2
STATE=/tmp/netem-ntbots.state

need_root() {
  [ "$(id -u)" = 0 ] || { echo "netem.sh: run as root" >&2; exit 1; }
}

is_up() {
  ip netns list 2>/dev/null | grep -q "^$NS\b"
}

cmd_up() {
  need_root
  modprobe sch_netem 2>/dev/null || true
  if is_up; then
    echo "netem: already up"
    return 0
  fi
  ip netns add "$NS"
  ip link add "$ROOT_DEV" type veth peer name "$NS_DEV"
  ip link set "$NS_DEV" netns "$NS"
  ip addr add "$ROOT_IP/24" dev "$ROOT_DEV"
  ip link set "$ROOT_DEV" up
  ip netns exec "$NS" ip addr add "$NS_IP/24" dev "$NS_DEV"
  ip netns exec "$NS" ip link set "$NS_DEV" up
  ip netns exec "$NS" ip link set lo up
  echo "netem: up ($ROOT_IP <-> $NS_IP)"
}

# Both directions get a netem qdisc: delay is half of the RTT on each.
apply() {
  local rtt=$1 jitter=$2 loss=$3 rate=${4:-0}
  local d=$((rtt / 2)) j=$((jitter / 2))
  local args="delay ${d}ms"
  if [ "$j" -gt 0 ]; then args="$args ${j}ms distribution normal"; fi
  if [ "$loss" != "0" ]; then args="$args loss ${loss}%"; fi
  if [ "$rate" != "0" ]; then args="$args rate ${rate}kbit"; fi
  # A queue limit big enough that the delay does not drop packets by itself.
  tc qdisc replace dev "$ROOT_DEV" root netem $args limit 10000
  ip netns exec "$NS" tc qdisc replace dev "$NS_DEV" root netem $args limit 10000
  echo "$rtt $jitter $loss $rate" >"$STATE"
}

cmd_set() {
  need_root
  is_up || { echo "netem: run 'up' first" >&2; exit 1; }
  [ $# -ge 3 ] || { echo "usage: netem.sh set <rtt_ms> <jitter_ms> <loss_pct> [rate_kbit]" >&2; exit 2; }
  apply "$1" "$2" "$3" "${4:-0}"
  echo "netem: rtt=$1ms jitter=$2ms loss=$3% rate=${4:-0}kbit"
}

cmd_blackhole() {
  need_root
  is_up || { echo "netem: run 'up' first" >&2; exit 1; }
  case "${1:-}" in
    on)
      tc qdisc replace dev "$ROOT_DEV" root netem loss 100%
      ip netns exec "$NS" tc qdisc replace dev "$NS_DEV" root netem loss 100%
      echo "netem: blackhole on"
      ;;
    off)
      if [ -f "$STATE" ]; then
        # shellcheck disable=SC2046
        apply $(cat "$STATE")
      else
        tc qdisc del dev "$ROOT_DEV" root 2>/dev/null || true
        ip netns exec "$NS" tc qdisc del dev "$NS_DEV" root 2>/dev/null || true
      fi
      echo "netem: blackhole off"
      ;;
    *) echo "usage: netem.sh blackhole on|off" >&2; exit 2 ;;
  esac
}

cmd_status() {
  is_up || { echo "netem: down"; return 0; }
  tc qdisc show dev "$ROOT_DEV"
  ip netns exec "$NS" tc qdisc show dev "$NS_DEV"
}

cmd_down() {
  need_root
  if is_up; then
    # Removing the netns removes the veth pair with it.
    ip link del "$ROOT_DEV" 2>/dev/null || true
    ip netns del "$NS" 2>/dev/null || true
  fi
  rm -f "$STATE"
  echo "netem: down"
}

case "${1:-}" in
  up) cmd_up ;;
  set) shift; cmd_set "$@" ;;
  blackhole) shift; cmd_blackhole "$@" ;;
  status) cmd_status ;;
  down) cmd_down ;;
  *) echo "usage: netem.sh up | set <rtt_ms> <jitter_ms> <loss_pct> [rate_kbit] | blackhole on|off | status | down" >&2; exit 2 ;;
esac
