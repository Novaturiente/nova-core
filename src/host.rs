//! The seam between shared code and the browser engine: each edition registers a
//! `Host` at startup (UI thread) and a `post` function that runs closures on the UI
//! thread from worker threads. Shared modules call the free functions below.
use std::cell::RefCell;
use std::sync::OnceLock;

pub trait Host {
    /// Status bar message: "info", "warning", "error".
    fn message(&self, level: &str, text: &str);
    /// After `:set` / `:config-source` / the settings page: re-apply config everywhere.
    fn config_changed(&self);
    /// New window with `url` (None = default page).
    fn open_window(&self, url: Option<String>);
    /// Completion rows JSON for the palette (see ui/palette.html), None = hide.
    fn show_completion(&self, json: Option<&str>);
    /// Replace the palette's text without leaving command mode.
    fn set_cmdline_text(&self, text: &str);
    /// Incremental find in the active tab (`/text`, `?text`).
    fn search(&self, text: &str, backward: bool);
    /// Every window's tabs in `session` shape, None before a window exists.
    fn session_json(&self) -> Option<serde_json::Value>;
    /// Open a saved session (see `session::shape`); `clear` closes current tabs.
    fn restore(&self, v: &serde_json::Value, clear: bool);
    /// Run `f` on the UI thread after `ms` milliseconds.
    fn later(&self, ms: u64, f: Box<dyn FnOnce()>);
    /// Run a command line in tab `tab` (edition's id); false if that tab is gone.
    fn run_in_tab(&self, tab: u64, line: &str) -> bool;
    /// Show `text` as plain text in a new tab (`spawn -o`).
    fn show_text(&self, text: &str);
}

thread_local! {
    static HOST: RefCell<Option<Box<dyn Host>>> = const { RefCell::new(None) };
}

type Post = fn(Box<dyn FnOnce() + Send>);
static POST: OnceLock<Post> = OnceLock::new();

/// Register the edition's host (UI thread) and its cross-thread `post`.
pub fn set(host: Box<dyn Host>, post: Post) {
    HOST.set(Some(host));
    let _ = POST.set(post);
}

/// Without a host (tests, CLI subcommands) calls are logged or dropped.
fn with<R>(f: impl FnOnce(&dyn Host) -> R) -> Option<R> {
    HOST.with_borrow(|h| h.as_deref().map(f))
}

pub fn message(level: &str, text: &str) {
    if with(|h| h.message(level, text)).is_none() {
        eprintln!("{}: {level}: {text}", crate::paths::app());
    }
}
pub fn config_changed() {
    with(|h| h.config_changed());
}
pub fn open_window(url: Option<String>) {
    with(|h| h.open_window(url));
}
pub fn show_completion(json: Option<&str>) {
    with(|h| h.show_completion(json));
}
pub fn set_cmdline_text(text: &str) {
    with(|h| h.set_cmdline_text(text));
}
pub fn search(text: &str, backward: bool) {
    with(|h| h.search(text, backward));
}
pub fn session_json() -> Option<serde_json::Value> {
    with(|h| h.session_json()).flatten()
}
pub fn restore(v: &serde_json::Value, clear: bool) {
    with(|h| h.restore(v, clear));
}
pub fn later(ms: u64, f: impl FnOnce() + 'static) {
    with(|h| h.later(ms, Box::new(f)));
}
pub fn run_in_tab(tab: u64, line: &str) -> bool {
    with(|h| h.run_in_tab(tab, line)).unwrap_or(false)
}
pub fn show_text(text: &str) {
    with(|h| h.show_text(text));
}

/// Run `f` on the UI thread (from a worker thread). Dropped before `set`.
pub fn on_ui(f: impl FnOnce() + Send + 'static) {
    if let Some(post) = POST.get() {
        post(Box::new(f));
    }
}

/// JS string literal; everything but ASCII alphanumerics and space as \uXXXX,
/// so page titles/URLs can't break out of the script we inject them into.
pub fn js_str(v: &str) -> String {
    let mut out = String::with_capacity(v.len() + 2);
    out.push('"');
    for u in v.encode_utf16() {
        match u {
            0x20 | 0x30..=0x39 | 0x41..=0x5a | 0x61..=0x7a => out.push(u as u8 as char),
            _ => out.push_str(&format!("\\u{u:04x}")),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::js_str;

    #[test]
    fn js_str_escapes_everything_risky() {
        assert_eq!(js_str("ab 09"), "\"ab 09\"");
        assert_eq!(
            js_str("\"</script>"),
            "\"\\u0022\\u003c\\u002fscript\\u003e\""
        );
        assert_eq!(js_str("\\\n\u{2028}é"), "\"\\u005c\\u000a\\u2028\\u00e9\"");
        assert_eq!(js_str("😀"), "\"\\ud83d\\ude00\"");
    }
}
