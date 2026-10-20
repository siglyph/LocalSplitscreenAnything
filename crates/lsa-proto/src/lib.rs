//! Types shared between the orchestrator (`lsa-core`) and the injected hook DLL (`lsa-hook`).
//!
//! Before launching an instance the orchestrator writes [`InstanceConfig`] as `instance.json`
//! into a per-instance directory. The hook DLL finds that directory either through the
//! [`INSTANCE_DIR_ENV`] environment variable or, when the variable did not propagate (for
//! example because Steam hands `-applaunch` off to an already running client), next to the
//! DLL itself. The hook re-reads the file whenever it changes, so controller or viewport
//! changes made in the orchestrator take effect live, and it writes [`InstanceState`] back as
//! `state.json` so the orchestrator can see which hooks are active and which window is the
//! game's main window.

use serde::{Deserialize, Serialize};

/// Environment variable naming the directory that holds `instance.json`.
pub const INSTANCE_DIR_ENV: &str = "LSA_INSTANCE_DIR";
/// File the orchestrator writes for the hook DLL.
pub const INSTANCE_FILE: &str = "instance.json";
/// File the hook DLL writes back for the orchestrator.
pub const STATE_FILE: &str = "state.json";
/// Bumped whenever the JSON shape changes incompatibly.
pub const PROTO_VERSION: u32 = 1;

/// Drop a leading UTF-8 byte-order mark. A text editor saving a hand-edited `instance.json` as
/// "UTF-8 with BOM" would otherwise make `serde_json` reject the whole file, and the hook's
/// hot-reload would silently keep the old config.
fn strip_bom(s: &str) -> &str {
    s.strip_prefix('\u{feff}').unwrap_or(s)
}

/// A rectangle in virtual-desktop pixel coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct PixelRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

impl PixelRect {
    pub const fn new(x: i32, y: i32, width: i32, height: i32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn right(&self) -> i32 {
        self.x + self.width
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.height
    }

    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.right() && py >= self.y && py < self.bottom()
    }
}

/// Which physical controller an instance owns.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ControllerAssignment {
    /// XInput user index (0..=3) of the physical controller. The hook exposes it as virtual
    /// slot 0 and reports every other virtual slot as disconnected.
    XInputSlot { slot: u32 },
    /// HID device interface path for DirectInput/HID-only controllers (Phase 3).
    HidPath { path: String },
    /// No controller: the instance only gets keyboard/mouse when focused.
    #[default]
    None,
}

/// Which hooks the DLL should install in this instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HookFlags {
    /// Remap XInput so the instance only sees its assigned controller.
    pub xinput: bool,
    /// Make the game believe its window is always focused.
    pub fake_focus: bool,
    /// Refuse exclusive fullscreen (DXGI `SetFullscreenState`, `ChangeDisplaySettings`).
    pub force_windowed: bool,
    /// Report the viewport size as the screen size (`GetSystemMetrics`, monitor queries).
    pub lie_screen_size: bool,
    /// Keep the main window at the viewport rect even when the game moves/resizes it.
    pub pin_window: bool,
    /// Confine the cursor to the viewport instead of the whole screen.
    pub clip_cursor: bool,
    /// Named kernel objects (mutexes, events, file mappings, pipes) to rename per instance so
    /// single-instance checks do not see sibling instances.
    pub rename_handles: Vec<String>,
    /// Rename every named kernel object the game creates (aggressive mode).
    pub rename_all_handles: bool,
}

impl Default for HookFlags {
    fn default() -> Self {
        Self {
            xinput: true,
            fake_focus: true,
            force_windowed: true,
            lie_screen_size: true,
            pin_window: true,
            clip_cursor: false,
            rename_handles: Vec::new(),
            rename_all_handles: false,
        }
    }
}

/// Contents of `instance.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstanceConfig {
    pub version: u32,
    /// Random id of the session this instance belongs to.
    pub session_id: String,
    /// 1-based player number.
    pub player: u8,
    pub nickname: String,
    /// Executable file name (case-insensitive, e.g. `Terraria.exe`) the hook activates in. In
    /// any other process in the same box (steam.exe, launchers) the DLL stays dormant.
    pub target_exe: String,
    pub controller: ControllerAssignment,
    /// Where this instance's window belongs, in virtual-desktop pixels.
    pub viewport: PixelRect,
    pub hooks: HookFlags,
    /// Optional regex to recognise the game's main window title.
    pub window_title_regex: Option<String>,
}

impl InstanceConfig {
    pub fn to_json_pretty(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    pub fn from_json_str(s: &str) -> serde_json::Result<Self> {
        serde_json::from_str(strip_bom(s))
    }

    /// Whether the hook should activate inside a process with this executable file name.
    pub fn targets_exe(&self, exe_file_name: &str) -> bool {
        self.target_exe.eq_ignore_ascii_case(exe_file_name)
    }
}

/// Contents of `state.json`, written by the hook DLL.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct InstanceState {
    pub version: u32,
    pub pid: u32,
    /// Main window handle as an integer, once known.
    pub hwnd: Option<u64>,
    pub hooks_installed: Vec<String>,
    pub errors: Vec<String>,
    /// Free-form hook diagnostics (call counts, what the IAT patch captured). Not load-bearing.
    #[serde(default)]
    pub debug: Vec<String>,
    pub updated_unix_ms: u64,
}

impl InstanceState {
    pub fn to_json_pretty(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    pub fn from_json_str(s: &str) -> serde_json::Result<Self> {
        serde_json::from_str(strip_bom(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_config_round_trips() {
        let cfg = InstanceConfig {
            version: PROTO_VERSION,
            session_id: "abc".into(),
            player: 2,
            nickname: "P2".into(),
            target_exe: "Game.exe".into(),
            controller: ControllerAssignment::XInputSlot { slot: 1 },
            viewport: PixelRect::new(960, 0, 960, 1080),
            hooks: HookFlags::default(),
            window_title_regex: Some("^Game".into()),
        };
        let json = cfg.to_json_pretty().unwrap();
        assert!(json.contains("\"kind\": \"x_input_slot\""));
        let back = InstanceConfig::from_json_str(&json).unwrap();
        assert_eq!(back, cfg);
        assert!(back.targets_exe("game.EXE"));
        assert!(!back.targets_exe("steam.exe"));

        // A hand-edit saved as UTF-8 with a BOM still parses (hot-reload robustness).
        let with_bom = format!("\u{feff}{json}");
        assert_eq!(InstanceConfig::from_json_str(&with_bom).unwrap(), cfg);
    }

    #[test]
    fn hook_flags_default_from_empty_object() {
        let flags: HookFlags = serde_json::from_str("{}").unwrap();
        assert_eq!(flags, HookFlags::default());
        assert!(flags.xinput && flags.fake_focus && !flags.rename_all_handles);
    }

    #[test]
    fn pixel_rect_contains_is_half_open() {
        let r = PixelRect::new(10, 10, 5, 5);
        assert!(r.contains(10, 10));
        assert!(r.contains(14, 14));
        assert!(!r.contains(15, 10));
        assert!(!r.contains(9, 10));
    }
}
