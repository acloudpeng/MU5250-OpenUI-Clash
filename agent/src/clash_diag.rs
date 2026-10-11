//! One-tap Clash diagnosis: the checks of scripts/device/diagnose-clash.sh as structured
//! results, so the page can tell "the nodes fail" from "a device's traffic never reaches
//! the core". Runs on its own thread because the HTTP server has few workers; the page
//! polls the stored result.
use crate::{clash, clash_ctl, network_ext};
use serde_json::{json, Value};
use std::{
    fs,
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

static RUNNING: AtomicBool = AtomicBool::new(false);
static LAST: Mutex<Option<Value>> = Mutex::new(None);
const CURL: &str = "/usr/bin/curl";
const DOMESTIC: &str = "https://www.baidu.com";
const FOREIGN: &str = "https://www.google.com/generate_204";
/// Clock skew that time-checked protocols still accept (VMess allows 120 s).
const SKEW_LIMIT: i64 = 120;
/// B31 keeps Beijing time in the system clock as if it were UTC; with the profile's NTP
/// block the core corrects its own clock, so that offset is expected (see clash_config).
const BEIJING_OFFSET: i64 = 8 * 3600;
/// Firewall hooks of the Clash egress policy: table, stock chain, owned chain suffix.
const HOOKS: [(&str, &str, &str); 4] = [
    ("mangle", "PREROUTING", "MARK"),
    ("nat", "PREROUTING", "DNS"),
    ("filter", "FORWARD", "FWD"),
    ("filter", "INPUT", "DNS4"),
];

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
/// Standard output of a command with a deadline; empty when it cannot run.
fn output(program: &str, args: &[&str], secs: u64) -> String {
    process_runner::output(
        Command::new(program).args(args),
        None,
        Duration::from_secs(secs),
        256 * 1024,
    )
    .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
    .unwrap_or_default()
}
fn check(id: &str, status: &str, data: Value, detail: String) -> Value {
    json!({"id": id, "status": status, "data": data, "detail": detail})
}
fn skipped(id: &str) -> Value {
    check(id, "skip", json!({}), String::new())
}

/// Seconds since the epoch for an HTTP date such as "Fri, 09 Oct 2026 18:23:59 GMT".
fn http_date(text: &str) -> Option<i64> {
    let parts: Vec<&str> = text.split_whitespace().collect();
    let [_, day, month, year, time, ..] = parts.as_slice() else {
        return None;
    };
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let month = months.iter().position(|m| m == month)? as i64 + 1;
    let (d, y): (i64, i64) = (day.parse().ok()?, year.parse().ok()?);
    let hms: Vec<i64> = time
        .split(':')
        .map(|v| v.parse().ok())
        .collect::<Option<_>>()?;
    let [h, m, s] = hms.as_slice() else {
        return None;
    };
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if month <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * ((month + 9) % 12) + 2) / 5 + d - 1;
    let days = era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468;
    Some(days * 86400 + h * 3600 + m * 60 + s)
}

fn clock() -> Value {
    // A domestic site's Date header, fetched directly rather than through the core.
    let headers = output(CURL, &["-sI", "--max-time", "6", "http://www.baidu.com"], 8);
    let skew = headers
        .lines()
        .find_map(|l| {
            l.split_once(':')
                .filter(|(k, _)| k.eq_ignore_ascii_case("date"))
        })
        .and_then(|(_, v)| http_date(v))
        .map(|server| now() as i64 - server);
    let ntp =
        fs::read_to_string(clash::PROFILE).is_ok_and(|p| p.lines().any(|l| l.starts_with("ntp:")));
    let beijing = ntp && skew.is_some_and(|s| (s - BEIJING_OFFSET).abs() <= SKEW_LIMIT);
    let status = match skew {
        None => "skip",
        Some(s) if s.abs() <= SKEW_LIMIT || beijing => "pass",
        Some(_) => "warn",
    };
    check(
        "clock",
        status,
        json!({"skew_s": skew, "ntp": ntp, "beijing": beijing}),
        String::new(),
    )
}
fn core() -> (bool, Value) {
    // The controller answering is what the other checks need; supervision is reported.
    let version = clash::ctl("GET", "/version", None, 3)
        .ok()
        .and_then(|v| v["version"].as_str().map(String::from));
    let up = version.is_some();
    let status = if up { "pass" } else { "fail" };
    let data = json!({"running": up, "supervised": clash::core_running(), "version": version});
    (up, check("core", status, data, String::new()))
}
fn resolves(name: &str) -> bool {
    clash::ctl("GET", &format!("/dns/query?name={name}&type=A"), None, 6)
        .is_ok_and(|v| v["Status"] == 0 && v["Answer"].as_array().is_some_and(|a| !a.is_empty()))
}
fn dns() -> Value {
    let (domestic, foreign) = (resolves("www.baidu.com"), resolves("www.google.com"));
    // With fake-IP, proxied names never need a local answer; direct ones do.
    let status = if domestic { "pass" } else { "fail" };
    let data = json!({"domestic": domestic, "foreign": foreign});
    check("dns", status, data, String::new())
}
fn delay(kind: &str, name: &str, url: &str) -> Option<Value> {
    let path = format!(
        "/{kind}/{}/delay?url={}&timeout=5000",
        clash_ctl::segment(name),
        clash_ctl::segment(url)
    );
    clash::ctl("GET", &path, None, 12).ok()
}
/// The first selector in profile order (GLOBAL lists them that way) and the node it
/// finally resolves to through nested groups.
fn main_choice(all: &Value) -> (String, String) {
    let group = all["GLOBAL"]["all"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .find(|g| all[*g]["type"] == "Selector")
        .unwrap_or("GLOBAL")
        .to_string();
    let mut node = group.clone();
    for _ in 0..6 {
        match all[node.as_str()]["now"].as_str() {
            Some(next) if !next.is_empty() => node = next.into(),
            _ => break,
        }
    }
    (group, node)
}
fn nodes() -> Value {
    let Ok(raw) = clash::ctl("GET", "/proxies", None, 10) else {
        return skipped("nodes");
    };
    let all = &raw["proxies"];
    let (group, node) = main_choice(all);
    let ms = |url: &str| delay("proxies", &node, url).and_then(|v| v["delay"].as_u64());
    let https = ms(clash::DELAY_URL);
    let http = ms(&clash::DELAY_URL.replacen("https://", "http://", 1));
    let answered = delay("group", &group, clash::DELAY_URL)
        .as_ref()
        .and_then(Value::as_object)
        .map_or(0, |m| m.len());
    let total = all[group.as_str()]["all"].as_array().map_or(0, Vec::len);
    let status = if https.is_some() {
        "pass"
    } else if http.is_some() {
        "warn"
    } else {
        "fail"
    };
    let data = json!({"group": group, "node": node, "https_ms": https, "http_ms": http,
        "answered": answered, "total": total});
    check("nodes", status, data, String::new())
}
fn fetch_code(url: &str) -> Option<u16> {
    let args = [
        "-sS",
        "-o",
        "/dev/null",
        "-w",
        "%{http_code}",
        "--max-time",
        "10",
        "-x",
        clash::PROXY,
        url,
    ];
    output(CURL, &args, 12)
        .trim()
        .parse()
        .ok()
        .filter(|c| *c > 0)
}
fn fetch() -> Value {
    let (domestic, foreign) = (fetch_code(DOMESTIC), fetch_code(FOREIGN));
    let status = if foreign.is_some() { "pass" } else { "fail" };
    let data = json!({"domestic": domestic, "foreign": foreign});
    check("fetch", status, data, String::new())
}
/// Which LAN clients the saved selection proxies and whether the core sees them, plus
/// the address the route probe should use.
fn devices(
    mode: &str,
    macs: &[String],
    clients: &[Value],
    sources: &[String],
) -> (Option<String>, Value) {
    let list: Vec<Value> = clients
        .iter()
        .filter_map(|c| {
            let ip = c["ip"].as_str()?;
            let mac = c["mac"].as_str().unwrap_or("").to_lowercase();
            let listed = macs.contains(&mac);
            Some(json!({"ip": ip, "mac": mac, "name": c["hostname"], "interface": c["interface"],
                "proxied": (mode == "selected") == listed, "active": sources.iter().any(|s| s == ip)}))
        })
        .take(16)
        .collect();
    let proxied: Vec<&Value> = list.iter().filter(|c| c["proxied"] == true).collect();
    let probe = proxied
        .first()
        .copied()
        .or(list.first())
        .and_then(|c| c["ip"].as_str())
        .map(String::from);
    let status = if proxied.is_empty() { "warn" } else { "pass" };
    let data =
        json!({"mode": mode, "proxied": proxied.len(), "total": list.len(), "clients": list});
    (probe, check("devices", status, data, String::new()))
}
fn route(client: &str) -> Value {
    let e = &clash::EGRESS;
    let probe = |dst: &str| {
        let args = [
            "-4", "route", "get", dst, "from", client, "iif", "br-lan", "mark", e.mark,
        ];
        output("ip", &args, 5)
            .lines()
            .next()
            .unwrap_or("")
            .to_string()
    };
    let (internet, dns) = (probe("1.1.1.1"), probe(clash::DNS));
    let via = |r: &str| r.contains(&format!("dev {}", e.iface));
    let (tun, dns_tun) = (via(&internet), via(&dns));
    let detail = format!(
        "{internet}\n{dns}\n{}",
        output("ip", &["-4", "rule", "show"], 5).trim_end()
    );
    let status = if tun && dns_tun { "pass" } else { "fail" };
    let data = json!({"client": client, "tun": tun, "dns_tun": dns_tun});
    check("route", status, data, detail)
}
/// Where `-S <chain>` output jumps to the owned chain: `None` when missing, `Some(false)`
/// when a rule that is not one of ours comes first.
fn hook_first(rules: &str, prefix: &str, own: &str) -> Option<bool> {
    let jumps: Vec<&str> = rules.lines().filter(|l| l.starts_with("-A ")).collect();
    let target = format!("-j {prefix}_{own}");
    let at = jumps.iter().position(|l| l.ends_with(&target))?;
    let ours = format!("-j {prefix}_");
    Some(jumps[..at].iter().all(|l| l.contains(&ours)))
}
/// Packets the kill-switch rejected: marked traffic that was not routed into the TUN.
fn rejected(counters: &str) -> u64 {
    counters
        .lines()
        .filter(|l| l.contains("REJECT"))
        .filter_map(|l| l.split_whitespace().next()?.parse::<u64>().ok())
        .sum()
}
fn firewall() -> Value {
    let p = clash::EGRESS.prefix;
    let (mut missing, mut displaced) = (vec![], vec![]);
    for (table, chain, own) in HOOKS {
        let rules = output("iptables", &["-w", "2", "-t", table, "-S", chain], 5);
        match hook_first(&rules, p, own) {
            None => missing.push(format!("{p}_{own}")),
            Some(false) => displaced.push(format!("{p}_{own}")),
            Some(true) => {}
        }
    }
    let forward = output("iptables", &["-w", "2", "-S", "FORWARD"], 5);
    let held = forward.contains(&format!("-j {p}_HOLD"));
    let chain = format!("{p}_FWD");
    let rejected = rejected(&output(
        "iptables",
        &["-w", "2", "-L", &chain, "-v", "-n", "-x"],
        5,
    ));
    // IPA hardware acceleration may forward LAN flows without netfilter seeing them.
    let ipa_off = output(
        "uci",
        &["-q", "get", "zwrt_router.tmp_router.close_ipa_acce"],
        5,
    );
    let rp_filter = fs::read_to_string(format!(
        "/proc/sys/net/ipv4/conf/{}/rp_filter",
        clash::IFACE
    ))
    .ok()
    .and_then(|v| v.trim().parse::<u8>().ok());
    let status = if held || !missing.is_empty() {
        "fail"
    } else if !displaced.is_empty() || rejected > 0 {
        "warn"
    } else {
        "pass"
    };
    let data = json!({"missing": missing, "displaced": displaced, "held": held, "rejected": rejected,
        "ipa_acceleration": (!ipa_off.trim().is_empty()).then(|| ipa_off.trim() != "1"), "rp_filter": rp_filter});
    check("firewall", status, data, String::new())
}
fn verdict(enabled: bool, checks: &[Value]) -> &'static str {
    let failed = |id: &str, status: &str| {
        checks
            .iter()
            .any(|c| c["id"] == id && c["status"] == status)
    };
    if !enabled {
        "clash_off"
    } else if failed("core", "fail") {
        "core_down"
    } else if failed("route", "fail") || failed("firewall", "fail") {
        "not_routed"
    } else if failed("nodes", "fail") || failed("fetch", "fail") {
        "nodes"
    } else if failed("devices", "warn") {
        "no_devices"
    } else {
        "ok"
    }
}
fn diagnose() -> Value {
    let started = (now(), Instant::now());
    let (enabled, mode, macs) = clash::stored_routing().unwrap_or((false, "all".into(), vec![]));
    let (up, core) = core();
    let sources: Vec<String> = up
        .then(|| clash::ctl("GET", "/connections", None, 5).ok())
        .flatten()
        .and_then(|v| {
            v["connections"].as_array().map(|a| {
                a.iter()
                    .filter_map(|c| c["metadata"]["sourceIP"].as_str().map(String::from))
                    .collect()
            })
        })
        .unwrap_or_default();
    let (probe, devices) = devices(&mode, &macs, &network_ext::clients(), &sources);
    let mut checks = std::thread::scope(|s| {
        let clock = s.spawn(clock);
        let dns = s.spawn(|| if up { dns() } else { skipped("dns") });
        let nodes = s.spawn(|| if up { nodes() } else { skipped("nodes") });
        let fetch = s.spawn(|| if up { fetch() } else { skipped("fetch") });
        let mut checks = vec![clock.join().unwrap_or_else(|_| skipped("clock")), core];
        for (id, handle) in [("dns", dns), ("nodes", nodes), ("fetch", fetch)] {
            checks.push(handle.join().unwrap_or_else(|_| skipped(id)));
        }
        checks
    });
    checks.push(devices);
    checks.push(match probe {
        Some(client) if enabled => route(&client),
        _ => skipped("route"),
    });
    checks.push(if enabled {
        firewall()
    } else {
        skipped("firewall")
    });
    json!({"started_at": started.0, "finished_at": now(),
        "duration_ms": started.1.elapsed().as_millis() as u64,
        "verdict": verdict(enabled, &checks), "checks": checks})
}

pub fn get() -> (u16, Value) {
    let mut data = LAST.lock().unwrap().clone().unwrap_or_else(
        || json!({"started_at": null, "finished_at": null, "verdict": null, "checks": []}),
    );
    data["running"] = json!(RUNNING.load(Ordering::Acquire));
    (200, json!({"ok": true, "data": data}))
}
/// Starts a run unless one is in progress; either way returns the current state.
pub fn run() -> (u16, Value) {
    if !RUNNING.swap(true, Ordering::AcqRel) {
        struct Done;
        impl Drop for Done {
            fn drop(&mut self) {
                RUNNING.store(false, Ordering::Release);
            }
        }
        std::thread::spawn(|| {
            let _done = Done;
            let report = diagnose();
            *LAST.lock().unwrap() = Some(report);
        });
    }
    get()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn http_dates_convert_to_epoch_seconds() {
        assert_eq!(http_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0));
        assert_eq!(
            http_date(" Fri, 09 Oct 2026 18:25:01 GMT"),
            Some(1791570301)
        );
        assert_eq!(http_date("Tue, 29 Feb 2028 23:59:59 GMT"), Some(1835481599));
        for bad in [
            "",
            "Fri, 09 Foo 2026 18:25:01 GMT",
            "Fri, 09 Oct 2026 18:25 GMT",
        ] {
            assert_eq!(http_date(bad), None, "{bad}");
        }
    }
    #[test]
    fn device_selection_matches_the_policy() {
        let clients = [
            json!({"mac": "AA:BB:CC:DD:EE:01", "ip": "192.168.0.10", "hostname": "pc", "interface": "rndis0"}),
            json!({"mac": "aa:bb:cc:dd:ee:02", "ip": "192.168.0.11", "hostname": null, "interface": "wlan0"}),
            json!({"mac": "aa:bb:cc:dd:ee:03", "ip": null}),
        ];
        let excluded = vec!["aa:bb:cc:dd:ee:01".to_string()];
        let sources = vec!["192.168.0.11".to_string()];
        let (probe, c) = devices("all", &excluded, &clients, &sources);
        assert_eq!(probe.as_deref(), Some("192.168.0.11"));
        assert_eq!(c["status"], "pass");
        assert_eq!(c["data"]["proxied"], 1);
        assert_eq!(c["data"]["clients"][0]["proxied"], false);
        assert_eq!(c["data"]["clients"][1]["active"], true);
        let (probe, c) = devices("selected", &[], &clients, &sources);
        assert_eq!(
            c["status"], "warn",
            "selected mode with nothing chosen proxies nobody"
        );
        assert_eq!(probe.as_deref(), Some("192.168.0.10"));
    }
    #[test]
    fn hooks_must_come_first_except_for_our_guards() {
        let first = "-P FORWARD DROP\n-A FORWARD -j OUCL_HOLD\n-A FORWARD -j OUCL_FWD\n-A FORWARD -j forwarding_rule\n";
        assert_eq!(hook_first(first, "OUCL", "FWD"), Some(true));
        let pushed = "-P FORWARD DROP\n-A FORWARD -j QCMAP_FWD\n-A FORWARD -j OUCL_FWD\n";
        assert_eq!(hook_first(pushed, "OUCL", "FWD"), Some(false));
        assert_eq!(hook_first("-A INPUT -j OUCL_DNS4\n", "OUCL", "DNS"), None);
        let counters = "Chain OUCL_FWD (1 references)\n    pkts      bytes target     prot opt in     out     source               destination\n       5      300 ACCEPT     all  --  ou-clash0 br-lan  0.0.0.0/0            0.0.0.0/0            ctstate RELATED,ESTABLISHED\n      42     2520 REJECT     all  --  br-lan !br-lan  0.0.0.0/0            0.0.0.0/0            mark match 0x20000000/0x20000000 reject-with icmp-port-unreachable\n";
        assert_eq!(rejected(counters), 42);
    }
    #[test]
    fn the_verdict_names_the_first_broken_layer() {
        let c = |id: &str, status: &str| json!({"id": id, "status": status});
        assert_eq!(verdict(false, &[]), "clash_off");
        assert_eq!(
            verdict(true, &[c("core", "fail"), c("route", "fail")]),
            "core_down"
        );
        assert_eq!(
            verdict(true, &[c("route", "fail"), c("nodes", "fail")]),
            "not_routed"
        );
        assert_eq!(
            verdict(true, &[c("route", "pass"), c("nodes", "fail")]),
            "nodes"
        );
        assert_eq!(
            verdict(true, &[c("devices", "warn"), c("nodes", "pass")]),
            "no_devices"
        );
        assert_eq!(
            verdict(true, &[c("nodes", "warn"), c("fetch", "pass")]),
            "ok"
        );
    }
    #[test]
    fn live_diagnosis() {
        // `OPENUI_CLASH_TEST_DIAGNOSE=1 cargo test live_diagnosis -- --nocapture` runs every
        // check against a core listening on the agent's controller socket.
        if std::env::var("OPENUI_CLASH_TEST_DIAGNOSE").is_ok() {
            let report = diagnose();
            println!("{}", serde_json::to_string_pretty(&report).unwrap());
            assert_eq!(report["checks"].as_array().unwrap().len(), 8);
        }
    }
    #[test]
    fn the_main_choice_follows_nested_groups() {
        let all = json!({
            "GLOBAL": {"type": "Selector", "all": ["DIRECT", "Auto", "Proxy"], "now": "DIRECT"},
            "Auto": {"type": "URLTest", "all": ["HK"], "now": "HK"},
            "Proxy": {"type": "Selector", "all": ["Auto", "JP"], "now": "Auto"},
            "HK": {"type": "Vmess"}, "JP": {"type": "Vmess"}, "DIRECT": {"type": "Direct"},
        });
        assert_eq!(main_choice(&all), ("Proxy".into(), "HK".into()));
    }
}
