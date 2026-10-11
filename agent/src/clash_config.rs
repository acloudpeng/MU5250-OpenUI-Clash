//! Composes the mihomo configuration from a subscription profile. The subscription
//! supplies nodes, groups, rules, providers and DNS upstreams; everything that opens
//! a port, touches routing or the firewall, or reaches outside the state directory is
//! decided here instead.
use serde_norway::{Mapping, Value};

/// Subscription profiles are downloaded with this size cap.
pub const MAX_PROFILE_BYTES: usize = 8 * 1024 * 1024;
pub const MIXED_PORT: u16 = 7890;
pub const DNS_LISTEN: &str = "127.0.0.1:1053";
/// The TUN takes the first /30 of this range; the rest is the fake-IP pool.
pub const FAKE_IP_RANGE: &str = "198.18.0.1/16";
/// B31 has been seen keeping Beijing time in the system clock as if it were UTC (8 h
/// ahead), and the firmware's own NITZ/NTP writes it back. VMess, Shadowsocks 2022 and
/// some REALITY servers reject handshakes that far off, so the core keeps its own
/// NTP-corrected clock and never writes the system's. ntp.aliyun.com by address, so
/// time does not wait on DNS.
const NTP_SERVER: &str = "203.107.6.88";
/// Minutes between syncs; also how long the core waits after a failed sync, such as
/// the first one at boot before the WAN is up.
const NTP_INTERVAL: u16 = 5;

/// Top-level subscription keys carried into the generated configuration. Anything
/// else (listeners, tunnels, controller, iptables, interface/mark, ntp, tls, ...) is dropped.
const KEEP: &[&str] = &[
    "proxies",
    "proxy-groups",
    "proxy-providers",
    "rules",
    "rule-providers",
    "sub-rules",
    "hosts",
    "sniffer",
    "unified-delay",
    "tcp-concurrent",
    "global-client-fingerprint",
    "global-ua",
    "keep-alive-idle",
    "keep-alive-interval",
    "disable-keep-alive",
    "geodata-mode",
    "geosite-matcher",
    "geox-url",
    "etag-support",
];
/// DNS keys that bind, pick the resolver mode or the fake-IP pool belong to the agent.
const DNS_OWNED: &[&str] = &[
    "enable",
    "listen",
    "listen-routing-mark",
    "ipv6",
    "ipv6-timeout",
    "enhanced-mode",
    "fake-ip-range",
    "fake-ip-range6",
];
/// DNS keys holding server lists, and policies whose values are server lists.
const DNS_SERVER_LISTS: &[&str] = &[
    "nameserver",
    "fallback",
    "default-nameserver",
    "proxy-server-nameserver",
    "direct-nameserver",
];
const DNS_SERVER_POLICIES: &[&str] = &["nameserver-policy", "proxy-server-nameserver-policy"];
const DEFAULT_NAMESERVERS: &[&str] = &["223.5.5.5", "119.29.29.29"];
const DEFAULT_FAKE_IP_FILTER: &[&str] = &[
    "*.lan",
    "*.local",
    "+.msftconnecttest.com",
    "+.msftncsi.com",
    "+.pool.ntp.org",
    "time.*.com",
    "ntp.*.com",
];

pub struct Options<'a> {
    pub proxy_mode: &'a str,
    pub log_level: &'a str,
    pub stack: &'a str,
    pub dns_mode: &'a str,
    pub iface: &'a str,
    pub controller: &'a str,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Summary {
    pub proxies: usize,
    pub groups: usize,
    pub rules: usize,
    pub providers: usize,
}

fn key(k: &str) -> Value {
    Value::String(k.into())
}
fn strings(items: &[&str]) -> Value {
    Value::Sequence(items.iter().map(|s| key(s)).collect())
}
fn len(m: &Mapping, k: &str) -> usize {
    match m.get(k) {
        Some(Value::Sequence(s)) => s.len(),
        Some(Value::Mapping(m)) => m.len(),
        _ => 0,
    }
}

/// Parses a subscription and rejects anything that is not a usable Clash profile.
pub fn parse(text: &str) -> Result<Mapping, String> {
    if text.len() > MAX_PROFILE_BYTES {
        return Err("Subscription exceeds 8 MiB".into());
    }
    let mut value: Value = serde_norway::from_str(text).map_err(|e| {
        let at = e
            .location()
            .map(|l| format!(" at line {}", l.line()))
            .unwrap_or_default();
        if text
            .trim_start()
            .starts_with(|c: char| c.is_ascii_alphanumeric())
            && !text.contains(':')
        {
            "Subscription is not a Clash YAML profile (base64 node lists are not supported)".into()
        } else {
            format!("Subscription YAML is invalid{at}")
        }
    })?;
    value
        .apply_merge()
        .map_err(|_| "Subscription YAML has an invalid merge key")?;
    let Value::Mapping(mut map) = value else {
        return Err(
            "Subscription is not a Clash YAML profile (base64 node lists are not supported)".into(),
        );
    };
    // Some providers wrap lists in merge-applied mappings; only the kept sections matter.
    map.retain(|k, _| k.as_str().is_some());
    if len(&map, "proxies") + len(&map, "proxy-providers") == 0 {
        return Err("Subscription contains no proxies or proxy providers".into());
    }
    for k in ["proxies", "proxy-groups", "rules"] {
        if map.get(k).is_some_and(|v| !v.is_sequence() && !v.is_null()) {
            return Err(format!("Subscription {k} must be a list"));
        }
    }
    Ok(map)
}

pub fn summary(map: &Mapping) -> Summary {
    Summary {
        proxies: len(map, "proxies"),
        groups: len(map, "proxy-groups"),
        rules: len(map, "rules"),
        providers: len(map, "proxy-providers") + len(map, "rule-providers"),
    }
}

/// The port after the last colon of a listen address: "127.0.0.1:7874", ":7874", "[::]:53".
fn listen_port(listen: &str) -> Option<u16> {
    listen.rsplit_once(':')?.1.parse().ok()
}
/// The port of a DNS server entry on this device's own loopback (or unspecified)
/// address: "udp://127.0.0.1:7874", "127.0.0.1:7874", "tcp://[::1]:7874". Other hosts,
/// and schemes without a port (system, dhcp, rcode), are left alone.
fn loopback_port(server: &str) -> Option<u16> {
    let server = server.split('#').next()?.trim();
    let (scheme, rest) = server.split_once("://").unwrap_or(("udp", server));
    let default = match scheme {
        "udp" | "tcp" => 53,
        "tls" | "quic" => 853,
        "https" | "h3" => 443,
        _ => return None,
    };
    let authority = rest.split('/').next()?;
    let (host, port) = match authority.strip_prefix('[') {
        Some(v6) => {
            let (host, tail) = v6.split_once(']')?;
            (host, tail.strip_prefix(':'))
        }
        None => match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        },
    };
    let port = port.map_or(Some(default), |p| p.parse().ok())?;
    let local = host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback() || ip.is_unspecified());
    local.then_some(port)
}
/// Drops the entries of a server list (or a single server) that point at `ports` on
/// loopback; false when nothing is left.
fn drop_servers(value: &mut Value, ports: &[u16]) -> bool {
    let stale = |v: &Value| {
        v.as_str()
            .and_then(loopback_port)
            .is_some_and(|p| ports.contains(&p))
    };
    match value {
        Value::Sequence(list) => {
            list.retain(|v| !stale(v));
            !list.is_empty()
        }
        v => !stale(v),
    }
}
fn dns(source: Option<&Value>, o: &Options) -> Value {
    let mut dns = match source {
        Some(Value::Mapping(m)) => m.clone(),
        _ => Mapping::new(),
    };
    // A profile written for a standalone client may point its servers, typically
    // proxy-server-nameserver, at its own DNS listener. That listener is replaced by
    // DNS_LISTEN, and pointing them there instead would hand node names fake IPs: such
    // entries go, and what they resolved falls back to the main resolver.
    let ports: Vec<u16> = [dns.get("listen").and_then(Value::as_str), Some(DNS_LISTEN)]
        .into_iter()
        .flatten()
        .filter_map(listen_port)
        .collect();
    dns.retain(|k, _| k.as_str().is_some_and(|k| !DNS_OWNED.contains(&k)));
    for k in DNS_SERVER_LISTS {
        if dns.get_mut(*k).is_some_and(|v| !drop_servers(v, &ports)) {
            dns.remove(*k);
        }
    }
    for k in DNS_SERVER_POLICIES {
        if let Some(Value::Mapping(policy)) = dns.get_mut(*k) {
            policy.retain(|_, v| drop_servers(v, &ports));
        }
    }
    let empty = |m: &Mapping, k: &str| m.get(k).is_none_or(|v| len_value(v) == 0);
    if empty(&dns, "nameserver") {
        dns.insert(key("nameserver"), strings(DEFAULT_NAMESERVERS));
    }
    if empty(&dns, "default-nameserver") {
        dns.insert(key("default-nameserver"), strings(DEFAULT_NAMESERVERS));
    }
    // The core refuses respect-rules, or a proxy-server-nameserver-policy, without a
    // proxy-server-nameserver: the direct bootstrap servers take that role.
    if empty(&dns, "proxy-server-nameserver")
        && (dns.get("respect-rules").and_then(Value::as_bool) == Some(true)
            || !empty(&dns, "proxy-server-nameserver-policy"))
    {
        let bootstrap = dns["default-nameserver"].clone();
        dns.insert(key("proxy-server-nameserver"), bootstrap);
    }
    if o.dns_mode == "fake-ip" && empty(&dns, "fake-ip-filter") {
        dns.insert(key("fake-ip-filter"), strings(DEFAULT_FAKE_IP_FILTER));
    }
    let mut out = Mapping::new();
    out.insert(key("enable"), Value::Bool(true));
    out.insert(key("listen"), key(DNS_LISTEN));
    out.insert(key("ipv6"), Value::Bool(false));
    out.insert(key("enhanced-mode"), key(o.dns_mode));
    out.insert(key("fake-ip-range"), key(FAKE_IP_RANGE));
    out.extend(dns);
    Value::Mapping(out)
}
fn len_value(v: &Value) -> usize {
    match v {
        Value::Sequence(s) => s.len(),
        Value::Mapping(m) => m.len(),
        Value::Null => 0,
        _ => 1,
    }
}

/// Builds the complete mihomo configuration. Only local, non-routable listeners are
/// opened: a loopback mixed port, a loopback DNS listener and the Unix controller.
pub fn compose(source: &Mapping, o: &Options) -> String {
    let mut out = Mapping::new();
    let mut set = |k: &str, v: Value| {
        out.insert(key(k), v);
    };
    set("mixed-port", Value::Number(MIXED_PORT.into()));
    set("bind-address", key("127.0.0.1"));
    set("allow-lan", Value::Bool(false));
    set("mode", key(o.proxy_mode));
    set("log-level", key(o.log_level));
    set("ipv6", Value::Bool(false));
    set("external-controller-unix", key(o.controller));
    set("find-process-mode", key("off"));
    set("geodata-loader", key("memconservative"));
    set("geo-auto-update", Value::Bool(false));
    let mut ntp = Mapping::new();
    ntp.insert(key("enable"), Value::Bool(true));
    ntp.insert(key("server"), key(NTP_SERVER));
    ntp.insert(key("port"), Value::Number(123.into()));
    ntp.insert(key("interval"), Value::Number(NTP_INTERVAL.into()));
    ntp.insert(key("write-to-system"), Value::Bool(false));
    set("ntp", Value::Mapping(ntp));
    let mut profile = Mapping::new();
    profile.insert(key("store-selected"), Value::Bool(true));
    profile.insert(key("store-fake-ip"), Value::Bool(o.dns_mode == "fake-ip"));
    set("profile", Value::Mapping(profile));
    // The agent owns routing: no auto-route/redirect, and the device name is fixed so
    // the egress policy can point its routing table at it.
    let mut tun = Mapping::new();
    tun.insert(key("enable"), Value::Bool(true));
    tun.insert(key("device"), key(o.iface));
    tun.insert(key("stack"), key(o.stack));
    tun.insert(key("auto-route"), Value::Bool(false));
    tun.insert(key("auto-redirect"), Value::Bool(false));
    tun.insert(key("auto-detect-interface"), Value::Bool(false));
    tun.insert(key("strict-route"), Value::Bool(false));
    tun.insert(key("dns-hijack"), strings(&["any:53", "tcp://any:53"]));
    set("tun", Value::Mapping(tun));
    set("dns", dns(source.get("dns"), o));
    for k in KEEP {
        if let Some(v) = source.get(*k) {
            set(k, v.clone());
        }
    }
    if source.get("rules").is_none() {
        set("rules", strings(&["MATCH,GLOBAL"]));
    }
    serde_norway::to_string(&Value::Mapping(out)).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn options() -> Options<'static> {
        Options {
            proxy_mode: "rule",
            log_level: "warning",
            stack: "system",
            dns_mode: "fake-ip",
            iface: "ou-clash0",
            controller: "/data/local/tmp/openui-clash/home/mihomo.sock",
        }
    }
    const SUB: &str = r#"
port: 7890
socks-port: 7891
allow-lan: true
bind-address: '*'
mode: global
external-controller: 0.0.0.0:9090
secret: hunter2
external-ui: /etc/passwd
interface-name: rmnet_data0
routing-mark: 6666
iptables: {enable: true}
listeners: [{name: lan, type: socks, port: 1080, listen: 0.0.0.0}]
tunnels: [{network: [tcp], address: 0.0.0.0:6553, target: 1.1.1.1:53, proxy: A}]
tun: {enable: true, stack: gvisor, auto-route: true, device: utun}
ntp: {enable: false, server: time.example, write-to-system: true}
dns:
  enable: true
  listen: 0.0.0.0:53
  ipv6: true
  enhanced-mode: redir-host
  nameserver: [https://doh.pub/dns-query]
  fake-ip-filter: ['+.example.cn']
x-base: &base {type: ss, cipher: aes-128-gcm, password: pw, udp: true}
proxies:
  - {<<: *base, name: A, server: 192.0.2.1, port: 443}
  - {<<: *base, name: B, server: 192.0.2.2, port: 443, udp: false}
proxy-groups:
  - {name: PROXY, type: select, proxies: [A, B]}
rules:
  - DOMAIN-SUFFIX,cn,DIRECT
  - MATCH,PROXY
"#;
    fn composed() -> Mapping {
        let text = compose(&parse(SUB).unwrap(), &options());
        serde_norway::from_str(&text).unwrap()
    }
    #[test]
    fn unsafe_subscription_keys_are_replaced_or_dropped() {
        let c = composed();
        for k in [
            "port",
            "socks-port",
            "external-controller",
            "secret",
            "external-ui",
            "interface-name",
            "routing-mark",
            "iptables",
            "listeners",
            "tunnels",
            "x-base",
        ] {
            assert!(c.get(k).is_none(), "{k} survived");
        }
        assert_eq!(c["allow-lan"], Value::Bool(false));
        assert_eq!(c["bind-address"], key("127.0.0.1"));
        assert_eq!(c["mode"], key("rule"));
        assert_eq!(c["mixed-port"], Value::Number(7890.into()));
        assert_eq!(c["find-process-mode"], key("off"));
        let tun = &c["tun"];
        assert_eq!(tun["device"], key("ou-clash0"));
        assert_eq!(tun["stack"], key("system"));
        assert_eq!(tun["auto-route"], Value::Bool(false));
        assert_eq!(tun["auto-redirect"], Value::Bool(false));
    }
    #[test]
    fn core_keeps_its_own_clock_without_touching_the_system() {
        let ntp = &composed()["ntp"];
        assert_eq!(ntp["enable"], Value::Bool(true));
        assert_eq!(ntp["server"], key(NTP_SERVER));
        assert_eq!(ntp["port"], Value::Number(123.into()));
        assert_eq!(ntp["interval"], Value::Number(NTP_INTERVAL.into()));
        assert_eq!(ntp["write-to-system"], Value::Bool(false));
    }
    #[test]
    fn dns_keeps_upstreams_but_not_listeners() {
        let c = composed();
        let dns = &c["dns"];
        assert_eq!(dns["listen"], key(DNS_LISTEN));
        assert_eq!(dns["ipv6"], Value::Bool(false));
        assert_eq!(dns["enhanced-mode"], key("fake-ip"));
        assert_eq!(dns["fake-ip-range"], key(FAKE_IP_RANGE));
        assert_eq!(dns["nameserver"], strings(&["https://doh.pub/dns-query"]));
        assert_eq!(dns["fake-ip-filter"], strings(&["+.example.cn"]));
        assert_eq!(dns["default-nameserver"], strings(DEFAULT_NAMESERVERS));
    }
    /// Issue #1: a profile for a standalone client resolves node names through its own
    /// DNS listener, which the agent replaces.
    #[test]
    fn servers_pointing_at_the_profiles_own_listener_go() {
        let compose_dns = |dns: &str| -> Mapping {
            let map = parse(&format!(
                "dns:\n{dns}\nproxies:\n  - {{name: A, type: socks5, server: a.example, port: 1080}}\n"
            ))
            .unwrap();
            let c: Mapping = serde_norway::from_str(&compose(&map, &options())).unwrap();
            c["dns"].as_mapping().unwrap().clone()
        };
        let dns = compose_dns(
            "  listen: 127.0.0.1:7874\n  nameserver: [tls://119.29.29.29, tls://223.5.5.5]\n  proxy-server-nameserver: [udp://127.0.0.1:7874]",
        );
        assert!(dns.get("proxy-server-nameserver").is_none(), "{dns:?}");
        assert_eq!(
            dns["nameserver"],
            strings(&["tls://119.29.29.29", "tls://223.5.5.5"])
        );
        assert_eq!(dns["listen"], key(DNS_LISTEN));
        // Only the stale entries go; the agent's own port counts too (the issue's workaround).
        let dns = compose_dns(
            "  listen: 0.0.0.0:7874\n  proxy-server-nameserver: ['127.0.0.1:7874', 'tcp://[::1]:7874#DIRECT', 'udp://127.0.0.1:1053', 'tls://1.12.12.12', '127.0.0.1', 'udp://127.0.0.1:5353', 'udp://192.168.0.1:7874']\n  nameserver: [udp://localhost:7874]\n  fallback: ['127.0.0.1:7874']\n  nameserver-policy: {'+.a.example': '127.0.0.1:7874', '+.b.example': ['127.0.0.1:7874', '223.5.5.5']}",
        );
        assert_eq!(
            dns["proxy-server-nameserver"],
            strings(&[
                "tls://1.12.12.12",
                "127.0.0.1",
                "udp://127.0.0.1:5353",
                "udp://192.168.0.1:7874"
            ])
        );
        assert_eq!(dns["nameserver"], strings(DEFAULT_NAMESERVERS));
        assert!(dns.get("fallback").is_none());
        let policy = dns["nameserver-policy"].as_mapping().unwrap();
        assert!(policy.get("+.a.example").is_none());
        assert_eq!(policy["+.b.example"], strings(&["223.5.5.5"]));
        // The core needs a proxy-server-nameserver with respect-rules or a policy for it.
        for extra in [
            "  respect-rules: true",
            "  proxy-server-nameserver-policy: {'+.n.example': 223.5.5.5}",
        ] {
            let dns = compose_dns(&format!(
                "  listen: 127.0.0.1:7874\n  proxy-server-nameserver: [udp://127.0.0.1:7874]\n  default-nameserver: [119.29.29.29]\n{extra}"
            ));
            assert_eq!(
                dns["proxy-server-nameserver"],
                strings(&["119.29.29.29"]),
                "{extra}"
            );
        }
        // Without a listen of its own, a loopback server on port 53 stays.
        let dns = compose_dns("  proxy-server-nameserver: [127.0.0.1]");
        assert_eq!(dns["proxy-server-nameserver"], strings(&["127.0.0.1"]));
    }
    #[test]
    fn anchors_and_merge_keys_expand_in_order() {
        let c = composed();
        let proxies = c["proxies"].as_sequence().unwrap();
        assert_eq!(proxies[0]["name"], key("A"));
        assert_eq!(proxies[0]["cipher"], key("aes-128-gcm"));
        assert_eq!(proxies[0]["udp"], Value::Bool(true));
        assert_eq!(proxies[1]["udp"], Value::Bool(false));
        assert_eq!(c["rules"][1], key("MATCH,PROXY"));
        assert_eq!(
            summary(&parse(SUB).unwrap()),
            Summary {
                proxies: 2,
                groups: 1,
                rules: 2,
                providers: 0
            }
        );
    }
    #[test]
    fn minimal_profile_gets_default_dns_and_rules() {
        let map = parse("proxies:\n  - {name: A, type: socks5, server: 192.0.2.1, port: 1080}\n")
            .unwrap();
        let c: Mapping = serde_norway::from_str(&compose(&map, &options())).unwrap();
        assert_eq!(c["dns"]["nameserver"], strings(DEFAULT_NAMESERVERS));
        assert_eq!(c["dns"]["fake-ip-filter"], strings(DEFAULT_FAKE_IP_FILTER));
        assert_eq!(c["rules"], strings(&["MATCH,GLOBAL"]));
    }
    #[test]
    fn non_profiles_are_rejected() {
        for bad in [
            "c3M6Ly9ZV1Z6TFRFeU9DMW5ZMjA2Y0hjQE1USTNMakF1TUM0eE9qZ3pPRGc9",
            "just text",
            "- a\n- b\n",
            "rules: [MATCH,DIRECT]\n",
            "proxies: {a: 1}\n",
            "proxies: [\n",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
        let err =
            parse("c3M6Ly9ZV1Z6TFRFeU9DMW5ZMjA2Y0hjQE1USTNMakF1TUM0eE9qZ3pPRGc9").unwrap_err();
        assert!(err.contains("base64"), "{err}");
    }
    #[test]
    fn exported_smoke_configuration() {
        // `OPENUI_CLASH_TEST_CONFIG=path cargo test` writes a composed file for `mihomo -t`.
        if let Ok(path) = std::env::var("OPENUI_CLASH_TEST_CONFIG") {
            std::fs::write(path, compose(&parse(SUB).unwrap(), &options())).unwrap();
        }
    }
}
