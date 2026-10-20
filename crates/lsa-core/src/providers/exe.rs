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

        // A directly-spawned, un-sandboxed game gets the hook DLL loaded at startup: create it
        // suspended, inject, resume. Sandboxed games are injected by Sandboxie's `InjectDll`.
        let startup_inject =
            cfg!(windows) && req.profile.hooks.inject && req.box_name.is_none() && !req.dry_run;
        if startup_inject {
            let hooks = crate::isolation::sandboxie::HookDlls::locate();
            let spawn = crate::inject::InjectSpawn {
                program: &program,
                args: &args,
                cwd: cwd.as_deref(),
                env: &req.env,
            };
            let injected = crate::inject::spawn_suspended_and_inject(&spawn, &hooks)?;
            return Ok(Launched {
                root_pid: Some(injected.pid),
                child: None,
                exe_name,
                preexisting_pids: preexisting,
                title_regex: None,
                command_line: crate::inject::command_line(&program, &args),
                notes: injected.notes,
            });
        }

        let (child, command_line) = spawn(
            &program,
            &args,
            cwd.as_deref(),
            &req.env,
            req.box_name,
            req.dry_run,
        )?;
        let mut notes = Vec::new();
        if req.dry_run && req.profile.hooks.inject && req.box_name.is_none() {
            notes.push("would create suspended, inject the hook DLL, then resume".into());
        }
        Ok(Launched {
            root_pid: child.as_ref().map(|c| c.id()),
            child,
            exe_name,
            preexisting_pids: preexisting,
            title_regex: None,
            command_line,
            notes,
        })
    }
}
