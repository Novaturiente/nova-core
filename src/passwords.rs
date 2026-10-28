//! Password managers: `rbw` (Bitwarden), `bw` (Bitwarden CLI) and `pass` behind one
//! small contract — list entries for a site, secret, TOTP, save, update. We never keep
//! secrets at rest; they pass through memory on user action. Every call here blocks on
//! a CLI: run them on worker threads (the editions' UI parts do).
use crate::config;
use serde_json::Value;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Backend {
    Rbw,
    Bw,
    Pass,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub id: String,
    pub name: String,
    pub user: String,
    pub uris: Vec<String>,
}

pub fn backend() -> Option<Backend> {
    match config::get().str("passwords.backend").as_str() {
        "rbw" => Some(Backend::Rbw),
        "bw" => Some(Backend::Bw),
        "pass" => Some(Backend::Pass),
        _ => None,
    }
}

pub fn label(b: Backend) -> &'static str {
    match b {
        Backend::Rbw | Backend::Bw => "Bitwarden",
        Backend::Pass => "pass",
    }
}

/// Registrable domain ("base domain", Bitwarden's default match) of a URL or a
/// bare host as stored in vault URIs ("github.com", "100.88.1.2:8080/x").
pub fn site(uri: &str) -> Option<String> {
    let uri = uri.trim();
    let full = if uri.contains("://") {
        uri.to_string()
    } else {
        format!("https://{uri}")
    };
    if !(full.starts_with("http://") || full.starts_with("https://")) {
        return None; // androidapp://, ssh://, …
    }
    adblock::url_parser::parse_url(&full)
        .map(|u| u.domain().to_string())
        .filter(|d| !d.is_empty())
}

pub fn host(url: &str) -> String {
    crate::commands::host(url).to_string()
}

/// scheme://host[:port] of a URL (no userinfo, path or query).
fn origin(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("https", url));
    let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
    format!("{scheme}://{}", auth.rsplit_once('@').map_or(auth, |(_, h)| h))
}

// ---- running the CLIs ----

fn run(prog: &str, args: &[&str], stdin: Option<&str>, env: &[(&str, &str)]) -> Result<String, String> {
    let mut cmd = Command::new(prog);
    cmd.args(args).stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() });
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().map_err(|e| format!("{prog}: {e}"))?;
    if let Some(input) = stdin {
        let mut i = child.stdin.take().unwrap();
        i.write_all(input.as_bytes()).map_err(|e| format!("{prog}: {e}"))?;
    }
    let out = child.wait_with_output().map_err(|e| format!("{prog}: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.trim().lines().last().unwrap_or("failed");
        return Err(format!("{prog} {}: {err}", args.first().unwrap_or(&"")));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn json(text: &str) -> Result<Value, String> {
    serde_json::from_str(text).map_err(|e| format!("unexpected output: {e}"))
}

fn s(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

fn pass_dir() -> PathBuf {
    std::env::var_os("PASSWORD_STORE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").expect("HOME")).join(".password-store"))
}

/// Metadata only (no secrets kept). Cached: `bw` takes about a second per call.
static CACHE: Mutex<Option<(Instant, Backend, Vec<Entry>)>> = Mutex::new(None);

fn list_all(b: Backend) -> Result<Vec<Entry>, String> {
    if let Some((t, cb, e)) = &*CACHE.lock().unwrap() {
        if *cb == b && t.elapsed() < Duration::from_secs(60) {
            return Ok(e.clone());
        }
    }
    let entries: Vec<Entry> = match b {
        Backend::Rbw => json(&run("rbw", &["list", "--raw"], None, &[])?)?
            .as_array()
            .into_iter()
            .flatten()
            .filter(|e| e["type"] == "Login")
            .map(|e| Entry {
                id: s(&e["id"]),
                name: s(&e["name"]),
                user: s(&e["user"]),
                uris: e["uris"].as_array().into_iter().flatten().map(s).collect(),
            })
            .collect(),
        // shortcut: `bw list items` also returns secrets; we drop them right here.
        Backend::Bw => json(&run("bw", &["list", "items"], None, &[])?)?
            .as_array()
            .into_iter()
            .flatten()
            .filter(|e| e["type"] == 1)
            .map(|e| Entry {
                id: s(&e["id"]),
                name: s(&e["name"]),
                user: s(&e["login"]["username"]),
                uris: e["login"]["uris"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|u| s(&u["uri"]))
                    .collect(),
            })
            .collect(),
        Backend::Pass => pass_entries(),
    };
    *CACHE.lock().unwrap() = Some((Instant::now(), b, entries.clone()));
    Ok(entries)
}

/// pass: `<site>/<user>.gpg` or `<anything>/<site>.gpg`; path parts with a dot are sites.
fn pass_entries() -> Vec<Entry> {
    let root = pass_dir();
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else if let Some(stem) = name.strip_suffix(".gpg") {
                let id = p
                    .strip_prefix(&root)
                    .unwrap()
                    .with_extension("")
                    .to_string_lossy()
                    .into_owned();
                let parts: Vec<&str> = id.split('/').collect();
                let uris: Vec<String> = parts.iter().filter(|x| x.contains('.')).map(|x| x.to_string()).collect();
                let parent_is_site = parts.len() > 1 && parts[parts.len() - 2].contains('.');
                let user = if parent_is_site { stem.to_string() } else { String::new() };
                out.push(Entry { id: id.clone(), name: id, user, uris });
            }
        }
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// Locked vault? Listing then would make rbw pop its pinentry on a mere focus, so
/// the editions ask first and unlock only on a user action.
pub fn locked(b: Backend) -> bool {
    match b {
        Backend::Rbw => Command::new("rbw")
            .arg("unlocked")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| !s.success()),
        Backend::Bw => run("bw", &["status"], None, &[])
            .ok()
            .and_then(|t| json(&t).ok())
            .is_some_and(|v| v["status"] != "unlocked"),
        Backend::Pass => false, // gpg-agent asks on the first decrypt
    }
}

/// Unlock with the manager's own dialog (rbw: its configured pinentry).
pub fn unlock(b: Backend) -> Result<(), String> {
    match b {
        Backend::Rbw => run("rbw", &["unlock"], None, &[]).map(|_| ()),
        Backend::Bw => Err(format!(
            "bw is locked: run `bw unlock`, export BW_SESSION and restart {}",
            crate::paths::app()
        )),
        Backend::Pass => Ok(()),
    }
}

/// Entries whose URI shares the registrable domain of `url`; same host first.
pub fn matching(b: Backend, url: &str) -> Result<Vec<Entry>, String> {
    let Some(want) = site(url) else {
        return Ok(Vec::new());
    };
    let h = host(url);
    let mut v: Vec<Entry> = list_all(b)?
        .into_iter()
        .filter(|e| e.uris.iter().any(|u| site(u).as_deref() == Some(&want)))
        .collect();
    let exact = |e: &Entry| {
        e.uris.iter().any(|u| {
            site(u).is_some() && host(&format!("https://{}", u.split("://").last().unwrap_or(u))) == h
        })
    };
    v.sort_by_key(|e| !exact(e));
    Ok(v)
}

/// (username, password)
pub fn secret(b: Backend, e: &Entry) -> Result<(String, String), String> {
    match b {
        Backend::Rbw => {
            let d = json(&run("rbw", &["get", "--raw", &e.id], None, &[])?)?;
            Ok((s(&d["data"]["username"]), s(&d["data"]["password"])))
        }
        Backend::Bw => {
            let d = json(&run("bw", &["get", "item", &e.id], None, &[])?)?;
            Ok((s(&d["login"]["username"]), s(&d["login"]["password"])))
        }
        Backend::Pass => {
            let text = run("pass", &["show", &e.id], None, &[])?;
            let mut lines = text.lines();
            let pw = lines.next().unwrap_or_default().to_string();
            let user = lines
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    matches!(k.trim().to_lowercase().as_str(), "login" | "user" | "username" | "email")
                        .then(|| v.trim().to_string())
                })
                .unwrap_or_else(|| e.user.clone());
            Ok((user, pw))
        }
    }
}

pub fn totp(b: Backend, e: &Entry) -> Result<String, String> {
    let out = match b {
        Backend::Rbw => run("rbw", &["code", &e.id], None, &[])?,
        Backend::Bw => run("bw", &["get", "totp", &e.id], None, &[])?,
        Backend::Pass => run("pass", &["otp", &e.id], None, &[])?,
    };
    Ok(out.trim().to_string())
}

/// 20 chars from /dev/urandom: letters, digits and a few symbols sites accept.
pub fn generate() -> String {
    const CHARS: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789-_.!@#%";
    let mut buf = [0u8; 64];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut buf))
        .expect("/dev/urandom");
    // 256 % 64 == 0: no modulo bias
    buf.iter().take(20).map(|b| CHARS[*b as usize % CHARS.len()] as char).collect()
}

/// `$VISUAL` for `rbw add`/`rbw edit`: the browser binary itself in editor mode, the
/// secret in its environment (readable by this user only; never argv).
fn rbw_editor(mode: &str, secret: &str) -> Result<Vec<(&'static str, String)>, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    Ok(vec![
        ("VISUAL", exe.to_string_lossy().into_owned()),
        ("EDITOR", exe.to_string_lossy().into_owned()),
        ("NOVA_PW_SECRET", secret.to_string()),
        ("NOVA_PW_MODE", mode.to_string()),
    ])
}

/// Editor mode (see rbw_editor): put the secret on the first line of the file.
/// "replace" swaps the old password line (edit), "insert" adds it on top (add).
pub fn editor_main(path: &str) -> i32 {
    let (Ok(secret), Ok(mode)) = (std::env::var("NOVA_PW_SECRET"), std::env::var("NOVA_PW_MODE")) else {
        return 2;
    };
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let rest = match mode.as_str() {
        "replace" => text.split_once('\n').map(|(_, r)| r).unwrap_or(""),
        _ => text.as_str(),
    };
    match std::fs::write(path, format!("{secret}\n{rest}")) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

fn with_env<'a>(v: &'a [(&'static str, String)]) -> Vec<(&'a str, &'a str)> {
    v.iter().map(|(k, x)| (*k, x.as_str())).collect()
}

pub fn save(b: Backend, url: &str, user: &str, pw: &str) -> Result<(), String> {
    let h = host(url);
    let origin = origin(url);
    match b {
        Backend::Rbw => {
            let env = rbw_editor("insert", pw)?;
            let mut args = vec!["add", "--uri", origin.as_str(), h.as_str()];
            if !user.is_empty() {
                args.push(user);
            }
            run("rbw", &args, None, &with_env(&env))?;
        }
        Backend::Bw => {
            let item = serde_json::json!({
                "type": 1, "name": h, "notes": null, "favorite": false,
                "login": { "username": user, "password": pw, "uris": [{ "uri": origin, "match": null }] },
            });
            let enc = run("bw", &["encode"], Some(&item.to_string()), &[])?;
            run("bw", &["create", "item"], Some(enc.trim()), &[])?;
        }
        Backend::Pass => {
            let name = if user.is_empty() { h.clone() } else { format!("{h}/{}", user.replace('/', "_")) };
            if pass_dir().join(format!("{name}.gpg")).exists() {
                return Err(format!("pass: {name} already exists"));
            }
            run(
                "pass",
                &["insert", "-m", &name],
                Some(&format!("{pw}\nlogin: {user}\nurl: {origin}\n")),
                &[],
            )?;
        }
    }
    *CACHE.lock().unwrap() = None;
    Ok(())
}

pub fn update(b: Backend, e: &Entry, pw: &str) -> Result<(), String> {
    match b {
        Backend::Rbw => {
            let env = rbw_editor("replace", pw)?;
            run("rbw", &["edit", &e.id], None, &with_env(&env))?;
        }
        Backend::Bw => {
            let mut item = json(&run("bw", &["get", "item", &e.id], None, &[])?)?;
            item["login"]["password"] = pw.into();
            let enc = run("bw", &["encode"], Some(&item.to_string()), &[])?;
            run("bw", &["edit", "item", &e.id], Some(enc.trim()), &[])?;
        }
        Backend::Pass => {
            let old = run("pass", &["show", &e.id], None, &[])?;
            let rest = old.split_once('\n').map(|(_, r)| r).unwrap_or("");
            run("pass", &["insert", "-m", "-f", &e.id], Some(&format!("{pw}\n{rest}")), &[])?;
        }
    }
    *CACHE.lock().unwrap() = None;
    Ok(())
}

/// None = this password is stored already; Some(entry) = same user, other password
/// (update); Some(None) = new login (save).
pub fn known(b: Backend, url: &str, user: &str, pass: &str) -> Result<Option<Option<Entry>>, String> {
    let mut same_user = None;
    for e in matching(b, url)?.iter().filter(|e| user.is_empty() || e.user == user) {
        if secret(b, e)?.1 == pass {
            return Ok(None);
        }
        same_user.get_or_insert(e.clone());
    }
    Ok(Some(same_user))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_matches_base_domain() {
        assert_eq!(site("https://login.github.com/x").as_deref(), Some("github.com"));
        assert_eq!(site("github.com").as_deref(), Some("github.com"));
        assert_eq!(site("accounts.google.co.uk:443/a").as_deref(), Some("google.co.uk"));
        assert_eq!(site("http://129.159.225.36").as_deref(), Some("129.159.225.36"));
        assert_eq!(site("androidapp://com.x"), None);
        assert_ne!(site("evilgithub.com"), site("github.com"));
    }

    #[test]
    fn generated_passwords_differ() {
        let (a, b) = (generate(), generate());
        assert_eq!(a.len(), 20);
        assert_ne!(a, b);
    }

    #[test]
    fn editor_puts_secret_first() {
        let p = std::env::temp_dir().join(format!("nova-pw-ed-{}", std::process::id()));
        std::fs::write(&p, "old\nnotes line\n").unwrap();
        // SAFETY: test-only env; tests in this module don't read these concurrently
        unsafe {
            std::env::set_var("NOVA_PW_SECRET", "s3cret");
            std::env::set_var("NOVA_PW_MODE", "replace");
        }
        assert_eq!(editor_main(p.to_str().unwrap()), 0);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "s3cret\nnotes line\n");
        unsafe { std::env::set_var("NOVA_PW_MODE", "insert") };
        std::fs::write(&p, "# help\n").unwrap();
        editor_main(p.to_str().unwrap());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "s3cret\n# help\n");
        std::fs::remove_file(p).unwrap();
    }
}
