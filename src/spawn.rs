//! External programs and qutebrowser-compatible userscripts (env vars + `QUTE_FIFO`).
//! The edition reads the page (selection, HTML, text) and passes it in; FIFO lines
//! come back through `Host::run_in_tab`, output through `Host::show_text`.
use crate::{config, host};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// What a userscript sees of the tab that started it.
#[derive(Default)]
pub struct Page {
    pub url: String,
    pub title: String,
    pub selection: String,
    pub html: String,
    pub text: String,
    /// Edition's id of the tab (FIFO lines run there).
    pub tab: u64,
}

/// `spawn <cmd> [args]` (not a userscript): start it, never wait on the UI thread.
pub fn program(argv: &[String], output: bool) -> Result<(), String> {
    let (prog, args) = argv
        .split_first()
        .ok_or("usage: spawn [-u] [-o] <command> [args…]")?;
    let mut cmd = Command::new(expand_home(prog));
    cmd.args(args).stdin(Stdio::null());
    run_child(cmd, prog.clone(), output, None)
}

/// `spawn -u <script> [args]` with the page already read.
pub fn userscript(
    argv: &[String],
    output: bool,
    count: Option<u32>,
    page: Page,
) -> Result<(), String> {
    let (prog, args) = argv
        .split_first()
        .ok_or("usage: spawn [-u] [-o] <command> [args…]")?;
    let path = find_userscript(prog)?;
    let dir = tempdir()?;
    let fifo = dir.join("fifo");
    let _ = std::fs::write(dir.join("html"), &page.html);
    let _ = std::fs::write(dir.join("text"), &page.text);
    let cfg = config::get();
    let mut cmd = Command::new(&path);
    cmd.args(args)
        .stdin(Stdio::null())
        .env("QUTE_USERSCRIPT", "1")
        .env("QUTE_MODE", "command")
        .env("QUTE_URL", &page.url)
        .env("QUTE_TITLE", &page.title)
        .env("QUTE_SELECTED_TEXT", &page.selection)
        .env("QUTE_HTML", dir.join("html"))
        .env("QUTE_TEXT", dir.join("text"))
        .env("QUTE_FIFO", &fifo)
        .env("QUTE_CONFIG_DIR", config::dir())
        .env(
            "QUTE_DATA_DIR",
            crate::paths::xdg_dir("XDG_DATA_HOME", ".local/share"),
        )
        .env(
            "QUTE_DOWNLOAD_DIR",
            expand_home(&cfg.str("downloads.location.directory")),
        )
        .env("QUTE_COMMANDLINE_TEXT", "")
        .env(
            "QUTE_VERSION",
            format!("{} {}", crate::paths::app(), env!("CARGO_PKG_VERSION")),
        );
    if let Some(n) = count {
        cmd.env("QUTE_COUNT", n.to_string());
    }
    mkfifo(&fifo)?;
    read_fifo(fifo, page.tab);
    run_child(cmd, prog.clone(), output, Some(dir))
}

/// `name` as given if it has a `/`, else from `<config>/userscripts`, then `<data>/userscripts`.
pub fn find_userscript(name: &str) -> Result<PathBuf, String> {
    if name.contains('/') {
        return Ok(PathBuf::from(expand_home(name)));
    }
    let dirs = [
        config::dir().join("userscripts"),
        crate::paths::xdg_dir("XDG_DATA_HOME", ".local/share").join("userscripts"),
    ];
    dirs.into_iter()
        .map(|d| d.join(name))
        .find(|p| p.is_file())
        .ok_or_else(|| format!("userscript {name} not found in the config or data userscripts dir"))
}

/// Start `cmd`; a thread waits for it, reports a failure (last stderr line) or
/// the output (`-o`), and cleans up the userscript dir.
fn run_child(
    mut cmd: Command,
    name: String,
    output: bool,
    dir: Option<PathBuf>,
) -> Result<(), String> {
    cmd.stderr(Stdio::piped());
    if output {
        cmd.stdout(Stdio::piped());
    }
    let child = cmd.spawn().map_err(|e| format!("{name}: {e}"))?;
    std::thread::spawn(move || {
        // reads stdout and stderr together: one at a time deadlocks once the other pipe fills
        let (status, out, err) = match child.wait_with_output() {
            Ok(o) => (
                Ok(o.status),
                String::from_utf8_lossy(&o.stdout).into_owned(),
                String::from_utf8_lossy(&o.stderr).into_owned(),
            ),
            Err(e) => (Err(e), String::new(), String::new()),
        };
        if let Some(dir) = &dir {
            // Wake the FIFO reader so it stops (it then removes the dir).
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .write(true)
                .open(dir.join("fifo"))
            {
                let _ = f.write_all(b"\0\n");
            }
        }
        match status {
            Ok(s) if !s.success() => {
                let last = err
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or("")
                    .to_string();
                let text = format!("{name} exited with {}: {last}", s.code().unwrap_or(-1));
                host::on_ui(move || host::message("error", &text));
            }
            Err(e) => {
                let text = format!("{name}: {e}");
                host::on_ui(move || host::message("error", &text));
            }
            _ if output => host::on_ui(move || host::show_text(&out)),
            _ => {}
        }
    });
    Ok(())
}

/// Each line written to the FIFO runs as a command in the tab that started the script.
/// Opened read+write so writers closing it don't end the loop; `\0` from the waiter does.
fn read_fifo(fifo: PathBuf, tab: u64) {
    std::thread::spawn(move || {
        let Ok(f) = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&fifo)
        else {
            return;
        };
        for line in BufReader::new(f).lines() {
            let Ok(line) = line else { break };
            if line == "\0" {
                break;
            }
            if !line.trim().is_empty() {
                host::on_ui(move || {
                    if !host::run_in_tab(tab, &line) {
                        host::message("warning", &format!("userscript tab closed, dropped: {line}"));
                    }
                });
            }
        }
        if let Some(dir) = fifo.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    });
}

fn tempdir() -> Result<PathBuf, String> {
    let base = std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, PathBuf::from);
    for i in 0..100u32 {
        let d = base.join(format!(
            "{}-userscript-{}-{i}",
            crate::paths::app(),
            std::process::id()
        ));
        if std::fs::create_dir(&d).is_ok() {
            return Ok(d);
        }
    }
    Err("can't create a userscript dir".into())
}

fn mkfifo(path: &Path) -> Result<(), String> {
    use std::os::unix::ffi::OsStrExt;
    unsafe extern "C" {
        fn mkfifo(path: *const std::ffi::c_char, mode: u32) -> i32;
    }
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    // SAFETY: c is a valid NUL-terminated path for the duration of the call.
    if unsafe { mkfifo(c.as_ptr(), 0o600) } == 0 {
        Ok(())
    } else {
        Err(format!(
            "mkfifo {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ))
    }
}

pub fn expand_home(p: &str) -> String {
    match p.strip_prefix('~') {
        Some(rest) => format!("{}{rest}", std::env::var("HOME").unwrap_or_default()),
        None => p.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fifo_and_finding_scripts() {
        let d = tempdir().unwrap();
        mkfifo(&d.join("fifo")).unwrap();
        use std::os::unix::fs::FileTypeExt;
        assert!(std::fs::metadata(d.join("fifo"))
            .unwrap()
            .file_type()
            .is_fifo());
        std::fs::remove_dir_all(&d).unwrap();
        assert!(find_userscript("/bin/sh").is_ok());
        assert!(find_userscript("no-such-script-xyz").is_err());
    }
}
