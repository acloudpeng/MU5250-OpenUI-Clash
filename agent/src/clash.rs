//! Clash (mihomo) proxy backend: subscriptions, a supervised core with a TUN device,
//! and the shared MAC-based egress policy. Mutually exclusive with WireGuard. Never
//! changes main routes, stock firewall chains, init.d, or UCI; the core listens only
//! on loopback and a Unix socket inside the root-only state directory.
use crate::{
    clash_config::{self, Options},
    clash_ctl,
    clash_sub::{self, Usage},
    egress::Egress,
    storage, wireguard,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    fs,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Mutex,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const DIR: &str = "/data/local/tmp/openui-clash";
const CONFIG: &str = "/data/local/tmp/openui-clash/config.json";
const SUBS: &str = "/data/local/tmp/openui-clash/subscriptions";
const PIDFILE: &str = "/data/local/tmp/openui-clash/mihomo.pid";
/// mihomo home: generated profile, cache.db, providers and bundled geo data.
const HOME: &str = "/data/local/tmp/openui-clash/home";
const PROFILE: &str = "/data/local/tmp/openui-clash/home/config.yaml";
const SOCKET: &str = "/data/local/tmp/openui-clash/home/mihomo.sock";
const MIHOMO: &str = "/data/bin/mihomo";
/// Geo data deployed beside the core; linked into HOME so the core never downloads it.
const GEODATA: &str = "/data/share/mihomo";
const GEO_FILES: &[&str] = &["geoip.metadb", "GeoSite.dat", "ASN.mmdb"];
const IFACE: &str = "ou-clash0";
/// Marked clients' DNS is redirected here; it routes into the TUN and is hijacked.
/// Outside the TUN /30 (whose second address the system stack uses for NAT).
const DNS: &str = "198.19.0.53";
const PROXY: &str = "http://127.0.0.1:7890";
const EGRESS: Egress = Egress {
    label: "Clash",
    prefix: "OUCL",
    mark: "0x20000000",
    table: "10421",
    iface: IFACE,
    dir: DIR,
    owner: "openui-clash-v1",
    masquerade: false,
    accept_input: true,
};
const MAX_SUBSCRIPTIONS: usize = 5;
const LOG_LINES: usize = 300;
const DELAY_URL: &str = "https://www.gstatic.com/generate_204";
static LOCK: Mutex<()> = Mutex::new(());
static ERROR: Mutex<Option<String>> = Mutex::new(None);
static CORE: Mutex<Option<Core>> = Mutex::new(None);
static LOGS: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

struct Core {
    child: Child,
    started: Instant,
    version: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Settings {
    revision: u64,
    enabled: bool,
    mode: String,
    macs: Vec<String>,
    subscriptions: Vec<Subscription>,
    active_subscription: Option<u64>,
    proxy_mode: String,
    log_level: String,
    stack: String,
    dns_mode: String,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            revision: 0,
            enabled: false,
            mode: "all".into(),
            macs: vec![],
            subscriptions: vec![],
            active_subscription: None,
            proxy_mode: "rule".into(),
            log_level: "warning".into(),
            stack: "system".into(),
            dns_mode: "fake-ip".into(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Subscription {
    id: u64,
    name: String,
    url: String,
    user_agent: String,
    /// Automatic refresh period; 0 refreshes only on request.
    interval_hours: u32,
    /// Download through the running core instead of the carrier connection.
    via_proxy: bool,
}
/// Download results kept beside the profile; not part of the revisioned settings.
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
struct Meta {
    updated_at: u64,
    /// Last failed attempt; automatic refresh waits an hour after one.
    attempted_at: u64,
    usage: Option<Usage>,
    proxies: usize,
    groups: usize,
    rules: usize,
    providers: usize,
    error: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum SubscriptionAction {
    Add {
        name: Option<String>,
        url: String,
        user_agent: Option<String>,
        interval_hours: Option<u32>,
        via_proxy: Option<bool>,
    },
    Edit {
        id: u64,
        name: Option<String>,
        url: Option<String>,
        user_agent: Option<String>,
        interval_hours: Option<u32>,
        via_proxy: Option<bool>,
    },
    Activate {
        id: u64,
    },
    Delete {
        id: u64,
    },
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Update {
    expected_revision: u64,
    enabled: Option<bool>,
    /// Turn WireGuard off in the same transaction when enabling Clash.
    takeover: Option<bool>,
    mode: Option<String>,
    macs: Option<Vec<String>>,
    devices: Option<Vec<DeviceChoice>>,
    proxy_mode: Option<String>,
    log_level: Option<String>,
    stack: Option<String>,
    dns_mode: Option<String>,
    subscription: Option<SubscriptionAction>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceChoice {
    mac: String,
    selected: bool,
}

/// A downloaded and parsed profile waiting to be stored for a subscription.
struct Fetched {
    body: String,
    name: Option<String>,
    interval_hours: Option<u32>,
    meta: Meta,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn sub_path(id: u64, ext: &str) -> PathBuf {
    Path::new(SUBS).join(format!("{id}.{ext}"))
}
fn name_ok(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 48 || name.chars().any(char::is_control) {
        return Err(
            "Subscription name must contain 1–48 characters without control characters".into(),
        );
    }
    Ok(name.into())
}
fn one_of(value: &str, allowed: &[&str], what: &str) -> Result<(), String> {
    if allowed.contains(&value) {
        Ok(())
    } else {
        Err(format!("Invalid {what}"))
    }
}

fn subscription_edit(
    s: &mut Settings,
    action: SubscriptionAction,
    fetched: Option<&Fetched>,
) -> Result<Option<u64>, String> {
    let unique = |s: &Settings, id: u64, name: &str| {
        if s.subscriptions
            .iter()
            .any(|p| p.id != id && p.name.to_lowercase() == name.to_lowercase())
        {
            Err("A subscription with this name already exists".to_string())
        } else {
            Ok(())
        }
    };
    match action {
        SubscriptionAction::Add {
            name,
            url,
            user_agent,
            interval_hours,
            via_proxy,
        } => {
            if s.subscriptions.len() >= MAX_SUBSCRIPTIONS {
                return Err("A maximum of 5 subscriptions can be saved; delete one first".into());
            }
            let f = fetched.ok_or("Subscription was not downloaded")?;
            let id = s
                .subscriptions
                .iter()
                .map(|p| p.id)
                .max()
                .unwrap_or(0)
                .checked_add(1)
                .ok_or("Subscription IDs exhausted")?;
            let name = match name.filter(|n| !n.trim().is_empty()) {
                Some(n) => name_ok(&n)?,
                None => {
                    let base = f
                        .name
                        .clone()
                        .unwrap_or_else(|| format!("Subscription {id}"));
                    // A provider-supplied name must not collide with an existing one.
                    if unique(s, id, &base).is_ok() {
                        base
                    } else {
                        format!("{base} ({id})")
                    }
                }
            };
            unique(s, id, &name)?;
            s.subscriptions.push(Subscription {
                id,
                name,
                url,
                user_agent: user_agent.unwrap_or_else(|| clash_sub::DEFAULT_USER_AGENT.into()),
                interval_hours: interval_hours.or(f.interval_hours).unwrap_or(24),
                via_proxy: via_proxy.unwrap_or(false),
            });
            if s.active_subscription.is_none() {
                s.active_subscription = Some(id);
            }
            Ok(Some(id))
        }
        SubscriptionAction::Edit {
            id,
            name,
            url,
            user_agent,
            interval_hours,
            via_proxy,
        } => {
            let name = name.map(|n| name_ok(&n)).transpose()?;
            if let Some(n) = &name {
                unique(s, id, n)?;
            }
            let p = s
                .subscriptions
                .iter_mut()
                .find(|p| p.id == id)
                .ok_or("Subscription not found")?;
            if let Some(n) = name {
                p.name = n;
            }
            if let Some(v) = user_agent {
                p.user_agent = v;
            }
            if let Some(v) = interval_hours {
                p.interval_hours = v;
            }
            if let Some(v) = via_proxy {
                p.via_proxy = v;
            }
            match url {
                Some(u) if u != p.url => {
                    fetched.ok_or("Subscription was not downloaded")?;
                    p.url = u;
                    Ok(Some(id))
                }
                _ => Ok(None),
            }
        }
        SubscriptionAction::Activate { id } => {
            if !s.subscriptions.iter().any(|p| p.id == id) {
                return Err("Subscription not found".into());
            }
            s.active_subscription = Some(id);
            Ok(None)
        }
        SubscriptionAction::Delete { id } => {
            if !s.subscriptions.iter().any(|p| p.id == id) {
                return Err("Subscription not found".into());
            }
            if s.active_subscription == Some(id) {
                if s.enabled {
                    return Err(
                        "Switch subscriptions or turn Clash off before deleting the active one"
                            .into(),
                    );
                }
                s.active_subscription = None;
            }
            s.subscriptions.retain(|p| p.id != id);
            Ok(None)
        }
    }
}

/// Returns the next settings and the subscription whose profile `fetched` belongs to.
fn edited(
    old: &Settings,
    u: Update,
    fetched: Option<&Fetched>,
) -> Result<(Settings, Option<u64>), String> {
    if u.expected_revision != old.revision {
        return Err("Configuration changed; refresh before saving".into());
    }
    if u.devices.is_some() && (u.mode.is_some() || u.macs.is_some()) {
        return Err("Cannot combine device changes with a routing mode change".into());
    }
    let mut next = old.clone();
    next.revision = old
        .revision
        .checked_add(1)
        .ok_or("Configuration revision exhausted")?;
    if let Some(v) = u.enabled {
        next.enabled = v;
    }
    if let Some(v) = u.mode {
        next.mode = v;
    }
    if let Some(v) = u.macs {
        next.macs = v.into_iter().map(|m| m.to_lowercase()).collect();
    }
    if let Some(choices) = u.devices {
        if choices.len() > 128 {
            return Err("Too many device changes".into());
        }
        for c in choices {
            let m = c.mac.to_lowercase();
            if !wireguard::mac(&m) {
                return Err("Invalid device MAC".into());
            }
            next.macs.retain(|v| v != &m);
            if c.selected == (next.mode == "selected") {
                next.macs.push(m);
            }
        }
    }
    for (field, value) in [
        (&mut next.proxy_mode, u.proxy_mode),
        (&mut next.log_level, u.log_level),
        (&mut next.stack, u.stack),
        (&mut next.dns_mode, u.dns_mode),
    ] {
        if let Some(v) = value {
            *field = v;
        }
    }
    let written = match u.subscription {
        Some(action) => subscription_edit(&mut next, action, fetched)?,
        None => None,
    };
    next.macs.sort();
    next.macs.dedup();
    validate(&next)?;
    Ok((next, written))
}

fn validate(s: &Settings) -> Result<(), String> {
    one_of(&s.mode, &["all", "selected"], "routing mode")?;
    one_of(&s.proxy_mode, &["rule", "global", "direct"], "proxy mode")?;
    one_of(
        &s.log_level,
        &["silent", "error", "warning", "info", "debug"],
        "log level",
    )?;
    one_of(&s.stack, &["system", "gvisor", "mixed"], "TUN stack")?;
    one_of(&s.dns_mode, &["fake-ip", "redir-host"], "DNS mode")?;
    if s.macs.len() > 128 || s.macs.iter().any(|m| !wireguard::mac(m)) {
        return Err("Invalid device MAC selection (maximum 128)".into());
    }
    if s.subscriptions.len() > MAX_SUBSCRIPTIONS {
        return Err("Too many saved subscriptions".into());
    }
    let mut ids = std::collections::HashSet::new();
    let mut names = std::collections::HashSet::new();
    for p in &s.subscriptions {
        if p.id == 0
            || !ids.insert(p.id)
            || name_ok(&p.name)? != p.name
            || !names.insert(p.name.to_lowercase())
        {
            return Err("Invalid or duplicate saved subscription".into());
        }
        if !clash_sub::valid_url(&p.url) {
            return Err("Subscription URL must be an http(s) address without spaces".into());
        }
        if !clash_sub::valid_user_agent(&p.user_agent) {
            return Err("Invalid subscription user agent".into());
        }
        if p.interval_hours > 24 * 30 {
            return Err("Update interval must be 0–720 hours".into());
        }
    }
    if s.active_subscription
        .is_some_and(|id| !s.subscriptions.iter().any(|p| p.id == id))
    {
        return Err("Active subscription not found".into());
    }
    if s.enabled && s.active_subscription.is_none() {
        return Err("Add a subscription first".into());
    }
    Ok(())
}
fn load() -> Result<Settings, String> {
    match fs::read(CONFIG) {
        Ok(b) => {
            let s: Settings =
                serde_json::from_slice(&b).map_err(|_| "Saved Clash configuration is invalid")?;
            validate(&s)?;
            Ok(s)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(_) => Err("Cannot read saved Clash configuration".into()),
    }
}
fn persist(s: &Settings) -> Result<(), String> {
    storage::atomic_write(Path::new(CONFIG), &serde_json::to_vec(s).unwrap())
        .map_err(|_| "Cannot persist Clash configuration".into())
}
fn meta(id: u64) -> Meta {
    fs::read(sub_path(id, "json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}
fn save_meta(id: u64, m: &Meta) {
    let _ = storage::atomic_write(&sub_path(id, "json"), &serde_json::to_vec(m).unwrap());
}
fn ensure_dirs() -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    for d in [DIR, SUBS, HOME] {
        fs::create_dir_all(d).map_err(|_| "Cannot create Clash state directory")?;
    }
    link_geodata(Path::new(GEODATA), Path::new(HOME));
    // The controller socket has no authentication; this directory is its only guard.
    fs::set_permissions(DIR, fs::Permissions::from_mode(0o700))
        .map_err(|_| "Cannot protect Clash state directory".into())
}
/// Points the core's geo files at the deployed copies, replacing any it downloaded.
/// Files that were not deployed are left alone.
fn link_geodata(share: &Path, home: &Path) {
    for name in GEO_FILES {
        let source = share.join(name);
        let target = home.join(name);
        if !source.is_file() || fs::read_link(&target).is_ok_and(|p| p == source) {
            continue;
        }
        let staged = home.join(format!(".{name}.link"));
        let _ = fs::remove_file(&staged);
        if std::os::unix::fs::symlink(&source, &staged).is_err()
            || fs::rename(&staged, &target).is_err()
        {
            let _ = fs::remove_file(&staged);
        }
    }
}
fn geodata_installed() -> Vec<&'static str> {
    GEO_FILES
        .iter()
        .copied()
        .filter(|n| Path::new(GEODATA).join(n).is_file())
        .collect()
}

// ---------- Core process ----------

fn log_line(line: &str) {
    let mut logs = LOGS.lock().unwrap();
    if logs.len() >= LOG_LINES {
        logs.pop_front();
    }
    logs.push_back(line.trim_end().chars().take(400).collect());
}
fn capture(reader: impl Read + Send + 'static) {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut line = Vec::new();
        // Lossy per line: invalid UTF-8 must never stop draining the pipe.
        while reader.read_until(b'\n', &mut line).is_ok_and(|n| n > 0) {
            log_line(&String::from_utf8_lossy(&line));
            line.clear();
        }
    });
}
fn last_log_message() -> String {
    let logs = LOGS.lock().unwrap();
    let line = logs
        .iter()
        .rev()
        .find(|l| l.contains("level=error") || l.contains("level=fatal") || l.contains("panic"))
        .or(logs.back())
        .cloned()
        .unwrap_or_default();
    let msg = line
        .split_once("msg=\"")
        .map(|(_, m)| m.trim_end_matches('"'))
        .unwrap_or(&line);
    msg.chars().take(300).collect()
}
fn signal_and_wait(pid: i32, alive: impl Fn() -> bool) {
    unsafe { libc::kill(pid, libc::SIGTERM) };
    for _ in 0..50 {
        if !alive() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    unsafe { libc::kill(pid, libc::SIGKILL) };
}
/// A core left by a previous agent keeps the TUN name; it must go before a new start.
fn kill_stale() {
    let Some(pid) = fs::read_to_string(PIDFILE)
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok())
        .filter(|p| *p > 1)
    else {
        return;
    };
    let exe = format!("/proc/{pid}/exe");
    let ours = || fs::read_link(&exe).is_ok_and(|p| p.to_string_lossy().starts_with(MIHOMO));
    if ours() {
        signal_and_wait(pid, ours);
    }
    let _ = fs::remove_file(PIDFILE);
}
fn stop_core() {
    let core = CORE.lock().unwrap().take();
    if let Some(mut core) = core {
        let pid = core.child.id() as i32;
        let child = Mutex::new(&mut core.child);
        signal_and_wait(pid, || matches!(child.lock().unwrap().try_wait(), Ok(None)));
        let _ = core.child.wait();
    }
    kill_stale();
}
/// True while the supervised core process is alive.
fn core_running() -> bool {
    let mut core = CORE.lock().unwrap();
    match core.as_mut().map(|c| c.child.try_wait()) {
        Some(Ok(None)) => true,
        Some(_) => {
            if let Some(mut c) = core.take() {
                let _ = c.child.wait();
            }
            false
        }
        None => false,
    }
}
fn ctl(method: &str, path: &str, body: Option<&Value>, secs: u64) -> Result<Value, String> {
    clash_ctl::call(
        Path::new(SOCKET),
        method,
        path,
        body,
        Duration::from_secs(secs),
    )
}
fn start_core() -> Result<(), String> {
    stop_core();
    let _ = fs::remove_file(SOCKET);
    let mut child = Command::new(MIHOMO)
        .args(["-d", HOME, "-f", PROFILE])
        .env("GOMEMLIMIT", "128MiB")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| "Cannot start the Clash core")?;
    let _ = storage::atomic_write(Path::new(PIDFILE), child.id().to_string().as_bytes());
    capture(child.stdout.take().unwrap());
    capture(child.stderr.take().unwrap());
    *CORE.lock().unwrap() = Some(Core {
        child,
        started: Instant::now(),
        version: String::new(),
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if !core_running() {
            return Err(format!("Clash core exited: {}", last_log_message()));
        }
        if Path::new(&format!("/sys/class/net/{IFACE}")).exists() {
            if let Ok(v) = ctl("GET", "/version", None, 2) {
                if let Some(c) = CORE.lock().unwrap().as_mut() {
                    c.version = v["version"]
                        .as_str()
                        .unwrap_or("")
                        .chars()
                        .take(64)
                        .collect();
                }
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    stop_core();
    Err("Clash core did not become ready within 30 s".into())
}

// ---------- Profile and policy ----------

fn options(s: &Settings) -> Options<'_> {
    Options {
        proxy_mode: &s.proxy_mode,
        log_level: &s.log_level,
        stack: &s.stack,
        dns_mode: &s.dns_mode,
        iface: IFACE,
        controller: SOCKET,
    }
}
/// `mihomo -t` against a candidate profile; the error is the core's own message.
fn test_profile(path: &Path) -> Result<(), String> {
    let mut cmd = Command::new(MIHOMO);
    cmd.args(["-d", HOME, "-t", "-f"]).arg(path);
    let out = process_runner::output(&mut cmd, None, Duration::from_secs(90), 256 * 1024)
        .map_err(|_| "Clash profile check timed out")?;
    if out.status.success() {
        return Ok(());
    }
    let text =
        String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
    let line = text
        .lines()
        .rev()
        .find(|l| {
            l.contains("level=error") || l.contains("level=fatal") || l.contains("test failed")
        })
        .or(text.lines().last())
        .unwrap_or("");
    let msg = line
        .split_once("msg=\"")
        .map(|(_, m)| m.trim_end_matches('"'))
        .unwrap_or(line);
    Err(format!(
        "Clash profile rejected: {}",
        msg.chars().take(300).collect::<String>()
    ))
}
/// Composes the running profile from the active subscription and checks it.
fn write_profile(s: &Settings) -> Result<(), String> {
    let id = s.active_subscription.ok_or("Add a subscription first")?;
    let text = fs::read_to_string(sub_path(id, "yaml"))
        .map_err(|_| "Active subscription has not been downloaded; refresh it")?;
    let map = clash_config::parse(&text)?;
    let candidate = Path::new(HOME).join(".config.candidate.yaml");
    storage::atomic_write(
        &candidate,
        clash_config::compose(&map, &options(s)).as_bytes(),
    )
    .map_err(|_| "Cannot write Clash profile")?;
    if let Err(e) = test_profile(&candidate) {
        let _ = fs::remove_file(&candidate);
        return Err(e);
    }
    fs::rename(&candidate, PROFILE).map_err(|_| "Cannot install Clash profile".into())
}
fn prerequisites() -> Result<(), String> {
    if !Path::new(MIHOMO).exists() {
        return Err("Clash core missing: install /data/bin/mihomo".into());
    }
    if !Path::new("/dev/net/tun").exists() {
        return Err("Kernel TUN support is unavailable".into());
    }
    Ok(())
}
/// Brings the core up on the settings' profile without touching the LAN policy.
fn prepare(s: &Settings) -> Result<(), String> {
    validate(s)?;
    prerequisites()?;
    ensure_dirs()?;
    write_profile(s)?;
    EGRESS.claim()?;
    if core_running() {
        ctl(
            "PUT",
            "/configs?force=true",
            Some(&json!({"path":"","payload":""})),
            60,
        )?;
    } else {
        start_core()?;
    }
    // A stack change recreates the device; wait for it before routing to it.
    for _ in 0..50 {
        if Path::new(&format!("/sys/class/net/{IFACE}")).exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // Loose reverse-path checking only on our device (replies come from the internet).
    fs::write(format!("/proc/sys/net/ipv4/conf/{IFACE}/rp_filter"), "2\n")
        .map_err(|_| "Cannot set TUN reverse-path check".into())
}
fn policy(s: &Settings) -> String {
    EGRESS.cleanup_policy() + &EGRESS.policy(&s.mode, &s.macs, DNS)
}
fn apply(s: &Settings) -> Result<(), String> {
    if !s.enabled {
        stop_core();
        if EGRESS.owned() {
            EGRESS.script(&EGRESS.cleanup())?;
        }
        return Ok(());
    }
    prepare(s)?;
    EGRESS.script(&policy(s))
}
fn active_changed(old: &Settings, next: &Settings) -> bool {
    old.enabled != next.enabled
        || old.mode != next.mode
        || old.macs != next.macs
        || old.active_subscription != next.active_subscription
        || old.stack != next.stack
        || old.dns_mode != next.dns_mode
}

// ---------- WireGuard exclusivity ----------

pub(crate) struct Suspended(Settings);
/// Whether Clash is saved as on. Read without the lock (atomic file replacement).
pub(crate) fn stored_enabled() -> bool {
    load().is_ok_and(|s| s.enabled)
}
/// Turns Clash off for a switch to WireGuard. The caller holds the WireGuard lock and
/// its transition guard; `resume` restores Clash if that switch fails.
pub(crate) fn suspend() -> Result<Option<Suspended>, String> {
    let _guard = LOCK.lock().unwrap();
    let old = load()?;
    if !old.enabled {
        return Ok(None);
    }
    let mut next = old.clone();
    next.enabled = false;
    next.revision = old.revision.saturating_add(1);
    apply(&next)?;
    if let Err(e) = persist(&next) {
        let _ = apply(&old);
        return Err(e);
    }
    Ok(Some(Suspended(old)))
}
pub(crate) fn resume(saved: Suspended) -> Result<(), String> {
    let _guard = LOCK.lock().unwrap();
    let mut s = saved.0;
    s.revision = load()?.revision.saturating_add(1);
    apply(&s)?;
    persist(&s)
}

// ---------- API ----------

fn subscription_status(s: &Settings) -> Value {
    json!(s
        .subscriptions
        .iter()
        .map(|p| {
            let m = meta(p.id);
            json!({"id":p.id,"name":p.name,"url":clash_sub::mask(&p.url),"user_agent":p.user_agent,
                "interval_hours":p.interval_hours,"via_proxy":p.via_proxy,"updated_at":m.updated_at,
                "usage":m.usage,"proxies":m.proxies,"groups":m.groups,"rules":m.rules,"providers":m.providers,
                "error":m.error,"downloaded":sub_path(p.id, "yaml").exists()})
        })
        .collect::<Vec<_>>())
}
fn rss_kib(pid: u32) -> u64 {
    fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmRSS:"))?
                .split_whitespace()
                .nth(1)?
                .parse()
                .ok()
        })
        .unwrap_or(0)
}
fn response(s: &Settings) -> Value {
    let running = core_running();
    let (pid, uptime, version) = CORE
        .lock()
        .unwrap()
        .as_ref()
        .map(|c| {
            (
                c.child.id(),
                c.started.elapsed().as_secs(),
                c.version.clone(),
            )
        })
        .unwrap_or_default();
    let interface = Path::new(&format!("/sys/class/net/{IFACE}")).exists();
    let healthy = !s.enabled || (running && EGRESS.healthy());
    let error = ERROR.lock().unwrap().clone().or_else(|| {
        (!healthy).then(|| "Clash policy is incomplete; automatic recovery is pending".into())
    });
    json!({"revision":s.revision,"enabled":s.enabled,"mode":s.mode,"macs":s.macs,
        "proxy_mode":s.proxy_mode,"log_level":s.log_level,"stack":s.stack,"dns_mode":s.dns_mode,
        "subscriptions":subscription_status(s),"active_subscription":s.active_subscription,
        "max_subscriptions":MAX_SUBSCRIPTIONS,"core_installed":Path::new(MIHOMO).exists(),"geodata":geodata_installed(),
        "tun_supported":Path::new("/dev/net/tun").exists(),"running":running,"pid":running.then_some(pid),
        "uptime":uptime,"version":version,"memory_kib":if running { rss_kib(pid) } else { 0 },
        "interface_up":interface,"connected":s.enabled && healthy && interface,"error":error,
        "wireguard_enabled":wireguard::stored_enabled(),"ipv6_policy":"blocked_for_proxy_devices"})
}
fn reply(result: Result<Value, String>) -> (u16, Value) {
    match result {
        Ok(v) => (200, json!({"ok":true,"data":v})),
        Err(e) => (
            if e.starts_with("Configuration changed;") {
                409
            } else {
                400
            },
            json!({"ok":false,"error":e}),
        ),
    }
}
pub fn get() -> (u16, Value) {
    match load() {
        Ok(s) => (200, json!({"ok":true,"data":response(&s)})),
        Err(e) => (500, json!({"ok":false,"error":e})),
    }
}

fn download(url: &str, user_agent: &str, via_proxy: bool) -> Result<Fetched, String> {
    ensure_dirs()?;
    let proxy = (via_proxy && core_running()).then_some(PROXY);
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let scratch = Path::new(DIR).join(format!(
        ".headers-{}",
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let d = clash_sub::fetch(url, user_agent, proxy, &scratch)?;
    let summary = clash_config::summary(&clash_config::parse(&d.body)?);
    Ok(Fetched {
        body: d.body,
        name: d.name,
        interval_hours: d.interval_hours,
        meta: Meta {
            updated_at: now(),
            attempted_at: 0,
            usage: d.usage,
            proxies: summary.proxies,
            groups: summary.groups,
            rules: summary.rules,
            providers: summary.providers,
            error: None,
        },
    })
}
fn store(id: u64, f: &Fetched) -> Result<(), String> {
    storage::atomic_write(&sub_path(id, "yaml"), f.body.as_bytes())
        .map_err(|_| "Cannot save subscription profile")?;
    save_meta(id, &f.meta);
    Ok(())
}

/// Hold → [WireGuard off] → apply → persist → release; rolls back on any failure and
/// keeps the guard when the previous state cannot be restored.
fn transact(old: &Settings, next: &Settings, takeover: bool) -> Result<(), String> {
    EGRESS.script(&EGRESS.hold())?;
    let mut suspended = None;
    let result = (|| {
        if takeover {
            suspended = wireguard::suspend()?;
        }
        apply(next)?;
        persist(next)
    })();
    if let Err(error) = result {
        let restored = apply(old).and_then(|_| match suspended.take() {
            Some(s) => wireguard::resume(s),
            None => Ok(()),
        });
        if restored.is_err() {
            return Err(format!("{error}; previous policy could not be restored"));
        }
        EGRESS.script(&EGRESS.release()).map_err(|e| {
            format!(
                "{error}; previous policy restored but transition guard could not be released: {e}"
            )
        })?;
        return Err(error);
    }
    EGRESS.script(&EGRESS.release())
}

pub fn update(body: &[u8]) -> (u16, Value) {
    reply((|| -> Result<Value, String> {
        let u: Update = serde_json::from_slice(body).map_err(|_| "Invalid Clash request")?;
        // Downloads can take 30 s; do them before taking any lock.
        let fetched = match &u.subscription {
            Some(SubscriptionAction::Add {
                url,
                user_agent,
                via_proxy,
                ..
            }) => {
                let ua = user_agent
                    .as_deref()
                    .unwrap_or(clash_sub::DEFAULT_USER_AGENT);
                Some(download(url, ua, via_proxy.unwrap_or(false))?)
            }
            Some(SubscriptionAction::Edit {
                id,
                url: Some(url),
                user_agent,
                via_proxy,
                ..
            }) => {
                let current = load()?;
                let p = current
                    .subscriptions
                    .iter()
                    .find(|p| p.id == *id)
                    .ok_or("Subscription not found")?;
                if &p.url == url {
                    None
                } else {
                    let ua = user_agent.as_deref().unwrap_or(&p.user_agent);
                    Some(download(url, ua, via_proxy.unwrap_or(p.via_proxy))?)
                }
            }
            _ => None,
        };
        let takeover = u.takeover == Some(true);
        // Lock order: WireGuard before Clash, everywhere both are needed.
        let _wg = wireguard::lock();
        let _guard = LOCK.lock().unwrap();
        let old = load()?;
        let (next, written) = edited(&old, u, fetched.as_ref())?;
        let wg_on = next.enabled && wireguard::stored_enabled();
        if wg_on && !takeover {
            return Err("WireGuard is on; turn it off or confirm switching to Clash".into());
        }
        ensure_dirs()?;
        let previous = written.and_then(|id| fs::read(sub_path(id, "yaml")).ok());
        if let (Some(id), Some(f)) = (written, &fetched) {
            store(id, f)?;
        }
        let profile_changed = written.is_some() && written == next.active_subscription;
        let result = if !next.enabled && !old.enabled {
            persist(&next)
        } else if active_changed(&old, &next) || profile_changed {
            transact(&old, &next, wg_on)
        } else if old.proxy_mode != next.proxy_mode || old.log_level != next.log_level {
            // Runtime-only: patch the live core, then keep the profile consistent for restarts.
            ctl(
                "PATCH",
                "/configs",
                Some(&json!({"mode":next.proxy_mode,"log-level":next.log_level})),
                10,
            )
            .and_then(|_| write_profile(&next))
            .and_then(|_| persist(&next))
        } else {
            persist(&next)
        };
        if let Err(e) = result {
            if let Some(id) = written {
                match &previous {
                    Some(bytes) => {
                        let _ = storage::atomic_write(&sub_path(id, "yaml"), bytes);
                    }
                    None => {
                        let _ = fs::remove_file(sub_path(id, "yaml"));
                        let _ = fs::remove_file(sub_path(id, "json"));
                    }
                }
            }
            *ERROR.lock().unwrap() = Some(e.clone());
            return Err(e);
        }
        for p in &old.subscriptions {
            if !next.subscriptions.iter().any(|n| n.id == p.id) {
                let _ = fs::remove_file(sub_path(p.id, "yaml"));
                let _ = fs::remove_file(sub_path(p.id, "json"));
            }
        }
        *ERROR.lock().unwrap() = None;
        Ok(response(&next))
    })())
}

/// Downloads a subscription again; reloads the core when it is the active profile.
fn refresh_subscription(id: u64) -> Result<(), String> {
    let s = load()?;
    let p = s
        .subscriptions
        .iter()
        .find(|p| p.id == id)
        .ok_or("Subscription not found")?
        .clone();
    let fetched = match download(&p.url, &p.user_agent, p.via_proxy) {
        Ok(f) => f,
        Err(e) => {
            let mut m = meta(id);
            m.error = Some(e.clone());
            m.attempted_at = now();
            save_meta(id, &m);
            return Err(e);
        }
    };
    let _guard = LOCK.lock().unwrap();
    let s = load()?;
    if !s.subscriptions.iter().any(|q| q.id == id && q.url == p.url) {
        return Err("Subscription changed during the download; try again".into());
    }
    let previous = fs::read(sub_path(id, "yaml")).ok();
    store(id, &fetched)?;
    if s.enabled && s.active_subscription == Some(id) && core_running() {
        let reload = write_profile(&s).and_then(|_| {
            ctl(
                "PUT",
                "/configs?force=true",
                Some(&json!({"path":"","payload":""})),
                60,
            )
        });
        if let Err(e) = reload {
            if let Some(bytes) = previous {
                let _ = storage::atomic_write(&sub_path(id, "yaml"), &bytes);
            }
            let mut m = meta(id);
            m.error = Some(e.clone());
            m.attempted_at = now();
            save_meta(id, &m);
            return Err(e);
        }
    }
    Ok(())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IdRequest {
    id: u64,
}
pub fn refresh(body: &[u8]) -> (u16, Value) {
    reply((|| {
        let r: IdRequest = serde_json::from_slice(body).map_err(|_| "Invalid request")?;
        refresh_subscription(r.id)?;
        Ok(response(&load()?))
    })())
}

fn running_or_err() -> Result<(), String> {
    if core_running() {
        Ok(())
    } else {
        Err("Clash is not running".into())
    }
}
/// Groups in profile order (the GLOBAL group lists them first), with each member's
/// type and latest delay.
fn proxies_view(raw: &Value) -> Value {
    let Some(all) = raw["proxies"].as_object() else {
        return json!({"groups":[],"nodes":{}});
    };
    let order: Vec<&str> = all
        .get("GLOBAL")
        .and_then(|g| g["all"].as_array())
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut groups: Vec<(&String, &Value)> =
        all.iter().filter(|(_, v)| v["all"].is_array()).collect();
    groups.sort_by_key(|(name, _)| order.iter().position(|o| o == name).unwrap_or(usize::MAX));
    let delay = |v: &Value| {
        v["history"]
            .as_array()
            .and_then(|h| h.last())
            .and_then(|h| h["delay"].as_u64())
    };
    let nodes: serde_json::Map<String, Value> = all
        .iter()
        .map(|(name, v)| {
            (
                name.clone(),
                json!({"type":v["type"],"delay":delay(v),"udp":v["udp"],"alive":v["alive"]}),
            )
        })
        .collect();
    json!({
        "groups": groups.iter().map(|(name, v)| json!({"name":name,"type":v["type"],"now":v["now"],
            "all":v["all"],"hidden":v["hidden"].as_bool().unwrap_or(false),
            "selectable":v["type"]=="Selector"})).collect::<Vec<_>>(),
        "nodes": nodes,
    })
}
pub fn proxies() -> (u16, Value) {
    reply(running_or_err().and_then(|_| Ok(proxies_view(&ctl("GET", "/proxies", None, 10)?))))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Select {
    group: String,
    name: String,
}
pub fn select(body: &[u8]) -> (u16, Value) {
    reply((|| {
        let r: Select = serde_json::from_slice(body).map_err(|_| "Invalid request")?;
        running_or_err()?;
        ctl(
            "PUT",
            &format!("/proxies/{}", clash_ctl::segment(&r.group)),
            Some(&json!({"name":r.name})),
            10,
        )?;
        Ok(proxies_view(&ctl("GET", "/proxies", None, 10)?))
    })())
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DelayRequest {
    name: String,
    /// Test every member of a group instead of one node.
    group: Option<bool>,
    timeout_ms: Option<u32>,
}
pub fn delay(body: &[u8]) -> (u16, Value) {
    reply((|| {
        let r: DelayRequest = serde_json::from_slice(body).map_err(|_| "Invalid request")?;
        running_or_err()?;
        let timeout = r.timeout_ms.unwrap_or(5000).clamp(1000, 10000);
        let kind = if r.group == Some(true) {
            "group"
        } else {
            "proxies"
        };
        let url: String = DELAY_URL
            .bytes()
            .map(|b| match b {
                b':' => "%3A".into(),
                b'/' => "%2F".into(),
                _ => (b as char).to_string(),
            })
            .collect();
        ctl(
            "GET",
            &format!(
                "/{kind}/{}/delay?url={url}&timeout={timeout}",
                clash_ctl::segment(&r.name)
            ),
            None,
            u64::from(timeout / 1000) + 10,
        )
    })())
}
pub fn connections() -> (u16, Value) {
    reply(running_or_err().and_then(|_| {
        let raw = ctl("GET", "/connections", None, 10)?;
        let list: Vec<Value> = raw["connections"]
            .as_array()
            .map(|a| {
                a.iter()
                    .take(500)
                    .map(|c| {
                        let m = &c["metadata"];
                        json!({"id":c["id"],"network":m["network"],"source":m["sourceIP"],
                        "host":m["host"],"destination":m["destinationIP"],"port":m["destinationPort"],
                        "sniff_host":m["sniffHost"],"rule":c["rule"],"rule_payload":c["rulePayload"],
                        "chains":c["chains"],"upload":c["upload"],"download":c["download"],"start":c["start"]})
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(json!({"upload_total":raw["uploadTotal"],"download_total":raw["downloadTotal"],
            "memory":raw["memory"],"connections":list}))
    }))
}
pub fn close_connections() -> (u16, Value) {
    reply(running_or_err().and_then(|_| {
        ctl("DELETE", "/connections", None, 10)?;
        Ok(json!({}))
    }))
}
pub fn logs() -> (u16, Value) {
    let lines: Vec<String> = LOGS.lock().unwrap().iter().cloned().collect();
    (200, json!({"ok":true,"data":{"lines":lines}}))
}

// ---------- Supervision ----------

/// Keeps an enabled core and its policy alive, retries with backoff, and refreshes
/// subscriptions on their intervals. Never takes the WireGuard lock.
pub fn start() {
    std::thread::spawn(|| {
        let mut tick: u64 = 0;
        let mut retry_at = Instant::now();
        let mut backoff = Duration::from_secs(5);
        loop {
            let first = tick == 0;
            {
                let _guard = LOCK.lock().unwrap();
                if let Ok(s) = load() {
                    if s.enabled && wireguard::stored_enabled() {
                        stop_core();
                        *ERROR.lock().unwrap() = Some(
                            "WireGuard is also on; Clash stays off until one is turned off".into(),
                        );
                    } else if s.enabled
                        && Instant::now() >= retry_at
                        && (first
                            || !core_running()
                            || (tick.is_multiple_of(6) && !EGRESS.healthy()))
                    {
                        if !first && !core_running() {
                            log_line(&format!(
                                "openui: core stopped ({}); restarting",
                                last_log_message()
                            ));
                        }
                        let result = prepare(&s)
                            .and_then(|_| EGRESS.script(&EGRESS.hold()))
                            .and_then(|_| EGRESS.script(&policy(&s)))
                            .and_then(|_| EGRESS.script(&EGRESS.release()));
                        match result {
                            Ok(()) => {
                                *ERROR.lock().unwrap() = None;
                                backoff = Duration::from_secs(5);
                            }
                            Err(e) => {
                                *ERROR.lock().unwrap() = Some(e);
                                retry_at = Instant::now() + backoff;
                                backoff = (backoff * 2).min(Duration::from_secs(300));
                            }
                        }
                    } else if first && !s.enabled && EGRESS.owned() {
                        if let Err(e) = apply(&s).and_then(|_| EGRESS.script(&EGRESS.release())) {
                            *ERROR.lock().unwrap() = Some(e);
                        }
                    }
                }
            }
            // Every 5 minutes, refresh subscriptions that are due (downloads run unlocked).
            if tick % 60 == 30 {
                if let Ok(s) = load() {
                    for p in s.subscriptions.iter().filter(|p| p.interval_hours > 0) {
                        let m = meta(p.id);
                        let due = m.updated_at + u64::from(p.interval_hours) * 3600;
                        // After a failure, wait an hour instead of retrying every pass.
                        let retry = m.error.is_some() && now() < m.attempted_at + 3600;
                        if now() >= due && !retry {
                            if let Err(e) = refresh_subscription(p.id) {
                                log_line(&format!(
                                    "openui: subscription {} refresh failed: {e}",
                                    p.id
                                ));
                            }
                        }
                    }
                }
            }
            tick = tick.wrapping_add(1);
            std::thread::sleep(Duration::from_secs(5));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fetched() -> Fetched {
        Fetched {
            body: "proxies: [{name: A, type: socks5, server: 192.0.2.1, port: 1080}]".into(),
            name: Some("Provider".into()),
            interval_hours: Some(12),
            meta: Meta::default(),
        }
    }
    fn change(
        s: &Settings,
        v: Value,
        f: Option<&Fetched>,
    ) -> Result<(Settings, Option<u64>), String> {
        let mut v = v;
        v["expected_revision"] = json!(s.revision);
        edited(s, serde_json::from_value(v).unwrap(), f)
    }
    fn with_subscription() -> Settings {
        change(
            &Settings::default(),
            json!({"subscription":{"action":"add","url":"https://sub.example.com/api?token=SECRET"}}),
            Some(&fetched()),
        )
        .unwrap()
        .0
    }
    #[test]
    fn add_uses_provider_name_interval_and_becomes_active() {
        let s = with_subscription();
        assert_eq!(s.revision, 1);
        assert_eq!(s.active_subscription, Some(1));
        let p = &s.subscriptions[0];
        assert_eq!(
            (p.name.as_str(), p.interval_hours, p.user_agent.as_str()),
            ("Provider", 12, "clash.meta")
        );
        // Same provider name again gets a distinct name; the first stays active.
        let (s2, written) = change(
            &s,
            json!({"subscription":{"action":"add","url":"https://b.example.com/x"}}),
            Some(&fetched()),
        )
        .unwrap();
        assert_eq!(written, Some(2));
        assert_eq!(s2.subscriptions[1].name, "Provider (2)");
        assert_eq!(s2.active_subscription, Some(1));
    }
    #[test]
    fn limits_and_validation() {
        assert!(change(&Settings::default(), json!({"enabled":true}), None)
            .unwrap_err()
            .contains("subscription first"));
        assert!(change(
            &Settings::default(),
            json!({"subscription":{"action":"add","url":"https://x/"}}),
            None
        )
        .is_err());
        let mut s = with_subscription();
        for i in 0..4 {
            s = change(&s, json!({"subscription":{"action":"add","name":format!("n{i}"),"url":"https://x.example/"}}), Some(&fetched())).unwrap().0;
        }
        assert!(change(
            &s,
            json!({"subscription":{"action":"add","url":"https://x.example/"}}),
            Some(&fetched())
        )
        .unwrap_err()
        .contains("maximum of 5"));
        for bad in [
            json!({"proxy_mode":"script"}),
            json!({"stack":"lwip"}),
            json!({"dns_mode":"normal"}),
            json!({"mode":"none"}),
            json!({"macs":["01:00:5e:00:00:01"]}),
            json!({"subscription":{"action":"edit","id":1,"interval_hours":721}}),
            json!({"subscription":{"action":"edit","id":1,"name":"n0"}}),
        ] {
            assert!(change(&s, bad.clone(), None).is_err(), "{bad}");
        }
        let stale = json!({"expected_revision": s.revision + 1, "enabled": false});
        assert!(edited(&s, serde_json::from_value(stale).unwrap(), None)
            .unwrap_err()
            .starts_with("Configuration changed;"));
    }
    #[test]
    fn active_subscription_cannot_be_deleted_while_on() {
        let (on, _) = change(&with_subscription(), json!({"enabled":true}), None).unwrap();
        assert!(change(
            &on,
            json!({"subscription":{"action":"delete","id":1}}),
            None
        )
        .is_err());
        let (off, _) = change(&on, json!({"enabled":false}), None).unwrap();
        let (gone, _) = change(
            &off,
            json!({"subscription":{"action":"delete","id":1}}),
            None,
        )
        .unwrap();
        assert_eq!(gone.active_subscription, None);
        assert!(gone.subscriptions.is_empty());
    }
    #[test]
    fn url_edit_requires_download_and_marks_profile() {
        let s = with_subscription();
        let same = json!({"subscription":{"action":"edit","id":1,"url":"https://sub.example.com/api?token=SECRET","name":"Mine"}});
        let (renamed, written) = change(&s, same, None).unwrap();
        assert_eq!(
            (renamed.subscriptions[0].name.as_str(), written),
            ("Mine", None)
        );
        let moved =
            json!({"subscription":{"action":"edit","id":1,"url":"https://new.example.com/"}});
        assert!(change(&s, moved.clone(), None).is_err());
        assert_eq!(change(&s, moved, Some(&fetched())).unwrap().1, Some(1));
    }
    #[test]
    fn device_choices_follow_mode() {
        let s = with_subscription();
        let (s, _) = change(&s, json!({"mode":"selected"}), None).unwrap();
        let (s, _) = change(
            &s,
            json!({"devices":[{"mac":"02:AA:BB:CC:DD:EE","selected":true}]}),
            None,
        )
        .unwrap();
        assert_eq!(s.macs, vec!["02:aa:bb:cc:dd:ee"]);
        assert!(!active_changed(
            &s,
            &change(&s, json!({"proxy_mode":"global"}), None).unwrap().0
        ));
        assert!(active_changed(
            &s,
            &change(&s, json!({"stack":"gvisor"}), None).unwrap().0
        ));
    }
    #[test]
    fn status_masks_subscription_urls() {
        let text = response(&with_subscription()).to_string();
        assert!(!text.contains("SECRET") && !text.contains("/api"), "{text}");
        assert!(text.contains("https://sub.example.com/…"));
    }
    #[test]
    fn clash_policy_keeps_sources_and_redirects_dns_into_tun() {
        let p = policy(
            &change(
                &with_subscription(),
                json!({"mode":"selected","macs":["02:11:22:33:44:55"]}),
                None,
            )
            .unwrap()
            .0,
        );
        assert!(!p.contains("MASQUERADE"));
        assert!(!p.contains("ip link del"));
        assert!(
            p.contains("--mac-source 02:11:22:33:44:55 -j MARK --set-xmark 0x20000000/0x20000000")
        );
        assert!(p.contains("DNAT --to-destination 198.19.0.53"));
        assert!(p.contains("ip -4 route replace default dev ou-clash0 metric 10 table 10421"));
        assert!(p.contains("iptables -w 2 -A OUCL_DNS4 -i ou-clash0 -j ACCEPT"));
        assert!(!p.contains("OUWG") && !p.contains("0x40000000"));
    }
    #[test]
    fn proxies_are_ordered_like_the_profile() {
        let raw = json!({"proxies":{
            "B":{"type":"Shadowsocks","history":[{"delay":80},{"delay":120}],"udp":true},
            "A":{"type":"Vmess","history":[]},
            "Auto":{"type":"URLTest","now":"B","all":["A","B"]},
            "PROXY":{"type":"Selector","now":"Auto","all":["Auto","A","B"]},
            "GLOBAL":{"type":"Selector","now":"PROXY","all":["PROXY","Auto","A","B"]}}});
        let v = proxies_view(&raw);
        let names: Vec<&str> = v["groups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| g["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["PROXY", "Auto", "GLOBAL"]);
        assert_eq!(v["groups"][0]["selectable"], json!(true));
        assert_eq!(v["groups"][1]["selectable"], json!(false));
        assert_eq!(v["nodes"]["B"]["delay"], json!(120));
        assert_eq!(v["nodes"]["A"]["delay"], Value::Null);
    }
    #[test]
    fn geodata_links_replace_downloads_and_skip_missing_files() {
        let root = std::env::temp_dir().join(format!("openui-geo-{}", std::process::id()));
        let (share, home) = (root.join("share"), root.join("home"));
        fs::create_dir_all(&share).unwrap();
        fs::create_dir_all(&home).unwrap();
        fs::write(share.join("geoip.metadb"), "bundled").unwrap();
        fs::write(home.join("geoip.metadb"), "downloaded").unwrap();
        fs::write(home.join("GeoSite.dat"), "downloaded").unwrap();
        link_geodata(&share, &home);
        link_geodata(&share, &home);
        assert_eq!(
            fs::read_link(home.join("geoip.metadb")).unwrap(),
            share.join("geoip.metadb")
        );
        assert_eq!(
            fs::read_to_string(home.join("geoip.metadb")).unwrap(),
            "bundled"
        );
        assert_eq!(
            fs::read_to_string(home.join("GeoSite.dat")).unwrap(),
            "downloaded"
        );
        assert!(!home.join("ASN.mmdb").exists());
        assert!(!home.join(".geoip.metadb.link").exists());
        fs::remove_dir_all(&root).unwrap();
    }
    #[test]
    fn export_integration_policies() {
        // `OPENUI_CLASH_TEST_POLICIES=dir cargo test` writes the scripts tests/clash-netns.sh runs.
        if let Ok(dir) = std::env::var("OPENUI_CLASH_TEST_POLICIES") {
            let p = Path::new(&dir);
            fs::create_dir_all(p).unwrap();
            let mut s = with_subscription();
            s.enabled = true;
            fs::write(p.join("all.sh"), policy(&s)).unwrap();
            s.mode = "selected".into();
            s.macs = vec!["02:11:22:33:44:55".into()];
            fs::write(p.join("selected.sh"), policy(&s)).unwrap();
            s.mode = "all".into();
            fs::write(p.join("excluded.sh"), policy(&s)).unwrap();
            fs::write(p.join("cleanup.sh"), EGRESS.cleanup()).unwrap();
            fs::write(p.join("hold.sh"), EGRESS.hold()).unwrap();
            fs::write(p.join("release.sh"), EGRESS.release()).unwrap();
            fs::write(p.join("health.sh"), EGRESS.health_check()).unwrap();
            let profile = "proxies: [{name: unused, type: socks5, server: 203.0.113.9, port: 1080}]\nhosts: {test.example: 203.0.113.1}\nrules: [\"MATCH,DIRECT\"]\n";
            let socket = p.join("mihomo.sock");
            let o = Options {
                log_level: "info",
                controller: socket.to_str().unwrap(),
                ..options(&s)
            };
            let text = clash_config::compose(&clash_config::parse(profile).unwrap(), &o);
            fs::write(p.join("config.yaml"), text).unwrap();
        }
    }
    #[test]
    fn live_controller() {
        // `OPENUI_CLASH_TEST_SOCKET=path cargo test live_controller` drives a running core
        // started from the exported smoke configuration (group PROXY: A, B).
        let Ok(sock) = std::env::var("OPENUI_CLASH_TEST_SOCKET") else {
            return;
        };
        let call = |m: &str, p: &str, b: Option<&Value>| {
            clash_ctl::call(Path::new(&sock), m, p, b, Duration::from_secs(20))
        };
        assert!(call("GET", "/version", None).unwrap()["version"].is_string());
        let view = proxies_view(&call("GET", "/proxies", None).unwrap());
        let group = &view["groups"][0];
        assert_eq!(group["name"], "PROXY", "{view}");
        assert_eq!(group["selectable"], true);
        call("PUT", "/proxies/PROXY", Some(&json!({"name":"B"}))).unwrap();
        assert!(
            call("PUT", "/proxies/PROXY", Some(&json!({"name":"missing"})))
                .unwrap_err()
                .starts_with("Clash core: ")
        );
        call(
            "PATCH",
            "/configs",
            Some(&json!({"mode":"global","log-level":"info"})),
        )
        .unwrap();
        assert_eq!(call("GET", "/configs", None).unwrap()["mode"], "global");
        // Reload from the -f file resets the runtime mode but keeps the stored selection.
        call(
            "PUT",
            "/configs?force=true",
            Some(&json!({"path":"","payload":""})),
        )
        .unwrap();
        assert_eq!(call("GET", "/configs", None).unwrap()["mode"], "rule");
        let view = proxies_view(&call("GET", "/proxies", None).unwrap());
        assert_eq!(view["groups"][0]["now"], "B", "{view}");
        let delay = call(
            "GET",
            "/proxies/A/delay?url=http%3A%2F%2F203.0.113.9%2F&timeout=1000",
            None,
        );
        assert!(delay.unwrap_err().starts_with("Clash core: "));
        // An empty list is `null` (Go nil slice); `connections()` maps it to [].
        let conns = call("GET", "/connections", None).unwrap();
        assert!(conns["connections"].is_array() || conns["connections"].is_null());
        call("DELETE", "/connections", None).unwrap();
    }
}
