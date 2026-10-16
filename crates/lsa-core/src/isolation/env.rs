//! Lightweight isolation through environment variables. Games (and launchers such as the
//! vanilla Minecraft launcher) that resolve `%APPDATA%` and friends from the environment get a
//! private profile folder per instance. Games that ask the shell for known folders ignore this;
//! those need Sandboxie.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

/// Build the environment overrides for `instance_dir`, creating the folders unless `dry_run`.
pub fn prepare(
    instance_dir: &Path,
    vars: &[String],
    dry_run: bool,
) -> io::Result<BTreeMap<String, String>> {
    let profile = instance_dir.join("profile");
    let roaming = profile.join("AppData").join("Roaming");
    let local = profile.join("AppData").join("Local");
    let temp = profile.join("Temp");
    let mut env = BTreeMap::new();
    for var in vars {
        let dir = match var.to_ascii_uppercase().as_str() {
            "APPDATA" => roaming.clone(),
            "LOCALAPPDATA" => local.clone(),
            "USERPROFILE" | "HOME" => profile.clone(),
            "TEMP" | "TMP" => temp.clone(),
            other => profile.join(other),
        };
        if !dry_run {
            fs::create_dir_all(&dir)?;
        }
        env.insert(var.clone(), dir.to_string_lossy().to_string());
    }
    Ok(env)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_known_vars_and_creates_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let vars: Vec<String> = ["APPDATA", "LOCALAPPDATA", "USERPROFILE", "TEMP", "CUSTOM"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let env = prepare(dir.path(), &vars, false).unwrap();
        assert!(env["APPDATA"].ends_with("Roaming"));
        assert!(env["LOCALAPPDATA"].ends_with("Local"));
        assert!(env["USERPROFILE"].ends_with("profile"));
        assert!(env["TEMP"].ends_with("Temp"));
        assert!(env["CUSTOM"].ends_with("CUSTOM"));
        assert!(dir
            .path()
            .join("profile")
            .join("AppData")
            .join("Roaming")
            .is_dir());
        let dry = prepare(&dir.path().join("dry"), &vars, true).unwrap();
        assert_eq!(dry.len(), 5);
        assert!(!dir.path().join("dry").exists());
    }
}
