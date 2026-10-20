//! Fake focus: the game keeps believing its window is the active, focused, foreground
//! window while another instance actually has it, so it keeps rendering and polling pads
//! instead of pausing or muting input. Same technique as Proto Input's FocusHooks: a few
//! user32 queries answer with the main window, a window subclass drops the deactivation
//! messages, and a small loop keeps re-sending the activation ones.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, GetWindowThreadProcessId, IsWindow, PostMessageW, GWLP_WNDPROC, WA_ACTIVE,
    WA_INACTIVE, WM_ACTIVATE, WM_ACTIVATEAPP, WM_KILLFOCUS, WM_NCACTIVATE, WM_SETFOCUS, WNDPROC,
};

/// Replace a window's procedure, returning the previous one as a raw pointer value.
///
/// `SetWindowLongPtrW` takes/returns `isize` on 64-bit, but on 32-bit it is an alias for
/// `SetWindowLongW`, which takes/returns `i32`; this hides that difference.
#[cfg(target_pointer_width = "64")]
unsafe fn set_wndproc(hwnd: HWND, proc: *const ()) -> usize {
    use windows::Win32::UI::WindowsAndMessaging::SetWindowLongPtrW;
    SetWindowLongPtrW(hwnd, GWLP_WNDPROC, proc as isize) as usize
}

#[cfg(target_pointer_width = "32")]
unsafe fn set_wndproc(hwnd: HWND, proc: *const ()) -> usize {
    use windows::Win32::UI::WindowsAndMessaging::SetWindowLongW;
    SetWindowLongW(hwnd, GWLP_WNDPROC, proc as i32) as u32 as usize
}

use super::mh::{self, Orig};
use super::state;
use crate::detour_ptr;

/// Interval of the activation-message loop (Proto Input uses 5 ms as well).
const LOOP_INTERVAL: Duration = Duration::from_millis(5);

type GetHwndFn = unsafe extern "system" fn() -> *mut c_void;
type SetFocusFn = unsafe extern "system" fn(*mut c_void) -> *mut c_void;
type SetForegroundFn = unsafe extern "system" fn(*mut c_void) -> i32;

static GET_FOREGROUND_WINDOW: Orig<GetHwndFn> = Orig::new();
static GET_ACTIVE_WINDOW: Orig<GetHwndFn> = Orig::new();
static GET_FOCUS: Orig<GetHwndFn> = Orig::new();
static SET_FOCUS: Orig<SetFocusFn> = Orig::new();
static SET_FOREGROUND_WINDOW: Orig<SetForegroundFn> = Orig::new();

/// The window procedure we replaced on the main window (0 until subclassed).
static PREV_WNDPROC: AtomicUsize = AtomicUsize::new(0);
/// Which window `PREV_WNDPROC` belongs to.
static SUBCLASSED: AtomicUsize = AtomicUsize::new(0);
static LOOP_STARTED: AtomicBool = AtomicBool::new(false);

fn fake() -> Option<*mut c_void> {
    if !state::has(state::F_FAKE_FOCUS) {
        return None;
    }
    let h = state::main_window();
    (h != 0).then_some(h as *mut c_void)
}

/// Whether the real foreground window belongs to this process.
unsafe fn really_foreground() -> bool {
    let Some(orig) = GET_FOREGROUND_WINDOW.get() else {
        return true;
    };
    let fg = orig();
    if fg.is_null() {
        return false;
    }
    let mut pid = 0u32;
    GetWindowThreadProcessId(HWND(fg), Some(&mut pid));
    pid == GetCurrentProcessId()
}

unsafe extern "system" fn get_foreground_window() -> *mut c_void {
    match (fake(), GET_FOREGROUND_WINDOW.get()) {
        (Some(h), _) => h,
        (None, Some(orig)) => orig(),
        (None, None) => std::ptr::null_mut(),
    }
}

unsafe extern "system" fn get_active_window() -> *mut c_void {
    match (fake(), GET_ACTIVE_WINDOW.get()) {
        (Some(h), _) => h,
        (None, Some(orig)) => orig(),
        (None, None) => std::ptr::null_mut(),
    }
}

unsafe extern "system" fn get_focus() -> *mut c_void {
    match (fake(), GET_FOCUS.get()) {
        (Some(h), _) => h,
        (None, Some(orig)) => orig(),
        (None, None) => std::ptr::null_mut(),
    }
}

/// Only let the game take focus when it already owns the foreground; otherwise pretend it
/// worked so the other instance keeps keyboard/mouse.
unsafe extern "system" fn set_focus(h: *mut c_void) -> *mut c_void {
    let Some(orig) = SET_FOCUS.get() else {
        return std::ptr::null_mut();
    };
    match fake() {
        Some(main) if !really_foreground() => main,
        _ => orig(h),
    }
}

unsafe extern "system" fn set_foreground_window(h: *mut c_void) -> i32 {
    let Some(orig) = SET_FOREGROUND_WINDOW.get() else {
        return 0;
    };
    match fake() {
        Some(_) if !really_foreground() => 1,
        _ => orig(h),
    }
}

/// Subclass procedure: drops the messages that tell the game it lost focus.
unsafe extern "system" fn subclass_proc(
    h: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if state::has(state::F_FAKE_FOCUS) {
        match msg {
            WM_KILLFOCUS => return LRESULT(0),
            WM_ACTIVATE if (wparam.0 & 0xFFFF) as u32 == WA_INACTIVE => return LRESULT(0),
            WM_ACTIVATEAPP if wparam.0 == 0 => return LRESULT(0),
            WM_NCACTIVATE if wparam.0 == 0 => return LRESULT(1),
            _ => {}
        }
    }
    let prev = PREV_WNDPROC.load(Ordering::Relaxed);
    let prev: WNDPROC = if prev == 0 {
        None
    } else {
        Some(std::mem::transmute::<usize, unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT>(prev))
    };
    CallWindowProcW(prev, h, msg, wparam, lparam)
}

/// Subclass the main window once it is known (or re-known after the game recreated it).
pub fn on_main_window(h: usize) {
    if SUBCLASSED.load(Ordering::SeqCst) == h {
        return;
    }
    unsafe {
        let prev = set_wndproc(HWND(h as *mut c_void), subclass_proc as *const ());
        if prev != 0 {
            PREV_WNDPROC.store(prev, Ordering::SeqCst);
            SUBCLASSED.store(h, Ordering::SeqCst);
        }
    }
}

fn focus_loop() {
    loop {
        std::thread::sleep(LOOP_INTERVAL);
        let Some(h) = fake() else { continue };
        let hwnd = HWND(h);
        unsafe {
            if !IsWindow(Some(hwnd)).as_bool() {
                continue;
            }
            let _ = PostMessageW(Some(hwnd), WM_ACTIVATE, WPARAM(WA_ACTIVE as usize), LPARAM(0));
            let _ = PostMessageW(Some(hwnd), WM_ACTIVATEAPP, WPARAM(1), LPARAM(0));
            let _ = PostMessageW(Some(hwnd), WM_NCACTIVATE, WPARAM(1), LPARAM(0));
            let _ = PostMessageW(Some(hwnd), WM_SETFOCUS, WPARAM(0), LPARAM(0));
        }
    }
}

pub fn install() -> Result<(), Vec<String>> {
    let results = unsafe {
        [
            mh::hook_export(
                "user32.dll",
                "GetForegroundWindow",
                detour_ptr!(get_foreground_window),
                &GET_FOREGROUND_WINDOW,
            ),
            mh::hook_export(
                "user32.dll",
                "GetActiveWindow",
                detour_ptr!(get_active_window),
                &GET_ACTIVE_WINDOW,
            ),
            mh::hook_export("user32.dll", "GetFocus", detour_ptr!(get_focus), &GET_FOCUS),
            mh::hook_export("user32.dll", "SetFocus", detour_ptr!(set_focus), &SET_FOCUS),
            mh::hook_export(
                "user32.dll",
                "SetForegroundWindow",
                detour_ptr!(set_foreground_window),
                &SET_FOREGROUND_WINDOW,
            ),
        ]
    };
    let errors: Vec<String> = results.into_iter().filter_map(Result::err).collect();
    if !errors.is_empty() {
        return Err(errors);
    }
    if !LOOP_STARTED.swap(true, Ordering::SeqCst) {
        std::thread::Builder::new()
            .name("lsa-focus".into())
            .spawn(focus_loop)
            .map_err(|e| vec![format!("focus loop thread: {e}")])?;
    }
    Ok(())
}
