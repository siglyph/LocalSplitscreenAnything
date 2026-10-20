//! `lsa-inject32.exe <pid> <dll-path>`
//!
//! Built for `i686-pc-windows-msvc`. The 64-bit orchestrator cannot start a thread at its own
//! `LoadLibraryW` address inside a 32-bit (WOW64) game, so for 32-bit targets it spawns this
//! 32-bit helper, which injects from a matching-bitness process. The orchestrator created the
//! game suspended and resumes it after this exits 0.

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (Some(pid), Some(dll)) = (args.next(), args.next()) else {
        eprintln!("usage: lsa-inject32 <pid> <dll-path>");
        return ExitCode::from(2);
    };
    let Ok(pid) = pid.parse::<u32>() else {
        eprintln!("lsa-inject32: bad pid {pid:?}");
        return ExitCode::from(2);
    };
    match lsa_inject::inject_into_pid(pid, &PathBuf::from(dll)) {
        Ok(injected) => {
            if let Some(base) = injected.module_base {
                println!("injected at {base:#x}");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("lsa-inject32: {e}");
            ExitCode::FAILURE
        }
    }
}
