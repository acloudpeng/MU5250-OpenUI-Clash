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

fn dns(source: Option<&Value>, o: &Options) -> Value {
    let mut dns = match source {
        Some(Value::Mapping(m)) => m.clone(),
        _ => Mapping::new(),
    };
    dns.retain(|k, _| k.as_str().is_some_and(|k| !DNS_OWNED.contains(&k)));
    let empty = |m: &Mapping, k: &str| m.get(k).is_none_or(|v| len_value(v) == 0);
    if empty(&dns, "nameserver") {
        dns.insert(key("nameserver"), strings(DEFAULT_NAMESERVERS));
    }
    if empty(&dns, "default-nameserver") {
        dns.insert(key("default-nameserver"), strings(DEFAULT_NAMESERVERS));
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
