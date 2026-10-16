//! The panel's line protocol, free of I/O so it can be tested off the device: the requests
//! the panel may send, the records it reads, and the Clash proxies view it pages through.
//! Groups and nodes travel to the panel by name but come back only as indices tied to a
//! generation number, so a name with spaces or emoji never has to be parsed from the
//! panel. No URLs, keys, endpoints or credentials appear in any record.
use serde_json::{json, Value};
use std::collections::HashMap;

/// The panel reads at most 48 KiB per reply; device rows are dropped first beyond this.
pub const STATUS_CAP: usize = 40 * 1024;
/// Rows per page on the 320×480 panel.
pub const PAGE: usize = 4;

/// One request line from the panel.
#[derive(Debug, PartialEq)]
pub enum Request {
    Status,
    /// A page of proxy groups from the cached view.
    Groups {
        offset: usize,
    },
    /// A page of one group's members; `generation` must match the cached view.
    Nodes {
        generation: u64,
        group: usize,
        offset: usize,
    },
    Job(Job),
}

/// Work done on the job thread, one at a time.
#[derive(Debug, PartialEq)]
pub enum Job {
    /// `toggle`, `profile` and `devices`: a WireGuard update. `toggle` never takes over
    /// from Clash; panels that know `backend` switch through it instead.
    Wireguard(Value),
    /// `backend <revision> wireguard|clash`, or `backend <revision> off <wireguard|clash>`
    /// naming the backend the panel shows as on (refused if another one is; older panels
    /// send plain `off`): the proxy switch, with takeover.
    Backend(Value),
    /// `cmode <revision> rule|global|direct`, `csub <revision> <id>`: a Clash update.
    Clash(Value),
    /// `crefresh <id>`: downloads a subscription again.
    Refresh(Value),
    /// `select <generation> <group> <node>`
    Select {
        generation: u64,
        group: usize,
        node: usize,
    },
    /// `delay <generation> <group>`: tests every member of the group.
    Delay { generation: u64, group: usize },
    /// `core check`, `core update <tag>`, `core rollback`, `core geodata`.
    Core(Value),
    /// `diag`: starts the one-tap diagnosis.
    Diagnose,
    /// `esim scan`, `esim enable <generation> <index>`, `esim reboot`.
    Esim(Esim),
    /// `uplink <revision> on|off`: the Wi-Fi uplink switch.
    Uplink { revision: u64, enabled: bool },
    /// `uplink login`: ten minutes of login mode for a network that wants a web login.
    UplinkLogin,
    /// Pocket OS toolbox actions: stock UI switch, Wi-Fi/USB/Network/TTL/Power/System controls.
    Tool(Tool),
}

/// Pocket OS toolbox actions from the panel.
#[derive(Debug, PartialEq)]
pub enum Tool {
    /// Return to the ZTE stock UI (`ztedevui`) cleanly.
    StockUi,
    /// Toggle 2.4GHz ("2g") or 5GHz ("5g") Wi-Fi radio.
    WifiToggle { band: String, enabled: bool },
    /// Switch USB mode (`rndis`, `ecm`, or the experimental `ncm`).
    UsbMode { mode: String },
    /// Switch the preferred network mode to one of the firmware's `net_select` values.
    NetMode { mode: String },
    /// Set IPv4/IPv6 hop limit / TTL (`0` clears TTL mangling, `64`/`65`/`128` applies it).
    Ttl { value: u16 },
    /// Toggle charging (`true` resumes, `false` pauses).
    ChargeSwitch { enabled: bool },
    /// Set charge stop limit (`70`..=`90` enables hysteresis, `100` disables limit).
    ChargeLimit { max_level: u8 },
    /// Kill ZTE carrier/telemetry bloatware processes to free RAM.
    KillBloat,
    /// Drop Linux pagecache/dentries/inodes (`sync` + `/proc/sys/vm/drop_caches`).
    DropCaches,
}

/// eSIM work from the panel. Profiles come back as indices into the card view of
/// `generation`, so the panel never sends an ICCID.
#[derive(Debug, PartialEq)]
pub enum Esim {
    Scan,
    Enable {
        generation: u64,
        index: usize,
    },
    /// Only while a switch waits for a restart.
    Reboot,
}

fn number(word: &str, error: &'static str) -> Result<u64, &'static str> {
    word.parse::<u64>().map_err(|_| error)
}
fn index(word: &str) -> Result<usize, &'static str> {
    word.parse::<usize>()
        .ok()
        .filter(|i| *i < 100_000)
        .ok_or("Invalid index")
}
fn tag_ok(tag: &str) -> bool {
    (1..=40).contains(&tag.len())
        && tag
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}
fn devices(list: &str) -> Result<Vec<Value>, &'static str> {
    let mut devices = vec![];
    for choice in list.split(',') {
        let (mac, selected) = choice.split_once('=').ok_or("Invalid device choice")?;
        if selected != "0" && selected != "1" {
            return Err("Invalid selection");
        }
        devices.push(json!({"mac":mac,"selected":selected=="1"}));
    }
    if devices.len() > 128 {
        return Err("Too many devices");
    }
    Ok(devices)
}

/// Parses one line; every mutation carries the revision the panel displayed, except a
/// subscription refresh (not revisioned) and node work (tied to the list generation).
pub fn parse(line: &str) -> Result<Request, &'static str> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let job = |j: Job| Ok(Request::Job(j));
    let revision = |w: &str| number(w, "Invalid revision");
    let generation = |w: &str| number(w, "Invalid generation");
    match words.as_slice() {
        ["status"] => Ok(Request::Status),
        ["groups", offset] => Ok(Request::Groups {
            offset: index(offset)?,
        }),
        ["nodes", g, group, offset] => Ok(Request::Nodes {
            generation: generation(g)?,
            group: index(group)?,
            offset: index(offset)?,
        }),
        ["toggle", rev, value @ ("0" | "1")] => job(Job::Wireguard(
            json!({"expected_revision":revision(rev)?,"enabled":*value=="1"}),
        )),
        ["profile", rev, id] => {
            let rev = revision(rev)?;
            let id = number(id, "Invalid profile ID")?;
            if id == 0 {
                return Err("Invalid profile ID");
            }
            job(Job::Wireguard(
                json!({"expected_revision":rev,"profile":{"action":"activate","id":id}}),
            ))
        }
        ["devices", rev, list] => {
            let rev = revision(rev)?;
            job(Job::Wireguard(
                json!({"expected_revision":rev,"devices":devices(list)?}),
            ))
        }
        ["backend", rev, target @ ("off" | "wireguard" | "clash")] => job(Job::Backend(
            json!({"backend":target,"expected_revision":revision(rev)?}),
        )),
        ["backend", rev, "off", active @ ("wireguard" | "clash")] => job(Job::Backend(
            json!({"backend":"off","expected_revision":revision(rev)?,"active":active}),
        )),
        ["cmode", rev, mode @ ("rule" | "global" | "direct")] => job(Job::Clash(
            json!({"expected_revision":revision(rev)?,"proxy_mode":mode}),
        )),
        ["csub", rev, id] => {
            let rev = revision(rev)?;
            let id = number(id, "Invalid subscription ID")?;
            if id == 0 {
                return Err("Invalid subscription ID");
            }
            job(Job::Clash(
                json!({"expected_revision":rev,"subscription":{"action":"activate","id":id}}),
            ))
        }
        ["crefresh", id] => match number(id, "Invalid subscription ID")? {
            0 => Err("Invalid subscription ID"),
            id => job(Job::Refresh(json!({ "id": id }))),
        },
        ["select", g, group, node] => job(Job::Select {
            generation: generation(g)?,
            group: index(group)?,
            node: index(node)?,
        }),
        ["delay", g, group] => job(Job::Delay {
            generation: generation(g)?,
            group: index(group)?,
        }),
        ["core", action @ ("check" | "rollback" | "geodata")] => {
            job(Job::Core(json!({ "action": action })))
        }
        ["core", "update", tag] if tag_ok(tag) => {
            job(Job::Core(json!({"action":"update","tag":tag})))
        }
        ["diag"] => job(Job::Diagnose),
        ["esim", "scan"] => job(Job::Esim(Esim::Scan)),
        ["esim", "reboot"] => job(Job::Esim(Esim::Reboot)),
        ["esim", "enable", g, i] => job(Job::Esim(Esim::Enable {
            generation: generation(g)?,
            index: index(i)?,
        })),
        ["uplink", "login"] => job(Job::UplinkLogin),
        ["uplink", rev, value @ ("on" | "off")] => job(Job::Uplink {
            revision: revision(rev)?,
            enabled: *value == "on",
        }),
        ["tool", "stock_ui"] => job(Job::Tool(Tool::StockUi)),
        ["tool", "wifi_toggle", band @ ("2g" | "5g"), value @ ("0" | "1")] => {
            job(Job::Tool(Tool::WifiToggle {
                band: (*band).into(),
                enabled: *value == "1",
            }))
        }
        ["tool", "usb_mode", mode @ ("rndis" | "ecm" | "ncm")] => job(Job::Tool(Tool::UsbMode {
            mode: (*mode).into(),
        })),
        ["tool", "net_mode", mode] => {
            if !crate::cell::NETWORK_MODES.iter().any(|(m, _)| m == mode) {
                return Err("Invalid network mode");
            }
            job(Job::Tool(Tool::NetMode {
                mode: (*mode).into(),
            }))
        }
        ["tool", "ttl", val] => {
            let v = number(val, "Invalid TTL")?;
            if !matches!(v, 0 | 64 | 65 | 128) {
                return Err("Invalid TTL");
            }
            job(Job::Tool(Tool::Ttl { value: v as u16 }))
        }
        ["tool", "charge_switch", value @ ("0" | "1")] => job(Job::Tool(Tool::ChargeSwitch {
            enabled: *value == "1",
        })),
        ["tool", "charge_limit", level] => {
            let lvl = number(level, "Invalid charge limit")?;
            if !matches!(lvl, 70 | 80 | 90 | 100) {
                return Err("Invalid charge limit");
            }
            job(Job::Tool(Tool::ChargeLimit {
                max_level: lvl as u8,
            }))
        }
        ["tool", "kill_bloat"] => job(Job::Tool(Tool::KillBloat)),
        ["tool", "drop_caches"] => job(Job::Tool(Tool::DropCaches)),
        [_, _, _, _] | [_, _, _] | [_, _] | [_] => Err("Unsupported action"),
        _ => Err("Invalid action"),
    }
}

/// Text for one tab-separated field: no control characters (tabs and newlines would
/// split the record), at most `max` characters, `-` when empty.
pub fn text(s: &str, max: usize) -> String {
    let s: String = s.chars().filter(|c| !c.is_control()).take(max).collect();
    if s.trim().is_empty() {
        "-".into()
    } else {
        s
    }
}
/// `text` for a field the panel keeps in a buffer of `max` bytes (versions 32, messages
/// 96): it ends before the first character that does not fit whole.
pub fn text_bytes(s: &str, max: usize) -> String {
    let mut out = String::new();
    for c in s.chars().filter(|c| !c.is_control()) {
        if out.len() + c.len_utf8() > max {
            break;
        }
        out.push(c);
    }
    if out.trim().is_empty() {
        "-".into()
    } else {
        out
    }
}
/// A short ASCII token (a type, state or version), `-` when absent.
fn word(s: &str, max: usize) -> String {
    let s: String = s
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || "._-".contains(*c))
        .take(max)
        .collect();
    if s.is_empty() {
        "-".into()
    } else {
        s
    }
}

/// Why a backend cannot be switched on, as a code the panel translates.
pub fn reason_code(reason: &Value) -> &'static str {
    let Some(reason) = reason.as_str() else {
        return "-";
    };
    let r = reason.to_ascii_lowercase();
    if r.contains("kernel wireguard") {
        "kernel"
    } else if r.contains("tools") {
        "tools"
    } else if r.contains("import") {
        "config"
    } else if r.contains("core") {
        "core"
    } else if r.contains("tun") {
        "tun"
    } else if r.contains("add a subscription") {
        "sub"
    } else if r.contains("download") {
        "download"
    } else if r.contains("invalid") {
        "invalid"
    } else {
        "other"
    }
}
/// `B active conflict` then, for WireGuard and Clash: revision, ready, reason, error.
pub fn backend_record(v: &Value) -> String {
    let side = |id: &str| {
        let b = v["backends"]
            .as_array()
            .and_then(|a| a.iter().find(|b| b["id"] == id))
            .unwrap_or(&Value::Null);
        format!(
            "{}\t{}\t{}\t{}",
            b["revision"].as_u64().unwrap_or(0),
            u8::from(b["ready"] == true),
            reason_code(&b["reason"]),
            u8::from(b["error"].is_string())
        )
    };
    let active = v["active"]
        .as_str()
        .filter(|a| matches!(*a, "wireguard" | "clash"))
        .unwrap_or("off");
    format!(
        "B\t{active}\t{}\t{}\t{}\n",
        u8::from(v["conflict"] == true),
        side("wireguard"),
        side("clash")
    )
}
/// `K` (Clash state) and up to five `U` (subscriptions). Subscription URLs stay out.
pub fn clash_records(v: &Value) -> String {
    let n = |k: &str| v[k].as_u64().unwrap_or(0);
    let b = |k: &str| u8::from(v[k] == true);
    let mode = v["proxy_mode"]
        .as_str()
        .filter(|m| matches!(*m, "rule" | "global" | "direct"))
        .unwrap_or("rule");
    let mut out = format!(
        "K\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
        n("revision"),
        b("enabled"),
        b("running"),
        b("connected"),
        mode,
        n("uptime"),
        n("memory_kib"),
        n("active_subscription"),
        b("core_installed"),
        text_bytes(v["version"].as_str().unwrap_or(""), 32),
        text_bytes(v["error"].as_str().unwrap_or(""), 96)
    );
    let active = v["active_subscription"].as_u64();
    for s in v["subscriptions"].as_array().into_iter().flatten().take(5) {
        let Some(id) = s["id"].as_u64().filter(|id| *id > 0) else {
            continue;
        };
        let u = |k: &str| s["usage"][k].as_u64().unwrap_or(0);
        out.push_str(&format!(
            "U\t{id}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            u8::from(active == Some(id)),
            u8::from(s["downloaded"] == true),
            s["updated_at"].as_u64().unwrap_or(0),
            u("upload"),
            u("download"),
            u("total"),
            u("expire"),
            s["proxies"].as_u64().unwrap_or(0),
            u8::from(s["error"].is_string()),
            text(s["name"].as_str().unwrap_or(""), 48)
        ));
    }
    out
}
/// `M`: installed, previous and latest core, the running or last core job, free space.
/// `M 0` when the agent has no core updater.
pub fn core_record(code: u16, reply: &Value) -> String {
    if code != 200 {
        return "M\t0\n".into();
    }
    let d = &reply["data"];
    let version = d["installed"]["version"].as_str().unwrap_or("");
    let tag = d["latest"]["tag"].as_str().unwrap_or("");
    let newer = !tag.is_empty() && tag.trim_start_matches('v') != version.trim_start_matches('v');
    let job = &d["job"];
    // Byte limits: the panel's buffers. The date is the first ten of an ISO timestamp.
    format!(
        "M\t1\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
        text_bytes(version, 32),
        word(d["installed"]["source"].as_str().unwrap_or(""), 12),
        text_bytes(d["previous"]["version"].as_str().unwrap_or(""), 32),
        text_bytes(tag, 32),
        text_bytes(d["latest"]["published_at"].as_str().unwrap_or(""), 10),
        u8::from(newer),
        word(job["kind"].as_str().unwrap_or(""), 12),
        word(job["state"].as_str().unwrap_or(""), 12),
        word(job["step"].as_str().unwrap_or(""), 16),
        job["bytes"].as_u64().unwrap_or(0),
        job["total"].as_u64().unwrap_or(0),
        d["free_bytes"].as_u64().unwrap_or(0),
        text_bytes(job["error"].as_str().unwrap_or(""), 96)
    )
}
/// Profile rows sent to the panel: two pages of four.
pub const ESIM_ROWS: usize = 8;
/// `Q installed card generation count reboot kind state step firmware_ready error`, then
/// one `q index enabled class name iccid-tail` per profile. No EID, and only the last four
/// digits of an ICCID, which tell profiles of one carrier apart.
pub fn esim_records(v: &Value) -> String {
    let view = &v["view"];
    let job = &v["job"];
    let profiles = view["profiles"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let firmware_ready = match (
        job["kind"].as_str(),
        job["result"]["firmware_ready"].as_bool(),
    ) {
        (Some("enable"), Some(ready)) => {
            if ready {
                "1"
            } else {
                "0"
            }
        }
        _ => "-",
    };
    let mut out = format!(
        "Q\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
        u8::from(v["installed"] == true),
        word(view["card"].as_str().unwrap_or(""), 8),
        view["generation"].as_u64().unwrap_or(0),
        profiles.len(),
        u8::from(v["reboot_needed"] == true),
        word(job["kind"].as_str().unwrap_or(""), 15),
        word(job["state"].as_str().unwrap_or(""), 11),
        word(job["step"].as_str().unwrap_or(""), 39),
        firmware_ready,
        text_bytes(
            job["error"]
                .as_str()
                .or(view["detail"].as_str())
                .unwrap_or(""),
            96
        )
    );
    for (i, p) in profiles.iter().take(ESIM_ROWS).enumerate() {
        let name = [&p["nickname"], &p["name"], &p["provider"]]
            .into_iter()
            .filter_map(Value::as_str)
            .find(|s| !s.trim().is_empty())
            .unwrap_or("");
        let iccid = p["iccid"].as_str().unwrap_or("");
        let tail = &iccid[iccid.len().saturating_sub(4)..];
        out.push_str(&format!(
            "q\t{i}\t{}\t{}\t{}\t{}\n",
            u8::from(p["enabled"] == true),
            word(p["class"].as_str().unwrap_or(""), 15),
            text_bytes(name, 96),
            word(tail, 4)
        ));
    }
    out
}

/// Why the Wi-Fi uplink is not carrying traffic, as a code the panel translates.
pub fn uplink_reason(reason: Option<&str>) -> &'static str {
    let Some(r) = reason.map(str::to_ascii_lowercase) else {
        return "-";
    };
    [
        // Before "client interface", which the pause message also names.
        ("relay is paused", "paused"),
        ("needs a web login", "portal"),
        ("login mode needs", "nologin"),
        ("finish starting", "booting"),
        ("radio is off", "radio"),
        ("radio to settle", "settle"),
        ("wpa_supplicant", "supplicant"),
        ("udhcpc", "dhcp"),
        ("client interface", "iface"),
        ("wrong wi-fi password", "password"),
        ("rejected the connection", "rejected"),
        ("no saved network", "range"),
        ("overlaps the lan", "overlap"),
        ("reach the internet", "internet"),
    ]
    .iter()
    .find(|(needle, _)| r.contains(needle))
    .map_or("other", |(_, code)| code)
}
/// `R revision enabled state reason ssid signal band ip rx tx online_since saved`: the
/// Wi-Fi uplink. `state` is one letter (see `wifi_uplink::State::code`); no password,
/// BSSID or gateway is sent.
pub fn uplink_record(u: &crate::wifi_uplink::Summary) -> String {
    format!(
        "R\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
        u.revision,
        u8::from(u.enabled),
        u.state.code(),
        uplink_reason(u.reason.as_deref()),
        text_bytes(&u.ssid, 64),
        u.signal.map_or("-".into(), |s| s.to_string()),
        word(u.band, 3),
        word(u.ip.as_deref().unwrap_or(""), 15),
        u.rx_bytes,
        u.tx_bytes,
        u.online_since.unwrap_or(0),
        u.saved
    )
}

/// The body of an eSIM enable for profile `index` of the view the panel showed.
pub fn esim_enable_body(
    view: &Value,
    generation: u64,
    index: usize,
) -> Result<Value, &'static str> {
    if view["generation"].as_u64() != Some(generation) {
        return Err("Stale");
    }
    let p = &view["profiles"][index];
    match p["iccid"].as_str() {
        Some(iccid) if p["enabled"] == false => Ok(json!({"action":"enable","iccid":iccid})),
        Some(_) => Err("Already enabled"),
        None => Err("Stale"),
    }
}

/// `X running finished verdict` and one `x` per check with up to four figures (`-` when
/// absent), which the panel words in its own language.
pub fn diag_records(v: &Value) -> String {
    let mut out = format!(
        "X\t{}\t{}\t{}\n",
        u8::from(v["running"] == true),
        v["finished_at"].as_u64().unwrap_or(0),
        word(v["verdict"].as_str().unwrap_or(""), 16)
    );
    let figure = |x: &Value| match x {
        Value::Bool(b) => u8::from(*b).to_string(),
        Value::Number(n) => n.as_i64().map_or("-".into(), |n| n.to_string()),
        Value::Array(a) => a.len().to_string(),
        _ => "-".into(),
    };
    for c in v["checks"].as_array().into_iter().flatten().take(8) {
        let d = &c["data"];
        let keys: [&str; 4] = match c["id"].as_str().unwrap_or("") {
            "clock" => ["skew_s", "ntp", "beijing", ""],
            "core" => ["running", "supervised", "", ""],
            "dns" | "fetch" => ["domestic", "foreign", "", ""],
            "nodes" => ["https_ms", "http_ms", "answered", "total"],
            "devices" => ["proxied", "total", "", ""],
            "route" => ["tun", "dns_tun", "", ""],
            "firewall" => ["missing", "displaced", "held", "rejected"],
            _ => ["", "", "", ""],
        };
        let f = keys.map(|k| {
            if k.is_empty() {
                "-".into()
            } else {
                figure(&d[k])
            }
        });
        out.push_str(&format!(
            "x\t{}\t{}\t{}\t{}\t{}\t{}\n",
            word(c["id"].as_str().unwrap_or(""), 12),
            word(c["status"].as_str().unwrap_or(""), 8),
            f[0],
            f[1],
            f[2],
            f[3]
        ));
    }
    out
}
/// `H\trssi\trsrp\trsrq\tsinr\tradio\tband\tnet_select\tlte_bands\tnr_bands\tpci\tkbps_down\tkbps_up\tcpu_pct\tmem_pct\ttemp_mc\tbloat_cnt\twifi_2g_on\twifi_5g_on\twifi_clients\twifi_ssid\tusb_mode\tusb_adb\tttl_val\tchg_enabled\tchg_limit_on\tchg_max\tchg_stop\tchg_start\tbatt_mv\tbatt_ma\tbatt_health`
/// Extended Pocket OS telemetry record for Signal, Wi-Fi, Power, Toolbox, and System subpages.
pub fn toolbox_record(v: &Value) -> String {
    let i = |k: &str| v[k].as_i64().unwrap_or(0);
    let u = |k: &str| v[k].as_u64().unwrap_or(0);
    let b = |k: &str| u8::from(v[k] == true);
    let w = |k: &str, max: usize| word(v[k].as_str().unwrap_or(""), max);
    let t = |k: &str, max: usize| text_bytes(v[k].as_str().unwrap_or(""), max);
    format!(
        "H\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\n",
        i("rssi"),
        i("rsrp"),
        i("rsrq"),
        i("sinr"),
        w("radio", 12),
        t("band", 24),
        w("net_select", 16),
        t("lte_bands", 24),
        t("nr_bands", 24),
        t("pci", 12),
        u("kbps_down"),
        u("kbps_up"),
        u("cpu_pct"),
        u("mem_pct"),
        i("temp_mc"),
        u("bloat_cnt"),
        b("wifi_2g_on"),
        b("wifi_5g_on"),
        u("wifi_clients"),
        t("wifi_ssid", 32),
        w("usb_mode", 16),
        b("usb_adb"),
        u("ttl_val"),
        b("chg_enabled"),
        b("chg_limit_on"),
        u("chg_max"),
        u("chg_stop"),
        u("chg_start"),
        u("batt_mv"),
        i("batt_ma"),
        w("batt_health", 16),
    )
}

/// The status reply: fixed records, then device rows while they fit, then the tail.
pub fn status(head: &[&str], devices: &[String], tail: &str) -> String {
    let fixed = head.iter().map(|s| s.len()).sum::<usize>() + tail.len();
    let mut room = STATUS_CAP.saturating_sub(fixed);
    let mut out = String::with_capacity(STATUS_CAP.min(fixed + devices.len() * 64));
    head.iter().for_each(|s| out.push_str(s));
    for d in devices {
        if d.len() > room {
            break;
        }
        room -= d.len();
        out.push_str(d);
    }
    out.push_str(tail);
    out
}

/// A visible proxy group as the panel pages through it.
#[derive(Debug, PartialEq)]
pub struct Group {
    pub name: String,
    pub kind: String,
    pub now: String,
    pub members: Vec<String>,
    pub selectable: bool,
}
/// Groups (hidden ones left out) with each member's type and last delay.
#[derive(Debug, PartialEq)]
pub struct View {
    pub generation: u64,
    pub groups: Vec<Group>,
    /// Name → (type, delay): -1 untested, 0 failed, else milliseconds.
    nodes: HashMap<String, (String, i64)>,
}
#[derive(Debug, PartialEq)]
pub enum Cache {
    Loading,
    Ready(View),
    /// Clash is not running or its controller does not answer.
    Unavailable,
}
/// FNV-1a over group names and members in order: indices the panel holds are only valid
/// for the list with the same number. Never 0, which the panel uses for "no list".
pub fn generation(groups: &[Group]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            hash ^= u64::from(*b);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
    };
    for g in groups {
        for part in std::iter::once(&g.name).chain(&g.members) {
            eat(part.as_bytes());
            eat(&[0]);
        }
        eat(&[1]);
    }
    hash.max(1)
}
/// The panel's view of `clash::proxies()` data.
pub fn view(data: &Value) -> View {
    let s = |v: &Value| v.as_str().unwrap_or("").to_string();
    let groups: Vec<Group> = data["groups"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|g| g["hidden"] != true && g["name"].is_string())
        .map(|g| Group {
            name: s(&g["name"]),
            kind: s(&g["type"]),
            now: s(&g["now"]),
            members: g["all"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|m| m.as_str().map(String::from))
                .collect(),
            selectable: g["selectable"] == true,
        })
        .collect();
    let nodes = data["nodes"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(name, n)| {
            let delay = match (n["delay"].as_u64(), n["alive"].as_bool()) {
                (Some(d), _) if d > 0 => i64::try_from(d).unwrap_or(i64::MAX),
                (Some(_), _) | (None, Some(false)) => 0,
                _ => -1,
            };
            (name.clone(), (s(&n["type"]), delay))
        })
        .collect();
    View {
        generation: generation(&groups),
        groups,
        nodes,
    }
}
fn first_row(offset: usize, total: usize) -> usize {
    if total == 0 {
        0
    } else {
        offset.min((total - 1) / PAGE * PAGE)
    }
}
/// `G state generation total offset` (state 0 loading, 1 ready, 2 unavailable) and up
/// to four `g index selectable members current type name current-name`.
pub fn groups_page(cache: &Cache, offset: usize) -> String {
    let v = match cache {
        Cache::Loading => return "G\t0\t0\t0\t0\n.\n".into(),
        Cache::Unavailable => return "G\t2\t0\t0\t0\n.\n".into(),
        Cache::Ready(v) => v,
    };
    let offset = first_row(offset, v.groups.len());
    let mut out = format!("G\t1\t{}\t{}\t{offset}\n", v.generation, v.groups.len());
    for (i, g) in v.groups.iter().enumerate().skip(offset).take(PAGE) {
        let current = g.members.iter().position(|m| *m == g.now);
        out.push_str(&format!(
            "g\t{i}\t{}\t{}\t{}\t{}\t{}\t{}\n",
            u8::from(g.selectable),
            g.members.len(),
            current.map_or(-1, |c| c as i64),
            word(&g.kind, 15),
            text(&g.name, 48),
            text(&g.now, 48)
        ));
    }
    out.push_str(".\n");
    out
}
fn group(cache: &Cache, generation: u64, index: usize) -> Result<&Group, &'static str> {
    match cache {
        Cache::Ready(v) if v.generation == generation => v.groups.get(index).ok_or("Stale"),
        _ => Err("Stale"),
    }
}
/// `N generation group total offset selectable current type name` and up to four
/// `n index delay type name` (delay -1 untested, 0 failed). `Stale` when the list the
/// panel holds is not the cached one.
pub fn nodes_page(
    cache: &Cache,
    generation: u64,
    index: usize,
    offset: usize,
) -> Result<String, &'static str> {
    let g = group(cache, generation, index)?;
    let Cache::Ready(v) = cache else {
        return Err("Stale");
    };
    let offset = first_row(offset, g.members.len());
    let current = g.members.iter().position(|m| *m == g.now);
    let mut out = format!(
        "N\t{generation}\t{index}\t{}\t{offset}\t{}\t{}\t{}\t{}\n",
        g.members.len(),
        u8::from(g.selectable),
        current.map_or(-1, |c| c as i64),
        word(&g.kind, 15),
        text(&g.name, 48)
    );
    for (i, m) in g.members.iter().enumerate().skip(offset).take(PAGE) {
        let (kind, delay) = v.nodes.get(m).cloned().unwrap_or_default();
        let delay = if kind.is_empty() { -1 } else { delay };
        out.push_str(&format!(
            "n\t{i}\t{delay}\t{}\t{}\n",
            word(&kind, 15),
            text(m, 48)
        ));
    }
    out.push_str(".\n");
    Ok(out)
}
/// The `clash::select` body for a node the panel chose by index.
pub fn select_body(
    cache: &Cache,
    generation: u64,
    index: usize,
    node: usize,
) -> Result<Value, &'static str> {
    let g = group(cache, generation, index)?;
    if !g.selectable {
        return Err("Group picks its node automatically");
    }
    let name = g.members.get(node).ok_or("Stale")?;
    Ok(json!({"group": g.name, "name": name}))
}
/// The `clash::delay` body that tests a whole group.
pub fn delay_body(cache: &Cache, generation: u64, index: usize) -> Result<Value, &'static str> {
    let g = group(cache, generation, index)?;
    Ok(json!({"name": g.name, "group": true, "timeout_ms": 5000}))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn job(line: &str) -> Job {
        match parse(line) {
            Ok(Request::Job(j)) => j,
            other => panic!("{line}: {other:?}"),
        }
    }
    #[test]
    fn verbs_are_narrow_and_carry_revisions() {
        assert_eq!(parse("status\n"), Ok(Request::Status));
        assert_eq!(
            job("toggle 8 1"),
            Job::Wireguard(json!({"expected_revision":8,"enabled":true}))
        );
        assert_eq!(
            job("profile 8 2"),
            Job::Wireguard(json!({"expected_revision":8,"profile":{"action":"activate","id":2}}))
        );
        assert_eq!(
            job("devices 3 02:11:22:33:44:55=1,02:11:22:33:44:56=0"),
            Job::Wireguard(json!({"expected_revision":3,"devices":[
                {"mac":"02:11:22:33:44:55","selected":true},{"mac":"02:11:22:33:44:56","selected":false}]}))
        );
        assert_eq!(
            job("backend 7 clash"),
            Job::Backend(json!({"backend":"clash","expected_revision":7}))
        );
        // Off names the backend the panel shows as on; plain off is an older panel's.
        assert_eq!(
            job("backend 2 off"),
            Job::Backend(json!({"backend":"off","expected_revision":2}))
        );
        assert_eq!(
            job("backend 2 off clash\n"),
            Job::Backend(json!({"backend":"off","expected_revision":2,"active":"clash"}))
        );
        assert_eq!(
            job("backend 5 off wireguard"),
            Job::Backend(json!({"backend":"off","expected_revision":5,"active":"wireguard"}))
        );
        assert_eq!(
            job("cmode 4 global"),
            Job::Clash(json!({"expected_revision":4,"proxy_mode":"global"}))
        );
        assert_eq!(
            job("csub 4 9"),
            Job::Clash(json!({"expected_revision":4,"subscription":{"action":"activate","id":9}}))
        );
        assert_eq!(job("crefresh 9"), Job::Refresh(json!({"id":9})));
        assert_eq!(
            job("select 123 1 4"),
            Job::Select {
                generation: 123,
                group: 1,
                node: 4
            }
        );
        assert_eq!(
            job("delay 123 0"),
            Job::Delay {
                generation: 123,
                group: 0
            }
        );
        assert_eq!(job("core check"), Job::Core(json!({"action":"check"})));
        assert_eq!(
            job("core update v1.19.14"),
            Job::Core(json!({"action":"update","tag":"v1.19.14"}))
        );
        assert_eq!(
            job("core rollback"),
            Job::Core(json!({"action":"rollback"}))
        );
        assert_eq!(job("core geodata"), Job::Core(json!({"action":"geodata"})));
        assert_eq!(job("diag"), Job::Diagnose);
        assert_eq!(job("esim scan"), Job::Esim(Esim::Scan));
        assert_eq!(
            job("uplink 4 on"),
            Job::Uplink {
                revision: 4,
                enabled: true
            }
        );
        assert_eq!(
            job("uplink 5 off"),
            Job::Uplink {
                revision: 5,
                enabled: false
            }
        );
        assert_eq!(job("uplink login"), Job::UplinkLogin);
        assert_eq!(job("esim reboot"), Job::Esim(Esim::Reboot));
        assert_eq!(
            job("esim enable 77 3"),
            Job::Esim(Esim::Enable {
                generation: 77,
                index: 3
            })
        );
        assert_eq!(parse("groups 4"), Ok(Request::Groups { offset: 4 }));
        assert_eq!(
            parse("nodes 99 2 8"),
            Ok(Request::Nodes {
                generation: 99,
                group: 2,
                offset: 8
            })
        );
        for bad in [
            "profile 8 0",
            "profile 8 -1",
            "profile 8 x",
            "profile 8 2 extra",
            "toggle 8 true",
            "toggle x 1",
            "config 8 secret",
            "devices 8 aa=2",
            "backend 7 sing-box",
            "backend -1 off",
            "backend 7",
            "backend 7 off off",
            "backend 7 off sing-box",
            "backend x off clash",
            "backend 7 off clash extra",
            // Only off names the active backend.
            "backend 7 clash wireguard",
            "backend 7 wireguard wireguard",
            "cmode 4 tun",
            "csub 4 0",
            "crefresh 0",
            "crefresh x",
            "select 1 0",
            "select x 0 0",
            "delay 1 -1",
            "nodes 1 2",
            "groups x",
            "core update",
            "core update ../../etc",
            "core update $(reboot)",
            "core install",
            "diag now",
            "esim",
            "esim download",
            "esim enable 77",
            "esim enable x 1",
            "esim enable 77 -1",
            "esim enable 77 1 extra",
            "esim delete 77 1",
            "uplink",
            "uplink 3",
            "uplink x on",
            "uplink 3 yes",
            "uplink 3 on now",
            "status extra",
            "",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        assert!(parse(&format!("devices 1 {}", ["aa=1"; 129].join(","))).is_err());
    }
    #[test]
    fn backend_reasons_become_codes() {
        let v = json!({"active":"clash","conflict":false,"backends":[
            {"id":"wireguard","enabled":false,"revision":5,"ready":false,"reason":"Import a WireGuard client configuration first","error":null},
            {"id":"clash","enabled":true,"revision":9,"ready":true,"reason":null,"error":"boom","running":true}]});
        assert_eq!(
            backend_record(&v),
            "B\tclash\t0\t5\t0\tconfig\t0\t9\t1\t-\t1\n"
        );
        for (reason, code) in [
            ("Kernel WireGuard or /data/bin/wg is unavailable", "kernel"),
            ("WireGuard tools missing: install /data/bin/wg", "tools"),
            ("Clash core missing: install /data/bin/mihomo", "core"),
            ("Kernel TUN support is unavailable", "tun"),
            ("Add a subscription first", "sub"),
            (
                "Active subscription has not been downloaded; refresh it",
                "download",
            ),
            ("Saved WireGuard configuration is invalid", "invalid"),
            ("Something new", "other"),
        ] {
            assert_eq!(reason_code(&json!(reason)), code, "{reason}");
        }
        assert_eq!(
            backend_record(&json!({})),
            "B\toff\t0\t0\t0\t-\t0\t0\t0\t-\t0\n"
        );
    }
    #[test]
    fn clash_records_redact_and_escape() {
        let v = json!({"revision":4,"enabled":true,"running":true,"connected":true,"proxy_mode":"global",
            "uptime":3600,"memory_kib":40960,"active_subscription":2,"core_installed":true,"version":"v1.19.14",
            "error":null,"subscriptions":[
                {"id":1,"name":"Home\tsub\n","url":"https://***","updated_at":10,"usage":null,"proxies":3,"downloaded":true,"error":null},
                {"id":2,"name":"🇭🇰 机场","url":"https://secret.example/token","updated_at":20,
                 "usage":{"upload":1,"download":2,"total":3,"expire":4},"proxies":30,"downloaded":true,"error":"timeout"}]});
        let s = clash_records(&v);
        assert_eq!(
            s,
            "K\t4\t1\t1\t1\tglobal\t3600\t40960\t2\t1\tv1.19.14\t-\n\
             U\t1\t0\t1\t10\t0\t0\t0\t0\t3\t0\tHomesub\n\
             U\t2\t1\t1\t20\t1\t2\t3\t4\t30\t1\t🇭🇰 机场\n"
        );
        assert!(!s.contains("secret") && !s.contains("https"));
        assert!(clash_records(&json!({"proxy_mode":"x;reboot"})).contains("\trule\t"));
    }
    #[test]
    fn core_and_diagnostics_records() {
        assert_eq!(core_record(503, &json!({})), "M\t0\n");
        let reply = json!({"ok":true,"data":{"installed":{"version":"v1.19.13","sha256":"ab","source":"pinned"},
            "previous":null,"latest":{"tag":"v1.19.14","published_at":"2026-09-30T08:00:00Z"},
            "job":{"kind":"update","state":"running","step":"downloading","bytes":5,"total":10,"error":null},
            "free_bytes":1000}});
        assert_eq!(
            core_record(200, &reply),
            "M\t1\tv1.19.13\tpinned\t-\tv1.19.14\t2026-09-30\t1\tupdate\trunning\tdownloading\t5\t10\t1000\t-\n"
        );
        let same = json!({"data":{"installed":{"version":"1.19.14"},"latest":{"tag":"v1.19.14"}}});
        assert!(core_record(200, &same).starts_with("M\t1\t1.19.14\t-\t-\tv1.19.14\t-\t0\t"));
        let d = json!({"running":false,"finished_at":77,"verdict":"nodes","checks":[
            {"id":"clock","status":"pass","data":{"skew_s":28796,"ntp":true,"beijing":true},"detail":""},
            {"id":"nodes","status":"fail","data":{"https_ms":null,"http_ms":250,"answered":2,"total":9},"detail":""},
            {"id":"firewall","status":"warn","data":{"missing":[],"displaced":["OUCL_FWD"],"held":false,"rejected":12},"detail":""},
            {"id":"route","status":"pass","data":{"client":"192.0.2.2","tun":true,"dns_tun":true},"detail":"ip route\nsecret"}]});
        assert_eq!(
            diag_records(&d),
            "X\t0\t77\tnodes\n\
             x\tclock\tpass\t28796\t1\t1\t-\n\
             x\tnodes\tfail\t-\t250\t2\t9\n\
             x\tfirewall\twarn\t0\t1\t0\t12\n\
             x\troute\tpass\t1\t1\t-\t-\n"
        );
        assert_eq!(diag_records(&json!({})), "X\t0\t0\t-\n");
    }
    #[test]
    fn fixed_size_fields_end_on_a_character_boundary() {
        // 95 bytes, then a three-byte character that does not fit in 96.
        let long = format!("{}机器", "a".repeat(95));
        assert_eq!(text_bytes(&long, 96), "a".repeat(95));
        assert_eq!(text_bytes(&"机".repeat(40), 96), "机".repeat(32));
        assert_eq!(text_bytes(&"机".repeat(40), 97), "机".repeat(32));
        assert_eq!(text_bytes("v1\t.2\n", 32), "v1.2");
        assert_eq!(text_bytes(" \t ", 32), "-");
        assert_eq!(text_bytes("", 96), "-");
        let fields = |record: &str| -> Vec<String> {
            record
                .lines()
                .next()
                .unwrap()
                .split('\t')
                .map(String::from)
                .collect()
        };
        // The panel keeps versions in 33 bytes and messages in 97, both NUL-terminated.
        let version = format!("v1-{}", "版".repeat(20));
        let k = fields(&clash_records(&json!({"version":version,"error":long})));
        assert_eq!(k[10], format!("v1-{}", "版".repeat(9)));
        assert_eq!(k[11], "a".repeat(95));
        let reply = json!({"data":{"installed":{"version":version},"previous":{"version":"旧".repeat(11)},
            "latest":{"tag":"v1.19.14","published_at":"２０２６-09-30"},
            "job":{"kind":"update","state":"failed","error":"下载失败".repeat(10)}}});
        let m = fields(&core_record(200, &reply));
        assert_eq!((m[2].len(), m[4].len()), (30, 30));
        // Three-byte digits: the fourth would end past the 10 bytes of the date.
        assert_eq!(m[6], "２０２");
        assert_eq!(m[14], "下载失败".repeat(8));
        for f in k.iter().chain(&m) {
            assert!(f.len() <= 96, "{f}");
        }
    }
    #[test]
    fn status_drops_device_rows_first() {
        let head = "S 1\n".repeat(10);
        let rows: Vec<String> = (0..2000)
            .map(|i| format!("D\t{i:017}\t1\t1\t-\t名字名字名字名字\n"))
            .collect();
        let s = status(&[&head], &rows, "C 1\nJ 0 0 0 0 0\n.\n");
        assert!(s.len() <= STATUS_CAP);
        assert!(s.starts_with(&head) && s.ends_with("J 0 0 0 0 0\n.\n"));
        assert!(s.contains(&rows[0]) && !s.contains(&rows[1999]));
        let small = status(&[&head], &rows[..3], ".\n");
        assert_eq!(small, format!("{head}{}{}{}.\n", rows[0], rows[1], rows[2]));
    }
    fn proxies() -> Value {
        json!({"groups":[
            {"name":"🚀 节点选择","type":"Selector","now":"🇭🇰 香港 01","all":["♻️ 自动选择","🇭🇰 香港 01","🇯🇵 Tokyo\t2","DIRECT","n4","n5"],"hidden":false,"selectable":true},
            {"name":"♻️ 自动选择","type":"URLTest","now":"🇭🇰 香港 01","all":["🇭🇰 香港 01","🇯🇵 Tokyo\t2"],"hidden":false,"selectable":false},
            {"name":"Hidden","type":"Selector","now":"DIRECT","all":["DIRECT"],"hidden":true,"selectable":true},
            {"name":"GLOBAL","type":"Selector","now":"DIRECT","all":["DIRECT","🚀 节点选择"],"hidden":false,"selectable":true}],
          "nodes":{"🇭🇰 香港 01":{"type":"Trojan","delay":88,"alive":true},
                   "🇯🇵 Tokyo\t2":{"type":"Vmess","delay":0,"alive":false},
                   "DIRECT":{"type":"Direct","delay":null,"alive":true},
                   "♻️ 自动选择":{"type":"URLTest","delay":88,"alive":true}}})
    }
    #[test]
    fn groups_and_nodes_page_by_index_and_generation() {
        let cache = Cache::Ready(view(&proxies()));
        let Cache::Ready(v) = &cache else {
            unreachable!()
        };
        assert_eq!(v.groups.len(), 3, "hidden groups are left out");
        let gen = v.generation;
        assert_eq!(
            groups_page(&cache, 0),
            format!(
                "G\t1\t{gen}\t3\t0\n\
                 g\t0\t1\t6\t1\tSelector\t🚀 节点选择\t🇭🇰 香港 01\n\
                 g\t1\t0\t2\t0\tURLTest\t♻️ 自动选择\t🇭🇰 香港 01\n\
                 g\t2\t1\t2\t0\tSelector\tGLOBAL\tDIRECT\n.\n"
            )
        );
        // Past the end: the last page.
        assert!(groups_page(&cache, 40).starts_with(&format!("G\t1\t{gen}\t3\t0\n")));
        assert_eq!(groups_page(&Cache::Loading, 0), "G\t0\t0\t0\t0\n.\n");
        assert_eq!(groups_page(&Cache::Unavailable, 0), "G\t2\t0\t0\t0\n.\n");
        assert_eq!(
            nodes_page(&cache, gen, 0, 0).unwrap(),
            format!(
                "N\t{gen}\t0\t6\t0\t1\t1\tSelector\t🚀 节点选择\n\
                 n\t0\t88\tURLTest\t♻️ 自动选择\n\
                 n\t1\t88\tTrojan\t🇭🇰 香港 01\n\
                 n\t2\t0\tVmess\t🇯🇵 Tokyo2\n\
                 n\t3\t-1\tDirect\tDIRECT\n.\n"
            )
        );
        assert_eq!(
            nodes_page(&cache, gen, 0, 4).unwrap(),
            format!("N\t{gen}\t0\t6\t4\t1\t1\tSelector\t🚀 节点选择\nn\t4\t-1\t-\tn4\nn\t5\t-1\t-\tn5\n.\n")
        );
        for (g, i) in [(gen + 1, 0), (gen, 3)] {
            assert_eq!(nodes_page(&cache, g, i, 0), Err("Stale"));
        }
        assert_eq!(nodes_page(&Cache::Loading, gen, 0, 0), Err("Stale"));
        assert_eq!(
            select_body(&cache, gen, 0, 2).unwrap(),
            json!({"group":"🚀 节点选择","name":"🇯🇵 Tokyo\t2"})
        );
        assert!(
            select_body(&cache, gen, 1, 0).is_err(),
            "URLTest is automatic"
        );
        assert_eq!(select_body(&cache, gen, 0, 6), Err("Stale"));
        assert_eq!(select_body(&cache, gen ^ 1, 0, 0), Err("Stale"));
        assert_eq!(
            delay_body(&cache, gen, 1).unwrap(),
            json!({"name":"♻️ 自动选择","group":true,"timeout_ms":5000})
        );
    }
    #[test]
    fn generation_tracks_names_and_members_only() {
        let a = view(&proxies());
        let mut moved = proxies();
        moved["groups"][0]["now"] = json!("DIRECT");
        moved["nodes"]["🇭🇰 香港 01"]["delay"] = json!(300);
        assert_eq!(
            view(&moved).generation,
            a.generation,
            "selection and delay keep it"
        );
        let mut renamed = proxies();
        renamed["groups"][1]["all"][1] = json!("🇯🇵 Tokyo 3");
        assert_ne!(view(&renamed).generation, a.generation);
        let mut reordered = proxies();
        reordered["groups"][0]["all"]
            .as_array_mut()
            .unwrap()
            .swap(0, 1);
        assert_ne!(view(&reordered).generation, a.generation);
        let split = |groups: Vec<Vec<&str>>| {
            generation(
                &groups
                    .iter()
                    .map(|m| Group {
                        name: "g".into(),
                        kind: String::new(),
                        now: String::new(),
                        members: m.iter().map(|s| s.to_string()).collect(),
                        selectable: true,
                    })
                    .collect::<Vec<_>>(),
            )
        };
        assert_ne!(
            split(vec![vec!["a", "b"]]),
            split(vec![vec!["a"], vec!["b"]])
        );
        assert_ne!(generation(&[]), 0);
    }
    // Built from pieces so no whole test identifier sits in the source
    // (scripts/check-device-secrets.py rejects those).
    const ESIM_ONE: &str = concat!("89852000", "12345678901");
    const ESIM_TWO: &str = concat!("8944476", "500001234567");
    #[test]
    fn esim_records_hide_identifiers() {
        let v = json!({"installed":true,"reboot_needed":true,
            "job":{"kind":"enable","state":"done","step":"firmware","error":null,
                   "result":{"iccid":ESIM_ONE,"firmware_ready":false}},
            "view":{"card":"euicc","generation":42,"eid":"89049032123451234512345678901235","profiles":[
                {"iccid":ESIM_ONE,"enabled":true,"name":"CMHK","nickname":"香港\t卡","provider":"China Mobile","class":"operational"},
                {"iccid":ESIM_TWO,"enabled":false,"name":"","nickname":"","provider":"Truphone","class":"test"}]}});
        let r = esim_records(&v);
        assert_eq!(
            r,
            "Q\t1\teuicc\t42\t2\t1\tenable\tdone\tfirmware\t0\t-\n\
             q\t0\t1\toperational\t香港卡\t8901\n\
             q\t1\t0\ttest\tTruphone\t4567\n"
        );
        assert!(!r.contains("89049032") && !r.contains(ESIM_ONE));
        assert_eq!(
            esim_records(&json!({"installed":false,"view":null,"job":null,"reboot_needed":false})),
            "Q\t0\t-\t0\t0\t0\t-\t-\t-\t-\t-\n"
        );
        let failed = json!({"installed":true,"job":{"kind":"scan","state":"failed","step":"reading","error":"The modem did not answer in time"},
            "view":{"card":"error","detail":"The modem did not answer in time","generation":0,"profiles":[]}});
        assert!(esim_records(&failed).ends_with("\t-\tThe modem did not answer in time\n"));
        let many: Vec<Value> = (0..12)
            .map(|i| json!({"iccid":format!("89{i:017}"),"enabled":i==0,"name":"x"}))
            .collect();
        let r = esim_records(
            &json!({"installed":true,"view":{"card":"euicc","generation":1,"profiles":many}}),
        );
        assert!(r.starts_with("Q\t1\teuicc\t1\t12\t"));
        assert_eq!(
            r.lines().filter(|l| l.starts_with("q\t")).count(),
            ESIM_ROWS
        );
    }
    #[test]
    fn esim_enable_names_a_profile_of_the_shown_view() {
        let view = json!({"generation":42,"profiles":[
            {"iccid":ESIM_ONE,"enabled":true},{"iccid":ESIM_TWO,"enabled":false}]});
        assert_eq!(
            esim_enable_body(&view, 42, 1),
            Ok(json!({"action":"enable","iccid":ESIM_TWO}))
        );
        assert_eq!(esim_enable_body(&view, 41, 1), Err("Stale"));
        assert_eq!(esim_enable_body(&view, 42, 2), Err("Stale"));
        assert_eq!(esim_enable_body(&view, 42, 0), Err("Already enabled"));
        assert_eq!(esim_enable_body(&Value::Null, 0, 0), Err("Stale"));
    }
    #[test]
    fn uplink_record_sanitises_and_codes_reasons() {
        use crate::wifi_uplink::{State, Summary};
        let mut u = Summary {
            revision: 6,
            enabled: true,
            state: State::Online,
            reason: None,
            ssid: "Café\t\n家".into(),
            signal: Some(-52),
            band: "5",
            ip: Some("192.168.50.23".into()),
            rx_bytes: 1024,
            tx_bytes: 2048,
            online_since: Some(1_700_000_000),
            saved: 2,
        };
        assert_eq!(
            uplink_record(&u),
            "R\t6\t1\tn\t-\tCafé家\t-52\t5\t192.168.50.23\t1024\t2048\t1700000000\t2\n"
        );
        u.state = State::Off;
        u.enabled = false;
        u.ssid.clear();
        u.signal = None;
        u.band = "";
        u.ip = None;
        u.online_since = None;
        assert_eq!(
            uplink_record(&u),
            "R\t6\t0\to\t-\t-\t-\t-\t-\t1024\t2048\t0\t2\n"
        );
        u.ssid = "长".repeat(30);
        let r = uplink_record(&u);
        assert_eq!(r.split('\t').nth(5).unwrap().len(), 63);
        for (reason, code) in [
            ("The Wi-Fi radio is off; turn the hotspot on first", "radio"),
            ("Waiting for the Wi-Fi radio to settle", "settle"),
            (
                "wpa_supplicant is not installed on this device",
                "supplicant",
            ),
            ("udhcpc is not installed on this device", "dhcp"),
            ("The Wi-Fi radio refused a client interface", "iface"),
            ("Wrong Wi-Fi password", "password"),
            ("The upstream network rejected the connection", "rejected"),
            ("No saved network is in range", "range"),
            (
                "The upstream network 192.168.0.0/24 overlaps the LAN; change the LAN address",
                "overlap",
            ),
            (
                "Connected, but the upstream network does not reach the internet",
                "internet",
            ),
            ("Cannot write the DHCP hook", "other"),
            ("The firmware kept removing the Wi-Fi client interface; the relay is paused. Turn it off and on to try again", "paused"),
            ("Waiting for the device to finish starting", "booting"),
        ] {
            assert_eq!(uplink_reason(Some(reason)), code, "{reason}");
        }
        assert_eq!(uplink_reason(None), "-");
    }
    #[test]
    fn toolbox_requests_and_record() {
        let tool = |line: &str| match parse(line) {
            Ok(Request::Job(Job::Tool(t))) => Ok(t),
            Ok(_) => Err("not a tool"),
            Err(e) => Err(e),
        };
        assert_eq!(tool("tool stock_ui"), Ok(Tool::StockUi));
        assert_eq!(
            tool("tool wifi_toggle 5g 1"),
            Ok(Tool::WifiToggle {
                band: "5g".into(),
                enabled: true
            })
        );
        assert_eq!(
            tool("tool usb_mode ncm"),
            Ok(Tool::UsbMode { mode: "ncm".into() })
        );
        assert!(tool("tool usb_mode mtp").is_err());
        assert_eq!(
            tool("tool net_mode Only_5G"),
            Ok(Tool::NetMode {
                mode: "Only_5G".into()
            })
        );
        assert!(tool("tool net_mode 5G_SA").is_err());
        assert_eq!(tool("tool ttl 64"), Ok(Tool::Ttl { value: 64 }));
        assert!(tool("tool ttl 99").is_err());
        assert_eq!(
            tool("tool charge_switch 0"),
            Ok(Tool::ChargeSwitch { enabled: false })
        );
        assert_eq!(
            tool("tool charge_limit 80"),
            Ok(Tool::ChargeLimit { max_level: 80 })
        );
        assert!(tool("tool charge_limit 50").is_err());
        assert_eq!(tool("tool kill_bloat"), Ok(Tool::KillBloat));
        assert_eq!(tool("tool drop_caches"), Ok(Tool::DropCaches));
        assert!(tool("tool").is_err() && tool("wifi 5g 1").is_err());
        let h = toolbox_record(&json!({
            "rssi": -65,
            "rsrp": -92,
            "rsrq": -10,
            "sinr": 18,
            "radio": "NR5G-SA",
            "band": "N78",
            "net_select": "5g_sa_nsa",
            "lte_bands": "1,3,8",
            "nr_bands": "41,78",
            "pci": "105",
            "kbps_down": 125000,
            "kbps_up": 32000,
            "cpu_pct": 14,
            "mem_pct": 42,
            "temp_mc": 38500,
            "bloat_cnt": 3,
            "wifi_2g_on": true,
            "wifi_5g_on": true,
            "wifi_clients": 5,
            "wifi_ssid": "ZTE-U60Pro",
            "usb_mode": "rndis",
            "usb_adb": true,
            "ttl_val": 64,
            "chg_enabled": true,
            "chg_limit_on": true,
            "chg_max": 80,
            "chg_stop": 80,
            "chg_start": 70,
            "batt_mv": 4120,
            "batt_ma": 850,
            "batt_health": "Good"
        }));
        assert_eq!(
            h,
            "H\t-65\t-92\t-10\t18\tNR5G-SA\tN78\t5g_sa_nsa\t1,3,8\t41,78\t105\t125000\t32000\t14\t42\t38500\t3\t1\t1\t5\tZTE-U60Pro\trndis\t1\t64\t1\t1\t80\t80\t70\t4120\t850\tGood\n"
        );
    }
}
