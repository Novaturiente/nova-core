// The `nova` module config.ts imports. Records settings, patterns and bindings
// into globalThis.__nova_out; config.rs reads it back as JSON after the run.
// Errors are thrown here so their stack points at the config line.
const N = globalThis.__nova;
const schema = JSON.parse(N.schema); // name -> [kind, noneOk, values, default]
const out = { settings: {}, patterns: [], binds: [] };
globalThis.__nova_out = out;

const prefixes = new Set();
for (const name in schema) {
  const parts = name.split(".");
  for (let i = 1; i < parts.length; i++) prefixes.add(parts.slice(0, i).join("."));
}

const MODES = ["normal", "insert", "hint", "passthrough", "command", "prompt", "yesno", "caret", "register"];

function show(v) {
  return typeof v === "string" ? JSON.stringify(v) : Array.isArray(v) ? "a list" : v === null ? "null" : typeof v;
}

function check(name, v) {
  const [kind, noneOk, values] = schema[name];
  if (v === null || v === undefined) {
    if (noneOk) return;
    throw new TypeError(`${name}: may not be null`);
  }
  const t = Array.isArray(v) ? "array" : typeof v;
  const want = {
    bool: t === "boolean" || "true or false",
    boolask: t === "boolean" || v === "ask" || 'true, false or "ask"',
    int: Number.isInteger(v) || "an integer",
    num: t === "number" || "a number",
    list: t === "array" || "a list",
    listorvalue: true,
    dict: t === "object" || "an object",
    perc: t === "number" || (t === "string" && /^\d+(\.\d+)?%$/.test(v)) || 'a number or "N%"',
  }[kind] ?? (t === "string" || "a string");
  if (want !== true) throw new TypeError(`${name}: expected ${want}, got ${show(v)}`);
  if (kind === "str" && values.length && !values.includes(v))
    throw new TypeError(`${name}: ${show(v)} is not one of ${values.join(", ")}`);
}

const copy = (v) => (v !== null && typeof v === "object" ? JSON.parse(JSON.stringify(v)) : v);

// c.a.b.c: a proxy per prefix; reading a dict/list setting returns a mutable copy
// stored in `store`, so `c.hints.selectors["frame"] = [...]` sticks.
function node(store, prefix, fallback) {
  return new Proxy({}, {
    get(_, key) {
      if (typeof key !== "string") return undefined;
      const name = prefix ? `${prefix}.${key}` : key;
      if (name in schema) {
        if (!(name in store)) {
          const v = fallback && name in fallback ? fallback[name] : schema[name][3];
          if (v === null || typeof v !== "object") return v;
          store[name] = copy(v);
        }
        return store[name];
      }
      if (prefixes.has(name)) return node(store, name, fallback);
      throw new Error(`unknown setting ${name}`);
    },
    set(_, key, v) {
      const name = prefix ? `${prefix}.${key}` : String(key);
      if (!(name in schema)) throw new Error(`unknown setting ${name}`);
      check(name, v);
      store[name] = v;
      return true;
    },
  });
}

export const c = node(out.settings, "", null);
export const configDir = N.configDir;
/** Profile name of this instance ("default" for the unnamed one). */
export const profile = N.profile;

/** Lines of a file, trimmed, empty ones dropped; [] if it doesn't exist. */
export function readLines(path) {
  return N.readLines(String(path));
}

export function bind(key, command, mode = "normal") {
  if (!MODES.includes(mode)) throw new Error(`bind: unknown mode ${mode}`);
  const err = N.checkKey(String(key));
  if (err) throw new Error(`bind ${key}: ${err}`);
  out.binds.push([mode, String(key), String(command)]);
}

export function unbind(key, mode = "normal") {
  if (!MODES.includes(mode)) throw new Error(`unbind: unknown mode ${mode}`);
  out.binds.push([mode, String(key), null]);
}

export function alias(name, command) {
  c.aliases[name] = command;
}

/** Settings for URLs matching `pat` (qutebrowser URL pattern syntax). */
export function pattern(pat, fn) {
  const err = N.checkPattern(String(pat));
  if (err) throw new Error(`pattern ${pat}: ${err}`);
  const store = {};
  out.patterns.push([String(pat), store]);
  fn(node(store, "", out.settings));
}
