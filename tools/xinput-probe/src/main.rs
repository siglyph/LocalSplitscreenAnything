//! A tiny window that reports, several times a second:
//!
//! - which XInput slots (0..=3) it sees connected, and the buttons held on each;
//! - whether it believes it is the foreground window (fake-focus check);
//! - the screen size it detects via `GetSystemMetrics` (screen-size-lie check).
//!
//! It is a stand-in for a real game, for testing the `lsa-hook` DLL without launching
//! anything you own. Run two or four copies through `lsa play` with one pad assigned to each:
//!
//! - Each window should see a pad only on **slot 0**, and only its own physical pad should
//!   move its reading, even while another window is focused.
//! - Each window should report `foreground: yes` even when it is not the active window.
//! - Each window should report a screen size equal to its viewport.
//!
//! Build it for both bitnesses to test both DLLs; see `Cargo.toml`. Nothing here is specific
//! to the hook: unhooked, every window sees the same pads on their real slots, only the truly
//! focused window reports `foreground: yes`, and the screen size is the real desktop.

#![allow(clippy::missing_safety_doc)]

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, DrawTextW, EndPaint, FillRect, GetStockObject, InvalidateRect, SetBkMode,
    SetTextColor, BLACK_BRUSH, DT_LEFT, DT_NOCLIP, DT_TOP, HBRUSH, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::XboxController::{XInputGetState, XINPUT_STATE};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetClientRect, GetForegroundWindow,
    GetMessageW, GetSystemMetrics, KillTimer, LoadCursorW, PostQuitMessage, RegisterClassW,
    SetTimer, SetWindowTextW, ShowWindow, TranslateMessage, CW_USEDEFAULT, IDC_ARROW, MSG,
    SM_CXSCREEN, SM_CYSCREEN, SW_SHOW, WM_DESTROY, WM_PAINT, WM_TIMER, WNDCLASSW,
    WS_OVERLAPPEDWINDOW,
};

const TIMER_ID: usize = 1;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Human-readable status string, repainted on each timer tick.
unsafe fn status_text(hwnd: HWND) -> String {
    let mut lines = Vec::new();
    let focused = GetForegroundWindow() == hwnd;
    lines.push(format!(
        "foreground (this window is focused?): {}",
        if focused { "yes" } else { "no" }
    ));
    let cx = GetSystemMetrics(SM_CXSCREEN);
    let cy = GetSystemMetrics(SM_CYSCREEN);
    lines.push(format!("detected screen size: {cx} x {cy}"));
    lines.push(String::new());
    lines.push("XInput slots:".into());
    for slot in 0..4u32 {
        let mut state = XINPUT_STATE::default();
        let connected = XInputGetState(slot, &mut state) == 0;
        if connected {
            let g = state.Gamepad;
            lines.push(format!(
                "  slot {slot}: CONNECTED  buttons={:#06x} LT={} RT={} LX={} LY={}",
                g.wButtons.0, g.bLeftTrigger, g.bRightTrigger, g.sThumbLX, g.sThumbLY
            ));
        } else {
            lines.push(format!("  slot {slot}: -"));
        }
    }
    lines.join("\r\n")
}

/// Short title that survives border stripping only as the taskbar text, but useful before the
/// window is pinned.
unsafe fn title_text(hwnd: HWND) -> String {
    let fg = GetForegroundWindow();
    let mut slot0 = "none";
    let mut state = XINPUT_STATE::default();
    if XInputGetState(0, &mut state) == 0 {
        slot0 = "slot0";
    }
    format!(
        "XInput Probe  focus={}  pad={slot0}  {}x{}",
        if fg == hwnd { "yes" } else { "no" },
        GetSystemMetrics(SM_CXSCREEN),
        GetSystemMetrics(SM_CYSCREEN),
    )
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER => {
            let _ = SetWindowTextW(hwnd, PCWSTR(wide(&title_text(hwnd)).as_ptr()));
            let _ = InvalidateRect(Some(hwnd), None, true);
            LRESULT(0)
        }
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            FillRect(hdc, &client, HBRUSH(GetStockObject(BLACK_BRUSH).0));
            SetBkMode(hdc, TRANSPARENT);
            SetTextColor(hdc, COLORREF(0x00_FF_FF_00));
            let mut text = wide(&status_text(hwnd));
            let mut rect = RECT {
                left: client.left + 16,
                top: client.top + 16,
                right: client.right - 16,
                bottom: client.bottom - 16,
            };
            DrawTextW(hdc, &mut text, &mut rect, DT_LEFT | DT_TOP | DT_NOCLIP);
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_DESTROY => {
            let _ = KillTimer(Some(hwnd), TIMER_ID);
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn main() {
    unsafe {
        let instance = GetModuleHandleW(None).expect("module handle");
        let class_name = w!("LSAXInputProbe");
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: class_name,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            Default::default(),
            class_name,
            w!("XInput Probe"),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            1280,
            720,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .expect("create window");
        let _ = ShowWindow(hwnd, SW_SHOW);
        SetTimer(Some(hwnd), TIMER_ID, 100, None);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
