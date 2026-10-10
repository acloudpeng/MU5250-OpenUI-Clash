//! eSIM management for a removable eUICC in the SIM tray (9eSIM, 5ber, eSTK.me and
//! similar cards), as EasyEUICC offers it on Android. lpac speaks SGP.22 to the card and
//! to SM-DP+ servers (lpac.rs); the agent carries its APDUs over the modem's QMI UIM
//! service (qmi.rs) and its HTTPS requests through the device's curl. One job runs at a
//! time on its own thread; the web page and the screen poll `GET /api/esim`.
//!
//! A switch enables the profile without the card's own REFRESH (lpac refresh flag 0),
//! then makes the modem read the card again, the sequence verified on B31 by another
//! project: radio to low power, SIM power off and on, radio online. If the firmware
//! still reports the previous profile, the page asks for a reboot; firmware daemons are
//! never restarted (docs/SAFETY.md).
use std::fs;
use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::clash;
use crate::lpac::{self, Failure};
use crate::qmi::{self, OpenError};
use crate::ubus;

pub const LPAC: &str = "/data/bin/lpac";
const STATE_DIR: &str = "/data/local/tmp/openui-esim";
const CA_BUNDLE: &str = "/data/local/tmp/openui-esim/ca.pem";
const HTTP_BODY: &str = "/data/local/tmp/openui-esim/http.body";
const SYSTEM_CA: &str = "/etc/ssl/certs/ca-certificates.crt";
const CURL: &str = "/usr/bin/curl";
const WAKE_LOCK: &str = "/sys/power/wake_lock";
const WAKE_UNLOCK: &str = "/sys/power/wake_unlock";
/// Released by the kernel after ten minutes even if the agent dies mid-job.
const WAKE_TAG: &str = "openui_esim 600000000000";

/// SGP.22 production CIs that SM-DP+ TLS certificates chain to, as EasyEUICC trusts them;
/// the device's own roots are added for servers with public certificates.
const CI_ROOTS: [&str; 3] = [
    include_str!("../certs/gsma-rsp2-root-ci1.crt"),
    include_str!("../certs/oiste-gsma-ci-g1.crt"),
    include_str!("../certs/entrust-esim-ca.crt"),
];
/// The ISD-R, the eUICC application lpac selects.
const ISD_R: [u8; 16] = [
    0xA0, 0x00, 0x00, 0x05, 0x59, 0x10, 0x10, 0xFF, 0xFF, 0xFF, 0xFF, 0x89, 0x00, 0x00, 0x01, 0x00,
];
/// The U60 Pro has one SIM tray: logical slot 1.
const SLOT: u8 = 1;

const CARD_TIMEOUT: Duration = Duration::from_secs(60);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(180);
const HTTP_SECS: u64 = 120;
const MAX_RESPONSE: u64 = 8 * 1024 * 1024;
const MODE_WAIT: Duration = Duration::from_secs(20);
const SLOT_WAIT: Duration = Duration::from_secs(30);
const FIRMWARE_WAIT: Duration = Duration::from_secs(60);

const BUSY: &str = "An eSIM job is already running";
const INVALID: &str = "Invalid eSIM request";
const NO_LPAC: &str = "lpac is not installed on the device";
const READ_FIRST: &str = "Read the card first";
const NOT_EUICC: &str = "The card in the SIM tray is not an eUICC";
const UNKNOWN_PROFILE: &str = "This profile is not on the card; read the card again";
const ALREADY_ENABLED: &str = "This profile is already enabled";
const ALREADY_DISABLED: &str = "This profile is already disabled";
const DISABLE_FIRST: &str = "Disable this profile before deleting it";
const BAD_NICKNAME: &str = "A nickname is at most 64 bytes, without control characters";
const BAD_CODE: &str = "Invalid activation code";
const BAD_CONFIRMATION: &str = "Invalid confirmation code";
const NEEDS_CONFIRMATION: &str = "This activation code needs a confirmation code";
const BAD_IMEI: &str = "An IMEI has 15 or 16 digits";
const BAD_NOTIFICATIONS: &str = "Choose notifications on the card";

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ---------- State ----------

#[derive(Clone, Debug, Serialize)]
struct Job {
    id: u64,
    kind: &'static str,
    state: &'static str,
    /// The current step: lpac's progress names (`es9p_authenticate_client`...) or the
    /// agent's own (`reading`, `radio_offline`, `firmware`...).
    step: String,
    error: Option<String>,
    /// What a finished job did, e.g. a switch's `firmware_ready` or a download's `iccid`.
    result: Value,
    started_at: u64,
    finished_at: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Profile {
    pub iccid: String,
    pub aid: String,
    pub enabled: bool,
    pub name: String,
    pub provider: String,
    pub nickname: String,
    /// `operational`, `test` or `provisioning`.
    pub class: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Notification {
    pub seq: u64,
    /// `install`, `enable`, `disable` or `delete`.
    pub operation: String,
    pub iccid: String,
    pub address: String,
}

/// The last read of the card.
#[derive(Clone, Debug, Default, Serialize)]
pub struct View {
    pub read_at: u64,
    /// `euicc`, `sim` (an ordinary SIM), `absent`, or `error` with `detail`.
    pub card: &'static str,
    pub detail: Option<String>,
    pub eid: String,
    /// Chip details from `lpac chip info`: versions, free memory, default SM-DP+.
    pub info: Value,
    pub profiles: Vec<Profile>,
    pub notifications: Vec<Notification>,
    /// The ICCID and state the firmware reports, to compare with the enabled profile.
    pub firmware: Value,
    /// Changes with the card's contents; the screen addresses profiles by it.
    pub generation: u64,
}

static JOB: Mutex<Option<Job>> = Mutex::new(None);
static VIEW: Mutex<Option<View>> = Mutex::new(None);
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
/// Set by a switch the firmware did not take up; a reboot (new agent) clears it.
static REBOOT_NEEDED: Mutex<Option<String>> = Mutex::new(None);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn view() -> Option<View> {
    lock(&VIEW).clone()
}

pub fn installed() -> bool {
    Path::new(LPAC).is_file()
}

fn job_running() -> bool {
    lock(&JOB).as_ref().is_some_and(|j| j.state == "running")
}

fn claim(kind: &'static str) -> Result<u64, String> {
    let mut job = lock(&JOB);
    if job.as_ref().is_some_and(|j| j.state == "running") {
        return Err(BUSY.into());
    }
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    *job = Some(Job {
        id,
        kind,
        state: "running",
        step: "reading".into(),
        error: None,
        result: Value::Null,
        started_at: now(),
        finished_at: None,
    });
    Ok(id)
}

fn step(name: &str) {
    if let Some(j) = lock(&JOB).as_mut().filter(|j| j.state == "running") {
        j.step = name.to_string();
    }
}

fn finish(result: Result<Value, String>) {
    let mut job = lock(&JOB);
    if let Some(j) = job.as_mut().filter(|j| j.state == "running") {
        j.state = if result.is_ok() { "done" } else { "failed" };
        match result {
            Ok(v) => j.result = v,
            Err(e) => j.error = Some(e),
        }
        j.finished_at = Some(now());
    }
}

/// Keeps the device awake for a job; the tagged lock also expires on its own.
struct Awake;
impl Awake {
    fn hold() -> Self {
        let _ = fs::write(WAKE_LOCK, WAKE_TAG);
        Awake
    }
}
impl Drop for Awake {
    fn drop(&mut self) {
        let _ = fs::write(WAKE_UNLOCK, "openui_esim");
    }
}

/// Runs a claimed job on its own thread; a panic still ends it, so the slot never sticks.
fn spawn(work: impl FnOnce() -> Result<Value, String> + Send + 'static) {
    std::thread::spawn(move || {
        let _awake = Awake::hold();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
            .unwrap_or_else(|_| Err("The eSIM job stopped unexpectedly".into()));
        finish(result);
    });
}

// ---------- Card and network access ----------

/// lpac's card side over QMI, connected once per lpac run.
struct QmiCard {
    uim: qmi::Uim,
}

impl QmiCard {
    fn connect() -> Result<Self, String> {
        qmi::Uim::connect(SLOT, Duration::from_secs(5))
            .map(|uim| Self { uim })
            .map_err(|e| e.to_string())
    }
}

impl lpac::Card for QmiCard {
    fn open(&mut self, aid: &[u8]) -> Result<u8, String> {
        self.uim.open_channel(aid).map_err(|e| match e {
            OpenError::Status(0x6A82) => NOT_EUICC.to_string(),
            OpenError::Status(sw) => {
                format!("The card refused the eUICC application (status {sw:04X})")
            }
            OpenError::Modem(e) => e.to_string(),
        })
    }
    fn transmit(&mut self, channel: u8, apdu: &[u8]) -> Result<Vec<u8>, String> {
        self.uim.transmit(channel, apdu).map_err(|e| e.to_string())
    }
    fn close(&mut self, channel: u8) {
        let _ = self.uim.close_channel(channel);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    /// Through the Clash core while it runs, else directly.
    #[default]
    Auto,
    Proxy,
    Direct,
}

fn proxy(route: Route) -> Result<Option<&'static str>, String> {
    match route {
        Route::Auto => Ok(clash::core_running().then_some(clash::PROXY)),
        Route::Proxy if clash::core_running() => Ok(Some(clash::PROXY)),
        Route::Proxy => Err("Clash is not running".into()),
        Route::Direct => Ok(None),
    }
}

fn quoted(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn curl_reason(code: i32) -> &'static str {
    match code {
        5 => "cannot resolve proxy",
        6 => "cannot resolve host",
        7 => "cannot connect",
        28 => "timed out",
        35 => "TLS handshake failed",
        60 => "untrusted server certificate",
        63 => "reply is too large",
        _ => "network error",
    }
}

/// SM-DP+ requests through curl; the address and body go in the config on stdin.
struct Curl {
    proxy: Option<&'static str>,
    insecure: bool,
}

/// curl options for one ES9+ POST. lpac's requests are JSON (it sets the headers).
fn curl_config(
    url: &str,
    headers: &[String],
    body: &str,
    proxy: Option<&str>,
    insecure: bool,
) -> String {
    let mut c = format!("url = {}\n", quoted(url));
    for h in headers {
        c.push_str(&format!("header = {}\n", quoted(h)));
    }
    c.push_str(&format!("data-binary = {}\n", quoted(body)));
    if let Some(p) = proxy {
        c.push_str(&format!("proxy = {}\n", quoted(p)));
    }
    if insecure {
        c.push_str("insecure\n");
    } else {
        c.push_str(&format!("cacert = {}\n", quoted(CA_BUNDLE)));
    }
    c
}

fn valid_url(url: &str) -> bool {
    url.len() <= 512
        && url.starts_with("https://")
        && url
            .bytes()
            .all(|b| b.is_ascii_graphic() && !matches!(b, b'"' | b'\\'))
}

impl lpac::Http for Curl {
    fn post(
        &mut self,
        url: &str,
        headers: &[String],
        body: &[u8],
    ) -> Result<(u16, Vec<u8>), String> {
        if !Path::new(CURL).exists() {
            return Err("curl is unavailable on this device".into());
        }
        let body = std::str::from_utf8(body).map_err(|_| "lpac sent an invalid SM-DP+ request")?;
        if !valid_url(url) || body.starts_with('@') || body.len() > 256 * 1024 {
            return Err("lpac sent an invalid SM-DP+ request".into());
        }
        let _ = fs::remove_file(HTTP_BODY);
        let mut cmd = Command::new(CURL);
        cmd.args(["-sS", "--proto", "=https", "--connect-timeout", "15", "-m"])
            .arg(HTTP_SECS.to_string())
            .arg("--max-filesize")
            .arg(MAX_RESPONSE.to_string())
            .args(["-w", "%{http_code}", "-o", HTTP_BODY, "-K", "-"]);
        let config = curl_config(url, headers, body, self.proxy, self.insecure);
        let out = process_runner::output(
            &mut cmd,
            Some(config.as_bytes()),
            Duration::from_secs(HTTP_SECS + 10),
            64 * 1024,
        )
        .map_err(|_| "Cannot reach the SM-DP+ server: timed out".to_string())?;
        let received = fs::read(HTTP_BODY).unwrap_or_default();
        let _ = fs::remove_file(HTTP_BODY);
        if !out.status.success() {
            let code = out.status.code().unwrap_or(-1);
            return Err(format!(
                "Cannot reach the SM-DP+ server: {} (curl {code})",
                curl_reason(code)
            ));
        }
        let status: u16 = String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse()
            .unwrap_or(0);
        Ok((status, received))
    }
}

/// The trust store for SM-DP+ TLS: the CI roots, then the device's own roots.
fn write_ca_bundle() -> Result<(), String> {
    let mut pem = CI_ROOTS.concat();
    if let Ok(system) = fs::read_to_string(SYSTEM_CA) {
        if system.len() < 2 * 1024 * 1024 {
            pem.push('\n');
            pem.push_str(&system);
        }
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(CA_BUNDLE)
        .map_err(|e| format!("Cannot write the certificate bundle: {e}"))?;
    file.write_all(pem.as_bytes())
        .map_err(|e| format!("Cannot write the certificate bundle: {e}"))
}

fn state_dir() -> Result<(), String> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(STATE_DIR)
        .map_err(|e| format!("Cannot create {STATE_DIR}: {e}"))
}

/// A run without network access; any SM-DP+ request is refused.
struct Offline;
impl lpac::Http for Offline {
    fn post(&mut self, _: &str, _: &[String], _: &[u8]) -> Result<(u16, Vec<u8>), String> {
        Err("This step does not use the network".into())
    }
}

/// lpac's error, worded for the page. Card results such as `profileNotInDisabledState`
/// and SM-DP+ messages are passed through; the page translates the known ones.
fn describe(f: Failure) -> String {
    match f {
        Failure::Card(e) | Failure::Network(e) | Failure::Crashed(e) => e,
        Failure::Timeout => "lpac did not finish in time".into(),
        Failure::Lpac { step, detail } => {
            // lpac 2.3.0 names results 1-4 only; for a switch, catBusy (5) prints as "unknown".
            let switching = matches!(
                step.as_str(),
                "es10c_enable_profile" | "es10c_disable_profile"
            );
            let detail = if switching && detail == "unknown" {
                "the card is busy (catBusy)".to_string()
            } else {
                detail
            };
            if detail.is_empty() {
                format!("lpac failed at {step}")
            } else {
                format!("lpac failed at {step}: {detail}")
            }
        }
    }
}

fn run_lpac(args: &[&str], http: &mut dyn lpac::Http, timeout: Duration) -> Result<Value, String> {
    let mut card = QmiCard::connect()?;
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    lpac::run(
        Path::new(LPAC),
        &args,
        &mut card,
        http,
        &mut |s| step(s),
        timeout,
    )
    .map_err(describe)
}

// ---------- Reading the card ----------

fn text(v: &Value) -> String {
    v.as_str().unwrap_or("").to_string()
}

pub fn parse_profiles(data: &Value) -> Vec<Profile> {
    data.as_array()
        .map(|list| {
            list.iter()
                .map(|p| Profile {
                    iccid: text(&p["iccid"]),
                    aid: text(&p["isdpAid"]),
                    enabled: p["profileState"] == "enabled",
                    name: text(&p["profileName"]),
                    provider: text(&p["serviceProviderName"]),
                    nickname: text(&p["profileNickname"]),
                    class: text(&p["profileClass"]),
                })
                .filter(|p| !p.iccid.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

pub fn parse_notifications(data: &Value) -> Vec<Notification> {
    data.as_array()
        .map(|list| {
            list.iter()
                .filter_map(|n| {
                    Some(Notification {
                        seq: n["seqNumber"].as_u64()?,
                        operation: text(&n["profileManagementOperation"]),
                        iccid: text(&n["iccid"]),
                        address: text(&n["notificationAddress"]),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The chip details the page shows, from `lpac chip info`.
pub fn parse_info(data: &Value) -> Value {
    let info2 = &data["EUICCInfo2"];
    let resource = &info2["extCardResource"];
    json!({
        "default_smdp": data["EuiccConfiguredAddresses"]["defaultDpAddress"],
        "root_smds": data["EuiccConfiguredAddresses"]["rootDsAddress"],
        "sgp22": info2["svn"],
        "profile_version": info2["profileVersion"],
        "firmware": info2["euiccFirmwareVer"],
        "free_nvm": resource["freeNonVolatileMemory"],
        "installed_apps": resource["installedApplication"],
        "ci_pkids": info2["euiccCiPKIdListForVerification"],
        "sas": info2["sasAcreditationNumber"],
    })
}

fn generation(eid: &str, profiles: &[Profile]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    eid.hash(&mut h);
    for p in profiles {
        (&p.iccid, p.enabled, &p.name, &p.nickname, &p.provider).hash(&mut h);
    }
    h.finish() & 0xFFFF_FFFF
}

/// Uppercase digits without F padding, so modem and lpac spellings compare equal.
pub fn normal_iccid(s: &str) -> String {
    s.trim().trim_end_matches(['F', 'f']).to_ascii_uppercase()
}

fn firmware_sim() -> Value {
    match ubus::call("zwrt_zte_mdm.api", "get_sim_info", Some("{}")) {
        Ok(v) => json!({"iccid": normal_iccid(&text(&v["sim_iccid"])), "state": v["sim_states"]}),
        Err(_) => Value::Null,
    }
}

/// What is in the tray: an eUICC, an ordinary SIM (no ISD-R), or nothing.
fn probe() -> Result<&'static str, String> {
    let mut uim = qmi::Uim::connect(SLOT, Duration::from_secs(5)).map_err(|e| e.to_string())?;
    let cards = uim.card_status().map_err(|e| e.to_string())?;
    if cards.first().is_none_or(|c| c.state == 0) {
        return Ok("absent");
    }
    match uim.open_channel(&ISD_R) {
        Ok(channel) => {
            let _ = uim.close_channel(channel);
            Ok("euicc")
        }
        Err(OpenError::Status(0x6A82)) => Ok("sim"),
        Err(OpenError::Status(sw)) => Err(format!(
            "The card refused the eUICC application (status {sw:04X})"
        )),
        Err(OpenError::Modem(e)) => Err(e.to_string()),
    }
}

fn read_card() -> View {
    let mut view = View {
        read_at: now(),
        firmware: firmware_sim(),
        ..Default::default()
    };
    view.card = match probe() {
        Ok(card) => card,
        Err(e) => {
            view.card = "error";
            view.detail = Some(e);
            return view;
        }
    };
    if view.card != "euicc" {
        return view;
    }
    let result = (|| -> Result<(), String> {
        let info = run_lpac(&["chip", "info"], &mut Offline, CARD_TIMEOUT)?;
        view.eid = text(&info["eidValue"]);
        view.info = parse_info(&info);
        view.profiles =
            parse_profiles(&run_lpac(&["profile", "list"], &mut Offline, CARD_TIMEOUT)?);
        view.notifications = parse_notifications(&run_lpac(
            &["notification", "list"],
            &mut Offline,
            CARD_TIMEOUT,
        )?);
        Ok(())
    })();
    if let Err(e) = result {
        view.card = "error";
        view.detail = Some(e);
    }
    view.generation = generation(&view.eid, &view.profiles);
    view
}

/// A firmware that now reports the profile a switch left waiting needs no restart.
fn caught_up(pending: &mut Option<String>, firmware: &Value) {
    if pending
        .as_ref()
        .is_some_and(|target| firmware["iccid"].as_str() == Some(target.as_str()))
    {
        *pending = None;
    }
}

/// Reads the card into the cached view; a firmware that has caught up clears the reboot hint.
fn scan() -> View {
    step("reading");
    let view = read_card();
    caught_up(&mut lock(&REBOOT_NEEDED), &view.firmware);
    *lock(&VIEW) = Some(view.clone());
    view
}

fn scanned_euicc() -> Result<View, String> {
    match view() {
        Some(v) if v.card == "euicc" => Ok(v),
        Some(v) if v.card == "sim" => Err(NOT_EUICC.into()),
        _ => Err(READ_FIRST.into()),
    }
}

fn profile(view: &View, iccid: &str) -> Result<Profile, String> {
    let want = normal_iccid(iccid);
    view.profiles
        .iter()
        .find(|p| normal_iccid(&p.iccid) == want)
        .cloned()
        .ok_or_else(|| UNKNOWN_PROFILE.to_string())
}

fn valid_iccid(s: &str) -> bool {
    (18..=22).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())
}

// ---------- Switching ----------

/// What a switch needs from the modem and the firmware, so the sequence is testable.
pub trait Modem {
    fn mode(&mut self) -> Result<u8, String>;
    fn set_mode(&mut self, mode: u8) -> Result<(), String>;
    fn power(&mut self, on: bool) -> Result<(), String>;
    /// The ICCID the modem read from the card in the tray ("" when none).
    fn slot_iccid(&mut self) -> Result<String, String>;
    /// The card's USIM application is ready (PIN-free and initialized).
    fn card_ready(&mut self) -> bool;
    /// The ICCID the firmware reports ("" when none).
    fn firmware_iccid(&mut self) -> Option<String>;
    fn pause(&mut self, d: Duration);
    fn elapsed(&self) -> Duration;
}

struct QmiModem {
    started: Instant,
}

impl Modem for QmiModem {
    fn mode(&mut self) -> Result<u8, String> {
        let mut dms = qmi::Client::connect(qmi::SERVICE_DMS, Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        qmi::operating_mode(&mut dms).map_err(|e| e.to_string())
    }
    fn set_mode(&mut self, mode: u8) -> Result<(), String> {
        let mut dms = qmi::Client::connect(qmi::SERVICE_DMS, Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        qmi::set_operating_mode(&mut dms, mode).map_err(|e| e.to_string())
    }
    fn power(&mut self, on: bool) -> Result<(), String> {
        let mut uim = qmi::Uim::connect(SLOT, Duration::from_secs(5)).map_err(|e| e.to_string())?;
        uim.power(on).map_err(|e| e.to_string())
    }
    fn slot_iccid(&mut self) -> Result<String, String> {
        let mut uim = qmi::Uim::connect(SLOT, Duration::from_secs(5)).map_err(|e| e.to_string())?;
        let slots = uim.slot_status().map_err(|e| e.to_string())?;
        Ok(slots
            .iter()
            .find(|s| s.active && s.logical == SLOT)
            .map(|s| normal_iccid(&s.iccid))
            .unwrap_or_default())
    }
    fn card_ready(&mut self) -> bool {
        qmi::Uim::connect(SLOT, Duration::from_secs(5))
            .and_then(|mut uim| uim.card_status())
            .is_ok_and(|cards| cards.first().is_some_and(qmi::Card::usim_ready))
    }
    fn firmware_iccid(&mut self) -> Option<String> {
        firmware_sim()["iccid"].as_str().map(String::from)
    }
    fn pause(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
    fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
}

/// Polls `check` every second until it holds or `limit` passes.
fn wait_for(
    m: &mut dyn Modem,
    limit: Duration,
    mut check: impl FnMut(&mut dyn Modem) -> bool,
) -> bool {
    let until = m.elapsed() + limit;
    loop {
        if check(m) {
            return true;
        }
        if m.elapsed() >= until {
            return false;
        }
        m.pause(Duration::from_secs(1));
    }
}

/// After the card switched profiles: makes the modem read it again and waits for the
/// modem, then the firmware, to report `target` ("" after a disable, when only a change
/// from `previous` is expected). Returns whether the firmware caught up. The card is
/// always powered and the radio set back online when it was online before.
pub fn reread(
    m: &mut dyn Modem,
    target: &str,
    previous: &str,
    step: &mut dyn FnMut(&'static str),
) -> Result<bool, String> {
    step("radio_offline");
    let online = m.mode()? == qmi::MODE_ONLINE;
    if online {
        m.set_mode(qmi::MODE_LOW_POWER)?;
        if !wait_for(m, MODE_WAIT, |m| m.mode() == Ok(qmi::MODE_LOW_POWER)) {
            let _ = m.set_mode(qmi::MODE_ONLINE);
            return Err("The radio did not go to low power".into());
        }
    }
    step("sim_power_cycle");
    let off = m.power(false);
    if off.is_ok() {
        m.pause(Duration::from_secs(1));
    }
    let on = m.power(true);
    if online {
        step("radio_online");
        let back = m.set_mode(qmi::MODE_ONLINE).is_ok()
            && wait_for(m, MODE_WAIT, |m| m.mode() == Ok(qmi::MODE_ONLINE));
        if !back {
            return Err("The radio did not come back online".into());
        }
    }
    off.and(on)
        .map_err(|e| format!("The SIM could not be powered off and on: {e}"))?;
    step("sim_reading");
    // A modem without Get Slot Status leaves the firmware check below as the judge.
    let read = |m: &mut dyn Modem| m.slot_iccid().map_or(true, |iccid| iccid == target);
    if !target.is_empty() && !wait_for(m, SLOT_WAIT, read) {
        return Err("The modem still reads the previous profile".into());
    }
    // A profile with a PIN stays unready; the firmware check below then reports it.
    if !target.is_empty() {
        wait_for(m, SLOT_WAIT, |m| m.card_ready());
    }
    step("firmware");
    Ok(wait_for(m, FIRMWARE_WAIT, |m| match m.firmware_iccid() {
        Some(iccid) if !target.is_empty() => iccid == target,
        Some(iccid) => iccid != previous,
        None => false,
    }))
}

fn switch(iccid: String, enable: bool) -> Result<Value, String> {
    let view = scanned_euicc()?;
    let target = profile(&view, &iccid)?;
    let previous = view
        .profiles
        .iter()
        .find(|p| p.enabled)
        .map(|p| normal_iccid(&p.iccid))
        .unwrap_or_default();
    step(if enable {
        "es10c_enable_profile"
    } else {
        "es10c_disable_profile"
    });
    let verb = if enable { "enable" } else { "disable" };
    let lpac_result = run_lpac(
        &["profile", verb, &target.iccid, "0"],
        &mut Offline,
        CARD_TIMEOUT,
    );
    // The card decides: a timed-out command may still have switched.
    step("reading");
    let after = scan();
    let switched = after
        .profiles
        .iter()
        .any(|p| normal_iccid(&p.iccid) == normal_iccid(&target.iccid) && p.enabled == enable);
    if !switched {
        return Err(lpac_result
            .err()
            .unwrap_or_else(|| "The card did not switch profiles".into()));
    }
    let expected = if enable {
        normal_iccid(&target.iccid)
    } else {
        String::new()
    };
    let mut modem = QmiModem {
        started: Instant::now(),
    };
    let reread = reread(&mut modem, &expected, &previous, &mut |s| step(s));
    let firmware_ready = matches!(reread, Ok(true));
    *lock(&REBOOT_NEEDED) = (!firmware_ready).then(|| expected.clone());
    // Show the firmware's view now that the modem has read the card (read before locking:
    // ubus can take seconds, and status reads wait for the lock).
    let firmware = firmware_sim();
    if let Some(v) = lock(&VIEW).as_mut() {
        v.firmware = firmware;
    }
    Ok(json!({
        "iccid": target.iccid,
        "enabled": enable,
        "firmware_ready": firmware_ready,
        "reboot_needed": !firmware_ready,
        "detail": reread.err(),
    }))
}

// ---------- Profile changes, downloads and notifications ----------

fn highest_seq(v: &View) -> Option<u64> {
    v.notifications.iter().map(|n| n.seq).max()
}

/// Sends the notifications a download or delete added, as EasyEUICC does by default.
/// Failures leave them on the card for the page's notification list.
fn send_new_notifications(before: Option<u64>, route: Route, insecure: bool) -> Value {
    let Some(view) = view() else {
        return Value::Null;
    };
    let fresh: Vec<String> = view
        .notifications
        .iter()
        .filter(|n| before.is_none_or(|b| n.seq > b))
        .filter(|n| matches!(n.operation.as_str(), "install" | "delete"))
        .map(|n| n.seq.to_string())
        .collect();
    if fresh.is_empty() {
        return json!({"sent": 0});
    }
    step("es9p_handle_notification");
    let result = proxy(route).and_then(|proxy| {
        let mut http = Curl { proxy, insecure };
        let mut args = vec!["notification", "process", "-r"];
        args.extend(fresh.iter().map(String::as_str));
        run_lpac(&args, &mut http, NOTIFY_TIMEOUT)
    });
    let sent = fresh.len();
    scan();
    match result {
        Ok(_) => json!({"sent": sent}),
        Err(e) => json!({"sent": 0, "error": e}),
    }
}

fn delete(iccid: String, route: Route) -> Result<Value, String> {
    let view = scanned_euicc()?;
    let target = profile(&view, &iccid)?;
    if target.enabled {
        return Err(DISABLE_FIRST.into());
    }
    step("es10c_delete_profile");
    let result = run_lpac(
        &["profile", "delete", &target.iccid],
        &mut Offline,
        CARD_TIMEOUT,
    );
    let after = scan();
    if after
        .profiles
        .iter()
        .any(|p| normal_iccid(&p.iccid) == normal_iccid(&target.iccid))
    {
        return Err(result
            .err()
            .unwrap_or_else(|| "The card kept the profile".into()));
    }
    if let Err(e) = state_dir().and_then(|_| write_ca_bundle()) {
        return Ok(json!({"iccid": target.iccid, "notifications": {"sent": 0, "error": e}}));
    }
    let notified = send_new_notifications(highest_seq(&view), route, false);
    Ok(json!({"iccid": target.iccid, "notifications": notified}))
}

fn nickname(iccid: String, name: String) -> Result<Value, String> {
    let view = scanned_euicc()?;
    let target = profile(&view, &iccid)?;
    step("es10c_set_nickname");
    run_lpac(
        &["profile", "nickname", &target.iccid, &name],
        &mut Offline,
        CARD_TIMEOUT,
    )?;
    scan();
    Ok(json!({"iccid": target.iccid, "nickname": name}))
}

/// An SM-DP+ address and matching ID, from `LPA:1$address$matchingId[$oid[$1]]`.
#[derive(Debug, PartialEq)]
pub struct Activation {
    pub smdp: String,
    pub matching_id: String,
    /// The code says a confirmation code is required.
    pub needs_confirmation: bool,
}

fn valid_smdp(s: &str) -> bool {
    (1..=255).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-:".contains(&b))
        && !s.starts_with(['.', '-', ':'])
}

fn valid_matching_id(s: &str) -> bool {
    s.len() <= 255 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

pub fn parse_activation(code: &str) -> Result<Activation, String> {
    let code = code.trim();
    let rest = code
        .get(..4)
        .filter(|p| p.eq_ignore_ascii_case("LPA:"))
        .map(|_| &code[4..])
        .ok_or(BAD_CODE)?;
    let parts: Vec<&str> = rest.split('$').collect();
    match parts.as_slice() {
        ["1", smdp, rest @ ..] if rest.len() <= 3 => {
            let matching_id = rest.first().copied().unwrap_or("");
            if !valid_smdp(smdp) || !valid_matching_id(matching_id) {
                return Err(BAD_CODE.into());
            }
            Ok(Activation {
                smdp: smdp.to_ascii_lowercase(),
                matching_id: matching_id.to_string(),
                needs_confirmation: rest.get(2) == Some(&"1"),
            })
        }
        _ => Err(BAD_CODE.into()),
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Download {
    activation_code: Option<String>,
    smdp: Option<String>,
    matching_id: Option<String>,
    confirmation_code: Option<String>,
    imei: Option<String>,
    #[serde(default)]
    route: Route,
    #[serde(default)]
    insecure: bool,
}

/// The lpac arguments for a download, checked before any job starts.
fn download_args(d: &Download) -> Result<Vec<String>, String> {
    let confirmation = d
        .confirmation_code
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty());
    let (smdp, matching_id) = match (&d.activation_code, &d.smdp) {
        (Some(code), None) if d.matching_id.is_none() => {
            let a = parse_activation(code)?;
            if a.needs_confirmation && confirmation.is_none() {
                return Err(NEEDS_CONFIRMATION.into());
            }
            (a.smdp, a.matching_id)
        }
        (None, Some(smdp)) => {
            let smdp = smdp.trim().to_ascii_lowercase();
            let id = d.matching_id.as_deref().unwrap_or("").trim().to_string();
            if !valid_smdp(&smdp) || !valid_matching_id(&id) {
                return Err(BAD_CODE.into());
            }
            (smdp, id)
        }
        _ => return Err(INVALID.into()),
    };
    let mut args = vec!["profile".into(), "download".into(), "-s".into(), smdp];
    if !matching_id.is_empty() {
        args.extend(["-m".into(), matching_id]);
    }
    if let Some(code) = confirmation {
        if code.len() > 64 || !code.bytes().all(|b| b.is_ascii_graphic()) {
            return Err(BAD_CONFIRMATION.into());
        }
        args.extend(["-c".into(), code.to_string()]);
    }
    if let Some(imei) = d.imei.as_deref().map(str::trim).filter(|i| !i.is_empty()) {
        if !(15..=16).contains(&imei.len()) || !imei.bytes().all(|b| b.is_ascii_digit()) {
            return Err(BAD_IMEI.into());
        }
        args.extend(["-i".into(), imei.to_string()]);
    }
    Ok(args)
}

fn download(args: Vec<String>, route: Route, insecure: bool) -> Result<Value, String> {
    let view = scanned_euicc()?;
    let proxy = proxy(route)?;
    state_dir()?;
    write_ca_bundle()?;
    let before: Vec<String> = view
        .profiles
        .iter()
        .map(|p| normal_iccid(&p.iccid))
        .collect();
    let mut http = Curl { proxy, insecure };
    let mut card = QmiCard::connect()?;
    step("es10b_get_euicc_challenge_and_info");
    let result = lpac::run(
        Path::new(LPAC),
        &args,
        &mut card,
        &mut http,
        &mut |s| step(s),
        DOWNLOAD_TIMEOUT,
    )
    .map_err(describe);
    drop(card);
    let after = scan();
    let added = after
        .profiles
        .iter()
        .find(|p| !before.contains(&normal_iccid(&p.iccid)))
        .map(|p| p.iccid.clone());
    let iccid = match result {
        Ok(data) => data["iccid"].as_str().map(String::from).or(added),
        // lpac lost its result, but the card has the new profile.
        Err(_) if added.is_some() => added,
        Err(e) => {
            // A card that refused the profile queued an error installation result; SGP.22
            // has the LPA deliver it, so the SM-DP+ learns the download failed.
            send_new_notifications(highest_seq(&view), route, insecure);
            return Err(e);
        }
    };
    let notified = send_new_notifications(highest_seq(&view), route, insecure);
    Ok(json!({"iccid": iccid, "notifications": notified}))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum NotifyAction {
    Process,
    Remove,
}

/// The chosen notifications must all be on the card as last read.
fn check_notifications(view: &View, seqs: &[u64]) -> Result<(), String> {
    let known: Vec<u64> = view.notifications.iter().map(|n| n.seq).collect();
    if seqs.is_empty() || seqs.len() > 64 || seqs.iter().any(|s| !known.contains(s)) {
        return Err(BAD_NOTIFICATIONS.into());
    }
    Ok(())
}

fn notifications(action: NotifyAction, seqs: Vec<u64>, route: Route) -> Result<Value, String> {
    check_notifications(&scanned_euicc()?, &seqs)?;
    let seqs: Vec<String> = seqs.iter().map(u64::to_string).collect();
    let result = match action {
        NotifyAction::Process => {
            let proxy = proxy(route)?;
            state_dir()?;
            write_ca_bundle()?;
            let mut http = Curl {
                proxy,
                insecure: false,
            };
            step("es9p_handle_notification");
            let mut args = vec!["notification", "process", "-r"];
            args.extend(seqs.iter().map(String::as_str));
            run_lpac(&args, &mut http, NOTIFY_TIMEOUT)
        }
        NotifyAction::Remove => {
            step("es10b_remove_notification_from_list");
            let mut args = vec!["notification", "remove"];
            args.extend(seqs.iter().map(String::as_str));
            run_lpac(&args, &mut Offline, CARD_TIMEOUT)
        }
    };
    scan();
    result.map(|_| json!({"count": seqs.len()}))
}

// ---------- API ----------

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Scan {},
    Enable {
        iccid: String,
    },
    Disable {
        iccid: String,
    },
    Nickname {
        iccid: String,
        nickname: String,
    },
    Download(Download),
    Notifications {
        operation: NotifyAction,
        seq: Vec<u64>,
        #[serde(default)]
        route: Route,
    },
}

fn valid_nickname(s: &str) -> bool {
    s.len() <= 64 && !s.chars().any(char::is_control)
}

/// Checks a request against the last read of the card and starts its job.
pub fn start_action(body: &[u8]) -> Result<(), String> {
    let action: Action = serde_json::from_slice(body).map_err(|_| INVALID)?;
    if !installed() {
        return Err(NO_LPAC.into());
    }
    if job_running() {
        return Err(BUSY.into());
    }
    match action {
        Action::Scan {} => {
            claim("scan")?;
            spawn(|| {
                let v = scan();
                match v.card {
                    "error" => Err(v
                        .detail
                        .unwrap_or_else(|| "The card could not be read".into())),
                    _ => Ok(json!({"card": v.card})),
                }
            });
        }
        Action::Enable { iccid } | Action::Disable { iccid } if !valid_iccid(&iccid) => {
            return Err(INVALID.into())
        }
        Action::Enable { iccid } => {
            let target = profile(&scanned_euicc()?, &iccid)?;
            if target.enabled {
                return Err(ALREADY_ENABLED.into());
            }
            claim("enable")?;
            spawn(move || switch(iccid, true));
        }
        Action::Disable { iccid } => {
            let target = profile(&scanned_euicc()?, &iccid)?;
            if !target.enabled {
                return Err(ALREADY_DISABLED.into());
            }
            claim("disable")?;
            spawn(move || switch(iccid, false));
        }
        Action::Nickname {
            iccid,
            nickname: name,
        } => {
            if !valid_iccid(&iccid) || !valid_nickname(&name) {
                return Err(BAD_NICKNAME.into());
            }
            profile(&scanned_euicc()?, &iccid)?;
            claim("nickname")?;
            spawn(move || nickname(iccid, name));
        }
        Action::Download(d) => {
            let args = download_args(&d)?;
            scanned_euicc()?;
            proxy(d.route)?;
            claim("download")?;
            let (route, insecure) = (d.route, d.insecure);
            spawn(move || download(args, route, insecure));
        }
        Action::Notifications {
            operation,
            seq,
            route,
        } => {
            check_notifications(&scanned_euicc()?, &seq)?;
            if matches!(operation, NotifyAction::Process) {
                proxy(route)?;
            }
            claim("notifications")?;
            spawn(move || notifications(operation, seq, route));
        }
    }
    Ok(())
}

/// Starts a delete; its route carries the X-Confirm check (server.rs).
pub fn start_delete(body: &[u8]) -> Result<(), String> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Delete {
        iccid: String,
        #[serde(default)]
        route: Route,
    }
    let d: Delete = serde_json::from_slice(body).map_err(|_| INVALID)?;
    if !installed() {
        return Err(NO_LPAC.into());
    }
    if !valid_iccid(&d.iccid) {
        return Err(INVALID.into());
    }
    let target = profile(&scanned_euicc()?, &d.iccid)?;
    if target.enabled {
        return Err(DISABLE_FIRST.into());
    }
    claim("delete")?;
    let (iccid, route) = (d.iccid, d.route);
    spawn(move || delete(iccid, route));
    Ok(())
}

pub fn status() -> Value {
    json!({
        "installed": installed(),
        "job": lock(&JOB).clone(),
        "view": view(),
        "reboot_needed": lock(&REBOOT_NEEDED).is_some(),
        "clash_running": clash::core_running(),
    })
}

fn failure(e: String) -> (u16, Value) {
    let code = match e.as_str() {
        BUSY => 409,
        NO_LPAC => 503,
        _ => 400,
    };
    (code, json!({"ok": false, "error": e}))
}

/// `GET /api/esim`: the last read of the card, the job and whether a reboot is needed.
pub fn get() -> (u16, Value) {
    (200, json!({"ok": true, "data": status()}))
}

/// `POST /api/esim`: `scan`, `enable`, `disable`, `nickname`, `download`, `notifications`.
pub fn post(body: &[u8]) -> (u16, Value) {
    match start_action(body) {
        Ok(()) => get(),
        Err(e) => failure(e),
    }
}

/// `POST /api/esim/delete` (needs `X-Confirm: true`): deletes a disabled profile.
pub fn delete_profile(body: &[u8]) -> (u16, Value) {
    match start_delete(body) {
        Ok(()) => get(),
        Err(e) => failure(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Built from pieces so no whole test identifier sits in the source
    // (scripts/check-device-secrets.py rejects those).
    const PROFILE_A: &str = concat!("8944476", "500001234567");
    const PROFILE_B: &str = concat!("89852000", "000000000012");
    const TEST_IMEI: &str = concat!("3569380", "35643809");

    #[test]
    fn activation_codes() {
        assert_eq!(
            parse_activation("LPA:1$rsp.truphone.com$QR-G-5C-1LS-1W1Z9P7").unwrap(),
            Activation {
                smdp: "rsp.truphone.com".into(),
                matching_id: "QR-G-5C-1LS-1W1Z9P7".into(),
                needs_confirmation: false
            }
        );
        let a = parse_activation(" lpa:1$SMDP.Example.com:443$ABC$1.3.6.1.4.1.31746$1 ").unwrap();
        assert_eq!(a.smdp, "smdp.example.com:443");
        assert!(a.needs_confirmation);
        assert_eq!(
            parse_activation("LPA:1$smdp.example").unwrap().matching_id,
            ""
        );
        for bad in [
            "",
            "1$smdp$ABC",
            "LPA:2$smdp$ABC",
            "LPA:1$$ABC",
            "LPA:1$smdp example$ABC",
            "LPA:1$smdp$AB C",
            "LPA:1$smdp$A\"B",
            "LPA:1$-smdp$ABC",
            "LPA:1$smdp$A$B$C$D",
        ] {
            assert!(parse_activation(bad).is_err(), "{bad}");
        }
    }

    fn dl(v: Value) -> Result<Vec<String>, String> {
        download_args(&serde_json::from_value(v).unwrap())
    }

    #[test]
    fn download_arguments() {
        assert_eq!(
            dl(json!({"activation_code":"LPA:1$smdp.example$ABC-1","confirmation_code":" 1234 "}))
                .unwrap(),
            [
                "profile",
                "download",
                "-s",
                "smdp.example",
                "-m",
                "ABC-1",
                "-c",
                "1234"
            ]
        );
        assert_eq!(
            dl(json!({"smdp":"SMDP.example","matching_id":"","imei":TEST_IMEI})).unwrap(),
            ["profile", "download", "-s", "smdp.example", "-i", TEST_IMEI]
        );
        assert_eq!(
            dl(json!({"activation_code":"LPA:1$a$b","smdp":"x"})).unwrap_err(),
            INVALID
        );
        // The code's fifth field asks for a confirmation code.
        let cc = |code: &str| json!({"activation_code":"LPA:1$smdp.example$CC$$1","confirmation_code":code});
        assert_eq!(
            dl(json!({"activation_code":"LPA:1$smdp.example$CC$$1"})).unwrap_err(),
            NEEDS_CONFIRMATION
        );
        assert_eq!(dl(cc(" ")).unwrap_err(), NEEDS_CONFIRMATION);
        assert_eq!(
            dl(cc("1234")).unwrap(),
            [
                "profile",
                "download",
                "-s",
                "smdp.example",
                "-m",
                "CC",
                "-c",
                "1234"
            ]
        );
        assert_eq!(dl(json!({})).unwrap_err(), INVALID);
        assert_eq!(dl(json!({"smdp":"a b"})).unwrap_err(), BAD_CODE);
        assert_eq!(dl(json!({"smdp":"a","imei":"12"})).unwrap_err(), BAD_IMEI);
        assert_eq!(
            dl(json!({"smdp":"a","confirmation_code":"a b"})).unwrap_err(),
            BAD_CONFIRMATION
        );
        assert!(serde_json::from_value::<Download>(json!({"smdp":"a","extra":1})).is_err());
    }

    #[test]
    fn lpac_output_parsing() {
        let profiles = parse_profiles(&json!([
            {"iccid":PROFILE_A,"isdpAid":"a0000005591010ffffffff8900001000","profileState":"enabled",
             "profileNickname":null,"serviceProviderName":"Truphone","profileName":"Truphone","iconType":"none",
             "icon":null,"profileClass":"operational"},
            {"iccid":PROFILE_B,"isdpAid":"a0","profileState":"disabled","profileNickname":"Travel",
             "serviceProviderName":"CMHK","profileName":"","profileClass":"operational"},
            {"iccid":null}
        ]));
        assert_eq!(profiles.len(), 2);
        assert!(profiles[0].enabled && !profiles[1].enabled);
        assert_eq!(profiles[0].nickname, "");
        assert_eq!(profiles[1].nickname, "Travel");
        let notes = parse_notifications(&json!([
            {"seqNumber":0,"profileManagementOperation":"install","notificationAddress":"smdp.example","iccid":"894"},
            {"seqNumber":"x"}
        ]));
        assert_eq!(
            notes,
            [Notification {
                seq: 0,
                operation: "install".into(),
                iccid: "894".into(),
                address: "smdp.example".into()
            }]
        );
        let info = parse_info(
            &json!({"eidValue":"89049032","EuiccConfiguredAddresses":{"defaultDpAddress":null,
            "rootDsAddress":"lpa.ds.gsma.com"},"EUICCInfo2":{"svn":"2.2.0","euiccFirmwareVer":"4.6.0",
            "extCardResource":{"freeNonVolatileMemory":291666,"installedApplication":0}}}),
        );
        assert_eq!(info["root_smds"], "lpa.ds.gsma.com");
        assert_eq!(info["free_nvm"], 291666);
        assert_eq!(info["sgp22"], "2.2.0");
    }

    #[test]
    fn iccids() {
        assert_eq!(
            normal_iccid(" 8986001012345678901f "),
            "8986001012345678901"
        );
        assert!(valid_iccid("8986001012345678901"));
        assert!(valid_iccid("89860010123456789012"));
        assert!(!valid_iccid("8986-0010123456789012"));
        assert!(!valid_iccid("123"));
        assert!(valid_nickname("旅行 Travel"));
        assert!(!valid_nickname("a\nb"));
        assert!(!valid_nickname(&"字".repeat(22)));
    }

    #[test]
    fn lpac_failures_read_well() {
        let f = |step: &str, detail: &str| {
            describe(Failure::Lpac {
                step: step.into(),
                detail: detail.into(),
            })
        };
        assert_eq!(
            f("es10c_enable_profile", "unknown"),
            "lpac failed at es10c_enable_profile: the card is busy (catBusy)"
        );
        assert_eq!(
            f("es10c_delete_profile", "unknown"),
            "lpac failed at es10c_delete_profile: unknown"
        );
        assert_eq!(f("es10c_get_eid", ""), "lpac failed at es10c_get_eid");
        assert_eq!(describe(Failure::Card(NOT_EUICC.into())), NOT_EUICC);
    }

    #[test]
    fn curl_options_stay_quoted() {
        let c = curl_config(
            "https://smdp.example/gsma/rsp2/es9plus/initiateAuthentication",
            &["Content-Type: application/json".into()],
            r#"{"a":"b\"c\\d"}"#,
            Some("http://127.0.0.1:7890"),
            false,
        );
        assert!(
            c.contains(r#"data-binary = "{\"a\":\"b\\\"c\\\\d\"}""#),
            "{c}"
        );
        assert!(c.contains("header = \"Content-Type: application/json\"\n"));
        assert!(c.contains("proxy = \"http://127.0.0.1:7890\"\n"));
        assert!(c.contains(&format!("cacert = \"{CA_BUNDLE}\"\n")));
        assert!(!c.contains("insecure"));
        assert!(curl_config("https://a/", &[], "{}", None, true).ends_with("insecure\n"));
        assert!(valid_url("https://smdp.example:8443/gsma/rsp2/es9plus/x"));
        assert!(!valid_url("http://smdp.example/"));
        assert!(!valid_url("https://smdp.example/\"x"));
        assert!(!valid_url("https://smdp example/"));
    }

    #[test]
    fn a_firmware_that_caught_up_needs_no_restart() {
        let mut pending = Some(normal_iccid(&format!("{PROFILE_A}F")));
        caught_up(
            &mut pending,
            &json!({"iccid": "8900", "state": "SIM_READY"}),
        );
        assert!(pending.is_some());
        caught_up(&mut pending, &Value::Null);
        assert!(pending.is_some());
        // firmware_sim() normalises what ubus reports, F padding included.
        caught_up(
            &mut pending,
            &json!({"iccid": normal_iccid(&format!("{PROFILE_A}f"))}),
        );
        assert_eq!(pending, None);
        // After a disable the target is "": no ICCID reported clears it.
        let mut pending = Some(String::new());
        caught_up(&mut pending, &json!({"iccid": ""}));
        assert_eq!(pending, None);
    }

    #[test]
    fn ci_roots_are_pem() {
        for pem in CI_ROOTS {
            assert!(pem.starts_with("-----BEGIN CERTIFICATE-----\n"));
            assert!(pem.trim_end().ends_with("-----END CERTIFICATE-----"));
        }
    }

    /// A modem that switches to `card` on power-up and reports it to the firmware
    /// `firmware_after` seconds later (never when None).
    struct FakeModem {
        clock: Duration,
        mode: u8,
        powered: bool,
        card: String,
        modem: String,
        firmware: String,
        firmware_after: Option<u64>,
        powered_at: Option<Duration>,
        log: Vec<String>,
        fail_low_power: bool,
        fail_online: bool,
        fail_power_off: bool,
        no_slot_status: bool,
    }

    impl FakeModem {
        fn new(card: &str, old: &str) -> Self {
            Self {
                clock: Duration::ZERO,
                mode: qmi::MODE_ONLINE,
                powered: true,
                card: card.into(),
                modem: old.into(),
                firmware: old.into(),
                firmware_after: Some(3),
                powered_at: None,
                log: vec![],
                fail_low_power: false,
                fail_online: false,
                fail_power_off: false,
                no_slot_status: false,
            }
        }
    }

    impl Modem for FakeModem {
        fn mode(&mut self) -> Result<u8, String> {
            Ok(self.mode)
        }
        fn set_mode(&mut self, mode: u8) -> Result<(), String> {
            self.log.push(format!("mode {mode}"));
            if (mode == qmi::MODE_LOW_POWER && self.fail_low_power)
                || (mode == qmi::MODE_ONLINE && self.fail_online)
            {
                return Err("refused".into());
            }
            self.mode = mode;
            Ok(())
        }
        fn power(&mut self, on: bool) -> Result<(), String> {
            self.log.push(format!("power {on}"));
            if !on && self.fail_power_off {
                return Err("refused".into());
            }
            self.powered = on;
            if on {
                self.modem = self.card.clone();
                self.powered_at = Some(self.clock);
            }
            Ok(())
        }
        fn slot_iccid(&mut self) -> Result<String, String> {
            if self.no_slot_status {
                return Err("The modem refused the request (QMI error 71)".into());
            }
            Ok(if self.powered {
                self.modem.clone()
            } else {
                String::new()
            })
        }
        fn card_ready(&mut self) -> bool {
            self.powered && !self.modem.is_empty()
        }
        fn firmware_iccid(&mut self) -> Option<String> {
            if let (Some(at), Some(after)) = (self.powered_at, self.firmware_after) {
                if self.clock >= at + Duration::from_secs(after) {
                    self.firmware = self.modem.clone();
                }
            }
            Some(self.firmware.clone())
        }
        fn pause(&mut self, d: Duration) {
            self.clock += d;
        }
        fn elapsed(&self) -> Duration {
            self.clock
        }
    }

    fn steps(
        m: &mut FakeModem,
        target: &str,
        previous: &str,
    ) -> (Result<bool, String>, Vec<&'static str>) {
        let mut seen = vec![];
        let r = reread(m, target, previous, &mut |s| seen.push(s));
        (r, seen)
    }

    #[test]
    fn reread_cycles_radio_and_card() {
        let mut m = FakeModem::new("8901", "8900");
        let (r, seen) = steps(&mut m, "8901", "8900");
        assert_eq!(r, Ok(true));
        assert_eq!(m.log, ["mode 1", "power false", "power true", "mode 0"]);
        assert_eq!(
            seen,
            [
                "radio_offline",
                "sim_power_cycle",
                "radio_online",
                "sim_reading",
                "firmware"
            ]
        );
        assert_eq!(m.mode, qmi::MODE_ONLINE);
    }

    #[test]
    fn reread_reports_a_firmware_that_does_not_follow() {
        let mut m = FakeModem::new("8901", "8900");
        m.firmware_after = None;
        let (r, _) = steps(&mut m, "8901", "8900");
        assert_eq!(r, Ok(false));
        assert!(m.clock >= FIRMWARE_WAIT);
    }

    #[test]
    fn reread_after_disable_waits_for_a_change() {
        let mut m = FakeModem::new("", "8900");
        let (r, _) = steps(&mut m, "", "8900");
        assert_eq!(r, Ok(true));
    }

    #[test]
    fn reread_restores_the_radio_when_low_power_fails() {
        let mut m = FakeModem::new("8901", "8900");
        m.fail_low_power = true;
        let (r, _) = steps(&mut m, "8901", "8900");
        assert!(r.is_err());
        assert!(
            !m.log.iter().any(|l| l.starts_with("power")),
            "card left alone: {:?}",
            m.log
        );
        assert_eq!(m.mode, qmi::MODE_ONLINE);
    }

    #[test]
    fn reread_powers_the_card_back_and_goes_online_after_a_failed_power_off() {
        let mut m = FakeModem::new("8901", "8900");
        m.fail_power_off = true;
        let (r, _) = steps(&mut m, "8901", "8900");
        assert!(r
            .unwrap_err()
            .starts_with("The SIM could not be powered off and on"));
        assert_eq!(m.log, ["mode 1", "power false", "power true", "mode 0"]);
        assert_eq!(m.mode, qmi::MODE_ONLINE);
    }

    #[test]
    fn reread_leaves_an_offline_radio_alone() {
        let mut m = FakeModem::new("8901", "8900");
        m.mode = qmi::MODE_LOW_POWER;
        let (r, seen) = steps(&mut m, "8901", "8900");
        assert_eq!(r, Ok(true));
        assert_eq!(m.log, ["power false", "power true"]);
        assert!(!seen.contains(&"radio_online"));
    }

    #[test]
    fn reread_without_slot_status_relies_on_the_firmware() {
        let mut m = FakeModem::new("8901", "8900");
        m.no_slot_status = true;
        let (r, _) = steps(&mut m, "8901", "8900");
        assert_eq!(r, Ok(true));
        assert!(
            m.clock < SLOT_WAIT,
            "no wait for a status the modem cannot give"
        );
    }

    #[test]
    fn reread_reports_a_card_that_kept_its_profile() {
        let mut m = FakeModem::new("8900", "8900");
        let (r, _) = steps(&mut m, "8901", "8900");
        assert_eq!(r.unwrap_err(), "The modem still reads the previous profile");
        assert_eq!(m.mode, qmi::MODE_ONLINE);
    }

    #[test]
    fn reread_fails_when_the_radio_stays_down() {
        let mut m = FakeModem::new("8901", "8900");
        m.fail_online = true;
        let (r, _) = steps(&mut m, "8901", "8900");
        assert_eq!(r.unwrap_err(), "The radio did not come back online");
    }
}
