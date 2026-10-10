//! Runs lpac with its stdio drivers. lpac prints one JSON line per card command and per
//! SM-DP+ request and reads the answer from its stdin, so the agent carries both: APDUs
//! to the modem's UIM service, HTTPS through curl. Progress lines report download steps;
//! the `lpa` line carries the result, after which lpac still closes its channel.
//! Neither APDUs nor request bodies are logged: they hold identifiers and activation codes.
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// The card side: a logical channel to an application, and APDUs on it.
pub trait Card {
    fn open(&mut self, aid: &[u8]) -> Result<u8, String>;
    fn transmit(&mut self, channel: u8, apdu: &[u8]) -> Result<Vec<u8>, String>;
    fn close(&mut self, channel: u8);
}

/// The network side: one HTTPS POST, answered with the status code and body.
pub trait Http {
    fn post(
        &mut self,
        url: &str,
        headers: &[String],
        body: &[u8],
    ) -> Result<(u16, Vec<u8>), String>;
}

#[derive(Debug, PartialEq)]
pub enum Failure {
    /// lpac reported an error: the failed step (`es10c_enable_profile`...) and its detail.
    Lpac {
        step: String,
        detail: String,
    },
    /// The modem or card failed underneath lpac; this message beats lpac's own.
    Card(String),
    /// The SM-DP+ request failed before an HTTP status arrived.
    Network(String),
    Timeout,
    /// lpac ended without a result line.
    Crashed(String),
}

/// Longest line read from lpac; profile lists with icons stay far below it.
const MAX_LINE: u64 = 4 * 1024 * 1024;
const STDERR_TAIL: usize = 4096;

pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn unhex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok())
        .collect()
}

fn reply(kind: &str, payload: Value) -> String {
    let mut line = json!({ "type": kind, "payload": payload }).to_string();
    line.push('\n');
    line
}

/// Card state across lpac's requests: the channel lpac opened last, and every channel
/// still open, which are closed if lpac stops without closing them.
struct Channels {
    current: Option<u8>,
    open: Vec<u8>,
}

fn apdu(
    card: &mut dyn Card,
    ch: &mut Channels,
    payload: &Value,
    error: &mut Option<String>,
) -> Value {
    let param = payload["param"].as_str().and_then(unhex);
    let mut fail = |e: String| {
        error.get_or_insert(e);
        json!({ "ecode": -1 })
    };
    match (payload["func"].as_str().unwrap_or(""), param) {
        ("connect" | "disconnect", _) => json!({ "ecode": 0 }),
        ("logic_channel_open", Some(aid)) => match card.open(&aid) {
            Ok(n) => {
                ch.current = Some(n);
                ch.open.push(n);
                json!({ "ecode": n })
            }
            Err(e) => fail(e),
        },
        ("logic_channel_close", Some(n)) if n.len() == 1 => {
            if ch.open.contains(&n[0]) {
                card.close(n[0]);
                ch.open.retain(|&c| c != n[0]);
            }
            if ch.current == Some(n[0]) {
                ch.current = None;
            }
            json!({ "ecode": 0 })
        }
        ("transmit", Some(command)) => match ch.current {
            Some(n) => match card.transmit(n, &command) {
                Ok(response) => json!({ "ecode": 0, "data": hex(&response) }),
                Err(e) => fail(e),
            },
            None => fail("No logical channel is open".into()),
        },
        _ => json!({ "ecode": -1 }),
    }
}

fn http(client: &mut dyn Http, payload: &Value, error: &mut Option<String>) -> Value {
    let url = payload["url"].as_str().unwrap_or("");
    let body = payload["tx"].as_str().and_then(unhex);
    let headers: Option<Vec<String>> = payload["headers"].as_array().map(|list| {
        list.iter()
            .filter_map(|h| h.as_str())
            .filter(|h| !h.contains(['\r', '\n']))
            .map(String::from)
            .collect()
    });
    let result = match (url.starts_with("https://"), body, headers) {
        (true, Some(body), Some(headers)) => client.post(url, &headers, &body),
        _ => Err("lpac sent an invalid SM-DP+ request".into()),
    };
    match result {
        Ok((code, body)) => json!({ "rcode": code, "rx": hex(&body) }),
        Err(e) => {
            error.get_or_insert(e);
            json!({ "rcode": 0, "rx": "" })
        }
    }
}

fn spawn(binary: &Path, args: &[String]) -> Result<Child, Failure> {
    let mut attempt = 0;
    loop {
        let spawned = Command::new(binary)
            .args(args)
            .env_clear()
            .env("LPAC_APDU", "stdio")
            .env("LPAC_HTTP", "stdio")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn();
        match spawned {
            // A binary being replaced, or just written, is briefly busy.
            Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) && attempt < 20 => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(Failure::Crashed(format!("Cannot start lpac: {e}"))),
            Ok(child) => return Ok(child),
        }
    }
}

fn send(stdin: &mut Option<ChildStdin>, line: &str) {
    // lpac may already have exited; its result line or exit status tells why.
    if let Some(pipe) = stdin.as_mut() {
        if pipe
            .write_all(line.as_bytes())
            .and_then(|_| pipe.flush())
            .is_err()
        {
            *stdin = None;
        }
    }
}

fn kill(child: &mut Child) {
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
}

/// Runs `lpac <args>`, carrying its requests until it exits or `timeout` passes.
/// `progress` receives each progress step (`es9p_initiate_authentication`...).
pub fn run(
    binary: &Path,
    args: &[String],
    card: &mut dyn Card,
    client: &mut dyn Http,
    progress: &mut dyn FnMut(&str),
    timeout: Duration,
) -> Result<Value, Failure> {
    let deadline = Instant::now() + timeout;
    let mut child = spawn(binary, args)?;
    let mut stdin = child.stdin.take();
    let stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let (tx, rx) = mpsc::channel::<Option<String>>();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut line = Vec::new();
            match (&mut reader).take(MAX_LINE).read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) if !line.ends_with(b"\n") && line.len() as u64 >= MAX_LINE => break,
                Ok(_) => {
                    if tx
                        .send(Some(String::from_utf8_lossy(&line).into_owned()))
                        .is_err()
                    {
                        return;
                    }
                }
            }
        }
        let _ = tx.send(None);
    });
    let tail = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = (&mut stderr).take(STDERR_TAIL as u64).read_to_end(&mut buf);
        let _ = std::io::copy(&mut stderr, &mut std::io::sink());
        String::from_utf8_lossy(&buf).into_owned()
    });

    let mut channels = Channels {
        current: None,
        open: Vec::new(),
    };
    let (mut card_error, mut http_error) = (None, None);
    let mut result: Option<(i64, String, Value)> = None;
    let mut timed_out = false;
    loop {
        let wait = deadline.saturating_duration_since(Instant::now());
        let line = match rx.recv_timeout(wait) {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(_) => {
                timed_out = true;
                break;
            }
        };
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let payload = &message["payload"];
        match message["type"].as_str() {
            Some("apdu") => {
                let answer = apdu(card, &mut channels, payload, &mut card_error);
                send(&mut stdin, &reply("apdu", answer));
            }
            Some("http") => {
                let answer = http(client, payload, &mut http_error);
                send(&mut stdin, &reply("http", answer));
            }
            Some("progress") => progress(payload["message"].as_str().unwrap_or("")),
            Some("lpa") if result.is_none() => {
                let code = payload["code"].as_i64().unwrap_or(-1);
                let step = payload["message"].as_str().unwrap_or("").to_string();
                result = Some((code, step, payload["data"].clone()));
            }
            _ => {}
        }
    }
    drop(stdin);
    if timed_out {
        kill(&mut child);
    } else {
        // lpac exits right after its last line; bound the wait all the same.
        let until = Instant::now() + Duration::from_secs(5);
        while matches!(child.try_wait(), Ok(None)) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        if matches!(child.try_wait(), Ok(None)) {
            kill(&mut child);
        }
    }
    let status = child.wait();
    let stderr = tail.join().unwrap_or_default();
    for n in channels.open.drain(..) {
        card.close(n);
    }
    if timed_out {
        return Err(Failure::Timeout);
    }
    match result {
        Some((0, _, data)) => Ok(data),
        _ if card_error.is_some() => Err(Failure::Card(card_error.unwrap_or_default())),
        _ if http_error.is_some() => Err(Failure::Network(http_error.unwrap_or_default())),
        Some((_, step, data)) => Err(Failure::Lpac {
            step,
            detail: match data {
                Value::String(s) => s,
                Value::Null => String::new(),
                other => other.to_string(),
            },
        }),
        None => {
            let how = match status {
                Ok(s) => s.to_string(),
                Err(e) => e.to_string(),
            };
            let last = stderr.lines().last().unwrap_or("").trim();
            Err(Failure::Crashed(if last.is_empty() {
                format!("lpac stopped without a result ({how})")
            } else {
                format!("lpac stopped without a result ({how}): {last}")
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[derive(Default)]
    struct FakeCard {
        opened: Vec<Vec<u8>>,
        sent: Vec<(u8, Vec<u8>)>,
        closed: Vec<u8>,
        fail_transmit: bool,
    }
    impl Card for FakeCard {
        fn open(&mut self, aid: &[u8]) -> Result<u8, String> {
            self.opened.push(aid.to_vec());
            Ok(2)
        }
        fn transmit(&mut self, channel: u8, apdu: &[u8]) -> Result<Vec<u8>, String> {
            if self.fail_transmit {
                return Err("The modem did not answer in time".into());
            }
            self.sent.push((channel, apdu.to_vec()));
            Ok(vec![0xBF, 0x3E, 0x90, 0x00])
        }
        fn close(&mut self, channel: u8) {
            self.closed.push(channel);
        }
    }

    #[derive(Default)]
    struct FakeHttp {
        posts: Vec<(String, Vec<String>, Vec<u8>)>,
    }
    impl Http for FakeHttp {
        fn post(
            &mut self,
            url: &str,
            headers: &[String],
            body: &[u8],
        ) -> Result<(u16, Vec<u8>), String> {
            self.posts
                .push((url.into(), headers.to_vec(), body.to_vec()));
            Ok((200, b"{}".to_vec()))
        }
    }

    /// A stand-in for lpac: prints `lines` in order; `<` reads one reply line, which a
    /// following `!` line echoes back as the data of a final result.
    fn script(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "#!/bin/sh\n{body}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn temp() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "openui-lpac-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const OPEN: &str = r#"echo '{"type":"apdu","payload":{"func":"connect","param":null}}'; read a
echo '{"type":"apdu","payload":{"func":"logic_channel_open","param":"a0000005591010ffffffff8900000100"}}'; read b"#;

    #[test]
    fn carries_apdus_and_returns_the_result() {
        let dir = temp();
        let lpac = script(
            &dir,
            "ok",
            &format!(
                r#"{OPEN}
echo '{{"type":"apdu","payload":{{"func":"transmit","param":"82e2910006bf3e035c015a"}}}}'; read c
echo '{{"type":"progress","payload":{{"code":0,"message":"es10c_get_eid","data":null}}}}'
printf '{{"type":"lpa","payload":{{"code":0,"message":"success","data":[%s,%s]}}}}\n' "$b" "$c"
echo '{{"type":"apdu","payload":{{"func":"logic_channel_close","param":"02"}}}}'; read d
echo '{{"type":"apdu","payload":{{"func":"disconnect","param":null}}}}'; read e"#
            ),
        );
        let (mut card, mut net, mut steps) = (FakeCard::default(), FakeHttp::default(), vec![]);
        let data = run(
            &lpac,
            &[],
            &mut card,
            &mut net,
            &mut |s| steps.push(s.to_string()),
            Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(data[0]["payload"]["ecode"], 2);
        assert_eq!(data[1]["payload"]["data"], "bf3e9000");
        assert_eq!(
            card.opened,
            [unhex("a0000005591010ffffffff8900000100").unwrap()]
        );
        assert_eq!(card.sent, [(2, unhex("82e2910006bf3e035c015a").unwrap())]);
        assert_eq!(card.closed, [2]);
        assert_eq!(steps, ["es10c_get_eid"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn card_errors_win_and_leaked_channels_close() {
        let dir = temp();
        let lpac = script(
            &dir,
            "fail",
            &format!(
                r#"{OPEN}
echo '{{"type":"apdu","payload":{{"func":"transmit","param":"80e2"}}}}'; read c
echo '{{"type":"lpa","payload":{{"code":-1,"message":"es10c_get_eid","data":""}}}}'"#
            ),
        );
        let mut card = FakeCard {
            fail_transmit: true,
            ..Default::default()
        };
        let err = run(
            &lpac,
            &[],
            &mut card,
            &mut FakeHttp::default(),
            &mut |_| {},
            Duration::from_secs(10),
        )
        .unwrap_err();
        assert_eq!(
            err,
            Failure::Card("The modem did not answer in time".into())
        );
        assert_eq!(card.closed, [2]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lpac_errors_and_http_requests() {
        let dir = temp();
        let lpac = script(
            &dir,
            "http",
            r#"printf '%s\n' '{"type":"http","payload":{"url":"https://smdp.example/gsma/rsp2/es9plus/initiateAuthentication","tx":"7b7d","headers":["User-Agent: gsma-rsp-lpad","Bad: a\rb"]}}'; read a
echo '{"type":"http","payload":{"url":"http://plain.example/","tx":"","headers":[]}}'; read b
printf '{"type":"lpa","payload":{"code":-1,"message":"es9p_initiate_authentication","data":%s}}\n' "$b""#,
        );
        let mut net = FakeHttp::default();
        let err = run(
            &lpac,
            &[],
            &mut FakeCard::default(),
            &mut net,
            &mut |_| {},
            Duration::from_secs(10),
        )
        .unwrap_err();
        // The plain-HTTP request never reached the network; its rejection is the error.
        assert_eq!(
            err,
            Failure::Network("lpac sent an invalid SM-DP+ request".into())
        );
        assert_eq!(net.posts.len(), 1);
        assert_eq!(
            net.posts[0].0,
            "https://smdp.example/gsma/rsp2/es9plus/initiateAuthentication"
        );
        assert_eq!(net.posts[0].1, ["User-Agent: gsma-rsp-lpad"]);
        assert_eq!(net.posts[0].2, b"{}");

        let lpac = script(
            &dir,
            "refused",
            r#"echo '{"type":"lpa","payload":{"code":-1,"message":"es10c_enable_profile","data":"profileNotInDisabledState"}}'"#,
        );
        let err = run(
            &lpac,
            &[],
            &mut FakeCard::default(),
            &mut net,
            &mut |_| {},
            Duration::from_secs(10),
        )
        .unwrap_err();
        assert_eq!(
            err,
            Failure::Lpac {
                step: "es10c_enable_profile".into(),
                detail: "profileNotInDisabledState".into()
            }
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn timeouts_and_crashes() {
        let dir = temp();
        let lpac = script(&dir, "hang", &format!("{OPEN}\nsleep 30"));
        let mut card = FakeCard::default();
        let started = Instant::now();
        let err = run(
            &lpac,
            &[],
            &mut card,
            &mut FakeHttp::default(),
            &mut |_| {},
            Duration::from_millis(500),
        )
        .unwrap_err();
        assert_eq!(err, Failure::Timeout);
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(card.closed, [2]);

        let lpac = script(&dir, "crash", "echo 'boom' >&2; exit 3");
        match run(
            &lpac,
            &[],
            &mut FakeCard::default(),
            &mut FakeHttp::default(),
            &mut |_| {},
            Duration::from_secs(10),
        ) {
            Err(Failure::Crashed(m)) => assert!(m.contains("boom"), "{m}"),
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn hex_round_trip() {
        assert_eq!(hex(&[0x00, 0xab, 0x10]), "00ab10");
        assert_eq!(unhex("00AB10"), Some(vec![0x00, 0xab, 0x10]));
        assert_eq!(unhex("0"), None);
        assert_eq!(unhex("zz"), None);
        assert_eq!(unhex("é0"), None);
    }
}
