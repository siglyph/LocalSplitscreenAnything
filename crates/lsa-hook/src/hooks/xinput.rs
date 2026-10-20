//! XInput remap: the game asks for user index 0 and gets the physical pad this instance
//! owns; every other index reads as disconnected. Hooked in every xinput module that is
//! loaded now or later (`loader` calls [`on_module_loaded`]).

/// `ERROR_DEVICE_NOT_CONNECTED`, what XInput returns for an empty slot.
pub const ERROR_DEVICE_NOT_CONNECTED: u32 = 1167;
/// `XUSER_INDEX_ANY`, accepted by `XInputGetKeystroke`.
pub const XUSER_INDEX_ANY: u32 = 0xFF;

/// Modules that export the XInput API, in the order games usually try them.
pub const MODULES: [&str; 4] = [
    "xinput1_4.dll",
    "xinput1_3.dll",
    "xinput9_1_0.dll",
    "xinputuap.dll",
];

/// Whether a lower-case module stem names one of [`MODULES`].
pub fn is_xinput_module(stem: &str) -> bool {
    MODULES
        .iter()
        .any(|m| m.trim_end_matches(".dll").eq_ignore_ascii_case(stem))
}

/// Which physical user index to forward a request for `requested` to, given the pad this
/// instance owns (`assigned`). `None` means "report not connected".
pub fn map_user_index(requested: u32, assigned: Option<u32>) -> Option<u32> {
    let assigned = assigned?;
    match requested {
        0 | XUSER_INDEX_ANY => Some(assigned),
        _ => None,
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, Ordering};

    use crate::detour_ptr;
    use crate::hooks::mh::{self, Orig};
    use crate::hooks::state;

    pub type GetStateFn = unsafe extern "system" fn(u32, *mut c_void) -> u32;
    pub type SetStateFn = unsafe extern "system" fn(u32, *mut c_void) -> u32;
    pub type GetCapsFn = unsafe extern "system" fn(u32, u32, *mut c_void) -> u32;
    pub type GetBatteryFn = unsafe extern "system" fn(u32, u8, *mut c_void) -> u32;
    pub type GetKeystrokeFn = unsafe extern "system" fn(u32, u32, *mut c_void) -> u32;

    fn target(user: u32) -> Option<u32> {
        if !state::has(state::F_XINPUT) {
            // Hook disabled live: behave like the real API for every slot.
            return Some(user);
        }
        map_user_index(user, state::xinput_slot())
    }

    macro_rules! xinput_module {
        ($m:ident, $dll:literal) => {
            pub mod $m {
                use super::*;

                pub const DLL: &str = $dll;
                static HOOKED: AtomicBool = AtomicBool::new(false);
                static GET_STATE: Orig<GetStateFn> = Orig::new();
                static GET_STATE_EX: Orig<GetStateFn> = Orig::new();
                static SET_STATE: Orig<SetStateFn> = Orig::new();
                static GET_CAPS: Orig<GetCapsFn> = Orig::new();
                static GET_BATTERY: Orig<GetBatteryFn> = Orig::new();
                static GET_KEYSTROKE: Orig<GetKeystrokeFn> = Orig::new();

                unsafe extern "system" fn get_state(user: u32, out: *mut c_void) -> u32 {
                    match (target(user), GET_STATE.get()) {
                        (Some(p), Some(orig)) => orig(p, out),
                        _ => ERROR_DEVICE_NOT_CONNECTED,
                    }
                }

                unsafe extern "system" fn get_state_ex(user: u32, out: *mut c_void) -> u32 {
                    match (target(user), GET_STATE_EX.get()) {
                        (Some(p), Some(orig)) => orig(p, out),
                        _ => ERROR_DEVICE_NOT_CONNECTED,
                    }
                }

                unsafe extern "system" fn set_state(user: u32, vibration: *mut c_void) -> u32 {
                    match (target(user), SET_STATE.get()) {
                        (Some(p), Some(orig)) => orig(p, vibration),
                        _ => ERROR_DEVICE_NOT_CONNECTED,
                    }
                }

                unsafe extern "system" fn get_caps(
                    user: u32,
                    flags: u32,
                    out: *mut c_void,
                ) -> u32 {
                    match (target(user), GET_CAPS.get()) {
                        (Some(p), Some(orig)) => orig(p, flags, out),
                        _ => ERROR_DEVICE_NOT_CONNECTED,
                    }
                }

                unsafe extern "system" fn get_battery(
                    user: u32,
                    dev: u8,
                    out: *mut c_void,
                ) -> u32 {
                    match (target(user), GET_BATTERY.get()) {
                        (Some(p), Some(orig)) => orig(p, dev, out),
                        _ => ERROR_DEVICE_NOT_CONNECTED,
                    }
                }

                unsafe extern "system" fn get_keystroke(
                    user: u32,
                    reserved: u32,
                    out: *mut c_void,
                ) -> u32 {
                    match (target(user), GET_KEYSTROKE.get()) {
                        (Some(p), Some(orig)) => orig(p, reserved, out),
                        _ => ERROR_DEVICE_NOT_CONNECTED,
                    }
                }

                /// Hook this module if it is loaded and not hooked yet. `Ok(true)` when hooks
                /// were installed now.
                pub fn hook_if_loaded() -> Result<bool, String> {
                    if HOOKED.load(Ordering::SeqCst) || !mh::module_loaded(DLL) {
                        return Ok(false);
                    }
                    unsafe {
                        // GetState is mandatory; everything else is best effort because the
                        // older modules lack some exports.
                        mh::hook_export(DLL, "XInputGetState", detour_ptr!(get_state), &GET_STATE)?;
                        HOOKED.store(true, Ordering::SeqCst);
                        let _ = mh::hook_export(
                            DLL,
                            "XInputSetState",
                            detour_ptr!(set_state),
                            &SET_STATE,
                        );
                        let _ = mh::hook_export(
                            DLL,
                            "XInputGetCapabilities",
                            detour_ptr!(get_caps),
                            &GET_CAPS,
                        );
                        let _ = mh::hook_export(
                            DLL,
                            "XInputGetBatteryInformation",
                            detour_ptr!(get_battery),
                            &GET_BATTERY,
                        );
                        let _ = mh::hook_export(
                            DLL,
                            "XInputGetKeystroke",
                            detour_ptr!(get_keystroke),
                            &GET_KEYSTROKE,
                        );
                        let _ = mh::hook_export_ordinal(
                            DLL,
                            100,
                            detour_ptr!(get_state_ex),
                            &GET_STATE_EX,
                        );
                    }
                    Ok(true)
                }
            }
        };
    }

    xinput_module!(xinput1_4, "xinput1_4.dll");
    xinput_module!(xinput1_3, "xinput1_3.dll");
    xinput_module!(xinput9_1_0, "xinput9_1_0.dll");
    xinput_module!(xinputuap, "xinputuap.dll");

    fn hook_by_stem(stem: &str) -> Result<bool, String> {
        match stem {
            "xinput1_4" => xinput1_4::hook_if_loaded(),
            "xinput1_3" => xinput1_3::hook_if_loaded(),
            "xinput9_1_0" => xinput9_1_0::hook_if_loaded(),
            "xinputuap" => xinputuap::hook_if_loaded(),
            _ => Ok(false),
        }
    }

    /// Called by the loader hooks for every module load.
    pub fn on_module_loaded(stem: &str) {
        if is_xinput_module(stem) {
            let _ = hook_by_stem(stem);
        }
    }

    /// Hook every xinput module that is already loaded. Succeeds even if none is loaded yet
    /// (the loader hooks pick them up later).
    pub fn install() -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        for m in MODULES {
            if let Err(e) = hook_by_stem(m.trim_end_matches(".dll")) {
                errors.push(e);
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[cfg(windows)]
pub use imp::{install, on_module_loaded};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn virtual_slot_zero_maps_to_the_assigned_pad() {
        assert_eq!(map_user_index(0, Some(2)), Some(2));
        assert_eq!(map_user_index(0, Some(0)), Some(0));
        assert_eq!(map_user_index(XUSER_INDEX_ANY, Some(3)), Some(3));
    }

    #[test]
    fn other_slots_read_as_disconnected() {
        for requested in 1..=3 {
            assert_eq!(map_user_index(requested, Some(0)), None);
        }
        assert_eq!(map_user_index(7, Some(1)), None);
    }

    #[test]
    fn no_assignment_means_no_pads_at_all() {
        for requested in [0, 1, 2, 3, XUSER_INDEX_ANY] {
            assert_eq!(map_user_index(requested, None), None);
        }
    }

    #[test]
    fn recognises_xinput_module_stems() {
        assert!(is_xinput_module("xinput1_4"));
        assert!(is_xinput_module("XINPUT9_1_0"));
        assert!(is_xinput_module("xinputuap"));
        assert!(!is_xinput_module("xinput1_4.dll"));
        assert!(!is_xinput_module("dinput8"));
    }
}
