//! aria2 download backend (`downloads.backend` = auto | aria2 | <engine>).
//! We start our own `aria2c` with JSON-RPC on 127.0.0.1 (random port, random secret),
//! hand it GET downloads (with the page's cookies, user agent and referer) and magnet:
//! links, and poll progress on a thread for the downloads pill/page. Unfinished
//! downloads live in `<data>/aria2.session` and resume on the next start; quitting shuts
//! aria2 down. Torrents stop seeding when done (`seed-time=0`).
//! Results reach the edition on the UI thread through the two callbacks given to `start`.
use crate::{config, host, paths::xdg_dir};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::{mpsc, OnceLock};
use std::time::Duration;

/// Every download aria2 knows (status poll), on the UI thread.
pub type OnUpdate = fn(Vec<Value>);
/// aria2 took (gid) or refused request `id`, on the UI thread.
pub type OnAdded = fn(u64, Result<String, String>);

struct Rpc {
    port: u16,
    secret: String,
    on_update: OnUpdate,
    on_added: OnAdded,
}

enum Job {
    Add { id: u64, uris: Vec<String>, opts: Value },
    Remove(String),
}

static RPC: OnceLock<Rpc> = OnceLock::new();
static TX: OnceLock<mpsc::Sender<Job>> = OnceLock::new();

const KEYS: &[&str] = &[
    "gid",
    "status",
    "totalLength",
    "completedLength",
    "files",
    "bittorrent",
    "followedBy",
    "errorMessage",
    "dir",
];

fn find_aria2c() -> Option<PathBuf> {
    std::env::var_os("PATH")?
        .to_str()?
        .split(':')
        .map(|d| PathBuf::from(d).join("aria2c"))
        .find(|p| p.is_file())
}

/// Start aria2c unless `downloads.backend` names the engine or aria2c isn't installed.
pub fn start(engine_backend: &str, on_update: OnUpdate, on_added: OnAdded) {
    let backend = config::get().str("downloads.backend");
    if backend == engine_backend || backend == "chromium" || backend == "webkit" {
        return;
    }
    let Some(bin) = find_aria2c() else {
        if backend == "aria2" {
            eprintln!("{}: downloads.backend = aria2 but aria2c isn't installed", crate::paths::app());
        }
        return;
    };
    let port = match std::net::TcpListener::bind("127.0.0.1:0").and_then(|l| l.local_addr()) {
        Ok(a) => a.port(),
        Err(e) => return eprintln!("{}: aria2: no free port: {e}", crate::paths::app()),
    };
    let secret = random_hex();
    // holds cookies of unfinished downloads; private via the 0700 data dir
    let session = xdg_dir("XDG_DATA_HOME", ".local/share").join("aria2.session");
    if !session.exists() {
        let _ = std::fs::write(&session, "");
    }
    let s = session.to_string_lossy();
    let child = std::process::Command::new(bin)
        .args([
            "--enable-rpc=true".into(),
            format!("--rpc-listen-port={port}"),
            format!("--rpc-secret={secret}"),
            format!("--input-file={s}"),
            format!("--save-session={s}"),
            "--save-session-interval=10".into(),
            // aria2 quits by itself if we crash
            format!("--stop-with-process={}", std::process::id()),
            // not --continue: it treats any same-named file as a partial copy of this one;
            // session resume works through aria2's .aria2 control files anyway
            "--max-connection-per-server=8".into(),
            "--split=8".into(),
            "--min-split-size=1M".into(),
            "--content-disposition-default-utf8=true".into(),
            "--auto-file-renaming=true".into(),
            "--file-allocation=falloc".into(),
            "--seed-time=0".into(),
            "--quiet=true".into(),
            format!(
                "--log={}",
                xdg_dir("XDG_STATE_HOME", ".local/state").join("aria2.log").display()
            ),
            "--log-level=warn".into(),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Err(e) = child {
        return eprintln!("{}: aria2: can't start aria2c: {e}", crate::paths::app());
    }
    RPC.set(Rpc { port, secret, on_update, on_added }).ok();
    let (tx, rx) = mpsc::channel();
    TX.set(tx).ok();
    std::thread::Builder::new()
        .name("aria2".into())
        .spawn(move || run(rx))
        .expect("spawn aria2 thread");
}

pub fn enabled() -> bool {
    RPC.get().is_some()
}

fn random_hex() -> String {
    let mut b = [0u8; 16];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .expect("/dev/urandom");
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// One JSON-RPC call (HTTP/1.1, Connection: close).
fn call(method: &str, params: Vec<Value>) -> Result<Value, String> {
    let rpc = RPC.get().ok_or("aria2 not running")?;
    let mut p = vec![json!(format!("token:{}", rpc.secret))];
    p.extend(params);
    let body = json!({ "jsonrpc": "2.0", "id": "n", "method": method, "params": p }).to_string();
    let mut s = TcpStream::connect(("127.0.0.1", rpc.port)).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
    write!(
        s,
        "POST /jsonrpc HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .map_err(|e| e.to_string())?;
    let mut resp = String::new();
    s.read_to_string(&mut resp).map_err(|e| e.to_string())?;
    let json = resp.split_once("\r\n\r\n").map_or("", |(_, b)| b);
    let v: Value = serde_json::from_str(json).map_err(|e| format!("bad reply: {e}"))?;
    match v.get("error") {
        Some(e) => Err(e["message"].as_str().unwrap_or("error").to_string()),
        None => Ok(v["result"].clone()),
    }
}

/// Worker: runs jobs, polls every second while something is going on, else
/// sleeps until a new job (or 30 s, for downloads resumed from the session).
fn run(rx: mpsc::Receiver<Job>) {
    for _ in 0..50 {
        if call("aria2.getVersion", vec![]).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let rpc = RPC.get().expect("aria2 rpc");
    let (on_update, on_added) = (rpc.on_update, rpc.on_added);
    let mut busy = true;
    loop {
        let wait = Duration::from_secs(if busy { 1 } else { 30 });
        match rx.recv_timeout(wait) {
            Ok(Job::Add { id, uris, opts }) => {
                let r = call("aria2.addUri", vec![json!(uris), opts])
                    .map(|g| g.as_str().unwrap_or("").to_string());
                host::on_ui(move || on_added(id, r));
            }
            Ok(Job::Remove(gid)) => {
                let _ = call("aria2.remove", vec![json!(gid)]);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
        busy = poll(on_update);
    }
}

/// Report every download aria2 knows; finished ones are reported once, then purged.
fn poll(on_update: OnUpdate) -> bool {
    let keys = json!(KEYS);
    let mut items = Vec::new();
    for (m, p) in [
        ("aria2.tellActive", vec![keys.clone()]),
        ("aria2.tellWaiting", vec![json!(0), json!(1000), keys.clone()]),
        ("aria2.tellStopped", vec![json!(0), json!(1000), keys]),
    ] {
        if let Ok(Value::Array(a)) = call(m, p) {
            items.extend(a);
        }
    }
    let busy = items
        .iter()
        .any(|i| matches!(i["status"].as_str(), Some("active" | "waiting")));
    let stopped: Vec<String> = items
        .iter()
        .filter(|i| matches!(i["status"].as_str(), Some("complete" | "error" | "removed")))
        .filter_map(|i| i["gid"].as_str().map(str::to_string))
        .collect();
    if !items.is_empty() {
        host::on_ui(move || on_update(items));
    }
    for gid in stopped {
        let _ = call("aria2.removeDownloadResult", vec![json!(gid)]);
    }
    busy
}

fn send(job: Job) {
    if let Some(tx) = TX.get() {
        let _ = tx.send(job);
    }
}

/// Hand `url` to aria2 into `dir` with the request headers the tab would send
/// (`Cookie: …`, `Referer: …`). Async: the result arrives through `on_added(id, …)`.
pub fn add(url: String, id: u64, dir: &str, headers: Vec<String>, user_agent: Option<String>) {
    let mut opts = json!({ "dir": dir, "header": headers });
    if let Some(ua) = user_agent.filter(|u| !u.is_empty()) {
        opts["user-agent"] = json!(ua);
    }
    send(Job::Add { id, uris: vec![url], opts });
}

pub fn remove(gid: &str) {
    send(Job::Remove(gid.to_string()));
}

/// Quit: let aria2 save its session (unfinished downloads resume next start).
pub fn stop() {
    if enabled() {
        let _ = call("aria2.shutdown", vec![]);
    }
}
