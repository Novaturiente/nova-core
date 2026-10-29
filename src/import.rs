//! `novabrowser --import [profile]`: one-shot history import from Chromium-based
//! browsers (`History`), Firefox-based browsers (`places.sqlite`) and qutebrowser
//! (`history.sqlite`). Runs before CEF starts; safe while the other browser runs
//! (works on a copy) and while novabrowser runs (SQLite WAL).
use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// (name for `--import <name>`, profile parent dir under $HOME).
const CHROMIUM: &[(&str, &str)] = &[
    ("chrome", ".config/google-chrome"),
    ("chromium", ".config/chromium"),
    ("helium", ".config/net.imput.helium"),
    ("brave", ".config/BraveSoftware/Brave-Browser"),
    ("vivaldi", ".config/vivaldi"),
    ("edge", ".config/microsoft-edge"),
];
const FIREFOX: &[(&str, &str)] = &[
    ("firefox", ".mozilla/firefox"),
    ("zen", ".config/zen"),
    ("zen", ".zen"),
    ("librewolf", ".librewolf"),
    ("floorp", ".floorp"),
];

/// `--import zen` etc.: that browser's profile with the biggest history file
/// (browsers keep empty leftover profiles next to the real one).
fn by_name(name: &str) -> Option<PathBuf> {
    if name == "qutebrowser" {
        return found()
            .into_iter()
            .find(|(n, _)| *n == "qutebrowser")
            .map(|(_, p)| p);
    }
    found()
        .into_iter()
        .filter(|(n, _)| *n == name)
        .max_by_key(|(_, p)| {
            history_file(p)
                .and_then(|f| f.metadata().ok())
                .map_or(0, |m| m.len())
        })
        .map(|(_, p)| p)
}

pub fn main(arg: Option<&str>, history_db: &Path) -> Result<(), String> {
    let Some(arg) = arg else {
        println!("Usage: novabrowser --import <browser name | profile dir | history file>\nFound:");
        for (name, p) in found() {
            let mb = history_file(&p)
                .and_then(|f| f.metadata().ok())
                .map_or(0.0, |m| m.len() as f64 / 1e6);
            println!("  {name:<12} {mb:>7.1} MB  {}", p.display());
        }
        println!("A name picks that browser's profile with the most history, e.g. `--import zen`.");
        if crate::paths::app() != "novabrowser" {
            println!("`--import novabrowser` takes the CEF edition's history, marks, config, sessions and permissions.");
        }
        return Ok(());
    };
    if arg == "novabrowser" {
        return from_cef(history_db);
    }
    let dir = if arg.contains('/') {
        Some(PathBuf::from(arg))
    } else {
        by_name(arg)
    };
    let file = dir
        .as_deref()
        .and_then(history_file)
        .ok_or_else(|| format!("no History, places.sqlite or history.sqlite for {arg} (run --import alone to list profiles)"))?;
    let (kind, rows, book) = read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    let db = crate::history::open(history_db).map_err(|e| e.to_string())?;
    let total = crate::history::import(&db, &rows).map_err(|e| e.to_string())?;
    println!(
        "imported {} {kind} entries from {}; history now has {total}",
        rows.len(),
        file.display()
    );
    let dir = file.parent().unwrap_or(Path::new("."));
    let (quick, book) = match kind {
        "Chromium" => (vec![], chromium_bookmarks(&dir.join("Bookmarks"))),
        "qutebrowser" => {
            let cfg = qutebrowser_config();
            let read = |f: &str| std::fs::read_to_string(cfg.join(f)).unwrap_or_default();
            if let Some(n) = qutebrowser_session(&dir.join("sessions/default.yml"))? {
                println!("imported qutebrowser session ({n} tabs) as session 'qutebrowser' (:session-load qutebrowser)");
            }
            (
                crate::marks::parse(&read("quickmarks"), true),
                crate::marks::parse(&read("bookmarks/urls"), false),
            )
        }
        _ => (vec![], book),
    };
    let (nq, nb) = crate::marks::import(&quick, &book)?;
    println!(
        "imported {nq} new quickmarks and {nb} new bookmarks (of {} and {})",
        quick.len(),
        book.len()
    );
    Ok(())
}

/// `--import novabrowser` (WebKit edition): the CEF edition's data for the same
/// profile. History, cmd history and marks merge; config files, sessions, permission
/// answers and the downloads list are copied only where this edition has none yet, so
/// running it twice changes nothing and never overwrites. Not carried over: cookies,
/// logins, site data, and Chromium's per-host zoom (inside its `Preferences`).
fn from_cef(history_db: &Path) -> Result<(), String> {
    if crate::paths::app() == "novabrowser" {
        return Err("this is the CEF edition already".into());
    }
    let base = |var: &str, fallback: &str| {
        std::env::var_os(var)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(fallback))
            .join("novabrowser")
    };
    let prof = |p: PathBuf| match crate::paths::profile() {
        Some(n) => p.join("profiles").join(n),
        None => p,
    };
    let (src_data, src_cfg) = (prof(base("XDG_DATA_HOME", ".local/share")), base("XDG_CONFIG_HOME", ".config"));
    let dst_data = crate::paths::xdg_dir("XDG_DATA_HOME", ".local/share");
    let dst_cfg = crate::config::dir();
    if !src_data.is_dir() && !src_cfg.is_dir() {
        return Err(format!("no CEF edition data in {} or {}", src_data.display(), src_cfg.display()));
    }

    let hist = src_data.join("history.sqlite");
    if hist.is_file() {
        let (_, rows, _) = read(&hist).map_err(|e| format!("{}: {e}", hist.display()))?;
        let db = crate::history::open(history_db).map_err(|e| e.to_string())?;
        let total = crate::history::import(&db, &rows).map_err(|e| e.to_string())?;
        let n = import_cmd_history(&db, &hist).map_err(|e| format!("cmd history: {e}"))?;
        println!("imported {} history entries and {n} commands; history now has {total}", rows.len());
    }

    // marks: next to config.ts for the default profile, in the data dir for named ones
    let marks = if crate::paths::profile().is_some() { &src_data } else { &src_cfg };
    let read_m = |f: &str| std::fs::read_to_string(marks.join(f)).unwrap_or_default();
    let quick = crate::marks::parse(&read_m("quickmarks"), true);
    let book = crate::marks::parse(&read_m("bookmarks/urls"), false);
    let (nq, nb) = crate::marks::import(&quick, &book)?;
    println!("imported {nq} new quickmarks and {nb} new bookmarks");

    let mut copied = vec![];
    let mut copy = |from: PathBuf, to: PathBuf| -> Result<(), String> {
        if from.is_file() && !to.exists() {
            std::fs::create_dir_all(to.parent().unwrap()).map_err(|e| e.to_string())?;
            std::fs::copy(&from, &to).map_err(|e| format!("{}: {e}", from.display()))?;
            copied.push(to.display().to_string());
        }
        Ok(())
    };
    // config.ts and every file next to it (excludes lists, files config.ts reads)
    let mut cfg_dirs = vec![(src_cfg.clone(), dst_cfg.clone())];
    if let Some(n) = crate::paths::profile() {
        cfg_dirs.push((src_cfg.join("profiles").join(n), dst_cfg.join("profiles").join(n)));
    }
    for (from, to) in cfg_dirs {
        for e in std::fs::read_dir(&from).into_iter().flatten().flatten() {
            if e.file_name() != "quickmarks" {
                copy(e.path(), to.join(e.file_name()))?;
            }
        }
    }
    for f in ["permissions.json", "downloads.json"] {
        copy(src_data.join(f), dst_data.join(f))?;
    }
    for e in std::fs::read_dir(src_data.join("sessions")).into_iter().flatten().flatten() {
        copy(e.path(), dst_data.join("sessions").join(e.file_name()))?;
    }
    println!("copied {} files (existing ones kept){}", copied.len(),
        copied.iter().map(|c| format!("\n  {c}")).collect::<String>());
    println!("not imported: cookies, logins and site data (sign in again), per-site zoom");
    Ok(())
}

/// Append the CEF edition's command-line history when ours is still empty
/// (it has no timestamps to merge by). Returns how many were added.
fn import_cmd_history(db: &Connection, src: &Path) -> rusqlite::Result<usize> {
    if db.query_row("SELECT count(*) FROM cmd_history", [], |r| r.get::<_, i64>(0))? > 0 {
        return Ok(0);
    }
    let src = Connection::open_with_flags(src, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let cmds: Vec<String> = src
        .prepare("SELECT cmd FROM cmd_history ORDER BY id")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut st = db.prepare("INSERT INTO cmd_history(cmd) VALUES (?1)")?;
    for c in &cmds {
        st.execute([c])?;
    }
    Ok(cmds.len())
}

fn qutebrowser_config() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        })
        .join("qutebrowser")
}

/// Chromium `Bookmarks` JSON: every `url` node under every root, as (url, title).
fn chromium_bookmarks(file: &Path) -> Vec<(String, String)> {
    fn walk(n: &serde_json::Value, out: &mut Vec<(String, String)>) {
        if n["type"] == "url" {
            if let (Some(u), Some(t)) = (n["url"].as_str(), n["name"].as_str()) {
                out.push((u.to_string(), t.to_string()));
            }
        }
        for c in n["children"].as_array().into_iter().flatten() {
            walk(c, out);
        }
    }
    let Ok(text) = std::fs::read_to_string(file) else {
        return vec![];
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return vec![];
    };
    let mut out = vec![];
    for root in v["roots"].as_object().into_iter().flat_map(|o| o.values()) {
        walk(root, &mut out);
    }
    out.retain(|(u, _)| u.starts_with("http") || u.starts_with("file:"));
    out
}

/// qutebrowser `default.yml` → session "qutebrowser": each tab's current history
/// entry. shortcut: line-based reader for the layout qutebrowser writes (no YAML
/// crate); all windows become one tab list; back history dropped (PLAN §5.4).
fn qutebrowser_session(file: &Path) -> Result<Option<usize>, String> {
    let Ok(text) = std::fs::read_to_string(file) else {
        return Ok(None);
    };
    let tabs = parse_qb_session(&text);
    if tabs.is_empty() {
        return Ok(None);
    }
    let n = tabs.len();
    let tabs = tabs
        .into_iter()
        .map(|(url, title)| serde_json::json!({ "url": url, "title": title }))
        .collect();
    let v = crate::session::shape(vec![crate::session::window(tabs, 0)], &[]);
    crate::session::write("qutebrowser", &v)?;
    Ok(Some(n))
}

fn parse_qb_session(text: &str) -> Vec<(String, String)> {
    let unquote = |v: &str| {
        let v = v.trim();
        match v.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')) {
            Some(q) => q.replace("''", "'"),
            None => v.trim_matches('"').to_string(),
        }
    };
    let (mut tabs, mut cur, mut active) = (vec![], None::<(String, String)>, false);
    for line in text.lines() {
        if line.starts_with("  - ") {
            tabs.extend(cur.take());
        } else if let Some(e) = line.strip_prefix("    - ") {
            active = e.trim() == "active: true";
        } else if active {
            if let Some(t) = line.strip_prefix("      title: ") {
                cur.get_or_insert_default().1 = unquote(t);
            } else if let Some(u) = line.strip_prefix("      url: ") {
                cur.get_or_insert_default().0 = unquote(u);
            }
        }
    }
    tabs.extend(cur);
    tabs.retain(|(u, _)| !u.is_empty());
    tabs
}

pub fn found() -> Vec<(&'static str, PathBuf)> {
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let mut out = vec![];
    for (name, base) in CHROMIUM
        .iter()
        .chain(FIREFOX)
        .map(|(n, b)| (*n, home.join(b)))
    {
        let Ok(dirs) = std::fs::read_dir(&base) else {
            continue;
        };
        let mut profiles: Vec<PathBuf> = dirs
            .flatten()
            .map(|d| d.path())
            .filter(|p| p.is_dir() && history_file(p).is_some())
            .collect();
        profiles.sort();
        out.extend(profiles.into_iter().map(|p| (name, p)));
    }
    let qb = home.join(".local/share/qutebrowser");
    if qb.join("history.sqlite").exists() {
        out.push(("qutebrowser", qb));
    }
    out
}

fn history_file(p: &Path) -> Option<PathBuf> {
    if p.is_file() {
        return Some(p.to_path_buf());
    }
    ["History", "places.sqlite", "history.sqlite"]
        .iter()
        .map(|f| p.join(f))
        .find(|f| f.is_file())
}

/// (url, title, unix seconds, visits)
/// url, title, last visit (unix s), visits, times typed into the address bar
pub type Row = (String, String, i64, i64, i64);

/// Chromium time: microseconds since 1601-01-01.
const WEBKIT_EPOCH_OFFSET: i64 = 11_644_473_600;

fn read(file: &Path) -> rusqlite::Result<(&'static str, Vec<Row>, Vec<(String, String)>)> {
    // Copy db + WAL: the owning browser may hold a lock, and its newest rows may be in the WAL.
    let tmp = std::env::temp_dir().join(format!("novabrowser-import-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))?;
    let copy = tmp.join("db");
    for (from, to) in [(file.to_path_buf(), copy.clone()), (wal(file), wal(&copy))] {
        if from.exists() {
            std::fs::copy(&from, &to)
                .map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))?;
        }
    }
    let db = Connection::open(&copy)?;
    let result = read_db(&db).map(|(k, rows)| (k, rows, firefox_bookmarks(&db)));
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

/// Firefox `moz_bookmarks` (type 1 = bookmark) joined to their places; empty elsewhere.
fn firefox_bookmarks(db: &Connection) -> Vec<(String, String)> {
    let sql = "SELECT p.url, coalesce(b.title, p.title, '') FROM moz_bookmarks b JOIN moz_places p ON b.fk = p.id
               WHERE b.type = 1 AND (p.url LIKE 'http%' OR p.url LIKE 'file:%') ORDER BY b.dateAdded";
    let Ok(mut st) = db.prepare(sql) else {
        return vec![];
    };
    st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
}

fn wal(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push("-wal");
    s.into()
}

fn read_db(db: &Connection) -> rusqlite::Result<(&'static str, Vec<Row>)> {
    let has = |t: &str| {
        db.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [t],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n > 0)
    };
    let (kind, sql) = if has("urls")? {
        let sql = format!(
            "SELECT url, title, last_visit_time / 1000000 - {WEBKIT_EPOCH_OFFSET}, visit_count, typed_count FROM urls WHERE hidden = 0"
        );
        ("Chromium", sql)
    } else if has("moz_places")? {
        let sql = "SELECT url, coalesce(title, ''), last_visit_date / 1000000, visit_count, typed FROM moz_places
                   WHERE visit_count > 0 AND last_visit_date IS NOT NULL AND hidden = 0";
        ("Firefox", sql.to_string())
    } else if has("cmd_history")? {
        // novabrowser's own schema (either edition)
        ("novabrowser", "SELECT url, title, last_atime, visits, typed FROM history".to_string())
    } else if has("CompletionHistory")? {
        (
            "qutebrowser",
            "SELECT url, title, last_atime, 1, 0 FROM CompletionHistory".to_string(),
        )
    } else {
        return Err(rusqlite::Error::InvalidQuery);
    };
    let mut st = db.prepare(&sql)?;
    let rows = st.query_map([], |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
    })?;
    let web = |r: &Row| {
        ["http:", "https:", "file:"]
            .iter()
            .any(|s| r.0.starts_with(s))
    };
    Ok((
        kind,
        rows.collect::<rusqlite::Result<Vec<Row>>>()?
            .into_iter()
            .filter(web)
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_qutebrowser_session_current_entries() {
        let y = "windows:\n- active: true\n  tabs:\n  - active: true\n    history:\n    - active: false\n      title: Old\n      url: https://old/\n    - active: true\n      title: 'It''s new'\n      url: https://new/?a=1\n  - history:\n    - active: true\n      title: B\n      url: https://b/\n";
        assert_eq!(
            parse_qb_session(y),
            [
                ("https://new/?a=1".into(), "It's new".into()),
                ("https://b/".into(), "B".into())
            ] as [(String, String); 2]
        );
    }

    #[test]
    fn reads_firefox_bookmarks() {
        let f = Connection::open_in_memory().unwrap();
        f.execute_batch(
            "CREATE TABLE moz_places(id, url, title);
             CREATE TABLE moz_bookmarks(type, fk, title, dateAdded);
             INSERT INTO moz_places VALUES (1, 'https://f.com/', 'F page'), (2, 'place:x', 'Q');
             INSERT INTO moz_bookmarks VALUES (1, 1, NULL, 1), (2, NULL, 'Folder', 2), (1, 2, 'q', 3);",
        )
        .unwrap();
        assert_eq!(
            firefox_bookmarks(&f),
            [("https://f.com/".into(), "F page".into())] as [(String, String); 1]
        );
    }

    #[test]
    fn reads_chromium_firefox_qutebrowser() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE urls(url, title, last_visit_time, visit_count, hidden, typed_count);
             INSERT INTO urls VALUES ('https://a.com/', 'A', 13435995854106267, 30, 0, 4),
                                     ('https://h.com/', 'H', 13435995854106267, 1, 1, 0),
                                     ('chrome://settings/', 'S', 13435995854106267, 1, 0, 0);",
        )
        .unwrap();
        let (kind, rows) = read_db(&c).unwrap();
        // 13435995854106267 µs since 1601 = 2026-10-10 (unix 1791522254)
        assert_eq!(
            (kind, rows),
            (
                "Chromium",
                vec![("https://a.com/".into(), "A".into(), 1791522254, 30, 4)]
            )
        );

        let f = Connection::open_in_memory().unwrap();
        f.execute_batch(
            "CREATE TABLE moz_places(url, title, last_visit_date, visit_count, hidden, typed);
             INSERT INTO moz_places VALUES ('https://f.com/', NULL, 1791522254000000, 2, 0, 1),
                                           ('https://never.com/', 'N', NULL, 0, 0, 0);",
        )
        .unwrap();
        assert_eq!(
            read_db(&f).unwrap(),
            (
                "Firefox",
                vec![("https://f.com/".into(), "".into(), 1791522254, 2, 1)]
            )
        );

        let q = Connection::open_in_memory().unwrap();
        q.execute_batch(
            "CREATE TABLE CompletionHistory(url, title, last_atime);
             INSERT INTO CompletionHistory VALUES ('https://q.com/', 'Q', 5);",
        )
        .unwrap();
        assert_eq!(
            read_db(&q).unwrap(),
            (
                "qutebrowser",
                vec![("https://q.com/".into(), "Q".into(), 5, 1, 0)]
            )
        );
    }
}