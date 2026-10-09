use crate::process::BoundedCommand;
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tiny_http::{Header, Method, Request, Response, Server};

use crate::at_cmd;
use crate::cell;
use crate::clash;
use crate::clash_core;
use crate::clash_diag;
use crate::connection_logger;
use crate::device_ext;
use crate::handlers::{self, AppState};
use crate::network_ext;
use crate::proxy;
use crate::router;
use crate::screen;
use crate::signal_logger;
use crate::sim;
use crate::sms;
use crate::usb;
use crate::wifi;
use crate::wireguard;

/// How long a worker blocks before re-checking whether the listener died.
/// Also bounds how long a rebuild waits for the other workers to drain.
const WORKER_POLL: Duration = Duration::from_millis(500);
const RETRY_MIN: Duration = Duration::from_secs(1);
const RETRY_MAX: Duration = Duration::from_secs(30);

pub fn start(threads: usize, state: Arc<AppState>) {
    // Seed the CPU tracker with a baseline (speed tracker self-seeds)
    state.cpu.seed();

    // tiny_http's accept thread exits permanently on its first accept() error
    // — plausible here via EMFILE, ENOBUFS under memory pressure, or interface
    // churn. That used to leave every worker blocked forever on a listener that
    // would never yield another request, with the process still alive and
    // nothing to restart it. Supervise instead: a worker that sees the failure
    // flags it, all workers drain, and we rebuild the listener.
    let mut retry = RETRY_MIN;
    loop {
        let bind = state.binding.address();
        let generation = state.binding.generation.load(Ordering::Acquire);
        let server = match listen(&bind) {
            Ok(s) => {
                retry = RETRY_MIN;
                Arc::new(s)
            }
            Err(e) => {
                eprintln!(
                    "[server] bind {bind} failed: {e}; retrying in {}s",
                    retry.as_secs()
                );
                std::thread::sleep(retry);
                retry = (retry * 2).min(RETRY_MAX);
                continue;
            }
        };

        let dead = Arc::new(AtomicBool::new(false));
        let mut handles = Vec::new();

        for _ in 0..threads {
            let server = Arc::clone(&server);
            let state = Arc::clone(&state);
            let dead = Arc::clone(&dead);
            handles.push(std::thread::spawn(move || loop {
                if state.binding.generation.load(Ordering::Acquire) != generation {
                    dead.store(true, Ordering::Relaxed);
                }
                if dead.load(Ordering::Relaxed) {
                    return;
                }
                match server.recv_timeout(WORKER_POLL) {
                    Ok(Some(request)) => handle_request(request, &state),
                    // Idle timeout — loop round and re-check `dead`.
                    Ok(None) => {}
                    Err(e) => {
                        eprintln!("[server] listener failed: {e}");
                        dead.store(true, Ordering::Relaxed);
                        return;
                    }
                }
            }));
        }

        for h in handles {
            let _ = h.join();
        }

        eprintln!("[server] listener down, rebuilding in {}s", retry.as_secs());
        std::thread::sleep(retry);
        retry = (retry * 2).min(RETRY_MAX);
    }
}

const DESTRUCTIVE_PATHS: &[&str] = &[
    "/api/device/reboot",
    "/api/device/shutdown",
    "/api/system/kill-bloat",
];

fn cors_headers(origin: Option<&str>) -> Vec<Header> {
    let allowed = origin
        .and_then(|o| is_lan_origin(o).then_some(o))
        .unwrap_or("");
    vec![
        Header::from_bytes("Access-Control-Allow-Origin", allowed).unwrap(),
        Header::from_bytes(
            "Access-Control-Allow-Methods",
            "GET, POST, PUT, DELETE, OPTIONS",
        )
        .unwrap(),
        Header::from_bytes(
            "Access-Control-Allow-Headers",
            "Authorization, Content-Type, X-Confirm",
        )
        .unwrap(),
        Header::from_bytes("Access-Control-Max-Age", "86400").unwrap(),
    ]
}

fn is_lan_origin(origin: &str) -> bool {
    if !origin.starts_with("http://") {
        return false;
    }
    let host = &origin[7..];
    let host = host.split(':').next().unwrap_or(host);
    if host == "localhost" || host == "127.0.0.1" || host == "::1" {
        return true;
    }
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() != 4 {
        return false;
    }
    let octets: Vec<u8> = parts.iter().filter_map(|p| p.parse().ok()).collect();
    if octets.len() != 4 {
        return false;
    }
    octets[0] == 10
        || (octets[0] == 172 && octets[1] >= 16 && octets[1] <= 31)
        || (octets[0] == 192 && octets[1] == 168)
}

/// Longest pause a core upload may make between reads.
const UPLOAD_STALL: Duration = Duration::from_secs(60);

/// Accepted connections inherit keepalive from the listener, so a client that vanished
/// without closing (out of Wi-Fi range, powered off) errors out of a blocked read after
/// about a minute; tiny_http sets no timeouts. Read timeouts cannot go on the listener:
/// they would also time out tiny_http's accept loop.
fn listen(bind: &str) -> Result<Server, Box<dyn std::error::Error + Send + Sync + 'static>> {
    let listener = std::net::TcpListener::bind(bind)?;
    let fd = listener.as_raw_fd();
    for (level, option, value) in [
        (libc::SOL_SOCKET, libc::SO_KEEPALIVE, 1),
        (libc::IPPROTO_TCP, libc::TCP_KEEPIDLE, 30),
        (libc::IPPROTO_TCP, libc::TCP_KEEPINTVL, 10),
        (libc::IPPROTO_TCP, libc::TCP_KEEPCNT, 3),
    ] {
        let value: libc::c_int = value;
        // SAFETY: fd is the listener's open socket and value outlives the call.
        unsafe {
            libc::setsockopt(
                fd,
                level,
                option,
                &value as *const libc::c_int as *const libc::c_void,
                std::mem::size_of::<libc::c_int>() as libc::socklen_t,
            );
        }
    }
    Server::from_listener(listener, None)
}

/// Bounds how long reads on the connection from `peer` may stall. tiny_http hides the
/// socket, so find it among this process's descriptors by its peer address.
fn limit_read_stall(peer: std::net::SocketAddr, stall: Duration) {
    let Ok(entries) = std::fs::read_dir("/proc/self/fd") else {
        return;
    };
    for entry in entries.flatten() {
        let Some(fd) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<i32>().ok())
        else {
            continue;
        };
        let socket = std::fs::read_link(entry.path())
            .is_ok_and(|target| target.to_string_lossy().starts_with("socket:"));
        if socket && peer_of(fd) == Some(peer) {
            let timeout = libc::timeval {
                tv_sec: stall.as_secs() as libc::time_t,
                tv_usec: 0,
            };
            // SAFETY: fd is an open socket of this process and timeout outlives the call.
            unsafe {
                libc::setsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_RCVTIMEO,
                    &timeout as *const libc::timeval as *const libc::c_void,
                    std::mem::size_of::<libc::timeval>() as libc::socklen_t,
                );
            }
            return;
        }
    }
}
fn peer_of(fd: i32) -> Option<std::net::SocketAddr> {
    // SAFETY: getpeername writes at most `len` bytes into the zeroed storage.
    let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::sockaddr_storage>() as libc::socklen_t;
    let ok = unsafe {
        libc::getpeername(
            fd,
            &mut storage as *mut libc::sockaddr_storage as *mut libc::sockaddr,
            &mut len,
        )
    } == 0;
    if !ok {
        return None;
    }
    // SAFETY: ss_family says which sockaddr type the kernel wrote.
    match storage.ss_family as i32 {
        libc::AF_INET => {
            let a = unsafe { &*(&storage as *const _ as *const libc::sockaddr_in) };
            let ip = std::net::Ipv4Addr::from(u32::from_be(a.sin_addr.s_addr));
            Some((ip, u16::from_be(a.sin_port)).into())
        }
        libc::AF_INET6 => {
            let a = unsafe { &*(&storage as *const _ as *const libc::sockaddr_in6) };
            let ip = std::net::Ipv6Addr::from(a.sin6_addr.s6_addr);
            Some((ip, u16::from_be(a.sin6_port)).into())
        }
        _ => None,
    }
}

fn handle_request(mut request: Request, state: &AppState) {
    let method = request.method().clone();
    let url = request.url().to_string();
    let path = url.split('?').next().unwrap_or(&url).to_string();
    let origin = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Origin"))
        .map(|h| h.value.as_str().to_string());
    let origin_ref = origin.as_deref();
    let client_ip = request
        .remote_addr()
        .map(|a| a.ip().to_string())
        .unwrap_or_default();
    let user_agent = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("User-Agent"))
        .map(|h| h.value.as_str().to_string());
    let user_agent_ref = user_agent.as_deref();

    if method == Method::Options {
        let mut response = Response::empty(200);
        for h in cors_headers(origin_ref) {
            response = response.with_header(h);
        }
        let _ = request.respond(response);
        return;
    }

    // The LAN confirmation nonce authorises only the pending change. It lets
    // the browser prove connectivity without sending its session bearer token
    // to a new IP that might already be occupied by another host.
    let needs_auth =
        path != "/api/auth/login" && !(method == Method::Post && path == "/api/router/lan/confirm");
    if needs_auth {
        let authorized = request
            .headers()
            .iter()
            .find(|h| h.field.as_str().to_ascii_lowercase() == "authorization")
            .and_then(|h| h.value.as_str().strip_prefix("Bearer "))
            .map(|token| state.auth.validate(token))
            .unwrap_or(false);

        if !state.auth.has_password() {
            respond(
                request,
                403,
                json!({"ok": false, "error": "no password configured. Set ZTE_AGENT_PASSWORD environment variable."}),
                origin_ref,
            );
            return;
        } else if !authorized {
            respond(
                request,
                401,
                json!({"ok": false, "error": "unauthorized"}),
                origin_ref,
            );
            return;
        }
    }

    if DESTRUCTIVE_PATHS.contains(&path.as_str()) {
        let confirmed = request
            .headers()
            .iter()
            .any(|h| h.field.equiv("X-Confirm") && h.value.as_str() == "true");
        if !confirmed {
            respond(
                request,
                400,
                json!({"ok": false, "error": "destructive action requires X-Confirm: true header"}),
                origin_ref,
            );
            return;
        }
    }

    // A core upload streams to disk instead of through the 1 MiB body buffer below. It is
    // refused before its body is accepted, and a sender that stops mid-way (a phone going
    // to sleep) must not hold this worker and the core job forever.
    if let (&Method::Put, "/api/clash/core/upload") = (&method, path.as_str()) {
        let length = request.body_length().map(|n| n as u64);
        let (status, body_json) = match clash_core::upload_rejection(length) {
            Some(rejected) => rejected,
            None => {
                if let Some(peer) = request.remote_addr().copied() {
                    limit_read_stall(peer, UPLOAD_STALL);
                }
                clash_core::upload(request.as_reader(), length)
            }
        };
        respond(request, status, body_json, origin_ref);
        return;
    }

    if method == Method::Get {
        let download = match (&method, path.as_str()) {
            (&Method::Get, "/api/logger/signal/download") => {
                Some((signal_logger::LOG_PATH, "signal_log.csv"))
            }
            (&Method::Get, "/api/logger/connection/download") => {
                Some((connection_logger::LOG_PATH, "connection_log.csv"))
            }
            _ => None,
        };
        if let Some((path, name)) = download {
            match crate::logging::open_download(path) {
                Ok((file, len)) => {
                    use std::io::Read;
                    let mut response = Response::new(
                        tiny_http::StatusCode(200),
                        vec![],
                        file.take(len),
                        Some(len as usize),
                        None,
                    )
                    .with_header(
                        Header::from_bytes("Content-Type", "text/csv; charset=utf-8").unwrap(),
                    )
                    .with_header(
                        Header::from_bytes(
                            "Content-Disposition",
                            format!("attachment; filename=\"{name}\""),
                        )
                        .unwrap(),
                    )
                    .with_header(Header::from_bytes("Cache-Control", "no-store").unwrap());
                    for header in cors_headers(origin_ref) {
                        response = response.with_header(header);
                    }
                    let _ = request.respond(response);
                }
                Err(_) => respond(
                    request,
                    404,
                    json!({"ok": false, "error": "no readable log file"}),
                    origin_ref,
                ),
            }
            return;
        }
    }

    let mut body = Vec::new();
    let mut reader = request.as_reader();
    let mut limited = std::io::Read::take(&mut reader, 1024 * 1024 + 1);
    if let Err(e) = std::io::Read::read_to_end(&mut limited, &mut body) {
        respond(
            request,
            400,
            json!({"ok": false, "error": format!("failed to read body: {e}")}),
            origin_ref,
        );
        return;
    }

    if body.len() > 1024 * 1024 {
        respond(
            request,
            413,
            json!({"ok": false, "error": "request body exceeds 1 MiB"}),
            origin_ref,
        );
        return;
    }

    let (status, body_json) = route(&method, &path, state, &body, &client_ip, user_agent_ref);
    respond(request, status, body_json, origin_ref);
}

pub fn route(
    method: &Method,
    path: &str,
    state: &AppState,
    body: &[u8],
    client_ip: &str,
    user_agent: Option<&str>,
) -> (u16, Value) {
    match (method, path) {
        // Auth
        (&Method::Post, "/api/auth/login") => handlers::login(state, body, client_ip, user_agent),
        // Batch — the dashboard's heartbeat; feeds Home, Signal and Modem/Data
        (&Method::Get, "/api/dashboard") => handlers::dashboard(state),
        // Device / system
        (&Method::Get, "/api/device") => handlers::device(state),
        (&Method::Get, "/api/cpu") => handlers::cpu(state),
        (&Method::Get, "/api/memory") => handlers::memory(state),
        (&Method::Get, "/api/system/top") => handlers::system_top(state),
        (&Method::Get, "/api/screen/settings") => crate::screen_settings::get(),
        (&Method::Put, "/api/screen/settings") => crate::screen_settings::update(body),
        (&Method::Put, "/api/screen/language") => crate::screen_settings::set_language(body),
        (&Method::Get, "/api/screen") => screen::get(),
        (&Method::Post, "/api/screen") => screen::open(),
        (&Method::Delete, "/api/screen") => screen::close(),
        (&Method::Get, "/api/wireguard") => wireguard::get(),
        (&Method::Put, "/api/wireguard") => wireguard::update(body),
        (&Method::Get, "/api/proxy") => proxy::get(),
        (&Method::Put, "/api/proxy") => proxy::set(body),
        (&Method::Get, "/api/clash") => clash::get(),
        (&Method::Put, "/api/clash") => clash::update(body),
        (&Method::Post, "/api/clash/subscription/refresh") => clash::refresh(body),
        (&Method::Get, "/api/clash/proxies") => clash::proxies(),
        (&Method::Put, "/api/clash/proxies") => clash::select(body),
        (&Method::Post, "/api/clash/delay") => clash::delay(body),
        (&Method::Get, "/api/clash/connections") => clash::connections(),
        (&Method::Delete, "/api/clash/connections") => clash::close_connections(),
        (&Method::Get, "/api/clash/logs") => clash::logs(),
        (&Method::Get, "/api/clash/core") => clash_core::get(),
        (&Method::Post, "/api/clash/core") => clash_core::post(body),
        (&Method::Get, "/api/clash/diagnostics") => clash_diag::get(),
        (&Method::Post, "/api/clash/diagnostics") => clash_diag::run(),
        (&Method::Post, "/api/system/kill-bloat") => handlers::system_kill_bloat(state, body),
        (&Method::Post, "/api/system/restart-agent") => device_ext::agent_restart(state),
        (&Method::Post, "/api/device/reboot") => device_ext::device_reboot(state),
        (&Method::Post, "/api/device/shutdown") => device_ext::device_shutdown(state),
        (&Method::Get, "/api/device/battery-info") => network_ext::network_battery_ubus(state),
        (&Method::Get, "/api/device/thermal/all") => device_ext::device_thermal_all(state),
        (&Method::Get, "/api/device/battery/detail") => device_ext::device_battery_detail(state),
        (&Method::Get, "/api/device/charger") => device_ext::device_charger(state),
        (&Method::Get, "/api/device/charge-control") => device_ext::charge_control_get(state),
        (&Method::Put, "/api/device/charge-control") => device_ext::charge_control_set(state, body),
        // Network
        (&Method::Get, "/api/network/clients") => network_ext::network_clients(state),
        // WiFi
        (&Method::Get, "/api/wifi/status") => wifi::wifi_status(state),
        (&Method::Put, "/api/wifi/settings") => wifi::wifi_set(state, body),
        // Modem
        (&Method::Put, "/api/data-usage/reset-day") => {
            handlers::data_usage_reset_day_set(state, body)
        }
        (&Method::Get, "/api/modem/capabilities") => cell::modem_capabilities(state),
        (&Method::Put, "/api/modem/network-mode") => cell::modem_network_mode_set(state, body),
        // SMS
        (&Method::Get, "/api/sms/capabilities") => sms::sms_capabilities(state),
        (&Method::Post, "/api/sms/list") => sms::sms_list(state, body),
        (&Method::Post, "/api/sms/send") => sms::sms_send(state, body),
        (&Method::Post, "/api/sms/delete") => sms::sms_delete(state, body),
        (&Method::Post, "/api/sms/read") => sms::sms_mark_read(state, body),
        // SIM
        (&Method::Get, "/api/sim/info") => sim::sim_info(state),
        (&Method::Get, "/api/sim/imei") => sim::sim_imei(state),
        // Cell / band lock
        (&Method::Post, "/api/cell/lock/nr") => cell::cell_lock_nr(state, body),
        (&Method::Post, "/api/cell/lock/lte") => cell::cell_lock_lte(state, body),
        (&Method::Post, "/api/cell/lock/reset") => cell::cell_lock_reset(state),
        (&Method::Post, "/api/cell/band/nr") => cell::cell_band_nr(state, body),
        (&Method::Post, "/api/cell/band/lte") => cell::cell_band_lte(state, body),
        (&Method::Post, "/api/cell/band/reset") => cell::cell_band_reset(state),
        // Router
        (&Method::Get, "/api/router/dns") => router::router_dns_get(state),
        (&Method::Put, "/api/router/dns") => router::router_dns_set(state, body),
        (&Method::Get, "/api/router/lan") => router::router_lan_get(state),
        (&Method::Put, "/api/router/lan") => router::router_lan_set(state, body),
        (&Method::Post, "/api/router/lan/confirm") => router::router_lan_confirm(state, body),
        (&Method::Get, "/api/router/apn/mode") => router::router_apn_mode_get(state),
        (&Method::Put, "/api/router/apn/mode") => router::router_apn_mode_set(state, body),
        (&Method::Get, "/api/router/apn/profiles") => router::router_apn_profiles_get(state),
        (&Method::Post, "/api/router/apn/profiles") => router::router_apn_profiles_add(state, body),
        (&Method::Post, "/api/router/apn/profiles/delete") => {
            router::router_apn_profiles_delete(state, body)
        }
        (&Method::Post, "/api/router/apn/profiles/activate") => {
            router::router_apn_profiles_activate(state, body)
        }
        // USB
        (&Method::Get, "/api/usb/status") => usb::usb_status(state),
        (&Method::Put, "/api/usb/mode") => usb::usb_mode_set(state, body),
        (&Method::Put, "/api/usb/default") => usb::usb_default_set(state, body),
        (&Method::Put, "/api/usb/powerbank") => usb::usb_powerbank_set(state, body),
        // TTL clamping
        (&Method::Get, "/api/ttl/status") => ttl_status(),
        (&Method::Put, "/api/ttl/set") => ttl_set(body),
        (&Method::Delete, "/api/ttl/clear") => ttl_clear(),
        // AT console
        (&Method::Post, "/api/at/send") => at_console(state, body),
        (&Method::Get, "/api/at/port") => at_port(state),
        // Signal logger
        (&Method::Post, "/api/logger/signal/start") => signal_logger::start_logging(state, body),
        (&Method::Post, "/api/logger/signal/stop") => signal_logger::stop_logging(state),
        (&Method::Get, "/api/logger/signal/status") => signal_logger::status(state),
        // Connection logger
        (&Method::Post, "/api/logger/connection/start") => {
            connection_logger::start_logging(state, body)
        }
        (&Method::Post, "/api/logger/connection/stop") => connection_logger::stop_logging(state),
        (&Method::Get, "/api/logger/connection/status") => connection_logger::status(state),
        // Fallback
        _ => (404, json!({"ok": false, "error": "not found"})),
    }
}

// --- AT console ---

fn at_console(state: &AppState, body: &[u8]) -> (u16, Value) {
    let parsed: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return (400, json!({"ok": false, "error": "invalid JSON"})),
    };
    let command = match parsed["command"].as_str() {
        Some(c) if !c.is_empty() => c,
        _ => return (400, json!({"ok": false, "error": "missing 'command'"})),
    };
    let timeout = parsed["timeout"].as_u64().unwrap_or(2).min(30);

    if !is_at_command_allowed(command) {
        return (
            403,
            json!({"ok": false, "error": "command not allowed. Only read-only AT commands are permitted."}),
        );
    }

    match at_cmd::send(&state.at_port, command, timeout) {
        Ok(resp) => (200, json!({"ok": true, "data": {"response": resp.trim()}})),
        Err(e) => (503, json!({"ok": false, "error": e})),
    }
}

const AT_BLOCKED_PREFIXES: &[&str] = &[
    "AT+CFUN",
    "AT^",
    "AT$QCRMCALL",
    "AT+CLCK",
    "AT+CMGD",
    "AT+CMGF=1;+CMGS",
    "AT+CGDCONT=",
    "AT+CGACT=",
];

const AT_ALLOWED_EXACT: &[&str] = &[
    "AT",
    "ATI",
    "AT+CSQ",
    "AT+COPS?",
    "AT+COPS=?",
    "AT+CGDCONT?",
    "AT+CREG?",
    "AT+CGREG?",
    "AT+CEREG?",
    "AT+CGPADDR",
    "AT+CGACT?",
    "AT+CLAC",
    "AT+CGSN",
    "AT+CGMI",
    "AT+CGMM",
    "AT+CGMR",
    "AT+QENG=\"SERVINGCELL\"",
    "AT+QNWINFO",
    "AT+QRSRP",
    "AT+QRSRQ",
    "AT+QINISTAT",
    "AT+QSPN",
    "AT+QCIDINCOMING",
    "AT+CGCONTRDP",
];

fn is_at_command_allowed(cmd: &str) -> bool {
    let upper = cmd.trim().to_uppercase();
    if upper.is_empty() {
        return false;
    }
    for prefix in AT_BLOCKED_PREFIXES {
        if upper.starts_with(prefix) {
            return false;
        }
    }
    AT_ALLOWED_EXACT.contains(&upper.as_str())
}

/// GET /api/at/port — report the detected AT serial port (if any)
fn at_port(state: &AppState) -> (u16, Value) {
    match state.at_port.detect_serialized() {
        Some(port) => (
            200,
            json!({"ok": true, "data": {"port": port, "available": true}}),
        ),
        None => (
            200,
            json!({"ok": true, "data": {"port": null, "available": false}}),
        ),
    }
}

// --- TTL handlers ---

fn ttl_status() -> (u16, Value) {
    let ipv4 = std::process::Command::new("iptables")
        .args(["-t", "mangle", "-L", "PREROUTING", "-n"])
        .bounded_output();
    let ipv6 = std::process::Command::new("ip6tables")
        .args(["-t", "mangle", "-L", "PREROUTING", "-n"])
        .bounded_output();
    let mut active = false;
    let mut ttl_value: u32 = 0;
    if let Ok(out) = &ipv4 {
        let s = String::from_utf8_lossy(&out.stdout);
        for line in s.lines() {
            if line.contains("TTL set to") {
                active = true;
                if let Some(v) = line.rsplit("TTL set to ").next() {
                    ttl_value = v.trim().parse().unwrap_or(0);
                }
                break;
            }
        }
    }
    let mut hl_active = false;
    if let Ok(out) = &ipv6 {
        let s = String::from_utf8_lossy(&out.stdout);
        for line in s.lines() {
            if line.contains("HL set to") {
                hl_active = true;
                if ttl_value == 0 {
                    if let Some(v) = line.rsplit("HL set to ").next() {
                        ttl_value = v.trim().parse().unwrap_or(0);
                    }
                }
                break;
            }
        }
    }
    (
        200,
        json!({"ok": true, "data": {
            "active": active,
            "ipv6_active": hl_active,
            "ttl_value": ttl_value,
        }}),
    )
}

fn ttl_set(body: &[u8]) -> (u16, Value) {
    let val: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return (400, json!({"ok": false, "error": "invalid JSON"})),
    };
    let ttl = match val.get("ttl").and_then(|v| v.as_u64()) {
        Some(v) if (1..=255).contains(&v) => v as u32,
        _ => return (400, json!({"ok": false, "error": "ttl must be 1-255"})),
    };
    // Clear existing rules first
    let _ = std::process::Command::new("sh").args(["-c",
        "iptables -t mangle -S PREROUTING 2>/dev/null | grep 'TTL --ttl-set' | while read -r rule; do iptables -t mangle $(echo \"$rule\" | sed 's/-A/-D/'); done"
    ]).bounded_output();
    let _ = std::process::Command::new("sh").args(["-c",
        "ip6tables -t mangle -S PREROUTING 2>/dev/null | grep 'HL --hl-set' | while read -r rule; do ip6tables -t mangle $(echo \"$rule\" | sed 's/-A/-D/'); done"
    ]).bounded_output();
    // Add new rules
    let r4 = std::process::Command::new("iptables")
        .args([
            "-t",
            "mangle",
            "-A",
            "PREROUTING",
            "-i",
            "br-lan",
            "-j",
            "TTL",
            "--ttl-set",
            &ttl.to_string(),
        ])
        .bounded_output();
    let r6 = std::process::Command::new("ip6tables")
        .args([
            "-t",
            "mangle",
            "-A",
            "PREROUTING",
            "-i",
            "br-lan",
            "-j",
            "HL",
            "--hl-set",
            &ttl.to_string(),
        ])
        .bounded_output();
    let ok4 = r4.map(|o| o.status.success()).unwrap_or(false);
    let ok6 = r6.map(|o| o.status.success()).unwrap_or(false);
    // Persist to start_ttl.sh
    let script = format!(
        "#!/bin/sh\niptables  -t mangle -C PREROUTING -i br-lan -j TTL --ttl-set {ttl} 2>/dev/null ||   iptables  -t mangle -A PREROUTING -i br-lan -j TTL --ttl-set {ttl}\nip6tables -t mangle -C PREROUTING -i br-lan -j HL  --hl-set  {ttl} 2>/dev/null ||   ip6tables -t mangle -A PREROUTING -i br-lan -j HL  --hl-set  {ttl}\n"
    );
    let _ = std::fs::write("/data/local/tmp/start_ttl.sh", script);
    if ok4 || ok6 {
        (
            200,
            json!({"ok": true, "data": {"ttl": ttl, "ipv4": ok4, "ipv6": ok6}}),
        )
    } else {
        (
            500,
            json!({"ok": false, "error": format!("ipv4={ok4} ipv6={ok6}")}),
        )
    }
}

fn ttl_clear() -> (u16, Value) {
    let _ = std::process::Command::new("sh").args(["-c",
        "iptables -t mangle -S PREROUTING 2>/dev/null | grep 'TTL --ttl-set' | while read -r rule; do iptables -t mangle $(echo \"$rule\" | sed 's/-A/-D/'); done"
    ]).bounded_output();
    let _ = std::process::Command::new("sh").args(["-c",
        "ip6tables -t mangle -S PREROUTING 2>/dev/null | grep 'HL --hl-set' | while read -r rule; do ip6tables -t mangle $(echo \"$rule\" | sed 's/-A/-D/'); done"
    ]).bounded_output();
    // Remove persistence script content (keep file but make it a no-op)
    let _ = std::fs::write(
        "/data/local/tmp/start_ttl.sh",
        "#!/bin/sh\n# TTL disabled\n",
    );
    (200, json!({"ok": true}))
}

fn respond(request: Request, status: u16, body: Value, origin: Option<&str>) {
    let body_str = serde_json::to_string(&body).unwrap_or_default();
    let content_type = Header::from_bytes("Content-Type", "application/json").unwrap();
    let mut response = Response::from_string(body_str)
        .with_status_code(status)
        .with_header(content_type);
    for h in cors_headers(origin) {
        response = response.with_header(h);
    }
    let _ = request.respond(response);
}

#[cfg(test)]
mod tests {
    use super::is_at_command_allowed;

    #[test]
    fn a_stalled_upload_connection_times_out() {
        use std::io::Read;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (mut accepted, peer) = listener.accept().unwrap();
        assert_eq!(peer, client.local_addr().unwrap());
        super::limit_read_stall(peer, std::time::Duration::from_secs(1));
        let started = std::time::Instant::now();
        let mut buf = [0u8; 8];
        assert!(accepted.read(&mut buf).is_err(), "the read must time out");
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        drop(client);
    }

    #[test]
    fn at_allowlist_is_exact_and_read_only() {
        assert!(is_at_command_allowed("AT"));
        assert!(is_at_command_allowed("at+csq"));
        assert!(is_at_command_allowed("AT+QENG=\"servingcell\""));
        assert!(!is_at_command_allowed("AT+CSQ=1"));
        assert!(!is_at_command_allowed("AT+FOO"));
        assert!(!is_at_command_allowed("AT+CFUN=1"));
        assert!(!is_at_command_allowed("AT^RESET"));
    }
}
