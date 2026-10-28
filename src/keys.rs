//! Key notation, key-sequence parsing and keychain matching.
//! Moved from novabrowser 9574fca src/keys.rs. `key_from_cef` (Windows VK codes, CEF edition
//! only) stays here because it is pure; the WebKit edition maps GDK keys itself.
//!
//! Port of qutebrowser's `keyinput/keyutils.py` (notation) and
//! `keyinput/basekeyparser.py` (BindingTrie + count/keychain matching).
//! SPDX-License-Identifier: GPL-3.0-or-later
//!
//! Canonical form (so that `==`/`Hash` work across notation and CEF events):
//! - printable char without Ctrl/Alt/Meta: `name` = the char as typed
//!   (shift applied: "J", "?", ":"), Shift dropped;
//! - printable char with Ctrl/Alt/Meta: letters lowercased, Shift explicit;
//! - space is the named key "Space";
//! - named keys ("Escape", "PgUp", "F5", ...) keep all modifiers incl. Shift.

use std::collections::HashMap;
use std::fmt;
use std::ops::{BitOr, BitOrAssign};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Mods(pub u8);

impl Mods {
    pub const SHIFT: Mods = Mods(1);
    pub const CTRL: Mods = Mods(2);
    pub const ALT: Mods = Mods(4);
    pub const META: Mods = Mods(8);
    pub const NONE: Mods = Mods(0);

    pub fn contains(self, o: Mods) -> bool {
        self.0 & o.0 == o.0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Ctrl, Alt or Meta held: the key needs `<...>` notation.
    fn has_command_mod(self) -> bool {
        self.0 & (Mods::CTRL.0 | Mods::ALT.0 | Mods::META.0) != 0
    }
}

impl BitOr for Mods {
    type Output = Mods;
    fn bitor(self, o: Mods) -> Mods {
        Mods(self.0 | o.0)
    }
}

impl BitOrAssign for Mods {
    fn bitor_assign(&mut self, o: Mods) {
        self.0 |= o.0;
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Key {
    pub name: String,
    pub mods: Mods,
}

/// (windows vk code, canonical name, extra lowercase aliases for notation).
const NAMED_KEYS: &[(i32, &str, &[&str])] = &[
    (0x1B, "Escape", &["esc"]),
    (0x0D, "Return", &["enter"]),
    (0x09, "Tab", &[]),
    (0x08, "Backspace", &[]),
    (0x2E, "Delete", &["del"]),
    (0x2D, "Insert", &["ins"]),
    (0x24, "Home", &[]),
    (0x23, "End", &[]),
    (0x21, "PgUp", &["pageup", "page up", "prior"]),
    (0x22, "PgDown", &["pagedown", "page down", "next"]),
    (0x25, "Left", &[]),
    (0x26, "Up", &[]),
    (0x27, "Right", &[]),
    (0x28, "Down", &[]),
    (0x20, "Space", &[]),
    (0x13, "Pause", &[]),
    (0x2C, "Print", &[]),
    (0x5D, "Menu", &[]),
    (0xA6, "Back", &[]),
    (0xA7, "Forward", &[]),
];

/// Modifier-only virtual keys: shift, ctrl, alt, capslock, super l/r, numlock, l/r shift/ctrl/alt.
const MODIFIER_VKS: &[i32] = &[
    0x10, 0x11, 0x12, 0x14, 0x5B, 0x5C, 0x90, 0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5,
];

fn named_from_vk(vk: i32) -> Option<String> {
    if (0x70..=0x87).contains(&vk) {
        return Some(format!("F{}", vk - 0x70 + 1));
    }
    NAMED_KEYS
        .iter()
        .find(|k| k.0 == vk)
        .map(|k| k.1.to_string())
}

/// Case-insensitive lookup of a named key in notation (input already lowercase).
fn named_from_str(lower: &str) -> Option<String> {
    if let Some(n) = lower.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
        if (1..=24).contains(&n) && !lower.starts_with("f0") {
            return Some(format!("F{n}"));
        }
    }
    NAMED_KEYS
        .iter()
        .find(|k| k.1.eq_ignore_ascii_case(lower) || k.2.contains(&lower))
        .map(|k| k.1.to_string())
}

/// Printable = may be a key name on its own (qutebrowser rejects keys < Space and
/// anything outside the BMP).
fn is_valid_char(c: char) -> bool {
    !c.is_control() && (c as u32) <= 0xFFFF
}

impl Key {
    /// Named (non-printable) key, e.g. `Key::named("Escape", Mods::SHIFT)`.
    pub fn named(name: &str, mods: Mods) -> Key {
        Key {
            name: name.to_string(),
            mods,
        }
    }

    /// Canonical key for a character plus modifiers (see module docs).
    pub fn from_char(c: char, mods: Mods) -> Key {
        if c == ' ' {
            return Key::named("Space", mods);
        }
        if mods.has_command_mod() {
            let lower: String = c.to_lowercase().collect();
            let name = if lower.chars().count() == 1 {
                lower
            } else {
                c.to_string()
            };
            return Key { name, mods };
        }
        let name = if mods.contains(Mods::SHIFT) && c.is_lowercase() {
            let upper: String = c.to_uppercase().collect();
            if upper.chars().count() == 1 {
                upper
            } else {
                c.to_string()
            }
        } else {
            c.to_string()
        };
        Key {
            name,
            mods: Mods::NONE,
        }
    }

    fn single_char(&self) -> Option<char> {
        let mut it = self.name.chars();
        let c = it.next()?;
        it.next().is_none().then_some(c)
    }
}

impl fmt::Display for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ch = self.single_char();
        if ch.is_some() && self.mods.is_empty() {
            return f.write_str(&self.name);
        }
        f.write_str("<")?;
        // Qt's modifier order (Meta+Ctrl+Alt+Shift), qutebrowser's '-' separator.
        for (m, s) in [
            (Mods::META, "Meta-"),
            (Mods::CTRL, "Ctrl-"),
            (Mods::ALT, "Alt-"),
            (Mods::SHIFT, "Shift-"),
        ] {
            if self.mods.contains(m) {
                f.write_str(s)?;
            }
        }
        match ch {
            Some('<') => f.write_str("less")?,
            Some('>') => f.write_str("greater")?,
            _ => f.write_str(&self.name)?,
        }
        f.write_str(">")
    }
}

/// Parse a key string like `gT`, `<Space>tg` or `<Ctrl-x><Meta-y>`.
pub fn parse_keystring(s: &str) -> Result<Vec<Key>, String> {
    let err = |e: String| format!("Could not parse {s:?}: {e}");
    let mut keys = Vec::new();
    let mut special = String::new();
    let mut in_special = false;
    // Mirrors keyutils._parse_keystring: an unclosed '<' is a literal '<'
    // followed by the collected chars; a '<' inside '<..' restarts nothing.
    for c in s.chars() {
        match c {
            '>' if in_special => {
                keys.push(parse_special_key(&special).map_err(err)?);
                special.clear();
                in_special = false;
            }
            '<' => in_special = true,
            _ if in_special => special.push(c),
            _ => keys.push(parse_single_char(c).map_err(err)?),
        }
    }
    if in_special {
        keys.push(Key::from_char('<', Mods::NONE));
        for c in special.chars() {
            keys.push(parse_single_char(c).map_err(err)?);
        }
    }
    Ok(keys)
}

fn parse_single_char(c: char) -> Result<Key, String> {
    if !is_valid_char(c) {
        return Err(format!("Got invalid key {c:?}!"));
    }
    Ok(Key::from_char(c, Mods::NONE))
}

/// Port of keyutils._parse_special_key plus Qt's "Mod+Mod+Key" decoding.
fn parse_special_key(inner: &str) -> Result<Key, String> {
    let mut s = inner.to_lowercase();
    for (orig, repl) in [
        ("control", "ctrl"),
        ("windows", "meta"),
        ("mod4", "meta"),
        ("command", "meta"),
        ("cmd", "meta"),
        ("super", "meta"),
        ("mod1", "alt"),
        ("less", "<"),
        ("greater", ">"),
    ] {
        s = s.replace(orig, repl);
    }
    for m in ["ctrl", "meta", "alt", "shift", "num"] {
        s = s.replace(&format!("{m}-"), &format!("{m}+"));
    }
    // Split "mods+key"; a trailing "++" (or a lone "+") means the '+' key.
    let (mod_part, key_part) = if s == "+" {
        ("", "+")
    } else if let Some(prefix) = s.strip_suffix("++") {
        (prefix, "+")
    } else {
        match s.rfind('+') {
            Some(i) => (&s[..i], &s[i + 1..]),
            None => ("", s.as_str()),
        }
    };
    if key_part.is_empty() {
        return Err("Got invalid key!".to_string());
    }
    let mut mods = Mods::NONE;
    if !mod_part.is_empty() {
        for m in mod_part.split('+') {
            mods |= match m {
                "ctrl" => Mods::CTRL,
                "alt" => Mods::ALT,
                "meta" => Mods::META,
                "shift" => Mods::SHIFT,
                // Keypad modifier: CEF events don't carry it here, so it's dropped.
                "num" => Mods::NONE,
                _ => return Err(format!("Unknown modifier {m:?}")),
            };
        }
    }
    let mut chars = key_part.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        if !is_valid_char(c) {
            return Err(format!("Got invalid key {c:?}!"));
        }
        return Ok(Key::from_char(c, mods));
    }
    match named_from_str(key_part) {
        Some(name) => Ok(Key::named(&name, mods)),
        None => Err(format!("Unknown key {key_part:?}")),
    }
}

// shortcut: US layout OEM map, used only when Ctrl/Alt/Meta is held (CEF's
// `character` is then a control code or layout-dependent); non-US layouts get
// US names for punctuation chords.
fn char_from_vk(vk: i32) -> Option<char> {
    let c = match vk {
        0x30..=0x39 | 0x41..=0x5A => {
            return char::from_u32(vk as u32).map(|c| c.to_ascii_lowercase())
        }
        0x60..=0x69 => return char::from_u32((vk - 0x60) as u32 + '0' as u32),
        0x6A => '*',
        0x6B => '+',
        0x6D => '-',
        0x6E => '.',
        0x6F => '/',
        0xBA => ';',
        0xBB => '=',
        0xBC => ',',
        0xBD => '-',
        0xBE => '.',
        0xBF => '/',
        0xC0 => '`',
        0xDB => '[',
        0xDC => '\\',
        0xDD => ']',
        0xDE => '\'',
        _ => return None,
    };
    Some(c)
}

/// From a CEF browser-process key event (Linux/Wayland). vk = windows_key_code, character = KeyEvent.character
/// (already shifted; for Ctrl+letter it's a control code), cef_mods = KeyEvent.modifiers bitfield
/// (SHIFT 0x2, CTRL 0x4, ALT 0x8, COMMAND/META 0x80; ignore 0x1 caps-lock, 0x100 num-lock, 0x2000 is-repeat and other bits).
/// Returns None for modifier-only presses (vk 0x10 shift,0x11 ctrl,0x12 alt,0x5B/0x5C super, 0x14 capslock, 0x90 numlock, 0xA0-0xA5).
pub fn key_from_cef(vk: i32, character: u16, cef_mods: u32) -> Option<Key> {
    if MODIFIER_VKS.contains(&vk) {
        return None;
    }
    let mut mods = Mods::NONE;
    for (bit, m) in [
        (0x2, Mods::SHIFT),
        (0x4, Mods::CTRL),
        (0x8, Mods::ALT),
        (0x80, Mods::META),
    ] {
        if cef_mods & bit != 0 {
            mods |= m;
        }
    }
    if let Some(name) = named_from_vk(vk) {
        return Some(Key::named(&name, mods));
    }
    let typed = char::from_u32(character as u32).filter(|&c| is_valid_char(c));
    if mods.has_command_mod() {
        // Ctrl+letter etc.: character is a control code, so name from vk.
        if let Some(c) = char_from_vk(vk).or(typed) {
            return Some(Key::from_char(c, mods));
        }
        return None;
    }
    // Plain/shifted: the typed char already has shift (and layout) applied.
    match typed {
        Some(c) => Some(Key::from_char(c, Mods::NONE)),
        None => char_from_vk(vk).map(|c| Key::from_char(c, mods)),
    }
}

/// One mode's bindings (qutebrowser's BindingTrie).
#[derive(Clone, Debug, Default)]
pub struct Keymap {
    root: Node,
}

#[derive(Clone, Debug, Default)]
struct Node {
    children: HashMap<Key, Node>,
    command: Option<String>,
}

enum Match<'a> {
    Exact(&'a str),
    Partial,
    None,
}

impl Keymap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind `seq` to `command`; an empty sequence is ignored.
    pub fn bind(&mut self, seq: &[Key], command: &str) {
        if seq.is_empty() {
            return;
        }
        let mut node = &mut self.root;
        for k in seq {
            node = node.children.entry(k.clone()).or_default();
        }
        node.command = Some(command.to_string());
    }

    /// Remove the binding for `seq`; returns whether one existed.
    pub fn unbind(&mut self, seq: &[Key]) -> bool {
        fn remove(node: &mut Node, seq: &[Key]) -> bool {
            let Some((first, rest)) = seq.split_first() else {
                return node.command.take().is_some();
            };
            let Some(child) = node.children.get_mut(first) else {
                return false;
            };
            let removed = remove(child, rest);
            if child.command.is_none() && child.children.is_empty() {
                node.children.remove(first);
            }
            removed
        }
        !seq.is_empty() && remove(&mut self.root, seq)
    }

    pub fn get(&self, seq: &[Key]) -> Option<&str> {
        match self.lookup(seq) {
            Some(node) if !seq.is_empty() => node.command.as_deref(),
            _ => None,
        }
    }

    /// Bindings that continue `seq`: (remaining keys, command), sorted by keys.
    pub fn continuations(&self, seq: &[Key]) -> Vec<(String, String)> {
        fn walk(node: &Node, path: &mut Vec<String>, out: &mut Vec<(String, String)>) {
            if let Some(c) = &node.command {
                out.push((path.concat(), c.clone()));
            }
            for (k, child) in &node.children {
                path.push(k.to_string());
                walk(child, path, out);
                path.pop();
            }
        }
        let mut out = vec![];
        if let Some(node) = self.lookup(seq).filter(|_| !seq.is_empty()) {
            for (k, child) in &node.children {
                walk(child, &mut vec![k.to_string()], &mut out);
            }
        }
        out.sort();
        out
    }

    fn lookup(&self, seq: &[Key]) -> Option<&Node> {
        let mut node = &self.root;
        for k in seq {
            node = node.children.get(k)?;
        }
        Some(node)
    }

    /// BindingTrie.matches: a command on the node wins over longer bindings.
    fn matches(&self, seq: &[Key]) -> Match<'_> {
        match self.lookup(seq) {
            Some(Node {
                command: Some(c), ..
            }) => Match::Exact(c),
            Some(n) if !n.children.is_empty() => Match::Partial,
            _ => Match::None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// key used as count digit or keychain is a strict prefix of a binding
    Pending,
    /// full binding matched
    Run { command: String, count: Option<u32> },
    /// no binding; buffer was cleared; caller decides whether to forward the key
    NoMatch,
}

/// Keychain buffer + count (qutebrowser's BaseKeyParser state).
#[derive(Clone, Debug, Default)]
pub struct Parser {
    sequence: Vec<Key>,
    count: String,
}

impl Parser {
    /// Port of BaseKeyParser.handle (without key_mappings / keypad stripping).
    pub fn handle(&mut self, key: &Key, map: &Keymap, supports_count: bool) -> Outcome {
        let mut seq = self.sequence.clone();
        seq.push(key.clone());
        match map.matches(&seq) {
            Match::Exact(command) => {
                // Like qutebrowser: a count that doesn't parse aborts the chain.
                let count = if self.count.is_empty() {
                    Ok(None)
                } else {
                    self.count.parse().map(Some)
                };
                self.clear();
                match count {
                    Ok(count) => Outcome::Run {
                        command: command.to_string(),
                        count,
                    },
                    Err(_) => Outcome::NoMatch,
                }
            }
            Match::Partial => {
                self.sequence = seq;
                Outcome::Pending
            }
            Match::None => {
                // _match_count: a lone digit (no leading 0) extends the count,
                // even in the middle of a keychain; the chain is kept.
                let digit =
                    key.mods.is_empty() && key.single_char().is_some_and(|c| c.is_ascii_digit());
                if supports_count && digit && !(self.count.is_empty() && key.name == "0") {
                    self.count.push_str(&key.name);
                    return Outcome::Pending;
                }
                self.clear();
                Outcome::NoMatch
            }
        }
    }

    pub fn clear(&mut self) {
        self.sequence.clear();
        self.count.clear();
    }

    pub fn sequence(&self) -> &[Key] {
        &self.sequence
    }

    /// e.g. "3g" for the status bar
    pub fn pending(&self) -> String {
        let mut s = self.count.clone();
        for k in &self.sequence {
            s.push_str(&k.to_string());
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq(s: &str) -> Vec<Key> {
        parse_keystring(s).unwrap_or_else(|e| panic!("{e}"))
    }

    fn one(s: &str) -> Key {
        let v = seq(s);
        assert_eq!(v.len(), 1, "{s}");
        v.into_iter().next().unwrap()
    }

    fn render(keys: &[Key]) -> String {
        keys.iter().map(|k| k.to_string()).collect()
    }

    // --- notation (test_keyutils.py) ---

    #[test]
    fn display_canonical_forms() {
        for s in [
            "a",
            "J",
            "?",
            ":",
            "<Space>",
            "<Escape>",
            "<Ctrl-w>",
            "<Ctrl-Shift-n>",
            "<Alt-Backspace>",
            "<F5>",
            "<Shift-Escape>",
            "<Shift-Tab>",
            "<Shift-Space>",
            "<Meta-Ctrl-Alt-Shift-a>",
            "<Ctrl-[>",
            "<PgUp>",
            "<Back>",
            "<Forward>",
            "<Return>",
            "<Alt-less>",
            "<Alt-greater>",
        ] {
            assert_eq!(render(&seq(s)), s, "roundtrip {s}");
        }
    }

    #[test]
    fn test_key_info_str_ports() {
        let k = |name: &str, m: Mods| Key::from_char(name.chars().next().unwrap(), m).to_string();
        assert_eq!(k("a", Mods::NONE), "a");
        assert_eq!(k("a", Mods::SHIFT), "A");
        assert_eq!(k(" ", Mods::NONE), "<Space>");
        assert_eq!(k(" ", Mods::SHIFT), "<Shift-Space>");
        assert_eq!(Key::named("Tab", Mods::SHIFT).to_string(), "<Shift-Tab>");
        assert_eq!(k("A", Mods::CTRL), "<Ctrl-a>");
        assert_eq!(k("A", Mods::CTRL | Mods::SHIFT), "<Ctrl-Shift-a>");
        assert_eq!(
            k("a", Mods::CTRL | Mods::ALT | Mods::META | Mods::SHIFT),
            "<Meta-Ctrl-Alt-Shift-a>"
        );
        assert_eq!(k("Œ", Mods::NONE), "Œ");
    }

    #[test]
    fn test_parse_keystr_parts() {
        let cases: &[(&str, &[&str])] = &[
            ("a", &["a"]),
            ("ab", &["a", "b"]),
            ("a<", &["a", "<"]),
            ("a>", &["a", ">"]),
            ("<a", &["<", "a"]),
            (">a", &[">", "a"]),
            ("aA", &["a", "A"]),
            ("a<Ctrl+a>b", &["a", "<Ctrl-a>", "b"]),
            ("<Ctrl+a>a", &["<Ctrl-a>", "a"]),
            ("a<Ctrl+a>", &["a", "<Ctrl-a>"]),
            ("<Ctrl-a>", &["<Ctrl-a>"]),
            ("<Num-a>", &["a"]),
            ("gT", &["g", "T"]),
            ("<Space>tg", &["<Space>", "t", "g"]),
        ];
        for (s, parts) in cases {
            let got: Vec<String> = seq(s).iter().map(|k| k.to_string()).collect();
            assert_eq!(&got, parts, "{s}");
        }
    }

    #[test]
    fn test_str_normalization() {
        for (orig, norm) in [
            ("<Control+x>", "<Ctrl-x>"),
            ("<Windows+x>", "<Meta-x>"),
            ("<Super+x>", "<Meta-x>"),
            ("<Mod4+x>", "<Meta-x>"),
            ("<Command+x>", "<Meta-x>"),
            ("<Cmd+x>", "<Meta-x>"),
            ("<Mod1+x>", "<Alt-x>"),
            ("<Control-->", "<Ctrl-->"),
            ("<Windows++>", "<Meta-+>"),
            ("<ctrl-x>", "<Ctrl-x>"),
            ("<control+x>", "<Ctrl-x>"),
            ("<a>b", "ab"),
            ("<esc>", "<Escape>"),
            ("<ESCAPE>", "<Escape>"),
            ("<Enter>", "<Return>"),
            ("<return>", "<Return>"),
            ("<PageUp>", "<PgUp>"),
            ("<Prior>", "<PgUp>"),
            ("<Next>", "<PgDown>"),
            ("<Page Down>", "<PgDown>"),
            ("<Del>", "<Delete>"),
            ("<Ins>", "<Insert>"),
            ("<f12>", "<F12>"),
            ("<back>", "<Back>"),
            ("<shift-escape>", "<Shift-Escape>"),
            ("<Shift+Escape>", "<Shift-Escape>"),
            ("<Ctrl-Alt-y>", "<Ctrl-Alt-y>"),
            ("<Alt-Ctrl-y>", "<Ctrl-Alt-y>"),
            ("<Control-x><Meta-y>", "<Ctrl-x><Meta-y>"),
        ] {
            assert_eq!(render(&seq(orig)), norm, "{orig}");
        }
    }

    #[test]
    fn test_parse_equalities() {
        // test_operators / test_hash / test_parse
        assert_eq!(seq("a"), seq("<a>"));
        assert_ne!(seq("a"), seq("A"));
        assert_ne!(seq("a"), seq("<Shift-a>"));
        assert_eq!(seq("A"), seq("<Shift-a>"));
        assert_eq!(seq("X"), seq("<Shift-x>"));
        assert_ne!(seq("abcd"), seq("abcde"));
        assert_eq!(seq("<Ctrl-x>"), seq("<Ctrl-X>"));
        assert_eq!(seq("<Ctrl-T>"), seq("<ctrl-t>"));
        assert_ne!(seq("<Ctrl-t>"), seq("<Ctrl-Shift-t>"));
        assert_eq!(one("<Ctrl-Shift-T>").mods, Mods::CTRL | Mods::SHIFT);
        assert_eq!(one("<Ctrl-Shift-T>").name, "t");
        assert_eq!(one(">"), Key::from_char('>', Mods::NONE));
        assert_eq!(one("<"), Key::from_char('<', Mods::NONE));
        assert_eq!(one("<alt+greater>"), Key::from_char('>', Mods::ALT));
        assert_eq!(one("<alt+less>"), Key::from_char('<', Mods::ALT));
        assert_eq!(one("<Ctrl-[>"), Key::from_char('[', Mods::CTRL));
        assert_eq!(one(" "), one("<Space>"));
        assert_eq!(one("<Shift-:>"), one(":"));
        assert_eq!(one("<Escape>"), Key::named("Escape", Mods::NONE));
        assert_eq!(seq("xyz").len(), 3);
        assert_eq!(seq("").len(), 0);
        assert_eq!(seq("<Ctrl-a>").len(), 1);
        assert_eq!(seq("abcde").len(), 5);
    }

    #[test]
    fn test_parse_invalid() {
        for s in [
            "<alt+<>",
            "<alt+>>",
            "<blub>",
            "<>",
            "\u{10000}",
            "\x1f",
            "<Ctrl-blub>",
            "<Hyper-x>",
            "<Ctrl->",
            "<f0>",
            "<f25>",
        ] {
            assert!(parse_keystring(s).is_err(), "{s:?} should fail");
        }
        let e = parse_keystring("<blub>").unwrap_err();
        assert!(e.starts_with("Could not parse \"<blub>\""), "{e}");
    }

    #[test]
    fn unclosed_bracket_is_literal() {
        // keyutils._parse_keystring: unclosed '<' yields '<' then the chars.
        assert_eq!(render(&seq("<Ctrl-<")), "<Ctrl-");
        assert_eq!(seq("<Ctrl-<").len(), 6);
    }

    // --- key_from_cef ---

    #[test]
    fn key_from_cef_cases() {
        let c = |vk: i32, ch: u16, m: u32| key_from_cef(vk, ch, m).map(|k| k.to_string());
        let ch = |x: char| x as u16;
        assert_eq!(c(0x4A, ch('j'), 0).as_deref(), Some("j"));
        assert_eq!(c(0x4A, ch('J'), 0x2).as_deref(), Some("J"));
        assert_eq!(c(0x4A, 10, 0x4).as_deref(), Some("<Ctrl-j>"));
        assert_eq!(c(0x54, 20, 0x4 | 0x2).as_deref(), Some("<Ctrl-Shift-t>"));
        assert_eq!(c(0x1B, 27, 0).as_deref(), Some("<Escape>"));
        assert_eq!(c(0x1B, 27, 0x2).as_deref(), Some("<Shift-Escape>"));
        assert_eq!(c(0xBA, ch(';'), 0).as_deref(), Some(";"));
        assert_eq!(c(0xDB, 27, 0x4).as_deref(), Some("<Ctrl-[>"));
        assert_eq!(c(0x20, ch(' '), 0).as_deref(), Some("<Space>"));
        assert_eq!(c(0x10, 0, 0x2), None);
        assert_eq!(c(0x41, ch('a'), 0x2000).as_deref(), Some("a"));
        assert_eq!(c(0x08, 8, 0x8).as_deref(), Some("<Alt-Backspace>"));
        // extras
        assert_eq!(c(0xBA, ch(':'), 0x2).as_deref(), Some(":"));
        assert_eq!(c(0x09, 9, 0x2).as_deref(), Some("<Shift-Tab>"));
        assert_eq!(c(0x74, 0, 0).as_deref(), Some("<F5>"));
        assert_eq!(c(0xA6, 0, 0).as_deref(), Some("<Back>"));
        assert_eq!(c(0x11, 0, 0x4), None);
        assert_eq!(c(0x5B, 0, 0x80), None);
        assert_eq!(c(0x4E, 14, 0x80).as_deref(), Some("<Meta-n>"));
        assert_eq!(c(0x41, ch('a'), 0x1 | 0x100).as_deref(), Some("a"));
        assert_eq!(c(0x4A, ch('j'), 0x1 | 0x2).as_deref(), Some("j")); // caps+shift typed 'j'
        assert_eq!(c(0x33, ch('#'), 0x2).as_deref(), Some("#"));
        assert_eq!(c(0x33, 0, 0x4).as_deref(), Some("<Ctrl-3>"));
        // CEF keys equal parsed notation
        assert_eq!(key_from_cef(0x54, 20, 0x6), Some(one("<Ctrl-Shift-T>")));
        assert_eq!(key_from_cef(0x4A, 10, 0x4), Some(one("<Ctrl-J>")));
        assert_eq!(key_from_cef(0x47, 'G' as u16, 0x2), Some(one("G")));
    }

    // --- Keymap / BindingTrie ---

    #[test]
    fn keymap_bind_get_unbind() {
        let mut m = Keymap::new();
        m.bind(&seq("abcd"), "x");
        m.bind(&seq("ab"), "y");
        assert_eq!(m.get(&seq("abcd")), Some("x"));
        assert_eq!(m.get(&seq("ab")), Some("y"));
        assert_eq!(m.get(&seq("abc")), None);
        assert_eq!(m.get(&[]), None);
        assert!(m.unbind(&seq("abcd")));
        assert!(!m.unbind(&seq("abcd")));
        assert!(!m.unbind(&seq("abc")));
        assert_eq!(m.get(&seq("ab")), Some("y"));
        assert!(matches!(m.matches(&seq("abc")), Match::None));
        m.bind(&seq("ab"), "z");
        assert_eq!(m.get(&seq("ab")), Some("z"));
    }

    #[test]
    fn test_matches() {
        // TestKeySequence.MATCH_TESTS (non-empty cases)
        let cases = [
            ("abc", "abcd", "partial"),
            ("abcd", "abcd", "exact"),
            ("ax", "abcd", "none"),
            ("abcdef", "abcd", "none"),
            ("abc", "abcdef", "partial"),
            ("abcde", "abcdef", "partial"),
            ("abcd", "abcdef", "partial"),
            ("abcdx", "abcdef", "none"),
            ("ax", "abcdef", "none"),
            ("abcdefg", "abcdef", "none"),
            ("abcdef", "abcdef", "exact"),
            ("ab", "a", "none"),
        ];
        for (entered, configured, want) in cases {
            let mut m = Keymap::new();
            m.bind(&seq(configured), "cmd");
            let got = match m.matches(&seq(entered)) {
                Match::Exact(_) => "exact",
                Match::Partial => "partial",
                Match::None => "none",
            };
            assert_eq!(got, want, "{entered} vs {configured}");
        }
    }

    // --- Parser (test_basekeyparser.py) ---

    /// conftest.BINDINGS['prompt']
    fn prompt_map() -> Keymap {
        let mut m = Keymap::new();
        for (k, c) in [
            ("<Ctrl-a>", "message-info ctrla"),
            ("a", "message-info a"),
            ("ba", "message-info ba"),
            ("ax", "message-info ax"),
            ("ccc", "message-info ccc"),
            ("yY", "yank -s"),
            ("0", "message-info 0"),
            ("1", "message-info 1"),
        ] {
            m.bind(&seq(k), c);
        }
        m
    }

    fn feed(p: &mut Parser, m: &Keymap, keys: &str, count: bool) -> Vec<Outcome> {
        seq(keys).iter().map(|k| p.handle(k, m, count)).collect()
    }

    fn runs(outs: &[Outcome]) -> Vec<(String, Option<u32>)> {
        outs.iter()
            .filter_map(|o| match o {
                Outcome::Run { command, count } => Some((command.clone(), *count)),
                _ => None,
            })
            .collect()
    }

    fn run(c: &str, n: Option<u32>) -> (String, Option<u32>) {
        (c.to_string(), n)
    }

    #[test]
    fn test_split_count() {
        let mut m = Keymap::new();
        m.bind(&seq("gg"), "scroll-to-perc 0");
        for (input, supports, count, pending_seq) in [
            ("10", true, "10", ""),
            ("10g", true, "10", "g"),
            ("10e4g", true, "4", "g"),
            ("g", true, "", "g"),
            ("0", true, "", ""),
            ("10g", false, "", "g"),
        ] {
            let mut p = Parser::default();
            feed(&mut p, &m, input, supports);
            assert_eq!(p.count, count, "{input}");
            assert_eq!(render(&p.sequence), pending_seq, "{input}");
            assert_eq!(p.pending(), format!("{count}{pending_seq}"));
        }
    }

    #[test]
    fn test_valid_key_and_count() {
        let m = prompt_map();
        let mut p = Parser::default();
        let out = feed(&mut p, &m, "<Ctrl-a><Ctrl-x>", true);
        assert_eq!(runs(&out), [run("message-info ctrla", None)]);
        assert_eq!(out[1], Outcome::NoMatch);
        assert!(p.sequence.is_empty());

        let out = feed(&mut p, &m, "5<Ctrl-a>", true);
        assert_eq!(out[0], Outcome::Pending);
        assert_eq!(runs(&out), [run("message-info ctrla", Some(5))]);
    }

    #[test]
    fn test_invalid_keys() {
        let m = prompt_map();
        for keys in ["bc", "<Ctrl-Alt-a>"] {
            let mut p = Parser::default();
            let out = feed(&mut p, &m, keys, true);
            assert!(runs(&out).is_empty(), "{keys}");
            assert_eq!(out.last(), Some(&Outcome::NoMatch));
            assert!(p.sequence.is_empty() && p.pending().is_empty());
        }
    }

    #[test]
    fn test_valid_keychain() {
        // 'x' ignored (no match), then the real chain.
        let m = prompt_map();
        let mut p = Parser::default();
        let out = feed(&mut p, &m, "xba", true);
        assert_eq!(
            out,
            [
                Outcome::NoMatch,
                Outcome::Pending,
                Outcome::Run {
                    command: "message-info ba".into(),
                    count: None
                }
            ]
        );
        assert!(p.sequence.is_empty());
    }

    #[test]
    fn non_continuing_key_resets_without_retry() {
        // qutebrowser matches the whole chain only: "b" then "a"-less "x" fails,
        // even though "a"/"ax" alone would match; it's not retried as a new chain.
        let m = prompt_map();
        let mut p = Parser::default();
        assert_eq!(
            feed(&mut p, &m, "bc", true),
            [Outcome::Pending, Outcome::NoMatch]
        );
        assert_eq!(
            feed(&mut p, &m, "ba", true)[1],
            Outcome::Run {
                command: "message-info ba".into(),
                count: None
            }
        );
        // "b" then "a"... but "b" then "y": no retry of "y" as start of "yY".
        assert_eq!(
            feed(&mut p, &m, "by", true),
            [Outcome::Pending, Outcome::NoMatch]
        );
        assert!(p.pending().is_empty());
    }

    #[test]
    fn test_number_press() {
        let m = prompt_map();
        for (k, n) in [("0", "0"), ("1", "1")] {
            let mut p = Parser::default();
            assert_eq!(
                runs(&feed(&mut p, &m, k, true)),
                [run(&format!("message-info {n}"), None)]
            );
            assert!(p.sequence.is_empty());
        }
        // A bound digit wins over count even mid-count.
        let mut p = Parser::default();
        assert_eq!(
            runs(&feed(&mut p, &m, "51", true)),
            [run("message-info 1", Some(5))]
        );
    }

    #[test]
    fn test_binding_with_shift_and_partial_before_full() {
        let m = prompt_map();
        let mut p = Parser::default();
        assert_eq!(runs(&feed(&mut p, &m, "yY", true)), [run("yank -s", None)]);

        let mut m = Keymap::new();
        m.bind(&seq("ab"), "message-info bar");
        m.bind(&seq("a"), "message-info foo");
        let mut p = Parser::default();
        assert_eq!(
            runs(&feed(&mut p, &m, "a", true)),
            [run("message-info foo", None)]
        );
    }

    #[test]
    fn test_count() {
        let m = prompt_map();
        let mut p = Parser::default();
        // test_no_count
        assert_eq!(
            runs(&feed(&mut p, &m, "ba", true)),
            [run("message-info ba", None)]
        );
        // test_count_0
        assert_eq!(
            runs(&feed(&mut p, &m, "0ba", true)),
            [run("message-info 0", None), run("message-info ba", None)]
        );
        // test_count_42
        assert_eq!(
            runs(&feed(&mut p, &m, "42ba", true)),
            [run("message-info ba", Some(42))]
        );
        assert!(p.sequence.is_empty());
        // test_count_42_invalid
        assert!(runs(&feed(&mut p, &m, "42ccx", true)).is_empty());
        assert!(p.pending().is_empty());
        assert_eq!(
            runs(&feed(&mut p, &m, "23ccc", true)),
            [run("message-info ccc", Some(23))]
        );
        assert!(p.pending().is_empty());
        // count with zero after first digit
        let mut q = Keymap::new();
        q.bind(&seq("j"), "scroll down");
        assert_eq!(
            runs(&feed(&mut p, &q, "10j", true)),
            [run("scroll down", Some(10))]
        );
        // supports_count = false: digits are plain keys
        assert_eq!(
            feed(&mut p, &q, "5j", false),
            [
                Outcome::NoMatch,
                Outcome::Run {
                    command: "scroll down".into(),
                    count: None
                }
            ]
        );
    }

    #[test]
    fn test_count_keystring_update() {
        let m = prompt_map();
        let mut p = Parser::default();
        p.handle(&one("4"), &m, true);
        assert_eq!(p.pending(), "4");
        p.handle(&one("2"), &m, true);
        assert_eq!(p.pending(), "42");
        p.handle(&one("c"), &m, true);
        assert_eq!(p.pending(), "42c");
        p.clear();
        assert_eq!(p.pending(), "");
    }

    #[test]
    fn digit_mid_chain_extends_count() {
        // _match_count runs on the last key even with a pending chain; the chain stays.
        let m = prompt_map();
        let mut p = Parser::default();
        assert_eq!(
            feed(&mut p, &m, "c5", true),
            [Outcome::Pending, Outcome::Pending]
        );
        assert_eq!(p.pending(), "5c");
        assert_eq!(
            runs(&feed(&mut p, &m, "cc", true)),
            [run("message-info ccc", Some(5))]
        );
    }

    #[test]
    fn test_count_limit_exceeded() {
        let mut m = Keymap::new();
        m.bind(&seq("ba"), "message-info ba");
        let mut p = Parser::default();
        let out = feed(&mut p, &m, "99999999999ba", true);
        assert!(runs(&out).is_empty());
        assert_eq!(out.last(), Some(&Outcome::NoMatch));
        assert!(p.pending().is_empty());
    }

    #[test]
    fn modifier_and_escape_not_special() {
        let mut m = Keymap::new();
        m.bind(
            &seq("<Escape>"),
            "clear-keychain ;; search ;; fullscreen --leave",
        );
        m.bind(&seq("gg"), "scroll-to-perc 0");
        let mut p = Parser::default();
        p.handle(&one("3"), &m, true);
        p.handle(&one("g"), &m, true);
        assert_eq!(p.pending(), "3g");
        // Escape doesn't continue "g": NoMatch and cleared (it's not retried).
        assert_eq!(p.handle(&one("<Escape>"), &m, true), Outcome::NoMatch);
        assert_eq!(
            p.handle(&one("<Escape>"), &m, true),
            Outcome::Run {
                command: m.get(&seq("<Escape>")).unwrap().into(),
                count: None
            }
        );
    }
}
