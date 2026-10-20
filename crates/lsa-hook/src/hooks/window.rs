//! Window placement and screen-size lies, so the game "detects" a display the size of its
//! viewport and its main window stays there:
//!
//! - `CreateWindowExW/A`: a new top-level window of game size is created borderless at the
//!   viewport.
//! - `SetWindowPos`/`MoveWindow`: moves of the main window are redirected to the viewport.
//! - `GetSystemMetrics`, `GetDeviceCaps`, `GetMonitorInfoW/A`, `EnumDisplayMonitors`,
//!   `EnumDisplaySettings(Ex)W/A`, `SystemParametersInfoW/A(SPI_GETWORKAREA)` report the
//!   viewport as the (only) screen.
//! - `ChangeDisplaySettings(Ex)W/A` succeed without changing anything.
//! - `ClipCursor` is clamped to the viewport when `clip_cursor` is on.
//!
//! Also owns main-window detection (`refresh_main_window`) and the periodic re-pin.

use lsa_proto::PixelRect;

/// `CW_USEDEFAULT` as the i32 Win32 uses.
pub const CW_USEDEFAULT: i32 = i32::MIN;

pub const WS_CHILD: u32 = 0x4000_0000;
pub const WS_CAPTION: u32 = 0x00C0_0000;
pub const WS_BORDER: u32 = 0x0080_0000;
pub const WS_DLGFRAME: u32 = 0x0040_0000;
pub const WS_THICKFRAME: u32 = 0x0004_0000;
pub const WS_SYSMENU: u32 = 0x0008_0000;
pub const WS_MINIMIZEBOX: u32 = 0x0002_0000;
pub const WS_MAXIMIZEBOX: u32 = 0x0001_0000;
pub const WS_MAXIMIZE: u32 = 0x0100_0000;
pub const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
pub const WS_EX_DLGMODALFRAME: u32 = 0x0000_0001;
pub const WS_EX_WINDOWEDGE: u32 = 0x0000_0100;
pub const WS_EX_CLIENTEDGE: u32 = 0x0000_0200;
pub const WS_EX_STATICEDGE: u32 = 0x0002_0000;

pub const SWP_NOSIZE: u32 = 0x0001;
pub const SWP_NOMOVE: u32 = 0x0002;

pub const SM_CXSCREEN: i32 = 0;
pub const SM_CYSCREEN: i32 = 1;
pub const SM_CXFULLSCREEN: i32 = 16;
pub const SM_CYFULLSCREEN: i32 = 17;
pub const SM_CXVIRTUALSCREEN: i32 = 78;
pub const SM_CYVIRTUALSCREEN: i32 = 79;
pub const SM_CMONITORS: i32 = 80;

pub const HORZRES: i32 = 8;
pub const VERTRES: i32 = 10;

pub const ENUM_CURRENT_SETTINGS: u32 = 0xFFFF_FFFF;
pub const ENUM_REGISTRY_SETTINGS: u32 = 0xFFFF_FFFE;

/// Smallest window that counts as "the game window" rather than a helper/tooltip/IME window.
pub const MIN_GAME_WINDOW: (i32, i32) = (320, 200);

/// Window styles that give a window a frame; all removed so the client area is the viewport.
pub const FRAME_STYLES: u32 = WS_CAPTION
    | WS_BORDER
    | WS_DLGFRAME
    | WS_THICKFRAME
    | WS_SYSMENU
    | WS_MINIMIZEBOX
    | WS_MAXIMIZEBOX
    | WS_MAXIMIZE;
pub const FRAME_EX_STYLES: u32 =
    WS_EX_DLGMODALFRAME | WS_EX_WINDOWEDGE | WS_EX_CLIENTEDGE | WS_EX_STATICEDGE;

/// Placement for a window about to be created: `(style, ex_style, x, y, width, height)`, or
/// `None` to create it as requested.
pub fn plan_create(
    style: u32,
    ex_style: u32,
    has_parent: bool,
    width: i32,
    height: i32,
    viewport: PixelRect,
) -> Option<(u32, u32, i32, i32, i32, i32)> {
    if style & WS_CHILD != 0 || has_parent || ex_style & WS_EX_TOOLWINDOW != 0 {
        return None;
    }
    let default_size = width == CW_USEDEFAULT || height == CW_USEDEFAULT;
    if !default_size && (width < MIN_GAME_WINDOW.0 || height < MIN_GAME_WINDOW.1) {
        return None;
    }
    if viewport.width <= 0 || viewport.height <= 0 {
        return None;
    }
    Some((
        style & !FRAME_STYLES,
        ex_style & !FRAME_EX_STYLES,
        viewport.x,
        viewport.y,
        viewport.width,
        viewport.height,
    ))
}

/// Arguments for a `SetWindowPos` on the main window: always the viewport, with the
/// no-move/no-size flags cleared so the call actually applies it.
pub fn plan_set_window_pos(flags: u32, viewport: PixelRect) -> (i32, i32, i32, i32, u32) {
    (
        viewport.x,
        viewport.y,
        viewport.width,
        viewport.height,
        flags & !(SWP_NOMOVE | SWP_NOSIZE),
    )
}

/// What `GetSystemMetrics(index)` should answer, if we lie about it.
pub fn metric_override(index: i32, viewport: PixelRect) -> Option<i32> {
    match index {
        SM_CXSCREEN | SM_CXFULLSCREEN | SM_CXVIRTUALSCREEN => Some(viewport.width),
        SM_CYSCREEN | SM_CYFULLSCREEN | SM_CYVIRTUALSCREEN => Some(viewport.height),
        SM_CMONITORS => Some(1),
        _ => None,
    }
}

/// What `GetDeviceCaps(hdc, index)` should answer, if we lie about it.
pub fn device_cap_override(index: i32, viewport: PixelRect) -> Option<i32> {
    match index {
        HORZRES => Some(viewport.width),
        VERTRES => Some(viewport.height),
        _ => None,
    }
}

/// Whether an `EnumDisplaySettings` mode index is one we rewrite to the viewport size.
pub fn lies_about_mode(mode_index: u32) -> bool {
    matches!(mode_index, ENUM_CURRENT_SETTINGS | ENUM_REGISTRY_SETTINGS)
}

/// Intersect a cursor clip rectangle `(left, top, right, bottom)` with the viewport. An empty
/// intersection collapses to the viewport itself.
pub fn clamp_rect(rect: (i32, i32, i32, i32), viewport: PixelRect) -> (i32, i32, i32, i32) {
    let (l, t, r, b) = rect;
    let left = l.max(viewport.x);
    let top = t.max(viewport.y);
    let right = r.min(viewport.right());
    let bottom = b.min(viewport.bottom());
    if right <= left || bottom <= top {
        (viewport.x, viewport.y, viewport.right(), viewport.bottom())
    } else {
        (left, top, right, bottom)
    }
}

/// Pick the main window among this process's top-level windows: `(hwnd, title, area)`
/// tuples in; the largest titled window wins, or the largest title-regex match if a regex
/// is set.
pub fn pick_main_window(
    candidates: &[(usize, String, i64)],
    title_regex: Option<&regex_lite::Regex>,
) -> Option<usize> {
    let mut best: Option<(i64, usize)> = None;
    for (h, title, area) in candidates {
        if let Some(re) = title_regex {
            if !re.is_match(title) {
                continue;
            }
        } else if title.is_empty() {
            continue;
        }
        if best.is_none_or(|(a, _)| *area > a) {
            best = Some((*area, *h));
        }
    }
    best.map(|(_, h)| h)
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::cell::Cell;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use windows::Win32::Foundation::{HWND, LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{DEVMODEA, DEVMODEW, MONITORINFO};
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindow, GetWindowLongW, GetWindowRect, GetWindowTextW,
        GetWindowThreadProcessId, IsWindow, IsWindowVisible, SetWindowLongW, GWL_EXSTYLE,
        GWL_STYLE, GW_OWNER, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOZORDER,
    };

    use crate::detour_ptr;
    use crate::hooks::mh::{self, Orig};
    use crate::hooks::state;

    type CreateWindowExWFn = unsafe extern "system" fn(
        u32, *const u16, *const u16, u32, i32, i32, i32, i32,
        *mut c_void, *mut c_void, *mut c_void, *mut c_void,
    ) -> *mut c_void;
    type CreateWindowExAFn = unsafe extern "system" fn(
        u32, *const u8, *const u8, u32, i32, i32, i32, i32,
        *mut c_void, *mut c_void, *mut c_void, *mut c_void,
    ) -> *mut c_void;
    type SetWindowPosFn =
        unsafe extern "system" fn(*mut c_void, *mut c_void, i32, i32, i32, i32, u32) -> i32;
    type MoveWindowFn = unsafe extern "system" fn(*mut c_void, i32, i32, i32, i32, i32) -> i32;
    type GetSystemMetricsFn = unsafe extern "system" fn(i32) -> i32;
    type GetDeviceCapsFn = unsafe extern "system" fn(*mut c_void, i32) -> i32;
    type GetMonitorInfoFn = unsafe extern "system" fn(*mut c_void, *mut MONITORINFO) -> i32;
    type MonitorEnumProc =
        unsafe extern "system" fn(*mut c_void, *mut c_void, *mut RECT, LPARAM) -> i32;
    type EnumDisplayMonitorsFn =
        unsafe extern "system" fn(*mut c_void, *const RECT, MonitorEnumProc, LPARAM) -> i32;
    type EnumDisplaySettingsWFn = unsafe extern "system" fn(*const u16, u32, *mut DEVMODEW) -> i32;
    type EnumDisplaySettingsExWFn =
        unsafe extern "system" fn(*const u16, u32, *mut DEVMODEW, u32) -> i32;
    type EnumDisplaySettingsAFn = unsafe extern "system" fn(*const u8, u32, *mut DEVMODEA) -> i32;
    type EnumDisplaySettingsExAFn =
        unsafe extern "system" fn(*const u8, u32, *mut DEVMODEA, u32) -> i32;
    type ChangeDisplaySettingsFn = unsafe extern "system" fn(*const c_void, u32) -> i32;
    type ChangeDisplaySettingsExFn =
        unsafe extern "system" fn(*const c_void, *const c_void, *mut c_void, u32, *const c_void) -> i32;
    type ClipCursorFn = unsafe extern "system" fn(*const RECT) -> i32;
    type SystemParametersInfoFn = unsafe extern "system" fn(u32, u32, *mut c_void, u32) -> i32;

    static CREATE_WINDOW_EX_W: Orig<CreateWindowExWFn> = Orig::new();
    static CREATE_WINDOW_EX_A: Orig<CreateWindowExAFn> = Orig::new();
    static SET_WINDOW_POS: Orig<SetWindowPosFn> = Orig::new();
    static MOVE_WINDOW: Orig<MoveWindowFn> = Orig::new();
    static GET_SYSTEM_METRICS: Orig<GetSystemMetricsFn> = Orig::new();
    static GET_DEVICE_CAPS: Orig<GetDeviceCapsFn> = Orig::new();
    static GET_MONITOR_INFO_W: Orig<GetMonitorInfoFn> = Orig::new();
    static GET_MONITOR_INFO_A: Orig<GetMonitorInfoFn> = Orig::new();
    static ENUM_DISPLAY_MONITORS: Orig<EnumDisplayMonitorsFn> = Orig::new();
    static ENUM_DISPLAY_SETTINGS_W: Orig<EnumDisplaySettingsWFn> = Orig::new();
    static ENUM_DISPLAY_SETTINGS_EX_W: Orig<EnumDisplaySettingsExWFn> = Orig::new();
    static ENUM_DISPLAY_SETTINGS_A: Orig<EnumDisplaySettingsAFn> = Orig::new();
    static ENUM_DISPLAY_SETTINGS_EX_A: Orig<EnumDisplaySettingsExAFn> = Orig::new();
    static CHANGE_DISPLAY_SETTINGS_W: Orig<ChangeDisplaySettingsFn> = Orig::new();
    static CHANGE_DISPLAY_SETTINGS_A: Orig<ChangeDisplaySettingsFn> = Orig::new();
    static CHANGE_DISPLAY_SETTINGS_EX_W: Orig<ChangeDisplaySettingsExFn> = Orig::new();
    static CHANGE_DISPLAY_SETTINGS_EX_A: Orig<ChangeDisplaySettingsExFn> = Orig::new();
    static CLIP_CURSOR: Orig<ClipCursorFn> = Orig::new();
    static SYSTEM_PARAMETERS_INFO_W: Orig<SystemParametersInfoFn> = Orig::new();
    static SYSTEM_PARAMETERS_INFO_A: Orig<SystemParametersInfoFn> = Orig::new();

    /// Window we last stripped and pinned ourselves.
    static PINNED: AtomicUsize = AtomicUsize::new(0);

    const SPI_GETWORKAREA: u32 = 0x0030;
    const DISP_CHANGE_SUCCESSFUL: i32 = 0;
    const MONITORINFOF_PRIMARY: u32 = 1;

    fn lie() -> Option<PixelRect> {
        state::has(state::F_LIE_SCREEN_SIZE).then(state::viewport)
    }

    fn pin() -> Option<PixelRect> {
        state::has(state::F_PIN_WINDOW).then(state::viewport)
    }

    fn viewport_rect(v: PixelRect) -> RECT {
        RECT {
            left: v.x,
            top: v.y,
            right: v.right(),
            bottom: v.bottom(),
        }
    }

    fn is_main(h: *mut c_void) -> bool {
        let main = state::main_window();
        main != 0 && h as usize == main
    }

    unsafe extern "system" fn create_window_ex_w(
        ex: u32, class: *const u16, title: *const u16, style: u32,
        x: i32, y: i32, w: i32, h: i32,
        parent: *mut c_void, menu: *mut c_void, inst: *mut c_void, param: *mut c_void,
    ) -> *mut c_void {
        let Some(orig) = CREATE_WINDOW_EX_W.get() else {
            return std::ptr::null_mut();
        };
        if let Some(v) = pin() {
            if let Some((style, ex, x, y, w, h)) = plan_create(style, ex, !parent.is_null(), w, h, v) {
                return orig(ex, class, title, style, x, y, w, h, parent, menu, inst, param);
            }
        }
        orig(ex, class, title, style, x, y, w, h, parent, menu, inst, param)
    }

    unsafe extern "system" fn create_window_ex_a(
        ex: u32, class: *const u8, title: *const u8, style: u32,
        x: i32, y: i32, w: i32, h: i32,
        parent: *mut c_void, menu: *mut c_void, inst: *mut c_void, param: *mut c_void,
    ) -> *mut c_void {
        let Some(orig) = CREATE_WINDOW_EX_A.get() else {
            return std::ptr::null_mut();
        };
        if let Some(v) = pin() {
            if let Some((style, ex, x, y, w, h)) = plan_create(style, ex, !parent.is_null(), w, h, v) {
                return orig(ex, class, title, style, x, y, w, h, parent, menu, inst, param);
            }
        }
        orig(ex, class, title, style, x, y, w, h, parent, menu, inst, param)
    }

    unsafe extern "system" fn set_window_pos(
        hwnd: *mut c_void, after: *mut c_void, x: i32, y: i32, cx: i32, cy: i32, flags: u32,
    ) -> i32 {
        let Some(orig) = SET_WINDOW_POS.get() else {
            return 0;
        };
        if let Some(v) = pin() {
            if is_main(hwnd) {
                let (x, y, cx, cy, flags) = plan_set_window_pos(flags, v);
                return orig(hwnd, after, x, y, cx, cy, flags);
            }
        }
        orig(hwnd, after, x, y, cx, cy, flags)
    }

    unsafe extern "system" fn move_window(
        hwnd: *mut c_void, x: i32, y: i32, w: i32, h: i32, repaint: i32,
    ) -> i32 {
        let Some(orig) = MOVE_WINDOW.get() else {
            return 0;
        };
        if let Some(v) = pin() {
            if is_main(hwnd) {
                return orig(hwnd, v.x, v.y, v.width, v.height, repaint);
            }
        }
        orig(hwnd, x, y, w, h, repaint)
    }

    unsafe extern "system" fn get_system_metrics(index: i32) -> i32 {
        let Some(orig) = GET_SYSTEM_METRICS.get() else {
            return 0;
        };
        if let Some(v) = lie() {
            if let Some(value) = metric_override(index, v) {
                return value;
            }
        }
        orig(index)
    }

    unsafe extern "system" fn get_device_caps(hdc: *mut c_void, index: i32) -> i32 {
        let Some(orig) = GET_DEVICE_CAPS.get() else {
            return 0;
        };
        if let Some(v) = lie() {
            if let Some(value) = device_cap_override(index, v) {
                return value;
            }
        }
        orig(hdc, index)
    }

    unsafe fn lie_monitor_info(info: *mut MONITORINFO) {
        if info.is_null() {
            return;
        }
        if let Some(v) = lie() {
            (*info).rcMonitor = viewport_rect(v);
            (*info).rcWork = viewport_rect(v);
            (*info).dwFlags |= MONITORINFOF_PRIMARY;
        }
    }

    unsafe extern "system" fn get_monitor_info_w(hmon: *mut c_void, info: *mut MONITORINFO) -> i32 {
        let Some(orig) = GET_MONITOR_INFO_W.get() else {
            return 0;
        };
        let ok = orig(hmon, info);
        if ok != 0 {
            lie_monitor_info(info);
        }
        ok
    }

    unsafe extern "system" fn get_monitor_info_a(hmon: *mut c_void, info: *mut MONITORINFO) -> i32 {
        let Some(orig) = GET_MONITOR_INFO_A.get() else {
            return 0;
        };
        let ok = orig(hmon, info);
        if ok != 0 {
            // MONITORINFOEXA shares the MONITORINFO prefix.
            lie_monitor_info(info);
        }
        ok
    }

    thread_local! {
        static ENUM_CTX: Cell<(usize, isize)> = const { Cell::new((0, 0)) };
    }

    /// Forwards only the first monitor, with the viewport as its rect.
    unsafe extern "system" fn monitor_enum_shim(
        hmon: *mut c_void, hdc: *mut c_void, _rect: *mut RECT, _lparam: LPARAM,
    ) -> i32 {
        let (cb, data) = ENUM_CTX.with(|c| c.get());
        if cb == 0 {
            return 0;
        }
        let cb: MonitorEnumProc = std::mem::transmute(cb);
        let mut rect = viewport_rect(state::viewport());
        let _ = cb(hmon, hdc, &mut rect, LPARAM(data));
        0
    }

    unsafe extern "system" fn enum_display_monitors(
        hdc: *mut c_void, clip: *const RECT, cb: MonitorEnumProc, data: LPARAM,
    ) -> i32 {
        let Some(orig) = ENUM_DISPLAY_MONITORS.get() else {
            return 0;
        };
        if lie().is_none() {
            return orig(hdc, clip, cb, data);
        }
        let prev = ENUM_CTX.with(|c| c.replace((cb as usize, data.0)));
        orig(hdc, clip, monitor_enum_shim, LPARAM(0));
        ENUM_CTX.with(|c| c.set(prev));
        1
    }

    unsafe fn lie_devmode_w(mode: u32, dm: *mut DEVMODEW) {
        if dm.is_null() || !lies_about_mode(mode) {
            return;
        }
        if let Some(v) = lie() {
            (*dm).dmPelsWidth = v.width as u32;
            (*dm).dmPelsHeight = v.height as u32;
        }
    }

    unsafe fn lie_devmode_a(mode: u32, dm: *mut DEVMODEA) {
        if dm.is_null() || !lies_about_mode(mode) {
            return;
        }
        if let Some(v) = lie() {
            (*dm).dmPelsWidth = v.width as u32;
            (*dm).dmPelsHeight = v.height as u32;
        }
    }

    unsafe extern "system" fn enum_display_settings_w(dev: *const u16, mode: u32, dm: *mut DEVMODEW) -> i32 {
        let Some(orig) = ENUM_DISPLAY_SETTINGS_W.get() else {
            return 0;
        };
        let ok = orig(dev, mode, dm);
        if ok != 0 {
            lie_devmode_w(mode, dm);
        }
        ok
    }

    unsafe extern "system" fn enum_display_settings_ex_w(
        dev: *const u16, mode: u32, dm: *mut DEVMODEW, flags: u32,
    ) -> i32 {
        let Some(orig) = ENUM_DISPLAY_SETTINGS_EX_W.get() else {
            return 0;
        };
        let ok = orig(dev, mode, dm, flags);
        if ok != 0 {
            lie_devmode_w(mode, dm);
        }
        ok
    }

    unsafe extern "system" fn enum_display_settings_a(dev: *const u8, mode: u32, dm: *mut DEVMODEA) -> i32 {
        let Some(orig) = ENUM_DISPLAY_SETTINGS_A.get() else {
            return 0;
        };
        let ok = orig(dev, mode, dm);
        if ok != 0 {
            lie_devmode_a(mode, dm);
        }
        ok
    }

    unsafe extern "system" fn enum_display_settings_ex_a(
        dev: *const u8, mode: u32, dm: *mut DEVMODEA, flags: u32,
    ) -> i32 {
        let Some(orig) = ENUM_DISPLAY_SETTINGS_EX_A.get() else {
            return 0;
        };
        let ok = orig(dev, mode, dm, flags);
        if ok != 0 {
            lie_devmode_a(mode, dm);
        }
        ok
    }

    unsafe extern "system" fn change_display_settings_w(dm: *const c_void, flags: u32) -> i32 {
        let Some(orig) = CHANGE_DISPLAY_SETTINGS_W.get() else {
            return DISP_CHANGE_SUCCESSFUL;
        };
        if state::has(state::F_FORCE_WINDOWED) {
            return DISP_CHANGE_SUCCESSFUL;
        }
        orig(dm, flags)
    }

    unsafe extern "system" fn change_display_settings_a(dm: *const c_void, flags: u32) -> i32 {
        let Some(orig) = CHANGE_DISPLAY_SETTINGS_A.get() else {
            return DISP_CHANGE_SUCCESSFUL;
        };
        if state::has(state::F_FORCE_WINDOWED) {
            return DISP_CHANGE_SUCCESSFUL;
        }
        orig(dm, flags)
    }

    unsafe extern "system" fn change_display_settings_ex_w(
        dev: *const c_void, dm: *const c_void, hwnd: *mut c_void, flags: u32, lparam: *const c_void,
    ) -> i32 {
        let Some(orig) = CHANGE_DISPLAY_SETTINGS_EX_W.get() else {
            return DISP_CHANGE_SUCCESSFUL;
        };
        if state::has(state::F_FORCE_WINDOWED) {
            return DISP_CHANGE_SUCCESSFUL;
        }
        orig(dev, dm, hwnd, flags, lparam)
    }

    unsafe extern "system" fn change_display_settings_ex_a(
        dev: *const c_void, dm: *const c_void, hwnd: *mut c_void, flags: u32, lparam: *const c_void,
    ) -> i32 {
        let Some(orig) = CHANGE_DISPLAY_SETTINGS_EX_A.get() else {
            return DISP_CHANGE_SUCCESSFUL;
        };
        if state::has(state::F_FORCE_WINDOWED) {
            return DISP_CHANGE_SUCCESSFUL;
        }
        orig(dev, dm, hwnd, flags, lparam)
    }

    unsafe extern "system" fn clip_cursor(rect: *const RECT) -> i32 {
        let Some(orig) = CLIP_CURSOR.get() else {
            return 0;
        };
        if !rect.is_null() && state::has(state::F_CLIP_CURSOR) {
            let r = *rect;
            let (l, t, rr, b) = clamp_rect((r.left, r.top, r.right, r.bottom), state::viewport());
            let clamped = RECT {
                left: l,
                top: t,
                right: rr,
                bottom: b,
            };
            return orig(&clamped);
        }
        orig(rect)
    }

    unsafe fn lie_work_area(action: u32, pv: *mut c_void) {
        if action != SPI_GETWORKAREA || pv.is_null() {
            return;
        }
        if let Some(v) = lie() {
            *(pv as *mut RECT) = viewport_rect(v);
        }
    }

    unsafe extern "system" fn system_parameters_info_w(action: u32, param: u32, pv: *mut c_void, winini: u32) -> i32 {
        let Some(orig) = SYSTEM_PARAMETERS_INFO_W.get() else {
            return 0;
        };
        let ok = orig(action, param, pv, winini);
        if ok != 0 {
            lie_work_area(action, pv);
        }
        ok
    }

    unsafe extern "system" fn system_parameters_info_a(action: u32, param: u32, pv: *mut c_void, winini: u32) -> i32 {
        let Some(orig) = SYSTEM_PARAMETERS_INFO_A.get() else {
            return 0;
        };
        let ok = orig(action, param, pv, winini);
        if ok != 0 {
            lie_work_area(action, pv);
        }
        ok
    }

    pub fn install() -> Result<(), Vec<String>> {
        let results = unsafe {
            [
                mh::hook_export("user32.dll", "CreateWindowExW", detour_ptr!(create_window_ex_w), &CREATE_WINDOW_EX_W),
                mh::hook_export("user32.dll", "CreateWindowExA", detour_ptr!(create_window_ex_a), &CREATE_WINDOW_EX_A),
                mh::hook_export("user32.dll", "SetWindowPos", detour_ptr!(set_window_pos), &SET_WINDOW_POS),
                mh::hook_export("user32.dll", "MoveWindow", detour_ptr!(move_window), &MOVE_WINDOW),
                mh::hook_export("user32.dll", "GetSystemMetrics", detour_ptr!(get_system_metrics), &GET_SYSTEM_METRICS),
                mh::hook_export("gdi32.dll", "GetDeviceCaps", detour_ptr!(get_device_caps), &GET_DEVICE_CAPS),
                mh::hook_export("user32.dll", "GetMonitorInfoW", detour_ptr!(get_monitor_info_w), &GET_MONITOR_INFO_W),
                mh::hook_export("user32.dll", "GetMonitorInfoA", detour_ptr!(get_monitor_info_a), &GET_MONITOR_INFO_A),
                mh::hook_export("user32.dll", "EnumDisplayMonitors", detour_ptr!(enum_display_monitors), &ENUM_DISPLAY_MONITORS),
                mh::hook_export("user32.dll", "EnumDisplaySettingsW", detour_ptr!(enum_display_settings_w), &ENUM_DISPLAY_SETTINGS_W),
                mh::hook_export("user32.dll", "EnumDisplaySettingsExW", detour_ptr!(enum_display_settings_ex_w), &ENUM_DISPLAY_SETTINGS_EX_W),
                mh::hook_export("user32.dll", "EnumDisplaySettingsA", detour_ptr!(enum_display_settings_a), &ENUM_DISPLAY_SETTINGS_A),
                mh::hook_export("user32.dll", "EnumDisplaySettingsExA", detour_ptr!(enum_display_settings_ex_a), &ENUM_DISPLAY_SETTINGS_EX_A),
                mh::hook_export("user32.dll", "ChangeDisplaySettingsW", detour_ptr!(change_display_settings_w), &CHANGE_DISPLAY_SETTINGS_W),
                mh::hook_export("user32.dll", "ChangeDisplaySettingsA", detour_ptr!(change_display_settings_a), &CHANGE_DISPLAY_SETTINGS_A),
                mh::hook_export("user32.dll", "ChangeDisplaySettingsExW", detour_ptr!(change_display_settings_ex_w), &CHANGE_DISPLAY_SETTINGS_EX_W),
                mh::hook_export("user32.dll", "ChangeDisplaySettingsExA", detour_ptr!(change_display_settings_ex_a), &CHANGE_DISPLAY_SETTINGS_EX_A),
                mh::hook_export("user32.dll", "ClipCursor", detour_ptr!(clip_cursor), &CLIP_CURSOR),
                mh::hook_export("user32.dll", "SystemParametersInfoW", detour_ptr!(system_parameters_info_w), &SYSTEM_PARAMETERS_INFO_W),
                mh::hook_export("user32.dll", "SystemParametersInfoA", detour_ptr!(system_parameters_info_a), &SYSTEM_PARAMETERS_INFO_A),
            ]
        };
        let errors: Vec<String> = results.into_iter().filter_map(Result::err).collect();
        if errors.len() == 20 {
            Err(errors)
        } else {
            Ok(())
        }
    }

    unsafe extern "system" fn enum_proc(h: HWND, lparam: LPARAM) -> windows::core::BOOL {
        let list = &mut *(lparam.0 as *mut Vec<(usize, String, i64)>);
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        if pid != GetCurrentProcessId() || !IsWindowVisible(h).as_bool() {
            return true.into();
        }
        if GetWindow(h, GW_OWNER).is_ok_and(|o| !o.0.is_null()) {
            return true.into();
        }
        let ex = GetWindowLongW(h, GWL_EXSTYLE) as u32;
        if ex & WS_EX_TOOLWINDOW != 0 {
            return true.into();
        }
        let mut rect = RECT::default();
        let _ = GetWindowRect(h, &mut rect);
        let (w, hgt) = (rect.right - rect.left, rect.bottom - rect.top);
        if w < 32 || hgt < 32 {
            return true.into();
        }
        let mut buf = [0u16; 512];
        let n = GetWindowTextW(h, &mut buf).max(0) as usize;
        list.push((
            h.0 as usize,
            String::from_utf16_lossy(&buf[..n]),
            w as i64 * hgt as i64,
        ));
        true.into()
    }

    /// Re-validate or (re)discover the main window. Returns `Some(hwnd)` only when a new
    /// window was found this call.
    pub fn refresh_main_window() -> Option<usize> {
        let current = state::main_window();
        if current != 0 {
            if unsafe { IsWindow(Some(HWND(current as *mut c_void))).as_bool() } {
                return None;
            }
            state::set_main_window(0);
        }
        let mut list: Vec<(usize, String, i64)> = Vec::new();
        unsafe {
            let _ = EnumWindows(Some(enum_proc), LPARAM(&mut list as *mut _ as isize));
        }
        let picked = state::with(|s| pick_main_window(&list, s.title_regex.as_ref())).flatten()?;
        state::set_main_window(picked);
        Some(picked)
    }

    /// Strip the frame and place the window at the viewport, through the original
    /// `SetWindowPos` so the detour does not need to run for our own call.
    fn pin_now(h: usize) {
        let Some(v) = pin() else { return };
        let hwnd = HWND(h as *mut c_void);
        unsafe {
            let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
            SetWindowLongW(hwnd, GWL_STYLE, (style & !FRAME_STYLES) as i32);
            let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            SetWindowLongW(hwnd, GWL_EXSTYLE, (ex & !FRAME_EX_STYLES) as i32);
            if let Some(orig) = SET_WINDOW_POS.get() {
                let flags = (SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOZORDER).0;
                orig(h as *mut c_void, std::ptr::null_mut(), v.x, v.y, v.width, v.height, flags);
            }
        }
        PINNED.store(h, Ordering::SeqCst);
    }

    pub fn on_main_window(h: usize) {
        pin_now(h);
    }

    /// The viewport changed: pin again.
    pub fn on_config_changed() {
        PINNED.store(0, Ordering::SeqCst);
    }

    /// Periodic: put the main window back if it drifted (the orchestrator does this too, but
    /// from inside the process it is immediate).
    pub fn repin() {
        let h = state::main_window();
        if h == 0 {
            return;
        }
        let Some(v) = pin() else { return };
        let mut rect = RECT::default();
        if unsafe { GetWindowRect(HWND(h as *mut c_void), &mut rect) }.is_err() {
            return;
        }
        let now = PixelRect::new(rect.left, rect.top, rect.right - rect.left, rect.bottom - rect.top);
        if now != v || PINNED.load(Ordering::SeqCst) != h {
            pin_now(h);
        }
    }
}

#[cfg(windows)]
pub use imp::{install, on_config_changed, on_main_window, refresh_main_window, repin};

#[cfg(test)]
mod tests {
    use super::*;

    const VP: PixelRect = PixelRect::new(1920, 0, 1920, 1080);

    #[test]
    fn create_override_applies_to_big_top_level_windows_only() {
        let overlapped = 0x00CF_0000u32; // WS_OVERLAPPEDWINDOW
        let got = plan_create(overlapped, 0, false, 1280, 720, VP).unwrap();
        assert_eq!(got, (overlapped & !FRAME_STYLES, 0, 1920, 0, 1920, 1080));
        assert_eq!(got.0 & WS_CAPTION, 0);
        assert!(plan_create(overlapped, 0, false, CW_USEDEFAULT, CW_USEDEFAULT, VP).is_some());
        assert!(plan_create(overlapped, 0, false, 100, 100, VP).is_none(), "tiny");
        assert!(plan_create(overlapped | WS_CHILD, 0, false, 1280, 720, VP).is_none());
        assert!(plan_create(overlapped, 0, true, 1280, 720, VP).is_none(), "owned");
        assert!(plan_create(overlapped, WS_EX_TOOLWINDOW, false, 1280, 720, VP).is_none());
        assert!(plan_create(overlapped, 0, false, 1280, 720, PixelRect::default()).is_none());
    }

    #[test]
    fn set_window_pos_forces_the_viewport() {
        let (x, y, w, h, flags) = plan_set_window_pos(SWP_NOMOVE | SWP_NOSIZE | 0x40, VP);
        assert_eq!((x, y, w, h), (1920, 0, 1920, 1080));
        assert_eq!(flags, 0x40);
    }

    #[test]
    fn metrics_and_caps_report_the_viewport() {
        assert_eq!(metric_override(SM_CXSCREEN, VP), Some(1920));
        assert_eq!(metric_override(SM_CYSCREEN, VP), Some(1080));
        assert_eq!(metric_override(SM_CYVIRTUALSCREEN, VP), Some(1080));
        assert_eq!(metric_override(SM_CMONITORS, VP), Some(1));
        assert_eq!(metric_override(42, VP), None);
        assert_eq!(device_cap_override(HORZRES, VP), Some(1920));
        assert_eq!(device_cap_override(VERTRES, VP), Some(1080));
        assert_eq!(device_cap_override(12, VP), None);
        assert!(lies_about_mode(ENUM_CURRENT_SETTINGS));
        assert!(lies_about_mode(ENUM_REGISTRY_SETTINGS));
        assert!(!lies_about_mode(0));
    }

    #[test]
    fn clamp_rect_intersects_or_falls_back_to_viewport() {
        assert_eq!(clamp_rect((0, 0, 3840, 2160), VP), (1920, 0, 3840, 1080));
        assert_eq!(clamp_rect((2000, 100, 2500, 500), VP), (2000, 100, 2500, 500));
        assert_eq!(clamp_rect((0, 0, 100, 100), VP), (1920, 0, 3840, 1080));
    }

    #[test]
    fn main_window_pick_prefers_regex_then_area() {
        let list = vec![
            (1, "Launcher".to_string(), 800 * 600),
            (2, "Game Window".to_string(), 1920 * 1080),
            (3, "".to_string(), 3840 * 2160),
        ];
        assert_eq!(pick_main_window(&list, None), Some(2), "untitled ignored");
        let re = regex_lite::Regex::new("^Launcher").unwrap();
        assert_eq!(pick_main_window(&list, Some(&re)), Some(1));
        let none = regex_lite::Regex::new("^Nope").unwrap();
        assert_eq!(pick_main_window(&list, Some(&none)), None);
    }
}
