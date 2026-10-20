//! Loading `lsa_hook{32,64}.dll` into a game process of the same bitness.
//!
//! The well-worn method (Nucleus Co-op, Proto Input, countless others): allocate a buffer in
//! the target, write the DLL path into it, and start a thread there whose entry point is
//! `kernel32!LoadLibraryW`. `kernel32` loads at the same base in every process of one
//! bitness, so the `LoadLibraryW` address taken from *this* process is valid in the target as
//! long as both are 64-bit or both 32-bit. That is why `lsa-core` (64-bit) calls this
//! directly for 64-bit games and runs the 32-bit `lsa-inject32.exe` (built from this same
//! crate) for 32-bit games.
//!
//! This crate only injects into an already-created process by pid. `lsa-core::inject` creates
//! the game suspended, calls the right injector, then resumes it, so the hooks are installed
//! before the game runs its own code.

use std::path::Path;

/// Outcome of a successful injection.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Injected {
    /// Base address of the loaded hook module in the target, when it could be read back.
    pub module_base: Option<u64>,
}

/// File name of a DLL path, lower-cased, for matching a module list entry.
pub fn dll_file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

#[cfg(windows)]
mod imp {
    use super::{dll_file_name, Injected};
    use std::ffi::c_void;
    use std::path::Path;

    use windows::core::{BOOL, PCSTR, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows::Win32::System::Diagnostics::Debug::WriteProcessMemory;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Module32FirstW, Module32NextW, MODULEENTRY32W,
        TH32CS_SNAPMODULE, TH32CS_SNAPMODULE32,
    };
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows::Win32::System::Memory::{
        VirtualAllocEx, VirtualFreeEx, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE,
    };
    use windows::Win32::System::Threading::{
        CreateRemoteThread, GetExitCodeThread, IsWow64Process, OpenProcess, WaitForSingleObject,
        LPTHREAD_START_ROUTINE, PROCESS_CREATE_THREAD, PROCESS_QUERY_INFORMATION,
        PROCESS_VM_OPERATION, PROCESS_VM_READ, PROCESS_VM_WRITE,
    };

    /// How long to wait for the remote `LoadLibraryW` thread before giving up.
    const LOAD_TIMEOUT_MS: u32 = 15_000;

    /// Owns a kernel handle and closes it on drop.
    struct OwnedHandle(HANDLE);

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn wide_to_string(buf: &[u16]) -> String {
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end])
    }

    /// Whether `pid` is a 32-bit (WOW64) process. On a 64-bit Windows a non-WOW64 process is
    /// 64-bit; this binary is assumed to run on 64-bit Windows.
    pub fn process_is_wow64(pid: u32) -> Result<bool, String> {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION, false, pid) }
            .map_err(|e| format!("OpenProcess({pid}) for bitness check: {e}"))?;
        let guard = OwnedHandle(handle);
        let mut wow64 = BOOL::default();
        unsafe { IsWow64Process(guard.0, &mut wow64) }
            .map_err(|e| format!("IsWow64Process({pid}): {e}"))?;
        Ok(wow64.as_bool())
    }

    /// `kernel32!LoadLibraryW` in this process, which is also its address in a same-bitness
    /// target.
    fn load_library_w_addr() -> Result<LPTHREAD_START_ROUTINE, String> {
        unsafe {
            let kernel32 = GetModuleHandleW(PCWSTR(wide("kernel32.dll").as_ptr()))
                .map_err(|e| format!("GetModuleHandleW(kernel32): {e}"))?;
            let proc = GetProcAddress(kernel32, PCSTR(c"LoadLibraryW".as_ptr() as *const u8));
            match proc {
                Some(f) => Ok(Some(std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "system" fn(*mut c_void) -> u32,
                >(f))),
                None => Err("kernel32 has no LoadLibraryW export".into()),
            }
        }
    }

    /// Base address of the module named `file_name` (lower-case) in process `pid`, if present.
    ///
    /// A module-list snapshot races with the loader and transiently fails with
    /// `ERROR_BAD_LENGTH`, so retry a few times. This is best-effort reporting only; success is
    /// decided by the remote thread's exit code, not by this.
    fn remote_module_base(pid: u32, file_name: &str) -> Option<u64> {
        for attempt in 0..6 {
            if let Some(base) = module_base_once(pid, file_name) {
                return Some(base);
            }
            if attempt < 5 {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
        None
    }

    fn module_base_once(pid: u32, file_name: &str) -> Option<u64> {
        unsafe {
            let snap =
                CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid).ok()?;
            let guard = OwnedHandle(snap);
            let mut entry = MODULEENTRY32W {
                dwSize: std::mem::size_of::<MODULEENTRY32W>() as u32,
                ..Default::default()
            };
            let mut found = None;
            if Module32FirstW(guard.0, &mut entry).is_ok() {
                loop {
                    let name = wide_to_string(&entry.szModule).to_ascii_lowercase();
                    if name == file_name {
                        found = Some(entry.modBaseAddr as u64);
                        break;
                    }
                    if Module32NextW(guard.0, &mut entry).is_err() {
                        break;
                    }
                }
            }
            found
        }
    }

    /// Inject `dll_path` into the already-running process `pid`.
    pub fn inject_into_pid(pid: u32, dll_path: &Path) -> Result<Injected, String> {
        let path = dll_path.to_string_lossy();
        if !dll_path.is_file() {
            return Err(format!("hook DLL not found: {path}"));
        }
        let load_library = load_library_w_addr()?;

        let handle = unsafe {
            OpenProcess(
                PROCESS_CREATE_THREAD
                    | PROCESS_QUERY_INFORMATION
                    | PROCESS_VM_OPERATION
                    | PROCESS_VM_WRITE
                    | PROCESS_VM_READ,
                false,
                pid,
            )
        }
        .map_err(|e| format!("OpenProcess({pid}): {e}"))?;
        let proc = OwnedHandle(handle);

        let wpath = wide(&path);
        let bytes = wpath.len() * std::mem::size_of::<u16>();
        let remote = unsafe {
            VirtualAllocEx(
                proc.0,
                None,
                bytes,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_READWRITE,
            )
        };
        if remote.is_null() {
            return Err("VirtualAllocEx returned null".into());
        }
        // From here on, free `remote` before returning.
        let result = (|| {
            unsafe {
                WriteProcessMemory(
                    proc.0,
                    remote,
                    wpath.as_ptr() as *const c_void,
                    bytes,
                    None,
                )
            }
            .map_err(|e| format!("WriteProcessMemory: {e}"))?;

            let thread = unsafe {
                CreateRemoteThread(proc.0, None, 0, load_library, Some(remote), 0, None)
            }
            .map_err(|e| format!("CreateRemoteThread: {e}"))?;
            let thread = OwnedHandle(thread);

            let wait = unsafe { WaitForSingleObject(thread.0, LOAD_TIMEOUT_MS) };
            if wait != WAIT_OBJECT_0 {
                return Err(format!(
                    "remote LoadLibraryW did not finish (WaitForSingleObject = {})",
                    wait.0
                ));
            }
            // LoadLibraryW's return (the HMODULE) becomes the thread's exit code: non-zero on
            // success, null on failure. This is the authoritative, race-free success signal
            // (the module-list snapshot below is only for reporting the full base address).
            let mut exit_code = 0u32;
            unsafe { GetExitCodeThread(thread.0, &mut exit_code) }
                .map_err(|e| format!("GetExitCodeThread: {e}"))?;
            Ok(exit_code)
        })();

        unsafe {
            let _ = VirtualFreeEx(proc.0, remote, 0, MEM_RELEASE);
        }
        let exit_code = result?;
        if exit_code == 0 {
            return Err(format!(
                "remote LoadLibraryW returned null in pid {pid}: {} failed to load",
                dll_file_name(dll_path)
            ));
        }

        // Prefer the real base from the module list; fall back to the (low 32 bits of the)
        // HMODULE the thread returned, which is the whole base for a 32-bit target.
        let base = remote_module_base(pid, &dll_file_name(dll_path)).or(Some(exit_code as u64));
        Ok(Injected { module_base: base })
    }
}

#[cfg(windows)]
pub use imp::{inject_into_pid, process_is_wow64};

#[cfg(not(windows))]
pub fn inject_into_pid(_pid: u32, _dll_path: &Path) -> Result<Injected, String> {
    Err("DLL injection is only available on Windows".into())
}

#[cfg(not(windows))]
pub fn process_is_wow64(_pid: u32) -> Result<bool, String> {
    Err("process bitness detection is only available on Windows".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn dll_file_name_is_lowercased_basename() {
        assert_eq!(
            dll_file_name(&PathBuf::from(r"C:\ProgramData\LSA\LSA_Hook64.DLL")),
            "lsa_hook64.dll"
        );
        assert_eq!(dll_file_name(&PathBuf::from("lsa_hook32.dll")), "lsa_hook32.dll");
        assert_eq!(dll_file_name(&PathBuf::from("")), "");
    }
}
