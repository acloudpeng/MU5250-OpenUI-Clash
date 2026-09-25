#!/bin/sh
# Read-only capability probe for the Clash backend (docs/PLAN.md, P0).
# Loads no modules, changes no firewall/routing/uci state, writes nothing.
# Run as root:  ssh -p 2222 root@192.168.0.1 sh -s < scripts/device/probe-clash.sh
#          or:  adb shell sh -s < scripts/device/probe-clash.sh
# Output omits IMEI, MAC addresses and the carrier WAN address.

section() { printf '\n## %s\n' "$1"; }
has() { command -v "$1" >/dev/null 2>&1; }
mask_ip() { sed -E 's/\b(100|10|172|192)\.[0-9]+\.[0-9]+\.[0-9]+\b/<ip>/g'; }

section firmware
uname -srm
grep -E 'DISTRIB_(RELEASE|REVISION|TARGET)' /etc/openwrt_release 2>/dev/null
ubus call zwrt_web device_info '{}' 2>/dev/null \
  | grep -E '"(software_version|sw_version|firmware_version|wa_inner_version|hardware_version|model_name|version)"'

section kernel-config
if [ -r /proc/config.gz ]; then
  zcat /proc/config.gz | grep -E '^CONFIG_(TUN|WIREGUARD|NETFILTER_XT_TARGET_TPROXY|NETFILTER_XT_MATCH_SOCKET|NETFILTER_XT_MATCH_MAC|NF_TPROXY_IPV4|NF_TABLES|NFT_TPROXY|NFT_SOCKET|IP_MULTIPLE_TABLES|IP_ADVANCED_ROUTER)='
else
  echo 'no /proc/config.gz'
fi

section tun
ls -l /dev/net/tun 2>&1
grep -w tun /proc/misc 2>/dev/null || echo 'tun not in /proc/misc'
[ -d /sys/module/tun ] && echo 'module tun present' || echo 'no /sys/module/tun'
[ -d /sys/module/wireguard ] && echo 'module wireguard present' || echo 'no /sys/module/wireguard'

section netfilter
for tool in iptables iptables-legacy ip6tables nft ipset; do
  has "$tool" && printf '%s: %s\n' "$tool" "$("$tool" -V 2>&1 | head -1)"
done
echo "tables: $(cat /proc/net/ip_tables_names 2>/dev/null | tr '\n' ' ')"
echo "loaded targets: $(cat /proc/net/ip_tables_targets 2>/dev/null | sort -u | tr '\n' ' ')"
echo "loaded matches: $(cat /proc/net/ip_tables_matches 2>/dev/null | sort -u | tr '\n' ' ')"
# Help output only; does not add rules. Tells whether the userspace extensions exist.
iptables -j TPROXY -h 2>&1 | grep -q -- '--on-port' && echo 'TPROXY extension: yes' || echo 'TPROXY extension: no'
iptables -m socket -h 2>&1 | grep -q -- '--transparent' && echo 'socket match: yes' || echo 'socket match: no'
iptables -m mac -h 2>&1 | grep -q -- '--mac-source' && echo 'mac match: yes' || echo 'mac match: no'
KMODS=/lib/modules/$(uname -r)
for mod in xt_TPROXY nf_tproxy_ipv4 xt_socket nf_socket_ipv4 xt_mac tun nft_tproxy nft_socket; do
  found=$(find "$KMODS" -name "$mod.ko" 2>/dev/null | head -1)
  [ -n "$found" ] && echo "kmod $mod: $found"
done
has nft && { echo 'nft tables:'; nft list tables 2>&1 | head -20; }
echo 'mangle PREROUTING hooks:'
iptables -w 2 -t mangle -S PREROUTING 2>&1 | head -20
echo 'nat PREROUTING hooks:'
iptables -w 2 -t nat -S PREROUTING 2>&1 | head -20

section offload
# Compare netfilter FORWARD byte counters against interface totals to spot IPA/NSS bypass.
iptables -w 2 -t mangle -L FORWARD -v -x -n 2>/dev/null | head -3
grep -E '^\s*(br-lan|rmnet_data[0-9]+|rmnet_ipa0):' /proc/net/dev
ls -d /sys/kernel/debug/ipa* /dev/ipa* 2>/dev/null
pidof ipacm >/dev/null && echo 'ipacm running'
for f in /proc/sys/net/netfilter/nf_conntrack_count /proc/sys/net/netfilter/nf_conntrack_max; do
  [ -r "$f" ] && echo "$(basename "$f"): $(cat "$f")"
done

section routing
ip rule show | mask_ip
ip route show | mask_ip
ip route show table all 2>/dev/null | awk '{for (i = 1; i < NF; i++) if ($i == "table") print $(i + 1)}' | sort -u | tr '\n' ' '
echo
has mwan3 && echo 'mwan3 present'
pidof mwan3rtmon >/dev/null && echo 'mwan3rtmon running'

section dns
netstat -lnup 2>/dev/null | grep -E ':53\b' | mask_ip
ls /tmp/dnsmasq.d/ 2>/dev/null

section curl
if has curl; then
  curl -V 2>&1 | head -3
else
  echo 'no curl'
fi
has uclient-fetch && echo 'uclient-fetch present'

section resources
free -m 2>/dev/null || grep -E 'Mem(Total|Free|Available)' /proc/meminfo
df -h /data /tmp /etc 2>/dev/null
nproc 2>/dev/null
ls /data/bin 2>/dev/null | tr '\n' ' '
echo

section power
cat /sys/power/autosleep 2>/dev/null && echo '(autosleep)'
cat /sys/power/wake_lock 2>/dev/null | tr ' ' '\n' | head -10
