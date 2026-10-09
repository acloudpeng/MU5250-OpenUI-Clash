//! Clash core and geo data updates: the official MetaCubeX release, an uploaded build, a
//! one-step rollback, and the meta-rules-dat geo databases. The deployer installs the
//! pinned FlClash build; this replaces it on request. Downloads and checks run on a job
//! thread (the HTTP server has few workers) and the page polls `get`. A new core or geo
//! data set is checked against GitHub's published digest and test-run on the current
//! profile before it is swapped in under the deployment lock and the Clash lock; with Clash
//! on, the core is started on it (restarted if it runs), and one that does not come back
//! gets the previous files again.
use crate::{clash, storage, wireguard};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    os::unix::fs::{symlink, DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MIB: u64 = 1024 * 1024;
const CURL: &str = "/usr/bin/curl";
/// The partition holding the core, the geo data and the agent's state.
const DATA: &str = "/data";
const BIN: &str = "/data/bin";
/// The core that was live before the last install or rollback.
const PREVIOUS: &str = "/data/bin/mihomo.previous";
const UPLOAD_PART: &str = "/data/bin/.mihomo.upload.part";
const DOWNLOAD_PART: &str = "/data/bin/.mihomo.download.part";
/// A decompressed and checked core waiting to replace the live one.
const STAGED: &str = "/data/bin/.mihomo.update";
/// Hard link that becomes `PREVIOUS` in one rename, so the old one is replaced atomically.
const PREVIOUS_NEW: &str = "/data/bin/.mihomo.previous.new";
/// The older previous core while a new core is unconfirmed: put back when the new one
/// fails to start (or at start, when the install was cut off), removed once it runs.
const PREVIOUS_OLD: &str = "/data/bin/.mihomo.previous.old";
/// The live core during a rollback exchange; at start, finishes an interrupted one.
const SWAP: &str = "/data/bin/.mihomo.swap";
/// Clash's root-only state directory.
const STATE_DIR: &str = "/data/local/tmp/openui-clash";
const STATE: &str = "/data/local/tmp/openui-clash/core.json";
const RELEASE_JSON: &str = "/data/local/tmp/openui-clash/.release.json";
const GEO_NEW: &str = "/data/local/tmp/openui-clash/geo.new";
const GEO_NEW_HOME: &str = "/data/local/tmp/openui-clash/geo.new-home";
const GEO_PREVIOUS: &str = "/data/local/tmp/openui-clash/geo.previous";
/// Written by scripts/deploy-components.py; its "mihomo" hash is the pinned build.
const MANIFEST: &str = "/data/open-u60-manifest.json";
/// The deployer's transaction lock (scripts/device/transaction.sh). While it or an
/// unfinished transaction exists, a deployment owns /data/bin and /data/share/mihomo.
const TRANSACTIONS: &str = "/data/local/tmp/open-u60-transactions";
const DEPLOY_LOCK: &str = "/data/local/tmp/open-u60-transactions/lock";
const DEPLOY_ACTIVE: &str = "/data/local/tmp/open-u60-transactions/active";
const LOCK_OWNER: &str = "agent-core-update";

const RELEASE_API: &str = "https://api.github.com/repos/MetaCubeX/mihomo/releases/latest";
const RELEASE_FILES: &str = "https://github.com/MetaCubeX/mihomo/releases/download/";
const API_HEADERS: [&str; 2] = [
    "Accept: application/vnd.github+json",
    "X-GitHub-Api-Version: 2022-11-28",
];
/// Rebuilt daily without versioned tags; each file has a `.sha256sum` beside it.
const GEO_BASE: &str = "https://github.com/MetaCubeX/meta-rules-dat/releases/download/latest/";
/// Upstream name and the name the core looks for (scripts/prepare-geodata.py).
const GEO_UPSTREAM: [(&str, &str); 2] = [
    ("geoip.metadb", "geoip.metadb"),
    ("geosite.dat", "GeoSite.dat"),
];
const MMDB_MARKER: &[u8] = b"\xab\xcd\xefMaxMind.com";
const EM_AARCH64: u16 = 183;
const MAX_UPLOAD: u64 = 64 * MIB;
const MAX_CORE: u64 = 160 * MIB;
const MAX_GEO: u64 = 32 * MIB;
/// Left free on /data beyond everything a job writes.
const MARGIN: u64 = 16 * MIB;
/// The unauthenticated GitHub API allows 60 requests an hour per address, which a
/// carrier NAT or a proxy node shares; a recent check is reused instead.
const CHECK_CACHE: u64 = 600;
const SOURCES: [&str; 4] = ["pinned", "official", "uploaded", "unknown"];

const BUSY: &str = "A core job is already running";
const DEPLOYING: &str = "A deployment is in progress";
const INVALID: &str = "Invalid core request";
const BAD_MIRROR: &str = "Mirror must be an https:// address prefix";
const NO_SPACE: &str = "Not enough free space on /data";
const TOO_LARGE: &str = "Upload exceeds 64 MiB";
const EMPTY: &str = "Upload is empty";
const INTERRUPTED: &str = "Upload was interrupted";
const SAVE_FAILED: &str = "Cannot save the upload";
const CANNOT_INSTALL: &str = "Cannot install the core";
const CANNOT_INSTALL_GEO: &str = "Cannot install geo data";

static JOB: Mutex<Option<Job>> = Mutex::new(None);
static STATE_LOCK: Mutex<()> = Mutex::new(());
/// Core identities by file, so a status poll does not hash a 55 MiB core every time.
static IDENTITIES: Mutex<Vec<(FileKey, Identity)>> = Mutex::new(Vec::new());

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

// ---------- State ----------

/// A core as installed: what it reports, its file hash and where it came from.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Record {
    version: String,
    sha256: String,
    source: String,
    installed_at: u64,
}
/// The last release check.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Latest {
    tag: String,
    published_at: String,
    checked_at: u64,
    asset: String,
    size: u64,
    digest: String,
    url: String,
}
/// core.json, beside the Clash settings (which reject unknown fields).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct State {
    current: Option<Record>,
    previous: Option<Record>,
    latest: Option<Latest>,
}

fn make_dir(dir: &Path) -> Result<(), String> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|_| "Cannot create Clash state directory".into())
}
fn load_state() -> State {
    fs::read(STATE)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}
/// Read-modify-write of core.json. Best effort, like the subscription metadata: it only
/// labels files that are hashed again on every read.
fn update_state(change: impl FnOnce(&mut State)) {
    update_state_if(|state| {
        change(state);
        true
    });
}
/// `update_state` for a change decided under the lock: writes only when `change` says so.
fn update_state_if(change: impl FnOnce(&mut State) -> bool) {
    let _guard = STATE_LOCK.lock().unwrap();
    let mut state = load_state();
    if change(&mut state) {
        let _ = make_dir(Path::new(STATE_DIR));
        let _ = storage::atomic_write(Path::new(STATE), &serde_json::to_vec(&state).unwrap());
    }
}

/// What identifies a file's content without reading it. A hard link or a rename keeps it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileKey {
    dev: u64,
    ino: u64,
    size: u64,
    mtime: i64,
    mtime_nsec: i64,
}
impl FileKey {
    fn of(m: &fs::Metadata) -> Self {
        Self {
            dev: m.dev(),
            ino: m.ino(),
            size: m.len(),
            mtime: m.mtime(),
            mtime_nsec: m.mtime_nsec(),
        }
    }
    fn mtime_secs(&self) -> u64 {
        u64::try_from(self.mtime).unwrap_or(0)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Identity {
    sha256: String,
    version: String,
}

fn sha256_reader(reader: &mut impl Read) -> io::Result<String> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
fn sha256_file(path: &Path) -> io::Result<String> {
    sha256_reader(&mut File::open(path)?)
}
fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}
fn file_len(path: &Path) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// The version from `mihomo -v` ("Mihomo Meta v1.19.32 linux arm64 with go1.24 ..."), or
/// its whole first line when that has another form.
fn parse_version(stdout: &str) -> String {
    let line = stdout.lines().next().unwrap_or("").trim();
    match line.strip_prefix("Mihomo Meta ") {
        Some(rest) => rest.split_whitespace().next().unwrap_or(""),
        None => line,
    }
    .chars()
    .take(64)
    .collect()
}
fn check_version(stdout: &str, tag: Option<&str>) -> Result<String, String> {
    if !stdout.contains("Mihomo") {
        return Err("Core is not a mihomo build".into());
    }
    if tag.is_some_and(|t| !stdout.contains(t)) {
        return Err("Core version does not match the release".into());
    }
    Ok(parse_version(stdout))
}
fn run_version(binary: &Path) -> Result<String, String> {
    let mut cmd = Command::new(binary);
    cmd.arg("-v");
    match process_runner::output(&mut cmd, None, Duration::from_secs(15), 64 * 1024) {
        Ok(out) if out.status.success() => Ok(String::from_utf8_lossy(&out.stdout).into_owned()),
        _ => Err("Core does not run on this device".into()),
    }
}
fn recorded<'a>(state: &'a State, sha256: &str) -> Option<&'a Record> {
    [&state.current, &state.previous]
        .into_iter()
        .flatten()
        .find(|r| r.sha256 == sha256)
}
/// sha256 and version of a core file, cached until the file changes. The version comes
/// from core.json when it recorded the same file, else from running `-v` once.
fn identity(path: &Path, state: &State) -> Option<(Identity, FileKey)> {
    let mut file = File::open(path).ok()?;
    let meta = file.metadata().ok().filter(|m| m.is_file())?;
    let key = FileKey::of(&meta);
    let cached = IDENTITIES
        .lock()
        .unwrap()
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, id)| id.clone());
    if let Some(id) = cached {
        return Some((id, key));
    }
    let sha256 = sha256_reader(&mut file).ok()?;
    let version = recorded(state, &sha256)
        .map(|r| r.version.clone())
        .filter(|v| !v.is_empty())
        .or_else(|| run_version(path).ok().map(|out| parse_version(&out)))
        .unwrap_or_default();
    let id = Identity { sha256, version };
    remember(key, &id);
    Some((id, key))
}
fn remember(key: FileKey, id: &Identity) {
    let mut cache = IDENTITIES.lock().unwrap();
    cache.retain(|(k, _)| *k != key);
    if cache.len() >= 4 {
        cache.remove(0);
    }
    cache.push((key, id.clone()));
}
/// The core hash the last deployment recorded in its manifest.
fn manifest_core(bytes: &[u8]) -> Option<String> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    v["files"]["mihomo"]
        .as_str()
        .filter(|h| is_hex64(h))
        .map(str::to_ascii_lowercase)
}
fn pinned_hash() -> Option<String> {
    manifest_core(&fs::read(MANIFEST).ok()?)
}
/// "pinned" for the deployed build, else what core.json recorded for the hash.
fn source(sha256: &str, pinned: Option<&str>, state: &State) -> String {
    if pinned == Some(sha256) {
        return "pinned".into();
    }
    recorded(state, sha256)
        .map(|r| r.source.as_str())
        .filter(|s| SOURCES.contains(s))
        .unwrap_or("unknown")
        .into()
}
fn record(id: &Identity, source: String, installed_at: u64) -> Record {
    Record {
        version: id.version.clone(),
        sha256: id.sha256.clone(),
        source,
        installed_at,
    }
}
/// When a core was installed: as recorded, else when its file was written.
fn installed_at(state: &State, id: &Identity, key: FileKey) -> u64 {
    recorded(state, &id.sha256).map_or(key.mtime_secs(), |r| r.installed_at)
}

// ---------- Job ----------

#[derive(Clone, Debug, Serialize)]
struct Job {
    kind: &'static str,
    state: &'static str,
    step: &'static str,
    bytes: u64,
    total: u64,
    error: Option<String>,
    started_at: u64,
    finished_at: Option<u64>,
}
/// Takes the single job slot; the record of a finished job is replaced.
fn claim(slot: &Mutex<Option<Job>>, kind: &'static str, step: &'static str) -> Result<(), String> {
    let mut job = slot.lock().unwrap();
    if job.as_ref().is_some_and(|j| j.state == "running") {
        return Err(BUSY.into());
    }
    *job = Some(Job {
        kind,
        state: "running",
        step,
        bytes: 0,
        total: 0,
        error: None,
        started_at: now(),
        finished_at: None,
    });
    Ok(())
}
fn finish(slot: &Mutex<Option<Job>>, result: &Result<(), String>) {
    let mut job = slot.lock().unwrap();
    if let Some(j) = job.as_mut().filter(|j| j.state == "running") {
        j.state = if result.is_ok() { "done" } else { "failed" };
        j.error = result.as_ref().err().cloned();
        j.finished_at = Some(now());
    }
}
fn job_running() -> bool {
    JOB.lock()
        .unwrap()
        .as_ref()
        .is_some_and(|j| j.state == "running")
}
fn step(name: &'static str, total: u64) {
    if let Some(j) = JOB.lock().unwrap().as_mut() {
        j.step = name;
        j.bytes = 0;
        j.total = total;
    }
}
fn progress(bytes: u64) {
    if let Some(j) = JOB.lock().unwrap().as_mut() {
        j.bytes = bytes;
    }
}
/// Runs a claimed job on its own thread; a panic still ends it, so the slot never sticks.
fn spawn(work: impl FnOnce() -> Result<(), String> + Send + 'static) {
    std::thread::spawn(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
            .unwrap_or_else(|_| Err("Core job stopped unexpectedly".into()));
        finish(&JOB, &result);
    });
}
/// GitHub could not be reached for a file: the page may suggest a mirror.
fn mirror_hint(job: Option<&Job>) -> bool {
    job.is_some_and(|j| {
        j.state == "failed"
            && matches!(j.kind, "update" | "geodata")
            && j.error
                .as_deref()
                .is_some_and(|e| e.starts_with("Download failed:") || e == "Download timed out")
    })
}

// ---------- Downloads ----------

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Route {
    /// Through the running core, else directly.
    #[default]
    Auto,
    Proxy,
    Direct,
    /// The large file from a mirror prefix; release data and checksums still from GitHub.
    Mirror,
}
fn proxy(route: Route) -> Result<Option<&'static str>, String> {
    match route {
        Route::Auto => Ok(clash::core_running().then_some(clash::PROXY)),
        Route::Proxy if clash::core_running() => Ok(Some(clash::PROXY)),
        Route::Proxy => Err("Clash is not running".into()),
        Route::Direct | Route::Mirror => Ok(None),
    }
}
/// A mirror is an https:// prefix put before the GitHub address, e.g. "https://ghfast.top/".
/// It only carries files whose digest comes from GitHub itself.
fn mirror_prefix(m: &str) -> Result<String, String> {
    let host = m
        .strip_prefix("https://")
        .and_then(|rest| rest.split('/').next())
        .unwrap_or("");
    let valid = m.len() <= 200
        && !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':'))
        && m.bytes()
            .all(|b| b.is_ascii_graphic() && !matches!(b, b'"' | b'\\' | b'?' | b'#' | b'@'));
    if !valid {
        return Err(BAD_MIRROR.into());
    }
    Ok(if m.ends_with('/') {
        m.into()
    } else {
        format!("{m}/")
    })
}
/// How a job reaches GitHub, and the mirror for its large file if any.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Via {
    github: Route,
    mirror: Option<String>,
}
impl Via {
    fn new(route: Option<Route>, mirror: Option<String>) -> Result<Self, String> {
        let mirror = mirror.map(|m| mirror_prefix(&m)).transpose()?;
        let route = route.unwrap_or(if mirror.is_some() {
            Route::Mirror
        } else {
            Route::Auto
        });
        match (route, mirror) {
            (Route::Mirror, Some(m)) => Ok(Self {
                github: Route::Auto,
                mirror: Some(m),
            }),
            (Route::Mirror, None) => Err(BAD_MIRROR.into()),
            (_, Some(_)) => Err(INVALID.into()),
            (github, None) => Ok(Self {
                github,
                mirror: None,
            }),
        }
    }
    fn proxy(&self) -> Result<Option<&'static str>, String> {
        proxy(self.github)
    }
    /// Address and proxy for a large file on GitHub.
    fn file(&self, url: &str) -> Result<(String, Option<&'static str>), String> {
        match &self.mirror {
            Some(prefix) => Ok((format!("{prefix}{url}"), None)),
            None => Ok((url.into(), self.proxy()?)),
        }
    }
}

fn quoted(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}
/// curl options for stdin (`-K -`), so addresses stay off the command line.
fn curl_config(url: &str, proxy: Option<&str>, headers: &[&str]) -> String {
    let mut c = format!("url = {}\nuser-agent = \"openui-agent\"\n", quoted(url));
    if let Some(p) = proxy {
        c.push_str(&format!("proxy = {}\n", quoted(p)));
    }
    for h in headers {
        c.push_str(&format!("header = {}\n", quoted(h)));
    }
    c
}
fn curl_reason(code: i32) -> &'static str {
    match code {
        5 => "cannot resolve proxy",
        6 => "cannot resolve host",
        7 => "cannot connect",
        28 => "timed out",
        35 | 60 => "TLS handshake or certificate failed",
        47 => "too many redirects",
        63 => "file is too large",
        _ => "network error",
    }
}
/// One HTTPS transfer to `dest`. Anything but a final 200 is an error.
fn curl(
    url: &str,
    proxy: Option<&str>,
    headers: &[&str],
    dest: &Path,
    limit: u64,
    secs: u64,
) -> Result<(), String> {
    if !Path::new(CURL).exists() {
        return Err("curl is unavailable on this device".into());
    }
    let mut cmd = Command::new(CURL);
    cmd.args(["-sS", "-L", "--max-redirs", "5", "--proto", "=https"])
        .args(["--proto-redir", "=https", "--connect-timeout", "15", "-m"])
        .arg(secs.to_string())
        .arg("--max-filesize")
        .arg(limit.to_string())
        .args(["-w", "%{http_code}", "-o"])
        .arg(dest)
        .args(["-K", "-"]);
    let config = curl_config(url, proxy, headers);
    let out = process_runner::output(
        &mut cmd,
        Some(config.as_bytes()),
        Duration::from_secs(secs + 10),
        64 * 1024,
    )
    .map_err(|e| {
        if e.kind() == io::ErrorKind::TimedOut {
            "Download timed out"
        } else {
            "Download failed: network error"
        }
    })?;
    if !out.status.success() {
        let code = out.status.code().unwrap_or(-1);
        return Err(format!(
            "Download failed: {} (curl {code})",
            curl_reason(code)
        ));
    }
    let status: u16 = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or(0);
    if status != 200 {
        return Err(
            if url.starts_with("https://api.github.com/") && matches!(status, 403 | 429) {
                "GitHub API rate limit reached; try again later".into()
            } else {
                format!("Server returned an error: HTTP {status}")
            },
        );
    }
    if file_len(dest) > limit {
        return Err(format!("Download failed: {} (curl 63)", curl_reason(63)));
    }
    Ok(())
}
/// Downloads to `dest`, reporting its size as the job's progress; removes it on failure.
fn download(
    url: &str,
    proxy: Option<&str>,
    headers: &[&str],
    dest: &Path,
    limit: u64,
    secs: u64,
) -> Result<(), String> {
    let _ = fs::remove_file(dest);
    let done = AtomicBool::new(false);
    let result = std::thread::scope(|scope| {
        scope.spawn(|| {
            while !done.load(Ordering::Acquire) {
                progress(file_len(dest));
                std::thread::sleep(Duration::from_millis(250));
            }
        });
        let result = curl(url, proxy, headers, dest, limit, secs);
        done.store(true, Ordering::Release);
        result
    });
    match result {
        Ok(()) => {
            progress(file_len(dest));
            Ok(())
        }
        Err(e) => {
            let _ = fs::remove_file(dest);
            Err(e)
        }
    }
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    assets: Vec<Asset>,
}
#[derive(Deserialize)]
struct Asset {
    #[serde(default)]
    name: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    digest: Option<String>,
    #[serde(default)]
    browser_download_url: String,
}
fn valid_tag(tag: &str) -> bool {
    tag.len() > 1
        && tag.len() <= 40
        && tag.starts_with('v')
        && tag
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
}
/// The arm64 archive of a release and the digest GitHub computed for it. There is no
/// checksum file; this digest from api.github.com is the only one trusted.
fn parse_release(body: &[u8], checked_at: u64) -> Result<Latest, String> {
    const INVALID_RELEASE: &str = "GitHub returned an invalid release";
    let r: Release = serde_json::from_slice(body).map_err(|_| INVALID_RELEASE)?;
    if !valid_tag(&r.tag_name) {
        return Err(INVALID_RELEASE.into());
    }
    let asset = format!("mihomo-linux-arm64-{}.gz", r.tag_name);
    let a = r
        .assets
        .iter()
        .find(|a| a.name == asset)
        .ok_or("Release has no arm64 core")?;
    let digest = a
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
        .filter(|h| is_hex64(h))
        .ok_or("Release has no SHA-256 digest for the core")?;
    let url = format!("{RELEASE_FILES}{}/{asset}", r.tag_name);
    if a.browser_download_url != url || a.size == 0 {
        return Err(INVALID_RELEASE.into());
    }
    if a.size > MAX_UPLOAD {
        return Err("Release asset is too large".into());
    }
    Ok(Latest {
        published_at: r
            .published_at
            .unwrap_or_default()
            .chars()
            .take(40)
            .collect(),
        checked_at,
        size: a.size,
        digest: format!("sha256:{}", digest.to_ascii_lowercase()),
        url,
        asset,
        tag: r.tag_name,
    })
}
/// Whether a check is recent enough to reuse (one from a clock that went back is not).
fn fresh(checked_at: u64, now: u64) -> bool {
    checked_at <= now && now - checked_at < CHECK_CACHE
}
/// The first hex token of a `.sha256sum` file ("<digest>  geoip.metadb").
fn published_digest(text: &str) -> Option<String> {
    let token = text.split_whitespace().next()?;
    is_hex64(token).then(|| token.to_ascii_lowercase())
}

// ---------- Files ----------

/// Available bytes on the partition holding `path` (0 when unknown).
fn free_bytes(path: &str) -> u64 {
    let Ok(c) = std::ffi::CString::new(path) else {
        return 0;
    };
    let mut st = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    // statvfs fills the buffer on success; it is read only then.
    if unsafe { libc::statvfs(c.as_ptr(), st.as_mut_ptr()) } != 0 {
        return 0;
    }
    let st = unsafe { st.assume_init() };
    u64::try_from(st.f_bavail as u128 * st.f_frsize as u128).unwrap_or(u64::MAX)
}
/// Fails unless /data can take `required` bytes, `written` of which are already there.
fn need_space(required: u64, written: u64) -> Result<(), String> {
    if free_bytes(DATA).saturating_add(written) >= required {
        Ok(())
    } else {
        Err(NO_SPACE.into())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Gzip,
    Elf,
}
/// Room a core install needs: the archive plus the binary unpacked from it (the official
/// arm64 core goes from 21 MB to 57 MB), or a raw binary twice, and the margin.
fn required(format: Format, size: u64) -> u64 {
    let factor = match format {
        Format::Gzip => 5,
        Format::Elf => 2,
    };
    size.saturating_mul(factor).saturating_add(MARGIN)
}
fn format_of(head: &[u8]) -> Option<Format> {
    if head.starts_with(&[0x1f, 0x8b]) {
        Some(Format::Gzip)
    } else if head.starts_with(b"\x7fELF") {
        Some(Format::Elf)
    } else {
        None
    }
}
/// The first bytes of a file, for format and header checks.
fn head(path: &Path) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(64);
    if let Ok(f) = File::open(path) {
        let _ = f.take(64).read_to_end(&mut bytes);
    }
    bytes
}
/// A 64-bit little-endian executable for `machine` (183 is aarch64).
fn check_elf(head: &[u8], machine: u16) -> Result<(), String> {
    let half = |at: usize| u16::from_le_bytes([head[at], head[at + 1]]);
    if head.len() >= 20
        && head.starts_with(b"\x7fELF")
        && head[4] == 2
        && head[5] == 1
        && matches!(half(16), 2 | 3)
        && half(18) == machine
    {
        Ok(())
    } else {
        Err("Core is not an arm64 Linux executable".into())
    }
}
#[derive(Debug, PartialEq, Eq)]
enum Unpack {
    Invalid,
    TooLarge,
    Write,
}
/// Decompresses `src` into a new executable `dst`, refusing more than `cap` bytes.
fn gunzip(src: &Path, dst: &Path, cap: u64) -> Result<u64, Unpack> {
    let _ = fs::remove_file(dst);
    let input = File::open(src).map_err(|_| Unpack::Write)?;
    let mut input = flate2::read::MultiGzDecoder::new(io::BufReader::new(input));
    let mut out = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(dst)
        .map_err(|_| Unpack::Write)?;
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = input.read(&mut buf).map_err(|_| Unpack::Invalid)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > cap {
            return Err(Unpack::TooLarge);
        }
        out.write_all(&buf[..n]).map_err(|_| Unpack::Write)?;
    }
    out.sync_all().map_err(|_| Unpack::Write)?;
    Ok(total)
}
fn sync_dir(dir: &Path) {
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
}
/// Whether two paths are links of one file.
fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::metadata(a), fs::metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}
/// `staged` becomes `core`; the replaced core, if any, becomes `previous` through a hard
/// link, so `core` is never missing. The older previous core stays at `aside` (another
/// link) until the new core is confirmed, so `swap_back` can restore both. Returns whether
/// there was a core.
fn swap_in(
    core: &Path,
    previous: &Path,
    link: &Path,
    aside: &Path,
    staged: &Path,
) -> io::Result<bool> {
    if !core.is_file() {
        fs::rename(staged, core)?;
        return Ok(false);
    }
    let _ = fs::remove_file(aside);
    if previous.is_file() && !same_file(previous, core) {
        fs::hard_link(previous, aside)?;
    }
    let _ = fs::remove_file(link);
    if let Err(e) = fs::hard_link(core, link).and_then(|_| fs::rename(link, previous)) {
        // `previous` is unchanged, and `aside` only another link of it.
        let _ = fs::remove_file(link);
        let _ = fs::remove_file(aside);
        return Err(e);
    }
    if let Err(e) = fs::rename(staged, core) {
        let _ = swap_back(core, previous, aside);
        return Err(e);
    }
    Ok(true)
}
/// Undoes a `swap_in` whose new core was not confirmed, from whichever step it reached:
/// the core it replaced goes live again and the older previous core comes back from
/// `aside`. Only once a swap began: from `swap_in` itself, for a new core that failed to
/// start, or while `aside` exists (`cleanup_on_start`).
fn swap_back(core: &Path, previous: &Path, aside: &Path) -> io::Result<()> {
    let set_aside = aside.is_file();
    if set_aside && same_file(aside, previous) {
        // Cut off before `previous` was replaced.
        return fs::remove_file(aside);
    }
    // A `previous` that is a link of the live core was replaced, but the core was not.
    if previous.is_file() && !same_file(previous, core) {
        fs::rename(previous, core)?;
    }
    if set_aside {
        fs::rename(aside, previous)
    } else {
        // There was no older previous core.
        match fs::remove_file(previous) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}
/// Exchanges `core` and `previous` through a hard link at `swap`, so `core` is never
/// missing; an interrupted exchange is finished by `cleanup_on_start`.
fn exchange(core: &Path, previous: &Path, swap: &Path) -> io::Result<()> {
    if !core.is_file() {
        return fs::rename(previous, core);
    }
    let _ = fs::remove_file(swap);
    fs::hard_link(core, swap)?;
    if let Err(e) = fs::rename(previous, core) {
        let _ = fs::remove_file(swap);
        return Err(e);
    }
    fs::rename(swap, previous)
}
/// A hard link, or a copy across filesystems.
fn link_or_copy(src: &Path, dst: &Path) -> io::Result<()> {
    let _ = fs::remove_file(dst);
    fs::hard_link(src, dst).or_else(|_| fs::copy(src, dst).map(|_| ()))
}
/// Renames `src` over `dst`; across filesystems, copies beside `dst` and renames that.
fn move_file(src: &Path, dst: &Path) -> io::Result<()> {
    match fs::rename(src, dst) {
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            let name = dst.file_name().unwrap_or_default().to_string_lossy();
            let staged = dst.with_file_name(format!(".{name}.new"));
            fs::copy(src, &staged)?;
            File::open(&staged)?.sync_all()?;
            fs::rename(&staged, dst)?;
            fs::remove_file(src)
        }
        other => other,
    }
}
/// The checks of scripts/prepare-geodata.py: the MaxMind marker near the end of a
/// database, and a plausible size for a geosite list.
fn check_geo(name: &str, path: &Path) -> Result<(), String> {
    let len = file_len(path);
    if name.ends_with(".metadb") || name.ends_with(".mmdb") {
        let mut tail = Vec::new();
        File::open(path)
            .and_then(|mut f| {
                f.seek(SeekFrom::Start(len.saturating_sub(128 * 1024)))?;
                f.read_to_end(&mut tail)
            })
            .map_err(|_| CANNOT_INSTALL_GEO)?;
        if !tail.windows(MMDB_MARKER.len()).any(|w| w == MMDB_MARKER) {
            return Err("Geo data is not a MaxMind database".into());
        }
    } else if len < 1024 {
        return Err("Geo data is too small".into());
    }
    Ok(())
}
/// A mihomo home for `-t` that sees the new files and the rest of the live geo data,
/// including files the core fetched into `live` itself (so the test downloads nothing).
fn test_home(new: &Path, share: &Path, live: &Path, home: &Path, names: &[&str]) -> io::Result<()> {
    for name in clash::GEO_FILES {
        let source = if names.contains(name) {
            new.join(name)
        } else {
            share.join(name)
        };
        if source.is_file() {
            symlink(&source, home.join(name))?;
        }
    }
    for entry in fs::read_dir(live).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        let ours = clash::GEO_FILES
            .iter()
            .any(|g| g.eq_ignore_ascii_case(&name));
        if !ours
            && [".dat", ".mmdb", ".metadb"]
                .iter()
                .any(|s| name.ends_with(s))
        {
            symlink(entry.path(), home.join(entry.file_name()))?;
        }
    }
    Ok(())
}
/// What `replace_geo` changed in the shared directory, so `restore_geo` undoes only that.
/// A name is listed as its move begins, so one that fails halfway is undone too.
#[derive(Debug, Default, PartialEq, Eq)]
struct GeoChange<'a> {
    /// Live files that were kept and then replaced: put back from the kept copy.
    replaced: Vec<&'a str>,
    /// Files installed where there was none: removed again.
    added: Vec<&'a str>,
}
/// Moves each new file over the live one after keeping the live set in `keep`, recording
/// each move in `change`. Which files exist is settled before anything changes, so a copy
/// that cannot be kept never makes `restore_geo` remove a live file.
fn replace_geo<'a>(
    new: &Path,
    share: &Path,
    keep: &Path,
    names: &[&'a str],
    change: &mut GeoChange<'a>,
) -> io::Result<()> {
    let existed: Vec<bool> = names.iter().map(|n| share.join(n).is_file()).collect();
    let _ = fs::remove_dir_all(keep);
    fs::create_dir_all(keep)?;
    fs::create_dir_all(share)?;
    for (name, &existed) in names.iter().zip(&existed) {
        if existed {
            link_or_copy(&share.join(name), &keep.join(name))?;
        }
    }
    for (&name, existed) in names.iter().zip(existed) {
        if existed {
            change.replaced.push(name);
        } else {
            change.added.push(name);
        }
        move_file(&new.join(name), &share.join(name))?;
    }
    sync_dir(share);
    Ok(())
}
/// Undoes `change`: replaced files come back from `keep`, added ones are removed.
fn restore_geo(share: &Path, keep: &Path, change: &GeoChange) {
    for name in &change.replaced {
        let (kept, live) = (keep.join(name), share.join(name));
        // One whose move failed can still be the kept file (a rename onto it does nothing).
        if same_file(&kept, &live) {
            continue;
        }
        let staged = share.join(format!(".{name}.restore"));
        if link_or_copy(&kept, &staged)
            .and_then(|_| fs::rename(&staged, &live))
            .is_err()
        {
            let _ = fs::remove_file(&staged);
        }
    }
    for name in &change.added {
        let _ = fs::remove_file(share.join(name));
    }
    sync_dir(share);
}

/// The deployer's lock directory, held while /data/bin or /data/share/mihomo change so a
/// deployment never snapshots half a change, and refused while a deployment runs.
struct DeployLock;
impl DeployLock {
    fn take() -> Result<Self, String> {
        let _ = fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(TRANSACTIONS);
        match fs::create_dir(DEPLOY_LOCK) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Err(DEPLOYING.into()),
            Err(_) => return Err("Cannot take the deployment lock".into()),
        }
        let lock = Self;
        if Path::new(DEPLOY_ACTIVE).exists() {
            return Err(DEPLOYING.into());
        }
        fs::write(
            Path::new(DEPLOY_LOCK).join("owner"),
            format!("{LOCK_OWNER}\n"),
        )
        .map_err(|_| "Cannot take the deployment lock")?;
        Ok(lock)
    }
}
impl Drop for DeployLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(Path::new(DEPLOY_LOCK).join("owner"));
        let _ = fs::remove_dir(DEPLOY_LOCK);
    }
}
fn deployment_idle() -> Result<(), String> {
    if Path::new(DEPLOY_LOCK).exists() || Path::new(DEPLOY_ACTIVE).exists() {
        Err(DEPLOYING.into())
    } else {
        Ok(())
    }
}
/// Removes the staging files of core jobs.
fn discard() {
    for path in [UPLOAD_PART, DOWNLOAD_PART, STAGED, PREVIOUS_NEW] {
        let _ = fs::remove_file(path);
    }
}
/// At agent start, before the supervisor: removes what an interrupted job left (staged
/// files, test homes, its deployment lock), finishes an interrupted rollback and undoes an
/// install that had not confirmed its core.
pub fn cleanup_on_start() {
    discard();
    let _ = fs::remove_file(RELEASE_JSON);
    let swap = Path::new(SWAP);
    if swap.is_file() {
        // `previous` gone means the exchange had moved it live; the core it replaced,
        // kept here, is the previous one now.
        if Path::new(PREVIOUS).exists() {
            let _ = fs::remove_file(swap);
        } else {
            let _ = fs::rename(swap, PREVIOUS);
        }
    }
    // The replaced core and the older previous one come back, as core.json (written only
    // once a core is confirmed) still describes them.
    let aside = Path::new(PREVIOUS_OLD);
    if aside.is_file() {
        let _ = swap_back(Path::new(clash::MIHOMO), Path::new(PREVIOUS), aside);
        sync_dir(Path::new(BIN));
    }
    for dir in [GEO_NEW, GEO_NEW_HOME] {
        let _ = fs::remove_dir_all(dir);
    }
    let owner = Path::new(DEPLOY_LOCK).join("owner");
    if fs::read_to_string(&owner).is_ok_and(|o| o.trim() == LOCK_OWNER) {
        let _ = fs::remove_file(&owner);
        let _ = fs::remove_dir(DEPLOY_LOCK);
    }
}

// ---------- Jobs ----------

/// What a replaced core or geo data set is started and checked on: the saved settings when
/// Clash is on, also while its core is down between a crash and the supervisor's retry,
/// unless WireGuard is on too (the supervisor then keeps the core stopped). Without them
/// the files are only swapped. The caller holds the Clash lock.
fn start_settings() -> Option<clash::Settings> {
    clash::enabled_settings().filter(|_| !wireguard::stored_enabled())
}
/// Stops the core if it runs and starts it on the saved settings, so a replaced binary or
/// geo data takes effect, then waits until it answers and stays up.
fn restart(s: &clash::Settings) -> Result<(), String> {
    clash::stop_core();
    clash::restart_locked(s)?;
    let deadline = Instant::now() + Duration::from_secs(15);
    while !(clash::core_running() && clash::ctl("GET", "/version", None, 2).is_ok()) {
        if Instant::now() >= deadline {
            return Err("Clash core did not answer within 15 s".into());
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    // A core that exits right after its first answer is no better.
    std::thread::sleep(Duration::from_secs(2));
    if clash::core_running() {
        Ok(())
    } else {
        Err("Clash core exited after starting".into())
    }
}

fn check(route: Route) -> Result<(), String> {
    let proxy = proxy(route)?;
    make_dir(Path::new(STATE_DIR))?;
    let scratch = Path::new(RELEASE_JSON);
    let result = download(RELEASE_API, proxy, &API_HEADERS, scratch, 2 * MIB, 60)
        .and_then(|_| parse_release(&fs::read(scratch).unwrap_or_default(), now()));
    let _ = fs::remove_file(scratch);
    let latest = result?;
    update_state(|s| s.latest = Some(latest));
    Ok(())
}

/// Moves a received or downloaded core to `STAGED`, decompressing an archive.
fn stage(part: &Path, format: Format) -> Result<(), String> {
    let staged = Path::new(STAGED);
    let _ = fs::remove_file(staged);
    match format {
        Format::Elf => fs::rename(part, staged).map_err(|_| CANNOT_INSTALL)?,
        Format::Gzip => {
            step("extracting", 0);
            // A small archive may unpack to far more than it promised; stay above the margin.
            let cap = MAX_CORE.min(free_bytes(DATA).saturating_sub(MARGIN));
            match gunzip(part, staged, cap) {
                Ok(_) => {}
                Err(Unpack::TooLarge) if cap < MAX_CORE => return Err(NO_SPACE.into()),
                Err(Unpack::TooLarge) => return Err("Core binary exceeds 160 MiB".into()),
                Err(Unpack::Invalid) => return Err("Core archive is not valid gzip".into()),
                Err(Unpack::Write) => return Err(CANNOT_INSTALL.into()),
            }
            let _ = fs::remove_file(part);
        }
    }
    Ok(())
}
/// Checks a core before it may go live: its CPU, that it runs and is mihomo (of the
/// expected release), and that it accepts the current profile. Returns its version.
fn check_staged(path: &Path, tag: Option<&str>, machine: u16) -> Result<String, String> {
    step("testing", 0);
    check_elf(&head(path), machine)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|_| CANNOT_INSTALL)?;
    let version = check_version(&run_version(path)?, tag)?;
    if Path::new(clash::PROFILE).is_file() {
        clash::test_profile(path, Path::new(clash::HOME), Path::new(clash::PROFILE))?;
    }
    Ok(version)
}
/// Puts `STAGED` in place of the live core, which becomes `PREVIOUS`, and with Clash on
/// starts the core on it. A core that does not come back is replaced by the previous again,
/// and the older previous core stays the rollback target.
fn install(source_name: &str, version: String) -> Result<(), String> {
    step("installing", 0);
    let (core, previous, aside, staged) = (
        Path::new(clash::MIHOMO),
        Path::new(PREVIOUS),
        Path::new(PREVIOUS_OLD),
        Path::new(STAGED),
    );
    let sha256 = sha256_file(staged).map_err(|_| CANNOT_INSTALL)?;
    // From here no deployment can replace the files that are read and swapped below.
    let _deploy = DeployLock::take()?;
    let state = load_state();
    let old = identity(core, &state);
    if old.as_ref().is_some_and(|(o, _)| o.sha256 == sha256) {
        return Err("This core is already installed".into());
    }
    let _guard = clash::lock();
    let settings = start_settings();
    let had_core = swap_in(core, previous, Path::new(PREVIOUS_NEW), aside, staged)
        .map_err(|_| CANNOT_INSTALL)?;
    sync_dir(Path::new(BIN));
    if let Some(s) = &settings {
        step("restarting", 0);
        if let Err(e) = restart(s) {
            if had_core {
                let _ = swap_back(core, previous, aside);
            } else {
                let _ = fs::remove_file(core);
            }
            sync_dir(Path::new(BIN));
            // core.json still describes both cores, unless there was no older one.
            if !previous.is_file() {
                update_state(|st| st.previous = None);
            }
            return Err(match restart(s) {
                Ok(()) => format!("New core failed to start; previous core restored: {e}"),
                Err(again) => format!("New core and previous core failed to start: {e}; {again}"),
            });
        }
    }
    // The new core runs, or Clash is off: the older previous core is no longer needed.
    let _ = fs::remove_file(aside);
    sync_dir(Path::new(BIN));
    let installed = Identity { sha256, version };
    if let Ok(m) = fs::metadata(core) {
        remember(FileKey::of(&m), &installed);
    }
    let pinned = pinned_hash();
    let replaced = old.map(|(o, key)| {
        let at = installed_at(&state, &o, key);
        record(&o, source(&o.sha256, pinned.as_deref(), &state), at)
    });
    update_state(|st| {
        if had_core {
            st.previous = replaced;
        }
        st.current = Some(record(&installed, source_name.into(), now()));
    });
    Ok(())
}

fn update(latest: &Latest, via: &Via) -> Result<(), String> {
    let result = (|| -> Result<(), String> {
        need_space(required(Format::Gzip, latest.size), 0)?;
        fs::create_dir_all(BIN).map_err(|_| CANNOT_INSTALL)?;
        step("downloading", latest.size);
        let (url, proxy) = via.file(&latest.url)?;
        let part = Path::new(DOWNLOAD_PART);
        download(&url, proxy, &[], part, latest.size, 1200)?;
        step("verifying", 0);
        let digest = sha256_file(part).map_err(|_| CANNOT_INSTALL)?;
        if Some(digest.as_str()) != latest.digest.strip_prefix("sha256:") {
            return Err("Downloaded core does not match the published checksum".into());
        }
        stage(part, Format::Gzip)?;
        let version = check_staged(Path::new(STAGED), Some(&latest.tag), EM_AARCH64)?;
        install("official", version)
    })();
    discard();
    result
}

fn install_upload(size: u64) -> Result<(), String> {
    let result = (|| -> Result<(), String> {
        let part = Path::new(UPLOAD_PART);
        let format =
            format_of(&head(part)).ok_or("Upload must be a .gz archive or an executable")?;
        need_space(required(format, size), size)?;
        stage(part, format)?;
        let version = check_staged(Path::new(STAGED), None, EM_AARCH64)?;
        install("uploaded", version)
    })();
    discard();
    result
}

/// Exchanges the live and previous cores after checking that the previous one still runs
/// and accepts the current profile; one that does not come back is exchanged back.
fn rollback() -> Result<(), String> {
    let (core, previous, swap) = (
        Path::new(clash::MIHOMO),
        Path::new(PREVIOUS),
        Path::new(SWAP),
    );
    check_staged(previous, None, EM_AARCH64)?;
    step("installing", 0);
    let _deploy = DeployLock::take()?;
    let state = load_state();
    let current = identity(core, &state);
    let (back, _) = identity(previous, &state).ok_or("No previous core to roll back to")?;
    let _guard = clash::lock();
    let settings = start_settings();
    exchange(core, previous, swap).map_err(|_| CANNOT_INSTALL)?;
    sync_dir(Path::new(BIN));
    if let Some(s) = &settings {
        step("restarting", 0);
        if let Err(e) = restart(s) {
            let again = exchange(core, previous, swap)
                .map_err(|_| CANNOT_INSTALL.to_string())
                .and_then(|_| {
                    sync_dir(Path::new(BIN));
                    restart(s)
                });
            return Err(match again {
                Ok(()) => format!("Previous core failed to start; current core restored: {e}"),
                Err(again) => {
                    format!("Previous core and current core failed to start: {e}; {again}")
                }
            });
        }
    }
    let pinned = pinned_hash();
    let restored = record(
        &back,
        source(&back.sha256, pinned.as_deref(), &state),
        now(),
    );
    let replaced = current.map(|(c, key)| {
        let at = installed_at(&state, &c, key);
        record(&c, source(&c.sha256, pinned.as_deref(), &state), at)
    });
    update_state(|st| {
        st.current = Some(restored);
        st.previous = replaced;
    });
    Ok(())
}

fn geodata(via: &Via) -> Result<(), String> {
    let (new, home) = (Path::new(GEO_NEW), Path::new(GEO_NEW_HOME));
    let result = geodata_steps(via, new, home);
    let _ = fs::remove_dir_all(new);
    let _ = fs::remove_dir_all(home);
    result
}
/// Downloads and verifies both files, tests the profile on them, then swaps them into the
/// shared directory (keeping the old set) and restarts a running core on them: mihomo
/// keeps geo databases open and cached, so a profile reload would not pick them up.
fn geodata_steps(via: &Via, new: &Path, home: &Path) -> Result<(), String> {
    let share = Path::new(clash::GEODATA);
    let names = GEO_UPSTREAM.map(|(_, local)| local);
    let need = names
        .iter()
        .map(|n| fs::metadata(share.join(n)).map_or(MAX_GEO, |m| m.len()))
        .sum::<u64>()
        .saturating_mul(2)
        .saturating_add(MARGIN);
    need_space(need, 0)?;
    for dir in [new, home] {
        let _ = fs::remove_dir_all(dir);
        make_dir(dir)?;
    }
    let github = via.proxy()?;
    let mut digests = Vec::new();
    for (upstream, local) in GEO_UPSTREAM {
        step("downloading", 0);
        let listed = new.join(format!("{local}.sha256sum"));
        download(
            &format!("{GEO_BASE}{upstream}.sha256sum"),
            github,
            &[],
            &listed,
            4096,
            60,
        )?;
        let digest = published_digest(&fs::read_to_string(&listed).unwrap_or_default())
            .ok_or("Geo data has no published checksum")?;
        let (url, proxy) = via.file(&format!("{GEO_BASE}{upstream}"))?;
        download(&url, proxy, &[], &new.join(local), MAX_GEO, 600)?;
        digests.push((local, digest));
    }
    step("verifying", 0);
    for (local, digest) in &digests {
        let file = new.join(local);
        if sha256_file(&file).ok().as_ref() != Some(digest) {
            return Err("Geo data does not match the published checksum".into());
        }
        check_geo(local, &file)?;
    }
    let core = Path::new(clash::MIHOMO);
    if core.is_file() && Path::new(clash::PROFILE).is_file() {
        step("testing", 0);
        test_home(new, share, Path::new(clash::HOME), home, &names)
            .map_err(|_| CANNOT_INSTALL_GEO)?;
        clash::test_profile(core, home, Path::new(clash::PROFILE))?;
    }
    step("installing", 0);
    let keep = Path::new(GEO_PREVIOUS);
    let _deploy = DeployLock::take()?;
    let _guard = clash::lock();
    let settings = start_settings();
    let mut change = GeoChange::default();
    if replace_geo(new, share, keep, &names, &mut change).is_err() {
        restore_geo(share, keep, &change);
        return Err(CANNOT_INSTALL_GEO.into());
    }
    if let Some(s) = &settings {
        step("restarting", 0);
        if let Err(e) = restart(s) {
            restore_geo(share, keep, &change);
            return Err(match restart(s) {
                Ok(()) => format!("New geo data failed to load; previous data restored: {e}"),
                Err(again) => format!("New and previous geo data failed to load: {e}; {again}"),
            });
        }
    }
    Ok(())
}

// ---------- API ----------

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Check {
        route: Option<Route>,
    },
    Update {
        tag: String,
        route: Option<Route>,
        mirror: Option<String>,
    },
    Rollback {},
    Geodata {
        route: Option<Route>,
        mirror: Option<String>,
    },
}

fn status() -> Value {
    let state = load_state();
    let pinned = pinned_hash();
    let job = JOB.lock().unwrap().clone();
    let describe = |path: &str| {
        identity(Path::new(path), &state).map(|(id, key)| {
            let source = source(&id.sha256, pinned.as_deref(), &state);
            record(&id, source, installed_at(&state, &id, key))
        })
    };
    let (installed, previous) = (describe(clash::MIHOMO), describe(PREVIOUS));
    let view = |r: &Option<Record>| {
        r.as_ref()
            .map(|r| json!({"version": r.version, "sha256": r.sha256, "source": r.source}))
    };
    let data = json!({"installed": view(&installed), "previous": view(&previous),
        "latest": state.latest, "job": job, "free_bytes": free_bytes(DATA),
        "mirror_hint": mirror_hint(job.as_ref())});
    // Something else replaced a file (a deployment, an interrupted job): record what is
    // there, so a source survives a later manifest that no longer lists the core.
    let sha = |r: &Option<Record>| r.as_ref().map(|r| r.sha256.clone());
    let busy = job.is_some_and(|j| j.state == "running");
    if !busy && (sha(&installed) != sha(&state.current) || sha(&previous) != sha(&state.previous)) {
        // Hashing can take seconds: a job that started meanwhile is checked for again.
        update_state_if(|now| relabel(now, &state, installed, previous, job_running()));
    }
    data
}
/// Records the cores a status poll found in `now`, core.json as it is under the lock,
/// unless a job is running (it may be swapping the files) or core.json no longer holds
/// `seen`, what the poll read before hashing: a job that wrote it meanwhile knows better.
fn relabel(
    now: &mut State,
    seen: &State,
    current: Option<Record>,
    previous: Option<Record>,
    busy: bool,
) -> bool {
    if busy || now.current != seen.current || now.previous != seen.previous {
        return false;
    }
    now.current = current;
    now.previous = previous;
    true
}
fn failure(e: String) -> (u16, Value) {
    let code = match e.as_str() {
        BUSY | DEPLOYING => 409,
        TOO_LARGE => 413,
        _ => 400,
    };
    (code, json!({"ok": false, "error": e}))
}

/// `GET /api/clash/core`: installed and previous core, the last release check, the job.
pub fn get() -> (u16, Value) {
    (200, json!({"ok": true, "data": status()}))
}

fn start(body: &[u8]) -> Result<(), String> {
    let action: Action = serde_json::from_slice(body).map_err(|_| INVALID)?;
    if job_running() {
        return Err(BUSY.into());
    }
    match action {
        Action::Check { route } => {
            let route = route.unwrap_or_default();
            if route == Route::Mirror {
                return Err(INVALID.into());
            }
            proxy(route)?;
            claim(&JOB, "check", "checking")?;
            if load_state()
                .latest
                .is_some_and(|l| fresh(l.checked_at, now()))
            {
                finish(&JOB, &Ok(()));
            } else {
                spawn(move || check(route));
            }
        }
        Action::Update { tag, route, mirror } => {
            let via = Via::new(route, mirror)?;
            via.proxy()?;
            let latest = load_state()
                .latest
                .filter(|l| l.tag == tag)
                .ok_or("Check for updates first")?;
            deployment_idle()?;
            claim(&JOB, "update", "downloading")?;
            spawn(move || update(&latest, &via));
        }
        Action::Rollback {} => {
            if !Path::new(PREVIOUS).is_file() {
                return Err("No previous core to roll back to".into());
            }
            deployment_idle()?;
            claim(&JOB, "rollback", "testing")?;
            spawn(rollback);
        }
        Action::Geodata { route, mirror } => {
            let via = Via::new(route, mirror)?;
            via.proxy()?;
            deployment_idle()?;
            claim(&JOB, "geodata", "downloading")?;
            spawn(move || geodata(&via));
        }
    }
    Ok(())
}
/// `POST /api/clash/core`: starts a check, update, rollback or geo data job and returns
/// the status at once; the page polls `get` until the job is no longer running.
pub fn post(body: &[u8]) -> (u16, Value) {
    match start(body) {
        Ok(()) => get(),
        Err(e) => failure(e),
    }
}

/// The most an upload may write: 64 MiB, less when /data would keep under 16 MiB free.
fn upload_cap(length: Option<u64>, free: u64) -> Result<u64, String> {
    if length.is_some_and(|n| n > MAX_UPLOAD) {
        return Err(TOO_LARGE.into());
    }
    let room = free.saturating_sub(MARGIN);
    if room == 0 || length.is_some_and(|n| n > room) {
        return Err(NO_SPACE.into());
    }
    Ok(MAX_UPLOAD.min(room))
}
/// Streams a request body to `dest`, refusing more than `cap` bytes or a short body.
fn receive(
    reader: &mut dyn Read,
    length: Option<u64>,
    dest: &Path,
    cap: u64,
    progress: &dyn Fn(u64),
) -> Result<u64, String> {
    let _ = fs::remove_file(dest);
    let mut out = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(dest)
        .map_err(|_| SAVE_FAILED)?;
    let mut buf = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(INTERRUPTED.into()),
        };
        total += n as u64;
        if total > cap {
            return Err(if cap < MAX_UPLOAD {
                NO_SPACE
            } else {
                TOO_LARGE
            }
            .into());
        }
        out.write_all(&buf[..n]).map_err(|_| SAVE_FAILED)?;
        progress(total);
    }
    if total == 0 {
        return Err(EMPTY.into());
    }
    if length.is_some_and(|n| n != total) {
        return Err(INTERRUPTED.into());
    }
    out.sync_all().map_err(|_| SAVE_FAILED)?;
    Ok(total)
}
fn receive_upload(reader: &mut dyn Read, length: Option<u64>) -> Result<u64, String> {
    if length == Some(0) {
        return Err(EMPTY.into());
    }
    upload_cap(length, u64::MAX)?;
    if job_running() {
        return Err(BUSY.into());
    }
    deployment_idle()?;
    claim(&JOB, "upload", "receiving")?;
    let result = (|| -> Result<u64, String> {
        let cap = upload_cap(length, free_bytes(DATA))?;
        fs::create_dir_all(BIN).map_err(|_| SAVE_FAILED)?;
        step("receiving", length.unwrap_or(0));
        receive(reader, length, Path::new(UPLOAD_PART), cap, &progress)
    })();
    if let Err(e) = &result {
        discard();
        finish(&JOB, &Err(e.clone()));
    }
    result
}
/// The checks `upload` makes before reading, so server.rs can refuse an upload without
/// accepting its body.
pub fn upload_rejection(length: Option<u64>) -> Option<(u16, Value)> {
    let check = || -> Result<(), String> {
        if length == Some(0) {
            return Err(EMPTY.into());
        }
        upload_cap(length, u64::MAX)?;
        if job_running() {
            return Err(BUSY.into());
        }
        deployment_idle()
    };
    check().err().map(failure)
}
/// `PUT /api/clash/core/upload`, after authentication and before the 1 MiB body limit:
/// streams a gzip archive or a raw arm64 executable (at most 64 MiB) to /data/bin, then
/// installs it on a job thread like an update. `length` is the Content-Length if any.
pub fn upload(reader: &mut dyn Read, length: Option<u64>) -> (u16, Value) {
    match receive_upload(reader, length) {
        Ok(size) => {
            spawn(move || install_upload(size));
            get()
        }
        Err(e) => failure(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Cursor, path::PathBuf, sync::atomic::AtomicU64};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("openui-core-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
    const DIGEST: &str = "9dd862e28b46ff7d775f169cceebc28deccaa0a9e804237d421cd2571e0caba0";
    const ASSET_URL: &str = "https://github.com/MetaCubeX/mihomo/releases/download/v1.19.32/mihomo-linux-arm64-v1.19.32.gz";
    fn arm64(digest: Value) -> Value {
        json!({"name": "mihomo-linux-arm64-v1.19.32.gz", "size": 20809592, "digest": digest,
            "browser_download_url": ASSET_URL})
    }
    /// Shaped like api.github.com/repos/MetaCubeX/mihomo/releases/latest (trimmed).
    fn release(asset: Value) -> Value {
        let base = "https://github.com/MetaCubeX/mihomo/releases/download/v1.19.32/";
        json!({"tag_name": "v1.19.32", "published_at": "2026-09-30T17:09:36Z", "draft": false,
            "prerelease": false, "body": "notes", "assets": [
            {"name": "mihomo-linux-arm64-v1.19.32.deb", "size": 20712454,
                "digest": format!("sha256:{}", "0".repeat(64)),
                "browser_download_url": format!("{base}mihomo-linux-arm64-v1.19.32.deb")},
            {"name": "mihomo-linux-armv7-v1.19.32.gz", "size": 19000000, "digest": null,
                "browser_download_url": format!("{base}mihomo-linux-armv7-v1.19.32.gz")},
            {"name": "version.txt", "size": 8, "uploader": {"login": "x"}},
            asset]})
    }
    fn parse(release: &Value) -> Result<Latest, String> {
        parse_release(release.to_string().as_bytes(), 1000)
    }

    #[test]
    fn release_selects_the_arm64_archive_and_its_digest() {
        let latest = parse(&release(arm64(json!(format!("sha256:{DIGEST}"))))).unwrap();
        assert_eq!(
            latest,
            Latest {
                tag: "v1.19.32".into(),
                published_at: "2026-09-30T17:09:36Z".into(),
                checked_at: 1000,
                asset: "mihomo-linux-arm64-v1.19.32.gz".into(),
                size: 20809592,
                digest: format!("sha256:{DIGEST}"),
                url: ASSET_URL.into(),
            }
        );
        let upper = arm64(json!(format!("sha256:{}", DIGEST.to_uppercase())));
        assert_eq!(parse(&release(upper)).unwrap().digest, latest.digest);
        for digest in [
            Value::Null,
            json!(DIGEST),
            json!(format!("sha512:{DIGEST}")),
            json!("sha256:9dd8"),
        ] {
            assert_eq!(
                parse(&release(arm64(digest.clone()))).unwrap_err(),
                "Release has no SHA-256 digest for the core",
                "{digest}"
            );
        }
        // Only the plain arm64 archive of this very tag counts.
        let mut variant = arm64(json!(format!("sha256:{DIGEST}")));
        variant["name"] = json!("mihomo-linux-arm64-go120-v1.19.32.gz");
        assert_eq!(
            parse(&release(variant)).unwrap_err(),
            "Release has no arm64 core"
        );
        let mut moved = arm64(json!(format!("sha256:{DIGEST}")));
        moved["browser_download_url"] =
            json!("https://mirror.example/mihomo-linux-arm64-v1.19.32.gz");
        assert!(parse(&release(moved)).is_err());
        let mut huge = arm64(json!(format!("sha256:{DIGEST}")));
        huge["size"] = json!(MAX_UPLOAD + 1);
        assert_eq!(
            parse(&release(huge)).unwrap_err(),
            "Release asset is too large"
        );
        let mut tag = release(arm64(json!(format!("sha256:{DIGEST}"))));
        tag["tag_name"] = json!("v1/../../x");
        assert!(parse(&tag).is_err());
        assert!(parse_release(b"<html>rate limited</html>", 0).is_err());
    }
    #[test]
    fn a_recent_check_is_reused_unless_the_clock_went_back() {
        assert!(fresh(1000, 1000) && fresh(1000, 1000 + CHECK_CACHE - 1));
        assert!(!fresh(1000, 1000 + CHECK_CACHE));
        assert!(!fresh(1000 + 8 * 3600, 1000));
    }
    #[test]
    fn mirrors_are_https_prefixes_for_the_large_file_only() {
        assert_eq!(
            mirror_prefix("https://ghfast.top/").unwrap(),
            "https://ghfast.top/"
        );
        assert_eq!(
            mirror_prefix("https://ghfast.top").unwrap(),
            "https://ghfast.top/"
        );
        assert_eq!(
            mirror_prefix("https://gh.example:8443/proxy/").unwrap(),
            "https://gh.example:8443/proxy/"
        );
        let long = format!("https://{}.example/", "a".repeat(200));
        for bad in [
            "http://ghfast.top/",
            "ghfast.top",
            "https://",
            "https:///path",
            "https://a b/",
            "https://x.example/?u=",
            "https://x.example/#",
            "https://user@x.example/",
            "https://x.example/\"",
            "https://x.example/\\",
            "https://x.example/\n",
            &long,
        ] {
            assert_eq!(mirror_prefix(bad).unwrap_err(), BAD_MIRROR, "{bad}");
        }
        let via = Via::new(Some(Route::Mirror), Some("https://ghfast.top".into())).unwrap();
        assert_eq!(via.github, Route::Auto);
        assert_eq!(
            via.file("https://github.com/a.gz").unwrap(),
            (
                "https://ghfast.top/https://github.com/a.gz".to_string(),
                None
            )
        );
        assert_eq!(
            Via::new(None, Some("https://ghfast.top/".into())).unwrap(),
            via
        );
        assert_eq!(Via::new(Some(Route::Mirror), None).unwrap_err(), BAD_MIRROR);
        assert_eq!(
            Via::new(Some(Route::Direct), Some("https://ghfast.top/".into())).unwrap_err(),
            INVALID
        );
        let direct = Via::new(Some(Route::Direct), None).unwrap();
        assert_eq!(
            direct.file("https://github.com/a.gz").unwrap(),
            ("https://github.com/a.gz".to_string(), None)
        );
    }
    #[test]
    fn curl_options_stay_on_stdin_and_are_quoted() {
        assert_eq!(
            curl_config("https://x/\"a\\", Some(clash::PROXY), &API_HEADERS[..1]),
            "url = \"https://x/\\\"a\\\\\"\nuser-agent = \"openui-agent\"\nproxy = \"http://127.0.0.1:7890\"\nheader = \"Accept: application/vnd.github+json\"\n"
        );
    }
    #[test]
    fn actions_parse_strictly() {
        let parse = |s: &str| serde_json::from_str::<Action>(s);
        assert!(matches!(
            parse(r#"{"action":"check"}"#),
            Ok(Action::Check { route: None })
        ));
        assert!(matches!(
            parse(r#"{"action":"check","route":"proxy"}"#),
            Ok(Action::Check {
                route: Some(Route::Proxy)
            })
        ));
        assert!(matches!(
            parse(
                r#"{"action":"update","tag":"v1.19.32","route":"mirror","mirror":"https://ghfast.top/"}"#
            ),
            Ok(Action::Update { .. })
        ));
        assert!(matches!(
            parse(r#"{"action":"rollback"}"#),
            Ok(Action::Rollback {})
        ));
        assert!(matches!(
            parse(r#"{"action":"geodata","route":"direct"}"#),
            Ok(Action::Geodata { .. })
        ));
        for bad in [
            r#"{"action":"rollback","force":true}"#,
            r#"{"action":"update"}"#,
            r#"{"action":"check","route":"tor"}"#,
            r#"{"action":"check","tag":"v1.19.32"}"#,
            r#"{"action":"install"}"#,
            r#"{"route":"auto"}"#,
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }
    #[test]
    fn digests_and_published_checksums() {
        let dir = scratch("sha");
        let file = dir.join("abc");
        fs::write(&file, "abc").unwrap();
        assert_eq!(
            sha256_file(&file).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // As published beside the meta-rules-dat files.
        let listed =
            "F23BFCF35BA92BD1A84E15E813622CDEF9C02D43CD3144DDFF7B9D6E7BA30798  geoip.metadb\n";
        assert_eq!(
            published_digest(listed).as_deref(),
            Some("f23bfcf35ba92bd1a84e15e813622cdef9c02d43cd3144ddff7b9d6e7ba30798")
        );
        let not_hex = "g".repeat(64);
        for bad in ["", "\n", "f23b  geoip.metadb", "<html>404</html>", &not_hex] {
            assert_eq!(published_digest(bad), None, "{bad}");
        }
        fs::remove_dir_all(&dir).unwrap();
    }
    fn elf(class: u8, data: u8, kind: u16, machine: u16) -> Vec<u8> {
        let mut h = vec![0u8; 64];
        h[..4].copy_from_slice(b"\x7fELF");
        h[4] = class;
        h[5] = data;
        h[6] = 1;
        h[16..18].copy_from_slice(&kind.to_le_bytes());
        h[18..20].copy_from_slice(&machine.to_le_bytes());
        h
    }
    #[test]
    fn elf_headers_must_match_the_device() {
        assert!(check_elf(&elf(2, 1, 2, 183), EM_AARCH64).is_ok());
        assert!(check_elf(&elf(2, 1, 3, 183), EM_AARCH64).is_ok());
        for (bad, why) in [
            (elf(2, 1, 2, 62), "x86-64"),
            (elf(1, 1, 2, 183), "32-bit"),
            (elf(2, 2, 2, 183), "big-endian"),
            (elf(2, 1, 1, 183), "object file"),
            (b"\x7fELF\x02\x01".to_vec(), "truncated"),
            (b"#!/bin/sh\n".to_vec(), "script"),
        ] {
            assert!(check_elf(&bad, EM_AARCH64).is_err(), "{why}");
        }
        // The machine is a parameter: the test binary passes for the host's own.
        let own = head(Path::new("/proc/self/exe"));
        let machine = u16::from_le_bytes([own[18], own[19]]);
        assert!(check_elf(&own, machine).is_ok());
        assert!(check_elf(&own, machine ^ 1).is_err());
    }
    #[test]
    fn gzip_is_detected_and_unpacked_within_the_cap() {
        use flate2::{write::GzEncoder, Compression};
        let dir = scratch("gzip");
        let core = elf(2, 1, 2, 183).repeat(1000);
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(&core).unwrap();
        let gz = encoder.finish().unwrap();
        assert_eq!(format_of(&gz), Some(Format::Gzip));
        assert_eq!(format_of(&core), Some(Format::Elf));
        assert_eq!(format_of(b"PK\x03\x04"), None);
        let (src, dst) = (dir.join("core.gz"), dir.join("core"));
        fs::write(&src, &gz).unwrap();
        let size = core.len() as u64;
        assert_eq!(gunzip(&src, &dst, size), Ok(size));
        assert_eq!(fs::read(&dst).unwrap(), core);
        assert_eq!(
            fs::metadata(&dst).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(gunzip(&src, &dst, size - 1), Err(Unpack::TooLarge));
        fs::write(&src, &gz[..gz.len() / 2]).unwrap();
        assert_eq!(gunzip(&src, &dst, MAX_CORE), Err(Unpack::Invalid));
        fs::write(&src, b"\x1f\x8bnot a deflate stream").unwrap();
        assert_eq!(gunzip(&src, &dst, MAX_CORE), Err(Unpack::Invalid));
        assert_eq!(
            required(Format::Gzip, 20 * MIB),
            required(Format::Elf, 50 * MIB)
        );
        fs::remove_dir_all(&dir).unwrap();
    }
    #[test]
    fn versions_and_sources() {
        let official = "Mihomo Meta v1.19.32 linux arm64 with go1.24.6 Tue Sep 30 17:00:00 UTC 2026\nUse tags: with_gvisor\n";
        assert_eq!(
            check_version(official, Some("v1.19.32")).unwrap(),
            "v1.19.32"
        );
        assert_eq!(
            check_version(official, Some("v1.19.33")).unwrap_err(),
            "Core version does not match the release"
        );
        let flclash = "Mihomo Meta flclash-d0c8d44-openui-ca20260925 linux arm64 with go1.26.8 unknown time\n";
        assert_eq!(
            check_version(flclash, None).unwrap(),
            "flclash-d0c8d44-openui-ca20260925"
        );
        assert!(check_version("Clash v1.18.0 linux arm64", None).is_err());
        assert!(check_version("", None).is_err());
        assert_eq!(
            parse_version("Mihomo (custom build)\n"),
            "Mihomo (custom build)"
        );

        let (a, b, c, d) = (
            "a".repeat(64),
            "b".repeat(64),
            "c".repeat(64),
            "d".repeat(64),
        );
        let manifest =
            json!({"format_version": 1, "files": {"agent": c, "mihomo": a.to_uppercase()}});
        let pinned = manifest_core(manifest.to_string().as_bytes());
        assert_eq!(pinned.as_deref(), Some(a.as_str()));
        assert_eq!(manifest_core(br#"{"files":{"agent":"x"}}"#), None);
        assert_eq!(manifest_core(br#"{"files":{"mihomo":"short"}}"#), None);
        assert_eq!(manifest_core(b"not json"), None);
        let rec = |sha: &str, source: &str| Record {
            version: "v1".into(),
            sha256: sha.into(),
            source: source.into(),
            installed_at: 7,
        };
        let state = State {
            current: Some(rec(&b, "official")),
            previous: Some(rec(&c, "uploaded")),
            latest: None,
        };
        let src = |sha: &str, state: &State| source(sha, pinned.as_deref(), state);
        assert_eq!(src(&a, &state), "pinned");
        assert_eq!(src(&b, &state), "official");
        assert_eq!(src(&c, &state), "uploaded");
        assert_eq!(src(&d, &state), "unknown");
        // The manifest wins over a record; only known sources are reported.
        let odd = State {
            current: Some(rec(&a, "uploaded")),
            previous: Some(rec(&d, "<script>")),
            latest: None,
        };
        assert_eq!(src(&a, &odd), "pinned");
        assert_eq!(src(&d, &odd), "unknown");
        let key = FileKey {
            dev: 1,
            ino: 2,
            size: 3,
            mtime: 42,
            mtime_nsec: 0,
        };
        let id = |sha: &str| Identity {
            sha256: sha.into(),
            version: String::new(),
        };
        assert_eq!(installed_at(&state, &id(&b), key), 7);
        assert_eq!(installed_at(&state, &id(&d), key), 42);
        let saved = serde_json::to_value(&state).unwrap();
        assert_eq!(
            saved,
            json!({"current": {"version": "v1", "sha256": b, "source": "official", "installed_at": 7},
                "previous": {"version": "v1", "sha256": c, "source": "uploaded", "installed_at": 7},
                "latest": null})
        );
        let loaded: State = serde_json::from_value(saved).unwrap();
        assert_eq!(loaded.current, state.current);
    }
    #[test]
    fn one_job_at_a_time() {
        let slot = Mutex::new(None);
        claim(&slot, "update", "downloading").unwrap();
        assert_eq!(claim(&slot, "check", "checking").unwrap_err(), BUSY);
        finish(&slot, &Err("Download timed out".into()));
        let job = slot.lock().unwrap().clone().unwrap();
        assert_eq!(
            (job.kind, job.state, job.error.as_deref()),
            ("update", "failed", Some("Download timed out"))
        );
        assert!(job.finished_at.is_some_and(|t| t >= job.started_at));
        assert!(mirror_hint(Some(&job)));
        // A finished job's record is replaced by the next; finishing twice changes nothing.
        claim(&slot, "check", "checking").unwrap();
        finish(&slot, &Ok(()));
        finish(&slot, &Err("late".into()));
        let job = slot.lock().unwrap().clone().unwrap();
        assert_eq!(
            (job.kind, job.state, job.error.as_deref()),
            ("check", "done", None)
        );
        assert!(!mirror_hint(Some(&job)));
        let value = serde_json::to_value(&job).unwrap();
        let mut keys: Vec<&String> = value.as_object().unwrap().keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            [
                "bytes",
                "error",
                "finished_at",
                "kind",
                "started_at",
                "state",
                "step",
                "total"
            ]
        );
        assert_eq!(failure(BUSY.into()).0, 409);
        assert_eq!(failure(DEPLOYING.into()).0, 409);
        assert_eq!(failure(TOO_LARGE.into()).0, 413);
        assert_eq!(failure(NO_SPACE.into()).0, 400);
    }
    #[test]
    fn a_status_poll_never_overwrites_what_a_job_recorded() {
        let rec = |sha: &str, source: &str| {
            Some(Record {
                version: "v1".into(),
                sha256: sha.repeat(64),
                source: source.into(),
                installed_at: 7,
            })
        };
        let seen = State {
            current: rec("a", "official"),
            previous: rec("b", "uploaded"),
            latest: None,
        };
        // What the poll hashed: a deployment replaced the core.
        let found = || (rec("c", "unknown"), rec("a", "official"));
        // core.json as the poll read it: the files found are recorded, the rest is kept.
        let mut now = seen.clone();
        now.latest = Some(Latest {
            tag: "v1.19.32".into(),
            ..Latest::default()
        });
        let (current, previous) = found();
        assert!(relabel(&mut now, &seen, current, previous, false));
        assert_eq!((now.current, now.previous), found());
        assert_eq!(now.latest.map(|l| l.tag).as_deref(), Some("v1.19.32"));
        // A rollback ran while the poll hashed and recorded its result: that stays.
        let job = State {
            current: rec("b", "uploaded"),
            previous: rec("a", "official"),
            latest: None,
        };
        let mut now = job.clone();
        let (current, previous) = found();
        assert!(!relabel(&mut now, &seen, current, previous, false));
        assert_eq!((now.current, now.previous), (job.current, job.previous));
        // A job still running has not recorded anything yet.
        let mut now = seen.clone();
        let (current, previous) = found();
        assert!(!relabel(&mut now, &seen, current, previous, true));
        assert_eq!((now.current, now.previous), (seen.current, seen.previous));
    }
    #[test]
    fn uploads_are_capped() {
        assert_eq!(
            upload_cap(Some(MAX_UPLOAD + 1), u64::MAX).unwrap_err(),
            TOO_LARGE
        );
        assert_eq!(upload_cap(Some(MAX_UPLOAD), u64::MAX).unwrap(), MAX_UPLOAD);
        assert_eq!(upload_cap(None, u64::MAX).unwrap(), MAX_UPLOAD);
        assert_eq!(upload_cap(Some(10 * MIB), 20 * MIB).unwrap_err(), NO_SPACE);
        assert_eq!(upload_cap(None, 20 * MIB).unwrap(), 4 * MIB);
        assert_eq!(upload_cap(None, MARGIN).unwrap_err(), NO_SPACE);
        let dir = scratch("upload");
        let dest = dir.join("part");
        let seen = AtomicU64::new(0);
        let report = |n: u64| seen.store(n, Ordering::Relaxed);
        let body = vec![7u8; 200_000];
        let mut reader = Cursor::new(&body);
        assert_eq!(
            receive(&mut reader, Some(200_000), &dest, MAX_UPLOAD, &report),
            Ok(200_000)
        );
        assert_eq!(fs::read(&dest).unwrap(), body);
        assert_eq!(seen.load(Ordering::Relaxed), 200_000);
        let mut reader = Cursor::new(&body);
        assert_eq!(
            receive(&mut reader, None, &dest, 100_000, &report).unwrap_err(),
            NO_SPACE
        );
        let mut reader = Cursor::new(&body);
        assert_eq!(
            receive(&mut reader, Some(300_000), &dest, MAX_UPLOAD, &report).unwrap_err(),
            INTERRUPTED
        );
        let mut reader = Cursor::new(Vec::new());
        assert_eq!(
            receive(&mut reader, None, &dest, MAX_UPLOAD, &report).unwrap_err(),
            EMPTY
        );
        // A body without a length stops at the 64 MiB cap.
        let mut endless = io::repeat(0).take(MAX_UPLOAD + 1);
        assert_eq!(
            receive(&mut endless, None, &dest, MAX_UPLOAD, &|_| {}).unwrap_err(),
            TOO_LARGE
        );
        fs::remove_dir_all(&dir).unwrap();
    }
    #[test]
    fn core_swaps_never_leave_the_core_missing() {
        let dir = scratch("swap");
        let core = dir.join("mihomo");
        let previous = dir.join("mihomo.previous");
        let (link, staged, swap) = (dir.join(".link"), dir.join(".update"), dir.join(".swap"));
        let aside = dir.join(".old");
        let read = |p: &Path| fs::read_to_string(p).ok();
        let both = || (read(&core), read(&previous));
        let some = |a: &str, b: &str| (Some(a.to_string()), Some(b.to_string()));
        fs::write(&staged, "A").unwrap();
        assert!(!swap_in(&core, &previous, &link, &aside, &staged).unwrap());
        assert_eq!(both(), (Some("A".into()), None));
        fs::write(&staged, "B").unwrap();
        assert!(swap_in(&core, &previous, &link, &aside, &staged).unwrap());
        assert_eq!(both(), some("B", "A"));
        assert!(!aside.exists());
        fs::write(&staged, "C").unwrap();
        assert!(swap_in(&core, &previous, &link, &aside, &staged).unwrap());
        assert_eq!(both(), some("C", "B"));
        // The older previous core waits beside them until the new one is confirmed.
        assert_eq!(read(&aside).as_deref(), Some("A"));
        fs::remove_file(&aside).unwrap();
        assert!(!staged.exists() && !link.exists());
        exchange(&core, &previous, &swap).unwrap();
        assert_eq!(both(), some("B", "C"));
        exchange(&core, &previous, &swap).unwrap();
        assert_eq!(both(), some("C", "B"));
        assert!(!swap.exists());
        // The previous core keeps its identity across the hard link.
        let key = |p: &Path| FileKey::of(&fs::metadata(p).unwrap());
        let before = key(&core);
        fs::write(&staged, "D").unwrap();
        swap_in(&core, &previous, &link, &aside, &staged).unwrap();
        assert_eq!(key(&previous), before);
        fs::remove_dir_all(&dir).unwrap();
    }
    #[test]
    fn an_unconfirmed_install_is_undone_and_keeps_the_rollback_target() {
        let dir = scratch("aside");
        let core = dir.join("mihomo");
        let previous = dir.join("mihomo.previous");
        let (link, aside, staged) = (dir.join(".link"), dir.join(".old"), dir.join(".update"));
        let read = |p: &Path| fs::read_to_string(p).ok();
        let files = || (read(&core), read(&previous), read(&aside));
        let files_are = |c: Option<&str>, p: Option<&str>, a: Option<&str>| {
            assert_eq!(
                files(),
                (
                    c.map(String::from),
                    p.map(String::from),
                    a.map(String::from)
                )
            );
        };
        // New files each time: writing through a hard link would change both names.
        let put = |p: &Path, text: &str| {
            let _ = fs::remove_file(p);
            fs::write(p, text).unwrap();
        };
        let link_to = |src: &Path, dst: &Path| {
            let _ = fs::remove_file(dst);
            fs::hard_link(src, dst).unwrap();
        };
        // Live core O, rollback target P.
        let start = || {
            put(&core, "O");
            put(&previous, "P");
            let _ = fs::remove_file(&aside);
        };

        // U fails to start: O is live again and P is still the rollback target.
        start();
        put(&staged, "U");
        assert!(swap_in(&core, &previous, &link, &aside, &staged).unwrap());
        files_are(Some("U"), Some("O"), Some("P"));
        swap_back(&core, &previous, &aside).unwrap();
        files_are(Some("O"), Some("P"), None);
        // A swap that fails before the new core is live (here: it is gone) undoes itself.
        assert!(swap_in(&core, &previous, &link, &aside, &staged).is_err());
        files_are(Some("O"), Some("P"), None);
        assert!(!link.exists());

        // Cut off at any step, the install is undone at the next start.
        start();
        link_to(&previous, &aside); // P set aside
        swap_back(&core, &previous, &aside).unwrap();
        files_are(Some("O"), Some("P"), None);
        start();
        link_to(&previous, &aside);
        link_to(&core, &previous); // and replaced by a link of O
        swap_back(&core, &previous, &aside).unwrap();
        files_are(Some("O"), Some("P"), None);
        start();
        link_to(&previous, &aside);
        link_to(&core, &previous);
        put(&staged, "U");
        fs::rename(&staged, &core).unwrap(); // U live, not confirmed
        swap_back(&core, &previous, &aside).unwrap();
        files_are(Some("O"), Some("P"), None);
        start();
        link_to(&previous, &aside);
        fs::remove_file(&previous).unwrap(); // an undo that had put O back
        swap_back(&core, &previous, &aside).unwrap();
        files_are(Some("O"), Some("P"), None);

        // Without an older previous core, the failed install leaves none.
        start();
        fs::remove_file(&previous).unwrap();
        put(&staged, "U");
        assert!(swap_in(&core, &previous, &link, &aside, &staged).unwrap());
        files_are(Some("U"), Some("O"), None);
        swap_back(&core, &previous, &aside).unwrap();
        files_are(Some("O"), None, None);
        // Nor when `previous` was only another link of the live core.
        start();
        link_to(&core, &previous);
        put(&staged, "U");
        assert!(swap_in(&core, &previous, &link, &aside, &staged).unwrap());
        files_are(Some("U"), Some("O"), None);
        swap_back(&core, &previous, &aside).unwrap();
        files_are(Some("O"), None, None);
        fs::remove_dir_all(&dir).unwrap();
    }
    #[test]
    fn geo_files_are_checked_and_replaced_with_a_kept_copy() {
        let dir = scratch("geo");
        let (new, share, keep, home) = (
            dir.join("new"),
            dir.join("share"),
            dir.join("keep"),
            dir.join("home"),
        );
        let live = dir.join("live");
        for d in [&new, &share, &home, &live] {
            fs::create_dir_all(d).unwrap();
        }
        let mut mmdb = vec![0u8; 200 * 1024];
        mmdb.extend_from_slice(MMDB_MARKER);
        mmdb.extend_from_slice(&[0u8; 1000]);
        fs::write(new.join("geoip.metadb"), &mmdb).unwrap();
        fs::write(new.join("GeoSite.dat"), vec![1u8; 4096]).unwrap();
        assert!(check_geo("geoip.metadb", &new.join("geoip.metadb")).is_ok());
        assert!(check_geo("GeoSite.dat", &new.join("GeoSite.dat")).is_ok());
        let mut buried = MMDB_MARKER.to_vec();
        buried.extend(vec![0u8; 129 * 1024]);
        fs::write(dir.join("buried.mmdb"), &buried).unwrap();
        assert_eq!(
            check_geo("ASN.mmdb", &dir.join("buried.mmdb")).unwrap_err(),
            "Geo data is not a MaxMind database"
        );
        fs::write(dir.join("tiny.dat"), "x").unwrap();
        assert_eq!(
            check_geo("GeoSite.dat", &dir.join("tiny.dat")).unwrap_err(),
            "Geo data is too small"
        );
        // The test home sees the new files, other deployed ones and the core's own downloads.
        fs::write(share.join("geoip.metadb"), "old").unwrap();
        fs::write(share.join("ASN.mmdb"), "asn").unwrap();
        fs::write(live.join("GeoIP.dat"), "fetched").unwrap();
        fs::write(live.join("geosite.dat"), "stale").unwrap();
        let names = ["geoip.metadb", "GeoSite.dat"];
        test_home(&new, &share, &live, &home, &names).unwrap();
        let target = |n: &str| fs::read_link(home.join(n)).ok();
        assert_eq!(target("geoip.metadb"), Some(new.join("geoip.metadb")));
        assert_eq!(target("GeoSite.dat"), Some(new.join("GeoSite.dat")));
        assert_eq!(target("ASN.mmdb"), Some(share.join("ASN.mmdb")));
        assert_eq!(target("GeoIP.dat"), Some(live.join("GeoIP.dat")));
        assert!(!home.join("geosite.dat").exists());
        // Only geoip existed before: it is kept, and restoring removes the added geosite.
        let mut change = GeoChange::default();
        replace_geo(&new, &share, &keep, &names, &mut change).unwrap();
        assert_eq!(
            change,
            GeoChange {
                replaced: vec!["geoip.metadb"],
                added: vec!["GeoSite.dat"]
            }
        );
        assert_eq!(fs::read(share.join("geoip.metadb")).unwrap(), mmdb);
        assert_eq!(
            fs::read_to_string(keep.join("geoip.metadb")).unwrap(),
            "old"
        );
        assert!(!keep.join("GeoSite.dat").exists() && !new.join("geoip.metadb").exists());
        restore_geo(&share, &keep, &change);
        assert_eq!(
            fs::read_to_string(share.join("geoip.metadb")).unwrap(),
            "old"
        );
        assert!(!share.join("GeoSite.dat").exists());
        assert_eq!(fs::read_to_string(share.join("ASN.mmdb")).unwrap(), "asn");
        fs::remove_dir_all(&dir).unwrap();
    }
    #[test]
    fn a_failed_geo_swap_restores_only_what_it_replaced() {
        let dir = scratch("geo-fail");
        let (new, share, keep) = (dir.join("new"), dir.join("share"), dir.join("keep"));
        fs::create_dir_all(share.join("sub")).unwrap();
        let read = |n: &str| fs::read_to_string(share.join(n)).ok();
        let fill = |names: &[&str]| {
            let _ = fs::remove_dir_all(&new);
            fs::create_dir_all(new.join("sub")).unwrap();
            for n in names {
                fs::write(share.join(n), format!("old {n}")).unwrap();
                fs::write(new.join(n), format!("new {n}")).unwrap();
            }
        };
        let names = ["geoip.metadb", "GeoSite.dat"];
        let swap = |keep: &Path, names: &[&'static str]| {
            let mut change = GeoChange::default();
            assert!(replace_geo(&new, &share, keep, names, &mut change).is_err());
            restore_geo(&share, keep, &change);
            change
        };
        // `keep` cannot be created: nothing was replaced, so nothing is removed.
        fill(&names);
        fs::write(dir.join("file"), "").unwrap();
        assert_eq!(swap(&dir.join("file/keep"), &names), GeoChange::default());
        assert_eq!(read("geoip.metadb").as_deref(), Some("old geoip.metadb"));
        assert_eq!(read("GeoSite.dat").as_deref(), Some("old GeoSite.dat"));
        // Keeping the second file fails (`keep` has no such subdirectory) after the first
        // was kept: both live files stay.
        let nested = ["geoip.metadb", "sub/GeoSite.dat"];
        fill(&nested);
        assert_eq!(swap(&keep, &nested), GeoChange::default());
        assert_eq!(read("geoip.metadb").as_deref(), Some("old geoip.metadb"));
        assert_eq!(
            read("sub/GeoSite.dat").as_deref(),
            Some("old sub/GeoSite.dat")
        );
        // The second move fails after the first file was replaced: that one comes back,
        // the other is as it was.
        fill(&names);
        fs::remove_file(new.join("GeoSite.dat")).unwrap();
        let change = swap(&keep, &names);
        assert_eq!(change.replaced, names);
        assert_eq!(read("geoip.metadb").as_deref(), Some("old geoip.metadb"));
        assert_eq!(read("GeoSite.dat").as_deref(), Some("old GeoSite.dat"));
        assert!(!share.join(".GeoSite.dat.restore").exists());
        // The same when the second file did not exist before: there is still none.
        fill(&names[..1]);
        fs::remove_file(share.join("GeoSite.dat")).unwrap();
        let change = swap(&keep, &names);
        assert_eq!(
            (change.replaced, change.added),
            (vec![names[0]], vec![names[1]])
        );
        assert_eq!(read("geoip.metadb").as_deref(), Some("old geoip.metadb"));
        assert!(!share.join("GeoSite.dat").exists());
        fs::remove_dir_all(&dir).unwrap();
    }
    #[test]
    fn moves_copy_across_filesystems() {
        let dir = scratch("move");
        let other = Path::new("/dev/shm");
        let device = |p: &Path| fs::metadata(p).map(|m| m.dev()).ok();
        // Only meaningful where a second filesystem is at hand.
        if device(other).is_none() || device(other) == device(&dir) {
            fs::remove_dir_all(&dir).unwrap();
            return;
        }
        let src = other.join(format!("openui-core-move-{}", std::process::id()));
        fs::write(&src, "new").unwrap();
        fs::write(dir.join("geoip.metadb"), "old").unwrap();
        move_file(&src, &dir.join("geoip.metadb")).unwrap();
        assert_eq!(fs::read_to_string(dir.join("geoip.metadb")).unwrap(), "new");
        assert!(!src.exists() && !dir.join(".geoip.metadb.new").exists());
        fs::remove_dir_all(&dir).unwrap();
    }
    #[test]
    fn live_download() {
        // `OPENUI_CLASH_TEST_CORE_URL=latest cargo test live_download` resolves the latest
        // release through api.github.com and verifies its digest; any other value is a .gz
        // core URL, checked against OPENUI_CLASH_TEST_CORE_SHA256 when that is set.
        let Ok(url) = std::env::var("OPENUI_CLASH_TEST_CORE_URL") else {
            return;
        };
        let dir = scratch("live");
        let (url, digest) = if url == "latest" {
            let json = dir.join("release.json");
            download(RELEASE_API, None, &API_HEADERS, &json, 2 * MIB, 60).unwrap();
            let latest = parse_release(&fs::read(&json).unwrap(), now()).unwrap();
            let digest = latest.digest.strip_prefix("sha256:").map(String::from);
            (latest.url, digest)
        } else {
            (url, std::env::var("OPENUI_CLASH_TEST_CORE_SHA256").ok())
        };
        let gz = dir.join("core.gz");
        download(&url, None, &[], &gz, MAX_UPLOAD, 600).unwrap();
        if let Some(digest) = digest {
            assert_eq!(sha256_file(&gz).unwrap(), digest);
        }
        let core = dir.join("core");
        let size = gunzip(&gz, &core, MAX_CORE).unwrap();
        check_elf(&head(&core), EM_AARCH64).unwrap();
        eprintln!("{url}: {} bytes, unpacked {size}", file_len(&gz));
        fs::remove_dir_all(&dir).unwrap();
    }
}
