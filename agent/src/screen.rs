//! Authenticated web lifecycle + root-only local screen protocol. No credentials or keys.
use crate::{clash, clash_diag, handlers::AppState, network_ext, proxy, system, wireguard};
// The protocol's I/O-free half (verbs, records, the proxies view) is a file of its own,
// declared here so that main.rs needs no change.
#[path = "screen_ipc.rs"]
mod ipc;
use ipc::{Cache, Esim, Job, Request, Tool};
use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixListener},
    panic::AssertUnwindSafe,
    path::Path,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering},
        Arc, Mutex, PoisonError,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
const DIR: &str = "/tmp/openui-screen";
const BIN: &str = "/data/bin/openui-screen";
const VERIFIED: &str = "/data/local/tmp/openui-screen-verified";
/// In tmpfs: the boot-time open happens once per boot, not again after an agent restart.
const BOOT_MARK: &str = "/tmp/openui-devui-boot";
static SUPERVISOR: AtomicI32 = AtomicI32::new(0);
static STARTING: AtomicBool = AtomicBool::new(false);
static LIFECYCLE: Mutex<()> = Mutex::new(());
/// Last request from the panel; the refreshers work only while it polls.
static INTEREST: AtomicU64 = AtomicU64::new(0);
/// Last groups or nodes query; the proxies view is read only for the node pages.
static PROXY_INTEREST: AtomicU64 = AtomicU64::new(0);
static BUSY: AtomicBool = AtomicBool::new(false);
static JOB: AtomicU64 = AtomicU64::new(0);
/// The last finished job and its result: 0 done, 1 conflict (409), 2 failed.
static RESULT: Mutex<(u64, u8)> = Mutex::new((0, 0));
/// The newest job whose outcome the status records already show, or that stopped early
/// (its failed result is the outcome).
static FRESH: AtomicU64 = AtomicU64::new(0);
/// The last failed job and the agent's message for it.
static FAILURE: Mutex<(u64, String)> = Mutex::new((0, String::new()));
static PARTS: Mutex<Parts> = Mutex::new(Parts::new());
static CLIENTS: Mutex<Vec<Value>> = Mutex::new(Vec::new());
static APP_STATE: std::sync::OnceLock<Arc<AppState>> = std::sync::OnceLock::new();
static PROXIES: Mutex<Proxies> = Mutex::new(Proxies {
    at: 0,
    pass: 0,
    cache: Cache::Loading,
});
/// Refresh passes, numbered as they begin. A pass stores what it read only if no later
/// pass has stored, so a read made before a job finished never replaces one made after
/// it: the job thread's own pass begins once the job is done, and `FRESH` follows it.
static PASS: AtomicU64 = AtomicU64::new(0);
fn begin_pass() -> u64 {
    PASS.fetch_add(1, Ordering::AcqRel) + 1
}
/// Whether `pass` may store over the one in `stored`, which it then becomes.
fn newest(stored: &mut u64, pass: u64) -> bool {
    if pass < *stored {
        return false;
    }
    *stored = pass;
    true
}
/// The proxies view for the node pages, when it was read, and by which pass.
struct Proxies {
    at: u64,
    pass: u64,
    cache: Cache,
}
/// Status records by source, each refreshed on its own: a backend switch holds the
/// WireGuard lock for minutes and must not stall the rest of the panel.
struct Parts {
    /// `S` and `P`, the WireGuard data they came from, and when and by which pass it was
    /// read.
    wireguard: String,
    devices: Vec<String>,
    wireguard_at: u64,
    wireguard_data: Value,
    wireguard_pass: u64,
    /// `O`
    system: String,
    /// `B`
    backend: String,
    /// `K` and `U`
    clash: String,
    /// `M`
    core: String,
    /// `X` and `x`
    diagnostics: String,
    /// `Q` and `q`
    esim: String,
    /// `H` (Pocket OS toolbox telemetry)
    toolbox: String,
    /// The pass that read `B`, `K`/`U` and `X`/`x`, and the one that read `M`.
    proxy_pass: u64,
    core_pass: u64,
}
impl Parts {
    const fn new() -> Self {
        Self {
            wireguard: String::new(),
            devices: Vec::new(),
            wireguard_at: 0,
            wireguard_data: Value::Null,
            wireguard_pass: 0,
            system: String::new(),
            backend: String::new(),
            clash: String::new(),
            core: String::new(),
            diagnostics: String::new(),
            esim: String::new(),
            toolbox: String::new(),
            proxy_pass: 0,
            core_pass: 0,
        }
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn owner() -> Option<i32> {
    let pid = fs::read_to_string(format!("{DIR}/owner"))
        .ok()?
        .trim()
        .parse::<i32>()
        .ok()?;
    if pid <= 1 {
        return None;
    }
    let exe = fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    (exe == Path::new(BIN)).then_some(pid)
}
pub fn running() -> bool {
    STARTING.load(Ordering::Acquire) || owner().is_some()
}
/// The firmware's power-off charging mode, in which the device is meant to stay off.
fn charging_boot(config: &str) -> bool {
    config
        .lines()
        .any(|l| l.contains("mode_main_state") && l.contains("mode_power_off"))
}
fn stock_ui_running() -> bool {
    fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| {
            fs::read_to_string(e.path().join("comm")).is_ok_and(|c| c.trim() == "zte_topsw_devui")
        })
}
/// The supervisor's own precondition: the stock daemons report sync and registration.
fn stock_synced() -> bool {
    crate::ubus::call("zwrt_topsw_daemon.sync", "get_sync_info", Some("{}")).is_ok_and(|v| {
        let text = v.to_string();
        text.contains("sync success") && text.contains("register success")
    })
}
/// Opens DevUI once per boot when the owner chose it, over the stock UI the firmware has
/// brought up: after its boot barrier, with the stock UI running and the daemons in sync,
/// never in power-off charging. If the open fails, the stock UI stays for this boot.
fn open_after_boot() {
    if Path::new(BOOT_MARK).exists() || fs::write(BOOT_MARK, b"1\n").is_err() {
        return;
    }
    let charging =
        fs::read_to_string("/etc/config/zwrt_zte_mc_tmp").is_ok_and(|c| charging_boot(&c));
    if !crate::screen_settings::current().open_at_boot || charging {
        return;
    }
    std::thread::spawn(|| {
        // Up to five minutes for the firmware to finish booting.
        for _ in 0..150 {
            let booted = fs::read_to_string("/tmp/zte_boot_done").is_ok_and(|v| v.trim() == "1");
            if booted && stock_ui_running() && stock_synced() {
                // Let the stock UI finish its first screens before the takeover.
                std::thread::sleep(Duration::from_secs(5));
                if crate::screen_settings::current().open_at_boot && !running() {
                    let _ = open();
                }
                return;
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
}
pub fn get() -> (u16, Value) {
    let running = running();
    let state = fs::read_to_string(format!("{DIR}/state")).unwrap_or_else(|_| "idle".into());
    (
        200,
        json!({"ok":true,"data":{"available":Path::new(BIN).is_file(),"verified":Path::new(VERIFIED).is_file(),"running":running,"shortcut":crate::screen_shortcut::status(),"touch":crate::screen_touch::status(),"settings_revision":crate::screen_settings::current().revision,"state":if STARTING.load(Ordering::Acquire) && owner().is_none() { "starting" } else if running || state.trim()=="error" {state.trim()} else {"idle"}}}),
    )
}
pub fn open() -> (u16, Value) {
    let _guard = LIFECYCLE.lock().unwrap();
    if running() {
        return get();
    }
    if !Path::new(BIN).is_file() || !Path::new(VERIFIED).is_file() {
        return (
            503,
            json!({"ok":false,"error":"Screen component has not passed device recovery verification"}),
        );
    }
    // A separate supervisor survives an agent restart. It owns the DRM child and restores stock UI.
    STARTING.store(true, Ordering::Release);
    match Command::new(BIN)
        .arg("--supervise")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(mut child) => {
            SUPERVISOR.store(child.id() as i32, Ordering::Release);
            std::thread::spawn(move || {
                let _ = child.wait();
                SUPERVISOR.store(0, Ordering::Release);
                STARTING.store(false, Ordering::Release);
            });
            (202, json!({"ok":true,"data":{"state":"starting"}}))
        }
        Err(_) => {
            STARTING.store(false, Ordering::Release);
            (
                500,
                json!({"ok":false,"error":"Cannot start screen supervisor"}),
            )
        }
    }
}
pub fn close() -> (u16, Value) {
    let _guard = LIFECYCLE.lock().unwrap();
    if let Some(pid) = owner().or_else(|| {
        let pid = SUPERVISOR.load(Ordering::Acquire);
        (pid > 1).then_some(pid)
    }) {
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
    } else if Path::new(BIN).is_file() && Path::new(&format!("{DIR}/ui")).is_file() {
        match Command::new(BIN)
            .arg("--restore")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(mut child) => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            Err(_) => {
                return (
                    500,
                    json!({"ok":false,"error":"Cannot start screen recovery"}),
                )
            }
        }
    }
    (202, json!({"ok":true,"data":{"state":"restoring"}}))
}
fn clean(s: &str) -> String {
    ipc::text(s, 48)
}
/// `S` and `P` records, and the `D` rows (sent while they fit).
fn projection(w: &Value, clients: &[Value]) -> (String, Vec<String>) {
    let b = |k: &str| u8::from(w[k].as_bool().unwrap_or(false));
    let n = |k: &str| w[k].as_u64().unwrap_or(0);
    let all = w["mode"] == "all";
    let macs: Vec<&str> = w["macs"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let mut out = format!(
        "S {} {} {} {} {} {} {} {} {} {}\n",
        n("revision"),
        b("has_config"),
        b("enabled"),
        b("connected"),
        u8::from(w["error"].is_string()),
        n("latest_handshake"),
        n("rx_bytes"),
        n("tx_bytes"),
        u8::from(all),
        now()
    );
    // Only profile IDs, names and selection leave the agent. No endpoints or keys.
    if let Some(profiles) = w["profiles"].as_array() {
        for p in profiles.iter().take(5) {
            if let Some(id) = p["id"].as_u64() {
                out.push_str(&format!(
                    "P\t{}\t{}\t{}\n",
                    id,
                    u8::from(w["active_profile"].as_u64() == Some(id)),
                    clean(p["name"].as_str().unwrap_or("Profile"))
                ));
            }
        }
    }
    let mut rows = std::collections::BTreeMap::<String, Value>::new();
    for c in clients {
        if let Some(m) = c["mac"].as_str() {
            rows.insert(m.to_lowercase(), c.clone());
        }
    }
    for m in &macs {
        rows.entry(m.to_string())
            .or_insert_with(|| json!({"mac":m}));
    }
    let devices = rows
        .into_iter()
        .take(128)
        .map(|(m, c)| {
            let selected = if all {
                !macs.contains(&m.as_str())
            } else {
                macs.contains(&m.as_str())
            };
            let online = c["medium"].is_string(); // Same discovery evidence as the dashboard, not a reachability probe.
            format!(
                "D\t{}\t{}\t{}\t{}\t{}\n",
                clean(&m),
                u8::from(selected),
                u8::from(online),
                clean(c["ip"].as_str().unwrap_or("-")),
                clean(c["hostname"].as_str().unwrap_or("-"))
            )
        })
        .collect();
    (out, devices)
}
/// Core updates, kept behind these two so the panel depends on nothing else of the updater.
fn core_status() -> (u16, Value) {
    crate::clash_core::get()
}
fn core_action(body: &[u8]) -> (u16, Value) {
    crate::clash_core::post(body)
}
fn store_wireguard(data: &Value, pass: u64) {
    let clients = CLIENTS.lock().unwrap().clone();
    let (head, devices) = projection(data, &clients);
    let mut p = PARTS.lock().unwrap();
    if !newest(&mut p.wireguard_pass, pass) {
        return;
    }
    p.wireguard = head;
    p.devices = devices;
    p.wireguard_at = now();
    p.wireguard_data = data.clone();
}
/// Waits for the WireGuard lock.
fn refresh_wireguard() {
    let pass = begin_pass();
    let (code, v) = wireguard::get();
    if code == 200 {
        store_wireguard(&v["data"], pass);
    }
}
/// While a switch holds the WireGuard lock, `S` keeps the last tunnel figures with the
/// saved revision and state (read without the lock) and a current stamp.
fn bridge_wireguard() {
    let at = PARTS.lock().unwrap().wireguard_at;
    if at == 0 || now().saturating_sub(at) < 6 {
        return;
    }
    let Ok(summary) = wireguard::summary() else {
        return;
    };
    let mut p = PARTS.lock().unwrap();
    if p.wireguard_at != at {
        return;
    }
    let mut data = p.wireguard_data.clone();
    for k in ["revision", "enabled", "error"] {
        data[k] = summary[k].clone();
    }
    if summary["enabled"] != true {
        data["connected"] = json!(false);
    }
    p.wireguard = projection(&data, &[]).0;
}
fn refresh_system(state: &AppState) {
    let battery = system::read_battery();
    let device = system::read_device_info();
    let memory = system::read_meminfo();
    let cpu = state.cpu.sample();
    let o_line = format!(
        "O {} {} {} {:.1} {:.1}\n",
        battery.as_ref().map_or(-1, |b| b.capacity),
        battery.as_ref().map_or(-999, |b| b.temperature),
        device.uptime_secs,
        memory.as_ref().map_or(-1.0, |m| m.usage_pct),
        cpu.overall
    );
    PARTS.lock().unwrap().system = o_line;
    if now().saturating_sub(TOOLBOX_AT.load(Ordering::Relaxed)) >= TOOLBOX_EVERY {
        refresh_toolbox(state);
    }
}
/// How often the `H` record is read: it costs a dozen firmware and system calls, so it
/// follows the panel's two-second status only every ten seconds (and right after an
/// action from a toolbox page).
const TOOLBOX_EVERY: u64 = 10;
static TOOLBOX_AT: AtomicU64 = AtomicU64::new(0);
/// `H`: signal, traffic, Wi-Fi, USB, TTL and charging for the toolbox pages.
fn refresh_toolbox(state: &AppState) {
    TOOLBOX_AT.store(now(), Ordering::Relaxed);
    let speed = state.speed.sample();
    let memory = system::read_meminfo();
    let cpu = state.cpu.sample();
    let radio_sample = crate::handlers::read_radio(&state.radio);
    let sig = radio_sample.value.as_ref().unwrap_or(&Value::Null);
    let (_, wifi_v) = crate::wifi::wifi_status(state);
    let wifi_d = &wifi_v["data"];
    let (_, usb_v) = crate::usb::usb_status(state);
    let usb_d = &usb_v["data"];
    let (_, ttl_v) = crate::server::ttl_status();
    let ttl_d = &ttl_v["data"];
    let (_, chg_v) = crate::device_ext::charge_control_get(state);
    let chg_d = &chg_v["data"];
    let (_, bat_v) = crate::device_ext::device_battery_detail(state);
    let bat_d = &bat_v["data"];
    let top = state.proc_tracker.sample();
    // The firmware reports NR fields on 5G and LTE fields on 4G; take whichever is set.
    let text = |k: &str| {
        sig.get(k)
            .and_then(|v| {
                v.as_str()
                    .map(str::to_string)
                    .or_else(|| v.as_i64().map(|n| n.to_string()))
            })
            .filter(|s| !s.trim().is_empty())
    };
    let either = |nr: &str, lte: &str| text(nr).or_else(|| text(lte));
    let num = |nr: &str, lte: &str| {
        either(nr, lte)
            .and_then(|s| s.trim().parse::<f64>().ok())
            .map_or(0, |n| n.round() as i64)
    };
    let word = |s: Option<String>| s.unwrap_or_else(|| "-".into());
    let limit = chg_d["charge_limit"].as_u64().unwrap_or(100);
    let hysteresis = chg_d["hysteresis"].as_u64().unwrap_or(5);
    let h_line = ipc::toolbox_record(&json!({
        "rssi": num("nr5g_rssi", "lte_rssi"),
        "rsrp": num("nr5g_rsrp", "lte_rsrp"),
        "rsrq": num("nr5g_rsrq", "lte_rsrq"),
        "sinr": num("nr5g_snr", "lte_snr"),
        "radio": word(text("network_type")),
        "band": word(either("nr5g_action_band", "wan_active_band")),
        "net_select": word(text("net_select")),
        "lte_bands": word(text("wan_active_band")),
        "nr_bands": word(text("nr5g_action_band")),
        "pci": word(either("nr5g_pci", "lte_pci")),
        "kbps_down": (speed.rx_speed * 8.0 / 1000.0).round() as u64,
        "kbps_up": (speed.tx_speed * 8.0 / 1000.0).round() as u64,
        "cpu_pct": cpu.overall.round() as u64,
        "mem_pct": memory.as_ref().map_or(0, |m| m.usage_pct.round() as u64),
        "temp_mc": (bat_d["temperature_c"].as_f64().unwrap_or(0.0) * 1000.0).round() as i64,
        "bloat_cnt": top.bloat_count as u64,
        "wifi_2g_on": wifi_d["radio2_disabled"].as_str() != Some("1"),
        "wifi_5g_on": wifi_d["radio5_disabled"].as_str() != Some("1"),
        "wifi_clients": wifi_d["clients_total"].as_u64().unwrap_or(0),
        "wifi_ssid": wifi_d["ssid_5g"]
            .as_str()
            .filter(|s| !s.is_empty())
            .or_else(|| wifi_d["ssid_2g"].as_str())
            .unwrap_or("-"),
        "usb_mode": usb_d["active_mode"].as_str().unwrap_or("-"),
        "usb_adb": false,
        "ttl_val": ttl_d["ttl_value"].as_u64().unwrap_or(0),
        "chg_enabled": chg_d["charging_stopped"].as_bool() != Some(true),
        "chg_limit_on": chg_d["charge_limit_enabled"].as_bool().unwrap_or(false),
        "chg_max": limit,
        "chg_stop": limit,
        "chg_start": limit.saturating_sub(hysteresis),
        "batt_mv": bat_d["voltage_mv"].as_i64().unwrap_or(0).max(0) as u64,
        "batt_ma": bat_d["current_ma"].as_i64().unwrap_or(0),
        "batt_health": bat_d["health"].as_str().unwrap_or("-"),
    }));
    PARTS.lock().unwrap().toolbox = h_line;
}
/// `B`, `K`/`U`, `X`/`x`, `Q`/`q` and `M`: quick reads that never take the WireGuard lock. The
/// periodic refresher and the job thread both run this. `M` is stored after the others:
/// the first read of a core file hashes it, which can take seconds.
fn refresh_proxy_parts() {
    let pass = begin_pass();
    let backend = ipc::backend_record(&proxy::get().1["data"]);
    let (code, v) = clash::get();
    let clash = if code == 200 {
        ipc::clash_records(&v["data"])
    } else {
        String::new()
    };
    let diagnostics = ipc::diag_records(&clash_diag::get().1["data"]);
    let esim = ipc::esim_records(&crate::esim::status());
    store_proxy_parts(pass, [backend, clash, diagnostics, esim]);
    let (code, v) = core_status();
    store_core(pass, ipc::core_record(code, &v));
}
fn store_proxy_parts(pass: u64, [backend, clash, diagnostics, esim]: [String; 4]) {
    let mut p = PARTS.lock().unwrap();
    if !newest(&mut p.proxy_pass, pass) {
        return;
    }
    p.backend = backend;
    p.clash = clash;
    p.diagnostics = diagnostics;
    p.esim = esim;
}
fn store_core(pass: u64, core: String) {
    let mut p = PARTS.lock().unwrap();
    if newest(&mut p.core_pass, pass) {
        p.core = core;
    }
}
fn proxies_wanted() -> bool {
    now().saturating_sub(PROXY_INTEREST.load(Ordering::Relaxed)) < 10
}
/// Asks the core for its groups (up to its 10 s controller timeout).
fn refresh_proxies() {
    let pass = begin_pass();
    let (code, v) = clash::proxies();
    let cache = if code == 200 {
        Cache::Ready(ipc::view(&v["data"]))
    } else {
        Cache::Unavailable
    };
    store_proxies(pass, cache);
}
fn store_proxies(pass: u64, cache: Cache) {
    let mut p = PROXIES.lock().unwrap();
    if newest(&mut p.pass, pass) {
        p.at = now();
        p.cache = cache;
    }
}
fn status() -> String {
    // The job result first: records read after it already show a finished job.
    let (done, result) = *RESULT.lock().unwrap();
    let failure = FAILURE.lock().unwrap().clone();
    let mut tail = crate::screen_settings::current().projection();
    if result != 0 && failure.0 == done {
        tail.push_str(&format!("E\t{done}\t{}\n", failure.1));
    }
    tail.push_str(&format!(
        "J {} {} {} {} {}\n.\n",
        u8::from(BUSY.load(Ordering::Acquire)),
        JOB.load(Ordering::Acquire),
        done,
        result,
        FRESH.load(Ordering::Acquire)
    ));
    // Read fresh: a saved file and the supervisor's snapshot, no lock held for long.
    let uplink = crate::wifi_uplink::summary()
        .map(|u| ipc::uplink_record(&u))
        .unwrap_or_default();
    let p = PARTS.lock().unwrap();
    ipc::status(
        &[
            &p.wireguard,
            &p.system,
            &p.backend,
            &p.clash,
            &p.core,
            &p.diagnostics,
            &p.esim,
            &uplink,
            &p.toolbox,
        ],
        &p.devices,
        &tail,
    )
}
/// A job with its handler's body; node work is resolved to names here.
enum Work {
    Wireguard(Value),
    Backend(Value),
    Clash(Value),
    Refresh(Value),
    Select(Value),
    Delay(Value),
    Core(Value),
    Diagnose,
    Esim(Value),
    Reboot,
    Uplink { revision: u64, enabled: bool },
    UplinkLogin,
    Tool(Tool),
}
fn work(job: Job) -> Result<Work, &'static str> {
    let proxies = || PROXIES.lock().unwrap();
    Ok(match job {
        Job::Wireguard(v) => Work::Wireguard(v),
        Job::Backend(v) => Work::Backend(v),
        Job::Clash(v) => Work::Clash(v),
        Job::Refresh(v) => Work::Refresh(v),
        Job::Select {
            generation,
            group,
            node,
        } => Work::Select(ipc::select_body(&proxies().cache, generation, group, node)?),
        Job::Delay { generation, group } => {
            Work::Delay(ipc::delay_body(&proxies().cache, generation, group)?)
        }
        Job::Core(v) => Work::Core(v),
        Job::Diagnose => Work::Diagnose,
        Job::Esim(Esim::Scan) => Work::Esim(json!({"action":"scan"})),
        Job::Esim(Esim::Enable { generation, index }) => {
            let view = serde_json::to_value(crate::esim::view()).unwrap_or_default();
            Work::Esim(ipc::esim_enable_body(&view, generation, index)?)
        }
        // A restart only to finish a switch the firmware has not taken up.
        Job::Esim(Esim::Reboot) if crate::esim::status()["reboot_needed"] == true => Work::Reboot,
        Job::Esim(Esim::Reboot) => return Err("No restart is needed"),
        Job::Uplink { revision, enabled } => Work::Uplink { revision, enabled },
        Job::UplinkLogin => Work::UplinkLogin,
        Job::Tool(t) => Work::Tool(t),
    })
}
fn run_tool(t: &Tool) -> (u16, Value) {
    let body = |v: &Value| serde_json::to_vec(v).unwrap_or_default();
    let Some(state) = APP_STATE.get() else {
        return (503, json!({"ok":false,"error":"Agent state not ready"}));
    };
    match t {
        Tool::StockUi => {
            let (code, v) = close();
            (if code == 202 { 200 } else { code }, v)
        }
        Tool::WifiToggle { band, enabled } => {
            let key = if band == "2g" {
                "radio2_disabled"
            } else {
                "radio5_disabled"
            };
            let val = if *enabled { "0" } else { "1" };
            crate::wifi::wifi_set(state, &body(&json!({ key: val })))
        }
        // The panel is not on USB, so the experimental NCM switch is safe to confirm here.
        Tool::UsbMode { mode } => crate::usb::usb_mode_set(
            state,
            &body(&json!({"mode": mode, "confirm_experimental": mode == "ncm"})),
        ),
        Tool::NetMode { mode } => {
            crate::cell::modem_network_mode_set(state, &body(&json!({"net_select": mode})))
        }
        Tool::Ttl { value } => {
            if *value == 0 {
                crate::server::ttl_clear()
            } else {
                crate::server::ttl_set(&body(&json!({"ttl": value})))
            }
        }
        Tool::ChargeSwitch { enabled } => crate::device_ext::charge_control_set(
            state,
            &body(&json!({"charging_stopped": !enabled})),
        ),
        Tool::ChargeLimit { max_level } => {
            let update = if *max_level >= 100 {
                json!({"charge_limit_enabled": false})
            } else {
                json!({"charge_limit_enabled": true, "charge_limit": max_level, "hysteresis": 10})
            };
            crate::device_ext::charge_control_set(state, &body(&update))
        }
        Tool::KillBloat => crate::handlers::system_kill_bloat(state, b"{\"all\":true}"),
        Tool::DropCaches => crate::handlers::system_drop_caches(),
    }
}
fn run(work: &Work) -> (u16, Value) {
    let body = |v: &Value| serde_json::to_vec(v).unwrap_or_default();
    match work {
        Work::Wireguard(v) => wireguard::update(&body(v)),
        // The switch turns the other backend off in the same transaction.
        Work::Backend(v) => proxy::set(&body(v)),
        Work::Clash(v) => clash::update(&body(v)),
        Work::Refresh(v) => clash::refresh(&body(v)),
        Work::Select(v) => clash::select(&body(v)),
        Work::Delay(v) => clash::delay(&body(v)),
        // These two start background work and return; their records show progress.
        Work::Core(v) => core_action(&body(v)),
        Work::Diagnose => clash_diag::run(),
        Work::Esim(v) => crate::esim::post(&body(v)),
        Work::Reboot => crate::device_ext::reboot(),
        // Saves at once; the uplink's own supervisor joins or leaves the network.
        Work::Uplink { revision, enabled } => {
            match crate::wifi_uplink::set_enabled(*revision, *enabled) {
                Ok(()) => (200, json!({"ok":true})),
                Err(e) if e.starts_with("Configuration changed;") => {
                    (409, json!({"ok":false,"error":e}))
                }
                Err(e) => (400, json!({"ok":false,"error":e})),
            }
        }
        Work::UplinkLogin => match crate::wifi_uplink::set_login(true) {
            Ok(()) => (200, json!({"ok":true})),
            Err(e) => (400, json!({"ok":false,"error":e})),
        },
        Work::Tool(t) => run_tool(t),
    }
}
/// Ends a job on every path, a panic included: publishes its result (failed unless the
/// job thread got to the end) and marks it fresh, then clears the busy state. The panel
/// waits for both, so without them it would wait on the job until it gives up.
struct JobEnd {
    job: u64,
    result: u8,
    message: String,
}
impl JobEnd {
    fn new(job: u64) -> Self {
        Self {
            job,
            result: 2,
            message: "Screen job stopped".into(),
        }
    }
}
impl Drop for JobEnd {
    fn drop(&mut self) {
        // Also runs while unwinding, where a second panic would abort the agent.
        if self.result != 0 {
            *FAILURE.lock().unwrap_or_else(PoisonError::into_inner) =
                (self.job, std::mem::take(&mut self.message));
        }
        // `status` reads the result before `FRESH` and the failure: a result it shows
        // comes with both.
        FRESH.store(self.job, Ordering::Release);
        *RESULT.lock().unwrap_or_else(PoisonError::into_inner) = (self.job, self.result);
        BUSY.store(false, Ordering::Release);
    }
}
fn start_job(job: Job) -> String {
    let work = match work(job) {
        Ok(w) => w,
        Err(e) => return format!("ERR {e}\n"),
    };
    if BUSY
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return "ERR Busy\n".into();
    }
    let job = JOB.fetch_add(1, Ordering::AcqRel) + 1;
    let spawned = std::thread::Builder::new().spawn(move || {
        let mut end = JobEnd::new(job);
        let (code, reply) = std::panic::catch_unwind(AssertUnwindSafe(|| run(&work)))
            .unwrap_or_else(|_| (500, json!({"ok":false,"error":"Screen job stopped"})));
        // Refresh what the job changed, so the next status already shows its outcome.
        match &work {
            Work::Wireguard(_) if code == 200 => store_wireguard(&reply["data"], begin_pass()),
            Work::Wireguard(_) | Work::Backend(_) => refresh_wireguard(),
            Work::Tool(_) => {
                if let Some(state) = APP_STATE.get() {
                    refresh_toolbox(state);
                }
            }
            _ => {}
        }
        if matches!(work, Work::Select(_) | Work::Delay(_)) || proxies_wanted() {
            refresh_proxies();
        }
        refresh_proxy_parts();
        end.result = match code {
            200 => 0,
            409 => 1,
            _ => 2,
        };
        end.message = ipc::text_bytes(reply["error"].as_str().unwrap_or(""), 96);
    });
    if spawned.is_err() {
        // The job never ran; the panel shows the refusal.
        BUSY.store(false, Ordering::Release);
        return "ERR Cannot start the job\n".into();
    }
    format!("OK {job}\n")
}
fn request(line: &str) -> String {
    INTEREST.store(now(), Ordering::Relaxed);
    match ipc::parse(line) {
        Ok(Request::Status) => status(),
        Ok(Request::Groups { offset }) => {
            PROXY_INTEREST.store(now(), Ordering::Relaxed);
            ipc::groups_page(&PROXIES.lock().unwrap().cache, offset)
        }
        Ok(Request::Nodes {
            generation,
            group,
            offset,
        }) => {
            PROXY_INTEREST.store(now(), Ordering::Relaxed);
            ipc::nodes_page(&PROXIES.lock().unwrap().cache, generation, group, offset)
                .unwrap_or_else(|e| format!("ERR {e}\n"))
        }
        Ok(Request::Job(job)) => start_job(job),
        Err(e) => format!("ERR {e}\n"),
    }
}
pub fn start(state: Arc<AppState>) {
    let _ = APP_STATE.set(state.clone());
    if fs::create_dir_all(DIR).is_err()
        || fs::set_permissions(DIR, fs::Permissions::from_mode(0o700)).is_err()
    {
        return;
    }
    // Only one agent instance should own the listener; reject a live existing endpoint.
    let path = format!("{DIR}/control.sock");
    if std::os::unix::net::UnixStream::connect(&path).is_ok() {
        return;
    }
    let _ = fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(v) => v,
        Err(_) => return,
    };
    if fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).is_err() {
        return;
    }
    crate::screen_shortcut::start();
    crate::screen_touch::start();
    open_after_boot();
    let polled = || now().saturating_sub(INTEREST.load(Ordering::Relaxed)) < 10;
    let clients_state = state.clone();
    // WireGuard and devices: `wireguard::get` waits while a switch holds its lock.
    std::thread::spawn(move || {
        let mut refreshed = 0;
        loop {
            if polled() {
                if now().saturating_sub(refreshed) >= 10 {
                    let (_, v) = network_ext::network_clients(&clients_state);
                    *CLIENTS.lock().unwrap() =
                        v["data"]["clients"].as_array().cloned().unwrap_or_default();
                    refreshed = now();
                }
                refresh_wireguard();
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
    // Everything else, so a held WireGuard lock never makes the panel's status stale.
    std::thread::spawn(move || loop {
        if polled() {
            refresh_system(&state);
            refresh_proxy_parts();
            bridge_wireguard();
        }
        std::thread::sleep(Duration::from_secs(2));
    });
    // Clash groups, only while the panel shows the node pages.
    std::thread::spawn(|| loop {
        if proxies_wanted() && now().saturating_sub(PROXIES.lock().unwrap().at) >= 3 {
            refresh_proxies();
        }
        std::thread::sleep(Duration::from_secs(1));
    });
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
            let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
            let mut line = String::new();
            if BufReader::new((&stream).take(4097))
                .read_line(&mut line)
                .is_ok()
                && line.len() <= 4096
                && line.ends_with('\n')
            {
                let _ = stream.write_all(request(&line).as_bytes());
            }
        }
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn boot_open_skips_power_off_charging() {
        assert!(charging_boot(
            "config zwrt_zte_mc_tmp 'state'\n\toption mode_main_state 'mode_power_off_charger'\n"
        ));
        assert!(!charging_boot(
            "config zwrt_zte_mc_tmp 'state'\n\toption mode_main_state 'mode_power_on'\n"
        ));
        assert!(!charging_boot(
            "\toption mode_main_state 'mode_power_on_charger'\n\toption last 'mode_power_off'\n"
        ));
        assert!(!charging_boot(""));
    }
    fn joined(v: &Value) -> String {
        let (head, devices) = projection(v, &[]);
        head + &devices.concat()
    }
    #[test]
    fn profile_projection_omits_credentials_and_escapes_protocol() {
        let v = json!({"active_profile":2,"profiles":[
            {"id":1,"name":"Home\nVPN","endpoint":"private.example","private_key":"secret"},
            {"id":2,"name":"Travel\tVPN","preshared_key":"secret"}]});
        let s = joined(&v);
        assert!(s.contains("P\t1\t0\tHomeVPN\n"));
        assert!(s.contains("P\t2\t1\tTravelVPN\n"));
        assert!(!s.contains("secret") && !s.contains("private.example"));
    }
    #[test]
    fn snapshot_redacts_and_keeps_offline_choices() {
        let v = json!({"revision":3,"mode":"all","macs":["02:11:22:33:44:55"],"private_key":"secret","enabled":false});
        let s = joined(&v);
        assert!(!s.contains("secret"));
        assert!(s.contains("D\t02:11:22:33:44:55\t0\t0\t-\t-"));
        assert_eq!(clean("a\nb\t\u{1b}"), "ab");
    }
    #[test]
    fn a_refresh_never_replaces_one_that_began_later() {
        // The only test that stores records. A periodic pass reads before a job is done
        // and stores after the job thread's pass, which began after the job: the job's
        // outcome must stay, since FRESH already says the records show it.
        let (periodic, job) = (begin_pass(), begin_pass());
        let parts = |b: &str| {
            [
                format!("B\t{b}\n"),
                String::new(),
                String::new(),
                String::new(),
            ]
        };
        store_proxy_parts(job, parts("after"));
        store_proxy_parts(periodic, parts("before"));
        store_core(job, "M\tafter\n".into());
        store_core(periodic, "M\tbefore\n".into());
        store_wireguard(&json!({"revision": 9}), job);
        store_wireguard(&json!({"revision": 8}), periodic);
        store_proxies(job, Cache::Unavailable);
        store_proxies(periodic, Cache::Loading);
        let s = status();
        assert!(s.starts_with("S 9 ") && s.contains("B\tafter\n"), "{s}");
        assert!(s.contains("M\tafter\n") && !s.contains("B\tbefore"), "{s}");
        assert!(!s.contains("M\tbefore"), "{s}");
        assert_eq!(PROXIES.lock().unwrap().cache, Cache::Unavailable);
        // A pass that began later stores again.
        store_proxy_parts(begin_pass(), parts("later"));
        store_wireguard(&json!({"revision": 10}), begin_pass());
        let s = status();
        assert!(s.starts_with("S 10 ") && s.contains("B\tlater\n"), "{s}");
    }
    #[test]
    fn a_job_thread_that_panics_still_ends_its_job() {
        // The only test that runs or ends jobs, so the job statics are its own.
        BUSY.store(true, Ordering::Release);
        let job = JOB.fetch_add(1, Ordering::AcqRel) + 1;
        let stopped = std::panic::catch_unwind(|| {
            let _end = JobEnd::new(job);
            panic!("a refresh after the job panicked");
        });
        assert!(stopped.is_err());
        assert_eq!(*RESULT.lock().unwrap(), (job, 2));
        assert_eq!(FRESH.load(Ordering::Acquire), job);
        assert!(!BUSY.load(Ordering::Acquire));
        let s = status();
        assert!(
            s.contains(&format!("E\t{job}\tScreen job stopped\n")),
            "{s}"
        );
        assert!(s.ends_with(&format!("J 0 {job} {job} 2 {job}\n.\n")), "{s}");
        // A job that got to the end publishes its own result and no failure.
        BUSY.store(true, Ordering::Release);
        let job = JOB.fetch_add(1, Ordering::AcqRel) + 1;
        let mut end = JobEnd::new(job);
        end.result = 0;
        drop(end);
        assert_eq!(*RESULT.lock().unwrap(), (job, 0));
        assert!(status().ends_with(&format!("J 0 {job} {job} 0 {job}\n.\n")));
        assert!(!BUSY.load(Ordering::Acquire));
    }
    #[test]
    fn device_rows_are_separate_from_the_status_head() {
        let clients: Vec<Value> = (0..200)
            .map(|i| json!({"mac":format!("02:00:00:00:{:02x}:{:02x}", i / 256, i % 256),"medium":"wifi","hostname":"名".repeat(60)}))
            .collect();
        let (head, devices) = projection(&json!({"revision":1,"mode":"selected"}), &clients);
        assert!(head.starts_with("S 1 ") && !head.contains("\nD\t"));
        assert_eq!(devices.len(), 128, "at most 128 rows are projected");
        assert!(devices[0].ends_with(&format!("{}\n", "名".repeat(48))));
    }
}
