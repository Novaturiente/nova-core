//! Request policy shared by both editions: HTTPS-only upgrades and excludes, cookie
//! decisions per first party, request headers, Referer for our own downloads.
use crate::config;
use serde_json::Value;
use std::collections::BTreeSet;
use std::net::IpAddr;
use std::sync::{Mutex, RwLock};

/// `host` or `host:port` lines from <config>/https_excludes: left on http.
static HTTPS_EXCLUDES: RwLock<Vec<String>> = RwLock::new(Vec::new());
/// Hosts we upgraded to https this run: a load error there gets an https-toggle hint.
static UPGRADED: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// `url` failed to load: was it https only because we upgraded it?
pub fn was_upgraded(url: &str) -> bool {
    url.starts_with("https://")
        && authority(url).is_some_and(|(_, h)| UPGRADED.lock().unwrap().contains(h))
}

pub fn excludes_file() -> std::path::PathBuf {
    config::path().with_file_name("https_excludes")
}

/// (Re)read https_excludes; at start and after `https-toggle` / `config-source`.
pub fn load_https_excludes() {
    let lines = std::fs::read_to_string(excludes_file()).unwrap_or_default();
    *HTTPS_EXCLUDES.write().unwrap() = parse_excludes(&lines);
}

pub fn parse_excludes(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// Current excludes (for the WebKit web-process extension).
pub fn https_excludes() -> Vec<String> {
    HTTPS_EXCLUDES.read().unwrap().clone()
}

/// "host:port" (port only if explicit) and the bare host of an http(s) URL.
pub fn authority(url: &str) -> Option<(&str, &str)> {
    let rest = url.split_once("://")?.1;
    let auth = rest.split(['/', '?', '#']).next()?;
    let auth = auth.rsplit_once('@').map_or(auth, |(_, a)| a);
    let host = match auth.strip_prefix('[') {
        Some(v6) => v6.split(']').next()?,
        None => auth.split(':').next()?,
    };
    Some((auth, host))
}

/// Hosts that can't hold a public certificate (same rules as the old https_only.py).
pub fn is_local(host: &str) -> bool {
    if host.is_empty() || host == "localhost" || !host.contains('.') && !host.contains(':') {
        return true;
    }
    if [".local", ".lan", ".internal", ".home", ".arpa", ".ts.net"]
        .iter()
        .any(|s| host.ends_with(s))
    {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(a)) => {
            let o = a.octets();
            a.is_private()
                || a.is_loopback()
                || a.is_link_local()
                || (o[0] == 100 && (o[1] & 0xc0) == 64) // Tailscale 100.64/10
        }
        Ok(IpAddr::V6(a)) => {
            a.is_loopback()
                || (a.segments()[0] & 0xfe00) == 0xfc00
                || (a.segments()[0] & 0xffc0) == 0xfe80
        }
        Err(_) => false,
    }
}

/// HTTPS-only: the https:// URL to load instead, given these excludes.
pub fn https_upgrade_with(url: &str, excludes: &[String]) -> Option<String> {
    let rest = url.strip_prefix("http://")?;
    let (auth, host) = authority(url)?;
    if is_local(host) || excludes.iter().any(|e| e == host || e == auth) {
        return None;
    }
    Some(format!("https://{rest}"))
}

/// HTTPS-only: the https:// URL to load instead, if this request should be upgraded.
pub fn https_upgrade(url: &str) -> Option<String> {
    let up = https_upgrade_with(url, &HTTPS_EXCLUDES.read().unwrap())?;
    if let Some((_, host)) = authority(url) {
        UPGRADED.lock().unwrap().insert(host.to_string());
    }
    Some(up)
}

/// Registrable domain (eTLD+1), via the adblock crate's public suffix list.
pub fn site(url: &str) -> Option<String> {
    adblock::url_parser::parse_url(url).map(|u| u.domain().to_string())
}

pub fn origin(url: &str) -> Option<String> {
    let scheme = url.split_once("://")?.0;
    Some(format!("{scheme}://{}/", authority(url)?.0))
}

/// Referer for a request we make ourselves (aria2), like Chromium's default
/// `strict-origin-when-cross-origin`: full URL same-origin, origin cross-origin,
/// nothing on https → http or from non-http pages.
pub fn referrer(page: &str, target: &str) -> Option<String> {
    if !page.starts_with("http") || page.starts_with("https://") && target.starts_with("http://") {
        return None;
    }
    if origin(page)? == origin(target)? {
        page.split('#').next().map(str::to_string)
    } else {
        origin(page)
    }
}

pub fn global_blocks_third_party(cfg: &config::Config) -> bool {
    matches!(
        cfg.str("content.cookies.accept").as_str(),
        "no-3rdparty" | "no-unknown-3rdparty"
    )
}

pub fn page_allows_all(cfg: &config::Config, page: &str) -> bool {
    cfg.get_for("content.cookies.accept", page).as_str() == Some("all")
}

/// `content.cookies.accept`, looked up for the tab's page (`first_party`) like qutebrowser.
pub fn cookie_allowed(url: &str, first_party: &str) -> bool {
    let first_party = if first_party.is_empty() {
        url
    } else {
        first_party
    };
    match config::get()
        .get_for("content.cookies.accept", first_party)
        .as_str()
    {
        Some("never") => false,
        // shortcut: no-unknown-3rdparty treated as no-3rdparty (no "already has cookies" lookup).
        Some("no-3rdparty" | "no-unknown-3rdparty") => {
            matches!((site(url), site(first_party)), (Some(a), Some(b)) if a == b)
        }
        _ => true,
    }
}

/// `content.headers.custom` + `content.headers.do_not_track` for a request URL.
pub fn headers_for(url: &str) -> Vec<(String, String)> {
    let cfg = config::get();
    let mut out = Vec::new();
    if let Value::Object(h) = cfg.get_for("content.headers.custom", url) {
        for (k, v) in h {
            if let Some(v) = v.as_str() {
                out.push((k, v.to_string()));
            }
        }
    }
    match cfg.get_for("content.headers.do_not_track", url) {
        Value::Bool(true) => out.push(("DNT".into(), "1".into())),
        Value::Bool(false) => out.push(("DNT".into(), "0".into())),
        _ => {}
    }
    out
}

/// "en-US,en;q=0.9" -> "en-US,en": engines want bare languages and add q-values themselves.
pub fn language_list(header: &str) -> String {
    header
        .split(',')
        .map(|l| l.split(';').next().unwrap_or("").trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    #[test]
    fn site_is_registrable_domain() {
        assert_eq!(
            super::site("https://a.b.example.co.uk/x").as_deref(),
            Some("example.co.uk")
        );
        use super::referrer;
        let p = "https://mail.example/inbox?t=secret#x";
        assert_eq!(
            referrer(p, "https://mail.example/f.zip").as_deref(),
            Some("https://mail.example/inbox?t=secret")
        );
        assert_eq!(
            referrer(p, "https://cdn.other/f.zip").as_deref(),
            Some("https://mail.example/")
        );
        assert_eq!(referrer(p, "http://mail.example/f.zip"), None);
        assert_eq!(referrer("data:text/html,x", "https://a/"), None);
        assert_eq!(
            super::site("https://accounts.google.com/").as_deref(),
            Some("google.com")
        );
        assert_eq!(
            super::language_list("en-US,en;q=0.9, de ;q=0.5"),
            "en-US,en,de"
        );
    }

    #[test]
    fn https_upgrade_rules() {
        use super::https_upgrade as up;
        assert_eq!(
            up("http://example.com/a?b").as_deref(),
            Some("https://example.com/a?b")
        );
        assert_eq!(up("https://example.com/"), None);
        for local in [
            "http://localhost:3000/",
            "http://novahome:2283/",
            "http://192.168.1.1/",
            "http://10.0.0.5/",
            "http://100.88.215.101:8080/",
            "http://nas.local/",
            "http://x.tail9d0106.ts.net/",
            "http://[::1]:80/",
            "http://127.0.0.1/",
            "http://u:p@router/",
        ] {
            assert_eq!(up(local), None, "{local}");
        }
        assert!(up("http://100.128.0.1/").is_some(), "outside 100.64/10");
        *super::HTTPS_EXCLUDES.write().unwrap() =
            vec!["old.example".into(), "port.example:8080".into()];
        assert_eq!(up("http://old.example/x"), None);
        assert_eq!(up("http://port.example:8080/"), None);
        assert!(up("http://port.example/").is_some());
    }
}
