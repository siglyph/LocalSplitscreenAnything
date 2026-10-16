//! Controller discovery and the "press A to assign" flow.
//!
//! v1 handles XInput controllers (Xbox pads and anything presenting as one). Each player slot
//! is bound to an XInput user index; the hook DLL then exposes exactly that index to the
//! game. HID-only pads (DualShock/DualSense over HIDAPI) come in Phase 3.

use std::time::{Duration, Instant};

use lsa_proto::ControllerAssignment;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XInputPad {
    pub slot: u32,
    pub connected: bool,
}

/// Connection state of XInput slots 0..=3.
pub fn xinput_slots() -> Vec<XInputPad> {
    (0..4)
        .map(|slot| XInputPad {
            slot,
            connected: imp::is_connected(slot),
        })
        .collect()
}

/// Wait until a controller not in `exclude` presses A (or `timeout` passes). Returns the slot.
pub fn wait_for_a_button(exclude: &[u32], timeout: Duration) -> Option<u32> {
    let start = Instant::now();
    // Require a release first so one long press does not assign several players.
    let mut armed = [false; 4];
    loop {
        for slot in 0..4u32 {
            if exclude.contains(&slot) {
                continue;
            }
            match imp::a_pressed(slot) {
                Some(true) if armed[slot as usize] => return Some(slot),
                Some(false) => armed[slot as usize] = true,
                _ => {}
            }
        }
        if start.elapsed() >= timeout {
            return None;
        }
        std::thread::sleep(Duration::from_millis(16));
    }
}

/// Parse `--controller 2=1` style assignments: player number to XInput slot.
pub fn parse_assignment(spec: &str) -> Result<(u8, ControllerAssignment), String> {
    let (player, value) = spec
        .split_once('=')
        .ok_or_else(|| format!("expected PLAYER=SLOT, got '{spec}'"))?;
    let player: u8 = player
        .trim()
        .parse()
        .map_err(|_| format!("bad player number in '{spec}'"))?;
    if !(1..=4).contains(&player) {
        return Err(format!("player must be 1..=4 in '{spec}'"));
    }
    let value = value.trim();
    let assignment = match value {
        "none" | "-" => ControllerAssignment::None,
        v if v.starts_with("hid:") => ControllerAssignment::HidPath {
            path: v[4..].to_string(),
        },
        v => {
            let slot: u32 = v
                .parse()
                .map_err(|_| format!("bad XInput slot in '{spec}' (0-3, none, or hid:<path>)"))?;
            if slot > 3 {
                return Err(format!("XInput slot must be 0..=3 in '{spec}'"));
            }
            ControllerAssignment::XInputSlot { slot }
        }
    };
    Ok((player, assignment))
}

#[cfg(windows)]
mod imp {
    use windows::Win32::UI::Input::XboxController::{
        XInputGetState, XINPUT_GAMEPAD_A, XINPUT_STATE,
    };

    pub fn is_connected(slot: u32) -> bool {
        let mut state = XINPUT_STATE::default();
        unsafe { XInputGetState(slot, &mut state) == 0 }
    }

    /// `None` when disconnected, otherwise whether A is down.
    pub fn a_pressed(slot: u32) -> Option<bool> {
        let mut state = XINPUT_STATE::default();
        let ok = unsafe { XInputGetState(slot, &mut state) } == 0;
        ok.then_some((state.Gamepad.wButtons.0 & XINPUT_GAMEPAD_A.0) != 0)
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn is_connected(_slot: u32) -> bool {
        false
    }

    pub fn a_pressed(_slot: u32) -> Option<bool> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_assignments() {
        assert_eq!(
            parse_assignment("2=1").unwrap(),
            (2, ControllerAssignment::XInputSlot { slot: 1 })
        );
        assert_eq!(
            parse_assignment("1=none").unwrap(),
            (1, ControllerAssignment::None)
        );
        assert_eq!(
            parse_assignment("3=hid:\\\\?\\hid#vid_054c").unwrap(),
            (
                3,
                ControllerAssignment::HidPath {
                    path: "\\\\?\\hid#vid_054c".into()
                }
            )
        );
        assert!(parse_assignment("5=0").is_err());
        assert!(parse_assignment("1=4").is_err());
        assert!(parse_assignment("oops").is_err());
    }
}
