//! `:settings`: the common settings as a form. Changes are saved to
//! autoconfig.json (commands::set_user); ones config.ts sets are shown read-only.

use crate::config;

/// (setting, label). Kind and allowed values come from the schema.
const SHOWN: &[(&str, &str)] = &[
    ("theme", "Theme"),
    ("search", "Search engine URL ({} = query)"),
    ("url.start_pages", "Start page"),
    ("url.default_page", "New tab page"),
    ("tabs.position", "Tab bar position"),
    ("tabs.show", "Show tab bar"),
    ("colors.webpage.darkmode.enabled", "Dark mode for web pages"),
    (
        "colors.webpage.preferred_color_scheme",
        "Preferred colour scheme for sites",
    ),
    ("content.blocking.enabled", "Block ads and trackers"),
    ("content.cookies.accept", "Cookies"),
    ("content.autoplay", "Autoplay media"),
    ("downloads.location.directory", "Download folder"),
    ("auto_save.session", "Restore tabs on start"),
    ("fonts.default_family", "UI font"),
    ("fonts.default_size", "UI font size"),
    ("hints.chars", "Hint characters"),
    ("passwords.backend", "Password manager (rbw = Bitwarden)"),
];

/// Display string for a value (what goes back through `:set` parsing).
fn shown(name: &str, cfg: &config::Config) -> String {
    let v = if name == "search" {
        cfg.get("url.searchengines")["DEFAULT"].clone()
    } else {
        cfg.get(name)
    };
    match v {
        serde_json::Value::String(s) => s,
        serde_json::Value::Array(a) if a.len() == 1 && a[0].is_string() => {
            a[0].as_str().unwrap().into()
        }
        serde_json::Value::Null => String::new(),
        v => v.to_string(),
    }
}

pub fn page() -> String {
    let cfg = config::get();
    let rows: Vec<serde_json::Value> = SHOWN
        .iter()
        .map(|(name, label)| {
            let schema = crate::settings::SETTINGS.iter().find(|s| s.0 == *name);
            let (kind, values) = match (*name, schema) {
                ("search", _) => ("str", vec![]),
                (_, Some(s)) => (s.1, s.4.to_vec()),
                _ => ("str", vec![]),
            };
            let locked = cfg.in_config_ts(if *name == "search" {
                "url.searchengines"
            } else {
                name
            });
            let n = if *name == "theme" {
                cfg.colors_in_config_ts()
            } else {
                0
            };
            let note = if n > 0 {
                format!("config.ts sets {n} colours itself, they override the theme")
            } else {
                String::new()
            };
            serde_json::json!({ "name": name, "label": label, "kind": kind, "values": values,
                                "value": shown(name, &cfg), "locked": locked, "note": note })
        })
        .collect();
    let data = serde_json::json!({ "token": crate::paths::page_token(), "rows": rows, "profile": crate::paths::profile() });
    crate::theme::themed_page(
        PAGE.replace(
            "/*DATA*/{}",
            &serde_json::to_string(&data)
                .unwrap()
                .replace('<', "\\u003c"),
        ),
    )
}

/// {"a": "set", "k": name, "v": value} | {"a": "reset", "k": name}
pub fn on_page(msg: &serde_json::Value) -> Result<(), String> {
    let name = msg["k"].as_str().unwrap_or("");
    if !SHOWN.iter().any(|s| s.0 == name) {
        return Err(format!("not a settings-page setting: {name}"));
    }
    let v = msg["v"].as_str().unwrap_or("").trim();
    let (name, value) = match name {
        "search" => {
            if !v.contains("{}") {
                return Err("search URL needs {} where the query goes".into());
            }
            let mut engines = config::get().get("url.searchengines");
            engines["DEFAULT"] = v.into();
            ("url.searchengines", engines.to_string())
        }
        "url.start_pages" => (name, serde_json::json!([v]).to_string()),
        _ => (name, v.to_string()),
    };
    match msg["a"].as_str() {
        Some("set") => crate::commands::set_user(name, &value, false)?,
        Some("reset") => config::save(name, None)?,
        _ => return Err("unknown page action".into()),
    }
    crate::host::config_changed();
    Ok(())
}

const PAGE: &str = r##"<!doctype html><meta charset=utf-8><title>Settings</title>
<style>
body{font:14px system-ui,sans-serif;max-width:46em;margin:2em auto;padding:0 1em;color-scheme:light dark}
h1{font-size:1.3em;font-weight:600}p.note{opacity:.65}
.r{display:flex;gap:1em;align-items:center;padding:.6em .8em;border-radius:10px;margin:.35em 0;background:color-mix(in srgb,currentColor 6%,transparent)}
.r label{flex:1}.r small{display:block;opacity:.55;font-size:.8em}
input,select{font:inherit;padding:.3em .6em;border-radius:8px;border:1px solid color-mix(in srgb,currentColor 25%,transparent);background:Canvas;color:CanvasText;width:18em}
option{background:Canvas;color:CanvasText}
.warn{color:#e5a50a;opacity:1}
input[type=checkbox]{width:auto;transform:scale(1.3)}
button{font:inherit;padding:.25em .7em;border-radius:8px;border:0;cursor:pointer;background:color-mix(in srgb,currentColor 12%,transparent);color:inherit}
.locked{opacity:.55}
</style>
<h1>Settings</h1>
<p class=note>Saved right away. Settings set in config.ts are shown greyed out — change them there. Everything else: <code>:set</code> or config.ts.</p>
<div id=rows></div>
<script>
const D=/*DATA*/{};
if(D.profile){document.querySelector("h1").textContent="Settings — profile "+D.profile;document.querySelector("p.note").append(" Changes here apply to this profile only; unchanged ones follow the default profile.");}
const send=(m)=>window.nova&&nova.send("page "+D.token+" "+JSON.stringify({...m,page:"settings"}));
const el=(t,p,...c)=>{const e=document.createElement(t);Object.assign(e,p);e.append(...c);return e;};
for(const r of D.rows){
  const set=(v)=>send({a:"set",k:r.name,v:String(v)});
  let input;
  if(r.kind==="bool") input=el("input",{type:"checkbox",checked:r.value==="true",onchange:(e)=>set(e.target.checked)});
  else if(r.values.length) input=el("select",{onchange:(e)=>set(e.target.value)},...r.values.map(v=>el("option",{value:v,textContent:v,selected:v===r.value})));
  else input=el("input",{value:r.value,onchange:(e)=>set(e.target.value)});
  input.disabled=r.locked;
  const row=el("div",{className:"r"+(r.locked?" locked":"")},el("label",{textContent:r.label},el("small",{textContent:r.name+(r.locked?" · set in config.ts":"")}),...(r.note?[el("small",{className:"warn",textContent:r.note})]:[])),input);
  if(!r.locked) row.append(el("button",{textContent:"Reset",title:"back to the default",onclick:()=>send({a:"reset",k:r.name})}));
  document.getElementById("rows").append(row);
}
</script>"##;