//! Where LSA keeps its data, and where profiles are looked up.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Per-user data directory: `%APPDATA%\LSA` on Windows, `~/.config/lsa` elsewhere.
pub fn data_dir() -> PathBuf {
    if cfg!(windows) {
        if let Some(a) = std::env::var_os("APPDATA") {
            return PathBuf::from(a).join("LSA");
        }
    }
    if let Some(x) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(x).join("lsa");
    }
    if let Some(h) = std::env::var_os("HOME") {
        return PathBuf::from(h).join(".config").join("lsa");
    }
    PathBuf::from("lsa-data")
}

pub fn sessions_dir() -> PathBuf {
    data_dir().join("sessions")
}

pub fn layouts_dir() -> PathBuf {
    data_dir().join("layouts")
}

/// Machine-wide folder that Sandboxie boxes get access to (`C:\ProgramData\LSA\boxes`).
pub fn boxes_root() -> PathBuf {
    if cfg!(windows) {
        if let Some(p) = std::env::var_os("ProgramData") {
            return PathBuf::from(p).join("LSA").join("boxes");
        }
    }
    data_dir().join("boxes")
}

/// Profile directories in priority order: next to the executable, the current directory,
/// then the user data directory (which overrides the others).
pub fn profile_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.join("profiles"));
            // Running from `target/debug` inside the repo: use the repo's profiles.
            if let Some(repo) = dir
                .ancestors()
                .find(|a| a.join("profiles").is_dir() && a.join("Cargo.toml").is_file())
            {
                dirs.push(repo.join("profiles"));
            }
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        dirs.push(cwd.join("profiles"));
    }
    dirs.push(data_dir().join("profiles"));
    dirs.dedup();
    dirs
}

/// Time-based session id, unique enough for one machine.
pub fn new_session_id() -> String {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("{ms:x}-{:x}", std::process::id())
}

/// Environment lookup used for `%VAR%` expansion.
pub fn env_lookup(name: &str) -> Option<String> {
    std::env::var(name).ok()
}
