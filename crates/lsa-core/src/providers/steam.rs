//! Real Steam client per player.
//!
//! Player 1 normally runs on the host's own Steam. Every other player runs inside a Sandboxie
//! box that holds a second Steam install logged into that player's account; the box name is
//! passed in by the session and the command is wrapped in Sandboxie's `Start.exe`. Steam
//! starts the game itself, so the game process is found afterwards by executable name.

use std::path::PathBuf;

use anyhow::{anyhow, Result};

use super::{expand_path, pids_running, spawn, LaunchRequest, Launched, Provider};
use crate::profile::SteamSection;

pub struct SteamProvider;

/// Locate the host's `steam.exe` (a nominal path in dry runs when it is not installed).
pub fn find_steam_exe(
    section: Option<&SteamSection>,
    placeholders: &crate::profile::Placeholders,
    dry_run: bool,
) -> Result<PathBuf> {
    if let Some(p) = section.and_then(|s| s.steam_exe.as_deref()) {
        return expand_path(p, placeholders);
    }
    if let Some(p) = registry_steam_exe() {
        if p.is_file() {
            return Ok(p);
        }
    }
    for var in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Some(pf) = std::env::var_os(var) {
            let p = PathBuf::from(pf).join("Steam").join("steam.exe");
            if p.is_file() {
                return Ok(p);
            }
        }
    }
    if dry_run {
        return Ok(PathBuf::from(r"C:\Program Files (x86)\Steam\steam.exe"));
    }
    Err(anyhow!("steam.exe not found; set [steam].steam_exe"))
}

#[cfg(windows)]
fn registry_steam_exe() -> Option<PathBuf> {
    use std::ffi::c_void;
    use windows::core::w;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ};

    let mut buf = [0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Valve\\Steam"),
            w!("SteamExe"),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut c_void),
            Some(&mut size),
        )
    };
    if status.is_err() {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    let s = String::from_utf16_lossy(&buf[..end]).replace('/', "\\");
    (!s.is_empty()).then(|| PathBuf::from(s))
}

#[cfg(not(windows))]
fn registry_steam_exe() -> Option<PathBuf> {
    None
}

impl Provider for SteamProvider {
    fn name(&self) -> &'static str {
        "steam"
    }

    fn launch(&self, req: &LaunchRequest<'_>) -> Result<Launched> {
        let game = &req.profile.game;
        let appid = game
            .steam_appid
            .ok_or_else(|| anyhow!("steam provider needs game.steam_appid"))?;
        let exe_name = game.executable.clone().ok_or_else(|| {
            anyhow!("steam provider needs game.executable to find the game process")
        })?;
        let steam_exe = find_steam_exe(req.profile.steam.as_ref(), req.placeholders, req.dry_run)?;
        let mut args = vec!["-applaunch".to_string(), appid.to_string()];
        args.extend(req.placeholders.expand_all(&req.profile.launch.args)?);
        let preexisting = pids_running(&exe_name);
        let (child, command_line) =
            spawn(&steam_exe, &args, None, &req.env, req.box_name, req.dry_run)?;
        let mut notes =
            vec!["Steam starts the game itself; the game process is found by exe name".to_string()];
        match req.box_name {
            Some(b) => notes.push(format!(
                "runs inside Sandboxie box {b}; that box's Steam must be installed and logged in"
            )),
            None => notes.push("runs on the host's Steam client".into()),
        }
        Ok(Launched {
            root_pid: None,
            child,
            exe_name: Some(exe_name),
            preexisting_pids: preexisting,
            title_regex: None,
            command_line,
            notes,
        })
    }
}
