//! Hook installation. Each submodule owns one concern; the pure decision logic in each (slot
//! mapping, name rewriting, rect math) is platform-independent and unit-tested, the detours
//! themselves are Windows-only.
//!
//! - `xinput`: `XInputGetState`/`GetStateEx`/`GetCapabilities`/`SetState`/
//!   `GetBatteryInformation`/`GetKeystroke` in every xinput DLL (including ones loaded later,
//!   via the `LoadLibrary` hooks in `loader`) so virtual slot 0 is the assigned physical slot
//!   and the other slots read as disconnected.
//! - `focus`: `GetForegroundWindow`/`GetActiveWindow`/`GetFocus`/`SetFocus`/
//!   `SetForegroundWindow` and a window subclass that swallows deactivation messages, plus a
//!   periodic activate-message loop.
//! - `handles`: `CreateMutex`/`CreateEvent`/`CreateSemaphore`/`CreateFileMapping`/
//!   `CreateNamedPipe`/`Open*` name suffixing per instance.
//! - `window`: `CreateWindowExW` size override, `SetWindowPos`/`MoveWindow` pinning,
//!   `GetSystemMetrics`/monitor/display-settings lies, `ChangeDisplaySettings` refusal,
//!   `ClipCursor` clamping, and main-window detection.
//! - `dx`: DXGI/D3D11 swap chains forced windowed and `SetFullscreenState(TRUE)` refused,
//!   D3D9 `CreateDevice`/`Reset` forced windowed.
//!
//! State shared by the detours lives in [`state`]; `update` swaps it on a config reload.

use lsa_proto::InstanceConfig;

pub mod handles;
pub mod mh;
pub mod state;
pub mod window;
pub mod xinput;

#[cfg(windows)]
pub mod dx;
#[cfg(windows)]
pub mod focus;
#[cfg(windows)]
pub mod iat;
#[cfg(windows)]
pub mod loader;

/// What `install` managed to do.
#[derive(Debug, Default)]
pub struct InstallOutcome {
    pub installed: Vec<String>,
    pub errors: Vec<String>,
}

impl InstallOutcome {
    #[cfg_attr(not(windows), allow(dead_code))]
    fn record(&mut self, name: &str, result: Result<(), Vec<String>>) {
        match result {
            Ok(()) => self.installed.push(name.to_string()),
            Err(errors) => {
                for e in errors {
                    self.errors.push(format!("{name}: {e}"));
                }
            }
        }
    }
}

/// Install every hook the config asks for.
#[cfg(windows)]
pub fn install(config: &InstanceConfig) -> InstallOutcome {
    let mut outcome = InstallOutcome::default();
    state::set(config);
    // Before anything else and before the game makes a window: put this process in the same
    // (physical-pixel) coordinate space the orchestrator pins in. See `window`.
    window::ensure_process_dpi_aware();
    if let Err(e) = mh::init() {
        outcome.errors.push(e);
        return outcome;
    }
    // Queue every hook below and enable them with one thread freeze at the end.
    mh::begin_batch();
    let f = &config.hooks;
    let wants_window = f.pin_window || f.lie_screen_size || f.force_windowed || f.clip_cursor;
    let wants_handles = !f.rename_handles.is_empty() || f.rename_all_handles;

    // The loader hooks exist for xinput and graphics modules that load after us.
    if f.xinput || f.force_windowed {
        outcome.record("loader", loader::install());
    }
    if f.xinput {
        outcome.record("xinput", xinput::install());
    }
    if f.fake_focus {
        outcome.record("fake_focus", focus::install());
    }
    if wants_handles {
        outcome.record("rename_handles", handles::install());
    }
    if wants_window {
        outcome.record("window", window::install());
    }
    if f.force_windowed {
        outcome.record("force_windowed", dx::install());
    }
    if let Err(e) = mh::apply_queued() {
        outcome.errors.push(e);
    }
    outcome
}

#[cfg(not(windows))]
pub fn install(config: &InstanceConfig) -> InstallOutcome {
    state::set(config);
    InstallOutcome {
        installed: Vec::new(),
        errors: vec!["hooks are only available on Windows".into()],
    }
}

/// Apply a changed config (controller swap, new viewport) to already installed hooks.
pub fn update(config: &InstanceConfig) {
    state::set(config);
    #[cfg(windows)]
    {
        window::on_config_changed();
        handles::on_config_changed();
    }
}

/// Periodic work: find the main window, (re)subclass and (re)pin it. Called every few
/// hundred milliseconds by the DLL's watcher thread.
#[cfg(windows)]
pub fn tick() {
    if let Some(h) = window::refresh_main_window() {
        focus::on_main_window(h);
        window::on_main_window(h);
    }
    window::repin();
    // Re-apply the XInput IAT patch: an overlay (Steam's, say) may have re-pointed the game's
    // xinput imports to its own emulation after our first install.
    if state::has(state::F_XINPUT) {
        xinput::rescan();
    }
}

/// Hook diagnostics for `state.json` (call counts, what the IAT patch captured).
#[cfg(windows)]
pub fn debug_lines() -> Vec<String> {
    xinput::debug_lines()
}

#[cfg(not(windows))]
pub fn debug_lines() -> Vec<String> {
    Vec::new()
}

#[cfg(not(windows))]
pub fn tick() {}

/// The game's main window, once a hook has identified it.
pub fn main_window() -> Option<u64> {
    let h = state::main_window();
    (h != 0).then_some(h as u64)
}
