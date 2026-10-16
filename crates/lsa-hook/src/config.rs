//! Finding and reading `instance.json`, writing `state.json`.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use lsa_proto::{InstanceConfig, InstanceState, INSTANCE_DIR_ENV, INSTANCE_FILE, STATE_FILE};

/// `LSA_INSTANCE_DIR` if set, otherwise the folder the DLL was loaded from (each Sandboxie
/// box gets its own DLL copy next to its `instance.json`).
pub fn find_instance_dir(dll_path: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(INSTANCE_DIR_ENV) {
        let dir = PathBuf::from(dir);
        if dir.join(INSTANCE_FILE).is_file() {
            return Some(dir);
        }
    }
    let dir = dll_path?.parent()?.to_path_buf();
    dir.join(INSTANCE_FILE).is_file().then_some(dir)
}

pub fn load_config(dir: &Path) -> std::io::Result<InstanceConfig> {
    let text = std::fs::read_to_string(dir.join(INSTANCE_FILE))?;
    InstanceConfig::from_json_str(&text)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

pub fn save_state(dir: &Path, state: &InstanceState) -> std::io::Result<()> {
    let json = state
        .to_json_pretty()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let tmp = dir.join(format!("{STATE_FILE}.tmp"));
    std::fs::write(&tmp, json)?;
    std::fs::rename(tmp, dir.join(STATE_FILE))
}

pub fn modified(dir: &Path) -> Option<SystemTime> {
    std::fs::metadata(dir.join(INSTANCE_FILE))
        .ok()?
        .modified()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_config_next_to_dll_when_env_is_unset() {
        let dir = tempfile::tempdir().unwrap();
        std::env::remove_var(INSTANCE_DIR_ENV);
        let dll = dir.path().join("lsa_hook64.dll");
        assert!(find_instance_dir(Some(&dll)).is_none());
        std::fs::write(dir.path().join(INSTANCE_FILE), "{}").unwrap();
        assert_eq!(find_instance_dir(Some(&dll)).unwrap(), dir.path());
    }
}
