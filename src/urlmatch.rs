// SPDX-FileCopyrightText: Freya Bruhin (The Compiler) <mail@qutebrowser.org>
// SPDX-License-Identifier: GPL-3.0-or-later

//! A Chromium-like URL matching pattern.
//!
//! Port of qutebrowser's `qutebrowser/utils/urlmatch.py` (itself based on
//! Chromium's `extensions/common/url_pattern.cc`). Pattern semantics follow
//! qutebrowser, including its deviations from Chromium:
//! - a missing scheme means any scheme, and `*` matches every scheme (not only
//!   http/https);
//! - a missing path means any path;
//! - `<all_urls>` matches every URL.
//!
//! qutebrowser parses patterns with Python's `urllib.parse.urlparse` and URLs
//! with `QUrl`. Both are emulated here, only as far as matching needs:
//! [`urlsplit`] mirrors CPython 3.12+ `urlsplit` (including the bracketed-host
//! validation), [`parse_url`] mirrors what `QUrl` reports for scheme, host,
//! port and path of an already canonical browser URL. No IDNA.

use std::fmt;
use std::hash::{Hash, Hasher};

/// The default ports used for schemes which support ports.
const DEFAULT_PORTS: [(&str, u16); 3] = [("https", 443), ("http", 80), ("ftp", 21)];
/// Schemes which don't need a host.
const SCHEMES_WITHOUT_HOST: [&str; 4] = ["about", "file", "data", "javascript"];
/// Python's `urllib.parse.uses_params`: schemes whose last path segment loses `;params`.
const USES_PARAMS: [&str; 16] = [
    "", "ftp", "hdl", "prospero", "http", "imap", "https", "shttp", "rtsp", "rtsps", "rtspu",
    "sip", "sips", "mms", "sftp", "tel",
];

fn default_port(scheme: &str) -> Option<u16> {
    DEFAULT_PORTS
        .iter()
        .find(|(s, _)| *s == scheme)
        .map(|(_, p)| *p)
}

/// Raised when a pattern could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

fn parse_error(msg: impl Into<String>) -> ParseError {
    ParseError(msg.into())
}

/// A Chromium-like URL matching pattern.
///
/// Equality and hashing ignore the original pattern string (as in
/// qutebrowser): `http://example.com` equals `http://example.com/*`.
#[derive(Debug, Clone)]
pub struct UrlPattern {
    /// The given pattern as string.
    pattern: String,
    /// Whether the pattern should match all URLs.
    match_all: bool,
    /// Whether the pattern should match subdomains of the given host.
    match_subdomains: bool,
    /// The scheme to match to, or None to match any scheme.
    scheme: Option<String>,
    /// The host to match to, or None for any host.
    host: Option<String>,
    /// The path to match to (fnmatch glob), or None for any path.
    path: Option<String>,
    /// The port to match to, or None for any port.
    port: Option<u16>,
}

impl PartialEq for UrlPattern {
    fn eq(&self, other: &Self) -> bool {
        self.match_all == other.match_all
            && self.match_subdomains == other.match_subdomains
            && self.scheme == other.scheme
            && self.host == other.host
            && self.path == other.path
            && self.port == other.port
    }
}

impl Eq for UrlPattern {}

impl Hash for UrlPattern {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.match_all.hash(state);
        self.match_subdomains.hash(state);
        self.scheme.hash(state);
        self.host.hash(state);
        self.path.hash(state);
        self.port.hash(state);
    }
}

impl fmt::Display for UrlPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.pattern)
    }
}

impl UrlPattern {
    pub fn parse(pattern: &str) -> Result<UrlPattern, ParseError> {
        let mut up = UrlPattern {
            pattern: pattern.to_string(),
            match_all: false,
            match_subdomains: false,
            scheme: None,
            host: None,
            path: None,
            port: None,
        };

        // > The special pattern <all_urls> matches any URL that starts with a
        // > permitted scheme.
        if pattern == "<all_urls>" {
            up.match_all = true;
            return Ok(up);
        }

        if pattern.contains('\0') {
            return Err(parse_error("May not contain NUL byte"));
        }

        let fixed = fixup_pattern(pattern);
        let parsed = urlsplit(&fixed)?;

        up.init_scheme(&parsed)?;
        up.init_host(&parsed)?;
        up.init_path(&parsed)?;
        up.init_port(&parsed)?;
        Ok(up)
    }

    /// Parse the scheme. Deviation from Chromium: we assume * when no scheme
    /// has been given.
    fn init_scheme(&mut self, parsed: &SplitUrl) -> Result<(), ParseError> {
        if parsed.scheme.is_empty() {
            return Err(parse_error("Missing scheme"));
        }
        if parsed.scheme != "any" {
            self.scheme = Some(parsed.scheme.clone());
        }
        Ok(())
    }

    /// Parse the path. Deviation from Chromium: we assume * when no path has
    /// been given.
    fn init_path(&mut self, parsed: &SplitUrl) -> Result<(), ParseError> {
        if self.scheme.as_deref() == Some("about") && parsed.path.trim().is_empty() {
            return Err(parse_error("Pattern without path"));
        }
        // When the user doesn't add a trailing slash, we assume the pattern
        // matches any path.
        if parsed.path != "/*" && !parsed.path.is_empty() {
            self.path = Some(parsed.path.clone());
        }
        Ok(())
    }

    /// Parse the host. Deviations from Chromium:
    /// - `http://:1234/` is not a valid URL because it has no host.
    /// - We don't allow patterns for dot/space hosts which QUrl considers invalid.
    fn init_host(&mut self, parsed: &SplitUrl) -> Result<(), ParseError> {
        let hostname = match parsed.hostname() {
            Some(h) if !h.trim().is_empty() => h,
            _ => {
                let scheme_without_host = self
                    .scheme
                    .as_deref()
                    .is_some_and(|s| SCHEMES_WITHOUT_HOST.contains(&s));
                if !scheme_without_host {
                    return Err(parse_error("Pattern without host"));
                }
                return Ok(());
            }
        };

        if parsed.netloc.starts_with('[') {
            // qutebrowser goes through QUrl::setHost to minimize IPv6 addresses.
            self.host = Some(qt_bracketed_host(&hostname)?);
            return Ok(());
        }

        let host: Option<&str> = if hostname == "*" {
            self.match_subdomains = true;
            None
        } else if let Some(rest) = hostname.strip_prefix("*.") {
            if rest.is_empty() {
                // We don't allow just '*.' as a host.
                return Err(parse_error("Pattern without host"));
            }
            self.match_subdomains = true;
            Some(rest)
        } else if hostname.chars().all(|c| c == '.' || c == ' ') && hostname.contains('.') {
            return Err(parse_error("Invalid host"));
        } else {
            Some(&hostname)
        };

        if let Some(host) = host {
            if host.contains('*') {
                // Only * or *.foo is allowed as host.
                return Err(parse_error("Invalid host wildcard"));
            }
            self.host = Some(host.trim_end_matches('.').to_string());
        }
        Ok(())
    }

    /// Parse the port. Deviation from Chromium: None instead of "*" if there's
    /// no port filter.
    fn init_port(&mut self, parsed: &SplitUrl) -> Result<(), ParseError> {
        if parsed.netloc.ends_with(":*") {
            self.port = None;
        } else if parsed.netloc.ends_with(':') {
            return Err(parse_error("Invalid port: Port is empty"));
        } else {
            self.port = parsed
                .port()
                .map_err(|e| parse_error(format!("Invalid port: {e}")))?;
        }

        let scheme_has_port = match &self.scheme {
            None => true,
            Some(s) => default_port(s).is_some(),
        };
        if self.port.is_some() && !scheme_has_port {
            return Err(parse_error(format!(
                "Ports are unsupported with {} scheme",
                self.scheme.as_deref().unwrap_or("")
            )));
        }
        Ok(())
    }

    fn matches_scheme(&self, scheme: &str) -> bool {
        self.scheme.as_deref().is_none_or(|s| s == scheme)
    }

    fn matches_host(&self, host: &str) -> bool {
        let host = host.trim_end_matches('.');

        // No host in the pattern matches all hosts. Contrary to Chromium, we
        // don't check match_subdomains, so this is true for file:// as well.
        let Some(own) = self.host.as_deref() else {
            return true;
        };
        if host == own {
            return true;
        }
        if !self.match_subdomains {
            return false;
        }
        // We don't do subdomain matching against IP addresses.
        if is_ip_address(host) {
            return false;
        }
        // Check if the test host is a subdomain of our host.
        if host.len() <= own.len() + 1 {
            return false;
        }
        if !host.ends_with(own) {
            return false;
        }
        host.as_bytes()[host.len() - own.len() - 1] == b'.'
    }

    fn matches_port(&self, scheme: &str, port: Option<u16>) -> bool {
        let port = port.or_else(|| default_port(scheme));
        self.port.is_none() || self.port == port
    }

    /// Deviation from Chromium: Chromium only matches <all_urls> with
    /// "javascript:" (pathless); we also match *://*/* and friends.
    fn matches_path(&self, path: &str) -> bool {
        let Some(own) = self.path.as_deref() else {
            return true;
        };
        // Match 'google.com' with 'google.com/'
        if own.strip_suffix("/*") == Some(path) {
            return true;
        }
        fnmatch_case(path, own)
    }

    /// Check if the pattern matches the given URL (full URL string as the
    /// browser reports it). URLs which QUrl would reject (bad port, broken
    /// IPv6 host) never match.
    pub fn matches(&self, url: &str) -> bool {
        let Some(url) = parse_url(url) else {
            return false;
        };
        if self.match_all {
            return true;
        }
        self.matches_scheme(&url.scheme)
            && self.matches_host(&url.host)
            && self.matches_port(&url.scheme, url.port)
            && self.matches_path(&url.path)
    }
}

/// Make sure the given pattern is parseable by `urlsplit`.
fn fixup_pattern(pattern: &str) -> String {
    let mut pattern = match pattern.strip_prefix("*:") {
        // Any scheme, but *:// is unparsable
        Some(rest) => format!("any:{rest}"),
        None => pattern.to_string(),
    };

    let has_hostless_scheme = SCHEMES_WITHOUT_HOST
        .iter()
        .any(|s| pattern.strip_prefix(s).is_some_and(|r| r.starts_with(':')));
    if !pattern.contains("://") && !has_hostless_scheme {
        pattern = format!("any://{pattern}");
    }

    // Chromium handles file://foo like file:///foo
    // FIXME (from qutebrowser) This doesn't actually strip the hostname correctly.
    if let Some(rest) = pattern.strip_prefix("file://") {
        if !rest.starts_with('/') {
            pattern = format!("file:///{rest}");
        }
    }
    pattern
}

// ---------------------------------------------------------------------------
// urllib.parse.urlsplit emulation (CPython 3.12+)
// ---------------------------------------------------------------------------

/// The parts of `urllib.parse.urlparse()` the pattern parser uses.
#[derive(Debug)]
struct SplitUrl {
    scheme: String,
    netloc: String,
    path: String,
}

impl SplitUrl {
    /// `(hostname, port)` as in `NetlocResultMixinStr._hostinfo`.
    fn hostinfo(&self) -> (&str, Option<&str>) {
        let hostinfo = rpartition_last(&self.netloc, '@');
        let (hostname, port) = match hostinfo.split_once('[') {
            Some((_, bracketed)) => {
                let (hostname, rest) = bracketed.split_once(']').unwrap_or((bracketed, ""));
                (hostname, rest.split_once(':').map_or("", |(_, p)| p))
            }
            None => hostinfo.split_once(':').unwrap_or((hostinfo, "")),
        };
        (hostname, (!port.is_empty()).then_some(port))
    }

    /// `ParseResult.hostname`: lowercased, None when empty.
    fn hostname(&self) -> Option<String> {
        let (hostname, _) = self.hostinfo();
        if hostname.is_empty() {
            return None;
        }
        Some(match hostname.split_once('%') {
            Some((h, zone)) => format!("{}%{}", h.to_lowercase(), zone),
            None => hostname.to_lowercase(),
        })
    }

    /// `ParseResult.port`; the error text is Python's ValueError message.
    fn port(&self) -> Result<Option<u16>, String> {
        let Some(port) = self.hostinfo().1 else {
            return Ok(None);
        };
        if !port.bytes().all(|b| b.is_ascii_digit()) {
            return Err(format!(
                "Port could not be cast to integer value as {}",
                py_repr(port)
            ));
        }
        match port.parse::<u32>() {
            Ok(p) if p <= 65535 => Ok(Some(p as u16)),
            _ => Err("Port out of range 0-65535".to_string()),
        }
    }
}

/// Python's `s.rpartition(sep)[2]`.
fn rpartition_last(s: &str, sep: char) -> &str {
    s.rsplit_once(sep).map_or(s, |(_, after)| after)
}

/// Python's `repr()` for a str, good enough for error messages.
fn py_repr(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\");
    if s.contains('\'') && !s.contains('"') {
        format!("\"{escaped}\"")
    } else {
        format!("'{}'", escaped.replace('\'', "\\'"))
    }
}

/// `urllib.parse.urlparse()`, reduced to scheme, netloc and path.
fn urlsplit(url: &str) -> Result<SplitUrl, ParseError> {
    let url: String = url
        .trim_start_matches(|c: char| c <= ' ')
        .chars()
        .filter(|c| !matches!(c, '\t' | '\r' | '\n'))
        .collect();

    let mut scheme = String::new();
    let mut rest: &str = &url;
    if let Some(i) = url.find(':') {
        let candidate = &url[..i];
        let valid = candidate
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
            && candidate
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
        if valid {
            scheme = candidate.to_ascii_lowercase();
            rest = &url[i + 1..];
        }
    }

    let mut netloc = "";
    if let Some(after) = rest.strip_prefix("//") {
        let delim = after.find(['/', '?', '#']).unwrap_or(after.len());
        netloc = &after[..delim];
        rest = &after[delim..];
        let (open, close) = (netloc.contains('['), netloc.contains(']'));
        if open != close {
            return Err(parse_error("Invalid IPv6 URL"));
        }
        if open && close {
            check_bracketed_netloc(netloc)?;
        }
    }

    let rest = rest.split('#').next().unwrap_or("");
    let mut path = rest.split('?').next().unwrap_or("");

    // urlparse splits off ;params from the last path segment.
    if USES_PARAMS.contains(&scheme.as_str()) && path.contains(';') {
        let semicolon = match path.rfind('/') {
            Some(slash) => path[slash..].find(';').map(|i| slash + i),
            None => path.find(';'),
        };
        if let Some(i) = semicolon {
            path = &path[..i];
        }
    }

    Ok(SplitUrl {
        scheme,
        netloc: netloc.to_string(),
        path: path.to_string(),
    })
}

/// `urllib.parse._check_bracketed_netloc`.
fn check_bracketed_netloc(netloc: &str) -> Result<(), ParseError> {
    let hostname_and_port = rpartition_last(netloc, '@');
    if let Some((before, bracketed)) = hostname_and_port.split_once('[') {
        // No data is allowed before a bracket.
        if !before.is_empty() {
            return Err(parse_error("Invalid IPv6 URL"));
        }
        let (hostname, port) = bracketed.split_once(']').unwrap_or((bracketed, ""));
        // No data is allowed after the bracket but before the port delimiter.
        if !port.is_empty() && !port.starts_with(':') {
            return Err(parse_error("Invalid IPv6 URL"));
        }
        check_bracketed_host(hostname)?;
    }
    Ok(())
}

/// `urllib.parse._check_bracketed_host`.
fn check_bracketed_host(hostname: &str) -> Result<(), ParseError> {
    if let Some(future) = hostname.strip_prefix('v') {
        // \Av[a-fA-F0-9]+\..+\z
        let valid = future.split_once('.').is_some_and(|(hex, rest)| {
            !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit()) && !rest.is_empty()
        });
        if !valid {
            return Err(parse_error("IPvFuture address is invalid"));
        }
        return Ok(());
    }
    if parse_ipv4(hostname).is_some() {
        return Err(parse_error("An IPv4 address cannot be in brackets"));
    }
    if parse_ipv6(hostname).is_none() {
        return Err(parse_error(format!(
            "{} does not appear to be an IPv4 or IPv6 address",
            py_repr(hostname)
        )));
    }
    Ok(())
}

/// What `QUrl::setHost(hostname); QUrl::host()` gives for a bracketed pattern
/// host: the canonical IPv6 form.
fn qt_bracketed_host(hostname: &str) -> Result<String, ParseError> {
    match parse_ipv6(hostname) {
        Some(addr) => Ok(qt_ipv6_to_string(&addr)),
        // IPvFuture passed urlsplit's check; keep as is.
        None if hostname.starts_with('v') => Ok(hostname.to_string()),
        None => Err(parse_error(format!("Invalid IPv6 address: {hostname}"))),
    }
}

// ---------------------------------------------------------------------------
// IP addresses (Python ipaddress parsing, Qt IPv6 formatting)
// ---------------------------------------------------------------------------

/// `ipaddress.IPv4Address` parsing (strict dotted quad, no leading zeros).
fn parse_ipv4(s: &str) -> Option<[u8; 4]> {
    let mut out = [0u8; 4];
    let mut parts = s.split('.');
    for octet in out.iter_mut() {
        let part = parts.next()?;
        if part.is_empty() || part.len() > 3 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        if part != "0" && part.starts_with('0') {
            return None;
        }
        *octet = part.parse::<u8>().ok()?;
    }
    parts.next().is_none().then_some(out)
}

/// `ipaddress.IPv6Address` parsing, without scope IDs.
fn parse_ipv6(s: &str) -> Option<[u16; 8]> {
    let mut parts: Vec<String> = s.split(':').map(str::to_string).collect();
    // An IPv6 address needs at least 2 colons (3 parts).
    if parts.len() < 3 {
        return None;
    }
    // If the address has an IPv4-style suffix, convert it to hexadecimal.
    if parts.last()?.contains('.') {
        let v4 = parse_ipv4(&parts.pop()?)?;
        parts.push(format!("{:x}", u16::from_be_bytes([v4[0], v4[1]])));
        parts.push(format!("{:x}", u16::from_be_bytes([v4[2], v4[3]])));
    }
    // An IPv6 address can't have more than 8 colons (9 parts).
    if parts.len() > 9 {
        return None;
    }
    // Disregarding the endpoints, find '::' with nothing in between.
    let mut skip_index = None;
    for (i, part) in parts.iter().enumerate().take(parts.len() - 1).skip(1) {
        if part.is_empty() {
            if skip_index.is_some() {
                return None; // Can't have more than one '::'
            }
            skip_index = Some(i);
        }
    }

    let n = parts.len();
    let (parts_hi, parts_lo, parts_skipped) = match skip_index {
        Some(skip) => {
            let mut hi = skip;
            let mut lo = n - skip - 1;
            if parts[0].is_empty() {
                hi -= 1;
                if hi != 0 {
                    return None; // ^: requires ^::
                }
            }
            if parts[n - 1].is_empty() {
                lo -= 1;
                if lo != 0 {
                    return None; // :$ requires ::$
                }
            }
            if hi + lo >= 8 {
                return None;
            }
            (hi, lo, 8 - (hi + lo))
        }
        None => {
            if n != 8 || parts[0].is_empty() || parts[n - 1].is_empty() {
                return None;
            }
            (n, 0, 0)
        }
    };

    let hextet = |p: &str| -> Option<u16> {
        if p.is_empty() || p.len() > 4 || !p.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        u16::from_str_radix(p, 16).ok()
    };
    let mut out = Vec::with_capacity(8);
    for part in &parts[..parts_hi] {
        out.push(hextet(part)?);
    }
    out.extend(std::iter::repeat_n(0, parts_skipped));
    for part in &parts[n - parts_lo..] {
        out.push(hextet(part)?);
    }
    out.try_into().ok()
}

/// `ipaddress.ip_address(host)` succeeds.
fn is_ip_address(host: &str) -> bool {
    parse_ipv4(host).is_some() || parse_ipv6(host).is_some()
}

/// `QIPAddressUtils::toString` for IPv6: lowercase hex, longest run (>= 2
/// groups, first wins) of zero groups compressed, IPv4-mapped/compatible
/// addresses written with a dotted-quad tail.
fn qt_ipv6_to_string(addr: &[u16; 8]) -> String {
    let embedded_ipv4 = if addr[..5].iter().all(|&h| h == 0) {
        match addr[5] {
            0xffff => true,
            0 if addr[6] != 0 || (addr[7] >> 8) != 0 => true,
            0 if addr[7] == 0 => return "::".to_string(),
            _ => false,
        }
    } else {
        false
    };

    // Longest run of zero groups; ties go to the first one.
    let (mut run_start, mut run_len) = (0, 0);
    let mut i = 0;
    while i < 8 {
        if addr[i] == 0 {
            let start = i;
            while i < 8 && addr[i] == 0 {
                i += 1;
            }
            if i - start > run_len {
                run_start = start;
                run_len = i - start;
            }
        } else {
            i += 1;
        }
    }
    let compress = run_len >= 2;

    let mut out = String::new();
    if compress && run_start == 0 {
        out.push(':');
    }
    let mut i = 0;
    while i < 8 {
        if compress && i == run_start {
            out.push(':');
            i += run_len;
            continue;
        }
        if i == 6 && embedded_ipv4 {
            let [a, b] = addr[6].to_be_bytes();
            let [c, d] = addr[7].to_be_bytes();
            out.push_str(&format!("{a}.{b}.{c}.{d}"));
            return out;
        }
        out.push_str(&format!("{:x}", addr[i]));
        if i != 7 {
            out.push(':');
        }
        i += 1;
    }
    out
}

// ---------------------------------------------------------------------------
// QUrl emulation for the URL being matched
// ---------------------------------------------------------------------------

/// The parts of a URL that `UrlPattern::matches` looks at, as QUrl reports them.
#[derive(Debug, PartialEq, Eq)]
struct MatchUrl {
    /// Lowercased scheme, empty for relative URLs.
    scheme: String,
    /// Lowercased host, IPv6 without brackets and canonicalized; empty if none.
    host: String,
    /// Explicit port, None when absent (QUrl's -1).
    port: Option<u16>,
    /// Percent-decoded path (QUrl::FullyDecoded).
    path: String,
}

/// Split a URL string like QUrl does. None for URLs QUrl considers invalid
/// (bad port, broken IPv6 host).
fn parse_url(url: &str) -> Option<MatchUrl> {
    let mut scheme = String::new();
    let mut rest = url;
    if let Some(i) = url.find(':') {
        let candidate = &url[..i];
        let valid = candidate
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
            && candidate
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
        if valid {
            scheme = candidate.to_ascii_lowercase();
            rest = &url[i + 1..];
        }
    }

    let mut host = String::new();
    let mut port = None;
    if let Some(after) = rest.strip_prefix("//") {
        let delim = after.find(['/', '?', '#']).unwrap_or(after.len());
        let authority = &after[..delim];
        rest = &after[delim..];
        let hostport = rpartition_last(authority, '@');
        let port_str;
        if let Some(bracketed) = hostport.strip_prefix('[') {
            let (inner, tail) = bracketed.split_once(']')?;
            host = qt_ipv6_to_string(&parse_ipv6(inner)?);
            port_str = match tail {
                "" => "",
                _ => tail.strip_prefix(':')?,
            };
        } else {
            let (h, p) = hostport.split_once(':').unwrap_or((hostport, ""));
            if p.contains(':') {
                return None;
            }
            host = h.to_lowercase();
            port_str = p;
        }
        if !port_str.is_empty() {
            if !port_str.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            port = Some(port_str.parse::<u16>().ok()?);
        }
    }

    let end = rest.find(['?', '#']).unwrap_or(rest.len());
    Some(MatchUrl {
        scheme,
        host,
        port,
        path: percent_decode(&rest[..end]),
    })
}

/// Decode %XX escapes; keep the input unchanged if the result isn't UTF-8.
fn percent_decode(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && bytes[i + 1].is_ascii_hexdigit()
            && bytes[i + 2].is_ascii_hexdigit()
        {
            // Both are ASCII hex digits, so this is a valid 2-byte str slice.
            out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap_or(0));
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

// ---------------------------------------------------------------------------
// fnmatch.fnmatchcase
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
enum GlobToken {
    /// `*`: any run of characters, including `/`.
    Star,
    /// `?`: any single character.
    AnyChar,
    Literal(char),
    /// `[...]` / `[!...]`: inclusive char ranges (single chars are `(c, c)`).
    Class {
        negated: bool,
        ranges: Vec<(char, char)>,
    },
    /// An empty class like `[z-a]`: never matches.
    Never,
}

/// `fnmatch.translate`, producing tokens instead of a regex.
fn compile_glob(pattern: &str) -> Vec<GlobToken> {
    let p: Vec<char> = pattern.chars().collect();
    let n = p.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        let c = p[i];
        i += 1;
        match c {
            '*' => {
                if out.last() != Some(&GlobToken::Star) {
                    out.push(GlobToken::Star);
                }
            }
            '?' => out.push(GlobToken::AnyChar),
            '[' => {
                let mut j = i;
                if j < n && p[j] == '!' {
                    j += 1;
                }
                if j < n && p[j] == ']' {
                    j += 1;
                }
                while j < n && p[j] != ']' {
                    j += 1;
                }
                if j >= n {
                    out.push(GlobToken::Literal('['));
                    continue;
                }
                let mut body = &p[i..j];
                i = j + 1;
                let negated = body.first() == Some(&'!');
                if negated {
                    body = &body[1..];
                }
                let ranges = class_ranges(body);
                out.push(match (ranges.is_empty(), negated) {
                    (true, false) => GlobToken::Never,
                    (true, true) => GlobToken::AnyChar,
                    (false, _) => GlobToken::Class { negated, ranges },
                });
            }
            c => out.push(GlobToken::Literal(c)),
        }
    }
    out
}

/// Ranges in a class body; `a-z` is a range, a leading/trailing `-` is
/// literal, reversed ranges are dropped (as fnmatch does).
fn class_ranges(body: &[char]) -> Vec<(char, char)> {
    let mut ranges = Vec::new();
    let mut k = 0;
    while k < body.len() {
        if k + 2 < body.len() && body[k + 1] == '-' {
            if body[k] <= body[k + 2] {
                ranges.push((body[k], body[k + 2]));
            }
            k += 3;
        } else {
            ranges.push((body[k], body[k]));
            k += 1;
        }
    }
    ranges
}

fn glob_token_matches(token: &GlobToken, c: char) -> bool {
    match token {
        GlobToken::Star | GlobToken::AnyChar => true,
        GlobToken::Literal(l) => *l == c,
        GlobToken::Class { negated, ranges } => {
            ranges.iter().any(|&(lo, hi)| lo <= c && c <= hi) != *negated
        }
        GlobToken::Never => false,
    }
}

/// `fnmatch.fnmatchcase(name, pattern)`: whole-string, case-sensitive glob.
fn fnmatch_case(name: &str, pattern: &str) -> bool {
    let tokens = compile_glob(pattern);
    let text: Vec<char> = name.chars().collect();
    let (mut t, mut p) = (0, 0);
    // Last star seen: (token index after it, text index it's tried up to).
    let mut backtrack: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < tokens.len() && tokens[p] == GlobToken::Star {
            backtrack = Some((p + 1, t));
            p += 1;
        } else if p < tokens.len() && glob_token_matches(&tokens[p], text[t]) {
            p += 1;
            t += 1;
        } else if let Some((star_p, star_t)) = backtrack {
            p = star_p;
            t = star_t + 1;
            backtrack = Some((star_p, star_t + 1));
        } else {
            return false;
        }
    }
    tokens[p..].iter().all(|tok| *tok == GlobToken::Star)
}

// ---------------------------------------------------------------------------
// Tests: port of qutebrowser's tests/unit/utils/test_urlmatch.py
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::hash_map::DefaultHasher;

    fn up(pattern: &str) -> UrlPattern {
        UrlPattern::parse(pattern).unwrap_or_else(|e| panic!("{pattern:?}: {e}"))
    }

    fn hash_of(p: &UrlPattern) -> u64 {
        let mut h = DefaultHasher::new();
        p.hash(&mut h);
        h.finish()
    }

    /// Compare (scheme, host, match_subdomains, match_all, path) in one go.
    fn assert_attrs(
        p: &UrlPattern,
        scheme: Option<&str>,
        host: Option<&str>,
        match_subdomains: bool,
        match_all: bool,
        path: Option<&str>,
    ) {
        assert_eq!(p.scheme.as_deref(), scheme, "scheme of {p}");
        assert_eq!(p.host.as_deref(), host, "host of {p}");
        assert_eq!(
            p.match_subdomains, match_subdomains,
            "match_subdomains of {p}"
        );
        assert_eq!(p.match_all, match_all, "match_all of {p}");
        assert_eq!(p.path.as_deref(), path, "path of {p}");
    }

    fn assert_matches(p: &UrlPattern, cases: &[(&str, bool)]) {
        for &(url, expected) in cases {
            assert_eq!(p.matches(url), expected, "{p} vs {url}");
        }
    }

    const INVALID_IP: &str = "does not appear to be an IPv4 or IPv6 address";

    #[test]
    fn invalid_patterns() {
        // Error texts follow CPython 3.12+ urllib.parse (the test accepts the
        // older/QUrl variants too via regex alternatives).
        let cases: &[(&str, &str)] = &[
            // Chromium: kMissingSchemeSeparator
            ("http:", "Invalid port: Port is empty"),
            ("http:/", "Invalid port: Port is empty"),
            ("about://", "Pattern without path"),
            ("http:/bar", "Invalid port: Port is empty"),
            // Chromium: kEmptyHost
            ("http://", "Pattern without host"),
            ("http:///", "Pattern without host"),
            ("http://:1234/", "Pattern without host"),
            ("http://*./", "Pattern without host"),
            ("http://[]:8888/*", &format!("'' {INVALID_IP}")),
            // Chromium: kInvalidHost
            ("http://\0www/", "May not contain NUL byte"),
            ("http://[2607:f8b0:4005:805::200e/*", "Invalid IPv6 URL"),
            ("http://[2607:f8b0:4005:805::200e]]/*", "Invalid IPv6 URL"),
            (
                "http://[[2607:f8b0:4005:805::200e]/*",
                &format!("'[2607:f8b0:4005:805::200e' {INVALID_IP}"),
            ),
            (
                "http://[2607:f8b0:4005:805:200e]/*",
                &format!("'2607:f8b0:4005:805:200e' {INVALID_IP}"),
            ),
            (
                "http://[2607:f8b0:4005:805:200e:12:bogus]/*",
                &format!("'2607:f8b0:4005:805:200e:12:bogus' {INVALID_IP}"),
            ),
            // Chromium: kInvalidHostWildcard
            ("http://*foo/bar", "Invalid host wildcard"),
            ("http://foo.*.bar/baz", "Invalid host wildcard"),
            ("http://fo.*.ba:123/baz", "Invalid host wildcard"),
            ("http://foo.*/bar", "Invalid host wildcard"),
            // Chromium: kInvalidPort
            ("http://foo:/", "Invalid port: Port is empty"),
            ("http://*.foo:/", "Invalid port: Port is empty"),
            (
                "http://foo:com/",
                "Invalid port: Port could not be cast to integer value as 'com'",
            ),
            (
                "http://foo:123456/",
                "Invalid port: Port out of range 0-65535",
            ),
            (
                "http://foo:80:80/monkey",
                "Invalid port: Port could not be cast to integer value as '80:80'",
            ),
            (
                "chrome://foo:1234/bar",
                "Ports are unsupported with chrome scheme",
            ),
            (
                "http://[2607:f8b0:4005:805::200e]:/*",
                "Invalid port: Port is empty",
            ),
            // Additional tests
            ("http://[", "Invalid IPv6 URL"),
            (
                "http://[fc2e::bb88::edac]",
                &format!("'fc2e::bb88::edac' {INVALID_IP}"),
            ),
            (
                "http://[fc2e:0e35:bb88::edac:fc2e:0e35:bb88:edac]",
                &format!("'fc2e:0e35:bb88::edac:fc2e:0e35:bb88:edac' {INVALID_IP}"),
            ),
            (
                "http://[fc2e:0e35:bb88:af:edac:fc2e:0e35:bb88:edac]",
                &format!("'fc2e:0e35:bb88:af:edac:fc2e:0e35:bb88:edac' {INVALID_IP}"),
            ),
            (
                "http://[127.0.0.1:fc2e::bb88:edac]",
                &format!("'127.0.0.1:fc2e::bb88:edac' {INVALID_IP}"),
            ),
            ("http://[fc2e::bb88", "Invalid IPv6 URL"),
            (
                "http://[fc2e:bb88:edac]",
                &format!("'fc2e:bb88:edac' {INVALID_IP}"),
            ),
            (
                "http://[fc2e:bb88:edac::z]",
                &format!("'fc2e:bb88:edac::z' {INVALID_IP}"),
            ),
            (
                "http://[fc2e:bb88:edac::2]:2a2",
                "Invalid port: Port could not be cast to integer value as '2a2'",
            ),
            ("://", "Missing scheme"),
        ];
        for (pattern, error) in cases {
            assert_eq!(
                UrlPattern::parse(pattern),
                Err(ParseError(error.to_string())),
                "pattern {pattern:?}"
            );
        }
    }

    #[test]
    fn whitespace_hosts() {
        // Deviation from Chromium. (The QUrl-invalidity half of the Python
        // test is about QUrl itself and not ported.)
        for host in [".", " ", " .", ". ", ". .", ". . .", " . "] {
            let pattern = format!("https://{host}/*");
            let err = UrlPattern::parse(&pattern).unwrap_err();
            assert!(
                err.0 == "Invalid host" || err.0 == "Pattern without host",
                "{pattern:?}: {err}"
            );
        }
    }

    #[test]
    fn port() {
        let cases: &[(&str, Option<u16>)] = &[
            ("http://foo:1234/", Some(1234)),
            ("http://foo:1234/bar", Some(1234)),
            ("http://*.foo:1234/", Some(1234)),
            ("http://*.foo:1234/bar", Some(1234)),
            ("http://*:1234/", Some(1234)),
            ("http://*:*/", None),
            ("http://foo:*/", None),
            ("file://foo:1234/bar", None),
            // Port-like strings in the path should not trigger a warning.
            ("http://*/:1234", None),
            ("http://*.foo/bar:1234", None),
            ("http://foo/bar:1234/path", None),
        ];
        for &(pattern, port) in cases {
            assert_eq!(up(pattern).port, port, "{pattern}");
        }
    }

    #[test]
    fn parse_path() {
        assert_eq!(up("http://foo/").path.as_deref(), Some("/"));
        assert_eq!(up("http://foo/*").path, None);
    }

    #[test]
    fn lightweight_patterns() {
        // Deviation from Chromium to make patterns more user-friendly.
        // (pattern, scheme, host, path)
        type Row<'a> = (&'a str, Option<&'a str>, Option<&'a str>, Option<&'a str>);
        let cases: &[Row] = &[
            (
                "http://example.com",
                Some("http"),
                Some("example.com"),
                None,
            ), // no path
            ("example.com/path", None, Some("example.com"), Some("/path")), // no scheme
            ("example.com", None, Some("example.com"), None),               // no scheme and no path
            ("example.com:1234", None, Some("example.com"), None), // no scheme/path but port
            ("data:monkey", Some("data"), None, Some("monkey")),   // existing scheme
        ];
        for &(pattern, scheme, host, path) in cases {
            let p = up(pattern);
            assert_eq!(p.scheme.as_deref(), scheme, "{pattern}");
            assert_eq!(p.host.as_deref(), host, "{pattern}");
            assert_eq!(p.path.as_deref(), path, "{pattern}");
        }
    }

    /// TEST(ExtensionURLPatternTest, Match1)
    #[test]
    fn match_all_pages_for_given_scheme() {
        let p = up("http://*/*");
        assert_attrs(&p, Some("http"), None, true, false, None);
        assert_matches(
            &p,
            &[
                ("http://google.com", true),
                ("http://yahoo.com", true),
                ("http://google.com/foo", true),
                ("https://google.com", false),
                ("http://74.125.127.100/search", true),
                // Additional tests
                ("http://google.com:80", true),
                ("http://google.com.", true),
                ("http://[fc2e:0e35:bb88::edac]", true),
                ("http://[fc2e:e35:bb88::edac]", true),
                ("http://[fc2e:e35:bb88::127.0.0.1]", true),
                ("http://[::1]/bar", true),
            ],
        );
    }

    /// TEST(ExtensionURLPatternTest, Match2)
    #[test]
    fn match_all_domains() {
        let p = up("https://*/foo*");
        assert_attrs(&p, Some("https"), None, true, false, Some("/foo*"));
        assert_matches(
            &p,
            &[
                ("https://google.com/foo", true),
                ("https://google.com/foobar", true),
                ("http://google.com/foo", false),
                ("https://google.com/", false),
            ],
        );
    }

    /// TEST(ExtensionURLPatternTest, Match3)
    #[test]
    fn match_subdomains() {
        let p = up("http://*.google.com/foo*bar");
        assert_attrs(
            &p,
            Some("http"),
            Some("google.com"),
            true,
            false,
            Some("/foo*bar"),
        );
        assert_matches(
            &p,
            &[
                ("http://google.com/foobar", true),
                // skipped: "http://www.google.com/foo?bar" - commented out
                // upstream too (?bar is a query for QUrl, path for GURL).
                ("http://monkey.images.google.com/foooobar", true),
                ("http://yahoo.com/foobar", false),
            ],
        );
    }

    /// TEST(ExtensionURLPatternTest, Match5)
    #[test]
    fn match_glob_escaping() {
        let p = up(r"file:///foo-bar\*baz");
        assert_attrs(&p, Some("file"), None, false, false, Some(r"/foo-bar\*baz"));
        assert_matches(
            &p,
            &[
                (r"file:///foo-bar\hellobaz", true),
                (r"file:///fooXbar\hellobaz", false),
            ],
        );
    }

    /// TEST(ExtensionURLPatternTest, Match6/7)
    #[test]
    fn match_ip_addresses_attrs() {
        let cases: &[(&str, &str, bool)] = &[
            ("http://127.0.0.1/*", "127.0.0.1", false),
            ("http://*.0.0.1/*", "0.0.1", true),
            ("http://[::1]/*", "::1", false),
            ("http://[0::1]/*", "::1", false),
            ("http://[::01]/*", "::1", false),
            ("http://[0:0:0:0:20::1]/*", "::20:0:0:1", false),
        ];
        for &(pattern, host, match_subdomains) in cases {
            assert_attrs(
                &up(pattern),
                Some("http"),
                Some(host),
                match_subdomains,
                false,
                None,
            );
        }
    }

    #[test]
    fn match_ip_addresses_urls() {
        assert!(up("http://127.0.0.1/*").matches("http://127.0.0.1"));
        // No subdomain matching is done with IPs
        assert!(!up("http://*.0.0.1/*").matches("http://127.0.0.1"));
    }

    /// TEST(ExtensionURLPatternTest, Match9/10)
    #[test]
    fn match_chrome_urls() {
        let p = up("chrome://favicon/*");
        assert_attrs(&p, Some("chrome"), Some("favicon"), false, false, None);
        assert_matches(
            &p,
            &[
                ("chrome://favicon/http://google.com", true),
                ("chrome://favicon/https://google.com", true),
                ("chrome://history", false),
            ],
        );
    }

    /// TEST(ExtensionURLPatternTest, Match10/11)
    #[test]
    fn match_anything() {
        let urls = [
            "http://127.0.0.1",
            // We deviate from Chromium as we allow other schemes as well
            "chrome://favicon/http://google.com",
            "file:///foo/bar",
            "file://localhost/foo/bar",
            "qute://version",
            "about:blank",
            "data:text/html;charset=utf-8,<html>asdf</html>",
            "javascript:",
        ];
        for pattern in ["*://*/*", "*://*:*/*", "<all_urls>", "*://*"] {
            let p = up(pattern);
            assert_eq!(p.scheme, None, "{pattern}");
            assert_eq!(p.host, None, "{pattern}");
            assert_eq!(p.path, None, "{pattern}");
            for url in urls {
                assert!(p.matches(url), "{pattern} vs {url}");
            }
        }
        let wildcard = up("*://*/*");
        assert!(wildcard.match_subdomains && !wildcard.match_all);
        let all = up("<all_urls>");
        assert!(!all.match_subdomains && all.match_all);
    }

    /// TEST(ExtensionURLPatternTest, Match13)
    #[test]
    fn special_schemes() {
        let cases = [
            ("about:*", "about:blank", true),
            ("about:blank", "about:blank", true),
            ("about:*", "about:version", true),
            ("data:*", "data:monkey", true),
            ("javascript:*", "javascript:atemyhomework", true),
            ("data:*", "about:blank", false),
        ];
        for (pattern, url, expected) in cases {
            assert_eq!(up(pattern).matches(url), expected, "{pattern} vs {url}");
        }
    }

    /// TEST(ExtensionURLPatternTest, Match14/15/16)
    #[test]
    fn file_scheme() {
        // skipped: 'file://localhost/foo*' - skipped upstream too ("We're not
        // handling this correctly in all cases").
        for pattern in ["file:///foo*", "file://foo*"] {
            let p = up(pattern);
            assert_attrs(&p, Some("file"), None, false, false, Some("/foo*"));
            assert_matches(
                &p,
                &[
                    ("file://foo", false),
                    ("file://foobar", false),
                    ("file:///foo", true),
                    ("file:///foobar", true),
                    ("file://localhost/foo", true),
                ],
            );
        }
    }

    /// TEST(ExtensionURLPatternTest, Match17)
    #[test]
    fn match_specific_port() {
        let p = up("http://www.example.com:80/foo");
        assert_attrs(
            &p,
            Some("http"),
            Some("www.example.com"),
            false,
            false,
            Some("/foo"),
        );
        assert_eq!(p.port, Some(80));
        assert_matches(
            &p,
            &[
                ("http://www.example.com:80/foo", true),
                ("http://www.example.com/foo", true),
                ("http://www.example.com:8080/foo", false),
            ],
        );
    }

    /// TEST(ExtensionURLPatternTest, Match18)
    #[test]
    fn explicit_port_wildcard() {
        let p = up("http://www.example.com:*/foo");
        assert_attrs(
            &p,
            Some("http"),
            Some("www.example.com"),
            false,
            false,
            Some("/foo"),
        );
        assert_eq!(p.port, None);
        assert_matches(
            &p,
            &[
                ("http://www.example.com:80/foo", true),
                ("http://www.example.com/foo", true),
                ("http://www.example.com:8080/foo", true),
            ],
        );
    }

    /// TEST(ExtensionURLPatternTest, IgnoreMissingBackslashes)
    #[test]
    fn ignore_missing_slashes() {
        let pattern1 = up("http://www.example.com/example");
        let pattern2 = up("http://www.example.com/example/*");
        let url1 = "http://www.example.com/example";
        let url2 = "http://www.example.com/example/";
        assert!(pattern1.matches(url1));
        // The not terminated path should match the terminated pattern.
        assert!(pattern2.matches(url1));
        // The terminated path however should not match the unterminated pattern.
        assert!(!pattern1.matches(url2));
    }

    #[test]
    fn trailing_slash() {
        // Contrary to Chromium, we allow to leave off a trailing slash.
        assert!(up("http://www.example.com").matches("http://www.example.com/"));
    }

    #[test]
    fn trailing_dot_domain() {
        for pattern in ["*://example.com/*", "*://example.com./*"] {
            for url in ["http://example.com/", "http://example.com./"] {
                assert!(up(pattern).matches(url), "{pattern} vs {url}");
            }
        }
    }

    /// TEST(ExtensionURLPatternTest, UncanonicalizedUrl)
    #[test]
    fn uncanonicalized_url_lowercase() {
        let p = up("*://*.gOoGle.com/*");
        assert!(p.matches("https://google.com"));
        assert!(p.matches("https://maps.google.com"));
    }

    // skipped: TestUncanonicalizedUrl.test_punycode ('https://ɡoogle.com',
    //   'https://xn--oogle-qmc.com/') - needs IDNA, out of scope.
    // skipped: TestUncanonicalizedUrl.test_failing_canonicalization - xfail
    //   upstream (needs IDNA / NFKC checks).
    // skipped: test_urlpattern_benchmark - pytest-benchmark only.
    // skipped: test_equal_string - comparing a pattern to a str can't be
    //   expressed in Rust's type system.
    // skipped: test_repr - Python repr format.

    /// TestUncanonicalizedUrl.test_str is xfail upstream: str() and host keep
    /// the original values. Ported for the gOoGle row as the actual behaviour
    /// (the ɡoogle row needs IDNA).
    #[test]
    fn uncanonicalized_str_keeps_original() {
        let p = up("*://*.gOoGle.com/*");
        assert_eq!(p.to_string(), "*://*.gOoGle.com/*");
        // urllib lowercases the hostname, so .host is canonical here.
        assert_eq!(p.host.as_deref(), Some("google.com"));
    }

    /// test_urlpattern_hypothesis, as a fixed grid over the same alphabet:
    /// parsing must not panic, and matching must not panic.
    #[test]
    fn grid_no_panics() {
        let schemes = ["*", "http", "file"];
        let separators = [":", "://"];
        let hosts = ["*", "*.", "*.abc", "*.AbC", "", "abc", "XyZ"];
        let ports = ["", ":0", ":80", ":65535"];
        let paths = ["", "/", "/abc", "/AbC"];
        for scheme in schemes {
            for sep in separators {
                for host in hosts {
                    for port in ports {
                        for path in paths {
                            let pattern = format!("{scheme}{sep}{host}{port}{path}");
                            if let Ok(p) = UrlPattern::parse(&pattern) {
                                p.matches("https://www.example.com/");
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn equal() {
        let cases = [
            // schemes
            (
                "http://en.google.com/blah/*/foo",
                "https://en.google.com/blah/*/foo",
                false,
            ),
            (
                "https://en.google.com/blah/*/foo",
                "https://en.google.com/blah/*/foo",
                true,
            ),
            (
                "https://en.google.com/blah/*/foo",
                "ftp://en.google.com/blah/*/foo",
                false,
            ),
            // subdomains
            (
                "https://en.google.com/blah/*/foo",
                "https://fr.google.com/blah/*/foo",
                false,
            ),
            (
                "https://www.google.com/blah/*/foo",
                "https://*.google.com/blah/*/foo",
                false,
            ),
            (
                "https://*.google.com/blah/*/foo",
                "https://*.google.com/blah/*/foo",
                true,
            ),
            // domains
            (
                "http://en.example.com/blah/*/foo",
                "http://en.google.com/blah/*/foo",
                false,
            ),
            // ports
            (
                "http://en.google.com:8000/blah/*/foo",
                "http://en.google.com/blah/*/foo",
                false,
            ),
            (
                "http://fr.google.com:8000/blah/*/foo",
                "http://fr.google.com:8000/blah/*/foo",
                true,
            ),
            (
                "http://en.google.com:8000/blah/*/foo",
                "http://en.google.com:8080/blah/*/foo",
                false,
            ),
            // paths
            (
                "http://en.google.com/blah/*/foo",
                "http://en.google.com/blah/*",
                false,
            ),
            ("http://en.google.com/*", "http://en.google.com/", false),
            ("http://en.google.com/*", "http://en.google.com/*", true),
            // all_urls
            ("<all_urls>", "<all_urls>", true),
            ("<all_urls>", "http://*/*", false),
            // Not upstream: equality ignores the pattern string.
            ("http://example.com", "http://example.com/*", true),
        ];
        for (text1, text2, equal) in cases {
            let (p1, p2) = (up(text1), up(text2));
            assert_eq!(p1 == p2, equal, "{text1} == {text2}");
            assert_eq!(
                hash_of(&p1) == hash_of(&p2),
                equal,
                "hash {text1} / {text2}"
            );
        }
    }

    #[test]
    fn str() {
        let text = "https://www.example.com/";
        assert_eq!(up(text).to_string(), text);
        assert_eq!(up("<all_urls>").to_string(), "<all_urls>");
    }

    // --- Not upstream: helpers that replace Python/Qt library code. ---

    #[test]
    fn fnmatch_semantics() {
        let cases = [
            ("/foo", "/foo", true),
            ("/foo/bar", "/foo*", true),
            ("/foo", "/fo?", true),
            ("/fo", "/fo?", false),
            ("/a", "/[abc]", true),
            ("/d", "/[abc]", false),
            ("/d", "/[!abc]", true),
            ("/m", "/[a-z]", true),
            ("/-", "/[a-]", true),
            ("/x", "/[z-a]", false),
            ("/x", "/[!z-a]", true),
            ("/[", "/[", true),
            ("/]", "/[]]", true),
            ("/FOO", "/foo", false),
            ("/a/b/c", "/*/c", true),
            ("/abcabd", "/*abd", true),
            ("", "*", true),
            ("", "?", false),
        ];
        for (name, pattern, expected) in cases {
            assert_eq!(
                fnmatch_case(name, pattern),
                expected,
                "{name:?} ~ {pattern:?}"
            );
        }
    }

    #[test]
    fn qt_ipv6_formatting() {
        let cases = [
            ("::", "::"),
            ("::1", "::1"),
            ("1::", "1::"),
            ("1:0:0:2::3", "1:0:0:2::3"),        // longest run wins
            ("1:0:0:2:0:0:3:4", "1::2:0:0:3:4"), // tie: first run wins
            ("1:0:2:3:4:5:6:7", "1:0:2:3:4:5:6:7"),
            ("FC2E:0E35:BB88::EDAC", "fc2e:e35:bb88::edac"),
            ("::ffff:1.2.3.4", "::ffff:1.2.3.4"),
            ("::1.2.3.4", "::1.2.3.4"),
        ];
        for (input, expected) in cases {
            let addr = parse_ipv6(input).unwrap_or_else(|| panic!("{input}"));
            assert_eq!(qt_ipv6_to_string(&addr), expected, "{input}");
        }
    }

    #[test]
    fn parse_url_like_qurl() {
        let u = parse_url("HTTPS://User@WWW.Example.com:8080/a%20b/c?q#f").unwrap();
        assert_eq!(
            u,
            MatchUrl {
                scheme: "https".into(),
                host: "www.example.com".into(),
                port: Some(8080),
                path: "/a b/c".into(),
            }
        );
        assert_eq!(parse_url("about:blank").unwrap().path, "blank");
        assert_eq!(parse_url("http://[::1]:99/").unwrap().host, "::1");
        assert!(parse_url("http://foo:99999/").is_none());
        assert!(parse_url("http://[::1/").is_none());
        // Invalid URLs never match, even <all_urls>.
        assert!(!up("<all_urls>").matches("http://foo:bar/"));
        // Percent-encoded path matches a decoded pattern path.
        assert!(up("*://example.com/a b").matches("https://example.com/a%20b"));
    }
}
