//! `:` command line: completion (commands, history) and command history.
//! The status bar `<input>` edits text natively and reports every change here.
use crate::{commands, config, history, marks};
use std::cell::RefCell;

#[derive(Default)]
struct State {
    /// Current command line text, `:` included.
    text: String,
    /// Text before the completed part, e.g. `:open -t `.
    prefix: String,
    title: &'static str,
    items: Vec<(String, String)>,
    /// Category of each item; a header row is drawn where it changes (empty = none).
    cats: Vec<&'static str>,
    /// Quickmark/bookmark rows for the pending `open` query, shown above history.
    marks: Vec<(String, String, &'static str)>,
    sel: Option<usize>,
    /// History query whose answer we still want.
    query: u64,
    /// Typed command lines, oldest first.
    hist: Vec<String>,
    /// While browsing history: (index into hist, text typed before browsing).
    hist_pos: Option<(usize, String)>,
}

thread_local! {
    static S: RefCell<State> = RefCell::new(State::default());
}

/// The text changed by typing: recompute completion, stop browsing history.
pub fn on_text(text: &str) {
    S.with_borrow_mut(|s| s.hist_pos = None);
    complete(text);
}

fn complete(text: &str) {
    let body = text.strip_prefix(':').unwrap_or(text);
    S.with_borrow_mut(|s| {
        s.text = text.to_string();
        s.sel = None;
        s.query = 0;
    });
    // search.incremental: search as you type `/text` or `?text`
    if let Some(t) = text.strip_prefix('/').or_else(|| text.strip_prefix('?')) {
        crate::host::search(t, text.starts_with('?'));
        return set(String::new(), "", Vec::new());
    }
    // a bare `:profile-open` already lists the profiles, no space needed
    let split = match body {
        "profile-open" => Some((body, "")),
        _ => body.split_once(' '),
    };
    match split {
        None => {
            let items = commands::NAMES
                .iter()
                .filter(|n| n.contains(body))
                .map(|n| (n.to_string(), String::new()))
                .collect();
            set(":".into(), "Commands", items);
        }
        Some((
            c @ ("quickmark-load" | "quickmark-del" | "bookmark-load" | "bookmark-del"),
            rest,
        )) => {
            let (arg, prefix) = strip_flags(text, rest);
            let (rows, title) = if c.starts_with("quick") {
                (marks::quickmarks(), "Quickmarks")
            } else {
                (marks::bookmarks(), "Bookmarks")
            };
            set(prefix, title, marks::filter(rows, arg));
        }
        Some(("profile-open", rest)) => {
            let rows = crate::profiles::list()
                .into_iter()
                .filter(|n| n.to_lowercase().contains(&rest.trim().to_lowercase()))
                .map(|n| {
                    (
                        format!(":profile-open {n}"),
                        if Some(n.as_str()) == crate::paths::profile().or(Some("default")) {
                            "this profile (new window)".into()
                        } else {
                            String::new()
                        },
                    )
                })
                .collect();
            set(String::new(), "Profiles", rows);
            render();
        }
        Some(("bindings", rest)) => {
            // yours first, then the defaults per mode; filter on key and command
            let words: Vec<String> = rest.split_whitespace().map(str::to_lowercase).collect();
            let mut rows: Vec<(&'static str, String, String)> = Vec::new();
            for user in [true, false] {
                for (mode, key, cmd, u) in crate::bindings::effective() {
                    let hay = format!("{key} {cmd} {mode}").to_lowercase();
                    if u == user && words.iter().all(|w| hay.contains(w)) {
                        let cat = match (user, mode.as_str()) {
                            (true, "normal") => "Your bindings",
                            (true, "insert") => "Your bindings · insert mode",
                            (true, _) => "Your bindings · passthrough mode",
                            (false, "normal") => "Normal mode",
                            (false, "insert") => "Insert mode",
                            (false, _) => "Passthrough mode",
                        };
                        rows.push((cat, key, cmd));
                    }
                }
            }
            // one block per category (stable: keeps the binding order inside each)
            let order = |c: &str| {
                [
                    "Your bindings",
                    "Your bindings · insert mode",
                    "Your bindings · passthrough mode",
                    "Normal mode",
                    "Insert mode",
                    "Passthrough mode",
                ]
                .iter()
                .position(|x| *x == c)
            };
            rows.sort_by_key(|r| order(r.0));
            let cats = rows.iter().map(|r| r.0).collect();
            set(
                String::new(),
                "",
                rows.into_iter().map(|(_, k, c)| (k, c)).collect(),
            );
            S.with_borrow_mut(|s| s.cats = cats);
            render();
        }
        Some(("open", rest)) => {
            // flags (-t, -b, …) stay in the prefix
            let (arg, prefix) = strip_flags(text, rest);
            // completion.open_categories order, history last (it answers asynchronously)
            let mut mk = Vec::new();
            for cat in config::get().list("completion.open_categories") {
                match cat.as_str() {
                    "quickmarks" => mk.extend(
                        marks::filter(marks::quickmarks(), arg)
                            .into_iter()
                            .map(|(n, u)| (u, n, "Quickmarks")),
                    ),
                    "bookmarks" => mk.extend(
                        marks::filter(marks::bookmarks(), arg)
                            .into_iter()
                            .map(|(u, t)| (u, t, "Bookmarks")),
                    ),
                    _ => {}
                }
            }
            let id = history::query(arg, mk.iter().map(|m| m.0.clone()).collect());
            S.with_borrow_mut(|s| {
                s.prefix = prefix;
                s.query = id;
                s.marks = mk;
            });
        }
        Some(_) => set(String::new(), "", Vec::new()),
    }
}

/// `rest` minus leading `-x` flags, and the command line text before the argument.
fn strip_flags<'a>(text: &str, rest: &'a str) -> (&'a str, String) {
    let mut arg = rest;
    while let Some((_, tail)) = arg.split_once(' ').filter(|(f, _)| f.starts_with('-')) {
        arg = tail;
    }
    (arg, text[..text.len() - arg.len()].to_string())
}

/// Answer from the history thread (`visits` = (typed, visits) per mark row); stale
/// answers are dropped.
pub fn history_results(id: u64, items: Vec<(String, String)>, visits: Vec<(i64, i64)>) {
    let Some((prefix, mk)) = S.with_borrow_mut(|s| {
        (s.query == id).then(|| (s.prefix.clone(), std::mem::take(&mut s.marks)))
    }) else {
        return;
    };
    let mk = by_visits(mk, visits);
    if mk.is_empty() {
        return set(prefix, "History", items);
    }
    // a bookmarked URL is listed once, in its bookmark section
    let marked: std::collections::HashSet<&str> = mk.iter().map(|m| m.0.as_str()).collect();
    let items: Vec<(String, String)> = items
        .into_iter()
        .filter(|(u, _)| !marked.contains(u.as_str()))
        .collect();
    let mut cats: Vec<&'static str> = mk.iter().map(|m| m.2).collect();
    let mut all: Vec<(String, String)> = mk.into_iter().map(|(a, b, _)| (a, b)).collect();
    cats.extend(std::iter::repeat_n("History", items.len()));
    all.extend(items);
    set(prefix, "", all);
    S.with_borrow_mut(|s| s.cats = cats);
    render();
}

/// Mark rows inside each category: most opened from the address bar first, then most
/// visited; categories kept in place. `visits` = (typed, visits) per row.
fn by_visits<T>(
    mk: Vec<(T, T, &'static str)>,
    visits: Vec<(i64, i64)>,
) -> Vec<(T, T, &'static str)> {
    let mut rows: Vec<_> = mk.into_iter().zip(visits).collect();
    for g in rows.chunk_by_mut(|a, b| a.0 .2 == b.0 .2) {
        g.sort_by_key(|r| std::cmp::Reverse(r.1)); // stable: ties keep file order
    }
    rows.into_iter().map(|r| r.0).collect()
}

fn set(prefix: String, title: &'static str, items: Vec<(String, String)>) {
    S.with_borrow_mut(|s| {
        s.prefix = prefix;
        s.title = title;
        s.items = items;
        s.cats.clear();
        s.sel = None;
    });
    render();
}

fn render() {
    let json = S.with_borrow(|s| {
        if s.items.is_empty() {
            return None;
        }
        let rows: Vec<String> = s
            .items
            .iter()
            .map(|(a, b)| format!("[{},{}]", crate::host::js_str(a), crate::host::js_str(b)))
            .collect();
        let cats: Vec<String> = s.cats.iter().map(|c| crate::host::js_str(c)).collect();
        Some(format!(
            "{{\"title\":{},\"items\":[{}],\"cats\":[{}],\"sel\":{}}}",
            crate::host::js_str(s.title),
            rows.join(","),
            cats.join(","),
            s.sel.map_or(-1, |i| i as i64)
        ))
    });
    crate::host::show_completion(json.as_deref());
}

/// Tab / Shift-Tab: select the next/previous item and put it on the command line.
pub fn select(by: i64) {
    let text = S.with_borrow_mut(|s| {
        let n = s.items.len() as i64;
        if n == 0 {
            return None;
        }
        let i = match s.sel {
            None if by > 0 => 0,
            None => n - 1,
            Some(i) => (i as i64 + by).rem_euclid(n),
        } as usize;
        s.sel = Some(i);
        s.text = format!("{}{}", s.prefix, s.items[i].0);
        Some(s.text.clone())
    });
    if let Some(t) = text {
        crate::host::set_cmdline_text(&t);
        render();
    }
}

pub fn set_cmd_history(cmds: Vec<String>) {
    S.with_borrow_mut(|s| s.hist = cmds);
}

/// A command line was run from `:`: remember it (skip immediate repeats).
pub fn record(line: &str) {
    let new = S.with_borrow_mut(|s| {
        let new = s.hist.last().map(String::as_str) != Some(line);
        if new {
            s.hist.push(line.to_string());
        }
        new
    });
    if new {
        history::cmd(line);
    }
}

/// Up / Down: older / newer command lines starting with what was typed (qutebrowser).
pub fn history_step(by: i64) {
    if let Some(t) = S.with_borrow_mut(|s| step(s, by)) {
        crate::host::set_cmdline_text(&t);
        complete(&t);
    }
}

/// The new command line text, or None at the oldest match.
fn step(s: &mut State, by: i64) -> Option<String> {
    let (pos, typed) = s.hist_pos.clone().unwrap_or((s.hist.len(), s.text.clone()));
    let mut i = pos as i64;
    loop {
        i += by;
        if i < 0 {
            return None;
        }
        if i as usize >= s.hist.len() {
            s.hist_pos = None;
            return Some(typed);
        }
        let h = &s.hist[i as usize];
        if h.starts_with(&typed) && *h != s.text {
            s.hist_pos = Some((i as usize, typed));
            return Some(h.clone());
        }
    }
}

/// Command line closed.
pub fn reset() {
    S.with_borrow_mut(|s| {
        s.items.clear();
        s.sel = None;
        s.query = 0;
        s.hist_pos = None;
    });
    crate::host::show_completion(None);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hist_test(hist: &[&str], typed: &str, steps: &[i64]) -> Vec<String> {
        let mut s = State {
            hist: hist.iter().map(|h| h.to_string()).collect(),
            text: typed.into(),
            ..Default::default()
        };
        steps
            .iter()
            .map(|&by| {
                if let Some(t) = step(&mut s, by) {
                    s.text = t;
                }
                s.text.clone()
            })
            .collect()
    }

    #[test]
    fn marks_sorted_by_visits_per_category() {
        let mk = vec![
            ("q1", "", "Quickmarks"),
            ("q2", "", "Quickmarks"),
            ("b1", "", "Bookmarks"),
            ("b2", "", "Bookmarks"),
            ("b3", "", "Bookmarks"),
        ];
        let got: Vec<_> = by_visits(mk, vec![(0, 1), (0, 5), (1, 0), (0, 9), (2, 1)])
            .into_iter()
            .map(|r| r.0)
            .collect();
        assert_eq!(got, ["q2", "q1", "b3", "b1", "b2"]);
    }

    #[test]
    fn history_prefix_navigation() {
        let h = [":open a", ":tab-close", ":open b", ":open b", ":zoom 50"];
        assert_eq!(
            hist_test(&h, ":open", &[-1, -1, -1, 1, 1]),
            [":open b", ":open a", ":open a", ":open b", ":open"]
        );
        assert_eq!(
            hist_test(&h, ":", &[-1, -1, -1]),
            [":zoom 50", ":open b", ":tab-close"]
        );
    }
}