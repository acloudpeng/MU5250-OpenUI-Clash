# Clash proxy

The **Proxy** sidebar page runs the Clash.Meta core from the
[FlClash branch](https://github.com/chen08209/Clash.Meta/tree/FlClash) as a transparent
proxy for chosen LAN devices. It is mutually exclusive with WireGuard: at most one of
them is on. Turning one on while the other runs asks for confirmation and switches
inside a single held transaction; a failure restores the previous backend.

Status: agent, web page and deployment are complete and tested in network namespaces
and against a mock agent. Validation on a real B31 modem is pending (see
[PLAN.md](PLAN.md), P0/P1/P6).

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

## Subscriptions

A subscription URL must return a complete Clash/mihomo YAML profile. Up to five are
kept; the URL is treated as a secret and only its host is shown. A download is checked
with `mihomo -t` before it replaces the previous profile. The agent keeps proxies,
groups, rules, providers, hosts, sniffer and geo settings from the profile and owns
everything that binds a port, routes traffic or picks the DNS mode.

## Traffic and recovery

- Chosen devices (all, or a MAC list, as with WireGuard) are marked and routed into the
  `ou-clash0` TUN; their DNS (UDP/TCP port 53 to any server) is answered by the core.
- If the core stops, a routing kill-switch blocks those devices instead of letting them
  fall back to the WAN. The router itself, unselected devices and the management ports
  are not affected.
- IPv6 is blocked for proxied devices.
- Turning Clash off removes its rules and restores normal routing.
