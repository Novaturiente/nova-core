//! `config.ts`: TypeScript stripped by oxc, run in QuickJS with a `nova` module
//! (config.js). Values are checked against the qutebrowser schema (settings.rs).
//! A failed load keeps the previous config.

use crate::settings::SETTINGS;
use crate::urlmatch::UrlPattern;
use rquickjs::loader::{Loader, Resolver};
use rquickjs::{CatchResultExt, Context, Ctx, Function, Module, Object, Runtime};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, RwLock};

static DEFAULTS: LazyLock<HashMap<&'static str, Value>> = LazyLock::new(|| {
    let mut d: HashMap<&'static str, Value> = SETTINGS
        .iter()
        .map(|(name, _, _, default, _)| {
            (*name, serde_json::from_str(default).unwrap_or(Value::Null))
        })
        .collect();
    d.insert("bindings.default", crate::bindings::default_setting());
    d
});

#[derive(Default)]
pub struct Config {
    settings: HashMap<String, Value>,
    patterns: Vec<(UrlPattern, HashMap<String, Value>)>,
    /// (mode, key, command); None = unbind. In config order.
    pub binds: Vec<(String, String, Option<String>)>,
    /// `:set` / `:config-cycle` values, over config.ts until the next `:config-source`.
    runtime: RwLock<HashMap<String, Value>>,
    /// `:set -u <pattern>` values, newest last; checked before config.ts patterns.
    runtime_patterns: RwLock<Vec<(UrlPattern, String, Value)>>,
}

/// Values saved by `:set` / the settings page: <config>/autoconfig.json, as typed
/// strings ({name: value}); re-validated on load. config.ts wins over them.
static SAVED: LazyLock<RwLock<HashMap<String, Value>>> = LazyLock::new(Default::default);

/// This profile's file: `<config>/autoconfig.json`, or for a named profile
/// `<config>/profiles/<name>/autoconfig.json`.
fn autoconfig() -> PathBuf {
    match crate::paths::profile() {
        Some(name) => dir().join("profiles").join(name).join("autoconfig.json"),
        None => dir().join("autoconfig.json"),
    }
}

fn read_json(p: &Path) -> serde_json::Map<String, Value> {
    std::fs::read_to_string(p)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

/// Named profiles start from the default profile's saved values; their own win.
fn load_saved() {
    let mut raw = read_json(&dir().join("autoconfig.json"));
    if crate::paths::profile().is_some() {
        raw.extend(read_json(&autoconfig()));
    }
    let mut saved = HashMap::new();
    for (name, v) in raw {
        match v.as_str().map(|s| parse_value(&name, s)) {
            Some(Ok(v)) => {
                saved.insert(name, v);
            }
            _ => eprintln!("novabrowser: autoconfig.json: skipping {name}"),
        }
    }
    *SAVED.write().unwrap() = saved;
}

/// Save (or with None, forget) a setting in autoconfig.json and apply it.
pub fn save(name: &str, value: Option<&str>) -> Result<(), String> {
    if let Some(v) = value {
        parse_value(name, v)?;
    }
    let mut raw = read_json(&autoconfig());
    match value {
        Some(v) => raw.insert(name.into(), v.into()),
        None => raw.remove(name),
    };
    let _ = std::fs::create_dir_all(autoconfig().parent().unwrap());
    crate::paths::write_atomic(
        &autoconfig(),
        serde_json::to_string_pretty(&raw).unwrap().as_bytes(),
    )
    .map_err(|e| format!("autoconfig.json: {e}"))?;
    load_saved();
    get().runtime.write().unwrap().remove(name);
    Ok(())
}

impl Config {
    /// Effective global value: `:set`, config.ts, saved (`:set`/settings page),
    /// the theme preset (colours), then the built-in default.
    pub fn get(&self, name: &str) -> Value {
        if let Some(v) = self.runtime.read().unwrap().get(name) {
            return v.clone();
        }
        if let Some(v) = self.settings.get(name) {
            return v.clone();
        }
        if let Some(v) = SAVED.read().unwrap().get(name) {
            return v.clone();
        }
        if name != "theme" {
            if let Some(v) = crate::theme::preset(&self.str("theme"), name) {
                return v.into();
            }
        }
        DEFAULTS.get(name).cloned().unwrap_or(Value::Null)
    }

    /// Set in config.ts (so `:set`/the settings page can't change it for good)?
    pub fn in_config_ts(&self, name: &str) -> bool {
        self.settings.contains_key(name)
    }

    /// How many `colors.*` settings config.ts sets (they hide the theme preset).
    pub fn colors_in_config_ts(&self) -> usize {
        self.settings
            .keys()
            .filter(|k| k.starts_with("colors.") && !k.starts_with("colors.webpage."))
            .count()
    }

    /// Value for `url`: the last matching `pattern()` block that sets it wins.
    pub fn get_for(&self, name: &str, url: &str) -> Value {
        for (pat, n, v) in self.runtime_patterns.read().unwrap().iter().rev() {
            if n == name && pat.matches(url) {
                return v.clone();
            }
        }
        for (pat, vals) in self.patterns.iter().rev() {
            if let Some(v) = vals.get(name) {
                if pat.matches(url) {
                    return v.clone();
                }
            }
        }
        self.get(name)
    }

    pub fn str(&self, name: &str) -> String {
        match self.get(name) {
            Value::String(s) => s,
            Value::Null => String::new(),
            v => v.to_string(),
        }
    }

    pub fn bool(&self, name: &str) -> bool {
        self.get(name).as_bool().unwrap_or(false)
    }

    pub fn int(&self, name: &str) -> i64 {
        self.get(name).as_i64().unwrap_or(0)
    }

    /// List setting as strings (a single value counts as a one-item list).
    pub fn list(&self, name: &str) -> Vec<String> {
        match self.get(name) {
            Value::Array(a) => a
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect(),
            Value::String(s) => vec![s],
            _ => Vec::new(),
        }
    }

    /// `:set name value` (value parsed like the schema expects, e.g. `true`, `40%`).
    pub fn set_runtime(&self, name: &str, value: &str) -> Result<(), String> {
        let v = parse_value(name, value)?;
        self.runtime.write().unwrap().insert(name.to_string(), v);
        Ok(())
    }

    /// `:set -u <pattern> name value`: per-site value until the next `:config-source`.
    pub fn set_runtime_for(&self, pattern: &str, name: &str, value: &str) -> Result<(), String> {
        let pat = UrlPattern::parse(pattern).map_err(|e| format!("{pattern}: {}", e.0))?;
        let v = parse_value(name, value)?;
        let mut rp = self.runtime_patterns.write().unwrap();
        rp.retain(|(p, n, _)| !(n == name && p.to_string() == pattern));
        rp.push((pat, name.to_string(), v));
        Ok(())
    }
}

/// A `:set` value parsed the way the schema expects (e.g. `true`, `40%`, JSON lists).
fn parse_value(name: &str, value: &str) -> Result<Value, String> {
    {
        let (_, kind, none_ok, _, values) = SETTINGS
            .iter()
            .find(|s| s.0 == name)
            .ok_or_else(|| format!("unknown setting {name}"))?;
        let v = match *kind {
            "str" | "perc" | "listorvalue" => Value::String(value.to_string()),
            "boolask" if value == "ask" => Value::String(value.to_string()),
            _ => {
                serde_json::from_str(value).map_err(|_| format!("{name}: invalid value {value}"))?
            }
        };
        let ok = match (*kind, &v) {
            (_, Value::Null) => *none_ok,
            ("bool" | "boolask", Value::Bool(_)) => true,
            ("int", Value::Number(n)) => n.is_i64(),
            ("num", Value::Number(_)) => true,
            ("list", Value::Array(_)) | ("dict", Value::Object(_)) => true,
            ("str", Value::String(s)) => values.is_empty() || values.contains(&s.as_str()),
            ("perc" | "listorvalue" | "boolask", Value::String(_)) => true,
            _ => false,
        };
        if !ok {
            return Err(format!("{name}: invalid value {value}"));
        }
        Ok(v)
    }
}

impl Config {
    /// Does any `pattern()` block set `name` to `value`?
    pub fn any_pattern_value(&self, name: &str, value: &Value) -> bool {
        self.patterns
            .iter()
            .any(|(_, v)| v.get(name) == Some(value))
    }

    /// Settings put in a `pattern()` that are only applied globally.
    pub fn ignored_per_site(&self) -> Vec<String> {
        // settings read with get_for()
        const PER_SITE: &[&str] = &[
            "colors.webpage.darkmode.enabled",
            "content.blocking.enabled",
            "content.cookies.accept",
            "content.headers.custom",
            "content.headers.do_not_track",
            "tabs.hibernate_after",
            "tabs.freeze_after",
        ];
        let mut names: Vec<String> = self
            .patterns
            .iter()
            .flat_map(|(_, v)| v.keys())
            .filter(|n| !PER_SITE.contains(&n.as_str()) && !crate::permissions::is_setting(n))
            .cloned()
            .collect();
        names.sort();
        names.dedup();
        names
    }
}

static CURRENT: LazyLock<RwLock<Arc<Config>>> =
    LazyLock::new(|| RwLock::new(Arc::new(Config::default())));

/// The active config (defaults until `load` succeeds once).
pub fn get() -> Arc<Config> {
    CURRENT.read().unwrap().clone()
}

/// Path of config.ts: `$XDG_CONFIG_HOME/<app>/config.ts` (see `paths::set_app`).
pub fn path() -> PathBuf {
    crate::paths::xdg_root("XDG_CONFIG_HOME", ".config").join("config.ts")
}

/// `$XDG_CONFIG_HOME/<app>`.
pub fn dir() -> PathBuf {
    path().parent().unwrap().to_path_buf()
}

/// Load config.ts and make it active. A missing file = defaults. On error the
/// previous config stays and the error (with config line) is returned.
pub fn load() -> Result<(), String> {
    load_saved();
    let p = path();
    let cfg = if p.exists() {
        eval(&p)?
    } else {
        Config::default()
    };
    *CURRENT.write().unwrap() = Arc::new(cfg);
    Ok(())
}

/// TypeScript → JavaScript. Returns the code and, per generated line, the source line.
fn strip_ts(path: &Path, source: &str) -> Result<(String, Vec<u32>), String> {
    use oxc_allocator::Allocator;
    use oxc_codegen::{Codegen, CodegenOptions};
    use oxc_parser::Parser;
    use oxc_semantic::SemanticBuilder;
    use oxc_span::SourceType;
    use oxc_transformer::{TransformOptions, Transformer};

    let alloc = Allocator::default();
    let ty = SourceType::from_path(path)
        .unwrap_or_default()
        .with_typescript(true)
        .with_module(true);
    let parsed = Parser::new(&alloc, source, ty).parse();
    if let Some(e) = parsed.diagnostics.first() {
        let line = e
            .labels
            .first()
            .map(|l| line_of(source, l.offset() as usize));
        return Err(format!(
            "{}{}: {e}",
            path.display(),
            line.map(|l| format!(":{l}")).unwrap_or_default()
        ));
    }
    let mut program = parsed.program;
    let scoping = SemanticBuilder::new()
        .build(&program)
        .semantic
        .into_scoping();
    // keep unused value imports: config code may import files for their side effects
    let mut opts = TransformOptions::default();
    opts.typescript.only_remove_type_imports = true;
    let ret = Transformer::new(&alloc, path, &opts).build_with_scoping(scoping, &mut program);
    if let Some(e) = ret.diagnostics.first() {
        return Err(format!("{}: {e}", path.display()));
    }
    let out = Codegen::new()
        .with_options(CodegenOptions {
            source_map_path: Some(path.to_path_buf()),
            ..Default::default()
        })
        .build(&program);
    let n = out.code.lines().count() + 1;
    let mut lines = vec![0u32; n];
    if let Some(map) = out.map {
        for t in map.get_tokens() {
            let (dst, src) = (t.get_dst_line() as usize, t.get_src_line() + 1);
            if dst < n && lines[dst] == 0 {
                lines[dst] = src;
            }
        }
    }
    Ok((out.code, lines))
}

fn line_of(source: &str, offset: usize) -> usize {
    source[..offset.min(source.len())].matches('\n').count() + 1
}

/// Generated-line → source-line tables per module, for error messages.
type LineMaps = Arc<RwLock<HashMap<String, Vec<u32>>>>;

/// Relative imports from config.ts (`./theme.ts`), resolved against the importer.
struct FileResolver(Arc<RwLock<Option<String>>>);
impl Resolver for FileResolver {
    fn resolve<'js>(
        &mut self,
        _: &Ctx<'js>,
        base: &str,
        name: &str,
        _: Option<rquickjs::loader::ImportAttributes<'js>>,
    ) -> rquickjs::Result<String> {
        if name == "nova" {
            return Ok(name.into());
        }
        if !name.starts_with('.') {
            return Err(rquickjs::Error::new_resolving(base, name));
        }
        let dir = Path::new(base).parent().unwrap_or(Path::new("."));
        let path = dir.join(name);
        if !path.is_file() {
            // QuickJS drops resolver/loader errors from module evaluation; keep ours
            *self.0.write().unwrap() = Some(format!("{}: no such file", path.display()));
            return Err(rquickjs::Error::new_resolving(base, name));
        }
        Ok(path.to_string_lossy().into_owned())
    }
}

/// (line maps, last load error — QuickJS drops the loader's message).
struct FileLoader(LineMaps, Arc<RwLock<Option<String>>>);
impl Loader for FileLoader {
    fn load<'js>(
        &mut self,
        ctx: &Ctx<'js>,
        name: &str,
        _: Option<rquickjs::loader::ImportAttributes<'js>>,
    ) -> rquickjs::Result<Module<'js, rquickjs::module::Declared>> {
        if name == "nova" {
            return Module::declare(ctx.clone(), "nova", include_str!("config.js"));
        }
        let path = Path::new(name);
        let res = std::fs::read_to_string(path)
            .map_err(|e| format!("{name}: {e}"))
            .and_then(|src| strip_ts(path, &src));
        let (code, lines) = res.map_err(|e| {
            *self.1.write().unwrap() = Some(e.clone());
            rquickjs::Error::new_loading_message(name, e)
        })?;
        self.0.write().unwrap().insert(name.to_string(), lines);
        Module::declare(ctx.clone(), name, code)
    }
}

fn eval(path: &Path) -> Result<Config, String> {
    let src = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let (code, lines) = strip_ts(path, &src)?;
    let name = path.to_string_lossy().into_owned();
    let maps: LineMaps = Arc::new(RwLock::new(HashMap::from([(name.clone(), lines)])));
    let rt = Runtime::new().map_err(|e| e.to_string())?;
    let load_err = Arc::new(RwLock::new(None));
    rt.set_loader(
        FileResolver(load_err.clone()),
        FileLoader(maps.clone(), load_err.clone()),
    );
    let ctx = Context::full(&rt).map_err(|e| e.to_string())?;
    let config_dir = path
        .parent()
        .unwrap_or(Path::new("."))
        .to_string_lossy()
        .into_owned();
    let json = ctx.with(|ctx| -> Result<String, String> {
        install_api(&ctx, &config_dir).map_err(|e| e.to_string())?;
        let res = Module::evaluate(ctx.clone(), name.as_str(), code)
            .and_then(|p| p.finish::<()>())
            .catch(&ctx);
        // a failed import rejects the module promise without an error here
        if let Some(le) = load_err.read().unwrap().clone() {
            return Err(format!("{}: import failed: {le}", path.display()));
        }
        if let Err(e) = res {
            return Err(js_error(&e.to_string(), &maps));
        }
        ctx.eval::<String, _>("JSON.stringify(globalThis.__nova_out ?? {})")
            .map_err(|e| e.to_string())
    })?;
    parse_output(&json)
}

/// globalThis.__nova: what config.js needs from Rust.
fn install_api(ctx: &Ctx<'_>, config_dir: &str) -> rquickjs::Result<()> {
    let n = Object::new(ctx.clone())?;
    let schema: serde_json::Map<String, Value> = SETTINGS
        .iter()
        .map(|(name, kind, none_ok, _, values)| {
            let v = serde_json::json!([kind, none_ok, values, DEFAULTS[name]]);
            (name.to_string(), v)
        })
        .collect();
    n.set("schema", Value::Object(schema).to_string())?;
    n.set("configDir", config_dir)?;
    n.set("profile", crate::paths::profile().unwrap_or("default"))?;
    let dir = config_dir.to_string();
    n.set(
        "readLines",
        Function::new(ctx.clone(), move |p: String| -> Vec<String> {
            let p = if Path::new(&p).is_absolute() {
                PathBuf::from(&p)
            } else {
                Path::new(&dir).join(&p)
            };
            std::fs::read_to_string(p)
                .map(|s| {
                    s.lines()
                        .map(str::trim)
                        .filter(|l| !l.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default()
        })?,
    )?;
    n.set(
        "checkPattern",
        Function::new(ctx.clone(), |p: String| -> String {
            UrlPattern::parse(&p).err().map(|e| e.0).unwrap_or_default()
        })?,
    )?;
    n.set(
        "checkKey",
        Function::new(ctx.clone(), |k: String| -> String {
            crate::keys::parse_keystring(&k)
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default()
        })?,
    )?;
    ctx.globals().set("__nova", n)
}

/// Map `file:line` in a QuickJS error/stack to config.ts lines.
fn js_error(msg: &str, maps: &LineMaps) -> String {
    let maps = maps.read().unwrap();
    let mut out = String::new();
    for line in msg.lines() {
        let mut l = line.to_string();
        for (file, table) in maps.iter() {
            let tag = format!("{file}:");
            if let Some(i) = l.find(&tag) {
                let rest = &l[i + tag.len()..];
                let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
                if let Ok(gen) = digits.parse::<usize>() {
                    // stack lines are 1-based; find the nearest mapped line at or above
                    let src = (0..gen)
                        .rev()
                        .find_map(|g| table.get(g).copied().filter(|&s| s > 0))
                        .unwrap_or(0);
                    let after = &rest[digits.len()..];
                    let after = after
                        .strip_prefix(':')
                        .map(|a| a.trim_start_matches(|c: char| c.is_ascii_digit()))
                        .unwrap_or(after);
                    l = format!("{}{file}:{src}{after}", &l[..i]);
                }
            }
        }
        out.push_str(&l);
        out.push('\n');
    }
    // One status-bar line: `<file>:<line>: <message>` (first frame in a config file).
    let mut lines = out.lines();
    let first = lines.next().unwrap_or("").trim();
    let msg = first
        .split_once(": ")
        .filter(|(k, _)| k.ends_with("Error"))
        .map_or(first, |(_, m)| m);
    let at = lines
        .filter_map(|l| l.trim().strip_prefix("at "))
        .find_map(|l| maps.keys().find(|f| l.contains(f.as_str())).map(|_| l))
        .map(|l| {
            l.rsplit_once('(')
                .map_or(l, |(_, r)| r)
                .trim_end_matches(')')
        });
    match at {
        Some(at) => format!("{at}: {msg}"),
        None => msg.to_string(),
    }
}

fn parse_output(json: &str) -> Result<Config, String> {
    let out: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let obj = |v: &Value| -> HashMap<String, Value> {
        v.as_object()
            .map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default()
    };
    let mut patterns = Vec::new();
    for p in out["patterns"].as_array().into_iter().flatten() {
        let pat = UrlPattern::parse(p[0].as_str().unwrap_or("")).map_err(|e| e.0)?;
        patterns.push((pat, obj(&p[1])));
    }
    let binds = out["binds"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|b| {
            let s = |i: usize| b[i].as_str().unwrap_or("").to_string();
            (s(0), s(1), b[2].as_str().map(str::to_string))
        })
        .collect();
    Ok(Config {
        settings: obj(&out["settings"]),
        patterns,
        binds,
        runtime: Default::default(),
        runtime_patterns: Default::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn load_str(src: &str) -> Result<Config, String> {
        // tests run in parallel: one dir per call
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("nova-cfg-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("excludes"), "a.com\n\n b.org \n").unwrap();
        std::fs::write(
            dir.join("colors.ts"),
            "export const RED: string = \"#f00\";\n",
        )
        .unwrap();
        let p = dir.join("config.ts");
        std::fs::write(&p, src).unwrap();
        eval(&p)
    }

    #[test]
    fn loads_settings_patterns_binds() {
        let cfg = load_str(
            r#"import { c, bind, unbind, alias, pattern, readLines } from "nova";
import { RED } from "./colors.ts";
interface X { a: number }
const chars: string = "asdf";
c.hints.chars = chars;
c.colors.hints.bg = RED;
c.zoom.levels = ["50%", "100%"];
c.hints.selectors["frame"] = ["div"];
c.tabs.padding = { top: 5, bottom: 5, left: 8, right: 8 };
c.url.start_pages = "https://example.com";
for (const d of readLines("excludes"))
  pattern(`*://${d}/*`, (p) => { p.colors.webpage.darkmode.enabled = false; });
alias("q2", "close");
bind("<Space>tt", "tab-next");
unbind("<Ctrl-v>");
bind("<Alt-Backspace>", "mode-leave", "insert");
"#,
        )
        .unwrap();
        assert_eq!(cfg.str("hints.chars"), "asdf");
        assert_eq!(cfg.str("colors.hints.bg"), "#f00");
        assert_eq!(cfg.list("zoom.levels"), ["50%", "100%"]);
        assert_eq!(cfg.get("hints.selectors")["frame"][0], "div");
        assert!(
            cfg.get("hints.selectors")["links"].is_array(),
            "defaults kept when mutating a dict"
        );
        assert_eq!(cfg.list("url.start_pages"), ["https://example.com"]);
        assert!(!cfg
            .get_for("colors.webpage.darkmode.enabled", "https://a.com/x")
            .as_bool()
            .unwrap());
        assert_eq!(
            cfg.get_for("colors.webpage.darkmode.enabled", "https://b.org/"),
            false
        );
        assert_eq!(
            cfg.get_for("colors.webpage.darkmode.enabled", "https://c.net/"),
            DEFAULTS["colors.webpage.darkmode.enabled"]
        );
        assert_eq!(cfg.get("aliases")["q2"], "close");
        assert_eq!(cfg.get("aliases")["q"], "close", "default aliases kept");
        assert_eq!(
            cfg.binds[0],
            ("normal".into(), "<Space>tt".into(), Some("tab-next".into()))
        );
        assert_eq!(cfg.binds[1], ("normal".into(), "<Ctrl-v>".into(), None));
        assert_eq!(cfg.binds[2].0, "insert");
        assert_eq!(cfg.str("tabs.position"), "top", "default");
        assert!(cfg.ignored_per_site().is_empty());
    }

    #[test]
    fn errors_name_the_config_line() {
        let cases = [
            ("import { c } from \"nova\";\n\nc.hints.chras = \"x\";\n", "unknown setting hints.chras", ":3"),
            ("import { c } from \"nova\";\nconst n: number = 1;\n\nc.hints.uppercase = \"yes\";\n", "expected true or false", ":4"),
            ("import { c } from \"nova\";\nc.tabs.position = \"middle\";\n", "is not one of top, bottom, left, right", ":2"),
            ("import { c } from \"nova\";\nc.zoom.levels = \"x\";\n", "expected a list", ":2"),
            ("import { pattern } from \"nova\";\n\npattern(\"*://[bad/*\", (p) => {});\n", "pattern", ":3"),
            ("import { bind } from \"nova\";\nbind(\"<Hyper-x>\", \"x\");\n", "bind <Hyper-x>", ":2"),
            ("import { c } from \"nova\";\nc.hints.chars = ;\n", "config.ts:2", ""),
            ("import { x } from \"./missing.ts\";\n", "missing.ts", ""),
        ];
        for (src, want, line) in cases {
            let err = load_str(src)
                .err()
                .unwrap_or_else(|| panic!("no error for {src}"));
            assert!(err.contains(want), "{src:?} -> {err}");
            assert!(
                err.contains(&format!("config.ts{line}")),
                "{src:?} -> {err}"
            );
        }
    }

    #[test]
    fn removed_and_global_only_settings() {
        assert!(
            load_str("import { c } from \"nova\";\nc.content.pdfjs = true;\n")
                .err()
                .unwrap()
                .contains("unknown setting")
        );
        let cfg = load_str(
            "import { c, pattern } from \"nova\";\npattern(\"*://*.google.com/*\", (p) => { p.content.cookies.accept = \"all\"; p.hints.chars = \"ab\"; });\n",
        )
        .unwrap();
        assert_eq!(cfg.ignored_per_site(), ["hints.chars"]);
    }

    #[test]
    fn runtime_set_overrides_and_validates() {
        let cfg = Config::default();
        cfg.set_runtime("tabs.position", "left").unwrap();
        assert_eq!(cfg.str("tabs.position"), "left");
        assert!(cfg.set_runtime("tabs.position", "middle").is_err());
        cfg.set_runtime("content.blocking.enabled", "false")
            .unwrap();
        assert!(!cfg.bool("content.blocking.enabled"));
        assert!(cfg
            .set_runtime("content.blocking.enabled", "maybe")
            .is_err());
        assert!(cfg.set_runtime("nope", "1").is_err());
        let d = "colors.webpage.darkmode.enabled";
        cfg.set_runtime_for("*://a.com/*", d, "false").unwrap();
        cfg.set_runtime_for("*://a.com/*", d, "true").unwrap();
        assert_eq!(cfg.get_for(d, "https://a.com/x"), Value::Bool(true));
        assert_eq!(cfg.runtime_patterns.read().unwrap().len(), 1);
        assert_eq!(cfg.get_for(d, "https://b.com/"), cfg.get(d));
        assert!(cfg.set_runtime_for("::bad", d, "true").is_err());
    }
}
