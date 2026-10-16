//! File-based exchange with the hook DLL: `instance.json` out, `state.json` back.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use lsa_proto::{InstanceConfig, InstanceState, INSTANCE_FILE, STATE_FILE};

pub fn instance_config_path(dir: &Path) -> PathBuf {
    dir.join(INSTANCE_FILE)
}

pub fn state_path(dir: &Path) -> PathBuf {
    dir.join(STATE_FILE)
}

/// Write `instance.json` atomically (temp file + rename) so the hook never reads a torn file.
pub fn write_instance_config(dir: &Path, cfg: &InstanceConfig) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    let json = cfg
        .to_json_pretty()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let tmp = dir.join(format!("{INSTANCE_FILE}.tmp"));
    fs::write(&tmp, json)?;
    fs::rename(&tmp, instance_config_path(dir))
}

pub fn read_instance_config(dir: &Path) -> io::Result<InstanceConfig> {
    let text = fs::read_to_string(instance_config_path(dir))?;
    InstanceConfig::from_json_str(&text).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// `state.json` if the hook has written one yet.
pub fn read_instance_state(dir: &Path) -> Option<InstanceState> {
    let text = fs::read_to_string(state_path(dir)).ok()?;
    InstanceState::from_json_str(&text).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsa_proto::{ControllerAssignment, HookFlags, PixelRect, PROTO_VERSION};

    #[test]
    fn config_round_trips_through_disk() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = InstanceConfig {
            version: PROTO_VERSION,
            session_id: "s".into(),
            player: 1,
            nickname: "P1".into(),
            target_exe: "x.exe".into(),
            controller: ControllerAssignment::XInputSlot { slot: 0 },
            viewport: PixelRect::new(0, 0, 1, 1),
            hooks: HookFlags::default(),
            window_title_regex: None,
        };
        write_instance_config(dir.path(), &cfg).unwrap();
        assert_eq!(read_instance_config(dir.path()).unwrap(), cfg);
        assert!(read_instance_state(dir.path()).is_none());
        assert!(!dir.path().join("instance.json.tmp").exists());
    }
}
