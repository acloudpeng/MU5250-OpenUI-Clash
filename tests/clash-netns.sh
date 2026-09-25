#!/bin/sh
# Clash data path in network namespaces: LAN client -> MAC mark -> ou-clash0 (real
# mihomo, system stack) -> DIRECT -> test server, without masquerading LAN sources.
# $BASE holds mihomo, config.yaml and the agent's exported policy scripts.
# MIHOMO_RUNNER runs a foreign-architecture binary (e.g. qemu-aarch64).
set -eu
BASE=${BASE:-/tmp/openui-clash-check}
RUNNER=${MIHOMO_RUNNER:-}
R=oucl-check-r
C=oucl-check-c
S=oucl-check-s
PID=
cleanup() {
 [ -n "$PID" ] && kill "$PID" 2>/dev/null || true
 for n in "$C" "$R" "$S"; do ip netns del "$n" 2>/dev/null || true; done
}
trap cleanup EXIT HUP INT TERM
for n in "$R" "$C" "$S"; do
 if ip netns list | grep -q "^$n "; then echo 'Test namespace already exists'; exit 1; fi
 ip netns add "$n"
 ip netns exec "$n" ip link set lo up
done
ip netns exec "$R" ip link add br-lan type bridge
ip netns exec "$R" ip link set br-lan up
ip netns exec "$R" ip address add 192.0.2.1/24 dev br-lan
ip netns exec "$R" ip link add cl-r type veth peer name cl-c
ip netns exec "$R" ip link set cl-c netns "$C"
ip netns exec "$R" ip link set cl-r master br-lan
ip netns exec "$R" ip link set cl-r up
ip netns exec "$C" ip link set cl-c address 02:11:22:33:44:55
ip netns exec "$C" ip address add 192.0.2.2/24 dev cl-c
ip netns exec "$C" ip link set cl-c up
ip netns exec "$C" ip route add default via 192.0.2.1
ip netns exec "$R" ip link add wan-test type veth peer name wan-peer
ip netns exec "$R" ip link set wan-peer netns "$S"
ip netns exec "$R" ip address add 100.64.0.1/30 dev wan-test
ip netns exec "$R" ip link set wan-test up
ip netns exec "$R" ip route add default via 100.64.0.2
ip netns exec "$S" ip address add 100.64.0.2/30 dev wan-peer
ip netns exec "$S" ip link set wan-peer up
ip netns exec "$S" ip address add 203.0.113.1/32 dev lo
ip netns exec "$S" ip route add 192.0.2.0/24 via 100.64.0.1
ip netns exec "$R" sh -c 'echo 1 > /proc/sys/net/ipv4/ip_forward'
ip netns exec "$R" iptables -t nat -A POSTROUTING -o wan-test -j MASQUERADE
mkdir -p "$BASE/www"
echo ok > "$BASE/www/ok"
ip netns exec "$S" python3 -m http.server 8080 --bind 203.0.113.1 --directory "$BASE/www" >"$BASE/server.log" 2>&1 &

start_core() {
 rm -f "$BASE/mihomo.sock"
 # shellcheck disable=SC2086
 ip netns exec "$R" $RUNNER "$BASE/mihomo" -d "$BASE" -f "$BASE/config.yaml" >>"$BASE/core.log" 2>&1 &
 PID=$!
 i=0
 until ip netns exec "$R" test -e /sys/class/net/ou-clash0 && [ -S "$BASE/mihomo.sock" ]; do
  i=$((i + 1)); [ "$i" -lt 150 ] || { echo 'FAIL: core did not start'; tail "$BASE/core.log"; exit 1; }
  sleep 0.2
 done
 ip netns exec "$R" sh -c 'echo 2 > /proc/sys/net/ipv4/conf/ou-clash0/rp_filter'
}
stop_core() {
 kill "$PID" 2>/dev/null || true; wait "$PID" 2>/dev/null || true; PID=
 while ip netns exec "$R" test -e /sys/class/net/ou-clash0; do sleep 0.2; done
}
seen() { grep -c '192.0.2.2:[0-9]* --> .*203.0.113.1:8080\|192.0.2.2:[0-9]* --> test.example:8080' "$BASE/core.log" || true; }
fetch() { ip netns exec "$C" curl -fsS --max-time 5 "$@" >/dev/null; }

: >"$BASE/core.log"
start_core
fetch http://203.0.113.1:8080/ok
echo 'PASS: direct network before policy'
for mode in all selected excluded; do
 ip netns exec "$R" sh "$BASE/$mode.sh"
 before=$(seen)
 fetch http://203.0.113.1:8080/ok
 sleep 0.5
 after=$(seen)
 if [ "$mode" = excluded ]; then test "$after" = "$before"; else test "$after" -gt "$before"; fi
 echo "PASS: $mode routing"
 if [ "$mode" != excluded ]; then
  # Only the core serves DNS here: profile hosts win over fake-ip, other names get fake-ips.
  answer=$(ip netns exec "$C" dig +short +time=3 +tries=1 @192.0.2.1 test.example A)
  [ "$answer" = 203.0.113.1 ] || { echo "FAIL: DNS not answered by the core: $answer"; exit 1; }
  fetch --resolve "test.example:8080:$answer" http://test.example:8080/ok
  answer=$(ip netns exec "$C" dig +short +tcp +time=3 +tries=1 @1.1.1.1 probe.example A)
  case "$answer" in 198.18.*) ;; *) echo "FAIL: TCP DNS not hijacked: $answer"; exit 1;; esac
  answer=$(ip netns exec "$C" dig +short +time=3 +tries=1 @9.9.9.9 probe.example A)
  case "$answer" in 198.18.*) ;; *) echo "FAIL: UDP DNS not hijacked: $answer"; exit 1;; esac
  echo 'PASS: client DNS (UDP and TCP, any server) answered by the core and routed back'
  stop_core
  if fetch http://203.0.113.1:8080/ok 2>/dev/null; then echo 'FAIL: core-down traffic escaped'; exit 1; fi
  if ip netns exec "$C" dig +short +time=2 +tries=1 @192.0.2.1 test.example A >/dev/null 2>&1; then
   echo 'FAIL: core-down DNS escaped'; exit 1
  fi
  echo 'PASS: core down blocks selected internet and DNS'
  ip netns exec "$C" ping -c 1 -W 1 192.0.2.1 >/dev/null
  echo 'PASS: local management remains reachable'
  start_core
  ip netns exec "$R" sh "$BASE/$mode.sh"
  fetch http://203.0.113.1:8080/ok
  echo 'PASS: restart and policy re-apply recover'
 fi
 ip netns exec "$R" sh "$BASE/health.sh"
 ip netns exec "$R" sh "$BASE/cleanup.sh"
 if ip netns exec "$R" sh "$BASE/health.sh" 2>/dev/null; then echo 'FAIL: health after cleanup'; exit 1; fi
 before=$(seen)
 fetch http://203.0.113.1:8080/ok
 test "$(seen)" = "$before"
 echo 'PASS: disable restores direct network'
 # Cleanup removes the interface too; the agent always stops the core first.
 ip netns exec "$R" test -e /sys/class/net/ou-clash0 || { stop_core; start_core; }
done
ip netns exec "$R" sh "$BASE/hold.sh"
if fetch http://203.0.113.1:8080/ok 2>/dev/null; then echo 'FAIL: transition guard'; exit 1; fi
ip netns exec "$C" ping -c 1 -W 1 192.0.2.1 >/dev/null
ip netns exec "$R" sh "$BASE/release.sh"
fetch http://203.0.113.1:8080/ok
echo 'PASS: transition guard and recovery'
