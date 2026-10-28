//! Data/config/state locations, profiles and small file helpers shared by both editions.
use std::path::PathBuf;
use std::sync::OnceLock;

static APP: OnceLock<&'static str> = OnceLock::new();
static PROFILE: OnceLock<Option<String>> = OnceLock::new();

/// Directory name under the XDG roots: "novabrowser" (CEF edition, the default) or
/// "novabrowser-webkit". Call once at startup, before any path is used.
pub fn set_app(name: &'static str) {
    let _ = APP.set(name);
}

pub fn app() -> &'static str {
    APP.get().copied().unwrap_or("novabrowser")
}

/// `<XDG dir>/<app>[/profiles/<name>]`: per-profile data/state.
pub fn xdg_dir(var: &str, fallback: &str) -> PathBuf {
    let root = xdg_root(var, fallback);
    match profile() {
        Some(name) => root.join("profiles").join(name),
        None => root,
    }
}

/// `<XDG dir>/<app>`, the same for every profile.
pub fn xdg_root(var: &str, fallback: &str) -> PathBuf {
    let base = std::env::var_os(var)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").expect("HOME")).join(fallback));
    base.join(app())
}

/// Named profile from `--profile=<name>`; None = the default profile.
pub fn profile() -> Option<&'static str> {
    PROFILE
        .get_or_init(|| {
            std::env::args()
                .find_map(|a| a.strip_prefix("--profile=").map(str::to_string))
                .filter(|n| n != "default")
        })
        .as_deref()
}

/// Profile names: letters, digits, space, `-`, `_`; "default" is the unnamed one.
pub fn valid_profile_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 40
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_'))
        && !name.starts_with(' ')
}

/// Random per-run secret that internal pages must echo back (manager-page actions).
pub fn page_token() -> &'static str {
    static T: OnceLock<String> = OnceLock::new();
    T.get_or_init(|| {
        let mut b = [0u8; 16];
        use std::io::Read;
        std::fs::File::open("/dev/urandom")
            .and_then(|mut f| f.read_exact(&mut b))
            .expect("/dev/urandom");
        b.iter().map(|x| format!("{x:02x}")).collect()
    })
}

/// Temp file + fsync + rename, mode 0600: a crash mid-write never leaves a torn file.
pub fn write_atomic(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600) // new files only; data dirs are 0700 anyway
        .open(&tmp)?;
    f.write_all(data)?;
    f.sync_all()?;
    std::fs::rename(tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_names() {
        assert!(valid_profile_name("work"));
        assert!(valid_profile_name("my work_2-x"));
        assert!(!valid_profile_name(""));
        assert!(!valid_profile_name(" lead"));
        assert!(!valid_profile_name("a/b"));
        assert!(!valid_profile_name(&"x".repeat(41)));
    }

    #[test]
    fn write_atomic_replaces_and_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("nova-core-wa-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("f.json");
        write_atomic(&p, b"one").unwrap();
        write_atomic(&p, b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(!p.with_extension("tmp").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
