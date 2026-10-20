//! Orchestrator-side injection: start a game suspended, load the hook DLL before it runs any
//! of its own code, then resume it.
//!
//! The 64-bit orchestrator can start a thread at its own `LoadLibraryW` address only inside a
//! 64-bit target, so:
//! - 64-bit game: inject directly through [`lsa_inject`].
//! - 32-bit game: run the bundled `lsa-inject32.exe` helper, which injects from a matching
//!   32-bit process.
//!
//! Sandboxed instances never come here; Sandboxie's `InjectDll` loads the DLL for them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::isolation::sandboxie::HookDlls;

/// Name of the 32-bit injector helper the build places next to `lsa.exe`.
pub const INJECT32_EXE: &str = "lsa-inject32.exe";

/// What to launch and inject into.
#[derive(Debug, Clone)]
pub struct InjectSpawn<'a> {
    pub program: &'a Path,
    pub args: &'a [String],
    pub cwd: Option<&'a Path>,
    /// Environment overrides merged onto the orchestrator's own environment.
    pub env: &'a BTreeMap<String, String>,
}

/// A started, injected and resumed process.
#[derive(Debug, Clone, Default)]
pub struct InjectedProcess {
    pub pid: u32,
    pub notes: Vec<String>,
}

/// The injector helper binary, looked up next to the current executable.
pub fn inject32_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    [dir.join(INJECT32_EXE), dir.join("hooks").join(INJECT32_EXE)]
        .into_iter()
        .find(|candidate| candidate.is_file())
}

/// Render a command line display string (also used to build the real command line).
fn quote(s: &str) -> String {
    if s.is_empty() || s.contains([' ', '\t', '"']) {
        format!("\"{}\"", s.replace('"', "\\\""))
    } else {
        s.to_string()
    }
}

/// `program arg1 arg2 ...` as one Windows command line, argv[0] included.
pub fn command_line(program: &Path, args: &[String]) -> String {
    let mut parts = vec![quote(&program.to_string_lossy())];
    parts.extend(args.iter().map(|a| quote(a)));
    parts.join(" ")
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;

    use anyhow::{anyhow, bail, Context};
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Threading::{
        CreateProcessW, ResumeThread, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT,
        PROCESS_INFORMATION, STARTUPINFOW,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// A double-null-terminated UTF-16 `KEY=VALUE\0...` block, as `CreateProcessW` wants with
    /// `CREATE_UNICODE_ENVIRONMENT`. `lpEnvironment` replaces (not merges), so this is the
    /// orchestrator's whole environment with `overrides` applied on top.
    fn environment_block(overrides: &BTreeMap<String, String>) -> Vec<u16> {
        let mut env: BTreeMap<String, String> = std::env::vars().collect();
        for (k, v) in overrides {
            env.insert(k.clone(), v.clone());
        }
        let mut block: Vec<u16> = Vec::new();
        for (k, v) in &env {
            // Skip the odd `=C:=...` drive-cwd entries, which must not be set by hand.
            if k.is_empty() {
                continue;
            }
            block.extend(std::ffi::OsStr::new(&format!("{k}={v}")).encode_wide());
            block.push(0);
        }
        block.push(0);
        block
    }

    struct OwnedHandle(HANDLE);
    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            if !self.0.is_invalid() {
                unsafe {
                    let _ = CloseHandle(self.0);
                }
            }
        }
    }

    pub fn spawn_suspended_and_inject(
        spawn: &InjectSpawn<'_>,
        hooks: &HookDlls,
    ) -> Result<InjectedProcess> {
        let mut notes = Vec::new();
        let cmdline = command_line(spawn.program, spawn.args);
        let mut cmd_w = wide(&cmdline);
        let app_w = wide(&spawn.program.to_string_lossy());
        let cwd_w = spawn.cwd.map(|p| wide(&p.to_string_lossy()));
        let mut env_block = environment_block(spawn.env);

        let mut si = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            ..Default::default()
        };
        let mut pi = PROCESS_INFORMATION::default();

        unsafe {
            CreateProcessW(
                windows::core::PCWSTR(app_w.as_ptr()),
                Some(PWSTR(cmd_w.as_mut_ptr())),
                None,
                None,
                false,
                CREATE_SUSPENDED | CREATE_UNICODE_ENVIRONMENT,
                Some(env_block.as_mut_ptr() as *const c_void),
                cwd_w
                    .as_ref()
                    .map(|w| windows::core::PCWSTR(w.as_ptr()))
                    .unwrap_or(windows::core::PCWSTR::null()),
                &si as *const STARTUPINFOW,
                &mut pi as *mut PROCESS_INFORMATION,
            )
        }
        .with_context(|| format!("CreateProcessW suspended: {cmdline}"))?;

        let _ = &mut si;
        let process = OwnedHandle(pi.hProcess);
        let thread = OwnedHandle(pi.hThread);
        let pid = pi.dwProcessId;

        // Inject, then always resume so a failed injection does not leave a frozen game.
        let inject_result = inject(pid, hooks, &mut notes);
        unsafe {
            if ResumeThread(thread.0) == u32::MAX {
                let _ = &process; // keep handle alive until here
                return Err(anyhow!("ResumeThread failed for pid {pid}"));
            }
        }
        inject_result?;
        Ok(InjectedProcess { pid, notes })
    }

    fn inject(pid: u32, hooks: &HookDlls, notes: &mut Vec<String>) -> Result<()> {
        let is_wow64 = lsa_inject::process_is_wow64(pid).map_err(|e| anyhow!(e))?;
        if is_wow64 {
            let dll = hooks
                .x86
                .as_ref()
                .ok_or_else(|| anyhow!("32-bit game needs {} (not found)", HookDlls::X86_NAME))?;
            let helper = inject32_exe().ok_or_else(|| {
                anyhow!("32-bit game needs the {INJECT32_EXE} helper next to lsa.exe")
            })?;
            let status = std::process::Command::new(&helper)
                .arg(pid.to_string())
                .arg(dll)
                .status()
                .with_context(|| format!("running {}", helper.display()))?;
            if !status.success() {
                bail!("{INJECT32_EXE} failed to inject into pid {pid}");
            }
            notes.push(format!("injected {} via {INJECT32_EXE}", HookDlls::X86_NAME));
        } else {
            let dll = hooks
                .x64
                .as_ref()
                .ok_or_else(|| anyhow!("64-bit game needs {} (not found)", HookDlls::X64_NAME))?;
            lsa_inject::inject_into_pid(pid, dll).map_err(|e| anyhow!(e))?;
            notes.push(format!("injected {}", HookDlls::X64_NAME));
        }
        Ok(())
    }
}

#[cfg(windows)]
pub use imp::spawn_suspended_and_inject;

#[cfg(not(windows))]
pub fn spawn_suspended_and_inject(
    _spawn: &InjectSpawn<'_>,
    _hooks: &HookDlls,
) -> Result<InjectedProcess> {
    anyhow::bail!("DLL injection is only available on Windows")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_line_quotes_spaces_and_argv0() {
        let line = command_line(
            Path::new(r"C:\Games\My Game\game.exe"),
            &["-w".into(), "1920".into(), "-name".into(), "Player 1".into()],
        );
        assert_eq!(
            line,
            r#""C:\Games\My Game\game.exe" -w 1920 -name "Player 1""#
        );
    }

    #[test]
    fn command_line_leaves_simple_args_bare() {
        let line = command_line(Path::new("game.exe"), &["-windowed".into()]);
        assert_eq!(line, "game.exe -windowed");
    }
}
