#!/bin/sh
# Node delay test as the Proxy page runs it, trusting only the roots built into the core
# (DISABLE_SYSTEM_CA), as on a modem without a system root store. A local Shadowsocks
# listener stands in for the node and reaches the agent's default HTTPS test URL
# directly, so this needs internet access. Usage: clash-delay.sh path/to/mihomo
# MIHOMO_RUNNER runs a foreign-architecture binary (e.g. qemu-aarch64).
set -eu
CORE=$(realpath "${1:?usage: clash-delay.sh path/to/mihomo}")
RUNNER=${MIHOMO_RUNNER:-}
URL=https%3A%2F%2Fwww.gstatic.com%2Fgenerate_204
PORT=18388
WORK=$(mktemp -d)
PIDS=
cleanup() {
 for pid in $PIDS; do kill "$pid" 2>/dev/null || true; done
 rm -rf "$WORK"
}
trap cleanup EXIT HUP INT TERM
mkdir "$WORK/node" "$WORK/client"
cat >"$WORK/node/config.yaml" <<EOF
log-level: warning
external-controller-unix: $WORK/node/mihomo.sock
listeners:
  - {name: node, type: shadowsocks, listen: 127.0.0.1, port: $PORT, cipher: aes-128-gcm, password: delay-check}
rules: ['MATCH,DIRECT']
EOF
cat >"$WORK/client/config.yaml" <<EOF
log-level: warning
external-controller-unix: $WORK/client/mihomo.sock
proxies:
  - {name: node, type: ss, server: 127.0.0.1, port: $PORT, cipher: aes-128-gcm, password: delay-check}
proxy-groups:
  - {name: PROXY, type: select, proxies: [node]}
rules: ['MATCH,PROXY']
EOF

# wait_for NAME CHECK...: retries CHECK for up to 30 s; NAME's log explains a timeout.
wait_for() {
 name=$1; shift
 i=0
 until "$@" >/dev/null 2>&1; do
  i=$((i + 1)); [ "$i" -lt 150 ] || { echo "FAIL: $name core not ready ($*)"; cat "$WORK/$name.log"; exit 1; }
  sleep 0.2
 done
}
ctl() { curl -fsS --unix-socket "$WORK/$1/mihomo.sock" "http://mihomo$2"; }
listening() { grep -q ":$(printf '%04X' "$PORT") 00000000:0000 0A" /proc/net/tcp; }
# start NAME [ENV=VALUE...]: runs the core on NAME's profile with only built-in roots.
start() {
 name=$1; shift
 rm -f "$WORK/$name/mihomo.sock"
 # shellcheck disable=SC2086
 env DISABLE_SYSTEM_CA=true "$@" $RUNNER "$CORE" -d "$WORK/$name" -f "$WORK/$name/config.yaml" >>"$WORK/$name.log" 2>&1 &
 PIDS="$PIDS $!"
 wait_for "$name" ctl "$name" /version
}
delay() {
 curl -sS --unix-socket "$WORK/client/mihomo.sock" "http://mihomo/group/PROXY/delay?url=$URL&timeout=10000"
}

# The controller answers before the core opens its listeners and loads its groups.
start node
wait_for node listening
start client
client=$!
wait_for client ctl client /proxies/PROXY
answer=$(delay)
case "$answer" in
 *'"node":'[1-9]*) echo "PASS: delay test through a node with built-in roots only: $answer" ;;
 *) echo "FAIL: delay test with built-in roots only: $answer"; cat "$WORK/client.log"; exit 1 ;;
esac
# Control: without any roots the same test must fail, so the pass above came from them.
kill "$client"; wait "$client" 2>/dev/null || true
start client DISABLE_EMBED_CA=true
wait_for client ctl client /proxies/PROXY
answer=$(delay)
case "$answer" in
 *'all proxies timeout'*) echo 'PASS: without roots the delay test fails as on an unfixed core' ;;
 *) echo "FAIL: delay test passed without any roots: $answer"; exit 1 ;;
esac
