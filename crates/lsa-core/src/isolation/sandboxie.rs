//! Sandboxie-Plus driver: one Standard box per player slot (`LSA_P2`..`LSA_P4`).
//!
//! The box gives its player a private Steam install/login, private saves and config, and
//! private named kernel objects, and injects our hook DLL into every process in the box via
//! `InjectDll`/`InjectDll64`. Config and state are exchanged through the box folder under
//! `C:\ProgramData\LSA\boxes\<box>\`, exposed with `OpenPipePath` (which, unlike
//! `OpenFilePath`, also applies to executables that live inside the box, such as a boxed
//! Steam's games).
//!
//! Standard (yellow) boxes are required: App Compartment boxes break Windows credential
//! storage, so Steam asks for a password and 2FA on every start.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, Context, Result};

pub struct Sandboxie {
    pub start_exe: PathBuf,
    pub sbieini_exe: PathBuf,
}

/// Locations of the built hook DLLs, if present next to the running executable.
#[derive(Debug, Clone, Default)]
pub struct HookDlls {
    pub x64: Option<PathBuf>,
    pub x86: Option<PathBuf>,
}

impl HookDlls {
    pub const X64_NAME: &'static str = "lsa_hook64.dll";
    pub const X86_NAME: &'static str = "lsa_hook32.dll";

    pub fn locate() -> HookDlls {
        let mut dlls = HookDlls::default();
        let Ok(exe) = std::env::current_exe() else {
            return dlls;
        };
        let Some(dir) = exe.parent() else { return dlls };
        for candidate in [dir.to_path_buf(), dir.join("hooks")] {
            let x64 = candidate.join(Self::X64_NAME);
            let x86 = candidate.join(Self::X86_NAME);
            if dlls.x64.is_none() && x64.is_file() {
                dlls.x64 = Some(x64);
            }
            if dlls.x86.is_none() && x86.is_file() {
                dlls.x86 = Some(x86);
            }
        }
        dlls
    }
}

impl Sandboxie {
    pub fn box_name(player: u8) -> String {
        format!("LSA_P{player}")
    }

    pub fn box_dir(player: u8) -> PathBuf {
        crate::paths::boxes_root().join(Self::box_name(player))
    }

    /// The default install location, for dry runs on machines without Sandboxie.
    pub fn nominal() -> Sandboxie {
        let dir = PathBuf::from(r"C:\Program Files\Sandboxie-Plus");
        Sandboxie {
            start_exe: dir.join("Start.exe"),
            sbieini_exe: dir.join("SbieIni.exe"),
        }
    }

    /// Find `Start.exe` and `SbieIni.exe`.
    pub fn detect() -> Result<Sandboxie> {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(custom) = std::env::var_os("LSA_SANDBOXIE_DIR") {
            candidates.push(PathBuf::from(custom));
        }
        for var in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
            if let Some(pf) = std::env::var_os(var) {
                candidates.push(PathBuf::from(&pf).join("Sandboxie-Plus"));
                candidates.push(PathBuf::from(&pf).join("Sandboxie"));
            }
        }
        candidates.push(PathBuf::from(r"C:\Program Files\Sandboxie-Plus"));
        for dir in candidates {
            let start = dir.join("Start.exe");
            let ini = dir.join("SbieIni.exe");
            if start.is_file() && ini.is_file() {
                return Ok(Sandboxie {
                    start_exe: start,
                    sbieini_exe: ini,
                });
            }
        }
        Err(anyhow!(
            "Sandboxie-Plus not found (looked in Program Files); install it from https://sandboxie-plus.com or set LSA_SANDBOXIE_DIR"
        ))
    }

    fn sbieini(&self, args: &[&str]) -> Result<String> {
        let out = Command::new(&self.sbieini_exe)
            .args(args)
            .output()
            .with_context(|| {
                format!("running {} {}", self.sbieini_exe.display(), args.join(" "))
            })?;
        if !out.status.success() {
            return Err(anyhow!(
                "SbieIni {} failed ({}): {}",
                args.join(" "),
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    /// The `SbieIni.exe` commands that define a player box; also what the docs show users.
    pub fn box_commands(name: &str, box_dir: &Path, hooks: &HookDlls) -> Vec<Vec<String>> {
        let dir = format!(
            "{}\\",
            box_dir.to_string_lossy().trim_end_matches(['\\', '/'])
        );
        let mut cmds = vec![
            vec!["set".into(), name.into(), "Enabled".into(), "y".into()],
            vec![
                "set".into(),
                name.into(),
                "Comment".into(),
                format!("LocalSplitscreenAnything {name}"),
            ],
            vec![
                "set".into(),
                name.into(),
                "OpenPipePath".into(),
                dir.clone(),
            ],
        ];
        if let Some(x64) = &hooks.x64 {
            let dest = format!("{dir}{}", HookDlls::X64_NAME);
            let _ = x64;
            cmds.push(vec!["set".into(), name.into(), "InjectDll64".into(), dest]);
        }
        if let Some(x86) = &hooks.x86 {
            let dest = format!("{dir}{}", HookDlls::X86_NAME);
            let _ = x86;
            cmds.push(vec!["set".into(), name.into(), "InjectDll".into(), dest]);
        }
        cmds
    }

    /// Create or update the box, copy the hook DLLs into its folder, and reload the driver.
    pub fn ensure_box(&self, name: &str, box_dir: &Path, hooks: &HookDlls) -> Result<()> {
        std::fs::create_dir_all(box_dir)
            .with_context(|| format!("creating {}", box_dir.display()))?;
        if let Some(x64) = &hooks.x64 {
            std::fs::copy(x64, box_dir.join(HookDlls::X64_NAME))?;
        }
        if let Some(x86) = &hooks.x86 {
            std::fs::copy(x86, box_dir.join(HookDlls::X86_NAME))?;
        }
        for cmd in Self::box_commands(name, box_dir, hooks) {
            let args: Vec<&str> = cmd.iter().map(String::as_str).collect();
            self.sbieini(&args)?;
        }
        let _ = Command::new(&self.start_exe).arg("/reload").status();
        Ok(())
    }

    /// Wrap `program args` so it runs inside `box_name` with `env` set.
    pub fn wrap(
        &self,
        box_name: &str,
        env: &BTreeMap<String, String>,
        program: &Path,
        args: &[String],
    ) -> (PathBuf, Vec<String>) {
        let mut out = vec![format!("/box:{box_name}"), "/silent".to_string()];
        for (k, v) in env {
            out.push(format!("/env:{k}={v}"));
        }
        out.push(program.to_string_lossy().to_string());
        out.extend(args.iter().cloned());
        (self.start_exe.clone(), out)
    }

    /// Kill every process in the box.
    pub fn terminate_box(&self, box_name: &str) -> Result<()> {
        let status = Command::new(&self.start_exe)
            .arg(format!("/box:{box_name}"))
            .arg("/terminate")
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(anyhow!(
                "Start.exe /terminate for {box_name} exited with {status}"
            ))
        }
    }

    /// Process ids running in the box.
    pub fn list_pids(&self, box_name: &str) -> Result<Vec<u32>> {
        let out = Command::new(&self.start_exe)
            .arg(format!("/box:{box_name}"))
            .arg("/listpids")
            .output()?;
        let text = String::from_utf8_lossy(&out.stdout);
        // First line is the count, then one pid per line.
        Ok(text
            .lines()
            .skip(1)
            .filter_map(|l| l.trim().parse().ok())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_names_and_commands() {
        assert_eq!(Sandboxie::box_name(2), "LSA_P2");
        let hooks = HookDlls {
            x64: Some(PathBuf::from("C:/build/lsa_hook64.dll")),
            x86: None,
        };
        let cmds = Sandboxie::box_commands(
            "LSA_P2",
            Path::new(r"C:\ProgramData\LSA\boxes\LSA_P2"),
            &hooks,
        );
        assert_eq!(cmds[0], vec!["set", "LSA_P2", "Enabled", "y"]);
        assert_eq!(cmds[2][2], "OpenPipePath");
        assert_eq!(cmds[2][3], r"C:\ProgramData\LSA\boxes\LSA_P2\");
        assert_eq!(cmds[3][2], "InjectDll64");
        assert_eq!(
            cmds[3][3],
            r"C:\ProgramData\LSA\boxes\LSA_P2\lsa_hook64.dll"
        );
        assert_eq!(cmds.len(), 4);
    }

    #[test]
    fn wrap_builds_start_exe_command() {
        let sb = Sandboxie {
            start_exe: PathBuf::from(r"C:\SB\Start.exe"),
            sbieini_exe: PathBuf::from(r"C:\SB\SbieIni.exe"),
        };
        let mut env = BTreeMap::new();
        env.insert("LSA_INSTANCE_DIR".to_string(), r"C:\x".to_string());
        let (prog, args) = sb.wrap(
            "LSA_P2",
            &env,
            Path::new(r"C:\Steam\steam.exe"),
            &["-applaunch".into(), "1".into()],
        );
        assert_eq!(prog, PathBuf::from(r"C:\SB\Start.exe"));
        assert_eq!(
            args,
            vec![
                "/box:LSA_P2",
                "/silent",
                r"/env:LSA_INSTANCE_DIR=C:\x",
                r"C:\Steam\steam.exe",
                "-applaunch",
                "1"
            ]
        );
    }
}
