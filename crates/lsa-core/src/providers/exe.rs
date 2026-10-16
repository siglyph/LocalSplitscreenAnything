//! Plain executable provider.

use anyhow::{anyhow, Result};

use super::{expand_path, pids_running, spawn, LaunchRequest, Launched, Provider};

pub struct ExeProvider;

impl Provider for ExeProvider {
    fn name(&self) -> &'static str {
        "exe"
    }

    fn launch(&self, req: &LaunchRequest<'_>) -> Result<Launched> {
        let game = &req.profile.game;
        let path_text = game
            .path
            .as_deref()
            .ok_or_else(|| anyhow!("exe provider needs game.path"))?;
        let program = expand_path(path_text, req.placeholders)?;
        let args = req.placeholders.expand_all(&req.profile.launch.args)?;
        let cwd = match &game.working_dir {
            Some(wd) => Some(expand_path(wd, req.placeholders)?),
            None => program.parent().map(|p| p.to_path_buf()),
        };
        let exe_name = game
            .executable
            .clone()
            .or_else(|| program.file_name().map(|n| n.to_string_lossy().to_string()));
        let preexisting = exe_name.as_deref().map(pids_running).unwrap_or_default();
        let (child, command_line) = spawn(
            &program,
            &args,
            cwd.as_deref(),
            &req.env,
            req.box_name,
            req.dry_run,
        )?;
        Ok(Launched {
            root_pid: child.as_ref().map(|c| c.id()),
            child,
            exe_name,
            preexisting_pids: preexisting,
            title_regex: None,
            command_line,
            notes: Vec::new(),
        })
    }
}
