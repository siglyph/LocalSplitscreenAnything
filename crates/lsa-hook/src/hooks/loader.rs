//! `LoadLibrary{A,W,ExA,ExW}` hooks: when the game (or SDL, Unity, Unreal, a D3D runtime)
//! loads an xinput or graphics module after we initialised, hook that module too.

use std::cell::Cell;
use std::ffi::c_void;
use std::sync::Mutex;

use super::mh::{self, Orig};
use crate::detour_ptr;

type LoadLibraryWFn = unsafe extern "system" fn(*const u16) -> *mut c_void;
type LoadLibraryAFn = unsafe extern "system" fn(*const u8) -> *mut c_void;
type LoadLibraryExWFn = unsafe extern "system" fn(*const u16, *mut c_void, u32) -> *mut c_void;
type LoadLibraryExAFn = unsafe extern "system" fn(*const u8, *mut c_void, u32) -> *mut c_void;

static LOAD_LIBRARY_W: Orig<LoadLibraryWFn> = Orig::new();
static LOAD_LIBRARY_A: Orig<LoadLibraryAFn> = Orig::new();
static LOAD_LIBRARY_EX_W: Orig<LoadLibraryExWFn> = Orig::new();
static LOAD_LIBRARY_EX_A: Orig<LoadLibraryExAFn> = Orig::new();

/// Serialises "is this module hooked yet" checks across threads loading modules at once.
static SCAN_LOCK: Mutex<()> = Mutex::new(());

thread_local! {
    static DEPTH: Cell<u32> = const { Cell::new(0) };
}

/// Lower-case file stem of a module path or name (`C:\x\XInput1_4.DLL` -> `xinput1_4`).
pub fn module_stem(name: &str) -> String {
    let base = name.rsplit(['\\', '/']).next().unwrap_or(name);
    let stem = match base.rfind('.') {
        Some(i) if base[i + 1..].eq_ignore_ascii_case("dll") => &base[..i],
        _ => base,
    };
    stem.to_ascii_lowercase()
}

/// # Safety
/// `p` is null or points to a NUL-terminated UTF-16 string.
pub unsafe fn wide_to_string(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0;
    while *p.add(len) != 0 {
        len += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
}

/// # Safety
/// `p` is null or points to a NUL-terminated byte string.
pub unsafe fn ansi_to_string(p: *const u8) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0;
    while *p.add(len) != 0 {
        len += 1;
    }
    String::from_utf8_lossy(std::slice::from_raw_parts(p, len)).into_owned()
}

/// Hook whatever interesting module `stem` names, if it is loaded now. Also used by the
/// initial scan at install time.
pub fn on_module_loaded(stem: &str) {
    let _guard = SCAN_LOCK.lock();
    super::xinput::on_module_loaded(stem);
    super::dx::on_module_loaded(stem);
}

fn after_load(name: String, handle: *mut c_void) {
    if handle.is_null() {
        return;
    }
    DEPTH.with(|d| {
        if d.get() > 0 {
            // Re-entered from our own hooking (MinHook, GetModuleHandle) or from an inner
            // LoadLibraryExW call made by LoadLibraryW: the outer call handles it.
            return;
        }
        d.set(1);
        on_module_loaded(&module_stem(&name));
        d.set(0);
    });
}

unsafe extern "system" fn load_library_w(name: *const u16) -> *mut c_void {
    let Some(orig) = LOAD_LIBRARY_W.get() else {
        return std::ptr::null_mut();
    };
    let h = orig(name);
    after_load(wide_to_string(name), h);
    h
}

unsafe extern "system" fn load_library_a(name: *const u8) -> *mut c_void {
    let Some(orig) = LOAD_LIBRARY_A.get() else {
        return std::ptr::null_mut();
    };
    let h = orig(name);
    after_load(ansi_to_string(name), h);
    h
}

unsafe extern "system" fn load_library_ex_w(
    name: *const u16,
    file: *mut c_void,
    flags: u32,
) -> *mut c_void {
    let Some(orig) = LOAD_LIBRARY_EX_W.get() else {
        return std::ptr::null_mut();
    };
    let h = orig(name, file, flags);
    after_load(wide_to_string(name), h);
    h
}

unsafe extern "system" fn load_library_ex_a(
    name: *const u8,
    file: *mut c_void,
    flags: u32,
) -> *mut c_void {
    let Some(orig) = LOAD_LIBRARY_EX_A.get() else {
        return std::ptr::null_mut();
    };
    let h = orig(name, file, flags);
    after_load(ansi_to_string(name), h);
    h
}

pub fn install() -> Result<(), Vec<String>> {
    let results = unsafe {
        [
            mh::hook_export(
                "kernel32.dll",
                "LoadLibraryW",
                detour_ptr!(load_library_w),
                &LOAD_LIBRARY_W,
            ),
            mh::hook_export(
                "kernel32.dll",
                "LoadLibraryA",
                detour_ptr!(load_library_a),
                &LOAD_LIBRARY_A,
            ),
            mh::hook_export(
                "kernel32.dll",
                "LoadLibraryExW",
                detour_ptr!(load_library_ex_w),
                &LOAD_LIBRARY_EX_W,
            ),
            mh::hook_export(
                "kernel32.dll",
                "LoadLibraryExA",
                detour_ptr!(load_library_ex_a),
                &LOAD_LIBRARY_EX_A,
            ),
        ]
    };
    let errors: Vec<String> = results.into_iter().filter_map(Result::err).collect();
    if errors.len() == 4 {
        Err(errors)
    } else {
        // Partial success still catches most loads (every path funnels into LoadLibraryExW).
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_stem_strips_path_and_dll_suffix() {
        assert_eq!(module_stem(r"C:\Windows\System32\XInput1_4.DLL"), "xinput1_4");
        assert_eq!(module_stem("xinput9_1_0"), "xinput9_1_0");
        assert_eq!(module_stem("d3d11.dll"), "d3d11");
        assert_eq!(
            module_stem("api-ms-win-core-file-l1-2-0"),
            "api-ms-win-core-file-l1-2-0"
        );
        assert_eq!(module_stem("some/dir/lib.so"), "lib.so");
    }
}
