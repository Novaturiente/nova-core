//! First-run page: shown once per profile (marker `<data>/welcomed`), and by `:help`.
//! Key basics, where settings live, and the `--import` command for each browser found.

use std::path::PathBuf;

fn marker() -> PathBuf {
    crate::paths::xdg_dir("XDG_DATA_HOME", ".local/share").join("welcomed")
}

/// True once: the first start of this profile.
pub fn first_run() -> bool {
    let m = marker();
    if m.exists() {
        return false;
    }
    let _ = std::fs::create_dir_all(m.parent().unwrap());
    let _ = crate::paths::write_atomic(&m, b"");
    true
}

pub fn page() -> String {
    let mut seen = Vec::new();
    let browsers: String = crate::import::found()
        .into_iter()
        .filter(|(n, _)| {
            let new = !seen.contains(n);
            seen.push(n);
            new
        })
        .map(|(n, _)| format!("<li><code>novabrowser --import {n}</code></li>"))
        .collect();
    let import = if browsers.is_empty() {
        "<p>No other browser profiles found. <code>novabrowser --import &lt;profile dir&gt;</code> imports from a path.</p>".to_string()
    } else {
        format!("<p>Found on this computer — run one in a terminal while novabrowser is closed:</p><ul>{browsers}</ul>")
    };
    let profile = crate::paths::profile()
        .map(|p| format!(" — profile {p}"))
        .unwrap_or_default();
    crate::theme::themed_page(
        PAGE.replace("{import}", &import)
            .replace("{profile}", &profile),
    )
}

const PAGE: &str = r##"<!doctype html><meta charset=utf-8><title>Welcome</title>
<style>
body{font:15px system-ui,sans-serif;max-width:44em;margin:2.5em auto;padding:0 1em;color-scheme:light dark;line-height:1.5}
h1{font-size:1.5em;font-weight:600}h2{font-size:1.1em;margin-top:1.6em}
table{border-collapse:collapse}td{padding:.2em 1em .2em 0;vertical-align:top}
kbd,code{font:.9em ui-monospace,monospace;padding:.1em .4em;border-radius:5px;background:color-mix(in srgb,currentColor 10%,transparent)}
</style>
<h1>Welcome to novabrowser{profile}</h1>
<p>Everything works from the keyboard; the mouse works too. Keys go to the page only in insert mode
(clicking or focusing a text field enters it, <kbd>Escape</kbd> leaves it).</p>
<h2>Basics</h2>
<table>
<tr><td><kbd>o</kbd> / <kbd>O</kbd></td><td>open an address or search, in this tab / a new tab</td></tr>
<tr><td><kbd>f</kbd> / <kbd>F</kbd></td><td>follow a link by its label, in this tab / a background tab</td></tr>
<tr><td><kbd>j</kbd> <kbd>k</kbd> <kbd>gg</kbd> <kbd>G</kbd></td><td>scroll down, up, to the top, to the end</td></tr>
<tr><td><kbd>H</kbd> / <kbd>L</kbd></td><td>back / forward</td></tr>
<tr><td><kbd>J</kbd> / <kbd>K</kbd></td><td>next / previous tab; <kbd>d</kbd> closes it, <kbd>u</kbd> brings it back</td></tr>
<tr><td><kbd>/</kbd></td><td>find in page (<kbd>n</kbd> / <kbd>N</kbd> for next / previous)</td></tr>
<tr><td><kbd>yy</kbd></td><td>copy the page address</td></tr>
<tr><td><kbd>:</kbd></td><td>run a command (Tab completes)</td></tr>
<tr><td><kbd>F1</kbd></td><td>every key binding; after a prefix like <kbd>g</kbd> a hint shows what can follow</td></tr>
</table>
<h2>Settings</h2>
<p><kbd>Ss</kbd> or <code>:settings</code> opens the settings page (theme, search engine, start page, dark mode,
blocking, password manager…). Everything else: <code>:set &lt;name&gt; &lt;value&gt;</code>, or
<code>~/.config/novabrowser/config.ts</code> for a config you keep in git. Downloads: <kbd>Sd</kbd>,
bookmarks: <kbd>Sb</kbd>.</p>
<h2>Passwords</h2>
<p>novabrowser uses your password manager's command-line tool (Bitwarden via <code>rbw</code> by default;
<code>bw</code> and <code>pass</code> work too). Focus a login field and pick a login from the list, or press
<kbd>Space</kbd> <kbd>p</kbd> <kbd>p</kbd>.</p>
<h2>Bring your history and bookmarks</h2>
{import}
<p>This page again: <code>:help</code>.</p>
"##;