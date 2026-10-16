//! Hook installation. Each submodule owns one concern and exposes `install`/`update`.
//!
//! Status: scaffold only (milestone M0). M1 fills in:
//! - `xinput`: `XInputGetState`/`GetStateEx`/`GetCapabilities`/`SetState` in every xinput DLL
//!   (including ones loaded later, via a `LoadLibrary` hook) so virtual slot 0 is the assigned
//!   physical slot and the other slots read as disconnected.
//! - `focus`: `GetForegroundWindow`/`GetActiveWindow`/`GetFocus` and a window subclass that
//!   swallows deactivation messages, plus a periodic activate-message loop.
//! - `handles`: `CreateMutex`/`CreateEvent`/`CreateSemaphore`/`CreateFileMapping`/
//!   `CreateNamedPipe` name suffixing per instance.
//! - `window`: `CreateWindowExW` size override, `SetWindowPos` pinning, `GetSystemMetrics`
//!   and monitor-info lies, DXGI `SetFullscreenState` refusal, `ClipCursor` clamping.

use lsa_proto::InstanceConfig;

/// What `install` managed to do.
#[derive(Debug, Default)]
pub struct InstallOutcome {
    pub installed: Vec<String>,
    pub errors: Vec<String>,
}

/// Install every hook the config asks for.
pub fn install(config: &InstanceConfig) -> InstallOutcome {
    let mut outcome = InstallOutcome::default();
    let wanted = [
        ("xinput", config.hooks.xinput),
        ("fake_focus", config.hooks.fake_focus),
        ("force_windowed", config.hooks.force_windowed),
        ("lie_screen_size", config.hooks.lie_screen_size),
        ("pin_window", config.hooks.pin_window),
        ("clip_cursor", config.hooks.clip_cursor),
        (
            "rename_handles",
            !config.hooks.rename_handles.is_empty() || config.hooks.rename_all_handles,
        ),
    ];
    for (name, enabled) in wanted {
        if enabled {
            outcome
                .errors
                .push(format!("{name}: not implemented yet (milestone M1)"));
        }
    }
    outcome
}

/// Apply a changed config (controller swap, new viewport) to already installed hooks.
pub fn update(_config: &InstanceConfig) {}

/// The game's main window, once a hook has identified it.
pub fn main_window() -> Option<u64> {
    None
}
