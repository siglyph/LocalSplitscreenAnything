//! Thin safe-ish wrapper over MinHook (`minhook-sys`) plus [`Orig`], a typed slot for the
//! trampoline ("original function") pointer MinHook hands back for every hook.
//!
//! All hooks are created and enabled immediately; MinHook serialises its own bookkeeping and
//! suspends other threads while it patches code, so this is safe to call from any thread,
//! including from inside a `LoadLibrary` detour.

use std::ffi::c_void;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicPtr, Ordering};

/// Holds the pointer MinHook returns as "original" for a hooked function, typed as the
/// function pointer type `T` so detours can call through without casts.
pub struct Orig<T> {
    ptr: AtomicPtr<c_void>,
    _t: PhantomData<T>,
}

// `T` is always a plain function pointer type; the slot itself is an atomic.
unsafe impl<T> Sync for Orig<T> {}
unsafe impl<T> Send for Orig<T> {}

impl<T: Copy> Orig<T> {
    pub const fn new() -> Self {
        Self {
            ptr: AtomicPtr::new(std::ptr::null_mut()),
            _t: PhantomData,
        }
    }

    pub fn set(&self, p: *mut c_void) {
        self.ptr.store(p, Ordering::SeqCst);
    }

    pub fn is_set(&self) -> bool {
        !self.ptr.load(Ordering::SeqCst).is_null()
    }

    /// The trampoline, or `None` before the hook exists.
    pub fn get(&self) -> Option<T> {
        let p = self.ptr.load(Ordering::SeqCst);
        if p.is_null() {
            return None;
        }
        debug_assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<*mut c_void>());
        // SAFETY: `T` is a function pointer type of pointer size, and MinHook guarantees the
        // trampoline matches the signature of the function that was hooked.
        Some(unsafe { std::mem::transmute_copy::<*mut c_void, T>(&p) })
    }
}

impl<T: Copy> Default for Orig<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(windows)]
mod imp {
    use super::Orig;
    use std::ffi::{c_void, CStr, CString};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Once;

    /// While set, new hooks are only queued; [`apply_queued`] enables them all in one go.
    /// Every `MH_EnableHook` freezes all threads of the process, so enabling the ~70
    /// install-time hooks one by one costs well over a second; one batch costs one freeze.
    static BATCHING: AtomicBool = AtomicBool::new(false);

    /// Queue instead of enabling hooks created from now on, until [`apply_queued`].
    pub fn begin_batch() {
        BATCHING.store(true, Ordering::SeqCst);
    }

    /// Enable every queued hook with a single thread freeze and leave batch mode.
    pub fn apply_queued() -> Result<(), String> {
        BATCHING.store(false, Ordering::SeqCst);
        let s = unsafe { mh::MH_ApplyQueued() };
        if s == mh::MH_OK {
            Ok(())
        } else {
            Err(format!("MH_ApplyQueued: {}", status_text(s)))
        }
    }

    use minhook_sys as mh;
    use windows::core::{PCSTR, PCWSTR};
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};

    fn status_text(status: mh::MH_STATUS) -> String {
        unsafe {
            let p = mh::MH_StatusToString(status);
            if p.is_null() {
                format!("MH_STATUS {status}")
            } else {
                CStr::from_ptr(p).to_string_lossy().into_owned()
            }
        }
    }

    /// `MH_Initialize`, once per process.
    pub fn init() -> Result<(), String> {
        static ONCE: Once = Once::new();
        let mut result = Ok(());
        ONCE.call_once(|| {
            let s = unsafe { mh::MH_Initialize() };
            if s != mh::MH_OK && s != mh::MH_ERROR_ALREADY_INITIALIZED {
                result = Err(format!("MH_Initialize: {}", status_text(s)));
            }
        });
        result
    }

    pub fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Base address of `module` if it is loaded in this process.
    pub fn module_handle(module: &str) -> Option<HMODULE> {
        let w = wide(module);
        unsafe { GetModuleHandleW(PCWSTR(w.as_ptr())).ok() }
    }

    pub fn module_loaded(module: &str) -> bool {
        module_handle(module).is_some()
    }

    fn export_by_name(module: &str, name: &str) -> Result<*mut c_void, String> {
        let h = module_handle(module).ok_or_else(|| format!("{module} is not loaded"))?;
        let c = CString::new(name).map_err(|_| format!("bad export name {name:?}"))?;
        let p = unsafe { GetProcAddress(h, PCSTR(c.as_ptr() as *const u8)) };
        p.map(|f| f as *mut c_void)
            .ok_or_else(|| format!("{module} has no export {name}"))
    }

    fn export_by_ordinal(module: &str, ordinal: u16) -> Result<*mut c_void, String> {
        let h = module_handle(module).ok_or_else(|| format!("{module} is not loaded"))?;
        let p = unsafe { GetProcAddress(h, PCSTR(ordinal as usize as *const u8)) };
        p.map(|f| f as *mut c_void)
            .ok_or_else(|| format!("{module} has no export #{ordinal}"))
    }

    /// Create and enable a hook on an arbitrary code address.
    ///
    /// # Safety
    /// `detour` must have exactly the calling convention and signature of the function at
    /// `target`, and `orig` must be typed accordingly.
    pub unsafe fn hook_address<T: Copy>(
        target: *mut c_void,
        detour: *mut c_void,
        orig: &Orig<T>,
    ) -> Result<(), String> {
        init()?;
        let mut original: *mut c_void = std::ptr::null_mut();
        let s = mh::MH_CreateHook(target, detour, &mut original);
        if s != mh::MH_OK {
            return Err(format!("MH_CreateHook({target:p}): {}", status_text(s)));
        }
        orig.set(original);
        let (s, what) = if BATCHING.load(Ordering::SeqCst) {
            (mh::MH_QueueEnableHook(target), "MH_QueueEnableHook")
        } else {
            (mh::MH_EnableHook(target), "MH_EnableHook")
        };
        if s != mh::MH_OK {
            let _ = mh::MH_RemoveHook(target);
            orig.set(std::ptr::null_mut());
            return Err(format!("{what}({target:p}): {}", status_text(s)));
        }
        Ok(())
    }

    /// Hook an export of an already loaded module by name.
    ///
    /// # Safety
    /// See [`hook_address`].
    pub unsafe fn hook_export<T: Copy>(
        module: &str,
        name: &str,
        detour: *mut c_void,
        orig: &Orig<T>,
    ) -> Result<(), String> {
        let target = export_by_name(module, name)?;
        hook_address(target, detour, orig).map_err(|e| format!("{module}!{name}: {e}"))
    }

    /// Hook an export of an already loaded module by ordinal (for unnamed exports such as
    /// `XInputGetStateEx`, ordinal 100).
    ///
    /// # Safety
    /// See [`hook_address`].
    pub unsafe fn hook_export_ordinal<T: Copy>(
        module: &str,
        ordinal: u16,
        detour: *mut c_void,
        orig: &Orig<T>,
    ) -> Result<(), String> {
        let target = export_by_ordinal(module, ordinal)?;
        hook_address(target, detour, orig).map_err(|e| format!("{module}!#{ordinal}: {e}"))
    }
}

#[cfg(windows)]
pub use imp::*;

/// Cast a detour function item to the pointer MinHook wants. Goes through `*const ()` rather
/// than `usize` so it does not trip the `function_casts_as_integer` lint.
#[macro_export]
macro_rules! detour_ptr {
    ($f:expr) => {
        $f as *const () as *mut ::std::ffi::c_void
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orig_round_trips_a_function_pointer() {
        extern "system" fn f(x: u32) -> u32 {
            x + 1
        }
        let slot: Orig<extern "system" fn(u32) -> u32> = Orig::new();
        assert!(slot.get().is_none());
        slot.set(f as *const () as *mut c_void);
        assert!(slot.is_set());
        assert_eq!(slot.get().unwrap()(41), 42);
    }
}
