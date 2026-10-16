//! Launch providers: how one instance of a game is started.
//!
//! A provider turns a [`LaunchRequest`] into a running process (or, in a dry run, into the
//! command line it would have run). It can also contribute per-instance config patches that
//! the session applies, with backups, before launching.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result};
use lsa_proto::PixelRect;

use crate::profile::{GameProfile, PatchSection, Placeholders, ProviderKind};
use crate::window;

pub mod exe;
pub mod prism;
pub mod steam;
pub mod steam_emu;

/// Everything a provider needs to start one instance.
pub struct LaunchRequest<'a> {
    pub profile: &'a GameProfile,
    pub session_id: &'a str,
    pub player: u8,
    pub nickname: &'a str,
    pub viewport: PixelRect,
    /// Directory holding this instance's `instance.json`.
    pub instance_dir: &'a Path,
    /// Environment for the game process (isolation redirects, `LSA_INSTANCE_DIR`, profile env).
    pub env: BTreeMap<String, String>,
    pub placeholders: &'a Placeholders,
    /// Sandboxie box to run in, if any.
    pub box_name: Option<&'a str>,
    /// Only compute the command line; do not spawn anything.
    pub dry_run: bool,
}

/// A config file a provider wants patched before launch.
#[derive(Debug, Clone)]
pub struct PreparedPatch {
    pub path: PathBuf,
    pub spec: PatchSection,
}

/// Result of starting an instance.
#[derive(Debug, Default)]
pub struct Launched {
    /// Process we spawned, when the game is (a descendant of) it.
    pub root_pid: Option<u32>,
    pub child: Option<Child>,
    /// Executable name to look for when the game is started by something we did not spawn.
    pub exe_name: Option<String>,
    /// Processes with that exe name that already existed before the launch.
    pub preexisting_pids: HashSet<u32>,
    /// Provider default for recognising the main window (profile settings win).
    pub title_regex: Option<String>,
    pub command_line: String,
    pub notes: Vec<String>,
}

pub trait Provider {
    fn name(&self) -> &'static str;

    /// Config patches to apply (with backups) before `launch`.
    fn prepare(&self, _req: &LaunchRequest<'_>) -> Result<Vec<PreparedPatch>> {
        Ok(Vec::new())
    }

    fn launch(&self, req: &LaunchRequest<'_>) -> Result<Launched>;
}

pub fn provider_for(kind: ProviderKind) -> Box<dyn Provider> {
    match kind {
        ProviderKind::Exe => Box::new(exe::ExeProvider),
        ProviderKind::Steam => Box::new(steam::SteamProvider),
        ProviderKind::Prism => Box::new(prism::PrismProvider),
        ProviderKind::SteamEmu => Box::new(steam_emu::SteamEmuProvider),
    }
}

/// Expand `%VAR%` and `${placeholder}` in a path from a profile.
pub(crate) fn expand_path(text: &str, placeholders: &Placeholders) -> Result<PathBuf> {
    let with_env = crate::profile::expand_env_refs(text, crate::paths::env_lookup);
    Ok(PathBuf::from(placeholders.expand(&with_env)?))
}

/// Quote a command line for display.
pub(crate) fn render_command(program: &Path, args: &[String]) -> String {
    let quote = |s: &str| {
        if s.contains(' ') || s.is_empty() {
            format!("\"{s}\"")
        } else {
            s.to_string()
        }
    };
    let mut parts = vec![quote(&program.to_string_lossy())];
    parts.extend(args.iter().map(|a| quote(a)));
    parts.join(" ")
}

/// Process ids currently running `exe_name`, so new ones can be told apart later.
pub(crate) fn pids_running(exe_name: &str) -> HashSet<u32> {
    window::list_processes()
        .into_iter()
        .filter(|p| p.exe.eq_ignore_ascii_case(exe_name))
        .map(|p| p.pid)
        .collect()
}

/// Spawn `program args` (wrapped in Sandboxie's `Start.exe` when `box_name` is set), or in a
/// dry run just return the command line.
pub(crate) fn spawn(
    program: &Path,
    args: &[String],
    cwd: Option<&Path>,
    env: &BTreeMap<String, String>,
    box_name: Option<&str>,
    dry_run: bool,
) -> Result<(Option<Child>, String)> {
    let (program, args): (PathBuf, Vec<String>) = match box_name {
        Some(name) => {
            let sandboxie = match crate::isolation::sandboxie::Sandboxie::detect() {
                Ok(sb) => sb,
                Err(e) if dry_run => {
                    let _ = e;
                    crate::isolation::sandboxie::Sandboxie::nominal()
                }
                Err(e) => return Err(e),
            };
            sandboxie.wrap(name, env, program, args)
        }
        None => (program.to_path_buf(), args.to_vec()),
    };
    let command_line = render_command(&program, &args);
    if dry_run {
        return Ok((None, command_line));
    }
    let mut cmd = Command::new(&program);
    cmd.args(&args)
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let child = cmd
        .spawn()
        .with_context(|| format!("cannot start {command_line}"))?;
    Ok((Some(child), command_line))
}
