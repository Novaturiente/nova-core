//! `c.colors.*`, `c.fonts.*` and bar sizes → CSS custom properties for the UI pages
//! (`nova.theme({vars})`), plus qutebrowser title formats.
use crate::config::Config;
use serde_json::{Map, Value};

/// Every colour and font setting as `--colors-tabs-bar-bg: …`, plus paddings and sizes.
pub fn vars(cfg: &Config) -> Map<String, Value> {
    let mut out = Map::new();
    for (name, kind, ..) in crate::settings::SETTINGS {
        let var = format!("--{}", name.replace(['.', '_'], "-"));
        if name.starts_with("colors.") && matches!(*kind, "str" | "listorvalue") {
            // list-valued colours (completion.fg) use their first entry
            let v = match cfg.get(name) {
                Value::Array(a) => a.first().and_then(Value::as_str).unwrap_or("").to_string(),
                v => v.as_str().unwrap_or("").to_string(),
            };
            out.insert(var, Value::String(css_color(&v)));
        } else if name.starts_with("fonts.")
            && !name.starts_with("fonts.web.")
            && !name.starts_with("fonts.default_")
        {
            out.insert(var, Value::String(css_font(cfg, &cfg.str(name))));
        }
    }
    for p in [
        "tabs.padding",
        "statusbar.padding",
        "tabs.indicator.padding",
    ] {
        out.insert(
            format!("--{}", p.replace(['.', '_'], "-")),
            Value::String(css_padding(&cfg.get(p))),
        );
    }
    let px = |n: i64| {
        if n < 0 {
            "none".to_string()
        } else {
            format!("{n}px")
        }
    };
    out.insert(
        "--tabs-min-width".into(),
        px(cfg.int("tabs.min_width")).into(),
    );
    out.insert(
        "--tabs-max-width".into(),
        px(cfg.int("tabs.max_width")).into(),
    );
    out.insert(
        "--tabs-indicator-width".into(),
        format!("{}px", cfg.int("tabs.indicator.width")).into(),
    );
    out.insert(
        "--tabs-title-alignment".into(),
        cfg.str("tabs.title.alignment").into(),
    );
    out.insert("--hints-border".into(), cfg.str("hints.border").into());
    out.insert(
        "--hints-radius".into(),
        format!("{}px", cfg.int("hints.radius")).into(),
    );
    out
}

/// Bar height in px: font line height + vertical padding (qutebrowser sizes bars the same way).
pub fn bar_height(cfg: &Config, font_setting: &str, padding_setting: &str) -> i32 {
    let font = resolve_font(cfg, &cfg.str(font_setting));
    let px = font_px(&font).unwrap_or(16.0);
    let pad = cfg.get(padding_setting);
    let v = |k: &str| pad[k].as_f64().unwrap_or(0.0);
    (px * 1.25).ceil() as i32 + (v("top") + v("bottom")) as i32
}

/// Pixel size of a font setting (16 when it has none).
pub fn setting_px(cfg: &Config, font_setting: &str) -> f64 {
    font_px(&resolve_font(cfg, &cfg.str(font_setting))).unwrap_or(16.0)
}

fn font_px(font: &str) -> Option<f64> {
    font.split_whitespace().find_map(|t| {
        if let Some(n) = t.strip_suffix("pt") {
            n.parse::<f64>().ok().map(|n| n * 96.0 / 72.0)
        } else {
            t.strip_suffix("px").and_then(|n| n.parse().ok())
        }
    })
}

/// Replace `default_size` / `default_family` (qutebrowser font placeholders).
fn resolve_font(cfg: &Config, font: &str) -> String {
    let families = cfg.list("fonts.default_family");
    let family = if families.is_empty() {
        "monospace".to_string()
    } else {
        families.join(", ")
    };
    font.replace("default_size", &cfg.str("fonts.default_size"))
        .replace("default_family", &family)
}

/// qutebrowser font string (`bold 12pt JetBrainsMono Nerd Font`) → CSS `font` shorthand.
pub fn css_font(cfg: &Config, font: &str) -> String {
    let font = resolve_font(cfg, font);
    let mut words = font.split_whitespace().peekable();
    let mut head = Vec::new();
    while let Some(w) = words.peek() {
        let lw = w.to_ascii_lowercase();
        let is_style = matches!(
            lw.as_str(),
            "bold" | "italic" | "oblique" | "normal" | "light" | "thin" | "medium"
        ) || lw
            .parse::<u32>()
            .is_ok_and(|n| n % 100 == 0 && n <= 900 && n > 0 && !lw.ends_with("pt"));
        let is_size = font_px(w).is_some();
        if !(is_style || is_size) {
            break;
        }
        head.push(match lw.as_str() {
            "light" => "300".to_string(),
            "thin" => "100".to_string(),
            "medium" => "500".to_string(),
            _ => w.to_string(),
        });
        let sized = is_size;
        words.next();
        if sized {
            break;
        }
    }
    if !head.iter().any(|w| font_px(w).is_some()) {
        head.push("10pt".into());
    }
    let family: Vec<&str> = words.collect();
    let family = family.join(" ");
    let family = family
        .split(',')
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .map(|f| {
            if f.contains(' ') && !f.starts_with('"') {
                format!("\"{f}\"")
            } else {
                f.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{} {}",
        head.join(" "),
        if family.is_empty() {
            "monospace".into()
        } else {
            family
        }
    )
}

/// Qt colours are mostly CSS already; `qlineargradient(… stop:0 a, stop:1 b)` isn't.
pub fn css_color(c: &str) -> String {
    let c = c.trim();
    if !c.starts_with("qlineargradient") {
        return c.to_string();
    }
    // shortcut: always top→bottom; qutebrowser's defaults only use vertical gradients
    let stops: Vec<String> = c
        .split("stop:")
        .skip(1)
        .map(|s| {
            let s = s
                .trim_start_matches(|ch: char| ch.is_ascii_digit() || ch == '.')
                .trim();
            let s = s.trim_end_matches(')').trim_end_matches(',').trim();
            // keep rgba(...) closed
            if s.starts_with("rgb") && !s.ends_with(')') {
                format!("{s})")
            } else {
                s.to_string()
            }
        })
        .collect();
    format!("linear-gradient(to bottom, {})", stops.join(", "))
}

fn css_padding(v: &Value) -> String {
    let n = |k: &str| v[k].as_i64().unwrap_or(0);
    format!(
        "{}px {}px {}px {}px",
        n("top"),
        n("right"),
        n("bottom"),
        n("left")
    )
}

/// Fields for qutebrowser title formats (`tabs.title.format`, `window.title_format`).
pub struct TitleFields<'a> {
    pub index: usize,
    pub title: &'a str,
    pub url: &'a str,
    pub progress: Option<f64>,
    pub muted: bool,
}

/// `{index}: {current_title}` etc.; unknown fields become empty.
pub fn format_title(fmt: &str, f: &TitleFields) -> String {
    let host = crate::commands::host(f.url);
    let protocol = f.url.split_once("://").map_or("", |(p, _)| p);
    let mut out = String::new();
    let mut rest = fmt;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        let Some(j) = rest[i..].find('}') else {
            out.push_str(&rest[i..]);
            return out;
        };
        let field = &rest[i + 1..i + j];
        let title = if f.title.is_empty() { f.url } else { f.title };
        out.push_str(&match field {
            "index" | "aligned_index" => f.index.to_string(),
            "current_title" => title.to_string(),
            "title_sep" => {
                if title.is_empty() {
                    String::new()
                } else {
                    " - ".into()
                }
            }
            "current_url" => f.url.to_string(),
            "host" => host.to_string(),
            "protocol" => protocol.to_string(),
            "perc" => f
                .progress
                .map(|p| format!("[{}%] ", (p * 100.0).round()))
                .unwrap_or_default(),
            "perc_raw" => f
                .progress
                .map(|p| format!("{}", (p * 100.0).round()))
                .unwrap_or_default(),
            // shortcut: audio state isn't tracked yet; only an explicit mute shows
            "audio" => {
                if f.muted {
                    "[M] ".into()
                } else {
                    String::new()
                }
            }
            _ => String::new(),
        });
        rest = &rest[i + j + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fonts_become_css() {
        let cfg = Config::default();
        cfg.set_runtime("fonts.default_family", "JetBrainsMonoNL Nerd Font")
            .unwrap();
        cfg.set_runtime("fonts.default_size", "11pt").unwrap();
        assert_eq!(
            css_font(&cfg, "bold 12pt JetBrainsMonoNL Nerd Font"),
            "bold 12pt \"JetBrainsMonoNL Nerd Font\""
        );
        assert_eq!(
            css_font(&cfg, "default_size default_family"),
            "11pt \"JetBrainsMonoNL Nerd Font\""
        );
        assert_eq!(css_font(&cfg, "Monospace"), "10pt Monospace");
        assert_eq!(css_font(&cfg, "light 9px a b, c"), "300 9px \"a b\", c");
        assert_eq!(
            bar_height(&cfg, "fonts.statusbar", "statusbar.padding"),
            19 + 2
        );
    }

    #[test]
    fn qt_gradients_become_css() {
        assert_eq!(css_color("#fff"), "#fff");
        assert_eq!(
            css_color("qlineargradient(x1:0, y1:0, x2:0, y2:1, stop:0 rgba(255, 247, 133, 0.8), stop:1 rgba(255, 197, 66, 0.8))"),
            "linear-gradient(to bottom, rgba(255, 247, 133, 0.8), rgba(255, 197, 66, 0.8))"
        );
    }

    #[test]
    fn title_formats() {
        let f = TitleFields {
            index: 3,
            title: "Rust",
            url: "https://rust-lang.org/x",
            progress: None,
            muted: false,
        };
        assert_eq!(
            format_title("{audio}{index}: {current_title}", &f),
            "3: Rust"
        );
        assert_eq!(
            format_title("{perc}{current_title}{title_sep}novabrowser", &f),
            "Rust - novabrowser"
        );
        assert_eq!(format_title("{host} {unknown}{", &f), "rust-lang.org {");
        let f = TitleFields {
            progress: Some(0.42),
            title: "",
            ..f
        };
        assert_eq!(
            format_title("{perc}{current_title}", &f),
            "[42%] https://rust-lang.org/x"
        );
    }
}

/// Built-in theme presets (setting `theme`): colours for every `colors.*` setting the
/// user doesn't set. (bg0 darkest .. bg3, fg, muted, border, accent, warm, green, red, on_accent)
const PRESETS: &[(&str, [&str; 12])] = &[
    (
        "dark",
        [
            "#141518", "#1c1d21", "#26282d", "#34373e", "#e4e5e7", "#9a9ca3", "#2e3036", "#7ab0f5",
            "#e8c170", "#8fcf8a", "#ec7b7b", "#141518",
        ],
    ),
    (
        "light",
        [
            "#e9ebef", "#f6f7f9", "#ffffff", "#d8e2f2", "#1f2328", "#5f6670", "#d0d6de", "#0b62d6",
            "#b07a00", "#1a7f37", "#cf222e", "#ffffff",
        ],
    ),
    (
        "tokyo-night",
        [
            "#16161e", "#1a1b26", "#24283b", "#2f3549", "#c0caf5", "#737aa2", "#292e42", "#7aa2f7",
            "#e0af68", "#9ece6a", "#f7768e", "#16161e",
        ],
    ),
    (
        "catppuccin",
        [
            "#181825", "#1e1e2e", "#313244", "#45475a", "#cdd6f4", "#a6adc8", "#45475a", "#cba6f7",
            "#f9e2af", "#a6e3a1", "#f38ba8", "#11111b",
        ],
    ),
];

/// CSS `color-scheme` for our own pages (settings, downloads, bookmarks).
pub fn scheme() -> &'static str {
    if crate::config::get().str("theme") == "light" {
        "light"
    } else {
        "dark"
    }
}

/// Shared look of our own pages (settings, history, bookmarks, downloads, welcome, crash):
/// cards, pill buttons and fields in the UI theme's colours (`--bg` … set by `themed_page`).
/// Added after the page's own style, so it wins over the older per-page rules.
const PAGE_CSS: &str = r#"
html{background:var(--bg)}
body{font:14px/1.45 system-ui,sans-serif;color:var(--fg);background:var(--bg);max-width:60em;margin:0 auto;padding:0 1.5em 3em}
a{color:var(--accent)}
h1{position:sticky;top:0;z-index:2;margin:0 -1.5em;padding:1.1em 1.5em .9em;font-size:1.45em;font-weight:650;
  display:flex;gap:.8em;align-items:center;flex-wrap:wrap;background:color-mix(in srgb,var(--bg) 88%,transparent);
  backdrop-filter:blur(12px);border-bottom:1px solid var(--border)}
h2{font-size:.78em;font-weight:650;letter-spacing:.08em;text-transform:uppercase;color:var(--muted);opacity:1;margin:1.8em 0 .6em .3em}
p.note,.note,.hint,.empty{color:var(--muted);opacity:1}
small{color:var(--muted)}
input,select{font:inherit;color:var(--fg);background:var(--surface);border:1px solid var(--border);border-radius:12px;
  padding:.5em .9em;outline:none;transition:border-color .12s,box-shadow .12s}
input:focus,select:focus{border-color:var(--accent);box-shadow:0 0 0 3px color-mix(in srgb,var(--accent) 25%,transparent)}
input::placeholder{color:var(--muted)}
option{background:var(--surface);color:var(--fg)}
h1 input{flex:1;max-width:22em;font-size:.6em;border-radius:999px;padding:.6em 1.1em}
h1 button{font-size:.6em}
button{font:inherit;font-size:.92em;color:var(--fg);background:var(--surface2);border:1px solid var(--border);border-radius:999px;
  padding:.4em 1.1em;cursor:pointer;transition:background .12s,border-color .12s,color .12s}
button:hover{border-color:color-mix(in srgb,var(--accent) 50%,var(--border))}
button.del{color:var(--red);background:color-mix(in srgb,var(--red) 10%,var(--surface))}
button.del:hover{background:color-mix(in srgb,var(--red) 22%,var(--surface));border-color:color-mix(in srgb,var(--red) 50%,transparent)}
form button:not([type=button]),button.primary{background:var(--accent);color:var(--on-accent);border-color:transparent;font-weight:600}
form button:not([type=button]):hover,button.primary:hover{filter:brightness(1.1)}
.r,.d,form{background:var(--surface);border:1px solid var(--border);border-radius:16px;padding:.7em 1em;margin:.5em 0;
  transition:border-color .12s,background .12s}
.r:hover,.d:hover{border-color:color-mix(in srgb,var(--accent) 40%,var(--border))}
form{background:color-mix(in srgb,var(--surface) 60%,var(--bg));border-style:dashed}
.r form{background:none;border:0;padding:0;margin:0;flex:1}
.r a{color:var(--fg)}
input[type=checkbox]{appearance:none;-webkit-appearance:none;flex:0 0 auto;width:2.6em;height:1.5em;padding:0;margin:0;border-radius:999px;
  position:relative;cursor:pointer;background:var(--surface2);transform:none}
input[type=checkbox]::before{content:"";position:absolute;top:.17em;left:.2em;width:1.05em;height:1.05em;border-radius:50%;
  background:var(--muted);transition:left .15s,background .15s}
input[type=checkbox]:checked{background:var(--accent);border-color:transparent}
input[type=checkbox]:checked::before{left:1.3em;background:var(--on-accent)}
kbd,code{font:.88em ui-monospace,monospace;padding:.15em .5em;border-radius:7px;background:var(--surface2);border:1px solid var(--border)}
table{border-collapse:separate;border-spacing:0 .35em}td{padding:.25em 1em .25em 0}
::-webkit-scrollbar{width:10px}::-webkit-scrollbar-thumb{background:var(--surface2);border-radius:999px;border:2px solid var(--bg)}
"#;

/// Our own page's HTML with its colour scheme and the shared look following the UI theme.
pub fn themed_page(html: String) -> String {
    // on :root, not body: text and canvas colours resolve from the root element
    let s = scheme();
    let cfg = crate::config::get();
    let c = |n: &str| css_color(&cfg.str(n));
    let vars = format!(
        ":root{{color-scheme:{s};--bg:{};--surface:{};--surface2:{};--border:{};--fg:{};--muted:{};--accent:{};--on-accent:{};--red:{};--green:{};--warm:{}}}",
        c("colors.completion.even.bg"),
        c("colors.tooltip.bg"),
        c("colors.completion.item.selected.bg"),
        c("colors.completion.category.border.top"),
        c("colors.completion.fg"),
        c("colors.tabs.odd.fg"),
        c("colors.tabs.indicator.start"),
        c("colors.downloads.start.fg"),
        c("colors.messages.error.bg"),
        c("colors.downloads.stop.bg"),
        c("colors.messages.warning.bg"),
    );
    html.replace("color-scheme:light dark", &format!("color-scheme:{s}"))
        .replacen("<style>", &format!("<style>{vars}"), 1)
        .replacen("</style>", &format!("</style><style>{PAGE_CSS}</style>"), 1)
}

/// The preset's value for colour setting `name` (None = not a themed colour).
pub fn preset(theme: &str, name: &str) -> Option<&'static str> {
    let p = &PRESETS
        .iter()
        .find(|p| p.0 == theme)
        .unwrap_or(&PRESETS[0])
        .1;
    let [bg0, bg1, bg2, bg3, fg, muted, border, accent, warm, green, red, on] = *p;
    Some(match name.strip_prefix("colors.")? {
        "tabs.bar.bg" | "tabs.odd.bg" | "tabs.even.bg" => bg0,
        "tabs.odd.fg" | "tabs.even.fg" => muted,
        "tabs.selected.odd.bg" | "tabs.selected.even.bg" => bg2,
        "tabs.selected.odd.fg" | "tabs.selected.even.fg" => fg,
        "tabs.indicator.start" | "statusbar.progress.bg" => accent,
        "tabs.indicator.stop" => green,
        "tabs.indicator.error" => red,
        "completion.fg"
        | "keyhint.fg"
        | "tooltip.fg"
        | "contextmenu.menu.fg"
        | "prompts.fg"
        | "statusbar.normal.fg"
        | "statusbar.command.fg"
        | "messages.info.fg"
        | "completion.item.selected.fg"
        | "contextmenu.selected.fg" => fg,
        "completion.even.bg"
        | "keyhint.bg"
        | "contextmenu.menu.bg"
        | "statusbar.normal.bg"
        | "statusbar.command.bg" => bg1,
        "tooltip.bg" | "prompts.bg" | "messages.info.bg" => bg2,
        "completion.item.selected.bg" | "contextmenu.selected.bg" => bg3,
        "completion.category.border.top" | "prompts.border" => border,
        "completion.category.fg" | "keyhint.suffix.fg" | "hints.match.fg" => accent,
        "hints.bg" | "statusbar.passthrough.bg" | "messages.warning.bg" => warm,
        "statusbar.insert.bg" | "downloads.stop.bg" => green,
        "downloads.start.bg" => accent,
        "messages.error.bg" | "downloads.error.bg" => red,
        "hints.fg"
        | "statusbar.insert.fg"
        | "statusbar.passthrough.fg"
        | "messages.warning.fg"
        | "messages.error.fg"
        | "downloads.start.fg"
        | "downloads.stop.fg"
        | "downloads.error.fg" => on,
        "webpage.bg" => {
            if theme == "light" {
                "#ffffff"
            } else {
                bg1
            }
        }
        _ => return None,
    })
}

#[cfg(test)]
mod preset_tests {
    #[test]
    fn every_themed_colour_setting_is_covered() {
        let themes = crate::settings::SETTINGS
            .iter()
            .find(|s| s.0 == "theme")
            .unwrap()
            .4;
        assert!(themes
            .iter()
            .all(|t| super::PRESETS.iter().any(|p| p.0 == *t)));
        for (name, kind, ..) in crate::settings::SETTINGS {
            let themed = name.starts_with("colors.")
                && matches!(*kind, "str" | "listorvalue")
                && !name.starts_with("colors.webpage.darkmode")
                && *name != "colors.webpage.preferred_color_scheme";
            for t in themes {
                assert_eq!(super::preset(t, name).is_some(), themed, "{t} {name}");
            }
        }
    }
}
