//! Wi-Fi uplink (relay): the device joins an upstream Wi-Fi network through an extra
//! client interface on the hotspot's radio and sends all IPv4 traffic through it while
//! that network reaches the internet; otherwise traffic stays on the cellular link.
//!
//! Runtime state only, owned by the agent: a `managed` vdev, a wpa_supplicant and a
//! udhcpc on it, owned firewall chains and policy rules after the tunnel backends'. Main
//! routes, stock firewall chains, init.d and UCI are never changed, so a reboot without
//! the agent leaves nothing behind. The hotspot keeps running; the radio may move it to
//! the upstream channel.
use crate::{egress, storage};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, TcpStream},
    os::unix::{fs::PermissionsExt, io::FromRawFd, net::UnixDatagram},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        Condvar, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const DIR: &str = "/data/local/tmp/openui-uplink";
const CONFIG: &str = "/data/local/tmp/openui-uplink/config.json";
/// Volatile files: supplicant configuration, DHCP hook and lease, control sockets.
const RUN: &str = "/tmp/openui-uplink";
const IFACE: &str = "ou-sta0";
/// Routing table for the uplink. Its rules sit after WireGuard (10420) and Clash (10421),
/// so devices those backends route keep their tunnels and only the cores' own traffic,
/// which leaves the device unmarked, follows the uplink.
const TABLE: &str = "10431";
/// `from <uplink address>`: replies and the connectivity check, whenever there is a lease.
const PRIO_SOURCE: &str = "10429";
/// Main without its default route, then the uplink table: only while online.
const PRIO_MAIN: &str = "10430";
const PRIO_ALL: &str = "10431";
const PREFIX: &str = "OUUP";
/// Fwmark bits of the WireGuard (0x40000000) and Clash (0x20000000) policies. Their
/// devices keep their own DNS redirect.
const TUNNEL_MARKS: &str = "0x60000000";
pub const MAX_NETWORKS: usize = 8;
const LOOP: Duration = Duration::from_secs(3);
/// The connectivity check while online, and while associated but offline.
const CHECK_EVERY: Duration = Duration::from_secs(10);
const FAILS_TO_DROP: u32 = 3;
const PASSES_TO_JOIN: u32 = 2;
/// How long a lost association may last before the lease is given up.
const RELEASE_AFTER: Duration = Duration::from_secs(15);
/// The interface vanishing this often (firmware Wi-Fi reloads) pauses the relay until
/// it is saved again, so the hotspot is never caught in a reload loop.
const LOSSES_TO_PAUSE: usize = 3;
const LOSS_WINDOW: Duration = Duration::from_secs(600);
const PAUSED: &str = "The firmware kept removing the Wi-Fi client interface; the relay is paused. Turn it off and on to try again";
const BOOTING: &str = "Waiting for the device to finish starting";
const NEEDS_LOGIN: &str =
    "This network needs a web login; start login mode, then sign in from a phone or computer";
const NO_INTERNET: &str = "Connected, but the upstream network does not reach the internet";
/// How long login mode routes traffic over a network that has not passed the check.
const LOGIN_WINDOW: Duration = Duration::from_secs(600);
const PLACEHOLDER: &str = "••••••••";

/// Serialises configuration changes; the supervisor only reads the saved file.
static LOCK: Mutex<()> = Mutex::new(());
/// What the supervisor last saw, for the API and the screen.
static SNAPSHOT: Mutex<Snapshot> = Mutex::new(Snapshot::new());
/// Set by a save so the supervisor acts on it at once.
static WAKE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());
static LOGS: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
static NEXT_SOCKET: AtomicU64 = AtomicU64::new(0);
/// Until when login mode is on: traffic goes over the uplink before it passes the check,
/// so a phone or computer can reach the network's login page.
static LOGIN: Mutex<Option<Instant>> = Mutex::new(None);
/// Bumped for every supplicant started; its event monitor stops when this moves on.
static GENERATION: AtomicU64 = AtomicU64::new(0);

// ---------- Settings ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Security {
    Open,
    Wpa2,
    Wpa3,
    /// WPA2/WPA3 transition networks; joined with WPA2-PSK, which every build supports.
    Wpa2Wpa3,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Network {
    id: u64,
    ssid: String,
    security: Security,
    #[serde(default)]
    password: String,
    #[serde(default)]
    hidden: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Settings {
    revision: u64,
    enabled: bool,
    /// In preference order: the first one in range is joined.
    networks: Vec<Network>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NetworkEdit {
    #[serde(default)]
    id: Option<u64>,
    ssid: String,
    security: Security,
    /// Omitted or the placeholder keeps the saved password of the same network.
    #[serde(default)]
    password: Option<String>,
    #[serde(default)]
    hidden: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Update {
    expected_revision: u64,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    networks: Option<Vec<NetworkEdit>>,
}

fn validate_ssid(ssid: &str) -> Result<(), String> {
    if ssid.is_empty() || ssid.len() > 32 || ssid.chars().any(char::is_control) {
        return Err("Network name must be 1–32 bytes without control characters".into());
    }
    Ok(())
}

fn validate_password(security: Security, password: &str) -> Result<(), String> {
    let hex = password.len() == 64 && password.bytes().all(|b| b.is_ascii_hexdigit());
    match security {
        Security::Open if password.is_empty() => Ok(()),
        Security::Open => Err("An open network has no password".into()),
        Security::Wpa3 if hex => Err("A WPA3 password cannot be a 64-digit key".into()),
        _ if hex => Ok(()),
        _ if (8..=63).contains(&password.len())
            && password.bytes().all(|b| (0x20..=0x7e).contains(&b)) =>
        {
            Ok(())
        }
        _ => Err("Wi-Fi password must be 8–63 printable ASCII characters".into()),
    }
}

fn validate(s: &Settings) -> Result<(), String> {
    if s.networks.len() > MAX_NETWORKS {
        return Err(format!("At most {MAX_NETWORKS} networks can be saved"));
    }
    for (i, n) in s.networks.iter().enumerate() {
        validate_ssid(&n.ssid)?;
        validate_password(n.security, &n.password)?;
        if s.networks[..i].iter().any(|o| o.ssid == n.ssid) {
            return Err("This network is already saved".into());
        }
        if s.networks[..i].iter().any(|o| o.id == n.id) {
            return Err("Duplicate network ID".into());
        }
    }
    if s.enabled && s.networks.is_empty() {
        return Err("Add an upstream Wi-Fi network first".into());
    }
    Ok(())
}

fn edited(old: &Settings, u: Update) -> Result<Settings, String> {
    if u.expected_revision != old.revision {
        return Err("Configuration changed; reload and try again".into());
    }
    let mut next = old.clone();
    if let Some(edits) = u.networks {
        let mut ids = old.networks.iter().map(|n| n.id).max().unwrap_or(0);
        let mut networks = Vec::with_capacity(edits.len());
        for e in edits {
            let saved = e.id.and_then(|id| old.networks.iter().find(|n| n.id == id));
            if e.id.is_some() && saved.is_none() {
                return Err("Network not found; reload and try again".into());
            }
            let password = match (e.security, e.password.as_deref()) {
                (Security::Open, _) => String::new(),
                (_, None | Some(PLACEHOLDER)) => {
                    saved.map(|n| n.password.clone()).unwrap_or_default()
                }
                (_, Some(p)) => p.to_string(),
            };
            let id = match saved {
                Some(n) => n.id,
                None => {
                    ids = ids.checked_add(1).ok_or("Network IDs exhausted")?;
                    ids
                }
            };
            networks.push(Network {
                id,
                ssid: e.ssid,
                security: e.security,
                password,
                hidden: e.hidden,
            });
        }
        next.networks = networks;
    }
    if let Some(enabled) = u.enabled {
        next.enabled = enabled;
    }
    validate(&next)?;
    next.revision = old.revision.saturating_add(1);
    Ok(next)
}

fn load() -> Result<Settings, String> {
    match fs::read(CONFIG) {
        Ok(b) => {
            let s: Settings = serde_json::from_slice(&b)
                .map_err(|_| "Saved Wi-Fi uplink configuration is invalid")?;
            validate(&s)?;
            Ok(s)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(_) => Err("Cannot read saved Wi-Fi uplink configuration".into()),
    }
}

fn persist(s: &Settings) -> Result<(), String> {
    fs::create_dir_all(DIR).map_err(|_| "Cannot create Wi-Fi uplink state directory")?;
    fs::set_permissions(DIR, fs::Permissions::from_mode(0o700))
        .map_err(|_| "Cannot protect Wi-Fi uplink state directory")?;
    storage::atomic_write(Path::new(CONFIG), &serde_json::to_vec(s).unwrap())
        .map_err(|_| "Cannot save Wi-Fi uplink configuration".into())
}

// ---------- Supplicant configuration ----------

fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes(chunk[i * 4..i * 4 + 4].try_into().unwrap());
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = h;
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let t = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*wi);
            (e, d, c, b, a) = (d, c, b.rotate_left(30), a, t);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 20];
    for (i, x) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&x.to_be_bytes());
    }
    out
}

fn hmac_sha1(key: &[u8], msg: &[u8]) -> [u8; 20] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..20].copy_from_slice(&sha1(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner: Vec<u8> = k.iter().map(|b| b ^ 0x36).collect();
    inner.extend_from_slice(msg);
    let mut outer: Vec<u8> = k.iter().map(|b| b ^ 0x5c).collect();
    outer.extend_from_slice(&sha1(&inner));
    sha1(&outer)
}

/// The WPA2 pre-shared key (IEEE 802.11i: PBKDF2-HMAC-SHA1, 4096 rounds, 32 bytes).
/// Writing the key instead of the passphrase keeps quotes and `#` in passwords away from
/// the supplicant's configuration parser.
fn wpa_psk(passphrase: &str, ssid: &[u8]) -> String {
    let mut out = Vec::with_capacity(40);
    for block in 1u32..=2 {
        let mut salt = ssid.to_vec();
        salt.extend_from_slice(&block.to_be_bytes());
        let mut u = hmac_sha1(passphrase.as_bytes(), &salt);
        let mut t = u;
        for _ in 1..4096 {
            u = hmac_sha1(passphrase.as_bytes(), &u);
            for (x, y) in t.iter_mut().zip(u) {
                *x ^= y;
            }
        }
        out.extend_from_slice(&t);
    }
    hex(&out[..32])
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Names and SAE passwords are written as hex, which the supplicant reads for any
/// string field, so no user text is ever quoted into the file.
fn supplicant_config(s: &Settings) -> String {
    let mut out = format!("ctrl_interface={RUN}/ctrl\nupdate_config=0\nap_scan=1\n");
    let count = s.networks.len();
    for (i, n) in s.networks.iter().enumerate() {
        out.push_str(&format!(
            "network={{\n\tssid={}\n\tid_str=\"ou{}\"\n\tpriority={}\n",
            hex(n.ssid.as_bytes()),
            n.id,
            count - i
        ));
        if n.hidden {
            out.push_str("\tscan_ssid=1\n");
        }
        match n.security {
            Security::Open => out.push_str("\tkey_mgmt=NONE\n"),
            Security::Wpa2 | Security::Wpa2Wpa3 => {
                let psk = if n.password.len() == 64 {
                    n.password.to_ascii_lowercase()
                } else {
                    wpa_psk(&n.password, n.ssid.as_bytes())
                };
                out.push_str(&format!("\tkey_mgmt=WPA-PSK\n\tpsk={psk}\n"));
            }
            Security::Wpa3 => out.push_str(&format!(
                "\tkey_mgmt=SAE\n\tieee80211w=2\n\tsae_password={}\n",
                hex(n.password.as_bytes())
            )),
        }
        out.push_str("}\n");
    }
    out
}

/// udhcpc hook: puts the address on the interface without a prefix route (the subnet is
/// routed only in the uplink table) and leaves the lease for the agent.
const DHCP_SCRIPT: &str = r#"#!/bin/sh
L=/tmp/openui-uplink/lease
case "$1" in
deconfig)
 ip -4 addr flush dev "$interface"
 rm -f "$L"
 ;;
bound|renew)
 m=${mask:-$subnet}
 cur=$(ip -4 -o addr show dev "$interface" | awk '{print $4}' | cut -d/ -f1)
 if [ "$cur" != "$ip" ]; then
  ip -4 addr flush dev "$interface"
  ip -4 addr add "$ip/$m" dev "$interface" noprefixroute
 fi
 printf 'ip=%s\nmask=%s\nrouter=%s\ndns=%s\n' "$ip" "$m" "${router%% *}" "$dns" > "$L.tmp" && mv "$L.tmp" "$L"
 ;;
esac
exit 0
"#;

// ---------- Lease ----------

#[derive(Clone, Debug, PartialEq, Eq)]
struct Lease {
    ip: Ipv4Addr,
    prefix: u8,
    gateway: Ipv4Addr,
    dns: Vec<Ipv4Addr>,
}

impl Lease {
    fn network(&self) -> (Ipv4Addr, u8) {
        let mask = if self.prefix == 0 {
            0
        } else {
            u32::MAX << (32 - u32::from(self.prefix))
        };
        (Ipv4Addr::from(u32::from(self.ip) & mask), self.prefix)
    }
    fn subnet(&self) -> String {
        let (net, prefix) = self.network();
        format!("{net}/{prefix}")
    }
    /// The resolver DNS is sent to while online: the network's own, else a public one.
    fn resolver(&self) -> Ipv4Addr {
        self.dns
            .first()
            .copied()
            .unwrap_or(Ipv4Addr::new(223, 5, 5, 5))
    }
}

fn prefix_of(mask: &str) -> Option<u8> {
    if let Ok(p) = mask.parse::<u8>() {
        return (p <= 32).then_some(p);
    }
    let m = u32::from(mask.parse::<Ipv4Addr>().ok()?);
    (m.leading_ones() + m.trailing_zeros() == 32).then_some(m.leading_ones() as u8)
}

fn parse_lease(text: &str) -> Option<Lease> {
    let field = |k: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(k)?.strip_prefix('='))
            .map(str::trim)
    };
    let lease = Lease {
        ip: field("ip")?.parse().ok()?,
        prefix: prefix_of(field("mask")?)?,
        gateway: field("router")?.parse().ok()?,
        dns: field("dns")
            .unwrap_or("")
            .split_whitespace()
            .filter_map(|d| d.parse().ok())
            .take(3)
            .collect(),
    };
    (lease.prefix >= 8 && lease.prefix <= 30 && !lease.ip.is_unspecified()).then_some(lease)
}

fn overlaps(a: (Ipv4Addr, u8), b: (Ipv4Addr, u8)) -> bool {
    let prefix = a.1.min(b.1);
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(prefix))
    };
    u32::from(a.0) & mask == u32::from(b.0) & mask
}

fn lan_networks() -> Vec<(Ipv4Addr, u8)> {
    egress::run(
        "ip",
        &[
            "-o", "-4", "route", "show", "dev", "br-lan", "scope", "link",
        ],
    )
    .unwrap_or_default()
    .lines()
    .filter_map(|l| {
        let (net, prefix) = l.split_whitespace().next()?.split_once('/')?;
        Some((net.parse().ok()?, prefix.parse().ok()?))
    })
    .collect()
}

// ---------- Radio ----------

#[derive(Debug, Default, PartialEq, Eq)]
struct Radio {
    /// `phy2` for `phy#2`: the wiphy carrying the hotspot. Its index changes on every
    /// firmware Wi-Fi reload, so it is read again on each use.
    phy: Option<String>,
    aps: Vec<(String, Option<u16>)>,
    station: bool,
}

/// `iw dev`: interfaces grouped under `phy#N` headers.
fn parse_iw_dev(text: &str) -> Radio {
    let mut radio = Radio::default();
    let (mut phy, mut iface, mut kind) = (String::new(), String::new(), String::new());
    let mut channel = None;
    let mut first_phy = None;
    let flush = |radio: &mut Radio, phy: &str, iface: &str, kind: &str, ch: Option<u16>| {
        if iface == IFACE {
            radio.station = true;
        } else if kind == "AP" && !iface.is_empty() {
            radio.aps.push((iface.to_string(), ch));
            radio.phy.get_or_insert_with(|| phy.to_string());
        }
    };
    for line in text.lines() {
        let t = line.trim();
        if let Some(n) = line.strip_prefix("phy#") {
            flush(&mut radio, &phy, &iface, &kind, channel);
            phy = format!("phy{}", n.trim());
            first_phy.get_or_insert_with(|| phy.clone());
            (iface, kind, channel) = (String::new(), String::new(), None);
        } else if let Some(name) = t.strip_prefix("Interface ") {
            flush(&mut radio, &phy, &iface, &kind, channel);
            (iface, kind, channel) = (name.to_string(), String::new(), None);
        } else if let Some(k) = t.strip_prefix("type ") {
            kind = k.to_string();
        } else if let Some(c) = t.strip_prefix("channel ") {
            channel = c.split_whitespace().next().and_then(|c| c.parse().ok());
        }
    }
    flush(&mut radio, &phy, &iface, &kind, channel);
    if radio.phy.is_none() {
        radio.phy = first_phy;
    }
    radio
}

fn radio() -> Radio {
    parse_iw_dev(&egress::run("iw", &["dev"]).unwrap_or_default())
}

fn iface_exists() -> bool {
    Path::new(&format!("/sys/class/net/{IFACE}")).exists()
}

fn create_iface(radio: &Radio) -> Result<(), String> {
    let phy = radio
        .phy
        .as_deref()
        .ok_or("The Wi-Fi radio is off; turn the hotspot on first")?;
    egress::run(
        "iw",
        &["phy", phy, "interface", "add", IFACE, "type", "managed"],
    )
    .map_err(|_| "The Wi-Fi radio refused a client interface".to_string())?;
    // An address shared with a hotspot would confuse the upstream network.
    let addr = |i: &str| fs::read_to_string(format!("/sys/class/net/{i}/address")).ok();
    if let Some(own) = addr(IFACE) {
        if radio
            .aps
            .iter()
            .any(|(ap, _)| addr(ap).as_deref() == Some(&own))
        {
            let mut bytes: Vec<u8> = own
                .trim()
                .split(':')
                .filter_map(|b| u8::from_str_radix(b, 16).ok())
                .collect();
            if bytes.len() == 6 {
                bytes[0] |= 0x02;
                bytes[5] ^= 0x5a;
                let mac = bytes
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(":");
                let _ = egress::run("ip", &["link", "set", IFACE, "address", &mac]);
            }
        }
    }
    for (key, value) in [("ipv6/conf", "disable_ipv6"), ("ipv4/conf", "rp_filter")] {
        let path = format!("/proc/sys/net/{key}/{IFACE}/{value}");
        let _ = fs::write(path, if value == "rp_filter" { "2\n" } else { "1\n" });
    }
    egress::run("ip", &["link", "set", IFACE, "up"])
        .map(|_| ())
        .map_err(|_| "Cannot bring the Wi-Fi client interface up".into())
}

fn delete_iface() {
    if iface_exists() {
        let _ = egress::run("iw", &["dev", IFACE, "del"]);
    }
}

// ---------- Supplicant control ----------

fn ctrl_dir() -> PathBuf {
    PathBuf::from(format!("{RUN}/ctrl"))
}

/// One request on the supplicant's control socket.
fn wpa(cmd: &str, timeout: Duration) -> Result<String, String> {
    let local = PathBuf::from(format!(
        "{RUN}/agent-{}-{}",
        std::process::id(),
        NEXT_SOCKET.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_file(&local);
    let result = (|| {
        let sock = UnixDatagram::bind(&local).map_err(|_| "control socket")?;
        sock.set_read_timeout(Some(timeout))
            .map_err(|_| "control socket")?;
        sock.connect(ctrl_dir().join(IFACE))
            .map_err(|_| "supplicant not running")?;
        sock.send(cmd.as_bytes()).map_err(|_| "control socket")?;
        let mut buf = vec![0u8; 16384];
        loop {
            let n = sock
                .recv(&mut buf)
                .map_err(|_| "supplicant did not answer")?;
            let text = String::from_utf8_lossy(&buf[..n]).to_string();
            // Unsolicited events arrive only on attached sockets; skip any anyway.
            if !text.starts_with('<') {
                return Ok(text);
            }
        }
    })();
    let _ = fs::remove_file(&local);
    result.map_err(|e: &str| e.to_string())
}

fn kv(text: &str, key: &str) -> Option<String> {
    text.lines()
        .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
        .map(|v| v.trim().to_string())
}

/// Decode the supplicant's and iw's printf-style escapes (`\xNN`, `\\`, `\"`).
fn unescape(s: &str) -> Option<String> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 1 < b.len() {
            if b[i + 1] == b'x' && i + 3 < b.len() {
                out.push(u8::from_str_radix(&s[i + 2..i + 4], 16).ok()?);
                i += 4;
                continue;
            }
            out.push(match b[i + 1] {
                b'n' => b'\n',
                b't' => b'\t',
                b'e' => 0x1b,
                c => c,
            });
            i += 2;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

// ---------- Scanning ----------

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Seen {
    pub ssid: String,
    pub bssid: String,
    pub frequency: u32,
    pub signal: i32,
    /// None for WEP, enterprise and other networks the uplink cannot join.
    pub security: Option<Security>,
}

fn security_of(psk: bool, sae: bool, encrypted: bool) -> Option<Security> {
    match (psk, sae) {
        (true, true) => Some(Security::Wpa2Wpa3),
        (true, false) => Some(Security::Wpa2),
        (false, true) => Some(Security::Wpa3),
        _ if !encrypted => Some(Security::Open),
        _ => None,
    }
}

/// One entry per name, strongest first; hidden networks are dropped.
fn dedup(mut seen: Vec<Seen>) -> Vec<Seen> {
    seen.retain(|s| !s.ssid.is_empty() && !s.ssid.chars().all(|c| c == '\0'));
    seen.sort_by_key(|s| std::cmp::Reverse(s.signal));
    let mut out: Vec<Seen> = Vec::new();
    for s in seen {
        if !out.iter().any(|o| o.ssid == s.ssid) {
            out.push(s);
        }
    }
    out.truncate(60);
    out
}

/// `SCAN_RESULTS`: `bssid\tfrequency\tsignal\tflags\tssid` after a header line.
fn parse_scan_results(text: &str) -> Vec<Seen> {
    let seen = text
        .lines()
        .skip(1)
        .filter_map(|l| {
            let f: Vec<&str> = l.splitn(5, '\t').collect();
            if f.len() < 5 {
                return None;
            }
            let flags = f[3];
            let psk = flags.contains("-PSK");
            let sae = flags.contains("SAE");
            let encrypted = flags.contains("WPA") || flags.contains("WEP") || flags.contains("RSN");
            Some(Seen {
                ssid: unescape(f[4])?,
                bssid: f[0].to_ascii_lowercase(),
                frequency: f[1].parse().ok()?,
                signal: f[2].parse().ok()?,
                security: if flags.contains("EAP") {
                    None
                } else {
                    security_of(psk, sae, encrypted)
                },
            })
        })
        .collect();
    dedup(seen)
}

/// `iw dev <ap> scan`: a `BSS <mac>(on …)` header, then indented fields.
fn parse_iw_scan(text: &str) -> Vec<Seen> {
    struct Cur {
        bssid: String,
        freq: u32,
        signal: i32,
        ssid: Option<String>,
        privacy: bool,
        psk: bool,
        sae: bool,
        eap: bool,
        wpa: bool,
    }
    let finish = |c: Cur| -> Option<Seen> {
        Some(Seen {
            ssid: c.ssid?,
            bssid: c.bssid,
            frequency: c.freq,
            signal: c.signal,
            security: if c.eap {
                None
            } else {
                security_of(c.psk, c.sae, c.privacy || c.wpa)
            },
        })
    };
    let mut seen = Vec::new();
    let mut cur: Option<Cur> = None;
    for line in text.lines() {
        let t = line.trim();
        if let Some(rest) = line.strip_prefix("BSS ") {
            if let Some(c) = cur.take() {
                seen.extend(finish(c));
            }
            cur = Some(Cur {
                bssid: rest
                    .split(['(', ' '])
                    .next()
                    .unwrap_or("")
                    .to_ascii_lowercase(),
                freq: 0,
                signal: -100,
                ssid: None,
                privacy: false,
                psk: false,
                sae: false,
                eap: false,
                wpa: false,
            });
            continue;
        }
        let Some(c) = cur.as_mut() else { continue };
        if let Some(v) = t.strip_prefix("freq:") {
            c.freq = v
                .trim()
                .split('.')
                .next()
                .unwrap_or("")
                .parse()
                .unwrap_or(0);
        } else if let Some(v) = t.strip_prefix("signal:") {
            c.signal = v
                .split_whitespace()
                .next()
                .and_then(|n| n.parse::<f64>().ok())
                .map_or(-100, |n| n as i32);
        } else if let Some(v) = t.strip_prefix("SSID:") {
            c.ssid = unescape(v.trim());
        } else if t.starts_with("capability:") && t.contains("Privacy") {
            c.privacy = true;
        } else if t.starts_with("RSN:") || t.starts_with("WPA:") {
            c.wpa = true;
        } else if let Some(v) = t.strip_prefix("* Authentication suites:") {
            c.psk |= v
                .split_whitespace()
                .any(|s| s == "PSK" || s == "PSK/SHA-256");
            c.sae |= v.split_whitespace().any(|s| s == "SAE");
            c.eap |= v.split_whitespace().any(|s| s.starts_with("IEEE"));
        }
    }
    if let Some(c) = cur.take() {
        seen.extend(finish(c));
    }
    dedup(seen)
}

/// Nearby networks. Uses the running supplicant when there is one, otherwise an
/// off-channel scan on a hotspot interface, which keeps the hotspot up.
pub fn scan() -> (u16, Value) {
    let result = (|| -> Result<Vec<Seen>, String> {
        if wpa("PING", Duration::from_secs(1)).is_ok_and(|r| r.starts_with("PONG")) {
            let _ = wpa("SCAN", Duration::from_secs(2));
            std::thread::sleep(Duration::from_secs(4));
            let results = wpa("SCAN_RESULTS", Duration::from_secs(2))?;
            return Ok(parse_scan_results(&results));
        }
        let radio = radio();
        let iface = radio
            .aps
            .first()
            .map(|(name, _)| name.clone())
            .ok_or("The Wi-Fi radio is off; turn the hotspot on first")?;
        let mut last = String::from("Wi-Fi scan failed");
        for attempt in 0..3 {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(1500));
            }
            let mut cmd = Command::new("iw");
            cmd.args(["dev", &iface, "scan"]);
            match process_runner::output(&mut cmd, None, Duration::from_secs(20), 1 << 20) {
                Ok(o) if o.status.success() => {
                    let seen = parse_iw_scan(&String::from_utf8_lossy(&o.stdout));
                    if !seen.is_empty() {
                        return Ok(seen);
                    }
                    last = "No Wi-Fi networks found".into();
                }
                _ => last = "Wi-Fi scan failed; the radio may be busy, try again".into(),
            }
        }
        Err(last)
    })();
    match result {
        Ok(seen) => (
            200,
            json!({"ok": true, "data": {"networks": seen.iter().map(seen_json).collect::<Vec<_>>()}}),
        ),
        Err(e) => (503, json!({"ok": false, "error": e})),
    }
}

fn band(freq: u32) -> &'static str {
    match freq {
        2400..=2500 => "2.4",
        4900..=5900 => "5",
        5925..=7125 => "6",
        _ => "",
    }
}

fn channel(freq: u32) -> Option<u32> {
    match freq {
        2484 => Some(14),
        2412..=2472 => Some((freq - 2407) / 5),
        5000..=5900 => Some((freq - 5000) / 5),
        5955..=7115 => Some((freq - 5950) / 5),
        _ => None,
    }
}

fn seen_json(s: &Seen) -> Value {
    json!({"ssid": s.ssid, "bssid": s.bssid, "frequency": s.frequency, "band": band(s.frequency),
        "channel": channel(s.frequency), "signal": s.signal, "security": s.security,
        "supported": s.security.is_some()})
}

// ---------- Processes ----------

fn log_line(line: &str) {
    let mut logs = LOGS.lock().unwrap();
    if logs.len() >= 80 {
        logs.pop_front();
    }
    logs.push_back(line.trim_end().chars().take(300).collect());
}

fn capture(reader: impl Read + Send + 'static) {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut line = Vec::new();
        while reader.read_until(b'\n', &mut line).is_ok_and(|n| n > 0) {
            log_line(&String::from_utf8_lossy(&line));
            line.clear();
        }
    });
}

/// Why joining fails, from the supplicant's latest events.
fn join_problem() -> Option<&'static str> {
    let logs = LOGS.lock().unwrap();
    let recent = logs.iter().rev().take(20);
    for l in recent {
        if l.contains("reason=WRONG_KEY") || l.contains("4-Way Handshake failed") {
            return Some("Wrong Wi-Fi password");
        }
        if l.contains("CTRL-EVENT-CONNECTED") {
            return None;
        }
        if l.contains("CTRL-EVENT-ASSOC-REJECT") {
            return Some("The upstream network rejected the connection");
        }
        if l.contains("CTRL-EVENT-NETWORK-NOT-FOUND") {
            return Some("No saved network is in range");
        }
    }
    None
}

fn find_tool(name: &str) -> Option<PathBuf> {
    let path = std::env::var("PATH").unwrap_or_default();
    let found = path
        .split(':')
        .chain(["/usr/sbin", "/usr/bin", "/sbin", "/bin", "/data/bin"])
        .map(|d| Path::new(d).join(name))
        .find(|p| p.is_file());
    found
}

fn spawn(program: &Path, args: &[&str]) -> Result<Child, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| format!("Cannot start {}", program.display()))?;
    capture(child.stdout.take().unwrap());
    capture(child.stderr.take().unwrap());
    Ok(child)
}

fn alive(child: &mut Option<Child>) -> bool {
    match child.as_mut().map(|c| c.try_wait()) {
        Some(Ok(None)) => true,
        Some(_) => {
            if let Some(mut c) = child.take() {
                let _ = c.wait();
            }
            false
        }
        None => false,
    }
}

fn stop(child: &mut Option<Child>) {
    if let Some(mut c) = child.take() {
        let pid = c.id() as i32;
        unsafe { libc::kill(pid, libc::SIGTERM) };
        for _ in 0..30 {
            if matches!(c.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = c.kill();
        let _ = c.wait();
    }
}

/// Processes for our interface left by a previous agent.
fn kill_stale() {
    let Ok(entries) = fs::read_dir("/proc") else {
        return;
    };
    let me = std::process::id().to_string();
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !name.bytes().all(|b| b.is_ascii_digit()) || name == me {
            continue;
        }
        let Ok(cmdline) = fs::read(e.path().join("cmdline")) else {
            continue;
        };
        let args: Vec<&[u8]> = cmdline.split(|b| *b == 0).collect();
        let ours = args.iter().any(|a| *a == IFACE.as_bytes())
            && args
                .iter()
                .any(|a| a.starts_with(RUN.as_bytes()) && a.len() > RUN.len());
        if ours {
            if let Ok(pid) = name.parse::<i32>() {
                unsafe { libc::kill(pid, libc::SIGTERM) };
            }
        }
    }
}

// ---------- Network policy ----------

fn sh(text: &str) -> Result<(), String> {
    egress::script("Wi-Fi uplink", text)
}

/// Hooks in insertion order. Each chain only ever matches the uplink interface, LAN
/// DNS while online, or LAN IPv6 forwarding while online.
const HOOKS: [(&str, &str, &str, &str); 7] = [
    ("iptables", "filter", "INPUT", "IN"),
    ("iptables", "filter", "FORWARD", "FWD"),
    ("iptables", "nat", "POSTROUTING", "NAT"),
    ("iptables", "mangle", "FORWARD", "MSS"),
    ("iptables", "nat", "PREROUTING", "DNS"),
    ("iptables", "nat", "OUTPUT", "OUT"),
    ("ip6tables", "filter", "FORWARD", "V6"),
];

/// Chains and hooks, with the interface guards that hold whether or not it is online:
/// nothing from the upstream network may reach the device or the LAN unasked.
fn firewall_script() -> String {
    let p = PREFIX;
    let mut out = String::from("set -eu\n");
    for (bin, table, _, chain) in HOOKS {
        out.push_str(&format!(
            "{bin} -w 2 -t {table} -N {p}_{chain} 2>/dev/null || {bin} -w 2 -t {table} -F {p}_{chain}\n"
        ));
    }
    out.push_str(&format!(
        "iptables -w 2 -A {p}_IN -i {IFACE} -m conntrack --ctstate RELATED,ESTABLISHED -j ACCEPT
iptables -w 2 -A {p}_IN -i {IFACE} -p udp --sport 67 --dport 68 -j ACCEPT
iptables -w 2 -A {p}_IN -i {IFACE} -j DROP
iptables -w 2 -A {p}_FWD -i br-lan -o {IFACE} -j ACCEPT
iptables -w 2 -A {p}_FWD -i {IFACE} -o br-lan -m conntrack --ctstate RELATED,ESTABLISHED -j ACCEPT
iptables -w 2 -A {p}_FWD -i {IFACE} -j DROP
iptables -w 2 -t nat -A {p}_NAT -o {IFACE} -j MASQUERADE
iptables -w 2 -t mangle -A {p}_MSS -o {IFACE} -p tcp --tcp-flags SYN,RST SYN -j TCPMSS --clamp-mss-to-pmtu
"
    ));
    out.push_str(&hook_script());
    out
}

fn hook_script() -> String {
    let mut out = String::new();
    for (bin, table, hook, chain) in HOOKS {
        let c = format!("{PREFIX}_{chain}");
        out.push_str(&format!(
            "{bin} -w 2 -t {table} -C {hook} -j {c} 2>/dev/null || {bin} -w 2 -t {table} -I {hook} 1 -j {c}\n"
        ));
    }
    out
}

/// Exits non-zero unless every chain is hooked (a firmware firewall reload drops hooks).
fn hooks_check() -> String {
    let mut out = String::from("set -e\n");
    for (bin, table, hook, chain) in HOOKS {
        out.push_str(&format!(
            "{bin} -w 2 -t {table} -C {hook} -j {PREFIX}_{chain} 2>/dev/null\n"
        ));
    }
    out
}

fn cleanup_script() -> String {
    let mut out = String::new();
    for (bin, table, hook, chain) in HOOKS {
        let c = format!("{PREFIX}_{chain}");
        out.push_str(&format!(
            "while {bin} -w 2 -t {table} -C {hook} -j {c} 2>/dev/null; do {bin} -w 2 -t {table} -D {hook} -j {c} || break; done\n{bin} -w 2 -t {table} -F {c} 2>/dev/null || true\n{bin} -w 2 -t {table} -X {c} 2>/dev/null || true\n"
        ));
    }
    for prio in [PRIO_SOURCE, PRIO_MAIN, PRIO_ALL] {
        out.push_str(&format!(
            "while ip -4 rule del priority {prio} 2>/dev/null; do :; done\n"
        ));
    }
    out.push_str(&format!(
        "ip -4 route flush table {TABLE} 2>/dev/null || true\n"
    ));
    out
}

/// The uplink table and the source rule, for a lease.
fn lease_script(l: &Lease) -> String {
    format!(
        "set -eu
ip -4 route flush table {TABLE} 2>/dev/null || true
ip -4 route replace {subnet} dev {IFACE} src {ip} table {TABLE}
ip -4 route replace default via {gw} dev {IFACE} src {ip} table {TABLE}
while ip -4 rule del priority {PRIO_SOURCE} 2>/dev/null; do :; done
ip -4 rule add priority {PRIO_SOURCE} from {ip} table {TABLE}
",
        subnet = l.subnet(),
        ip = l.ip,
        gw = l.gateway
    )
}

/// Everything onto the uplink: rules, DNS redirect and IPv6 hold for LAN devices.
/// Devices routed by WireGuard or Clash keep their own DNS.
fn online_script(l: &Lease) -> String {
    let (p, dns) = (PREFIX, l.resolver());
    let mut out = format!(
        "set -eu
while ip -4 rule del priority {PRIO_MAIN} 2>/dev/null; do :; done
while ip -4 rule del priority {PRIO_ALL} 2>/dev/null; do :; done
ip -4 rule add priority {PRIO_MAIN} table main suppress_prefixlength 0
ip -4 rule add priority {PRIO_ALL} table {TABLE}
iptables -w 2 -t nat -F {p}_DNS
iptables -w 2 -t nat -F {p}_OUT
ip6tables -w 2 -F {p}_V6
"
    );
    for proto in ["udp", "tcp"] {
        out.push_str(&format!(
            "iptables -w 2 -t nat -A {p}_DNS -i br-lan -m mark --mark 0/{TUNNEL_MARKS} -p {proto} --dport 53 -j DNAT --to-destination {dns}
iptables -w 2 -t nat -A {p}_OUT -o {IFACE} ! -d {dns} -p {proto} --dport 53 -j DNAT --to-destination {dns}
"
        ));
    }
    out.push_str(&format!(
        "ip6tables -w 2 -A {p}_V6 -i br-lan -o br-lan -j RETURN\nip6tables -w 2 -A {p}_V6 -i br-lan -j REJECT\n"
    ));
    out
}

fn offline_script() -> String {
    let p = PREFIX;
    format!(
        "while ip -4 rule del priority {PRIO_MAIN} 2>/dev/null; do :; done
while ip -4 rule del priority {PRIO_ALL} 2>/dev/null; do :; done
iptables -w 2 -t nat -F {p}_DNS 2>/dev/null || true
iptables -w 2 -t nat -F {p}_OUT 2>/dev/null || true
ip6tables -w 2 -F {p}_V6 2>/dev/null || true
"
    )
}

fn rules_present(online: bool) -> bool {
    let rules = egress::run("ip", &["-4", "rule", "show"]).unwrap_or_default();
    let has = |prio: &str| rules.lines().any(|l| l.starts_with(&format!("{prio}:")));
    has(PRIO_SOURCE) && (!online || (has(PRIO_MAIN) && has(PRIO_ALL)))
}

/// What the connectivity check found.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Probe {
    Online,
    /// Something answered for the resolver with a page of its own: a network that wants a
    /// web login first. The login page, when the answer named one.
    Portal(Option<String>),
    Unreachable,
}

/// HTTP endpoints of public resolvers, addressed by IP so the check needs no DNS. Each
/// has a recognisable answer; a redirect or any other page instead is a login portal.
/// Not port 53: DNS leaving over the uplink is redirected to the upstream network's own
/// resolver, which answers even when that network has no internet.
/// Whether a body is the endpoint's real answer.
type Answer = fn(&str) -> bool;
const PROBES: [(Ipv4Addr, &str, Answer); 3] = [
    (
        Ipv4Addr::new(223, 5, 5, 5),
        "/resolve?name=www.taobao.com&type=1",
        alidns_answer,
    ),
    (
        Ipv4Addr::new(119, 29, 29, 29),
        "/d?dn=www.taobao.com",
        dnspod_answer,
    ),
    (
        Ipv4Addr::new(223, 6, 6, 6),
        "/resolve?name=www.taobao.com&type=1",
        alidns_answer,
    ),
];
/// Opened by a browser when a portal gave no address of its own; any portal intercepts it.
const PORTAL_FALLBACK: &str = "http://neverssl.com/";

fn alidns_answer(body: &str) -> bool {
    let compact: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    compact.starts_with('{') && compact.contains("\"Status\":0")
}

fn dnspod_answer(body: &str) -> bool {
    let t = body.trim();
    !t.is_empty() && t.split([';', ',']).all(|ip| ip.parse::<Ipv4Addr>().is_ok())
}

/// The check, from the uplink address. Packets bound to that address follow the source
/// rule, so it works whichever link carries other traffic.
fn probe(ip: Ipv4Addr) -> Probe {
    let mut portal = None;
    for (target, path, valid) in PROBES {
        let Some((status, location, body)) = http_get(ip, target, path, Duration::from_secs(3))
        else {
            continue;
        };
        if status == 200 && valid(&body) {
            return Probe::Online;
        }
        if portal.is_none() {
            let base = format!("http://{target}{path}");
            portal = Some(portal_url(&base, location.as_deref(), &body));
        }
    }
    portal.map_or(Probe::Unreachable, Probe::Portal)
}

/// The login page a portal's answer points at: its redirect, else a URL in a meta refresh
/// or script, made absolute against the probed URL. Only plain http(s) URLs pass.
fn portal_url(base: &str, location: Option<&str>, body: &str) -> Option<String> {
    let from_body = || {
        let lower = body.to_ascii_lowercase();
        [
            "url=",
            "location.href=",
            "location.href = ",
            "location=",
            "location.replace(",
        ]
        .iter()
        .find_map(|key| {
            let at = lower.find(key)? + key.len();
            let rest = body[at..].trim_start_matches(['\'', '"', ' ', '(']);
            let end = rest
                .find(['\'', '"', ' ', '>', ';', ')'])
                .unwrap_or(rest.len());
            Some(rest[..end].to_string())
        })
    };
    let raw = location.map(str::to_string).or_else(from_body)?;
    let raw = raw.trim();
    let url = if raw.starts_with("http://") || raw.starts_with("https://") {
        raw.to_string()
    } else {
        let path = raw.strip_prefix('/')?;
        let host_end = base[7..].find('/').map_or(base.len(), |i| i + 7);
        format!("{}/{path}", &base[..host_end])
    };
    (url.len() <= 512 && !url.chars().any(|c| c.is_control() || c.is_whitespace())).then_some(url)
}

fn sockaddr(ip: Ipv4Addr, port: u16) -> libc::sockaddr_in {
    let mut a: libc::sockaddr_in = unsafe { std::mem::zeroed() };
    a.sin_family = libc::AF_INET as libc::sa_family_t;
    a.sin_port = port.to_be();
    a.sin_addr = libc::in_addr {
        s_addr: u32::from(ip).to_be(),
    };
    a
}

/// A TCP connection from `source` on the uplink interface.
fn connect_from(
    source: Ipv4Addr,
    target: Ipv4Addr,
    port: u16,
    timeout: Duration,
) -> Option<TcpStream> {
    unsafe {
        let fd = libc::socket(
            libc::AF_INET,
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            0,
        );
        if fd < 0 {
            return None;
        }
        let ok = (|| {
            let dev = IFACE.as_bytes();
            if libc::setsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_BINDTODEVICE,
                dev.as_ptr().cast(),
                dev.len() as libc::socklen_t,
            ) != 0
            {
                return false;
            }
            let local = sockaddr(source, 0);
            let len = std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t;
            if libc::bind(fd, (&local as *const libc::sockaddr_in).cast(), len) != 0 {
                return false;
            }
            let remote = sockaddr(target, port);
            if libc::connect(fd, (&remote as *const libc::sockaddr_in).cast(), len) == 0 {
                return true;
            }
            if *libc::__errno_location() != libc::EINPROGRESS {
                return false;
            }
            let mut pfd = libc::pollfd {
                fd,
                events: libc::POLLOUT,
                revents: 0,
            };
            if libc::poll(&mut pfd, 1, timeout.as_millis() as i32) != 1 {
                return false;
            }
            let mut err: libc::c_int = 0;
            let mut elen = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_ERROR,
                (&mut err as *mut libc::c_int).cast(),
                &mut elen,
            ) == 0
                && err == 0
        })();
        if !ok {
            libc::close(fd);
            return None;
        }
        let stream = TcpStream::from_raw_fd(fd);
        (stream.set_nonblocking(false).is_ok()
            && stream.set_read_timeout(Some(timeout)).is_ok()
            && stream.set_write_timeout(Some(timeout)).is_ok())
        .then_some(stream)
    }
}

/// `GET http://<target><path>` from the uplink address: status, Location and up to 16 KiB
/// of body. None when nothing answered.
fn http_get(
    source: Ipv4Addr,
    target: Ipv4Addr,
    path: &str,
    timeout: Duration,
) -> Option<(u16, Option<String>, String)> {
    let mut stream = connect_from(source, target, 80, timeout)?;
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {target}\r\nUser-Agent: Mozilla/5.0\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).ok()?;
    let deadline = Instant::now() + timeout;
    let mut raw = Vec::new();
    let mut buf = [0u8; 4096];
    while raw.len() < 16384 && Instant::now() < deadline {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&buf[..n]),
            Err(_) => break,
        }
    }
    parse_http(&String::from_utf8_lossy(&raw))
}

fn parse_http(text: &str) -> Option<(u16, Option<String>, String)> {
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((text, ""));
    let mut lines = head.lines();
    let status = lines
        .next()?
        .strip_prefix("HTTP/1.")?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()?;
    let location = lines.find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim()
            .eq_ignore_ascii_case("location")
            .then(|| v.trim().to_string())
    });
    let chunked = head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked");
    let body = if chunked {
        dechunk(body)
    } else {
        body.to_string()
    };
    Some((status, location, body))
}

fn dechunk(mut body: &str) -> String {
    let mut out = String::new();
    while let Some((size, rest)) = body.split_once("\r\n") {
        let Ok(n) = usize::from_str_radix(size.split(';').next().unwrap_or("").trim(), 16) else {
            break;
        };
        if n == 0 || rest.len() < n {
            out.push_str(&rest[..rest.len().min(n)]);
            break;
        }
        out.push_str(&rest[..n]);
        body = rest[n..].trim_start_matches("\r\n");
    }
    out
}

// ---------- Supervisor ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Off,
    Unavailable,
    Connecting,
    ObtainingIp,
    Checking,
    Online,
    NoInternet,
    /// Associated with an address, but the network answers with its own login page.
    NeedsLogin,
    /// Login mode: traffic is on the uplink until the login is done or the window ends.
    LoggingIn,
}

impl State {
    /// One character for the screen's `R` record.
    pub fn code(self) -> char {
        match self {
            State::Off => 'o',
            State::Unavailable => 'u',
            State::Connecting => 'c',
            State::ObtainingIp => 'd',
            State::Checking => 'k',
            State::Online => 'n',
            State::NoInternet => 'i',
            State::NeedsLogin => 'p',
            State::LoggingIn => 'g',
        }
    }
}

#[derive(Clone)]
struct Link {
    ssid: String,
    bssid: String,
    frequency: u32,
    network_id: Option<u64>,
}

#[derive(Clone)]
struct Snapshot {
    state: State,
    reason: Option<String>,
    link: Option<Link>,
    signal: Option<i32>,
    lease: Option<Lease>,
    online: bool,
    online_since: Option<u64>,
    /// The login page the network's portal pointed at.
    portal_url: Option<String>,
    /// Interface counters when it was added, so traffic counts from then.
    rx_base: u64,
    tx_base: u64,
}

impl Snapshot {
    const fn new() -> Self {
        Self {
            state: State::Off,
            reason: None,
            link: None,
            signal: None,
            lease: None,
            online: false,
            online_since: None,
            portal_url: None,
            rx_base: 0,
            tx_base: 0,
        }
    }
}

/// The supervisor's own state: the snapshot plus the processes it runs.
struct Runtime {
    view: Snapshot,
    supplicant: Option<Child>,
    dhcp: Option<Child>,
    /// The saved revision the supplicant configuration was last checked for, and the
    /// text written then: a save that leaves it unchanged must not drop the connection.
    written: Option<(u64, String)>,
    passes: u32,
    fails: u32,
    /// The online rules and DNS redirect are in place (online, or login mode).
    routing: bool,
    last_check: Option<Instant>,
    lost_since: Option<Instant>,
    /// The phy seen on the previous pass: a vdev is only added to a stable radio.
    last_phy: Option<String>,
    /// The interface was added by this run and not removed by it.
    created: bool,
    /// When the interface vanished without the agent removing it.
    losses: VecDeque<Instant>,
    /// The saved revision the relay was paused at.
    paused: Option<u64>,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Online after `PASSES_TO_JOIN` passing checks in a row, offline after
/// `FAILS_TO_DROP` failures in a row. Returns the new online flag.
fn judge(online: bool, passed: bool, passes: &mut u32, fails: &mut u32) -> bool {
    if passed {
        *passes = passes.saturating_add(1);
        *fails = 0;
    } else {
        *fails = fails.saturating_add(1);
        *passes = 0;
    }
    if online {
        *fails < FAILS_TO_DROP
    } else {
        *passes >= PASSES_TO_JOIN
    }
}

fn go_offline(rt: &mut Runtime) {
    if rt.routing {
        let _ = sh(&offline_script());
        log_line("uplink: traffic back on the cellular link");
    }
    rt.routing = false;
    rt.view.online = false;
    rt.view.online_since = None;
}

/// Whether login mode is on; an elapsed window is cleared.
fn login_active() -> bool {
    let mut login = LOGIN.lock().unwrap_or_else(|e| e.into_inner());
    if login.is_some_and(|until| Instant::now() >= until) {
        *login = None;
        log_line("uplink: login mode ended");
    }
    login.is_some()
}

fn login_left() -> Option<u64> {
    LOGIN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .map(|until| until.saturating_duration_since(Instant::now()).as_secs())
}

fn drop_lease(rt: &mut Runtime) {
    go_offline(rt);
    stop(&mut rt.dhcp);
    let _ = fs::remove_file(format!("{RUN}/lease"));
    if rt.view.lease.take().is_some() {
        let _ = sh(&format!(
            "while ip -4 rule del priority {PRIO_SOURCE} 2>/dev/null; do :; done\nip -4 route flush table {TABLE} 2>/dev/null || true\nip -4 addr flush dev {IFACE} 2>/dev/null || true\n"
        ));
    }
    rt.passes = 0;
    rt.fails = 0;
    rt.last_check = None;
    rt.view.portal_url = None;
}

/// Everything down: processes, interface, rules and chains.
fn teardown(rt: &mut Runtime) {
    drop_lease(rt);
    stop(&mut rt.supplicant);
    GENERATION.fetch_add(1, Ordering::Relaxed);
    kill_stale();
    delete_iface();
    let _ = sh(&cleanup_script());
    rt.created = false;
    rt.written = None;
    rt.view.link = None;
    rt.view.signal = None;
    rt.lost_since = None;
}

fn prepare_run_dir() -> Result<(), String> {
    fs::create_dir_all(ctrl_dir()).map_err(|_| "Cannot create the uplink runtime directory")?;
    fs::set_permissions(RUN, fs::Permissions::from_mode(0o700))
        .map_err(|_| "Cannot protect the uplink runtime directory")?;
    let script = Path::new(RUN).join("dhcp.sh");
    if fs::read_to_string(&script).ok().as_deref() != Some(DHCP_SCRIPT) {
        storage::atomic_write(&script, DHCP_SCRIPT.as_bytes())
            .map_err(|_| "Cannot write the DHCP hook")?;
    }
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700))
        .map_err(|_| "Cannot write the DHCP hook".into())
}

/// Copies the supplicant's events (wrong password, network not found…) into the log
/// through an attached control socket; its stdout is block-buffered in a pipe.
fn monitor(generation: u64) {
    std::thread::spawn(move || {
        let local = PathBuf::from(format!("{RUN}/monitor-{}-{generation}", std::process::id()));
        while GENERATION.load(Ordering::Relaxed) == generation {
            let _ = fs::remove_file(&local);
            let attached = UnixDatagram::bind(&local).ok().filter(|sock| {
                sock.set_read_timeout(Some(Duration::from_secs(5))).is_ok()
                    && sock.connect(ctrl_dir().join(IFACE)).is_ok()
                    && sock.send(b"ATTACH").is_ok()
            });
            let Some(sock) = attached else {
                std::thread::sleep(Duration::from_secs(1));
                continue;
            };
            let mut buf = vec![0u8; 4096];
            let mut idle = 0;
            while GENERATION.load(Ordering::Relaxed) == generation {
                match sock.recv(&mut buf) {
                    Ok(n) => {
                        idle = 0;
                        let text = String::from_utf8_lossy(&buf[..n]);
                        if let Some(event) = text.strip_prefix('<').and_then(|t| t.split_once('>'))
                        {
                            log_line(event.1);
                        }
                    }
                    // Quiet for a minute: check the supplicant is still there.
                    Err(_) => {
                        idle += 1;
                        if idle >= 12 {
                            if sock.send(b"PING").is_err() {
                                break;
                            }
                            idle = 0;
                        }
                    }
                }
            }
        }
        let _ = fs::remove_file(&local);
    });
}

fn ensure_supplicant(rt: &mut Runtime, s: &Settings) -> Result<(), String> {
    let conf = Path::new(RUN).join("wpa.conf");
    if rt.written.as_ref().map(|w| w.0) != Some(s.revision) {
        let text = supplicant_config(s);
        if rt.written.as_ref().map(|w| &w.1) != Some(&text) {
            storage::atomic_write(&conf, text.as_bytes())
                .map_err(|_| "Cannot write the Wi-Fi client configuration")?;
            if alive(&mut rt.supplicant) {
                let _ = wpa("RECONFIGURE", Duration::from_secs(3));
            }
        }
        rt.written = Some((s.revision, text));
    }
    if alive(&mut rt.supplicant) {
        return Ok(());
    }
    let program =
        find_tool("wpa_supplicant").ok_or("wpa_supplicant is not installed on this device")?;
    let _ = fs::remove_file(ctrl_dir().join(IFACE));
    let conf = conf.to_string_lossy().to_string();
    rt.supplicant = Some(spawn(
        &program,
        &["-i", IFACE, "-D", "nl80211", "-c", &conf],
    )?);
    monitor(GENERATION.fetch_add(1, Ordering::Relaxed) + 1);
    Ok(())
}

fn read_link(rt: &mut Runtime) {
    let status = wpa("STATUS", Duration::from_secs(2)).unwrap_or_default();
    if kv(&status, "wpa_state").as_deref() == Some("COMPLETED") {
        rt.view.link = Some(Link {
            ssid: kv(&status, "ssid")
                .and_then(|s| unescape(&s))
                .unwrap_or_default(),
            bssid: kv(&status, "bssid").unwrap_or_default(),
            frequency: kv(&status, "freq")
                .and_then(|f| f.parse().ok())
                .unwrap_or(0),
            network_id: kv(&status, "id_str").and_then(|s| s.strip_prefix("ou")?.parse().ok()),
        });
        rt.view.signal = wpa("SIGNAL_POLL", Duration::from_secs(2))
            .ok()
            .and_then(|r| kv(&r, "RSSI"))
            .and_then(|v| v.parse().ok());
    } else {
        rt.view.link = None;
        rt.view.signal = None;
    }
}

/// The firmware has finished booting (or the device has been up for a while).
fn booted() -> bool {
    fs::read_to_string("/tmp/zte_boot_done").is_ok_and(|v| v.trim() == "1")
        || fs::read_to_string("/proc/uptime")
            .ok()
            .and_then(|u| u.split('.').next()?.parse::<u64>().ok())
            .is_some_and(|secs| secs > 180)
}

fn counters() -> (u64, u64) {
    let read = |f: &str| {
        fs::read_to_string(format!("/sys/class/net/{IFACE}/statistics/{f}"))
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(0)
    };
    (read("rx_bytes"), read("tx_bytes"))
}

fn step(rt: &mut Runtime, s: &Settings) -> Result<(), String> {
    if !s.enabled {
        *LOGIN.lock().unwrap_or_else(|e| e.into_inner()) = None;
        if rt.view.state != State::Off || iface_exists() || rt.supplicant.is_some() {
            teardown(rt);
        }
        rt.view.state = State::Off;
        rt.view.reason = None;
        return Ok(());
    }
    if rt.paused.is_some() && rt.paused != Some(s.revision) {
        rt.paused = None;
        rt.losses.clear();
    }
    if rt.paused.is_some() {
        rt.view.state = State::Unavailable;
        rt.view.reason = Some(PAUSED.into());
        return Ok(());
    }
    if !iface_exists() {
        // Gone with a firmware Wi-Fi reload (or never made): start over on the new radio.
        if rt.created {
            rt.created = false;
            rt.losses.push_back(Instant::now());
            rt.losses.retain(|t| t.elapsed() < LOSS_WINDOW);
            log_line("uplink: the client interface disappeared");
        }
        if rt.supplicant.is_some() || rt.view.lease.is_some() {
            teardown(rt);
        }
        if rt.losses.len() >= LOSSES_TO_PAUSE {
            teardown(rt);
            rt.paused = Some(s.revision);
            rt.view.state = State::Unavailable;
            rt.view.reason = Some(PAUSED.into());
            return Ok(());
        }
        if !booted() {
            rt.view.state = State::Unavailable;
            rt.view.reason = Some(BOOTING.into());
            return Ok(());
        }
        let radio = radio();
        let stable = radio.phy.is_some() && radio.phy == rt.last_phy;
        rt.last_phy = radio.phy.clone();
        if !stable {
            rt.view.state = State::Unavailable;
            rt.view.reason = Some(if radio.phy.is_none() {
                "The Wi-Fi radio is off; turn the hotspot on first".into()
            } else {
                "Waiting for the Wi-Fi radio to settle".into()
            });
            return Ok(());
        }
        if find_tool("wpa_supplicant").is_none() {
            return Err("wpa_supplicant is not installed on this device".into());
        }
        if find_tool("udhcpc").is_none() {
            return Err("udhcpc is not installed on this device".into());
        }
        prepare_run_dir()?;
        sh(&firewall_script())?;
        create_iface(&radio)?;
        rt.created = true;
        rt.written = None;
        (rt.view.rx_base, rt.view.tx_base) = counters();
        log_line(&format!(
            "uplink: client interface added on {}",
            radio.phy.unwrap_or_default()
        ));
    } else if egress::run("sh", &["-c", &hooks_check()]).is_err() {
        sh(&firewall_script())?;
    }
    let was_running = rt.supplicant.is_some();
    if was_running && !alive(&mut rt.supplicant) {
        let last = LOGS.lock().unwrap().back().cloned().unwrap_or_default();
        log_line(&format!("uplink: wpa_supplicant exited: {last}"));
    }
    ensure_supplicant(rt, s)?;
    let before = rt.view.link.as_ref().and_then(|l| l.network_id);
    read_link(rt);
    let Some(link) = rt.view.link.as_ref() else {
        let lost = *rt.lost_since.get_or_insert_with(Instant::now);
        go_offline(rt);
        if lost.elapsed() >= RELEASE_AFTER {
            drop_lease(rt);
        }
        rt.view.state = State::Connecting;
        rt.view.reason = join_problem().map(String::from);
        return Ok(());
    };
    rt.lost_since = None;
    if before.is_some() && before != link.network_id {
        // Another saved network: the old lease belongs to the old one.
        drop_lease(rt);
    }
    if !alive(&mut rt.dhcp) {
        let udhcpc = find_tool("udhcpc").ok_or("udhcpc is not installed on this device")?;
        let script = format!("{RUN}/dhcp.sh");
        rt.dhcp = Some(spawn(
            &udhcpc,
            &[
                "-f", "-R", "-i", IFACE, "-s", &script, "-t", "4", "-T", "3", "-A", "5",
            ],
        )?);
    }
    let lease = fs::read_to_string(format!("{RUN}/lease"))
        .ok()
        .and_then(|t| parse_lease(&t));
    let Some(lease) = lease else {
        if rt.view.lease.is_some() {
            drop_lease(rt);
        }
        rt.view.state = State::ObtainingIp;
        rt.view.reason = None;
        return Ok(());
    };
    if lan_networks()
        .iter()
        .any(|lan| overlaps(*lan, lease.network()))
    {
        go_offline(rt);
        rt.view.state = State::NoInternet;
        rt.view.reason = Some(format!(
            "The upstream network {} overlaps the LAN; change the LAN address",
            lease.subnet()
        ));
        return Ok(());
    }
    if rt.view.lease.as_ref() != Some(&lease) || !rules_present(false) {
        go_offline(rt);
        sh(&lease_script(&lease))?;
        rt.view.lease = Some(lease.clone());
        rt.passes = 0;
        rt.fails = 0;
        rt.last_check = None;
    }
    let login = login_active();
    // Every pass while waiting for a login, so the switch follows it at once.
    let every = if (rt.view.online || rt.fails > 0) && !login {
        CHECK_EVERY
    } else {
        LOOP
    };
    if rt.last_check.is_none_or(|t| t.elapsed() >= every) {
        let result = probe(lease.ip);
        rt.last_check = Some(Instant::now());
        match &result {
            Probe::Online => rt.view.portal_url = None,
            Probe::Portal(url) => {
                rt.view.portal_url = Some(url.clone().unwrap_or_else(|| PORTAL_FALLBACK.into()))
            }
            Probe::Unreachable => {}
        }
        let online = judge(
            rt.view.online,
            result == Probe::Online,
            &mut rt.passes,
            &mut rt.fails,
        );
        if online && !rt.view.online {
            rt.view.online = true;
            rt.view.online_since = Some(now_secs());
            *LOGIN.lock().unwrap_or_else(|e| e.into_inner()) = None;
            log_line(&format!("uplink: traffic on Wi-Fi via {}", lease.gateway));
        } else if !online && rt.view.online {
            rt.view.online = false;
            rt.view.online_since = None;
        }
    }
    let route = rt.view.online || login_active();
    if route && (!rt.routing || !rules_present(true)) {
        sh(&online_script(&lease))?;
        if !rt.routing && !rt.view.online {
            log_line("uplink: login mode, traffic on Wi-Fi until the login is done");
        }
        rt.routing = true;
    } else if !route && rt.routing {
        go_offline(rt);
    }
    rt.view.state = if rt.view.online {
        State::Online
    } else if route {
        State::LoggingIn
    } else if rt.view.portal_url.is_some() {
        State::NeedsLogin
    } else if rt.fails > 0 {
        State::NoInternet
    } else {
        State::Checking
    };
    rt.view.reason = match rt.view.state {
        State::NeedsLogin => Some(NEEDS_LOGIN.into()),
        State::NoInternet => Some(NO_INTERNET.into()),
        _ => None,
    };
    Ok(())
}

fn publish(view: &Snapshot) {
    *SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()) = view.clone();
}

fn wake() {
    *WAKE.0.lock().unwrap_or_else(|e| e.into_inner()) = true;
    WAKE.1.notify_all();
}

pub fn start() {
    std::thread::spawn(|| {
        kill_stale();
        // A previous agent's rules would keep all traffic on an uplink nobody checks;
        // this run puts them back once its own checks pass.
        let _ = sh(&offline_script());
        let mut rt = Runtime {
            view: Snapshot::new(),
            supplicant: None,
            dhcp: None,
            written: None,
            passes: 0,
            fails: 0,
            routing: false,
            last_check: None,
            lost_since: None,
            last_phy: None,
            created: false,
            losses: VecDeque::new(),
            paused: None,
        };
        let mut first = true;
        loop {
            match load() {
                Ok(s) => {
                    if first && !s.enabled {
                        // A previous agent may have left the interface and rules.
                        teardown(&mut rt);
                    }
                    if let Err(e) = step(&mut rt, &s) {
                        go_offline(&mut rt);
                        rt.view.state = State::Unavailable;
                        rt.view.reason = Some(e);
                    }
                }
                Err(e) => {
                    rt.view.state = State::Unavailable;
                    rt.view.reason = Some(e);
                }
            }
            first = false;
            publish(&rt.view);
            let (lock, cvar) = &WAKE;
            let woken = lock.lock().unwrap_or_else(|e| e.into_inner());
            let mut woken = cvar
                .wait_timeout_while(woken, LOOP, |w| !*w)
                .unwrap_or_else(|e| e.into_inner())
                .0;
            *woken = false;
        }
    });
}

// ---------- API ----------

fn traffic(view: &Snapshot) -> (u64, u64) {
    if !iface_exists() {
        return (0, 0);
    }
    let (rx, tx) = counters();
    (
        rx.saturating_sub(view.rx_base),
        tx.saturating_sub(view.tx_base),
    )
}

fn response(s: &Settings) -> Value {
    let view = SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let (rx, tx) = traffic(&view);
    let link = view.link.as_ref();
    let lease = view.lease.as_ref();
    json!({
        "revision": s.revision,
        "enabled": s.enabled,
        "state": view.state,
        "reason": view.reason,
        "ssid": link.map(|l| l.ssid.as_str()),
        "bssid": link.map(|l| l.bssid.as_str()),
        "frequency": link.map(|l| l.frequency),
        "band": link.map(|l| band(l.frequency)),
        "channel": link.and_then(|l| channel(l.frequency)),
        "network_id": link.and_then(|l| l.network_id),
        "signal": view.signal,
        "ip": lease.map(|l| l.ip.to_string()),
        "prefix": lease.map(|l| l.prefix),
        "gateway": lease.map(|l| l.gateway.to_string()),
        "dns": lease.map(|l| l.dns.iter().map(|d| d.to_string()).collect::<Vec<_>>()).unwrap_or_default(),
        "online": view.online,
        "online_since": view.online_since,
        "portal_url": view.portal_url,
        "login_remaining": login_left(),
        // Device time runs 8 h ahead on B31: the duration is computed here.
        "online_for": view.online_since.map(|t| now_secs().saturating_sub(t)),
        "rx_bytes": rx,
        "tx_bytes": tx,
        "networks": s.networks.iter().map(|n| json!({"id": n.id, "ssid": n.ssid, "security": n.security,
            "hidden": n.hidden, "has_password": !n.password.is_empty()})).collect::<Vec<_>>(),
        "max_networks": MAX_NETWORKS,
        "tools": {"wpa_supplicant": find_tool("wpa_supplicant").is_some(), "udhcpc": find_tool("udhcpc").is_some()},
    })
}

pub fn get() -> (u16, Value) {
    match load() {
        Ok(s) => (200, json!({"ok": true, "data": response(&s)})),
        Err(e) => (500, json!({"ok": false, "error": e})),
    }
}

/// Saves under the lock; the supervisor applies the change on its next pass, at once.
fn save(u: Update) -> Result<Settings, String> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let old = load()?;
    let next = edited(&old, u)?;
    persist(&next)?;
    if next.enabled != old.enabled {
        let mut view = SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner());
        view.state = if next.enabled {
            State::Connecting
        } else {
            State::Off
        };
        view.reason = None;
        if next.enabled {
            LOGS.lock().unwrap().clear();
        }
    }
    wake();
    Ok(next)
}

pub fn update(body: &[u8]) -> (u16, Value) {
    let result = serde_json::from_slice::<Update>(body)
        .map_err(|_| "Invalid Wi-Fi uplink request".to_string())
        .and_then(save);
    match result {
        Ok(s) => (200, json!({"ok": true, "data": response(&s)})),
        Err(e) => (
            if e.starts_with("Configuration changed;") {
                409
            } else {
                400
            },
            json!({"ok": false, "error": e}),
        ),
    }
}

/// Starts or ends login mode: for ten minutes traffic goes over the joined network even
/// though it has not passed the check, so its login page can be opened from a phone or
/// computer. It ends early once the check passes.
pub fn set_login(on: bool) -> Result<(), String> {
    if on {
        let enabled = load().is_ok_and(|s| s.enabled);
        let joined = SNAPSHOT
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .lease
            .is_some();
        if !enabled || !joined {
            return Err("Login mode needs a joined network with an address".into());
        }
        *LOGIN.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now() + LOGIN_WINDOW);
        log_line("uplink: login mode started");
    } else {
        *LOGIN.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    wake();
    Ok(())
}

/// POST /api/wifi/uplink/login — `{enabled}`; answers with the status once the
/// supervisor has acted on it.
pub fn login(body: &[u8]) -> (u16, Value) {
    let result = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| v.get("enabled")?.as_bool())
        .ok_or_else(|| "Invalid Wi-Fi uplink request".to_string())
        .and_then(set_login);
    match result.and_then(|_| load()) {
        Ok(s) => {
            // Let the supervisor put the rules in place before answering.
            std::thread::sleep(Duration::from_millis(500));
            (200, json!({"ok": true, "data": response(&s)}))
        }
        Err(e) => (400, json!({"ok": false, "error": e})),
    }
}

/// For the screen: the saved revision and on/off, and what the uplink is doing.
pub struct Summary {
    pub revision: u64,
    pub enabled: bool,
    pub state: State,
    pub reason: Option<String>,
    pub ssid: String,
    pub signal: Option<i32>,
    pub band: &'static str,
    pub ip: Option<String>,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub online_since: Option<u64>,
    pub saved: usize,
}

pub fn summary() -> Option<Summary> {
    let s = load().ok()?;
    let view = SNAPSHOT.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let (rx_bytes, tx_bytes) = traffic(&view);
    Some(Summary {
        revision: s.revision,
        enabled: s.enabled,
        state: view.state,
        reason: view.reason.clone(),
        ssid: view
            .link
            .as_ref()
            .map(|l| l.ssid.clone())
            .unwrap_or_default(),
        signal: view.signal,
        band: view.link.as_ref().map_or("", |l| band(l.frequency)),
        ip: view.lease.as_ref().map(|l| l.ip.to_string()),
        rx_bytes,
        tx_bytes,
        online_since: view.online_since,
        saved: s.networks.len(),
    })
}

/// Turns the uplink on or off for the screen; a newer saved revision answers a conflict.
pub fn set_enabled(expected_revision: u64, enabled: bool) -> Result<(), String> {
    save(Update {
        expected_revision,
        enabled: Some(enabled),
        networks: None,
    })
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(id: u64, ssid: &str, security: Security, password: &str) -> Network {
        Network {
            id,
            ssid: ssid.into(),
            security,
            password: password.into(),
            hidden: false,
        }
    }

    #[test]
    fn wpa_psk_matches_ieee_test_vectors() {
        assert_eq!(
            hex(&sha1(b"abc")),
            "a9993e364706816aba3e25717850c26c9cd0d89d"
        );
        assert_eq!(
            wpa_psk("password", b"IEEE"),
            "f42c6fc52df0ebef9ebb4b90b38a5f902e83fe1b135a70e23aed762e9710a12e"
        );
        assert_eq!(
            wpa_psk("ThisIsAPassword", b"ThisIsASSID"),
            "0dc0d6eb90555ed6419756b9a15ec3e3209b63df707dd508d14581f8982721af"
        );
    }

    #[test]
    fn supplicant_config_never_quotes_user_text() {
        let s = Settings {
            revision: 1,
            enabled: true,
            networks: vec![
                net(3, "Café \"x\" #1", Security::Wpa2, "pa\"ss#word"),
                net(4, "Open", Security::Open, ""),
                net(5, "W3", Security::Wpa3, "sae\"pass#"),
            ],
        };
        let conf = supplicant_config(&s);
        assert!(!conf.contains("pa\"ss") && !conf.contains("sae\"pass") && !conf.contains("Café"));
        assert!(conf.contains(&format!("ssid={}", hex("Café \"x\" #1".as_bytes()))));
        assert!(conf.contains("id_str=\"ou3\"\n\tpriority=3"));
        assert!(conf.contains("key_mgmt=NONE"));
        assert!(conf.contains(&format!("sae_password={}", hex(b"sae\"pass#"))));
        assert!(conf.contains(&format!(
            "psk={}",
            wpa_psk("pa\"ss#word", "Café \"x\" #1".as_bytes())
        )));
    }

    #[test]
    fn edits_keep_saved_passwords_and_validate() {
        let old = Settings {
            revision: 4,
            enabled: false,
            networks: vec![net(1, "Home", Security::Wpa2, "homepassword")],
        };
        let u: Update = serde_json::from_value(json!({"expected_revision": 4, "enabled": true,
            "networks": [{"ssid": "Cafe", "security": "open"},
                         {"id": 1, "ssid": "Home", "security": "wpa2_wpa3", "password": PLACEHOLDER}]}))
        .unwrap();
        let next = edited(&old, u).unwrap();
        assert_eq!(next.revision, 5);
        assert!(next.enabled);
        assert_eq!(next.networks[0].id, 2);
        assert_eq!(next.networks[1].password, "homepassword");
        for body in [
            json!({"expected_revision": 3, "enabled": true}),
            json!({"expected_revision": 4, "networks": [{"ssid": "x", "security": "wpa2", "password": "short"}]}),
            json!({"expected_revision": 4, "networks": [{"ssid": "", "security": "open"}]}),
            json!({"expected_revision": 4, "networks": [{"ssid": "a", "security": "open"}, {"ssid": "a", "security": "open"}]}),
            json!({"expected_revision": 4, "networks": [{"id": 9, "ssid": "a", "security": "open"}]}),
            json!({"expected_revision": 4, "networks": [], "enabled": true}),
        ] {
            let u: Update = serde_json::from_value(body.clone()).unwrap();
            assert!(edited(&old, u).is_err(), "{body}");
        }
        let u: Update = serde_json::from_value(json!({"expected_revision": 3})).unwrap();
        assert!(edited(&old, u)
            .unwrap_err()
            .starts_with("Configuration changed;"));
    }

    #[test]
    fn parses_lease_and_detects_lan_overlap() {
        let l = parse_lease(
            "ip=192.168.50.23\nmask=255.255.255.0\nrouter=192.168.50.1\ndns=192.168.50.1 8.8.8.8\n",
        )
        .unwrap();
        assert_eq!(l.prefix, 24);
        assert_eq!(l.subnet(), "192.168.50.0/24");
        assert_eq!(l.resolver(), Ipv4Addr::new(192, 168, 50, 1));
        assert_eq!(
            parse_lease("ip=10.0.0.5\nmask=8\nrouter=10.0.0.1\ndns=\n")
                .unwrap()
                .resolver(),
            Ipv4Addr::new(223, 5, 5, 5)
        );
        assert!(parse_lease("ip=10.0.0.5\nmask=255.0.255.0\nrouter=10.0.0.1\n").is_none());
        assert!(overlaps(
            (Ipv4Addr::new(192, 168, 0, 0), 24),
            (Ipv4Addr::new(192, 168, 0, 0), 16)
        ));
        assert!(!overlaps(
            (Ipv4Addr::new(192, 168, 0, 0), 24),
            (Ipv4Addr::new(192, 168, 1, 0), 24)
        ));
    }

    #[test]
    fn parses_iw_dev_and_scan_output() {
        let dev = "phy#7\n\tInterface wlan2\n\t\tifindex 25\n\t\ttype AP\n\t\tchannel 149 (5745 MHz), width: 80 MHz\n\tInterface ou-sta0\n\t\ttype managed\n\tInterface wlan0\n\t\ttype AP\n\t\tchannel 6 (2437 MHz), width: 20 MHz\n";
        let r = parse_iw_dev(dev);
        assert_eq!(r.phy.as_deref(), Some("phy7"));
        assert!(r.station);
        assert_eq!(
            r.aps,
            vec![("wlan2".into(), Some(149)), ("wlan0".into(), Some(6))]
        );
        assert_eq!(parse_iw_dev("phy#3\n").phy.as_deref(), Some("phy3"));
        let scan = "BSS 02:00:00:00:00:01(on wlan0)\n\tfreq: 2437.0\n\tsignal: -48.00 dBm\n\tcapability: ESS Privacy (0x0011)\n\tSSID: Home\\xe5\\xae\\xb6\n\tRSN:\t * Version: 1\n\t\t * Authentication suites: PSK SAE\n\
BSS 02:00:00:00:00:02(on wlan0)\n\tfreq: 5180\n\tsignal: -70.00 dBm\n\tcapability: ESS (0x0001)\n\tSSID: Cafe\n\
BSS 02:00:00:00:00:03(on wlan0)\n\tfreq: 5200\n\tsignal: -60.00 dBm\n\tcapability: ESS Privacy\n\tSSID: Corp\n\tRSN:\n\t\t * Authentication suites: IEEE 802.1X\n\
BSS 02:00:00:00:00:04(on wlan0)\n\tfreq: 2412\n\tsignal: -80.00 dBm\n\tSSID: \n\
BSS 02:00:00:00:00:05(on wlan0)\n\tfreq: 2412\n\tsignal: -40.00 dBm\n\tcapability: ESS (0x0001)\n\tSSID: Cafe\n";
        let seen = parse_iw_scan(scan);
        let names: Vec<&str> = seen.iter().map(|s| s.ssid.as_str()).collect();
        assert_eq!(names, ["Cafe", "Home家", "Corp"]);
        assert_eq!(seen[0].bssid, "02:00:00:00:00:05");
        assert_eq!(seen[1].security, Some(Security::Wpa2Wpa3));
        assert_eq!(seen[1].frequency, 2437);
        assert_eq!(seen[2].security, None);
        let results = "bssid / frequency / signal level / flags / ssid\n02:00:00:00:00:0a\t5745\t-55\t[WPA2-PSK-CCMP][ESS]\tUp\\\"stream\n02:00:00:00:00:0b\t2412\t-65\t[RSN-SAE-CCMP][ESS]\tW3\n02:00:00:00:00:0c\t2412\t-75\t[WPA2-EAP-CCMP][ESS]\tEnt\n02:00:00:00:00:0d\t2412\t-79\t[ESS]\tFree\n";
        let seen = parse_scan_results(results);
        assert_eq!(seen[0].ssid, "Up\"stream");
        assert_eq!(seen[0].security, Some(Security::Wpa2));
        assert_eq!(seen[1].security, Some(Security::Wpa3));
        assert_eq!(seen[2].security, None);
        assert_eq!(seen[3].security, Some(Security::Open));
        assert_eq!((channel(5745), band(5745)), (Some(149), "5"));
    }

    #[test]
    fn probe_answers_and_portals_are_told_apart() {
        assert!(alidns_answer(
            r#"{"Status":0,"TC":false,"Answer":[{"data":"49.79.229.97"}]}"#
        ));
        assert!(alidns_answer("{ \"Status\": 0 }"));
        assert!(!alidns_answer("<html><body>Please log in</body></html>"));
        assert!(!alidns_answer(r#"{"Status":2}"#));
        assert!(dnspod_answer("49.79.229.97;49.79.229.98\n"));
        assert!(!dnspod_answer(""));
        assert!(!dnspod_answer("<html>login</html>"));
        let (status, location, body) = parse_http(
            "HTTP/1.1 302 Found\r\nLocation: http://portal.example/login?x=1\r\nContent-Length: 0\r\n\r\n",
        )
        .unwrap();
        assert_eq!(
            (status, location.as_deref(), body.as_str()),
            (302, Some("http://portal.example/login?x=1"), "")
        );
        let (status, _, body) = parse_http(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\n{\"Sta\r\n9\r\ntus\":0}\n\r\n0\r\n\r\n",
        )
        .unwrap();
        assert_eq!(status, 200);
        assert!(alidns_answer(&body), "{body:?}");
        assert!(parse_http("garbage").is_none());
        let base = "http://223.5.5.5/resolve?name=x";
        assert_eq!(
            portal_url(base, Some("/auth/login"), "").as_deref(),
            Some("http://223.5.5.5/auth/login")
        );
        assert_eq!(
            portal_url(
                base,
                None,
                "<meta http-equiv=\"refresh\" content=\"0; URL='http://10.1.0.1/portal?ip=1'\">"
            )
            .as_deref(),
            Some("http://10.1.0.1/portal?ip=1")
        );
        assert_eq!(
            portal_url(
                base,
                None,
                "<script>window.location.href=\"https://wifi.hotel.example/\";</script>"
            )
            .as_deref(),
            Some("https://wifi.hotel.example/")
        );
        assert_eq!(portal_url(base, Some("javascript:alert(1)"), ""), None);
        assert_eq!(portal_url(base, None, "<html>Welcome</html>"), None);
    }

    #[test]
    fn online_needs_two_passes_and_three_failures_to_drop() {
        let (mut p, mut f) = (0, 0);
        assert!(!judge(false, true, &mut p, &mut f));
        assert!(judge(false, true, &mut p, &mut f));
        assert!(judge(true, false, &mut p, &mut f));
        assert!(judge(true, false, &mut p, &mut f));
        assert!(!judge(true, false, &mut p, &mut f));
        assert!(!judge(false, true, &mut p, &mut f));
    }

    #[test]
    fn policy_scripts_stay_on_owned_names() {
        let l = parse_lease("ip=10.9.0.2\nmask=24\nrouter=10.9.0.1\ndns=10.9.0.1\n").unwrap();
        let all = [
            firewall_script(),
            cleanup_script(),
            lease_script(&l),
            online_script(&l),
            offline_script(),
        ]
        .join("\n");
        for line in all.lines().filter(|l| l.contains("tables ")) {
            assert!(line.contains(PREFIX), "{line}");
        }
        for line in all.lines().filter(|l| l.contains(" rule ")) {
            assert!(
                [PRIO_SOURCE, PRIO_MAIN, PRIO_ALL]
                    .iter()
                    .any(|p| line.contains(&format!("priority {p}"))),
                "{line}"
            );
        }
        assert!(
            !all.contains(" -P ")
                && !all.contains("table main\n")
                && !all.contains("route replace default dev")
        );
        assert_eq!(
            u32::from_str_radix(&TUNNEL_MARKS[2..], 16).unwrap(),
            u32::from_str_radix(&crate::clash::EGRESS.mark[2..], 16).unwrap()
                | u32::from_str_radix(&crate::wireguard::EGRESS.mark[2..], 16).unwrap()
        );
    }
}
