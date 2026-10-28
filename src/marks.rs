//! Quickmarks (`<config>/quickmarks`: `name url` per line) and bookmarks
//! (`<config>/bookmarks/urls`: `url title` per line) — qutebrowser's files, so the
//! user's existing ones can be copied over as they are.
use crate::config;
use std::cell::RefCell;
use std::path::PathBuf;

#[derive(Default)]
struct Marks {
    loaded: bool,
    /// (name, url), file order.
    quick: Vec<(String, String)>,
    /// (url, title), file order.
    book: Vec<(String, String)>,
}

thread_local! {
    static M: RefCell<Marks> = RefCell::new(Marks::default());
}

/// Default profile: next to config.ts (qutebrowser's place); named profiles keep
/// their own marks in their data dir.
fn dir() -> PathBuf {
    match crate::paths::profile() {
        Some(_) => crate::paths::xdg_dir("XDG_DATA_HOME", ".local/share"),
        None => config::dir(),
    }
}

fn quick_file() -> PathBuf {
    dir().join("quickmarks")
}
fn book_file() -> PathBuf {
    dir().join("bookmarks").join("urls")
}

/// `name url` / `url title`: split at the first (quickmarks: last) space.
pub fn parse(text: &str, name_first: bool) -> Vec<(String, String)> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let split = if name_first {
                l.rsplit_once(' ')
            } else {
                l.split_once(' ')
            };
            let (a, b) = split.unwrap_or((l, ""));
            (a.trim().to_string(), b.trim().to_string())
        })
        .collect()
}

fn with<R>(f: impl FnOnce(&mut Marks) -> R) -> R {
    M.with_borrow_mut(|m| {
        if !m.loaded {
            m.loaded = true;
            m.quick = parse(
                &std::fs::read_to_string(quick_file()).unwrap_or_default(),
                true,
            );
            m.book = parse(
                &std::fs::read_to_string(book_file()).unwrap_or_default(),
                false,
            );
        }
        f(m)
    })
}

fn save(path: PathBuf, rows: &[(String, String)]) -> Result<(), String> {
    let text: String = rows.iter().map(|(a, b)| format!("{a} {b}\n")).collect();
    std::fs::create_dir_all(path.parent().unwrap())
        .and_then(|_| crate::paths::write_atomic(&path, text.as_bytes()))
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Re-read both files on the next use (`:config-source`).
pub fn reload() {
    M.with_borrow_mut(|m| m.loaded = false);
}

pub fn quickmarks() -> Vec<(String, String)> {
    with(|m| m.quick.clone())
}
pub fn bookmarks() -> Vec<(String, String)> {
    with(|m| m.book.clone())
}

pub fn quickmark_url(name: &str) -> Result<String, String> {
    with(|m| {
        m.quick
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, u)| u.clone())
    })
    .ok_or_else(|| format!("Quickmark '{name}' does not exist!"))
}

/// `quickmark-add url name`: replaces an existing mark of that name.
pub fn quickmark_add(url: &str, name: &str) -> Result<(), String> {
    if name.is_empty() || name.contains(char::is_whitespace) {
        return Err(format!("Invalid quickmark name: '{name}'"));
    }
    let rows = with(|m| {
        match m.quick.iter_mut().find(|(n, _)| n == name) {
            Some(e) => e.1 = url.to_string(),
            None => m.quick.push((name.to_string(), url.to_string())),
        }
        m.quick.clone()
    });
    save(quick_file(), &rows)?;
    crate::host::message("info", &format!("Added quickmark {name} for {url}"));
    Ok(())
}

/// `quickmark-del [name]`: without a name, the mark pointing at `current`.
pub fn quickmark_del(name: Option<&str>, current: &str) -> Result<(), String> {
    let rows = with(|m| {
        let i = m.quick.iter().position(|(n, u)| match name {
            Some(name) => n == name,
            None => u.trim_end_matches('/') == current.trim_end_matches('/'),
        })?;
        m.quick.remove(i);
        Some(m.quick.clone())
    })
    .ok_or_else(|| format!("Quickmark '{}' not found!", name.unwrap_or(current)))?;
    save(quick_file(), &rows)
}

/// `bookmark-add [url] [title]`; with `toggle`, an existing bookmark is removed instead.
pub fn bookmark_add(url: &str, title: &str, toggle: bool) -> Result<(), String> {
    let (rows, added) = with(|m| {
        if let Some(i) = m.book.iter().position(|(u, _)| u == url) {
            if !toggle {
                return Err(format!("Bookmark already exists: {url}"));
            }
            m.book.remove(i);
            return Ok((m.book.clone(), false));
        }
        m.book.push((url.to_string(), title.to_string()));
        Ok((m.book.clone(), true))
    })?;
    save(book_file(), &rows)?;
    let what = if added {
        "Bookmarked"
    } else {
        "Removed bookmark"
    };
    crate::host::message("info", &format!("{what} {url}"));
    Ok(())
}

pub fn bookmark_del(url: &str) -> Result<(), String> {
    let rows = with(|m| {
        let i = m.book.iter().position(|(u, _)| u == url)?;
        m.book.remove(i);
        Some(m.book.clone())
    })
    .ok_or_else(|| format!("Bookmark '{url}' not found!"))?;
    save(book_file(), &rows)
}

/// Merge imported rows (import.rs); existing names/URLs win. Returns how many were new.
pub fn import(
    quick: &[(String, String)],
    book: &[(String, String)],
) -> Result<(usize, usize), String> {
    let (q, b, nq, nb) = with(|m| {
        let (q0, b0) = (m.quick.len(), m.book.len());
        for (n, u) in quick {
            if !m.quick.iter().any(|(x, _)| x == n) {
                m.quick.push((n.clone(), u.clone()));
            }
        }
        for (u, t) in book {
            if !m.book.iter().any(|(x, _)| x == u) {
                m.book.push((u.clone(), t.clone()));
            }
        }
        (
            m.quick.clone(),
            m.book.clone(),
            m.quick.len() - q0,
            m.book.len() - b0,
        )
    });
    save(quick_file(), &q)?;
    save(book_file(), &b)?;
    Ok((nq, nb))
}

/// Rows whose text or description contains every word of `pattern` (case-insensitive).
pub fn filter(rows: Vec<(String, String)>, pattern: &str) -> Vec<(String, String)> {
    let words: Vec<String> = pattern.split_whitespace().map(str::to_lowercase).collect();
    rows.into_iter()
        .filter(|(a, b)| {
            let hay = format!("{a} {b}").to_lowercase();
            words.iter().all(|w| hay.contains(w))
        })
        .collect()
}

/// `quickmark-add`/`bookmark-add` from the manager page, replacing `old` (the
/// previous name / URL) when editing. Empty name or URL is refused.
fn set(quick: bool, old: Option<&str>, key: &str, val: &str) -> Result<(), String> {
    let (key, val) = (key.trim(), val.trim());
    if key.is_empty() || (quick && (val.is_empty() || key.contains(char::is_whitespace))) {
        return Err(if quick {
            "quickmark needs a name (no spaces) and a URL"
        } else {
            "bookmark needs a URL"
        }
        .into());
    }
    let (path, rows) = with(|m| {
        let rows = if quick { &mut m.quick } else { &mut m.book };
        let at = old.and_then(|o| rows.iter().position(|(k, _)| k == o));
        if rows
            .iter()
            .enumerate()
            .any(|(i, (k, _))| k == key && Some(i) != at)
        {
            return Err(format!("'{key}' already exists"));
        }
        match at {
            Some(i) => rows[i] = (key.to_string(), val.to_string()),
            None => rows.push((key.to_string(), val.to_string())),
        }
        Ok((if quick { quick_file() } else { book_file() }, rows.clone()))
    })?;
    // quickmarks file is `name url`, bookmarks `url title`: same (key, val) order
    save(path, &rows)
}

fn del(quick: bool, key: &str) -> Result<(), String> {
    let (path, rows) = with(|m| {
        let rows = if quick { &mut m.quick } else { &mut m.book };
        rows.retain(|(k, _)| k != key);
        (if quick { quick_file() } else { book_file() }, rows.clone())
    });
    save(path, &rows)
}

/// A message from the manager page (client.rs checked frame + token):
/// {"a": "set"|"del", "q": quickmark?, "old": key?, "k": key, "v": value}.
pub fn on_page(msg: &serde_json::Value) -> Result<(), String> {
    let quick = msg["q"].as_bool().unwrap_or(false);
    let k = msg["k"].as_str().unwrap_or("");
    match msg["a"].as_str() {
        Some("set") => set(
            quick,
            msg["old"].as_str(),
            k,
            msg["v"].as_str().unwrap_or(""),
        ),
        Some("del") => del(quick, k),
        _ => Err("unknown page action".into()),
    }
}

/// `:bookmarks` (`bookmark-list`): manage quickmarks and bookmarks — add, edit, delete,
/// filter, open. Actions go back over nova.send with the per-run page token.
pub fn page() -> String {
    let data = serde_json::json!({ "token": crate::paths::page_token(), "quick": quickmarks(), "book": bookmarks() });
    crate::theme::themed_page(
        PAGE.replace(
            "/*DATA*/{}",
            &serde_json::to_string(&data)
                .unwrap()
                .replace('<', "\\u003c"),
        ),
    )
}

const PAGE: &str = r##"<!doctype html><meta charset=utf-8><title>Bookmarks</title>
<style>
body{font:14px system-ui,sans-serif;max-width:64em;margin:2em auto;padding:0 1em;color-scheme:light dark}
h1{font-size:1.3em;font-weight:600;display:flex;gap:1em;align-items:center}h2{font-size:1.05em;margin:1.6em 0 .5em}
input{font:inherit;padding:.35em .6em;border-radius:8px;border:1px solid color-mix(in srgb,currentColor 25%,transparent);background:transparent;color:inherit;min-width:0}
button{font:inherit;padding:.3em .8em;border-radius:8px;border:0;cursor:pointer;background:color-mix(in srgb,currentColor 12%,transparent);color:inherit}
button.del:hover{background:#c0303066}
#filter{flex:1;max-width:24em;font-size:.8em}
form,.r{display:flex;gap:.5em;align-items:center;padding:.45em .6em;border-radius:10px;margin:.3em 0}
form{background:color-mix(in srgb,currentColor 4%,transparent)}
.r{background:color-mix(in srgb,currentColor 6%,transparent)}
.r .main{flex:1;min-width:0;display:flex;flex-direction:column}
.r a{font-weight:600;color:inherit;text-decoration:none;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.r a:hover{text-decoration:underline}
.r small{opacity:.6;overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.k{flex:0 0 9em}.v{flex:1}.empty{opacity:.6}
</style>
<h1>Bookmarks <input id=filter placeholder="Filter…"></h1>
<h2>Quickmarks</h2><form id=qf><input class=k name=k placeholder=name><input class=v name=v placeholder=URL><button>Add</button></form><div id=ql></div>
<h2>Bookmarks</h2><form id=bf><input class=v name=k placeholder=URL><input class=v name=v placeholder=title><button>Add</button></form><div id=bl></div>
<script>
const D=/*DATA*/{};
const send=(m)=>window.nova&&nova.send("page "+D.token+" "+JSON.stringify(m));
const el=(t,p,...c)=>{const e=document.createElement(t);Object.assign(e,p);e.append(...c);return e;};
const row=(q,[k,v])=>{
  const url=q?v:k,label=q?k:(v||k);
  const r=el("div",{className:"r"});
  r.dataset.text=(k+" "+v).toLowerCase();
  const view=()=>{r.replaceChildren(
    el("div",{className:"main"},el("a",{href:/^(https?|file|about):/.test(url)?url:"#",textContent:label}),el("small",{textContent:q?url:(v?k:"")})),
    el("button",{textContent:"Edit",onclick:edit}),
    el("button",{className:"del",textContent:"Delete",onclick:()=>send({a:"del",q,k})}));};
  const edit=()=>{
    const a=el("input",{className:q?"k":"v",value:k}),b=el("input",{className:"v",value:v});
    const f=el("form",{onsubmit:(e)=>{e.preventDefault();send({a:"set",q,old:k,k:a.value,v:b.value});}},a,b,el("button",{textContent:"Save"}),el("button",{type:"button",textContent:"Cancel",onclick:view}));
    r.replaceChildren(f);a.focus();};
  view();return r;};
const fill=(id,q,rows)=>{const l=document.getElementById(id);l.replaceChildren(...rows.map(x=>row(q,x)));if(!rows.length)l.append(el("p",{className:"empty",textContent:"None yet."}));};
fill("ql",true,D.quick);fill("bl",false,D.book.slice().reverse());
for(const[id,q]of[["qf",true],["bf",false]])document.getElementById(id).onsubmit=(e)=>{e.preventDefault();const f=e.target;send({a:"set",q,k:f.k.value,v:f.v.value});};
document.getElementById("filter").oninput=(e)=>{const w=e.target.value.toLowerCase().split(/\s+/).filter(Boolean);for(const r of document.querySelectorAll(".r"))r.hidden=!w.every(x=>r.dataset.text.includes(x));};
</script>"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_qutebrowser_files() {
        assert_eq!(
            parse("klee https://a/b\n\n# c\nmy mark https://x", true),
            [
                ("klee".into(), "https://a/b".into()),
                ("my mark".into(), "https://x".into())
            ] as [(String, String); 2]
        );
        assert_eq!(
            parse("https://g/app?q=1 Google Gemini\nhttps://bare", false),
            [
                ("https://g/app?q=1".into(), "Google Gemini".into()),
                ("https://bare".into(), String::new())
            ] as [(String, String); 2]
        );
    }

    #[test]
    fn filter_needs_every_word() {
        let rows = vec![
            ("https://gemini.google.com".into(), "Google Gemini".into()),
            ("https://x".into(), "Zoom".into()),
        ];
        assert_eq!(filter(rows.clone(), "gem GOOGLE").len(), 1);
        assert_eq!(filter(rows, "").len(), 2);
    }
}