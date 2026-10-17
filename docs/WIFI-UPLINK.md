# Wi-Fi relay (uplink)

The device joins an existing Wi-Fi network (home, hotel, office) and uses it as its
internet connection instead of mobile data. Its own hotspot, LAN subnet and DHCP stay
as they are. While the upstream network reaches the internet, all IPv4 traffic goes
through it: devices on the hotspot or USB, the device itself, and the WireGuard and
Clash cores. When it does not, traffic falls back to the cellular link on its own and
comes back once the upstream network works again.

Web: **Network → Wi-Fi relay**. Screen: the **Wi-Fi relay** page (a menu option, and a
row on Overview). See [DASHBOARD.md](DASHBOARD.md) and [SCREEN.md](SCREEN.md).

![Network → Wi-Fi relay, online](images/network-relay.png)

| On Wi-Fi | Needs a web login |
| --- | --- |
| ![Screen: Wi-Fi relay](images/screen-uplink.png) | ![Screen: login mode offered](images/screen-uplink-login.png) |

## How it works

The agent (`agent/src/wifi_uplink.rs`) runs a supervisor thread. Everything it creates
is runtime state, removed when the relay is turned off and gone after a reboot without
the agent. It never writes UCI, init.d, stock firewall chains or main routes.

1. **Client interface.** `iw phy <phy> interface add ou-sta0 type managed` on the radio
   that carries the hotspot. The phy index changes on every firmware Wi-Fi reload, so
   it is read from `iw dev` each time, and the interface is only added after the same
   phy was seen on two passes in a row. If the interface's address equals a hotspot's,
   it gets a locally administered variant. IPv6 is disabled on it; `rp_filter` is loose.
2. **Association.** The device's `wpa_supplicant` (`-D nl80211`) runs as the agent's
   child with `/tmp/openui-uplink/wpa.conf` (mode 0600 in a 0700 directory) and a private
   control socket. Network names and SAE passwords are written as hex and WPA2
   passphrases as the derived 64-digit key (PBKDF2-HMAC-SHA1), so no user text is ever
   quoted into the file. Saved networks are tried in list order (`priority`).
   WPA2/WPA3 transition networks are joined with WPA2-PSK; WPA3-only networks use SAE
   with management frame protection. WEP and enterprise networks are not supported.
   An attached control socket copies the supplicant's events into the agent's log, which
   is how "Wrong Wi-Fi password" and "No saved network is in range" are reported.
3. **Address.** Busybox `udhcpc` with the hook `/tmp/openui-uplink/dhcp.sh`, which adds
   the address with `noprefixroute` and leaves the lease in `/tmp/openui-uplink/lease`.
   An upstream subnet that overlaps the LAN is refused.
4. **Routing.** Table `10431` holds the upstream subnet and `default via <gateway>`.
   - Rule `10429` `from <uplink address> lookup 10431`, whenever there is a lease.
   - While online: rule `10430` `lookup main suppress_prefixlength 0` (main without its
     default route, so LAN and carrier subnets stay where they are), then rule `10431`
     `lookup 10431`.

   These come after WireGuard (`10420`) and Clash (`10421`), so devices those
   backends route keep their tunnels, while the cores' own connections, which leave the
   device unmarked, use the uplink.
5. **Firewall** (chains `OUUP_*`, each hooked first in its built-in chain):
   - `OUUP_IN` (INPUT) and `OUUP_FWD` (FORWARD): from `ou-sta0` only replies and DHCP
     offers are accepted; nothing on the upstream network can reach the device or the
     LAN. LAN → `ou-sta0` is accepted.
   - `OUUP_NAT` masquerades and `OUUP_MSS` clamps MSS on `ou-sta0`.
   - While online: `OUUP_DNS` redirects LAN DNS (except devices marked by WireGuard or
     Clash, which keep their own) and `OUUP_OUT` the device's DNS that would leave over
     the uplink to the upstream network's resolver (the first DHCP DNS server, else
     223.5.5.5). Carrier resolvers usually refuse queries from other networks, and with
     no SIM there may be none.
   - While online: `OUUP_V6` rejects forwarded IPv6 from the LAN, so devices fall back to
     IPv4 at once instead of using mobile data over IPv6.
6. **Online check.** HTTP requests from the uplink address (bound to `ou-sta0`) to
   public resolvers' HTTP endpoints, by IP so no DNS is needed:
   `http://223.5.5.5/resolve?name=www.taobao.com&type=1` (AliDNS, JSON with
   `"Status":0`), else `http://119.29.29.29/d?dn=www.taobao.com` (DNSPod, a list of
   addresses), else `http://223.6.6.6/resolve?…`. The expected answer means online. A
   redirect or any other page means a **captive portal**: the network wants a web login,
   and its `Location` (or a URL in a meta refresh or script) is the login page. Nothing
   answering means no internet. Not port 53: DNS leaving over the uplink is redirected
   to the upstream resolver, which answers even when that network has no internet.
   Two passes in a row put traffic on Wi-Fi; three failures in a row (checked every
   10 s) put it back on cellular. Losing the association moves traffic back at once; the
   lease is given up after 15 s without it.
7. **Web login (login mode).** A portal's network shows `needs_login` with the login
   page. Login mode (`POST /api/wifi/uplink/login`, or **Start login** on the screen)
   puts traffic on the uplink for ten minutes before the check passes, with the same
   rules and DNS redirect as online, so a phone or computer on the hotspot can open the
   login page. The check runs every 3 s meanwhile; once it passes, the relay is online
   and login mode ends. It also ends when the window runs out, when it is ended, or when
   the relay is turned off. The portal sees the router's uplink address, so one login
   covers every device behind it.
8. **Recovery.** Every 3 s (and at once after a save) the supervisor re-hooks chains a
   firmware firewall reload dropped, restores missing rules, restarts a supplicant or
   DHCP client that exited, and rebuilds everything when the interface vanished with a
   firmware Wi-Fi reload. At start it removes a previous agent's online rules and
   processes for `ou-sta0`, and puts traffic back on Wi-Fi only after its own checks.

## API

- `GET /api/wifi/uplink`: `revision`, `enabled`, `state` (`off`, `unavailable`,
  `connecting`, `obtaining_ip`, `checking`, `online`, `no_internet`, `needs_login`,
  `logging_in`), `reason`, the
  joined network (`ssid`, `bssid`, `frequency`, `band`, `channel`, `network_id`,
  `signal`), the lease (`ip`, `prefix`, `gateway`, `dns`), `online`, `online_since`
  (device time), `online_for` (seconds; the B31 clock runs 8 h ahead, so clients use
  this), `rx_bytes`/`tx_bytes` since the interface was added, `networks` (`id`, `ssid`,
  `security`, `hidden`, `has_password`; never the password), `max_networks` (8) and
  `tools` (whether `wpa_supplicant` and `udhcpc` were found).
- `PUT /api/wifi/uplink`: `{expected_revision, enabled?, networks?}`; `networks` is the
  whole list in preference order, each `{id?, ssid, security, password?, hidden}` with
  `security` one of `open`, `wpa2`, `wpa3`, `wpa2_wpa3`. A missing password (or the
  `••••••••` placeholder) keeps the saved one of that `id`. A stale revision answers 409.
  Names are 1–32 bytes without control characters; passwords 8–63 printable ASCII
  characters, or 64 hex digits (not for WPA3).
- `POST /api/wifi/uplink/login`: `{enabled}` starts (ten minutes) or ends login mode;
  starting needs the relay on with a lease. The status adds `portal_url` and
  `login_remaining` (seconds); `state` adds `needs_login` and `logging_in`.
- `POST /api/wifi/uplink/scan`: `{networks: [{ssid, bssid, frequency, band, channel,
  signal, security, supported}]}`, one entry per name, strongest first, hidden names
  dropped. Uses the running supplicant (`SCAN`/`SCAN_RESULTS`), otherwise an
  off-channel `iw dev <hotspot> scan`, which keeps the hotspot up. Takes a few seconds.

Saved settings: `/data/local/tmp/openui-uplink/config.json` (0600 in a 0700 directory;
it holds the passwords).

## Limits

- One radio serves both the hotspot and the uplink. Joining a network on another
  channel of the same band can move the hotspot to that channel, and its devices
  reconnect.
- Web login: a device that Clash routes may send the login page to a node, which the
  portal blocks until the login; turn the proxy off while signing in. Portals that
  charge per device see one device. Portals that only hijack DNS (no HTTP redirect for
  addresses) show as no internet; login mode can still be started then.
- IPv4 only. LAN IPv6 forwarding is held while traffic is on Wi-Fi.
- With no SIM, the device's own resolver (dnsmasq) may have no upstream servers; LAN
  devices are still answered because their DNS is redirected.
- Needs `wpa_supplicant` with the nl80211 driver and busybox `udhcpc` on the device.
  The page says so when either is missing.

## Verification

- Unit tests in `wifi_uplink.rs`: the PSK derivation against the IEEE 802.11i
  vectors, configuration escaping, edits and validation, lease parsing and LAN overlap,
  `iw dev`, `iw … scan` and `SCAN_RESULTS` parsing, the online/offline thresholds, and
  that every generated script stays on owned chains, rules and tables.
- End-to-end scenario S16 (simulated router): a fake `iw` and `wpa_supplicant` (the
  latter checks the derived PSK independently with Python's hashlib), real busybox
  `udhcpc`, iptables and policy rules, and an upstream router namespace with DHCP, DNS
  and NAT. It covers scan, wrong password, joining, traffic of LAN devices and of the
  device over Wi-Fi, DNS through the upstream resolver, inbound blocking, fallback when
  the upstream loses the internet and recovery, leaving range, a firmware Wi-Fi reload
  with a new phy index, a captive portal on the upstream router (needs_login with its
  login page, login mode, a LAN client signing in through the relay, online afterwards),
  preference between two saved networks, WPA3, Clash's node traffic over the uplink, the
  pause after repeated interface losses, agent restart and complete removal.
- Not covered by the simulation: the real radio and driver (vdev creation next to the
  hotspots, channel changes, whether the firmware's own Wi-Fi management tolerates the
  extra interface), and whether the device ships a suitable `wpa_supplicant`.
