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
    use std::cell::Cell;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, AtomicUsize, Ordering};

    use crate::detour_ptr;
    use crate::hooks::mh::{self, Orig};
    use crate::hooks::{iat, state};

    pub type GetStateFn = unsafe extern "system" fn(u32, *mut c_void) -> u32;
    pub type SetStateFn = unsafe extern "system" fn(u32, *mut c_void) -> u32;
    pub type GetCapsFn = unsafe extern "system" fn(u32, u32, *mut c_void) -> u32;
    pub type GetBatteryFn = unsafe extern "system" fn(u32, u8, *mut c_void) -> u32;
    pub type GetKeystrokeFn = unsafe extern "system" fn(u32, u32, *mut c_void) -> u32;

    /// The XInput entry points we remap. Index into the per-function tables below.
    #[derive(Clone, Copy)]
    enum XiFn {
        GetState = 0,
        GetStateEx,
        SetState,
        GetCaps,
        GetBattery,
        GetKeystroke,
    }
    const N: usize = 6;
    const NAMES: [&str; N] = [
        "GetState",
        "GetStateEx",
        "SetState",
        "GetCaps",
        "GetBattery",
        "GetKeystroke",
    ];

    thread_local! {
        /// Set while we are inside a remapped original call, so if that original is itself one
        /// of our detours (the export reached through a hooked IAT slot, say) it passes the
        /// already-mapped index straight through instead of remapping a second time.
        static REMAPPING: Cell<bool> = const { Cell::new(false) };
    }

    static CALLS_EXPORT: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
    static CALLS_IAT: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
    /// Function the IAT slot pointed at before we patched it (the emulation to call through).
    static IAT_ORIG: [AtomicPtr<c_void>; N] = [const { AtomicPtr::new(std::ptr::null_mut()) }; N];
    static IAT_PATCHED: AtomicUsize = AtomicUsize::new(0);
    static IAT_SCANS: AtomicU64 = AtomicU64::new(0);

    fn target(user: u32) -> Option<u32> {
        if !state::has(state::F_XINPUT) {
            // Hook disabled live: behave like the real API for every slot.
            return Some(user);
        }
        map_user_index(user, state::xinput_slot())
    }

    // The remap cores, one per signature. Each forwards to `orig` with the mapped index, once.
    unsafe fn core2(user: u32, out: *mut c_void, orig: Option<GetStateFn>) -> u32 {
        let Some(orig) = orig else {
            return ERROR_DEVICE_NOT_CONNECTED;
        };
        if REMAPPING.with(|r| r.get()) {
            return orig(user, out);
        }
        match target(user) {
            None => ERROR_DEVICE_NOT_CONNECTED,
            Some(p) => {
                REMAPPING.with(|r| r.set(true));
                let r = orig(p, out);
                REMAPPING.with(|r| r.set(false));
                r
            }
        }
    }

    unsafe fn core3(user: u32, a: u32, out: *mut c_void, orig: Option<GetCapsFn>) -> u32 {
        let Some(orig) = orig else {
            return ERROR_DEVICE_NOT_CONNECTED;
        };
        if REMAPPING.with(|r| r.get()) {
            return orig(user, a, out);
        }
        match target(user) {
            None => ERROR_DEVICE_NOT_CONNECTED,
            Some(p) => {
                REMAPPING.with(|r| r.set(true));
                let r = orig(p, a, out);
                REMAPPING.with(|r| r.set(false));
                r
            }
        }
    }

    unsafe fn core_batt(user: u32, dev: u8, out: *mut c_void, orig: Option<GetBatteryFn>) -> u32 {
        let Some(orig) = orig else {
            return ERROR_DEVICE_NOT_CONNECTED;
        };
        if REMAPPING.with(|r| r.get()) {
            return orig(user, dev, out);
        }
        match target(user) {
            None => ERROR_DEVICE_NOT_CONNECTED,
            Some(p) => {
                REMAPPING.with(|r| r.set(true));
                let r = orig(p, dev, out);
                REMAPPING.with(|r| r.set(false));
                r
            }
        }
    }

    // ---- IAT detours: one shared set, calling through to whatever the slot used to hold.

    fn iat_orig<T: Copy>(f: XiFn) -> Option<T> {
        let p = IAT_ORIG[f as usize].load(Ordering::SeqCst);
        if p.is_null() {
            None
        } else {
            Some(unsafe { std::mem::transmute_copy::<*mut c_void, T>(&p) })
        }
    }

    unsafe extern "system" fn iat_get_state(user: u32, out: *mut c_void) -> u32 {
        CALLS_IAT[XiFn::GetState as usize].fetch_add(1, Ordering::Relaxed);
        core2(user, out, iat_orig::<GetStateFn>(XiFn::GetState))
    }
    unsafe extern "system" fn iat_get_state_ex(user: u32, out: *mut c_void) -> u32 {
        CALLS_IAT[XiFn::GetStateEx as usize].fetch_add(1, Ordering::Relaxed);
        core2(user, out, iat_orig::<GetStateFn>(XiFn::GetStateEx))
    }
    unsafe extern "system" fn iat_set_state(user: u32, vib: *mut c_void) -> u32 {
        CALLS_IAT[XiFn::SetState as usize].fetch_add(1, Ordering::Relaxed);
        core2(user, vib, iat_orig::<SetStateFn>(XiFn::SetState))
    }
    unsafe extern "system" fn iat_get_caps(user: u32, flags: u32, out: *mut c_void) -> u32 {
        CALLS_IAT[XiFn::GetCaps as usize].fetch_add(1, Ordering::Relaxed);
        core3(user, flags, out, iat_orig::<GetCapsFn>(XiFn::GetCaps))
    }
    unsafe extern "system" fn iat_get_battery(user: u32, dev: u8, out: *mut c_void) -> u32 {
        CALLS_IAT[XiFn::GetBattery as usize].fetch_add(1, Ordering::Relaxed);
        core_batt(user, dev, out, iat_orig::<GetBatteryFn>(XiFn::GetBattery))
    }
    unsafe extern "system" fn iat_get_keystroke(user: u32, res: u32, out: *mut c_void) -> u32 {
        CALLS_IAT[XiFn::GetKeystroke as usize].fetch_add(1, Ordering::Relaxed);
        core3(user, res, out, iat_orig::<GetKeystrokeFn>(XiFn::GetKeystroke))
    }

    fn iat_detour(f: XiFn) -> *mut c_void {
        match f {
            XiFn::GetState => detour_ptr!(iat_get_state),
            XiFn::GetStateEx => detour_ptr!(iat_get_state_ex),
            XiFn::SetState => detour_ptr!(iat_set_state),
            XiFn::GetCaps => detour_ptr!(iat_get_caps),
            XiFn::GetBattery => detour_ptr!(iat_get_battery),
            XiFn::GetKeystroke => detour_ptr!(iat_get_keystroke),
        }
    }

    fn classify(import: iat::Import<'_>) -> Option<XiFn> {
        match import {
            iat::Import::Name("XInputGetState") => Some(XiFn::GetState),
            iat::Import::Name("XInputSetState") => Some(XiFn::SetState),
            iat::Import::Name("XInputGetCapabilities") => Some(XiFn::GetCaps),
            iat::Import::Name("XInputGetBatteryInformation") => Some(XiFn::GetBattery),
            iat::Import::Name("XInputGetKeystroke") => Some(XiFn::GetKeystroke),
            // GetStateEx is exported only by ordinal 100, and imported the same way.
            iat::Import::Ordinal(100) => Some(XiFn::GetStateEx),
            _ => None,
        }
    }

    /// Point every module's xinput imports at our detours, capturing whatever they currently
    /// resolve to (the real export, or an emulation such as Steam's overlay) as the function
    /// to call through. Safe to run repeatedly: a slot already holding our detour is left
    /// alone, so a re-scan after the overlay re-points the IAT puts us back in front without
    /// capturing ourselves.
    pub fn install_iat() {
        let detours: [*mut c_void; N] = [
            iat_detour(XiFn::GetState),
            iat_detour(XiFn::GetStateEx),
            iat_detour(XiFn::SetState),
            iat_detour(XiFn::GetCaps),
            iat_detour(XiFn::GetBattery),
            iat_detour(XiFn::GetKeystroke),
        ];
        let mut patched = 0usize;
        for base in iat::modules() {
            unsafe {
                iat::visit_iat(base, is_xinput_module, |_stem, import, slot| {
                    let Some(f) = classify(import) else { return };
                    let idx = f as usize;
                    let current = *slot;
                    if current.is_null() || current == detours[idx] {
                        return;
                    }
                    IAT_ORIG[idx].store(current, Ordering::SeqCst);
                    if iat::write_slot(slot, detours[idx]).is_some() {
                        patched += 1;
                    }
                });
            }
        }
        IAT_PATCHED.store(patched, Ordering::SeqCst);
        IAT_SCANS.fetch_add(1, Ordering::Relaxed);
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
                    CALLS_EXPORT[XiFn::GetState as usize].fetch_add(1, Ordering::Relaxed);
                    core2(user, out, GET_STATE.get())
                }
                unsafe extern "system" fn get_state_ex(user: u32, out: *mut c_void) -> u32 {
                    CALLS_EXPORT[XiFn::GetStateEx as usize].fetch_add(1, Ordering::Relaxed);
                    core2(user, out, GET_STATE_EX.get())
                }
                unsafe extern "system" fn set_state(user: u32, vib: *mut c_void) -> u32 {
                    CALLS_EXPORT[XiFn::SetState as usize].fetch_add(1, Ordering::Relaxed);
                    core2(user, vib, SET_STATE.get())
                }
                unsafe extern "system" fn get_caps(user: u32, flags: u32, out: *mut c_void) -> u32 {
                    CALLS_EXPORT[XiFn::GetCaps as usize].fetch_add(1, Ordering::Relaxed);
                    core3(user, flags, out, GET_CAPS.get())
                }
                unsafe extern "system" fn get_battery(user: u32, dev: u8, out: *mut c_void) -> u32 {
                    CALLS_EXPORT[XiFn::GetBattery as usize].fetch_add(1, Ordering::Relaxed);
                    core_batt(user, dev, out, GET_BATTERY.get())
                }
                unsafe extern "system" fn get_keystroke(user: u32, res: u32, out: *mut c_void) -> u32 {
                    CALLS_EXPORT[XiFn::GetKeystroke as usize].fetch_add(1, Ordering::Relaxed);
                    core3(user, res, out, GET_KEYSTROKE.get())
                }

                /// Hook this module's exports if it is loaded and not hooked yet.
                pub fn hook_if_loaded() -> Result<bool, String> {
                    if HOOKED.load(Ordering::SeqCst) || !mh::module_loaded(DLL) {
                        return Ok(false);
                    }
                    unsafe {
                        mh::hook_export(DLL, "XInputGetState", detour_ptr!(get_state), &GET_STATE)?;
                        HOOKED.store(true, Ordering::SeqCst);
                        let _ = mh::hook_export(DLL, "XInputSetState", detour_ptr!(set_state), &SET_STATE);
                        let _ = mh::hook_export(DLL, "XInputGetCapabilities", detour_ptr!(get_caps), &GET_CAPS);
                        let _ = mh::hook_export(DLL, "XInputGetBatteryInformation", detour_ptr!(get_battery), &GET_BATTERY);
                        let _ = mh::hook_export(DLL, "XInputGetKeystroke", detour_ptr!(get_keystroke), &GET_KEYSTROKE);
                        let _ = mh::hook_export_ordinal(DLL, 100, detour_ptr!(get_state_ex), &GET_STATE_EX);
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

    /// Called by the loader hooks for every module load: hook a newly loaded xinput module's
    /// exports, and re-scan IATs (a module that just loaded may import xinput, and the overlay
    /// may have just re-pointed existing imports).
    pub fn on_module_loaded(_stem: &str) {
        for m in MODULES {
            let _ = hook_by_stem(m.trim_end_matches(".dll"));
        }
        install_iat();
    }

    /// Hook every xinput export already loaded, then patch IATs. Succeeds even if no xinput
    /// module is loaded yet (the loader hooks pick them up later).
    pub fn install() -> Result<(), Vec<String>> {
        let mut errors = Vec::new();
        for m in MODULES {
            if let Err(e) = hook_by_stem(m.trim_end_matches(".dll")) {
                errors.push(e);
            }
        }
        install_iat();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Periodic: re-apply the IAT patch, so an overlay that re-points xinput imports after we
    /// first ran is put back behind us.
    pub fn rescan() {
        install_iat();
    }

    /// One-line-per-function diagnostics for `state.json`.
    pub fn debug_lines() -> Vec<String> {
        let mut out = Vec::with_capacity(N + 1);
        out.push(format!(
            "xinput: iat_scans={} iat_patched_slots={} assigned_slot={:?}",
            IAT_SCANS.load(Ordering::Relaxed),
            IAT_PATCHED.load(Ordering::SeqCst),
            state::xinput_slot(),
        ));
        for i in 0..N {
            let ex = CALLS_EXPORT[i].load(Ordering::Relaxed);
            let ia = CALLS_IAT[i].load(Ordering::Relaxed);
            let orig = IAT_ORIG[i].load(Ordering::SeqCst);
            if ex != 0 || ia != 0 || !orig.is_null() {
                out.push(format!(
                    "  {}: export_calls={ex} iat_calls={ia} iat_orig={orig:p}",
                    NAMES[i]
                ));
            }
        }
        out
    }
}

#[cfg(windows)]
pub use imp::{debug_lines, install, on_module_loaded, rescan};

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
