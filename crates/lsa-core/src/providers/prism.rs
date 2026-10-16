//! Minecraft Java through Prism Launcher.
//!
//! Prism already supports several instances and several accounts, so this provider only has
//! to pick the right instance/account per player, make the game run windowed at the viewport
//! size, keep it running when unfocused, and start it:
//! `prismlauncher --launch <instance> --profile <account> [--server host:port]`.
//!
//! Known quirk: if Prism is already running, `--launch` is handed to the running process and
//! our child exits at once, so the session falls back to finding the newest `Minecraft*`
//! window that no other instance has claimed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use super::{expand_path, spawn, LaunchRequest, Launched, PreparedPatch, Provider};
use crate::profile::{PatchFormat, PatchSection, PrismSection};

pub struct PrismProvider;

/// Default title regex for the Minecraft window (`Minecraft* 1.21.1`, `Minecraft 1.20.4 - Singleplayer`).
pub const MINECRAFT_TITLE_REGEX: &str = r"^Minecraft\*? ";

fn section<'a>(req: &LaunchRequest<'a>) -> Result<&'a PrismSection> {
    req.profile
        .prism
        .as_ref()
        .ok_or_else(|| anyhow!("prism provider needs a [prism] section"))
}

fn instance_for(section: &PrismSection, player: u8) -> Result<&str> {
    section.instances.get(player as usize - 1).map(String::as_str).ok_or_else(|| {
        anyhow!(
            "[prism].instances lists {} instance(s) but player {} needs one; add a Prism instance per player",
            section.instances.len(),
            player
        )
    })
}

/// Locate `prismlauncher.exe`. In a dry run a missing launcher is not an error: a nominal
/// path is returned so the command line can still be shown.
pub fn find_launcher(
    section: &PrismSection,
    placeholders: &crate::profile::Placeholders,
    dry_run: bool,
) -> Result<PathBuf> {
    if let Some(p) = &section.launcher {
        let path = expand_path(p, placeholders)?;
        if path.is_file() {
            return Ok(path);
        }
        return Err(anyhow!(
            "[prism].launcher = {} does not exist",
            path.display()
        ));
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(
            PathBuf::from(&local)
                .join("Programs")
                .join("PrismLauncher")
                .join("prismlauncher.exe"),
        );
    }
    for var in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(pf) = std::env::var_os(var) {
            candidates.push(
                PathBuf::from(&pf)
                    .join("PrismLauncher")
                    .join("prismlauncher.exe"),
            );
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            candidates.push(dir.join("prismlauncher.exe"));
            candidates.push(dir.join("prismlauncher"));
        }
    }
    match candidates.into_iter().find(|c| c.is_file()) {
        Some(found) => Ok(found),
        None if dry_run => Ok(PathBuf::from("prismlauncher.exe")),
        None => Err(anyhow!(
            "Prism Launcher not found; set [prism].launcher to the path of prismlauncher.exe"
        )),
    }
}

/// Prism's data directory (the one containing `instances/`).
pub fn find_data_dir(
    section: &PrismSection,
    launcher: &Path,
    placeholders: &crate::profile::Placeholders,
    dry_run: bool,
) -> Result<PathBuf> {
    if let Some(d) = &section.data_dir {
        return expand_path(d, placeholders);
    }
    // Portable install: instances live next to the executable.
    if let Some(dir) = launcher.parent() {
        if dir.join("instances").is_dir() {
            return Ok(dir.to_path_buf());
        }
    }
    if let Some(appdata) = std::env::var_os("APPDATA") {
        let d = PathBuf::from(appdata).join("PrismLauncher");
        if d.is_dir() {
            return Ok(d);
        }
    }
    if dry_run {
        return Ok(PathBuf::from("%APPDATA%").join("PrismLauncher"));
    }
    Err(anyhow!(
        "Prism data directory not found; set [prism].data_dir"
    ))
}

/// The `.minecraft`/`minecraft` folder of an instance (Prism renamed it over time).
pub fn minecraft_dir(instance_dir: &Path) -> PathBuf {
    let new = instance_dir.join("minecraft");
    let old = instance_dir.join(".minecraft");
    if new.is_dir() || !old.is_dir() {
        new
    } else {
        old
    }
}

fn kv_patch(path: &Path, format: PatchFormat, set: &[(&str, toml::Value)]) -> PreparedPatch {
    PreparedPatch {
        path: path.to_path_buf(),
        spec: PatchSection {
            file: path.to_string_lossy().to_string(),
            format,
            section: None,
            set: set
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect::<BTreeMap<_, _>>(),
            pattern: None,
            replace: None,
            create_if_missing: true,
        },
    }
}

impl Provider for PrismProvider {
    fn name(&self) -> &'static str {
        "prism"
    }

    fn prepare(&self, req: &LaunchRequest<'_>) -> Result<Vec<PreparedPatch>> {
        let section = section(req)?;
        let instance = instance_for(section, req.player)?;
        let mut patches = Vec::new();
        if !(section.patch_options || section.patch_instance_cfg) {
            return Ok(patches);
        }
        let launcher = find_launcher(section, req.placeholders, req.dry_run)?;
        let data_dir = find_data_dir(section, &launcher, req.placeholders, req.dry_run)?;
        let instance_dir = data_dir.join("instances").join(instance);
        if !instance_dir.is_dir() && !req.dry_run {
            return Err(anyhow!(
                "Prism instance folder {} does not exist",
                instance_dir.display()
            ));
        }
        let w = toml::Value::Integer(req.viewport.width as i64);
        let h = toml::Value::Integer(req.viewport.height as i64);
        if section.patch_options {
            patches.push(kv_patch(
                &minecraft_dir(&instance_dir).join("options.txt"),
                PatchFormat::KeyValueColon,
                &[
                    ("fullscreen", toml::Value::Boolean(false)),
                    ("pauseOnLostFocus", toml::Value::Boolean(false)),
                    ("overrideWidth", w.clone()),
                    ("overrideHeight", h.clone()),
                ],
            ));
        }
        if section.patch_instance_cfg {
            patches.push(kv_patch(
                &instance_dir.join("instance.cfg"),
                PatchFormat::KeyValueEquals,
                &[
                    ("OverrideWindow", toml::Value::Boolean(true)),
                    ("LaunchMaximized", toml::Value::Boolean(false)),
                    ("MinecraftWinWidth", w),
                    ("MinecraftWinHeight", h),
                ],
            ));
        }
        Ok(patches)
    }

    fn launch(&self, req: &LaunchRequest<'_>) -> Result<Launched> {
        let section = section(req)?;
        let instance = instance_for(section, req.player)?;
        let launcher = find_launcher(section, req.placeholders, req.dry_run)?;
        let mut args = vec!["--launch".to_string(), instance.to_string()];
        if let Some(account) = section.accounts.get(req.player as usize - 1) {
            args.push("--profile".into());
            args.push(req.placeholders.expand(account)?);
        }
        if let Some(server) = &section.server {
            args.push("--server".into());
            args.push(req.placeholders.expand(server)?);
        }
        args.extend(req.placeholders.expand_all(&req.profile.launch.args)?);
        let (child, command_line) =
            spawn(&launcher, &args, None, &req.env, req.box_name, req.dry_run)?;
        let mut notes = Vec::new();
        if section.accounts.get(req.player as usize - 1).is_none() {
            notes.push(format!(
                "player {} uses Prism's default account (no [prism].accounts entry)",
                req.player
            ));
        }
        notes.push("if Prism was already running, the launch is handed to it; the window is found by title".into());
        Ok(Launched {
            root_pid: child.as_ref().map(|c| c.id()),
            child,
            exe_name: None,
            preexisting_pids: Default::default(),
            title_regex: Some(MINECRAFT_TITLE_REGEX.to_string()),
            command_line,
            notes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minecraft_dir_prefers_new_name_unless_only_old_exists() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(minecraft_dir(dir.path()), dir.path().join("minecraft"));
        std::fs::create_dir(dir.path().join(".minecraft")).unwrap();
        assert_eq!(minecraft_dir(dir.path()), dir.path().join(".minecraft"));
        std::fs::create_dir(dir.path().join("minecraft")).unwrap();
        assert_eq!(minecraft_dir(dir.path()), dir.path().join("minecraft"));
    }

    #[test]
    fn title_regex_matches_real_titles() {
        let re = regex::Regex::new(MINECRAFT_TITLE_REGEX).unwrap();
        assert!(re.is_match("Minecraft* 1.21.1"));
        assert!(re.is_match("Minecraft 1.20.4 - Singleplayer"));
        assert!(!re.is_match("Prism Launcher 9.0"));
    }
}
