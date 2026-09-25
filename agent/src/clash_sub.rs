//! Subscription download through the device's curl. The URL is a credential: it is
//! passed on stdin (never argv or logs) and only a masked form leaves the agent.
use crate::clash_config::MAX_PROFILE_BYTES;
use serde::{Deserialize, Serialize};
use std::{fs, path::Path, process::Command, time::Duration};

pub const DEFAULT_USER_AGENT: &str = "clash.meta";
const CURL: &str = "/usr/bin/curl";

#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub upload: u64,
    pub download: u64,
    pub total: u64,
    /// Unix seconds; 0 when the provider sends none.
    pub expire: u64,
}

pub struct Download {
    pub body: String,
    pub name: Option<String>,
    pub interval_hours: Option<u32>,
    pub usage: Option<Usage>,
}

pub fn valid_url(url: &str) -> bool {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"));
    url.len() <= 2048
        && rest.is_some_and(|r| {
            !r.is_empty() && !r.starts_with('/') && !r.starts_with('@') && !r.starts_with(':')
        })
        && url.bytes().all(|b| b.is_ascii_graphic())
}
pub fn valid_user_agent(ua: &str) -> bool {
    !ua.is_empty()
        && ua.len() <= 128
        && ua
            .bytes()
            .all(|b| (0x20..0x7f).contains(&b) && b != b'"' && b != b'\\')
}
/// Scheme and host only, e.g. `https://sub.example.com/…`.
pub fn mask(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit('@').next().unwrap_or("");
    format!("{scheme}://{host}/…")
}

fn quoted(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}
fn curl_config(url: &str, user_agent: &str, proxy: Option<&str>) -> String {
    let mut c = format!(
        "url = {}\nuser-agent = {}\n",
        quoted(url),
        quoted(user_agent)
    );
    if let Some(p) = proxy {
        c.push_str(&format!("proxy = {}\n", quoted(p)));
    }
    c
}

/// Status and headers of the final response after redirects.
fn parse_headers(text: &str) -> (u16, Vec<(String, String)>) {
    let block = text
        .split("\r\n\r\n")
        .map(str::trim)
        .filter(|b| b.starts_with("HTTP/"))
        .last()
        .unwrap_or("");
    let mut lines = block.lines();
    let status = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
        .unwrap_or(0);
    let headers = lines
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            Some((k.trim().to_ascii_lowercase(), v.trim().to_string()))
        })
        .collect();
    (status, headers)
}
fn usage(value: &str) -> Option<Usage> {
    let mut u = Usage::default();
    let mut any = false;
    for part in value.split(';') {
        let Some((k, v)) = part.split_once('=') else {
            continue;
        };
        // Some providers send floats or empty values; treat those as unknown.
        let Ok(n) = v.trim().parse::<f64>().map(|f| f.max(0.0) as u64) else {
            continue;
        };
        match k.trim() {
            "upload" => u.upload = n,
            "download" => u.download = n,
            "total" => u.total = n,
            "expire" => u.expire = n,
            _ => continue,
        }
        any = true;
    }
    any.then_some(u)
}
fn percent_decode(s: &str) -> Option<String> {
    let mut out = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            out.push(u8::from_str_radix(s.get(i + 1..i + 3)?, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}
fn filename(value: &str) -> Option<String> {
    let params: Vec<(&str, &str)> = value
        .split(';')
        .filter_map(|p| {
            let (k, v) = p.split_once('=')?;
            Some((k.trim(), v.trim()))
        })
        .collect();
    let name = params
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("filename*"))
        .and_then(|(_, v)| percent_decode(v.split_once("''")?.1))
        .or_else(|| {
            params
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("filename"))
                .map(|(_, v)| v.trim_matches('"').to_string())
        })?;
    let name = name
        .trim_end_matches(".yaml")
        .trim_end_matches(".yml")
        .trim();
    let name: String = name.chars().filter(|c| !c.is_control()).take(48).collect();
    (!name.is_empty()).then_some(name)
}

/// Downloads a profile. `scratch` receives response headers and is removed afterwards.
pub fn fetch(
    url: &str,
    user_agent: &str,
    proxy: Option<&str>,
    scratch: &Path,
) -> Result<Download, String> {
    if !valid_url(url) || !valid_user_agent(user_agent) {
        return Err("Invalid subscription URL or user agent".into());
    }
    if !Path::new(CURL).exists() {
        return Err("curl is unavailable on this device".into());
    }
    let _ = fs::remove_file(scratch);
    let mut cmd = Command::new(CURL);
    cmd.args([
        "-sS",
        "-L",
        "--max-redirs",
        "5",
        "--proto",
        "=http,https",
        "--proto-redir",
        "=http,https",
        "--connect-timeout",
        "10",
        "-m",
        "30",
        "--max-filesize",
        &MAX_PROFILE_BYTES.to_string(),
        "-D",
    ])
    .arg(scratch)
    .args(["-K", "-"]);
    let config = curl_config(url, user_agent, proxy);
    let out = process_runner::output(
        &mut cmd,
        Some(config.as_bytes()),
        Duration::from_secs(35),
        MAX_PROFILE_BYTES,
    );
    let headers = fs::read_to_string(scratch).unwrap_or_default();
    let _ = fs::remove_file(scratch);
    let out = out.map_err(|e| {
        if e.kind() == std::io::ErrorKind::TimedOut {
            "Subscription download timed out".to_string()
        } else {
            "Subscription download failed or exceeded 8 MiB".to_string()
        }
    })?;
    if !out.status.success() {
        // curl's own message names the failure (DNS, TLS, size) but never the URL path.
        let code = out.status.code().unwrap_or(-1);
        let reason = match code {
            6 => "cannot resolve host",
            7 => "cannot connect",
            28 => "timed out",
            35 | 60 => "TLS handshake or certificate failed",
            47 => "too many redirects",
            63 => "profile exceeds 8 MiB",
            _ => "network error",
        };
        return Err(format!(
            "Subscription download failed: {reason} (curl {code})"
        ));
    }
    let (status, headers) = parse_headers(&headers);
    if status != 200 {
        return Err(format!("Subscription server returned HTTP {status}"));
    }
    let header = |name: &str| {
        headers
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    let body = String::from_utf8(out.stdout).map_err(|_| "Subscription is not UTF-8 text")?;
    Ok(Download {
        body: body.trim_start_matches('\u{feff}').to_string(),
        name: header("content-disposition").and_then(filename),
        interval_hours: header("profile-update-interval")
            .and_then(|v| v.trim().parse::<u32>().ok())
            .filter(|h| (1..=24 * 30).contains(h)),
        usage: header("subscription-userinfo").and_then(usage),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn urls_are_validated_and_masked() {
        let url = "https://user:pw@sub.example.com:8443/api/v1/client/subscribe?token=SECRET";
        assert!(valid_url(url));
        assert_eq!(mask(url), "https://sub.example.com:8443/…");
        assert!(!mask(url).contains("SECRET"));
        for bad in [
            "ftp://x",
            "https://",
            "https:///path",
            "https://a b",
            "file:///etc/passwd",
            "https://x\n",
        ] {
            assert!(!valid_url(bad), "{bad}");
        }
        assert!(
            !valid_user_agent("a\"b") && !valid_user_agent("") && valid_user_agent("clash.meta")
        );
    }
    #[test]
    fn curl_config_escapes_quotes_and_backslashes() {
        let c = curl_config(
            "https://x/?a=\"b\\",
            "clash.meta",
            Some("http://127.0.0.1:7890"),
        );
        assert_eq!(
            c,
            "url = \"https://x/?a=\\\"b\\\\\"\nuser-agent = \"clash.meta\"\nproxy = \"http://127.0.0.1:7890\"\n"
        );
    }
    #[test]
    fn final_response_headers_after_redirect() {
        let raw = "HTTP/1.1 302 Found\r\nLocation: https://b/\r\nsubscription-userinfo: upload=9\r\n\r\nHTTP/2 200\r\ncontent-type: text/yaml\r\nSubscription-Userinfo: upload=1024; download=2048.0; total=10737418240; expire=1767225600\r\nprofile-update-interval: 12\r\ncontent-disposition: attachment; filename*=UTF-8''%E6%88%91%E7%9A%84%E6%9C%BA%E5%9C%BA.yaml; filename=\"sub.yaml\"\r\n\r\n";
        let (status, headers) = parse_headers(raw);
        assert_eq!(status, 200);
        let get = |n: &str| {
            headers
                .iter()
                .find(|(k, _)| k == n)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(
            get("subscription-userinfo").and_then(usage),
            Some(Usage {
                upload: 1024,
                download: 2048,
                total: 10737418240,
                expire: 1767225600
            })
        );
        assert_eq!(
            get("content-disposition").and_then(filename).as_deref(),
            Some("我的机场")
        );
        assert_eq!(
            filename("attachment; filename=\"Provider.yml\"").as_deref(),
            Some("Provider")
        );
        assert_eq!(usage("garbage"), None);
    }
}
