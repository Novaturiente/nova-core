//! History store on its own thread (PLAN.md §5.2): visits, command history and
//! completion queries. The UI thread only sends messages and never waits;
//! results come back with `host::on_ui`.
use rusqlite::{params, Connection};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, OnceLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

pub const LIMIT: usize = 200;
const SHORT_SCAN: i64 = 20_000;
/// Newest matches ranked (typed, then visits) for completion.
const FREQ_POOL: usize = 2_000;

enum Msg {
    /// url, opened from the address bar (`o`/`:open`)
    Visit(String, bool),
    Title(String, String),
    Cmd(String),
    /// id, text, quickmark/bookmark URLs whose visit counts to send back
    Query(u64, String, Vec<String>),
}

static TX: OnceLock<mpsc::Sender<Msg>> = OnceLock::new();
/// Newest query id: older queued queries are skipped, so fast typing never queues work.
static LATEST: AtomicU64 = AtomicU64::new(0);

static PATH: OnceLock<PathBuf> = OnceLock::new();

pub fn start(path: PathBuf) {
    PATH.set(path.clone()).ok();
    let (tx, rx) = mpsc::channel();
    TX.set(tx).ok();
    std::thread::Builder::new()
        .name("history".into())
        .spawn(move || {
            let db = match open(&path) {
                Ok(db) => db,
                Err(e) => return eprintln!("novabrowser: history: can't open {path:?}: {e}"),
            };
            match cmd_history(&db) {
                Ok(cmds) => crate::host::on_ui(move || crate::completion::set_cmd_history(cmds)),
                Err(e) => eprintln!("novabrowser: history: {e}"),
            };
            for msg in rx {
                if let Err(e) = handle(&db, msg) {
                    eprintln!("novabrowser: history: {e}");
                }
            }
        })
        .expect("spawn history thread");
}

fn send(msg: Msg) {
    if let Some(tx) = TX.get() {
        let _ = tx.send(msg);
    }
}

/// A main-frame navigation (`typed` = opened from the address bar). Only web and
/// file URLs are kept.
pub fn visit(url: &str, typed: bool) {
    if ["http:", "https:", "file:"]
        .iter()
        .any(|s| url.starts_with(s))
    {
        send(Msg::Visit(url.to_string(), typed));
    }
}
pub fn title(url: &str, title: &str) {
    send(Msg::Title(url.to_string(), title.to_string()));
}
pub fn cmd(line: &str) {
    send(Msg::Cmd(line.to_string()));
}
/// Ask for history matching `text` (most typed, then most visited first) and the
/// (typed, visits) counts of `marks`; the answer arrives as `completion::history_results(id, …)`.
pub fn query(text: &str, marks: Vec<String>) -> u64 {
    let id = LATEST.fetch_add(1, Ordering::SeqCst) + 1;
    send(Msg::Query(id, text.to_string(), marks));
    id
}

pub fn open(path: &std::path::Path) -> rusqlite::Result<Connection> {
    let db = Connection::open(path)?;
    db.execute_batch(
        // `id` order = recency: a visit re-inserts its row with the next id, so the
        // trigram index can return newest matches first and stop at LIMIT.
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;
         CREATE TABLE IF NOT EXISTS history(
             id INTEGER PRIMARY KEY, url TEXT NOT NULL UNIQUE, title TEXT NOT NULL DEFAULT '',
             last_atime INTEGER NOT NULL, visits INTEGER NOT NULL DEFAULT 1);
         CREATE VIRTUAL TABLE IF NOT EXISTS history_fts USING fts5(
             url, title, tokenize='trigram', content='history', content_rowid='id');
         CREATE TRIGGER IF NOT EXISTS history_ai AFTER INSERT ON history BEGIN
             INSERT INTO history_fts(rowid, url, title) VALUES (new.id, new.url, new.title); END;
         CREATE TRIGGER IF NOT EXISTS history_ad AFTER DELETE ON history BEGIN
             INSERT INTO history_fts(history_fts, rowid, url, title) VALUES ('delete', old.id, old.url, old.title); END;
         CREATE TRIGGER IF NOT EXISTS history_au AFTER UPDATE ON history BEGIN
             INSERT INTO history_fts(history_fts, rowid, url, title) VALUES ('delete', old.id, old.url, old.title);
             INSERT INTO history_fts(rowid, url, title) VALUES (new.id, new.url, new.title); END;
         CREATE TABLE IF NOT EXISTS cmd_history(id INTEGER PRIMARY KEY, cmd TEXT NOT NULL);",
    )?;
    // `typed` = times opened from the address bar (added 2026-10-20; older dbs get it here)
    let has_typed: bool = db.query_row(
        "SELECT count(*) FROM pragma_table_info('history') WHERE name = 'typed'",
        [],
        |r| r.get::<_, i64>(0).map(|n| n > 0),
    )?;
    if !has_typed {
        db.execute_batch("ALTER TABLE history ADD COLUMN typed INTEGER NOT NULL DEFAULT 0;")?;
    }
    db.execute_batch(
        "DROP INDEX IF EXISTS history_visits;
         CREATE INDEX IF NOT EXISTS history_rank ON history(typed, visits, id);",
    )?;
    Ok(db)
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

fn handle(db: &Connection, msg: Msg) -> rusqlite::Result<()> {
    match msg {
        // shortcut: redirects are recorded too (qutebrowser skips them); filter if completion gets noisy
        Msg::Visit(url, typed) => visit_row(db, &url, now(), typed)?,
        Msg::Title(url, title) => {
            db.prepare_cached("UPDATE history SET title = ?2 WHERE url = ?1")?
                .execute(params![url, title])?;
        }
        Msg::Cmd(line) => {
            db.prepare_cached("INSERT INTO cmd_history(cmd) VALUES (?1)")?
                .execute(params![line])?;
        }
        Msg::Query(id, text, marks) => {
            if id != LATEST.load(Ordering::SeqCst) {
                return Ok(());
            }
            let t = Instant::now();
            let items = search(db, &text, LIMIT)?;
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            if ms > 16.0 {
                eprintln!("novabrowser: history: slow query {text:?}: {ms:.1} ms");
            }
            let mut st = db.prepare_cached("SELECT typed, visits FROM history WHERE url = ?1")?;
            let mark_visits = marks
                .iter()
                .map(|u| {
                    st.query_row(params![u], |r| Ok((r.get(0)?, r.get(1)?)))
                        .unwrap_or((0, 0))
                })
                .collect();
            crate::host::on_ui(move || {
                crate::completion::history_results(id, items, mark_visits)
            });
        }
    }
    Ok(())
}

/// Move `url` to the newest id, keeping its title and counting the visit (and the
/// address-bar open when `typed`).
fn visit_row(db: &Connection, url: &str, atime: i64, typed: bool) -> rusqlite::Result<()> {
    let old: Option<(String, i64, i64)> = db
        .prepare_cached("SELECT title, visits, typed FROM history WHERE url = ?1")?
        .query_row(params![url], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .ok();
    let (title, visits, typed_n) = old.unwrap_or_default();
    db.execute_batch("BEGIN")?;
    db.prepare_cached("DELETE FROM history WHERE url = ?1")?
        .execute(params![url])?;
    db.prepare_cached(
        "INSERT INTO history(url, title, last_atime, visits, typed) VALUES (?1, ?2, ?3, ?4, ?5)",
    )?
    .execute(params![
        url,
        title,
        atime,
        visits + 1,
        typed_n + typed as i64
    ])?;
    db.execute_batch("COMMIT")
}

/// Merge imported rows into history and renumber everything by recency (the id
/// order search relies on). Per URL: newest time, newest non-empty title, max visits
/// and max typed count (max, not sum, so importing the same profile twice changes nothing). Returns the row count.
pub fn import(db: &Connection, rows: &[crate::import::Row]) -> rusqlite::Result<i64> {
    db.execute_batch(
        "BEGIN;
         CREATE TEMP TABLE imp(url TEXT, title TEXT, atime INTEGER, visits INTEGER, typed INTEGER);
         INSERT INTO imp SELECT url, title, last_atime, visits, typed FROM history;",
    )?;
    {
        let mut st = db.prepare("INSERT INTO imp VALUES (?1, ?2, ?3, ?4, ?5)")?;
        for (url, title, atime, visits, typed) in rows {
            st.execute(params![url, title, atime, visits, typed])?;
        }
    }
    db.execute_batch(
        "CREATE INDEX temp.imp_url ON imp(url, atime);
         DELETE FROM history;
         INSERT INTO history(url, title, last_atime, visits, typed)
             SELECT url,
                    coalesce((SELECT i.title FROM imp i WHERE i.url = imp.url AND i.title != ''
                              ORDER BY i.atime DESC LIMIT 1), ''),
                    max(atime), max(visits), max(typed)
             FROM imp GROUP BY url ORDER BY max(atime), url;
         DROP TABLE imp;
         COMMIT;",
    )?;
    db.query_row("SELECT count(*) FROM history", [], |r| r.get(0))
}

fn cmd_history(db: &Connection) -> rusqlite::Result<Vec<String>> {
    let mut st = db.prepare(
        "SELECT cmd FROM (SELECT id, cmd FROM cmd_history ORDER BY id DESC LIMIT 1000) ORDER BY id",
    )?;
    let rows = st.query_map([], |r| r.get(0))?;
    rows.collect()
}

/// Completion: most opened from the address bar first, then most visited, then newest; every word must be in the URL or the
/// title (qutebrowser's history model). Words of 3+ chars go through the trigram index; shorter ones (which it
/// can't index) are LIKE filters on top.
pub fn search(
    db: &Connection,
    text: &str,
    limit: usize,
) -> rusqlite::Result<Vec<(String, String)>> {
    Ok(search_rows(db, text, limit, true)?
        .into_iter()
        .map(|(u, t, _)| (u, t))
        .collect())
}

/// `search` plus each row's last visit time (unix seconds); `frequent` = most typed,
/// then most visited first, else newest first.
fn search_rows(
    db: &Connection,
    text: &str,
    limit: usize,
    frequent: bool,
) -> rusqlite::Result<Vec<(String, String, i64)>> {
    let (long, short): (Vec<&str>, Vec<&str>) = text
        .split_whitespace()
        .partition(|w| w.chars().count() >= 3);
    let mut args: Vec<String> = Vec::new();
    if !long.is_empty() {
        args.push(
            long.iter()
                .map(|w| format!("\"{}\"", w.replace('"', "\"\"")))
                .collect::<Vec<_>>()
                .join(" AND "),
        );
    }
    let mut conds = Vec::new();
    for w in short {
        args.push(format!(
            "%{}%",
            w.replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        ));
        let i = args.len();
        conds.push(format!(
            "(h.url LIKE ?{i} ESCAPE '\\' OR h.title LIKE ?{i} ESCAPE '\\')"
        ));
    }
    // newest first (the trigram index returns rowid order, so LIMIT stops it early)
    let newest = if long.is_empty() {
        // The index can't serve 1-2 char words: scan only the newest SHORT_SCAN rows (~5 ms),
        // these queries are transient while typing.
        conds.push(format!(
            "h.id > (SELECT coalesce(max(id), 0) FROM history) - {SHORT_SCAN}"
        ));
        format!(
            "SELECT h.id, h.typed, h.visits, h.url, h.title, h.last_atime FROM history h WHERE {} ORDER BY h.id DESC",
            conds.join(" AND ")
        )
    } else {
        let more: String = conds.iter().map(|c| format!(" AND {c}")).collect();
        format!(
            "SELECT h.id, h.typed, h.visits, h.url, h.title, h.last_atime FROM history_fts f
             JOIN history h ON h.id = f.rowid WHERE history_fts MATCH ?1{more} ORDER BY f.rowid DESC"
        )
    };
    let sql = if !frequent {
        format!(
            "SELECT url, title, last_atime FROM ({newest} LIMIT {limit})
             ORDER BY id DESC"
        )
    } else if text.trim().is_empty() {
        // `o` with nothing typed: all of history, served by the history_rank index
        format!("SELECT url, title, last_atime FROM history ORDER BY typed DESC, visits DESC, id DESC LIMIT {limit}")
    } else {
        // shortcut: only the newest FREQ_POOL matches are ranked by visits (sorting every
        // match of a common word takes ~50 ms at 100k rows); raise it if old favourites go missing
        format!(
            "SELECT url, title, last_atime FROM ({newest} LIMIT {FREQ_POOL})
             ORDER BY typed DESC, visits DESC, id DESC LIMIT {limit}"
        )
    };
    let mut st = db.prepare_cached(&sql)?;
    let rows = st.query_map(rusqlite::params_from_iter(&args), |r| {
        Ok((r.get(0)?, r.get(1)?, r.get(2)?))
    })?;
    rows.collect()
}

/// Rows shown on the history page per search.
const PAGE_ROWS: usize = 500;

thread_local! {
    /// The history page's current search, kept across its reload after a delete.
    static PAGE_QUERY: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

/// Short-lived connection on the UI thread for the history page (a user action;
/// WAL lets it read and delete next to the history thread).
fn page_db() -> Result<Connection, String> {
    let p = PATH.get().ok_or("history not started")?;
    open(p).map_err(|e| e.to_string())
}

/// `:history [words]`: newest visits (matching the words), searchable, deletable.
pub fn page(query: Option<&str>) -> String {
    if let Some(q) = query {
        PAGE_QUERY.set(q.to_string());
    }
    let q = PAGE_QUERY.with_borrow(|q| q.clone());
    let rows = page_db()
        .and_then(|db| search_rows(&db, &q, PAGE_ROWS, false).map_err(|e| e.to_string()))
        .unwrap_or_else(|e| {
            eprintln!("novabrowser: history page: {e}");
            vec![]
        });
    let data =
        serde_json::json!({ "token": crate::paths::page_token(), "q": q, "rows": rows, "max": PAGE_ROWS });
    crate::theme::themed_page(
        PAGE.replace(
            "/*DATA*/{}",
            &serde_json::to_string(&data)
                .unwrap()
                .replace('<', "\\u003c"),
        ),
    )
}

/// History page actions: {a: "search", q} or {a: "del", urls: [...]}.
pub fn on_page(msg: &serde_json::Value) -> Result<(), String> {
    match msg["a"].as_str() {
        Some("search") => {
            PAGE_QUERY.set(msg["q"].as_str().unwrap_or("").to_string());
            Ok(())
        }
        Some("del") => {
            let db = page_db()?;
            let mut st = db
                .prepare("DELETE FROM history WHERE url = ?1")
                .map_err(|e| e.to_string())?;
            for u in msg["urls"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|u| u.as_str())
            {
                st.execute(params![u]).map_err(|e| e.to_string())?;
            }
            Ok(())
        }
        _ => Err("unknown page action".into()),
    }
}

const PAGE: &str = r##"<!doctype html><meta charset=utf-8><title>History</title>
<style>
body{font:14px system-ui,sans-serif;max-width:64em;margin:2em auto;padding:0 1em;color-scheme:light dark}
h1{font-size:1.3em;font-weight:600;display:flex;gap:1em;align-items:center}h2{font-size:1em;margin:1.4em 0 .4em;opacity:.7}
input{font:inherit;padding:.35em .6em;border-radius:8px;border:1px solid color-mix(in srgb,currentColor 25%,transparent);background:transparent;color:inherit;min-width:0}
button{font:inherit;padding:.3em .8em;border-radius:8px;border:0;cursor:pointer;background:color-mix(in srgb,currentColor 12%,transparent);color:inherit}
button.del:hover{background:#c0303066}
#q{flex:1;max-width:24em;font-size:.8em}
.r{display:flex;gap:.6em;align-items:center;padding:.35em .6em;border-radius:10px;margin:.2em 0;background:color-mix(in srgb,currentColor 6%,transparent)}
.r time{opacity:.6;flex:0 0 3.5em;font-variant-numeric:tabular-nums}
.r .main{flex:1;min-width:0;display:flex;flex-direction:column}
.r a{color:inherit;text-decoration:none;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.r a:hover{text-decoration:underline}
.r small{opacity:.6;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.empty,.note{opacity:.6}
</style>
<h1>History <form id=f style="display:contents"><input id=q placeholder="Search all history…" autocomplete=off></form><button id=all class=del>Delete shown</button></h1>
<p class=note id=note></p><div id=l></div>
<script>
const D=/*DATA*/{};
const send=(m)=>window.nova&&nova.send("page "+D.token+" "+JSON.stringify({page:"history",...m}));
const el=(t,p,...c)=>{const e=document.createElement(t);Object.assign(e,p);e.append(...c);return e;};
const q=document.getElementById("q");q.value=D.q;
document.getElementById("f").onsubmit=(e)=>{e.preventDefault();send({a:"search",q:q.value});};
document.getElementById("all").onclick=()=>{if(D.rows.length&&confirm("Delete these "+D.rows.length+" entries from history?"))send({a:"del",urls:D.rows.map(r=>r[0])});};
const note=document.getElementById("note");
note.textContent=(D.q?"Matching “"+D.q+"”. ":"")+(D.rows.length>=D.max?"Showing the newest "+D.max+" — search to find older visits.":"");
const l=document.getElementById("l");let day="";
for(const[url,title,t]of D.rows){
  const d=new Date(t*1000),k=d.toLocaleDateString(undefined,{weekday:"long",year:"numeric",month:"long",day:"numeric"});
  if(k!==day){day=k;l.append(el("h2",{textContent:k}));}
  l.append(el("div",{className:"r"},el("time",{textContent:d.toLocaleTimeString(undefined,{hour:"2-digit",minute:"2-digit"})}),
    el("div",{className:"main"},el("a",{href:/^(https?|file):/.test(url)?url:"#",textContent:title||url}),el("small",{textContent:url})),
    el("button",{className:"del",textContent:"Delete",onclick:()=>send({a:"del",urls:[url]})})));
}
if(!D.rows.length)l.append(el("p",{className:"empty",textContent:D.q?"Nothing matches.":"No history yet."}));
if(!D.q)q.focus();
</script>"##;

#[cfg(test)]
mod tests {
    use super::*;

    fn fake(n: i64) -> Connection {
        let mut db = open(std::path::Path::new(":memory:")).unwrap();
        let tx = db.transaction().unwrap();
        {
            let mut st = tx
                .prepare("INSERT INTO history(id, url, title, last_atime) VALUES (?3, ?1, ?2, ?3)")
                .unwrap();
            for i in 0..n {
                st.execute(params![
                    format!(
                        "https://site{}.example.com/articles/{i}/some-slug-text",
                        i % 5000
                    ),
                    format!("Article {i} about topic {} - Site {}", i % 300, i % 5000),
                    i
                ])
                .unwrap();
            }
        }
        tx.commit().unwrap();
        db
    }

    #[test]
    fn search_matches_all_words_newest_first_and_escapes() {
        let db = open(std::path::Path::new(":memory:")).unwrap();
        for (u, t) in [
            ("https://a.com/x", "Rust lang"),
            ("https://b.com/rust", "B"),
            ("https://c.com/100%_", "C"),
        ] {
            visit_row(&db, u, 0, false).unwrap();
            db.execute(
                "UPDATE history SET title = ?2 WHERE url = ?1",
                params![u, t],
            )
            .unwrap();
        }
        let urls = |q: &str| {
            search(&db, q, 10)
                .unwrap()
                .into_iter()
                .map(|r| r.0)
                .collect::<Vec<_>>()
        };
        assert_eq!(urls("rust"), ["https://b.com/rust", "https://a.com/x"]);
        assert_eq!(urls("RUST a.com"), ["https://a.com/x"]);
        assert_eq!(urls("%_"), ["https://c.com/100%_"]);
        assert_eq!(urls("_"), ["https://c.com/100%_"]);
        assert_eq!(urls("").len(), 3);
        assert_eq!(urls("rust x"), ["https://a.com/x"]); // trigram + short word
        assert_eq!(urls("\"rust"), Vec::<String>::new()); // quote can't break the MATCH syntax
                                                          // a revisit moves to the front and keeps title + count
        visit_row(&db, "https://a.com/x", 9, false).unwrap();
        assert_eq!(urls("rust"), ["https://a.com/x", "https://b.com/rust"]);
        let (t, v): (String, i64) = db
            .query_row(
                "SELECT title, visits FROM history WHERE url = 'https://a.com/x'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!((t.as_str(), v), ("Rust lang", 2));
        // more visits beat a newer visit
        visit_row(&db, "https://b.com/rust", 10, false).unwrap();
        visit_row(&db, "https://b.com/rust", 11, false).unwrap();
        visit_row(&db, "https://b.com/rust", 11, false).unwrap();
        visit_row(&db, "https://a.com/x", 12, false).unwrap();
        assert_eq!(urls("rust"), ["https://b.com/rust", "https://a.com/x"]);
        assert_eq!(urls("")[0], "https://b.com/rust");
        // opened from the address bar beats more visits by links
        visit_row(&db, "https://a.com/x", 13, true).unwrap();
        assert_eq!(urls("rust"), ["https://a.com/x", "https://b.com/rust"]);
        assert_eq!(urls("")[0], "https://a.com/x");
        visit_row(&db, "https://b.com/rust", 14, false).unwrap();
        assert_eq!(urls("rust")[0], "https://a.com/x");
        visit_row(&db, "https://b.com/rust", 15, true).unwrap();
        visit_row(&db, "https://b.com/rust", 16, true).unwrap();
        assert_eq!(urls("rust")[0], "https://b.com/rust"); // typed twice now
    }

    #[test]
    fn old_db_gets_typed_column() {
        let dir = std::env::temp_dir().join(format!("nova-hist-mig-{}", std::process::id()));
        let _ = std::fs::remove_file(&dir);
        {
            // the pre-2026-10-20 schema: today's minus `typed`, with the old index
            let db = open(&dir).unwrap();
            db.execute_batch(
                "DROP INDEX history_rank;
                 ALTER TABLE history DROP COLUMN typed;
                 CREATE INDEX history_visits ON history(visits, id);
                 INSERT INTO history(url, title, last_atime, visits) VALUES ('https://o.com/', 'O', 1, 5);",
            )
            .unwrap();
        }
        let db = open(&dir).unwrap();
        visit_row(&db, "https://o.com/", 2, true).unwrap();
        let row: (i64, i64) = db
            .query_row("SELECT visits, typed FROM history", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(row, (6, 1));
        drop(db);
        open(&dir).unwrap(); // second open: no duplicate ALTER
        let _ = std::fs::remove_file(&dir);
    }

    #[test]
    fn import_merges_by_recency_and_is_idempotent() {
        let db = open(std::path::Path::new(":memory:")).unwrap();
        visit_row(&db, "https://new.com/", 100, true).unwrap();
        db.execute(
            "UPDATE history SET title = 'New' WHERE url = 'https://new.com/'",
            [],
        )
        .unwrap();
        let rows: Vec<crate::import::Row> = vec![
            ("https://old.com/".into(), "Old".into(), 10, 3, 2),
            ("https://new.com/".into(), "".into(), 50, 7, 0),
            ("https://mid.com/".into(), "Mid".into(), 60, 1, 0),
        ];
        assert_eq!(import(&db, &rows).unwrap(), 3);
        assert_eq!(import(&db, &rows).unwrap(), 3);
        let all: Vec<(String, String, i64, i64, i64)> = db
            .prepare("SELECT url, title, last_atime, visits, typed FROM history ORDER BY id DESC")
            .unwrap()
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            all,
            [
                ("https://new.com/".into(), "New".into(), 100, 7, 1),
                ("https://mid.com/".into(), "Mid".into(), 60, 1, 0),
                ("https://old.com/".into(), "Old".into(), 10, 3, 2),
            ]
        );
        // the trigram index follows the rebuilt table
        assert_eq!(
            search(&db, "old", 10).unwrap(),
            [("https://old.com/".into(), "Old".into())]
        );
    }

    /// Phase 3 exit check (PLAN.md §8): completion under 16 ms over 100k rows.
    #[test]
    fn search_100k_rows_under_16ms() {
        let db = fake(100_000);
        for q in [
            "",
            "site42",
            "topic 7",
            "slug article",
            "nomatchanywhere",
            "site4999 99999",
            "site 7",
            "w",
            "zq",
            "x y",
        ] {
            search(&db, q, LIMIT).unwrap(); // warm the statement cache
            let t = Instant::now();
            let n = search(&db, q, LIMIT).unwrap().len();
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            eprintln!("query {q:?}: {n} rows in {ms:.2} ms");
            assert!(ms < 16.0, "{q:?} took {ms:.2} ms");
        }
    }
}