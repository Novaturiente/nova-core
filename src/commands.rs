//! Command-line parsing shared by both editions (qutebrowser names): tokenizing, `;;`
//! chains, aliases, flags, URL-or-search, zoom steps, and the command name list.
//! Running commands is engine work and stays in each edition.
use crate::config;

/// One parsed command: name, `-x`/`--long` flags, positional args.
#[derive(Debug, PartialEq)]
pub struct Cmd {
    pub name: String,
    pub flags: Vec<String>,
    pub args: Vec<String>,
}

impl Cmd {
    pub fn flag(&self, short: &str, long: &str) -> bool {
        self.flags.iter().any(|f| f == short || f == long)
    }
    pub fn rest(&self) -> String {
        self.args.join(" ")
    }
}

/// 0-based tab from a count or a 1-based first argument (`tab-pin 3`), else None = active.
pub fn tab_index(cmd: &Cmd, count: Option<u32>) -> Option<usize> {
    count
        .or_else(|| cmd.args.first()?.parse().ok())
        .map(|c| (c as usize).saturating_sub(1))
}

/// Shell-like split: whitespace separates, '…' and "…" group, `\` escapes outside
/// '…' (qutebrowser; `fake-key \<` sends a literal `<`).
pub fn tokenize(s: &str) -> Result<Vec<String>, String> {
    let (mut out, mut cur, mut quote, mut has) = (vec![], String::new(), None, false);
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (None, c) if c.is_whitespace() => {
                if has {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            (None, '\'' | '"') => {
                quote = Some(c);
                has = true;
            }
            (Some(q), c) if c == q => quote = None,
            (None | Some('"'), '\\') => {
                cur.extend(chars.next());
                has = true;
            }
            (_, c) => {
                cur.push(c);
                has = true;
            }
        }
    }
    if quote.is_some() {
        return Err(format!("unterminated quote in: {s}"));
    }
    if has {
        out.push(cur);
    }
    Ok(out)
}

/// `a ;; b` → ["a", "b"]; aliases expand on the first word, and an alias may
/// itself contain `;;` (qutebrowser expands before splitting). `expand` fills
/// placeholders ({url}, {title}, {clipboard}…) per token; the edition supplies it.
pub fn parse_line(line: &str, expand: &dyn Fn(&str) -> String) -> Result<Vec<Cmd>, String> {
    let aliases = config::get().get("aliases");
    let mut out = Vec::new();
    split_expand(line.trim().trim_start_matches(':'), &aliases, 0, &mut out)?;
    out.iter().map(|p| parse_one(p, expand)).collect()
}

fn split_expand(
    line: &str,
    aliases: &serde_json::Value,
    depth: u32,
    out: &mut Vec<String>,
) -> Result<(), String> {
    if depth > 8 {
        return Err("alias loop".into());
    }
    for part in line.split(";;").map(str::trim).filter(|p| !p.is_empty()) {
        let (first, tail) = part.split_once(char::is_whitespace).unwrap_or((part, ""));
        match aliases.get(first).and_then(|v| v.as_str()) {
            Some(target) => split_expand(&format!("{target} {tail}"), aliases, depth + 1, out)?,
            None => out.push(part.to_string()),
        }
    }
    Ok(())
}

fn parse_one(part: &str, expand: &dyn Fn(&str) -> String) -> Result<Cmd, String> {
    // Placeholders are filled per token, so a title with quotes or spaces stays one argument.
    let mut tokens = tokenize(part)?.into_iter().map(|t| {
        if t.contains('{') {
            expand(&t)
        } else {
            t
        }
    });
    let name = tokens.next().ok_or("empty command")?;
    let (mut flags, mut args, mut flags_done) = (vec![], vec![], false);
    for t in tokens {
        if !flags_done && t == "--" {
            flags_done = true;
        } else if !flags_done
            && args.is_empty()
            && t.len() > 1
            && t.starts_with('-')
            && !t[1..].starts_with(|c: char| c.is_ascii_digit())
        {
            if t.starts_with("--") {
                flags.push(t);
            } else {
                // -sr → -s -r
                flags.extend(t[1..].chars().map(|c| format!("-{c}")));
            }
        } else {
            args.push(t);
        }
    }
    Ok(Cmd { name, flags, args })
}

/// qutebrowser `url.searchengines` + `url.auto_search`: `g rust` uses engine `g`;
/// otherwise (naive) one word with a dot or localhost is a URL, the rest is searched.
pub fn url_or_search(text: &str) -> String {
    let cfg = config::get();
    let engines = cfg.get("url.searchengines");
    url_or_search_with(text, &engines, &cfg.str("url.auto_search"))
}

fn url_or_search_with(text: &str, engines: &serde_json::Value, auto_search: &str) -> String {
    let t = text.trim();
    if let Some((word, rest)) = t.split_once(' ') {
        if let Some(tpl) = engines
            .get(word)
            .and_then(|v| v.as_str())
            .filter(|_| word != "DEFAULT")
        {
            return search_url(tpl, rest.trim());
        }
    }
    let has_scheme = t.contains("://")
        || ["about:", "file:", "data:", "chrome:"]
            .iter()
            .any(|p| t.starts_with(p));
    if has_scheme {
        return t.to_string();
    }
    // shortcut: "dns" and "schemeless" behave like "naive" (no DNS lookups)
    let looks_like_url =
        !t.contains(char::is_whitespace) && (t.starts_with("localhost") || t.contains('.'));
    if auto_search == "never" || looks_like_url {
        let scheme = if t.starts_with("localhost") {
            "http"
        } else {
            "https"
        };
        return format!("{scheme}://{t}");
    }
    let tpl = engines
        .get("DEFAULT")
        .and_then(|v| v.as_str())
        .unwrap_or("https://www.google.com/search?q={}");
    search_url(tpl, t)
}

/// Fill a search engine template: `{}` quoted, `{unquoted}` raw (qutebrowser).
fn search_url(tpl: &str, term: &str) -> String {
    let q: String = term
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b' ' => "+".into(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    tpl.replace("{unquoted}", term)
        .replace("{semiquoted}", &q)
        .replace("{quoted}", &q)
        .replace("{}", &q)
}

/// `zoom.levels` as numbers ("110%" → 110).
fn zoom_levels() -> Vec<f64> {
    let mut v: Vec<f64> = config::get()
        .list("zoom.levels")
        .iter()
        .filter_map(|z| z.trim_end_matches('%').parse().ok())
        .collect();
    v.sort_by(f64::total_cmp);
    if v.is_empty() {
        v.push(100.0);
    }
    v
}

/// Next zoom level `steps` away from `current` percent (negative = out).
pub fn zoom_step(current: f64, steps: i32) -> f64 {
    zoom_step_in(&zoom_levels(), current, steps)
}

fn zoom_step_in(levels: &[f64], current: f64, steps: i32) -> f64 {
    let last = levels.len() as i32 - 1;
    // first level above (zoom in) / below (zoom out) the current one, then the remaining steps
    let target = if steps > 0 {
        levels
            .iter()
            .position(|&z| z > current + 0.5)
            .map_or(last, |i| i as i32 + steps - 1)
    } else {
        levels
            .iter()
            .rposition(|&z| z < current - 0.5)
            .map_or(0, |i| i as i32 + steps + 1)
    };
    levels[target.clamp(0, last) as usize]
}

/// `:set` / `:config-cycle` / settings page: save the value unless `temp`. A value
/// config.ts sets can only be overridden until restart (config.ts wins).
pub fn set_user(name: &str, value: &str, temp: bool) -> Result<(), String> {
    let cfg = config::get();
    if temp || cfg.in_config_ts(name) {
        cfg.set_runtime(name, value)?;
        if !temp {
            crate::host::message(
                "warning",
                &format!("{name} is set in config.ts: changed until restart only"),
            );
        }
        return Ok(());
    }
    config::save(name, Some(value))
}

/// Host of a URL, without userinfo or port.
pub fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
    let auth = auth.rsplit_once('@').map_or(auth, |(_, h)| h);
    if auth.starts_with('[') {
        return auth.split_inclusive(']').next().unwrap_or(auth);
    }
    auth.split(':').next().unwrap_or(auth)
}

/// Implemented commands, for `:` completion. Each edition's `run_one` must handle all
/// of them (or report "not supported in this edition").
pub const NAMES: &[&str] = &[
    "adblock-update",
    "back",
    "bindings",
    "bookmark-add",
    "bookmark-del",
    "bookmark-list",
    "bookmark-load",
    "password-fill",
    "bookmarks",
    "clear-keychain",
    "close",
    "config-cycle",
    "config-source",
    "cmd-set-text",
    "dark-toggle",
    "devtools",
    "download",
    "download-cancel",
    "download-clear",
    "download-open",
    "downloads",
    "help",
    "history",
    "https-toggle",
    "fake-key",
    "forward",
    "fullscreen",
    "hint",
    "insert-text",
    "message-error",
    "message-info",
    "message-warning",
    "mode-enter",
    "mode-leave",
    "nop",
    "open",
    "quickmark-add",
    "quickmark-del",
    "quickmark-load",
    "quickmark-save",
    "profile-new",
    "profile-open",
    "quit",
    "reload",
    "scroll",
    "scroll-page",
    "scroll-to-perc",
    "search",
    "search-next",
    "set",
    "settings",
    "search-prev",
    "session-delete",
    "session-load",
    "session-save",
    "spawn",
    "stop",
    "tab-clone",
    "tab-close",
    "tab-focus",
    "tab-move",
    "tab-mute",
    "tab-next",
    "tab-only",
    "tab-pin",
    "tab-give",
    "tab-hibernate",
    "tab-keep-awake",
    "tab-prev",
    "undo",
    "paste",
    "yank",
    "zoom",
    "zoom-in",
    "zoom-out",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(line: &str) -> Result<Vec<Cmd>, String> {
        parse_line(line, &|t| t.to_string())
    }

    #[test]
    fn aliases_expand_before_splitting() {
        let a = serde_json::json!({"t": "config-cycle tabs.position left top ;; message-info 'x'", "loop": "loop", "q": "close"});
        let mut out = Vec::new();
        split_expand("t ;; q", &a, 0, &mut out).unwrap();
        assert_eq!(
            out,
            [
                "config-cycle tabs.position left top",
                "message-info 'x'",
                "close"
            ]
        );
        assert!(split_expand("loop", &a, 0, &mut Vec::new()).is_err());
    }

    #[test]
    fn url_host() {
        assert_eq!(
            host("https://user:pw@example.com:8080/x?y#z"),
            "example.com"
        );
        assert_eq!(host("http://[::1]:80/"), "[::1]");
        assert_eq!(host("file:///tmp/x"), "");
        assert_eq!(host("example.org"), "example.org");
    }

    #[test]
    fn tokenize_quotes() {
        assert_eq!(
            tokenize("a 'b c' \"d \\\" e\"").unwrap(),
            ["a", "b c", "d \" e"]
        );
        assert_eq!(tokenize("  x  ").unwrap(), ["x"]);
        assert_eq!(tokenize("''").unwrap(), [""]);
        assert_eq!(
            tokenize(r#"fake-key \< \a " ""#).unwrap(),
            ["fake-key", "<", "a", " "]
        );
        assert_eq!(tokenize(r"'\x'").unwrap(), [r"\x"]);
        assert!(tokenize("'open").is_err());
    }

    #[test]
    fn parse_flags_args_chain_alias() {
        let c = parse(":open -t example.com").unwrap();
        assert_eq!(
            c,
            [Cmd {
                name: "open".into(),
                flags: vec!["-t".into()],
                args: vec!["example.com".into()]
            }]
        );
        let c = &parse("cmd-set-text -sr :tab-focus").unwrap()[0];
        assert_eq!(
            (c.flags.clone(), c.args.clone()),
            (vec!["-s".into(), "-r".into()], vec![":tab-focus".into()])
        );
        let c = &parse("open -- -t").unwrap()[0];
        assert_eq!(c.args, ["-t"]);
        let c = &parse("tab-move -").unwrap()[0];
        assert_eq!(c.args, ["-"]);
        let c = &parse("scroll-page 0 -0.5").unwrap()[0];
        assert_eq!(c.args, ["0", "-0.5"]);
        let names: Vec<_> = parse("clear-keychain ;; search ;; fullscreen --leave")
            .unwrap()
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(names, ["clear-keychain", "search", "fullscreen"]);
        assert_eq!(parse("q").unwrap()[0].name, "close");
        assert_eq!(parse("wq").unwrap()[0].flags, ["--save"]);
        let c = &parse("open {url}/x '{title}'").unwrap()[0];
        assert_eq!(c.args, ["{url}/x", "{title}"]);
        let c = &parse_line("open {url}", &|t| t.replace("{url}", "a b")).unwrap()[0];
        assert_eq!(c.args, ["a b"]);
    }

    #[test]
    fn urls_vs_search() {
        assert_eq!(url_or_search("example.com"), "https://example.com");
        assert_eq!(url_or_search("https://a.b/c?d"), "https://a.b/c?d");
        assert_eq!(url_or_search("localhost:8080"), "http://localhost:8080");
        assert_eq!(url_or_search("about:blank"), "about:blank");
        assert_eq!(
            url_or_search("rust lang"),
            "https://www.google.com/search?q=rust+lang"
        );
        assert_eq!(
            url_or_search("a&b"),
            "https://www.google.com/search?q=a%26b"
        );
        let e = serde_json::json!({"DEFAULT": "https://s.x/?q={}", "g": "https://g.x/search?q={}", "raw": "https://r.x/{unquoted}"});
        assert_eq!(
            url_or_search_with("g rust lang", &e, "naive"),
            "https://g.x/search?q=rust+lang"
        );
        assert_eq!(
            url_or_search_with("raw a/b", &e, "naive"),
            "https://r.x/a/b"
        );
        assert_eq!(
            url_or_search_with("hello world", &e, "naive"),
            "https://s.x/?q=hello+world"
        );
        assert_eq!(
            url_or_search_with("DEFAULT x", &e, "naive"),
            "https://s.x/?q=DEFAULT+x"
        );
        assert_eq!(
            url_or_search_with("intranet", &e, "never"),
            "https://intranet"
        );
    }

    #[test]
    fn zoom_steps() {
        assert_eq!(zoom_step(100., 1), 110.);
        assert_eq!(zoom_step(100., -1), 90.);
        assert_eq!(zoom_step(100., 3), 150.);
        assert_eq!(zoom_step(105., 1), 110.);
        assert_eq!(zoom_step(105., -1), 100.);
        assert_eq!(zoom_step(500., 1), 500.);
        assert_eq!(zoom_step(25., -1), 25.);
    }
}
