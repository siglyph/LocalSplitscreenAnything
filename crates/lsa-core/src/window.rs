//! Window and process management: monitors, process trees, finding a game's main window,
//! stripping decorations, pinning windows to viewports, and black backdrop windows.
//!
//! The real implementation is Windows-only; other platforms get stubs so the layout engine,
//! profiles and dry runs can be developed and tested anywhere.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use lsa_proto::PixelRect;
use regex::Regex;

use crate::layout::Monitor;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: u32,
    pub parent: u32,
    pub exe: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    pub hwnd: u64,
    pub pid: u32,
    pub title: String,
    pub class: String,
    pub rect: PixelRect,
}

impl WindowInfo {
    pub fn area(&self) -> i64 {
        self.rect.width as i64 * self.rect.height as i64
    }
}

/// How to recognise an instance's main window.
#[derive(Debug, Clone, Default)]
pub struct WindowQuery {
    /// Root process; its whole process tree is searched.
    pub root_pid: Option<u32>,
    /// Executable file name (case-insensitive) whose processes are searched too, for games
    /// started by a launcher we did not spawn (Steam, Prism hand-off).
    pub exe_name: Option<String>,
    pub title_regex: Option<Regex>,
    pub class: Option<String>,
    /// Windows already claimed by other instances.
    pub exclude: HashSet<u64>,
    /// Processes that existed before the launch (ignored when `exe_name` is used).
    pub preexisting_pids: HashSet<u32>,
}

/// Choose the best candidate: a title/class match wins, then the largest window. When a
/// title regex is given it is mandatory: a launcher's own windows (Prism's "Please wait...",
/// Steam's "Preparing to launch...") live in the same process tree and must never be taken for
/// the game.
pub fn pick_main_window(candidates: &[WindowInfo], query: &WindowQuery) -> Option<WindowInfo> {
    let mut best: Option<(i32, i64, &WindowInfo)> = None;
    for w in candidates {
        if query.exclude.contains(&w.hwnd) || w.rect.width < 32 || w.rect.height < 32 {
            continue;
        }
        let mut score = 0;
        if let Some(re) = &query.title_regex {
            if !re.is_match(&w.title) {
                continue;
            }
            score += 2;
        }
        if let Some(class) = &query.class {
            if w.class.eq_ignore_ascii_case(class) {
                score += 1;
            }
        }
        if w.title.is_empty() && score == 0 {
            continue;
        }
        let better = match best {
            None => true,
            Some((s, a, _)) => score > s || (score == s && w.area() > a),
        };
        if better {
            best = Some((score, w.area(), w));
        }
    }
    best.map(|(_, _, w)| w.clone())
}

/// Poll until a main window matching `query` appears or `timeout` passes.
///
/// Candidates come from the root's process tree and/or processes named `exe_name`. If that
/// yields no processes at all (no root, or the root was a launcher that exited after handing
/// the launch to an already-running copy of itself, as Prism and Steam do) and a title regex
/// is known, every window on the desktop is considered instead; the regex then decides.
pub fn wait_for_main_window(query: &WindowQuery, timeout: Duration) -> Option<WindowInfo> {
    let start = Instant::now();
    loop {
        let mut pids: HashSet<u32> = HashSet::new();
        if let Some(root) = query.root_pid {
            pids.extend(process_tree(root).into_iter().map(|p| p.pid));
        }
        if let Some(exe) = &query.exe_name {
            for p in list_processes() {
                if p.exe.eq_ignore_ascii_case(exe) && !query.preexisting_pids.contains(&p.pid) {
                    pids.insert(p.pid);
                }
            }
        }
        let title_only = pids.is_empty()
            && query.exe_name.is_none()
            && (query.root_pid.is_none() || query.title_regex.is_some());
        let candidates: Vec<WindowInfo> = if title_only {
            list_windows()
        } else {
                list_windows()
                    .into_iter()
                    .filter(|w| pids.contains(&w.pid))
                    .collect()
            };
        if let Some(w) = pick_main_window(&candidates, query) {
            return Some(w);
        }
        if start.elapsed() >= timeout {
            return None;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// All descendants of `root` (including `root`), from a process snapshot.
pub fn process_tree(root: u32) -> Vec<ProcessInfo> {
    let all = list_processes();
    let mut out = Vec::new();
    let mut frontier = vec![root];
    let mut seen: HashSet<u32> = HashSet::new();
    while let Some(pid) = frontier.pop() {
        if !seen.insert(pid) {
            continue;
        }
        if let Some(p) = all.iter().find(|p| p.pid == pid) {
            out.push(p.clone());
        }
        for child in all.iter().filter(|p| p.parent == pid && p.pid != pid) {
            frontier.push(child.pid);
        }
    }
    out
}

pub use imp::{
    ensure_dpi_aware, enumerate_monitors, is_window, list_processes, list_windows, place_window,
    strip_decorations, terminate_process, window_rect, Backdrop,
};

#[cfg(windows)]
mod imp {
    use super::*;
    use std::ffi::c_void;
    use std::sync::Once;

    use windows::core::{w, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, LRESULT, RECT, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, GetStockObject, BLACK_BRUSH, HBRUSH, HDC, HMONITOR,
        MONITORINFO, MONITORINFOEXW,
    };

    /// `MONITORINFO::dwFlags` bit for the primary display (from the Windows SDK).
    const MONITORINFOF_PRIMARY: u32 = 1;
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::{
        GetCurrentThreadId, OpenProcess, TerminateProcess, PROCESS_TERMINATE,
    };
    use windows::Win32::UI::HiDpi::{
        GetDpiForMonitor, SetProcessDpiAwarenessContext,
        DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumWindows,
        GetClassNameW, GetMessageW, GetWindowLongPtrW, GetWindowRect, GetWindowTextW,
        GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, PostThreadMessageW,
        RegisterClassW, SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage, CS_HREDRAW,
        CS_VREDRAW, GWL_EXSTYLE, GWL_STYLE, HWND_BOTTOM, HWND_NOTOPMOST, HWND_TOP, HWND_TOPMOST,
        MSG, SET_WINDOW_POS_FLAGS, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
        SWP_NOZORDER, SWP_SHOWWINDOW, SW_RESTORE, SW_SHOWNOACTIVATE, WINDOW_EX_STYLE, WINDOW_STYLE,
        WM_QUIT, WNDCLASSW, WS_BORDER, WS_CAPTION, WS_DLGFRAME, WS_EX_CLIENTEDGE,
        WS_EX_DLGMODALFRAME, WS_EX_NOACTIVATE, WS_EX_STATICEDGE, WS_EX_TOOLWINDOW,
        WS_EX_WINDOWEDGE, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU, WS_THICKFRAME,
    };

    fn hwnd(h: u64) -> HWND {
        HWND(h as usize as *mut c_void)
    }

    fn hwnd_id(h: HWND) -> u64 {
        h.0 as usize as u64
    }

    fn rect_to_pixels(r: &RECT) -> PixelRect {
        PixelRect::new(r.left, r.top, r.right - r.left, r.bottom - r.top)
    }

    fn wide_to_string(buf: &[u16]) -> String {
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end])
    }

    /// Opt into per-monitor DPI awareness so every coordinate we see is a physical pixel.
    pub fn ensure_dpi_aware() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        });
    }

    unsafe extern "system" fn monitor_enum(
        hmon: HMONITOR,
        _hdc: HDC,
        _rect: *mut RECT,
        lparam: LPARAM,
    ) -> windows::core::BOOL {
        let list = &mut *(lparam.0 as *mut Vec<HMONITOR>);
        list.push(hmon);
        true.into()
    }

    /// Connected monitors, primary first, then left to right.
    pub fn enumerate_monitors() -> Vec<Monitor> {
        ensure_dpi_aware();
        let mut handles: Vec<HMONITOR> = Vec::new();
        unsafe {
            let _ = EnumDisplayMonitors(
                None,
                None,
                Some(monitor_enum),
                LPARAM(&mut handles as *mut _ as isize),
            );
        }
        let mut monitors = Vec::new();
        for h in handles {
            let mut info = MONITORINFOEXW::default();
            info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
            let ok =
                unsafe { GetMonitorInfoW(h, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) };
            if !ok.as_bool() {
                continue;
            }
            let mut dpi_x = 96u32;
            let mut dpi_y = 96u32;
            unsafe {
                let _ = GetDpiForMonitor(h, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
            }
            let id = wide_to_string(&info.szDevice);
            let primary = (info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY) != 0;
            monitors.push(Monitor {
                name: id.trim_start_matches(r"\\.\").to_string(),
                id,
                rect: rect_to_pixels(&info.monitorInfo.rcMonitor),
                primary,
                scale: dpi_x as f64 / 96.0,
            });
        }
        monitors.sort_by(|a, b| {
            b.primary
                .cmp(&a.primary)
                .then(a.rect.x.cmp(&b.rect.x))
                .then(a.rect.y.cmp(&b.rect.y))
        });
        monitors
    }

    pub fn list_processes() -> Vec<ProcessInfo> {
        let mut out = Vec::new();
        unsafe {
            let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
                return out;
            };
            let mut entry = PROCESSENTRY32W {
                dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
                ..Default::default()
            };
            if Process32FirstW(snap, &mut entry).is_ok() {
                loop {
                    out.push(ProcessInfo {
                        pid: entry.th32ProcessID,
                        parent: entry.th32ParentProcessID,
                        exe: wide_to_string(&entry.szExeFile),
                    });
                    if Process32NextW(snap, &mut entry).is_err() {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snap);
        }
        out
    }

    unsafe extern "system" fn window_enum(h: HWND, lparam: LPARAM) -> windows::core::BOOL {
        let list = &mut *(lparam.0 as *mut Vec<WindowInfo>);
        if !IsWindowVisible(h).as_bool() {
            return true.into();
        }
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
        if ex & WS_EX_TOOLWINDOW.0 != 0 {
            return true.into();
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        let mut title = [0u16; 512];
        let n = GetWindowTextW(h, &mut title);
        let mut class = [0u16; 256];
        let c = GetClassNameW(h, &mut class);
        let mut rect = RECT::default();
        let _ = GetWindowRect(h, &mut rect);
        list.push(WindowInfo {
            hwnd: hwnd_id(h),
            pid,
            title: wide_to_string(&title[..n.max(0) as usize]),
            class: wide_to_string(&class[..c.max(0) as usize]),
            rect: rect_to_pixels(&rect),
        });
        true.into()
    }

    /// Every visible top-level window.
    pub fn list_windows() -> Vec<WindowInfo> {
        let mut out: Vec<WindowInfo> = Vec::new();
        unsafe {
            let _ = EnumWindows(Some(window_enum), LPARAM(&mut out as *mut _ as isize));
        }
        out
    }

    pub fn is_window(h: u64) -> bool {
        unsafe { IsWindow(Some(hwnd(h))).as_bool() }
    }

    pub fn window_rect(h: u64) -> Option<PixelRect> {
        let mut rect = RECT::default();
        unsafe { GetWindowRect(hwnd(h), &mut rect).ok()? };
        Some(rect_to_pixels(&rect))
    }

    /// Remove title bar, borders and resize frame.
    pub fn strip_decorations(h: u64) -> windows::core::Result<()> {
        let h = hwnd(h);
        unsafe {
            let style = GetWindowLongPtrW(h, GWL_STYLE) as u32;
            let remove: WINDOW_STYLE = WS_CAPTION
                | WS_THICKFRAME
                | WS_MINIMIZEBOX
                | WS_MAXIMIZEBOX
                | WS_SYSMENU
                | WS_BORDER
                | WS_DLGFRAME;
            SetWindowLongPtrW(h, GWL_STYLE, (style & !remove.0) as isize);
            let ex = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
            let remove_ex: WINDOW_EX_STYLE =
                WS_EX_DLGMODALFRAME | WS_EX_CLIENTEDGE | WS_EX_STATICEDGE | WS_EX_WINDOWEDGE;
            SetWindowLongPtrW(h, GWL_EXSTYLE, (ex & !remove_ex.0) as isize);
            SetWindowPos(
                h,
                None,
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            )
        }
    }

    /// Move/resize the window to `rect`, restoring it first if minimised.
    pub fn place_window(h: u64, rect: PixelRect, topmost: bool) -> windows::core::Result<()> {
        let h = hwnd(h);
        unsafe {
            if IsIconic(h).as_bool() {
                let _ = ShowWindow(h, SW_RESTORE);
            }
            let insert_after = if topmost {
                HWND_TOPMOST
            } else {
                HWND_NOTOPMOST
            };
            let flags: SET_WINDOW_POS_FLAGS = SWP_NOACTIVATE | SWP_SHOWWINDOW;
            SetWindowPos(
                h,
                Some(insert_after),
                rect.x,
                rect.y,
                rect.width,
                rect.height,
                flags,
            )?;
            if !topmost {
                // Bring it above the backdrop without making it topmost.
                SetWindowPos(
                    h,
                    Some(HWND_TOP),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                )?;
            }
        }
        Ok(())
    }

    pub fn terminate_process(pid: u32) -> windows::core::Result<()> {
        unsafe {
            let handle: HANDLE = OpenProcess(PROCESS_TERMINATE, false, pid)?;
            let result = TerminateProcess(handle, 1);
            let _ = CloseHandle(handle);
            result
        }
    }

    unsafe extern "system" fn backdrop_proc(
        h: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        DefWindowProcW(h, msg, wparam, lparam)
    }

    /// Black windows covering each monitor, kept at the bottom of the z-order so the desktop
    /// is hidden behind the game windows. Lives on its own thread with a message loop.
    pub struct Backdrop {
        thread_id: u32,
        handle: Option<std::thread::JoinHandle<()>>,
    }

    impl Backdrop {
        pub fn create(monitors: &[Monitor]) -> Option<Backdrop> {
            let rects: Vec<PixelRect> = monitors.iter().map(|m| m.rect).collect();
            let (tx, rx) = std::sync::mpsc::channel::<Option<u32>>();
            let handle = std::thread::spawn(move || unsafe {
                let instance = match GetModuleHandleW(None) {
                    Ok(m) => m.into(),
                    Err(_) => {
                        let _ = tx.send(None);
                        return;
                    }
                };
                let class_name: PCWSTR = w!("LSA_Backdrop");
                let wc = WNDCLASSW {
                    style: CS_HREDRAW | CS_VREDRAW,
                    lpfnWndProc: Some(backdrop_proc),
                    hInstance: instance,
                    hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
                    lpszClassName: class_name,
                    ..Default::default()
                };
                // Registering twice (second session in one process) just fails harmlessly.
                let _ = RegisterClassW(&wc);
                let mut windows_created: Vec<HWND> = Vec::new();
                for r in &rects {
                    let created = CreateWindowExW(
                        WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                        class_name,
                        w!("LSA backdrop"),
                        WS_POPUP,
                        r.x,
                        r.y,
                        r.width,
                        r.height,
                        None,
                        None,
                        Some(instance),
                        None,
                    );
                    if let Ok(h) = created {
                        let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
                        let _ = SetWindowPos(
                            h,
                            Some(HWND_BOTTOM),
                            0,
                            0,
                            0,
                            0,
                            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                        );
                        windows_created.push(h);
                    }
                }
                let _ = tx.send(Some(GetCurrentThreadId()));
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                for h in windows_created {
                    let _ = DestroyWindow(h);
                }
            });
            rx.recv().ok().flatten().map(|thread_id| Backdrop {
                thread_id,
                handle: Some(handle),
            })
        }
    }

    impl Drop for Backdrop {
        fn drop(&mut self) {
            unsafe {
                let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
            if let Some(h) = self.handle.take() {
                let _ = h.join();
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;

    pub fn ensure_dpi_aware() {}

    pub fn enumerate_monitors() -> Vec<Monitor> {
        Vec::new()
    }

    pub fn list_processes() -> Vec<ProcessInfo> {
        Vec::new()
    }

    pub fn list_windows() -> Vec<WindowInfo> {
        Vec::new()
    }

    pub fn is_window(_h: u64) -> bool {
        false
    }

    pub fn window_rect(_h: u64) -> Option<PixelRect> {
        None
    }

    pub fn strip_decorations(_h: u64) -> Result<(), String> {
        Err("window management is only available on Windows".into())
    }

    pub fn place_window(_h: u64, _rect: PixelRect, _topmost: bool) -> Result<(), String> {
        Err("window management is only available on Windows".into())
    }

    pub fn terminate_process(_pid: u32) -> Result<(), String> {
        Err("process control is only available on Windows".into())
    }

    pub struct Backdrop;

    impl Backdrop {
        pub fn create(_monitors: &[Monitor]) -> Option<Backdrop> {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(hwnd: u64, pid: u32, title: &str, w: i32, h: i32) -> WindowInfo {
        WindowInfo {
            hwnd,
            pid,
            title: title.into(),
            class: "C".into(),
            rect: PixelRect::new(0, 0, w, h),
        }
    }

    #[test]
    fn pick_prefers_title_match_then_area() {
        let candidates = vec![
            win(1, 10, "Launcher", 800, 600),
            win(2, 10, "Game Window", 1920, 1080),
            win(3, 10, "Game Window", 640, 480),
        ];
        let mut q = WindowQuery {
            root_pid: Some(10),
            ..Default::default()
        };
        assert_eq!(
            pick_main_window(&candidates, &q).unwrap().hwnd,
            2,
            "largest wins without a regex"
        );
        q.title_regex = Some(Regex::new("^Launcher").unwrap());
        assert_eq!(
            pick_main_window(&candidates, &q).unwrap().hwnd,
            1,
            "regex match beats size"
        );
        q.exclude.insert(1);
        assert!(
            pick_main_window(&candidates, &q).is_none(),
            "a regex is mandatory: no other window stands in for the excluded match"
        );
        q.title_regex = None;
        assert_eq!(
            pick_main_window(&candidates, &q).unwrap().hwnd,
            2,
            "excluded windows are skipped"
        );
    }

    #[test]
    fn launcher_windows_in_the_tree_are_not_taken_for_the_game() {
        // Prism shows "Please wait... - Prism Launcher" in the same process tree before the
        // game window exists; it must not be picked while the regex does not match.
        let candidates = vec![win(1, 10, "Please wait... - Prism Launcher 9.1", 400, 300)];
        let q = WindowQuery {
            root_pid: Some(10),
            title_regex: Some(Regex::new(r"^Minecraft\*? ").unwrap()),
            ..Default::default()
        };
        assert!(pick_main_window(&candidates, &q).is_none());
        let later = vec![
            win(1, 10, "Please wait... - Prism Launcher 9.1", 400, 300),
            win(2, 11, "Minecraft* 1.21.4", 1920, 2160),
        ];
        assert_eq!(pick_main_window(&later, &q).unwrap().hwnd, 2);
    }

    #[test]
    fn pick_requires_title_when_only_regex_is_known() {
        let candidates = vec![
            win(1, 10, "Minecraft* 1.21", 800, 600),
            win(2, 11, "Explorer", 1920, 1080),
        ];
        let q = WindowQuery {
            title_regex: Some(Regex::new("^Minecraft").unwrap()),
            ..Default::default()
        };
        assert_eq!(pick_main_window(&candidates, &q).unwrap().hwnd, 1);
        let none = WindowQuery {
            title_regex: Some(Regex::new("^Nope").unwrap()),
            ..Default::default()
        };
        assert!(pick_main_window(&candidates, &none).is_none());
    }

    #[test]
    fn tiny_and_untitled_windows_are_ignored() {
        let candidates = vec![win(1, 10, "", 1920, 1080), win(2, 10, "x", 10, 10)];
        let q = WindowQuery {
            root_pid: Some(10),
            ..Default::default()
        };
        assert!(pick_main_window(&candidates, &q).is_none());
    }
}
