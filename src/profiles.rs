//! Profiles: each is its own instance (process) with its own data dir; config is shared.
//! `:profile-new <name>`, `:profile-open <name>`.

/// Existing profile names, "default" first.
pub fn list() -> Vec<String> {
    let dir = crate::paths::xdg_root("XDG_DATA_HOME", ".local/share").join("profiles");
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| crate::paths::valid_profile_name(n))
        .collect();
    names.sort();
    names.insert(0, "default".into());
    names
}

pub fn create(name: &str) -> Result<(), String> {
    let name = name.trim();
    if !crate::paths::valid_profile_name(name) || name == "default" {
        return Err(format!(
            "bad profile name {name:?}: letters, digits, space, - and _"
        ));
    }
    if list().iter().any(|n| n == name) {
        return Err(format!("profile '{name}' exists: :profile-open {name}"));
    }
    let dir = crate::paths::xdg_root("XDG_DATA_HOME", ".local/share")
        .join("profiles")
        .join(name);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    open(name)
}

/// Start that profile's instance; if it already runs, it opens a new window.
pub fn open(name: &str) -> Result<(), String> {
    let name = name.trim();
    if !list().iter().any(|n| n == name) {
        return Err(format!("no profile '{name}' (:profile-new {name})"));
    }
    if Some(name) == crate::paths::profile().or(Some("default")) {
        crate::host::open_window(None);
        return Ok(());
    }
    // after an upgrade current_exe() is a deleted file: then go by how we were started
    let exe = std::env::current_exe()
        .ok()
        .filter(|p| p.exists())
        .or_else(|| std::env::args().next().map(std::path::PathBuf::from))
        .ok_or("can't find the novabrowser binary")?;
    let mut child = std::process::Command::new(exe)
        .arg(format!("--profile={name}"))
        .spawn()
        .map_err(|e| e.to_string())?;
    std::thread::spawn(move || child.wait()); // reap it when it exits
    crate::host::message("info", &format!("Opening profile {name}…"));
    Ok(())
}