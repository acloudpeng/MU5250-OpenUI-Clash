//! MAC-based LAN egress policy shared by the managed tunnel backends (WireGuard, Clash TUN).
//! Each backend owns a distinct chain prefix, fwmark bit, routing table and device, so
//! policies never change main routes, stock firewall chains, init.d, or UCI.
use crate::{process::BoundedCommand, storage};
use std::{path::Path, process::Command, time::Duration};

pub struct Egress {
    /// User-facing backend name used in error messages.
    pub label: &'static str,
    /// iptables chain prefix; every owned chain is `{prefix}_*`.
    pub prefix: &'static str,
    /// Single fwmark bit, also used as its own mask.
    pub mark: &'static str,
    /// Routing table number, also used as the ip rule priority.
    pub table: &'static str,
    pub iface: &'static str,
    /// Root-only state directory holding the ownership marker.
    pub dir: &'static str,
    pub owner: &'static str,
    /// Source-NAT forwarded traffic leaving the device. A userspace TUN proxy keeps
    /// real LAN sources instead, so its rules and connection view can match them.
    pub masquerade: bool,
    /// Accept input from the device itself. The mihomo system stack loops forwarded
    /// connections back into the kernel through it, and fw3 has no zone for it.
    pub accept_input: bool,
}

pub fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let o = Command::new(program)
        .args(args)
        .bounded_output()
        .map_err(|_| format!("Cannot run {program}"))?;
    if !o.status.success() {
        return Err(format!("{program} failed"));
    }
    Ok(String::from_utf8_lossy(&o.stdout).trim().into())
}

impl Egress {
    fn hooks(&self) -> [(&'static str, &'static str, &'static str, String); 8] {
        let p = self.prefix;
        [
            ("iptables", "mangle", "PREROUTING", format!("{p}_MARK")),
            ("iptables", "nat", "PREROUTING", format!("{p}_DNS")),
            ("iptables", "nat", "POSTROUTING", format!("{p}_NAT")),
            ("iptables", "filter", "FORWARD", format!("{p}_FWD")),
            ("iptables", "mangle", "FORWARD", format!("{p}_MSS")),
            ("ip6tables", "filter", "FORWARD", format!("{p}_V6")),
            ("ip6tables", "filter", "INPUT", format!("{p}_DNS6")),
            ("iptables", "filter", "INPUT", format!("{p}_DNS4")),
        ]
    }
    fn specs(&self, with_hook: bool, order: &[usize]) -> String {
        let hooks = self.hooks();
        order
            .iter()
            .map(|&i| {
                let (bin, table, hook, chain) = &hooks[i];
                if with_hook {
                    format!("'{bin} {table} {hook} {chain}'")
                } else {
                    format!("'{bin} {table} {chain}'")
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
    /// Removes owned firewall hooks, the rule and the table but leaves the device, for
    /// backends whose device belongs to a running process (the Clash TUN).
    pub fn cleanup_policy(&self) -> String {
        self.cleanup_script(false)
    }
    // Literal script is never combined with unvalidated configuration or arbitrary shell hooks.
    pub fn cleanup(&self) -> String {
        self.cleanup_script(true)
    }
    fn cleanup_script(&self, with_link: bool) -> String {
        let (specs, mark, table) = (
            self.specs(true, &[0, 1, 2, 3, 4, 5, 6, 7]),
            self.mark,
            self.table,
        );
        let link = if with_link {
            format!("ip link del {} 2>/dev/null || true\n", self.iface)
        } else {
            String::new()
        };
        format!(
            r#"
for spec in {specs}; do
 set -- $spec
 while "$1" -w 2 -t "$2" -C "$3" -j "$4" 2>/dev/null; do "$1" -w 2 -t "$2" -D "$3" -j "$4" || exit 1; done
 "$1" -w 2 -t "$2" -F "$4" 2>/dev/null || true
 "$1" -w 2 -t "$2" -X "$4" 2>/dev/null || true
done
while ip -4 rule del priority {table} fwmark {mark}/{mark} table {table} 2>/dev/null; do :; done
ip -4 route flush table {table} 2>/dev/null || true
{link}"#
        )
    }
    fn selection(
        &self,
        mode: &str,
        macs: &[String],
        bin: &str,
        table: &str,
        chain: &str,
        action: &str,
    ) -> String {
        let mut out = String::new();
        for mac in macs {
            let target = if mode == "all" { "RETURN" } else { action };
            out.push_str(&format!(
                "{bin} -w 2 -t {table} -A {chain} -m mac --mac-source {mac} -j {target}\n"
            ));
        }
        if mode == "all" {
            out.push_str(&format!("{bin} -w 2 -t {table} -A {chain} -j {action}\n"));
        }
        out
    }
    /// `mode` is `all` (macs are exclusions) or `selected`; `dns` is the IPv4 resolver that
    /// marked clients' port-53 traffic is redirected to. Inputs must already be validated.
    pub fn policy(&self, mode: &str, macs: &[String], dns: &str) -> String {
        let (p, mark, table, iface) = (self.prefix, self.mark, self.table, self.iface);
        let mut out = String::from("set -eu\n");
        // Create unreachable fallback first: marked packets must never fall through to WAN.
        out.push_str(&format!("ip -4 route replace unreachable default metric 32760 table {table}\nip -4 route replace default dev {iface} metric 10 table {table}\nip -4 rule add priority {table} fwmark {mark}/{mark} table {table}\n"));
        out.push_str(&format!(
            "for spec in {}; do set -- $spec; \"$1\" -w 2 -t \"$2\" -N \"$3\"; done\n",
            self.specs(false, &[0, 1, 2, 3, 4, 5, 6, 7])
        ));
        out.push_str(&format!(
            "iptables -w 2 -t mangle -A {p}_MARK ! -i br-lan -j RETURN\n"
        ));
        // Keep router/LAN and broadcast discovery local. Actual connected subnet is discovered, never hardcoded.
        out.push_str(&format!("for net in $(ip -o -4 route show dev br-lan scope link | awk '{{print $1}}'); do\niptables -w 2 -t mangle -A {p}_MARK -d \"$net\" -p tcp ! --dport 53 -j RETURN\niptables -w 2 -t mangle -A {p}_MARK -d \"$net\" -p udp ! --dport 53 -j RETURN\niptables -w 2 -t mangle -A {p}_MARK -d \"$net\" -p icmp -j RETURN\ndone\niptables -w 2 -t mangle -A {p}_MARK -d 224.0.0.0/4 -j RETURN\niptables -w 2 -t mangle -A {p}_MARK -d 255.255.255.255/32 -j RETURN\n"));
        out.push_str(&self.selection(
            mode,
            macs,
            "iptables",
            "mangle",
            &format!("{p}_MARK"),
            &format!("MARK --set-xmark {mark}/{mark}"),
        ));
        for proto in ["udp", "tcp"] {
            out.push_str(&format!("iptables -w 2 -t nat -A {p}_DNS -i br-lan -m mark --mark {mark}/{mark} -p {proto} --dport 53 -j DNAT --to-destination {dns}\n"));
        }
        if self.masquerade {
            out.push_str(&format!("iptables -w 2 -t nat -A {p}_NAT -o {iface} -m mark --mark {mark}/{mark} -j MASQUERADE\n"));
        }
        out.push_str(&format!("iptables -w 2 -A {p}_FWD -i {iface} -o br-lan -m conntrack --ctstate RELATED,ESTABLISHED -j ACCEPT\niptables -w 2 -A {p}_FWD -i br-lan -o {iface} -m mark --mark {mark}/{mark} -j ACCEPT\niptables -w 2 -A {p}_FWD -i br-lan -m mark --mark {mark}/{mark} ! -o br-lan -j REJECT\niptables -w 2 -t mangle -A {p}_MSS -o {iface} -p tcp --tcp-flags SYN,RST SYN -j TCPMSS --clamp-mss-to-pmtu\n"));
        out.push_str(&format!("ip6tables -w 2 -A {p}_V6 ! -i br-lan -j RETURN\nip6tables -w 2 -A {p}_V6 -o br-lan -j RETURN\nip6tables -w 2 -A {p}_DNS6 ! -i br-lan -j RETURN\n"));
        out.push_str(&self.selection(
            mode,
            macs,
            "ip6tables",
            "filter",
            &format!("{p}_V6"),
            "REJECT",
        ));
        // DNS over IPv6 to the router must not escape through the stock resolver.
        for m in macs {
            if mode == "all" {
                out.push_str(&format!(
                    "ip6tables -w 2 -A {p}_DNS6 -m mac --mac-source {m} -j RETURN\n"
                ));
            }
        }
        for proto in ["tcp", "udp"] {
            if mode == "all" {
                out.push_str(&format!(
                    "ip6tables -w 2 -A {p}_DNS6 -p {proto} --dport 53 -j REJECT\n"
                ));
            } else {
                for m in macs {
                    out.push_str(&format!("ip6tables -w 2 -A {p}_DNS6 -m mac --mac-source {m} -p {proto} --dport 53 -j REJECT\n"));
                }
            }
        }
        if self.accept_input {
            out.push_str(&format!("iptables -w 2 -A {p}_DNS4 -i {iface} -j ACCEPT\n"));
        }
        for proto in ["tcp", "udp"] {
            out.push_str(&format!("iptables -w 2 -A {p}_DNS4 -i br-lan -m mark --mark {mark}/{mark} -p {proto} --dport 53 -j REJECT\n"));
        }
        // Attach marking last, after all forwarding and DNS protection is ready.
        out.push_str(&format!(
            "for spec in {}; do set -- $spec; \"$1\" -w 2 -t \"$2\" -I \"$3\" 1 -j \"$4\"; done\n",
            self.specs(true, &[5, 6, 7, 3, 4, 2, 1, 0])
        ));
        out
    }
    // During a policy replacement, briefly pause forwarded LAN traffic and client DNS.
    // This guard remains if the agent is interrupted mid-transaction. LAN management
    // and the router's own WAN connection remain available for recovery.
    pub fn hold(&self) -> String {
        let p = self.prefix;
        format!(
            r#"
set -eu
for bin in iptables ip6tables; do
 $bin -w 2 -N {p}_HOLD 2>/dev/null || true
 $bin -w 2 -C {p}_HOLD -i br-lan ! -o br-lan -j REJECT 2>/dev/null || $bin -w 2 -A {p}_HOLD -i br-lan ! -o br-lan -j REJECT
 $bin -w 2 -C FORWARD -j {p}_HOLD 2>/dev/null || $bin -w 2 -I FORWARD 1 -j {p}_HOLD
 $bin -w 2 -N {p}_HD 2>/dev/null || true
 for proto in udp tcp; do
 $bin -w 2 -C {p}_HD -i br-lan -p $proto --dport 53 -j REJECT 2>/dev/null || $bin -w 2 -A {p}_HD -i br-lan -p $proto --dport 53 -j REJECT
 done
 $bin -w 2 -C INPUT -j {p}_HD 2>/dev/null || $bin -w 2 -I INPUT 1 -j {p}_HD
done
"#
        )
    }
    pub fn release(&self) -> String {
        let p = self.prefix;
        format!(
            r#"
set -eu
for bin in iptables ip6tables; do
 while $bin -w 2 -C FORWARD -j {p}_HOLD 2>/dev/null; do $bin -w 2 -D FORWARD -j {p}_HOLD; done
 while $bin -w 2 -C INPUT -j {p}_HD 2>/dev/null; do $bin -w 2 -D INPUT -j {p}_HD; done
 $bin -w 2 -F {p}_HOLD 2>/dev/null || true
 $bin -w 2 -X {p}_HOLD 2>/dev/null || true
 $bin -w 2 -F {p}_HD 2>/dev/null || true
 $bin -w 2 -X {p}_HD 2>/dev/null || true
done
"#
        )
    }
    pub fn script(&self, text: &str) -> Result<(), String> {
        let mut cmd = Command::new("sh");
        let out = process_runner::output(
            &mut cmd,
            Some(text.as_bytes()),
            Duration::from_secs(90),
            32768,
        )
        .map_err(|_| format!("{} network operation timed out", self.label))?;
        if !out.status.success() {
            // Generated policy contains only validated non-secret network identifiers.
            let msg = String::from_utf8_lossy(&out.stderr);
            return Err(format!(
                "{} network operation failed: {}",
                self.label,
                msg.chars().take(350).collect::<String>()
            ));
        }
        Ok(())
    }
    /// Whether the transition guard is in place. Only meaningful under the backend's lock,
    /// where no transaction can be running: a guard seen then was left by a failed one.
    pub fn held(&self) -> bool {
        run(
            "iptables",
            &[
                "-w",
                "2",
                "-C",
                "FORWARD",
                "-j",
                &format!("{}_HOLD", self.prefix),
            ],
        )
        .is_ok()
    }
    pub fn owned(&self) -> bool {
        Path::new(&format!("{}/owned", self.dir)).exists()
    }
    pub fn claim(&self) -> Result<(), String> {
        if self.owned() {
            return Ok(());
        }
        let (label, mark, table) = (self.label, self.mark, self.table);
        if Path::new(&format!("/sys/class/net/{}", self.iface)).exists()
            || run("ip", &["-4", "rule", "show"]).is_ok_and(|s| {
                s.lines()
                    .any(|l| l.starts_with(&format!("{table}:")) || l.contains(mark))
            })
            || run("ip", &["-4", "route", "show", "table", table]).is_ok_and(|s| !s.is_empty())
        {
            return Err(format!(
                "{label} interface, mark or routing table is already in use"
            ));
        }
        let prefix = format!("{}_", self.prefix);
        for bin in ["iptables", "ip6tables"] {
            for t in ["filter", "nat", "mangle"] {
                if run(bin, &["-t", t, "-S"]).is_ok_and(|s| s.contains(&prefix) || s.contains(mark))
                {
                    return Err(format!(
                        "Reserved {label} firewall resources are already in use"
                    ));
                }
            }
        }
        storage::atomic_write(
            Path::new(&format!("{}/owned", self.dir)),
            format!("{}\n", self.owner).as_bytes(),
        )
        .map_err(|_| format!("Cannot record {label} resource ownership"))
    }
    /// Exits non-zero unless every hook, the rule and both table routes are present.
    pub fn health_check(&self) -> String {
        let (p, mark, table, iface) = (self.prefix, self.mark, self.table, self.iface);
        format!(
            r#"
set -e
for spec in {}; do
 set -- $spec
 "$1" -w 2 -t "$2" -C "$3" -j "$4" 2>/dev/null
done
if iptables -w 2 -C FORWARD -j {p}_HOLD 2>/dev/null; then exit 1; fi
ip -4 rule show | grep -q '^{table}:.*fwmark {mark}/{mark}.*lookup {table}'
ip -4 route show table {table} | grep -q 'default dev {iface}'
ip -4 route show table {table} | grep -q 'unreachable default'
"#,
            self.specs(true, &[0, 1, 2, 3, 5, 7, 6])
        )
    }
    pub fn healthy(&self) -> bool {
        run("sh", &["-c", &self.health_check()]).is_ok()
    }
}
