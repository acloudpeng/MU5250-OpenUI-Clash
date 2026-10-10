# Clash proxy

The **Proxy** sidebar page runs the Clash.Meta core from the
[FlClash branch](https://github.com/chen08209/Clash.Meta/tree/FlClash) as a transparent
proxy for chosen LAN devices; its settings are under **Configure → Clash**. It is mutually
exclusive with WireGuard: at most one of them is on. **Active proxy** at the top of the page
(Off / WireGuard / Clash, `GET`/`PUT /api/proxy`) shows which one is on and turns them on
and off. Replacing one with the other, or turning it off, asks for confirmation and
switches inside a single held transaction; a failure restores the previous backend.
Clash cannot be selected until the core, TUN support and a downloaded subscription are
in place; the selector says which is missing. If both are ever saved as on, choosing one
turns the other off. Turning off names the backend the page shows as on (`active`); if
another one came on elsewhere meanwhile, the agent answers 409 instead of turning it off.

Status: deployed and running on a real B31 modem since 2026-10-10
([OPERATIONS.md](OPERATIONS.md)); proxied traffic from LAN devices there is still
being verified (see [PLAN.md](PLAN.md), P6).

## Build

```sh
python3 scripts/prepare-mihomo.py            # build/mihomo/mihomo, checksum-pinned
python3 scripts/prepare-geodata.py           # build/mihomo/geo/{geoip.metadb,GeoSite.dat}
python3 scripts/prepare-geodata.py --asn     # optional: add ASN.mmdb (12 MiB) for IP-ASN rules
```

`prepare-mihomo.py` builds the pinned commit as a static linux/arm64 binary with
`scripts/mihomo-patches/` applied (the FlClash branch leaves TUN and the mixed port to
its app; the patch restores them for a standalone daemon). The result must match the
pinned sha256; it needs Go or Docker and does not contact the modem.

The build also embeds Mozilla's root certificates as extracted by curl (MPL-2.0; date
and sha256 pinned in the script). Upstream's source holds only an empty placeholder that
its release workflow fills in. A core built without it trusts only the system's roots,
and on firmware with no root store where Go looks, every TLS check fails: HTTPS delay
tests report `get delay: all proxies timeout`, and DoH upstreams and TLS nodes fail.
B31 appears to ship `/etc/ssl/certs/ca-certificates.crt`, which the core also reads;
the built-in copy covers firmware without it. Cores built before 2026-10-09 (version
`flclash-d0c8d44-openui`) lack it. The version now ends with the bundle date
(`flclash-d0c8d44-openui-ca20260925`), shown on the Proxy page. `tests/clash-delay.sh`
runs a delay test through a local node with only the built-in roots (needs internet access).

`prepare-geodata.py` downloads the MetaCubeX geo databases and checks each against the
checksum published beside it. Upstream rebuilds them daily without versioned tags, so
the checksums are recorded in `SHA256SUMS` in the output directory and checked again at
deployment. Use `--base` for an HTTPS mirror that serves the same file names.

## Deploy

```sh
python3 scripts/deploy-components.py --gateway 192.168.0.1 \
  --ssh-key '/path/to/id_ed25519' \
  --agent target/aarch64-unknown-linux-musl/release/zte-agent \
  --dashboard build/dashboard-dist.tar.gz \
  --mihomo build/mihomo/mihomo \
  --mihomo-geodata build/mihomo/geo \
  --dry-run
```

Repeat without `--dry-run` to deploy. The core and geo data can also be deployed on
their own. Before any modem write, the deployer checks that the core is an aarch64 ELF
with the pinned checksum and version string (`--allow-unpinned-mihomo` accepts another
checksum from `prepare-mihomo.py --no-verify`), and that the geo files match
`SHA256SUMS`.

On the modem:

| Path | Content |
| --- | --- |
| `/data/bin/mihomo` | core, checked with `-v` before replacing the old one |
| `/data/share/mihomo/` | geo databases |
| `/data/local/tmp/openui-clash/` | settings, subscriptions, generated profile, controller socket (0700) |

Both deployed paths are in the transaction snapshot, so a failed deployment and the
printed recovery command restore them with the rest of OpenUI. A running core is
stopped after its files are replaced; the agent restarts it within seconds, and the
kill-switch holds proxied devices offline meanwhile.

The agent links the deployed geo files into the core's home directory, replacing copies
the core downloaded earlier. Without them, GEOIP/GEOSITE rules make the core download
the databases from GitHub, which often fails in mainland China; the Proxy page shows
**Geo data missing** in that case.

## Updating the core and geo data on the modem

**Configure → Clash → Advanced → Core and data** shows the installed core (its version
and whether it is the pinned build from `prepare-mihomo.py`, an official release or an
uploaded file) and can replace it without a computer:

- **Check for updates** asks api.github.com for the latest official MetaCubeX release
  (cached for 10 minutes; GitHub allows 60 unauthenticated requests per hour per address).
- **Update** downloads `mihomo-linux-arm64-<tag>.gz` through the running core, directly,
  or through an `https://` mirror prefix. The SHA-256 digest always comes from
  api.github.com, never the mirror. The archive is unpacked, checked to be an arm64
  executable that reports the release tag with `-v`, and tested with `-t` against the
  current profile before it replaces `/data/bin/mihomo`.
- **Upload** accepts a core you built (`.gz` or the binary, up to 64 MiB) with the same
  checks.
- The replaced core is kept as `/data/bin/mihomo.previous`; **Roll back** swaps them. A new
  core that does not answer within 15 s is rolled back automatically.
- **Update geo data** fetches GeoIP and GeoSite from MetaCubeX/meta-rules-dat with their
  published checksums and keeps the previous set.

With Clash on, each of these restarts the core: proxied devices pause for a few seconds
(the kill-switch holds their traffic) and the geo databases are reread (the core caches
them, so a configuration reload would not). Jobs refuse to run while a deployment holds
the transaction lock, and take that lock while they replace files. The deploy script keeps
installing the pinned FlClash build: deploying with `--mihomo` later replaces an on-device
update.

## Subscriptions

A subscription URL must return a complete Clash/mihomo YAML profile. Up to five are
kept; the URL is treated as a secret and only its host is shown. The agent keeps proxies,
groups, rules, providers, hosts, sniffer and geo settings from the profile and owns
everything that binds a port, routes traffic or picks the DNS mode.

Every download (adding a subscription, changing its URL, **Update now** and the automatic
refresh) is composed with the current settings and checked with `mihomo -t` before it
replaces the stored profile, also while Clash is off or the subscription is not the
active one. A rejected download keeps the previous profile and its details; the error
carries the core's reason (for example `proxy 0: … unknown method: …`), and a failed
refresh also shows it on the subscription. Downloads are stored unchecked only while no
core is installed. The running profile is composed and checked again each time it is
rebuilt: when Clash is turned on or its core restarts and, with Clash on, when another
subscription is chosen, a setting changes or the active one is refreshed.

## Clock

B31 has been seen keeping Beijing time in the system clock as if it were UTC, 8 hours
ahead, and its own NITZ/NTP writes it back. VMess, Shadowsocks 2022 and some REALITY
servers reject handshakes that far off; when every node uses one of them, a group delay
test reports `get delay: all proxies timeout`. The generated profile therefore turns on
the core's NTP client with `write-to-system: false`. It uses `203.107.6.88`
(ntp.aliyun.com by address, so it does not depend on DNS) over UDP 123 every 5 minutes.
Only the core's own protocol and TLS clock is corrected; the system clock is left alone.
Until the first sync succeeds, for example at boot before the WAN is up, time-checked
nodes can fail for up to 5 minutes. A failed sync logs `Sync time failed`. At log level
info, the core logs the measured offset; in this core version the line's "ahead" and
"behind" are swapped, so read only the amount.

## Traffic and recovery

- Chosen devices (all, or a MAC list, as with WireGuard) are marked and routed into the
  `ou-clash0` TUN; their DNS (UDP/TCP port 53 to any server) is answered by the core.
- If the core stops, a routing kill-switch blocks those devices instead of letting them
  fall back to the WAN. The router itself, unselected devices and the management ports
  are not affected.
- IPv6 is blocked for proxied devices.
- Turning Clash off removes its rules and restores normal routing.

## Troubleshooting

If sites that need the proxy fail while direct sites work, either the nodes fail or the
device's traffic never reaches the core. `scripts/device/diagnose-clash.sh` tells them
apart without changing anything: clock skew, whether the profile has the `ntp` block,
DNS through the core, HTTP and HTTPS node delays, fetches through the core, connection
sources, the TUN, `ip rule` and where a marked LAN packet is routed, and the firewall
hooks with their counters.

```sh
ssh -p 2222 root@192.168.0.1 sh -s < scripts/device/diagnose-clash.sh
```

On Windows, copy it with `scp -P 2222` and run `sh /tmp/diagnose-clash.sh` on the
modem; the file must keep LF line endings.

The **Diagnostics** card on the Proxy page's Clash **Overview** runs the same checks through the
agent (`POST /api/clash/diagnostics`, up to about 40 s in the background; it changes
nothing) and names the first broken layer: Clash off, core not running, traffic not
reaching Clash (route or firewall), nodes failing, or no device routed to Clash. Each
check shows its evidence: clock skew and core NTP, core DNS, the main group's node with
HTTPS/HTTP delays, fetches through the core, which LAN devices are proxied and active in
the core, the route of a marked LAN packet, and the firewall hooks, kill-switch counter,
IPA acceleration and `rp_filter`. The Overview also shows which devices Clash covers.
