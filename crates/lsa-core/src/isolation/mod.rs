//! Isolation backends: what keeps instances from stepping on each other's saves, config and
//! single-instance checks.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::profile::{GameProfile, IsolationMode, ProviderKind};

pub mod env;
pub mod sandboxie;

/// What the session must do for one instance.
#[derive(Debug, Clone, Default)]
pub struct IsolationPlan {
    /// Environment overrides for the game process.
    pub env: BTreeMap<String, String>,
    /// Sandboxie box the instance runs in.
    pub box_name: Option<String>,
    /// Directory the instance's `instance.json` must live in (for boxed instances this is the
    /// box's shared folder, where the hook DLL copy for that box also lives).
    pub instance_dir: Option<PathBuf>,
    pub notes: Vec<String>,
}

/// Decide and (unless `dry_run`) prepare isolation for `player`.
pub fn prepare(
    profile: &GameProfile,
    player: u8,
    default_instance_dir: &Path,
    dry_run: bool,
) -> Result<IsolationPlan> {
    let mut plan = IsolationPlan::default();
    match profile.isolation.mode {
        IsolationMode::None => {}
        IsolationMode::Env => {
            plan.env = env::prepare(
                default_instance_dir,
                &profile.isolation.env_redirect,
                dry_run,
            )?;
            plan.notes
                .push("per-instance APPDATA/LOCALAPPDATA/USERPROFILE/TEMP".into());
        }
        IsolationMode::Sandboxie => {
            let host_slot = profile.game.provider == ProviderKind::Steam
                && player == 1
                && profile
                    .steam
                    .as_ref()
                    .map(|s| s.host_slot_unsandboxed)
                    .unwrap_or(true);
            if host_slot {
                plan.notes
                    .push("player 1 runs unsandboxed on the host".into());
            } else {
                let name = sandboxie::Sandboxie::box_name(player);
                let dir = sandboxie::Sandboxie::box_dir(player);
                if !dry_run {
                    let sb = sandboxie::Sandboxie::detect()?;
                    let hooks = sandboxie::HookDlls::locate();
                    sb.ensure_box(&name, &dir, &hooks)?;
                }
                plan.notes.push(format!("Sandboxie box {name}"));
                plan.box_name = Some(name);
                plan.instance_dir = Some(dir);
            }
        }
    }
    Ok(plan)
}
