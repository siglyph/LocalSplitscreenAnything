//! Keep the game windowed at the graphics-API level:
//!
//! - DXGI: `CreateDXGIFactory{,1,2}` are hooked so every factory's `CreateSwapChain` /
//!   `CreateSwapChainForHwnd` is hooked (vtable method, shared by all factories). Those force
//!   `Windowed = TRUE`, then hook the swap chain's `SetFullscreenState`, which refuses
//!   `TRUE`, and disable DXGI's own Alt+Enter handling for the window.
//! - `D3D11CreateDeviceAndSwapChain` gets the same treatment for games that bypass the
//!   factory.
//! - D3D9: `Direct3DCreate9{,Ex}` lead to `IDirect3D9::CreateDevice{,Ex}` and
//!   `IDirect3DDevice9::Reset` hooks that set `Windowed = TRUE` in the presentation
//!   parameters.
//!
//! Hooked when the module is already loaded at install, or later through `loader`.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::core::{Interface, GUID};
use windows::Win32::Graphics::Direct3D9::D3DPRESENT_PARAMETERS;
use windows::Win32::Graphics::Dxgi::{
    IDXGIFactory2, DXGI_MWA_NO_ALT_ENTER, DXGI_MWA_NO_WINDOW_CHANGES, DXGI_SWAP_CHAIN_DESC,
    DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FULLSCREEN_DESC,
};

use super::mh::{self, Orig};
use super::state;
use crate::detour_ptr;

const S_OK: i32 = 0;

// IDXGIFactory vtable slots (IUnknown 0-2, IDXGIObject 3-6).
const SLOT_FACTORY_MAKE_WINDOW_ASSOCIATION: usize = 8;
const SLOT_FACTORY_CREATE_SWAP_CHAIN: usize = 10;
const SLOT_FACTORY2_CREATE_SWAP_CHAIN_FOR_HWND: usize = 15;
// IDXGISwapChain vtable slot (IUnknown 0-2, IDXGIObject 3-6, IDXGIDeviceSubObject 7).
const SLOT_SWAPCHAIN_SET_FULLSCREEN_STATE: usize = 10;
// IDirect3D9 / IDirect3D9Ex / IDirect3DDevice9 vtable slots.
const SLOT_D3D9_CREATE_DEVICE: usize = 16;
const SLOT_D3D9EX_CREATE_DEVICE_EX: usize = 20;
const SLOT_D3D9DEVICE_RESET: usize = 16;

type CreateFactoryFn = unsafe extern "system" fn(*const GUID, *mut *mut c_void) -> i32;
type CreateFactory2Fn = unsafe extern "system" fn(u32, *const GUID, *mut *mut c_void) -> i32;
type QueryInterfaceFn = unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32;
type ReleaseFn = unsafe extern "system" fn(*mut c_void) -> u32;
type MakeWindowAssociationFn = unsafe extern "system" fn(*mut c_void, *mut c_void, u32) -> i32;
type CreateSwapChainFn = unsafe extern "system" fn(
    *mut c_void, *mut c_void, *const DXGI_SWAP_CHAIN_DESC, *mut *mut c_void,
) -> i32;
type CreateSwapChainForHwndFn = unsafe extern "system" fn(
    *mut c_void, *mut c_void, *mut c_void, *const DXGI_SWAP_CHAIN_DESC1,
    *const DXGI_SWAP_CHAIN_FULLSCREEN_DESC, *mut c_void, *mut *mut c_void,
) -> i32;
type SetFullscreenStateFn = unsafe extern "system" fn(*mut c_void, i32, *mut c_void) -> i32;
type D3D11CreateDeviceAndSwapChainFn = unsafe extern "system" fn(
    *mut c_void, u32, *mut c_void, u32, *const u32, u32, u32,
    *const DXGI_SWAP_CHAIN_DESC, *mut *mut c_void, *mut *mut c_void, *mut u32, *mut *mut c_void,
) -> i32;
type Direct3DCreate9Fn = unsafe extern "system" fn(u32) -> *mut c_void;
type Direct3DCreate9ExFn = unsafe extern "system" fn(u32, *mut *mut c_void) -> i32;
type D3D9CreateDeviceFn = unsafe extern "system" fn(
    *mut c_void, u32, u32, *mut c_void, u32, *mut D3DPRESENT_PARAMETERS, *mut *mut c_void,
) -> i32;
type D3D9CreateDeviceExFn = unsafe extern "system" fn(
    *mut c_void, u32, u32, *mut c_void, u32, *mut D3DPRESENT_PARAMETERS, *mut c_void,
    *mut *mut c_void,
) -> i32;
type D3D9ResetFn = unsafe extern "system" fn(*mut c_void, *mut D3DPRESENT_PARAMETERS) -> i32;

static CREATE_DXGI_FACTORY: Orig<CreateFactoryFn> = Orig::new();
static CREATE_DXGI_FACTORY1: Orig<CreateFactoryFn> = Orig::new();
static CREATE_DXGI_FACTORY2: Orig<CreateFactory2Fn> = Orig::new();
static CREATE_SWAP_CHAIN: Orig<CreateSwapChainFn> = Orig::new();
static CREATE_SWAP_CHAIN_FOR_HWND: Orig<CreateSwapChainForHwndFn> = Orig::new();
static SET_FULLSCREEN_STATE: Orig<SetFullscreenStateFn> = Orig::new();
static D3D11_CREATE_DEVICE_AND_SWAP_CHAIN: Orig<D3D11CreateDeviceAndSwapChainFn> = Orig::new();
static DIRECT3D_CREATE9: Orig<Direct3DCreate9Fn> = Orig::new();
static DIRECT3D_CREATE9_EX: Orig<Direct3DCreate9ExFn> = Orig::new();
static D3D9_CREATE_DEVICE: Orig<D3D9CreateDeviceFn> = Orig::new();
static D3D9_CREATE_DEVICE_EX: Orig<D3D9CreateDeviceExFn> = Orig::new();
static D3D9_RESET: Orig<D3D9ResetFn> = Orig::new();

static DXGI_HOOKED: AtomicBool = AtomicBool::new(false);
static D3D11_HOOKED: AtomicBool = AtomicBool::new(false);
static D3D9_HOOKED: AtomicBool = AtomicBool::new(false);

fn forced() -> bool {
    state::has(state::F_FORCE_WINDOWED)
}

/// # Safety
/// `obj` must be a live COM object pointer.
unsafe fn vslot(obj: *mut c_void, index: usize) -> *mut c_void {
    let vtable = *(obj as *const *const *mut c_void);
    *vtable.add(index)
}

/// Hook a vtable method once (the implementation is shared by every object of the class).
unsafe fn hook_method<T: Copy>(obj: *mut c_void, slot: usize, detour: *mut c_void, orig: &Orig<T>) {
    if obj.is_null() || orig.is_set() {
        return;
    }
    let target = vslot(obj, slot);
    let _ = mh::hook_address(target, detour, orig);
}

// ---------------------------------------------------------------- DXGI

unsafe fn hook_swapchain(swapchain: *mut c_void) {
    hook_method(
        swapchain,
        SLOT_SWAPCHAIN_SET_FULLSCREEN_STATE,
        detour_ptr!(set_fullscreen_state),
        &SET_FULLSCREEN_STATE,
    );
}

unsafe fn disable_alt_enter(factory: *mut c_void, hwnd: *mut c_void) {
    if factory.is_null() || hwnd.is_null() {
        return;
    }
    let f: MakeWindowAssociationFn =
        std::mem::transmute(vslot(factory, SLOT_FACTORY_MAKE_WINDOW_ASSOCIATION));
    let _ = f(factory, hwnd, (DXGI_MWA_NO_ALT_ENTER | DXGI_MWA_NO_WINDOW_CHANGES).0);
}

unsafe extern "system" fn set_fullscreen_state(this: *mut c_void, fullscreen: i32, target: *mut c_void) -> i32 {
    let Some(orig) = SET_FULLSCREEN_STATE.get() else {
        return S_OK;
    };
    if forced() && fullscreen != 0 {
        return S_OK;
    }
    orig(this, fullscreen, target)
}

unsafe extern "system" fn create_swap_chain(
    this: *mut c_void, device: *mut c_void, desc: *const DXGI_SWAP_CHAIN_DESC, out: *mut *mut c_void,
) -> i32 {
    let Some(orig) = CREATE_SWAP_CHAIN.get() else {
        return -1;
    };
    let mut patched;
    let desc = if forced() && !desc.is_null() {
        patched = *desc;
        patched.Windowed = true.into();
        &patched as *const DXGI_SWAP_CHAIN_DESC
    } else {
        desc
    };
    let hr = orig(this, device, desc, out);
    if hr == S_OK && !out.is_null() && !(*out).is_null() {
        hook_swapchain(*out);
        if forced() && !desc.is_null() {
            disable_alt_enter(this, (*desc).OutputWindow.0);
        }
    }
    hr
}

unsafe extern "system" fn create_swap_chain_for_hwnd(
    this: *mut c_void, device: *mut c_void, hwnd: *mut c_void, desc: *const DXGI_SWAP_CHAIN_DESC1,
    fullscreen: *const DXGI_SWAP_CHAIN_FULLSCREEN_DESC, restrict: *mut c_void, out: *mut *mut c_void,
) -> i32 {
    let Some(orig) = CREATE_SWAP_CHAIN_FOR_HWND.get() else {
        return -1;
    };
    let mut patched;
    let fullscreen = if forced() && !fullscreen.is_null() {
        patched = *fullscreen;
        patched.Windowed = true.into();
        &patched as *const DXGI_SWAP_CHAIN_FULLSCREEN_DESC
    } else {
        fullscreen
    };
    let hr = orig(this, device, hwnd, desc, fullscreen, restrict, out);
    if hr == S_OK && !out.is_null() && !(*out).is_null() {
        hook_swapchain(*out);
        if forced() {
            disable_alt_enter(this, hwnd);
        }
    }
    hr
}

unsafe fn hook_factory(factory: *mut c_void) {
    if factory.is_null() {
        return;
    }
    hook_method(
        factory,
        SLOT_FACTORY_CREATE_SWAP_CHAIN,
        detour_ptr!(create_swap_chain),
        &CREATE_SWAP_CHAIN,
    );
    if CREATE_SWAP_CHAIN_FOR_HWND.is_set() {
        return;
    }
    let qi: QueryInterfaceFn = std::mem::transmute(vslot(factory, 0));
    let mut f2: *mut c_void = std::ptr::null_mut();
    if qi(factory, &IDXGIFactory2::IID, &mut f2) == S_OK && !f2.is_null() {
        hook_method(
            f2,
            SLOT_FACTORY2_CREATE_SWAP_CHAIN_FOR_HWND,
            detour_ptr!(create_swap_chain_for_hwnd),
            &CREATE_SWAP_CHAIN_FOR_HWND,
        );
        let release: ReleaseFn = std::mem::transmute(vslot(f2, 2));
        release(f2);
    }
}

unsafe extern "system" fn create_dxgi_factory(riid: *const GUID, out: *mut *mut c_void) -> i32 {
    let Some(orig) = CREATE_DXGI_FACTORY.get() else {
        return -1;
    };
    let hr = orig(riid, out);
    if hr == S_OK && !out.is_null() {
        hook_factory(*out);
    }
    hr
}

unsafe extern "system" fn create_dxgi_factory1(riid: *const GUID, out: *mut *mut c_void) -> i32 {
    let Some(orig) = CREATE_DXGI_FACTORY1.get() else {
        return -1;
    };
    let hr = orig(riid, out);
    if hr == S_OK && !out.is_null() {
        hook_factory(*out);
    }
    hr
}

unsafe extern "system" fn create_dxgi_factory2(flags: u32, riid: *const GUID, out: *mut *mut c_void) -> i32 {
    let Some(orig) = CREATE_DXGI_FACTORY2.get() else {
        return -1;
    };
    let hr = orig(flags, riid, out);
    if hr == S_OK && !out.is_null() {
        hook_factory(*out);
    }
    hr
}

fn hook_dxgi() {
    if DXGI_HOOKED.load(Ordering::SeqCst) || !mh::module_loaded("dxgi.dll") {
        return;
    }
    DXGI_HOOKED.store(true, Ordering::SeqCst);
    unsafe {
        let _ = mh::hook_export("dxgi.dll", "CreateDXGIFactory", detour_ptr!(create_dxgi_factory), &CREATE_DXGI_FACTORY);
        let _ = mh::hook_export("dxgi.dll", "CreateDXGIFactory1", detour_ptr!(create_dxgi_factory1), &CREATE_DXGI_FACTORY1);
        let _ = mh::hook_export("dxgi.dll", "CreateDXGIFactory2", detour_ptr!(create_dxgi_factory2), &CREATE_DXGI_FACTORY2);
    }
}

// ---------------------------------------------------------------- D3D11

unsafe extern "system" fn d3d11_create_device_and_swap_chain(
    adapter: *mut c_void, driver_type: u32, software: *mut c_void, flags: u32,
    levels: *const u32, level_count: u32, sdk: u32, desc: *const DXGI_SWAP_CHAIN_DESC,
    swapchain: *mut *mut c_void, device: *mut *mut c_void, level_out: *mut u32, context: *mut *mut c_void,
) -> i32 {
    let Some(orig) = D3D11_CREATE_DEVICE_AND_SWAP_CHAIN.get() else {
        return -1;
    };
    let mut patched;
    let desc = if forced() && !desc.is_null() {
        patched = *desc;
        patched.Windowed = true.into();
        &patched as *const DXGI_SWAP_CHAIN_DESC
    } else {
        desc
    };
    let hr = orig(adapter, driver_type, software, flags, levels, level_count, sdk, desc, swapchain, device, level_out, context);
    if hr == S_OK && !swapchain.is_null() && !(*swapchain).is_null() {
        hook_swapchain(*swapchain);
    }
    hr
}

fn hook_d3d11() {
    if D3D11_HOOKED.load(Ordering::SeqCst) || !mh::module_loaded("d3d11.dll") {
        return;
    }
    D3D11_HOOKED.store(true, Ordering::SeqCst);
    unsafe {
        let _ = mh::hook_export(
            "d3d11.dll",
            "D3D11CreateDeviceAndSwapChain",
            detour_ptr!(d3d11_create_device_and_swap_chain),
            &D3D11_CREATE_DEVICE_AND_SWAP_CHAIN,
        );
    }
}

// ---------------------------------------------------------------- D3D9

/// Make presentation parameters describe a windowed device.
pub fn force_windowed_pp(pp: &mut D3DPRESENT_PARAMETERS) {
    pp.Windowed = true.into();
    pp.FullScreen_RefreshRateInHz = 0;
}

unsafe fn patch_pp(pp: *mut D3DPRESENT_PARAMETERS) {
    if forced() && !pp.is_null() {
        force_windowed_pp(&mut *pp);
    }
}

unsafe extern "system" fn d3d9_reset(this: *mut c_void, pp: *mut D3DPRESENT_PARAMETERS) -> i32 {
    let Some(orig) = D3D9_RESET.get() else {
        return -1;
    };
    patch_pp(pp);
    orig(this, pp)
}

unsafe fn hook_d3d9_device(device: *mut c_void) {
    hook_method(device, SLOT_D3D9DEVICE_RESET, detour_ptr!(d3d9_reset), &D3D9_RESET);
}

unsafe extern "system" fn d3d9_create_device(
    this: *mut c_void, adapter: u32, devtype: u32, focus: *mut c_void, flags: u32,
    pp: *mut D3DPRESENT_PARAMETERS, out: *mut *mut c_void,
) -> i32 {
    let Some(orig) = D3D9_CREATE_DEVICE.get() else {
        return -1;
    };
    patch_pp(pp);
    let hr = orig(this, adapter, devtype, focus, flags, pp, out);
    if hr == S_OK && !out.is_null() && !(*out).is_null() {
        hook_d3d9_device(*out);
    }
    hr
}

unsafe extern "system" fn d3d9_create_device_ex(
    this: *mut c_void, adapter: u32, devtype: u32, focus: *mut c_void, flags: u32,
    pp: *mut D3DPRESENT_PARAMETERS, mode: *mut c_void, out: *mut *mut c_void,
) -> i32 {
    let Some(orig) = D3D9_CREATE_DEVICE_EX.get() else {
        return -1;
    };
    // A fullscreen display mode must be NULL for a windowed device.
    let mode = if forced() { std::ptr::null_mut() } else { mode };
    patch_pp(pp);
    let hr = orig(this, adapter, devtype, focus, flags, pp, mode, out);
    if hr == S_OK && !out.is_null() && !(*out).is_null() {
        hook_d3d9_device(*out);
    }
    hr
}

unsafe extern "system" fn direct3d_create9(sdk: u32) -> *mut c_void {
    let Some(orig) = DIRECT3D_CREATE9.get() else {
        return std::ptr::null_mut();
    };
    let d3d = orig(sdk);
    hook_method(d3d, SLOT_D3D9_CREATE_DEVICE, detour_ptr!(d3d9_create_device), &D3D9_CREATE_DEVICE);
    d3d
}

unsafe extern "system" fn direct3d_create9_ex(sdk: u32, out: *mut *mut c_void) -> i32 {
    let Some(orig) = DIRECT3D_CREATE9_EX.get() else {
        return -1;
    };
    let hr = orig(sdk, out);
    if hr == S_OK && !out.is_null() && !(*out).is_null() {
        hook_method(*out, SLOT_D3D9_CREATE_DEVICE, detour_ptr!(d3d9_create_device), &D3D9_CREATE_DEVICE);
        hook_method(*out, SLOT_D3D9EX_CREATE_DEVICE_EX, detour_ptr!(d3d9_create_device_ex), &D3D9_CREATE_DEVICE_EX);
    }
    hr
}

fn hook_d3d9() {
    if D3D9_HOOKED.load(Ordering::SeqCst) || !mh::module_loaded("d3d9.dll") {
        return;
    }
    D3D9_HOOKED.store(true, Ordering::SeqCst);
    unsafe {
        let _ = mh::hook_export("d3d9.dll", "Direct3DCreate9", detour_ptr!(direct3d_create9), &DIRECT3D_CREATE9);
        let _ = mh::hook_export("d3d9.dll", "Direct3DCreate9Ex", detour_ptr!(direct3d_create9_ex), &DIRECT3D_CREATE9_EX);
    }
}

/// Called by the loader hooks for every module load.
pub fn on_module_loaded(stem: &str) {
    match stem {
        "dxgi" => hook_dxgi(),
        "d3d11" => hook_d3d11(),
        "d3d9" => hook_d3d9(),
        _ => {}
    }
}

/// Hook whichever graphics modules are already loaded; the rest are caught on load.
pub fn install() -> Result<(), Vec<String>> {
    hook_dxgi();
    hook_d3d11();
    hook_d3d9();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presentation_parameters_become_windowed() {
        let mut pp = D3DPRESENT_PARAMETERS {
            Windowed: false.into(),
            FullScreen_RefreshRateInHz: 60,
            ..Default::default()
        };
        force_windowed_pp(&mut pp);
        assert!(pp.Windowed.as_bool());
        assert_eq!(pp.FullScreen_RefreshRateInHz, 0);
    }
}
