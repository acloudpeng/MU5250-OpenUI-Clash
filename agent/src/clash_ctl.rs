//! Minimal client for mihomo's REST controller on its Unix socket. HTTP/1.0 keeps
//! responses unchunked and delimited by close; the socket lives in a root-only directory.
use serde_json::Value;
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    time::Duration,
};

const RESPONSE_LIMIT: u64 = 4 * 1024 * 1024;

/// Percent-encodes one path segment (group and proxy names are arbitrary UTF-8).
pub fn segment(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn parse(raw: &[u8]) -> Result<(u16, Value), String> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("Clash controller returned an invalid response")?;
    let head = String::from_utf8_lossy(&raw[..split]);
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1)?.parse().ok())
        .ok_or("Clash controller returned an invalid status")?;
    let body = &raw[split + 4..];
    let value = if body.iter().all(u8::is_ascii_whitespace) {
        Value::Null
    } else {
        serde_json::from_slice(body).map_err(|_| "Clash controller returned invalid JSON")?
    };
    Ok((status, value))
}

pub fn request(
    socket: &Path,
    method: &str,
    path: &str,
    body: Option<&Value>,
    timeout: Duration,
) -> Result<(u16, Value), String> {
    let mut stream = UnixStream::connect(socket).map_err(|_| "Clash core is not running")?;
    stream
        .set_read_timeout(Some(timeout))
        .and_then(|_| stream.set_write_timeout(Some(timeout)))
        .map_err(|_| "Cannot use Clash controller")?;
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    let request = format!(
        "{method} {path} HTTP/1.0\r\nHost: mihomo\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|_| "Cannot reach Clash controller")?;
    let mut raw = Vec::new();
    stream
        .take(RESPONSE_LIMIT + 1)
        .read_to_end(&mut raw)
        .map_err(|_| "Clash controller did not answer in time")?;
    if raw.len() as u64 > RESPONSE_LIMIT {
        return Err("Clash controller response is too large".into());
    }
    parse(&raw)
}

/// Like `request`, but any non-2xx status becomes the controller's `message`.
pub fn call(
    socket: &Path,
    method: &str,
    path: &str,
    body: Option<&Value>,
    timeout: Duration,
) -> Result<Value, String> {
    let (status, value) = request(socket, method, path, body, timeout)?;
    if !(200..300).contains(&status) {
        let message = value
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("request failed");
        return Err(format!(
            "Clash core: {}",
            message.chars().take(200).collect::<String>()
        ));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::os::unix::net::UnixListener;
    #[test]
    fn segments_are_percent_encoded() {
        assert_eq!(
            segment("🇭🇰 HK-01/a"),
            "%F0%9F%87%AD%F0%9F%87%B0%20HK-01%2Fa"
        );
        assert_eq!(segment("GLOBAL"), "GLOBAL");
    }
    #[test]
    fn responses_parse_with_and_without_body() {
        assert_eq!(
            parse(b"HTTP/1.0 204 No Content\r\n\r\n").unwrap(),
            (204, Value::Null)
        );
        assert_eq!(
            parse(b"HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n{\"delay\":120}\n")
                .unwrap(),
            (200, json!({"delay":120}))
        );
        assert!(parse(b"garbage").is_err());
        assert!(parse(b"HTTP/1.0 200 OK\r\n\r\n{").is_err());
    }
    #[test]
    fn round_trip_over_unix_socket() {
        let dir = std::env::temp_dir().join(format!("openui-clash-ctl-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let sock = dir.join("c.sock");
        let _ = std::fs::remove_file(&sock);
        let listener = UnixListener::bind(&sock).unwrap();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut buf = [0u8; 1024];
            let n = s.read(&mut buf).unwrap();
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            s.write_all(b"HTTP/1.0 400 Bad Request\r\n\r\n{\"message\":\"Selector update error: proxy not exist\"}").unwrap();
            req
        });
        let err = call(
            &sock,
            "PUT",
            "/proxies/PROXY",
            Some(&json!({"name":"B"})),
            Duration::from_secs(2),
        )
        .unwrap_err();
        let req = server.join().unwrap();
        assert!(req.starts_with("PUT /proxies/PROXY HTTP/1.0\r\n"), "{req}");
        assert!(req.ends_with("\r\n\r\n{\"name\":\"B\"}"), "{req}");
        assert_eq!(err, "Clash core: Selector update error: proxy not exist");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
