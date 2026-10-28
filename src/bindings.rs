//! Default key bindings: one key per action, only commands that exist.
//! (mode, key, command). Users add/override with `bind()` in config.ts.

pub const CORE: &[(&str, &str, &str)] = &[
    // open / edit address
    ("normal", "o", "cmd-set-text -s :open"),
    ("normal", "O", "cmd-set-text -s :open -t"),
    ("normal", "go", "cmd-set-text :open {url}"),
    ("normal", "gO", "cmd-set-text :open -t {url}"),
    ("normal", ":", "cmd-set-text :"),
    ("normal", "pp", "open -- {clipboard}"),
    ("normal", "Pp", "open -t -- {clipboard}"),
    // page navigation
    ("normal", "H", "back"),
    ("normal", "L", "forward"),
    ("normal", "r", "reload"),
    ("normal", "R", "reload -f"),
    ("normal", "<Ctrl-s>", "stop"),
    // scrolling
    ("normal", "h", "scroll left"),
    ("normal", "j", "scroll down"),
    ("normal", "k", "scroll up"),
    ("normal", "l", "scroll right"),
    ("normal", "gg", "scroll-to-perc 0"),
    ("normal", "G", "scroll-to-perc"),
    ("normal", "<Ctrl-d>", "scroll-page 0 0.5"),
    ("normal", "<Ctrl-u>", "scroll-page 0 -0.5"),
    ("normal", "<Ctrl-f>", "scroll-page 0 1"),
    ("normal", "<Ctrl-b>", "scroll-page 0 -1"),
    // find in page
    ("normal", "/", "cmd-set-text /"),
    ("normal", "?", "cmd-set-text ?"),
    ("normal", "n", "search-next"),
    ("normal", "N", "search-prev"),
    // tabs
    ("normal", "J", "tab-next"),
    ("normal", "K", "tab-prev"),
    ("normal", "d", "tab-close"),
    ("normal", "u", "undo"),
    ("normal", "co", "tab-only"),
    ("normal", "gC", "tab-clone"),
    ("normal", "gJ", "tab-move +"),
    ("normal", "gK", "tab-move -"),
    ("normal", "<Ctrl-Tab>", "tab-focus last"),
    ("normal", "<Alt-m>", "tab-mute"),
    ("normal", "<Alt-1>", "tab-focus 1"),
    ("normal", "<Alt-2>", "tab-focus 2"),
    ("normal", "<Alt-3>", "tab-focus 3"),
    ("normal", "<Alt-4>", "tab-focus 4"),
    ("normal", "<Alt-5>", "tab-focus 5"),
    ("normal", "<Alt-6>", "tab-focus 6"),
    ("normal", "<Alt-7>", "tab-focus 7"),
    ("normal", "<Alt-8>", "tab-focus 8"),
    ("normal", "<Alt-9>", "tab-focus -1"),
    // hints
    ("normal", "f", "hint"),
    ("normal", "F", "hint all tab"),
    ("normal", ";b", "hint all tab-bg"),
    ("normal", ";r", "hint --rapid links tab-bg"),
    ("normal", ";y", "hint links yank"),
    ("normal", ";o", "hint links fill :open {hint-url}"),
    ("normal", "gi", "hint inputs --first"),
    // copy
    ("normal", "yy", "yank"),
    ("normal", "yt", "yank title"),
    ("normal", "yd", "yank domain"),
    ("normal", "ym", "yank inline [{title}]({url})"),
    // quickmarks / bookmarks
    ("normal", "m", "quickmark-save"),
    ("normal", "b", "cmd-set-text -s :quickmark-load"),
    ("normal", "B", "cmd-set-text -s :quickmark-load -t"),
    ("normal", "M", "bookmark-add"),
    ("normal", "gb", "cmd-set-text -s :bookmark-load"),
    ("normal", "gB", "cmd-set-text -s :bookmark-load -t"),
    ("normal", "Sb", "bookmarks"),
    ("normal", "Ss", "settings"),
    ("normal", "Sd", "downloads"),
    ("normal", "Sh", "history"),
    ("normal", "Sp", "password-fill"), // also fills a field the popup didn't detect
    ("normal", "<Ctrl-p>", "tab-pin"),
    ("normal", "gD", "tab-give"),
    // zoom
    ("normal", "+", "zoom-in"),
    ("normal", "-", "zoom-out"),
    ("normal", "=", "zoom"),
    // site toggles
    ("normal", "<Space>td", "dark-toggle"),
    ("normal", "<Space>th", "https-toggle"),
    ("normal", "<Space>po", "password-fill --otp"),
    // modes and the rest
    ("normal", "i", "mode-enter insert"),
    ("normal", "<Ctrl-v>", "mode-enter passthrough"),
    (
        "normal",
        "<Escape>",
        "clear-keychain ;; search ;; fullscreen --leave",
    ),
    ("normal", "gd", "download"),
    ("normal", "<F1>", "bindings"),
    ("normal", "<F11>", "fullscreen"),
    ("normal", "<F12>", "devtools"),
    ("normal", "<Ctrl-n>", "open -w"),
    ("normal", "<Ctrl-q>", "quit"),
    ("insert", "<Escape>", "mode-leave"),
    ("insert", "<Ctrl-Shift-v>", "paste"),
    ("normal", "<Ctrl-Shift-v>", "paste"),
    ("passthrough", "<Shift-Escape>", "mode-leave"),
];

/// CORE as the `bindings.default` setting value: {mode: {key: command}}.
pub fn default_setting() -> serde_json::Value {
    let mut out = serde_json::Map::new();
    for (mode, key, cmd) in CORE {
        out.entry(*mode).or_insert_with(|| serde_json::json!({}))[*key] = (*cmd).into();
    }
    out.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_is_valid_and_has_no_duplicates() {
        let mut seen = std::collections::HashSet::new();
        for (mode, key, cmd) in CORE {
            let seq = crate::keys::parse_keystring(key).unwrap();
            let canon: String = seq.iter().map(|k| k.to_string()).collect();
            assert!(seen.insert((*mode, canon)), "duplicate key {key}");
            for part in cmd.split(";;") {
                let name = part.split_whitespace().next().unwrap();
                assert!(
                    crate::commands::NAMES.contains(&name),
                    "unknown command {name} ({key})"
                );
            }
        }
        let actions: std::collections::HashSet<_> = CORE.iter().map(|(m, _, c)| (m, c)).collect();
        assert_eq!(actions.len(), CORE.len(), "two keys for one action");
    }
}
