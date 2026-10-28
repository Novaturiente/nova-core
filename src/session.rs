//! Sessions: `<data>/sessions/<name>.json` = {tabs: [{url, title}], active, closed}.
//! The default session autosaves (debounced) while `auto_save.session` is on and
//! is restored at startup; restored tabs load on first focus (window.rs: CEF
//! creates a tab's browser when its view first joins the window).
//! Each tab also keeps its back/forward URLs and scroll position (window.rs `go`).
use crate::{config, host, paths::xdg_dir};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;

const DEBOUNCE_MS: u64 = 1000;

thread_local! {
    static PENDING: Cell<bool> = const { Cell::new(false) };
    /// Last JSON written to the default session; unchanged state isn't rewritten.
    static LAST: RefCell<String> = const { RefCell::new(String::new()) };
}

fn path(name: &str) -> Result<PathBuf, String> {
    if name.is_empty() || name.starts_with('.') || name.contains('/') {
        return Err(format!("Invalid session name: {name}"));
    }
    Ok(xdg_dir("XDG_DATA_HOME", ".local/share")
        .join("sessions")
        .join(format!("{name}.json")))
}

fn default_name() -> String {
    let n = config::get().str("session.default_name");
    if n.is_empty() {
        "default".into()
    } else {
        n
    }
}

pub fn write(name: &str, v: &Value) -> Result<String, String> {
    let p = path(name)?;
    let text = serde_json::to_string_pretty(v).unwrap();
    std::fs::create_dir_all(p.parent().unwrap())
        .and_then(|_| crate::paths::write_atomic(&p, text.as_bytes()))
        .map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(text)
}

pub fn read(name: &str) -> Result<Value, String> {
    let p = path(name)?;
    let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))
}

/// Tabs changed (called from the edition's window refresh): save the default session soon.
pub fn dirty() {
    if !PENDING.get() && config::get().bool("auto_save.session") {
        PENDING.set(true);
        host::later(DEBOUNCE_MS, || {
            PENDING.set(false);
            save_now();
        });
    }
}

/// Write the default session now if autosave is on and something changed.
pub fn save_now() {
    if !config::get().bool("auto_save.session") {
        return;
    }
    let Some(v) = crate::host::session_json() else {
        return;
    };
    let text = serde_json::to_string_pretty(&v).unwrap();
    if LAST.with_borrow(|l| *l == text) {
        return;
    }
    match write(&default_name(), &v) {
        Ok(t) => LAST.set(t),
        Err(e) => eprintln!("novabrowser: session: {e}"),
    }
}

/// `:session-save [name]`.
pub fn save(name: Option<&str>) -> Result<(), String> {
    let name = name.map_or_else(default_name, str::to_string);
    let v = crate::host::session_json().ok_or("no window")?;
    write(&name, &v)?;
    crate::host::message("info", &format!("Saved session {name}."));
    Ok(())
}

/// `:session-load [--clear] name`: open its tabs (closing the current ones with --clear).
pub fn load(name: &str, clear: bool) -> Result<(), String> {
    let v = read(name)?;
    crate::host::restore(&v, clear);
    Ok(())
}

pub fn delete(name: &str) -> Result<(), String> {
    let p = path(name)?;
    std::fs::remove_file(&p).map_err(|e| format!("{}: {e}", p.display()))
}

/// Startup: the default session if autosave is on and it has tabs.
pub fn startup() -> Option<Value> {
    if !config::get().bool("auto_save.session") {
        return None;
    }
    let v = read(&default_name()).ok()?;
    let has_tabs = v["tabs"].as_array().is_some_and(|t| !t.is_empty())
        || v["windows"][0]["tabs"]
            .as_array()
            .is_some_and(|t| !t.is_empty());
    has_tabs.then(|| {
        LAST.set(serde_json::to_string_pretty(&v).unwrap());
        v
    })
}

/// The JSON shape window.rs writes; kept here so both sides agree.
/// {windows: [{tabs: [{url, title, back, fwd, scroll}], active}], closed}
pub fn shape(windows: Vec<Value>, closed: &[String]) -> Value {
    json!({ "windows": windows, "closed": closed })
}

/// tabs: {url, title, back (oldest first), fwd (nearest first), scroll}
pub fn window(tabs: Vec<Value>, active: usize) -> Value {
    json!({ "tabs": tabs, "active": active })
}

#[cfg(test)]
mod tests {
    #[test]
    fn names_cannot_escape_the_sessions_dir() {
        assert!(super::path("../x").is_err());
        assert!(super::path(".hidden").is_err());
        assert!(super::path("").is_err());
        assert!(super::path("work").unwrap().ends_with("sessions/work.json"));
    }
}