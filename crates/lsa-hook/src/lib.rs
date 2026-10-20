//! `lsa_hook{32,64}.dll`: injected into every game instance.
//!
//! Locates `instance.json` (through `LSA_INSTANCE_DIR` or next to the DLL), stays dormant in
//! processes that are not the target executable, installs the hooks in `hooks/` (XInput
//! remap, fake focus, handle rename, window pinning / screen-size lies, forced windowed
//! rendering), writes `state.json`, and re-reads the config when it changes so controller
//! and viewport changes apply live.
//!
//! Rules for code in this crate: `DllMain` must not block or load libraries (loader lock), so
//! all work happens on a thread it spawns; nothing may unwind across the FFI boundary (the
//! `hook` cargo profile sets `panic = "abort"`).

#![allow(clippy::missing_safety_doc)]

pub mod config;
pub mod hooks;

#[cfg(windows)]
mod entry {
    use std::ffi::c_void;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use lsa_proto::{InstanceState, PROTO_VERSION};
    use windows::core::BOOL;
    use windows::Win32::Foundation::HINSTANCE;
    use windows::Win32::System::LibraryLoader::{DisableThreadLibraryCalls, GetModuleFileNameW};
    use windows::Win32::System::SystemServices::DLL_PROCESS_ATTACH;
    use windows::Win32::System::Threading::GetCurrentProcessId;

    use crate::config::{find_instance_dir, load_config};

    static STARTED: AtomicBool = AtomicBool::new(false);

    fn module_path(module: Option<HINSTANCE>) -> Option<PathBuf> {
        let mut buf = [0u16; 32768];
        let len = unsafe { GetModuleFileNameW(module.map(|h| h.into()), &mut buf) } as usize;
        if len == 0 {
            return None;
        }
        Some(PathBuf::from(String::from_utf16_lossy(&buf[..len])))
    }

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    /// Runs on its own thread right after the DLL is loaded.
    fn init(dll: HINSTANCE) {
        let dll_path = module_path(Some(dll));
        let exe_path = module_path(None);
        let exe_name = exe_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let Some(dir) = find_instance_dir(dll_path.as_deref()) else {
            return;
        };
        let Ok(mut config) = load_config(&dir) else {
            return;
        };
        if !config.targets_exe(&exe_name) {
            // Not the game (steam.exe, a launcher, a crash handler): stay dormant.
            return;
        }

        let mut state = InstanceState {
            version: PROTO_VERSION,
            pid: unsafe { GetCurrentProcessId() },
            hwnd: None,
            hooks_installed: Vec::new(),
            errors: Vec::new(),
            updated_unix_ms: now_ms(),
        };
        let outcome = crate::hooks::install(&config);
        state.hooks_installed = outcome.installed;
        state.errors = outcome.errors;
        let _ = crate::config::save_state(&dir, &state);

        // Live reload: the orchestrator rewrites instance.json on controller/viewport changes.
        let mut last_modified = crate::config::modified(&dir);
        loop {
            std::thread::sleep(Duration::from_millis(250));
            crate::hooks::tick();
            let modified = crate::config::modified(&dir);
            if modified != last_modified {
                last_modified = modified;
                if let Ok(fresh) = load_config(&dir) {
                    config = fresh;
                    crate::hooks::update(&config);
                }
            }
            if let Some(h) = crate::hooks::main_window() {
                if state.hwnd != Some(h) {
                    state.hwnd = Some(h);
                    state.updated_unix_ms = now_ms();
                    let _ = crate::config::save_state(&dir, &state);
                }
            }
        }
    }

    fn start(dll: HINSTANCE) {
        if STARTED.swap(true, Ordering::SeqCst) {
            return;
        }
        // Raw handles are not `Send`; carry the address across as an integer.
        let dll_addr = dll.0 as usize;
        std::thread::spawn(move || init(HINSTANCE(dll_addr as *mut c_void)));
    }

    #[no_mangle]
    pub unsafe extern "system" fn DllMain(
        dll: HINSTANCE,
        reason: u32,
        _reserved: *mut c_void,
    ) -> BOOL {
        if reason == DLL_PROCESS_ATTACH {
            let _ = DisableThreadLibraryCalls(dll.into());
            start(dll);
        }
        BOOL::from(true)
    }

    /// Called by Sandboxie after an `InjectDll` load (`DllMain` has already started us).
    #[no_mangle]
    pub unsafe extern "system" fn InjectDllMain(_sbie_dll: HINSTANCE, _unused: usize) {}
}
