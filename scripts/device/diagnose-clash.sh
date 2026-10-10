#!/bin/sh
# Read-only diagnosis of the Clash data path (docs/CLASH.md): clock, core, nodes, DNS,
# routing of marked LAN traffic and the firewall hooks. Changes no firewall, routing or
# settings; the delay tests and the two test fetches only send traffic.
# Run as root:  ssh -p 2222 root@192.168.0.1 sh -s < scripts/device/diagnose-clash.sh
# Output omits subscription URLs, node servers, MAC addresses and private/carrier addresses.

STATE=/data/local/tmp/openui-clash
SOCK=$STATE/home/mihomo.sock
MARK=0x20000000
TABLE=10421
section() { printf '\n## %s\n' "$1"; }
mask() { sed -E 's/\b(100|10|172|192)\.[0-9]+\.[0-9]+\.[0-9]+\b/<ip>/g; s/([0-9a-fA-F]{2}:){5}[0-9a-fA-F]{2}/<mac>/g'; }
ctl() { curl -sS --max-time "${2:-10}" --unix-socket "$SOCK" "http://mihomo$1" 2>&1; }
delays() { ctl "/group/GLOBAL/delay?url=$1&timeout=5000" 20; }

section clock
echo "device UTC: $(date -u '+%Y-%m-%d %H:%M:%S')"
http_date=$(curl -sI --max-time 8 http://www.baidu.com 2>/dev/null | sed -n 's/^[Dd]ate: //p' | tr -d '\r')
echo "server Date: ${http_date:-unavailable}"
server=$(date -u -D '%a, %d %b %Y %H:%M:%S GMT' -d "$http_date" +%s 2>/dev/null) &&
 echo "device minus server: $(( $(date +%s) - server )) s"

section settings
for key in enabled mode proxy_mode stack dns_mode active_subscription; do
 grep -o "\"$key\":[^,}]*" $STATE/config.json 2>/dev/null
done
echo "listed MACs: $(grep -o '"macs":\[[^]]*\]' $STATE/config.json 2>/dev/null | grep -o '"[0-9a-f:]\{17\}"' | wc -l)"
echo "WireGuard chains present: $(iptables -w 2 -S 2>/dev/null | grep -c 'OUWG_')"

section core
pid=$(cat $STATE/mihomo.pid 2>/dev/null)
echo "pid ${pid:-none}: $(readlink /proc/"$pid"/exe 2>/dev/null) $(grep VmRSS /proc/"$pid"/status 2>/dev/null)"
echo "version: $(ctl /version 3)"
grep -q '^ntp:' $STATE/home/config.yaml 2>/dev/null && echo 'profile ntp: yes' || echo 'profile ntp: no (agent older than 60ce186)'
echo "configs: $(ctl /configs 5 | grep -oE '"mode":"[^"]*"|"tun":\{[^}]*\}' | tr '\n' ' ')"
echo "node types: $(ctl /proxies 10 | grep -o '"type":"[^"]*"' | sort | uniq -c | tr -s ' \n' ' ')"

section dns-through-core
for name in www.baidu.com www.google.com; do
 answer=$(ctl "/dns/query?name=$name&type=A" 10)
 echo "$name: $(echo "$answer" | grep -o '"Status":[0-9]*') $(echo "$answer" | grep -o '"data":"[^"]*"' | head -2 | tr '\n' ' ')$(echo "$answer" | grep -o '"message":"[^"]*"')"
done

section nodes
for url in https%3A%2F%2Fwww.gstatic.com%2Fgenerate_204 http%3A%2F%2Fwww.gstatic.com%2Fgenerate_204; do
 answer=$(delays "$url")
 echo "GLOBAL delay ($url): $(echo "$answer" | grep -o '":[0-9][0-9]*' | wc -l) answered; $(echo "$answer" | cut -c1-240)"
done

section fetch-from-router-through-core
for url in https://www.baidu.com https://www.google.com/generate_204; do
 echo "$url: $(curl -sS -o /dev/null -w '%{http_code} %{time_total}s' --max-time 10 -x http://127.0.0.1:7890 "$url" 2>&1 | tail -1)"
done

section connections
conns=$(ctl /connections 10)
echo "total: $(echo "$conns" | grep -o '"id":' | wc -l)"
echo "by source: $(echo "$conns" | grep -o '"sourceIP":"[^"]*"' | sort | uniq -c | mask | tr -s ' \n' ' ')"
echo "top chains:"; echo "$conns" | grep -o '"chains":\[[^]]*\]' | sort | uniq -c | sort -rn | head -5

section tun
ip -4 addr show dev ou-clash0 2>&1 | mask
ip -s link show dev ou-clash0 2>&1 | sed -n '1p;3,6p'
for conf in all br-lan ou-clash0; do echo "rp_filter $conf: $(cat /proc/sys/net/ipv4/conf/$conf/rp_filter 2>/dev/null)"; done
echo "ip_forward: $(cat /proc/sys/net/ipv4/ip_forward)"

section routing
ip -4 rule show | mask
echo "table $TABLE:"; ip -4 route show table $TABLE
echo "main default: $(ip -4 route show default | mask)"
client=$(ip -4 neigh show dev br-lan 2>/dev/null | awk '/lladdr/ && $1 !~ /^169\./ {print $1; exit}')
if [ -n "$client" ]; then
 mac=$(ip -4 neigh show dev br-lan | awk -v ip="$client" '$1 == ip {for (i = 1; i < NF; i++) if ($i == "lladdr") print $(i + 1)}')
 grep -qi "\"$mac\"" $STATE/config.json 2>/dev/null && listed=yes || listed=no
 echo "sample LAN client listed in macs: $listed"
 for dst in 1.1.1.1 198.19.0.53; do
  echo "marked   $dst: $(ip -4 route get $dst from "$client" iif br-lan mark $MARK 2>&1 | head -1 | mask)"
 done
 echo "unmarked 1.1.1.1: $(ip -4 route get 1.1.1.1 from "$client" iif br-lan 2>&1 | head -1 | mask)"
else
 echo 'no LAN client in the neighbour table'
fi

section hooks
for spec in mangle:PREROUTING nat:PREROUTING filter:FORWARD filter:INPUT nat:POSTROUTING; do
 echo "${spec}:"; iptables -w 2 -t "${spec%%:*}" -S "${spec#*:}" 2>&1 | head -6 | mask
done
echo "transition guard: $(iptables -w 2 -S FORWARD 2>/dev/null | grep -c '_HOLD')"
echo "other mark writers in mangle:"
iptables -w 2 -t mangle -S 2>/dev/null | grep -E -- '-j (MARK|CONNMARK)' | grep -v 'OUCL_' | head -15 | mask

section counters
for spec in mangle:OUCL_MARK nat:OUCL_DNS filter:OUCL_FWD filter:OUCL_DNS4; do
 echo "${spec}:"; iptables -w 2 -t "${spec%%:*}" -L "${spec#*:}" -v -n -x 2>&1 | mask
done
echo "conntrack to fake-ip range: $(grep -c 'dst=198\.18\.' /proc/net/nf_conntrack 2>/dev/null)"
