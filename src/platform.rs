#[cfg(target_os = "windows")]
use std::thread;
#[cfg(target_os = "windows")]
use std::time::Duration;

use slint::{ComponentHandle, PhysicalPosition};

use crate::MainWindow;

#[cfg(target_os = "windows")]
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
#[cfg(target_os = "windows")]
use windows_sys::Win32::{
    Foundation::{HWND, POINT, RECT},
    Graphics::Gdi::{
        ClientToScreen, GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    },
    System::Threading::{AttachThreadInput, GetCurrentThreadId},
    UI::{
        Input::KeyboardAndMouse::{
            SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VK_CONTROL,
            VK_V,
        },
        WindowsAndMessaging::{
            GetForegroundWindow, GetGUIThreadInfo, GetWindowLongPtrW, GetWindowRect,
            GetWindowThreadProcessId, IsWindow, SetForegroundWindow, SetWindowLongPtrW,
            SetWindowPos, GUITHREADINFO, GWLP_HWNDPARENT, GWL_EXSTYLE, HWND_TOPMOST,
            SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, WS_EX_APPWINDOW,
            WS_EX_TOOLWINDOW,
        },
    },
};

#[cfg(target_os = "windows")]
pub type ForegroundWindow = HWND;
#[cfg(not(target_os = "windows"))]
pub type ForegroundWindow = ();

pub enum PasteTarget {
    Foreground(ForegroundWindow),
}

pub fn open_panel_for_target(
    app: &MainWindow,
    paste_target: &std::cell::Cell<Option<ForegroundWindow>>,
) {
    let target = foreground_window();
    let anchor = panel_anchor_position(target);
    paste_target.set(target);
    show_panel_at_anchor(app, anchor);
}

fn show_panel_at_anchor(app: &MainWindow, anchor: Option<(i32, i32)>) {
    prepare_window(app);

    let (x, y) = anchor.unwrap_or((160, 120));
    let (x, y) = clamp_panel_position(app, x + 12, y + 12);
    app.window().set_position(PhysicalPosition::new(x, y));
    let _ = app.show();
    app.invoke_panel_opened();
    bring_panel_to_front(app);
}

fn panel_anchor_position(target: Option<ForegroundWindow>) -> Option<(i32, i32)> {
    input_caret_position(target).or_else(|| foreground_window_anchor(target))
}

#[cfg(target_os = "windows")]
fn foreground_window_anchor(target: Option<HWND>) -> Option<(i32, i32)> {
    unsafe {
        let hwnd = target?;
        if hwnd.is_null() {
            return None;
        }

        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect) == 0 {
            return None;
        }

        Some((rect.left + 24, rect.top + 72))
    }
}

#[cfg(not(target_os = "windows"))]
fn foreground_window_anchor(_target: Option<ForegroundWindow>) -> Option<(i32, i32)> {
    None
}

#[cfg(target_os = "windows")]
fn input_caret_position(target: Option<HWND>) -> Option<(i32, i32)> {
    unsafe {
        let foreground = target?;
        if foreground.is_null() {
            return None;
        }

        let thread_id = GetWindowThreadProcessId(foreground, std::ptr::null_mut());
        if thread_id == 0 {
            return None;
        }

        let current_thread_id = GetCurrentThreadId();
        let attached = current_thread_id != thread_id
            && AttachThreadInput(current_thread_id, thread_id, 1) != 0;
        let result = read_thread_caret_position(thread_id);
        if attached {
            AttachThreadInput(current_thread_id, thread_id, 0);
        }
        result
    }
}

#[cfg(target_os = "windows")]
unsafe fn read_thread_caret_position(thread_id: u32) -> Option<(i32, i32)> {
    let mut info = GUITHREADINFO {
        cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
        flags: 0,
        hwndActive: std::ptr::null_mut(),
        hwndFocus: std::ptr::null_mut(),
        hwndCapture: std::ptr::null_mut(),
        hwndMenuOwner: std::ptr::null_mut(),
        hwndMoveSize: std::ptr::null_mut(),
        hwndCaret: std::ptr::null_mut(),
        rcCaret: RECT::default(),
    };

    if unsafe { GetGUIThreadInfo(thread_id, &mut info) } == 0 || info.hwndCaret.is_null() {
        return None;
    }

    let mut point = POINT {
        x: info.rcCaret.left,
        y: info.rcCaret.bottom,
    };
    if unsafe { ClientToScreen(info.hwndCaret, &mut point) } == 0 {
        return None;
    }

    Some((point.x, point.y))
}

#[cfg(not(target_os = "windows"))]
fn input_caret_position(_target: Option<ForegroundWindow>) -> Option<(i32, i32)> {
    None
}

fn clamp_panel_position(app: &MainWindow, x: i32, y: i32) -> (i32, i32) {
    let size = app.window().size();
    let width = size.width as i32;
    let height = size.height as i32;
    let area = work_area_near(x, y);

    let max_x = area.right - width;
    let max_y = area.bottom - height;
    (x.clamp(area.left, max_x), y.clamp(area.top, max_y))
}

#[derive(Clone, Copy)]
struct WorkArea {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[cfg(target_os = "windows")]
fn work_area_near(x: i32, y: i32) -> WorkArea {
    unsafe {
        let monitor = MonitorFromPoint(
            windows_sys::Win32::Foundation::POINT { x, y },
            MONITOR_DEFAULTTONEAREST,
        );
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            rcMonitor: RECT::default(),
            rcWork: RECT::default(),
            dwFlags: 0,
        };

        if GetMonitorInfoW(monitor, &mut info) != 0 {
            return WorkArea {
                left: info.rcWork.left,
                top: info.rcWork.top,
                right: info.rcWork.right,
                bottom: info.rcWork.bottom,
            };
        }
    }

    default_work_area()
}

#[cfg(not(target_os = "windows"))]
fn work_area_near(_x: i32, _y: i32) -> WorkArea {
    default_work_area()
}

fn default_work_area() -> WorkArea {
    WorkArea {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    }
}

fn prepare_window(app: &MainWindow) {
    hide_taskbar_icon(app);
}

#[cfg(target_os = "windows")]
fn window_hwnd(app: &MainWindow) -> Option<HWND> {
    let window_handle = app.window().window_handle();
    let handle = window_handle.window_handle().ok()?;
    match handle.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as HWND),
        _ => None,
    }
}

#[cfg(not(target_os = "windows"))]
fn window_hwnd(_app: &MainWindow) -> Option<()> {
    None
}

#[cfg(target_os = "windows")]
fn hide_taskbar_icon(app: &MainWindow) {
    if let Some(hwnd) = window_hwnd(app) {
        unsafe {
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(
                hwnd,
                GWL_EXSTYLE,
                (style & !(WS_EX_APPWINDOW as isize)) | WS_EX_TOOLWINDOW as isize,
            );
            SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, 0);
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn hide_taskbar_icon(_app: &MainWindow) {}

#[cfg(target_os = "windows")]
pub fn is_app_foreground(app: &MainWindow) -> bool {
    let Some(hwnd) = window_hwnd(app) else {
        return false;
    };
    unsafe { IsWindow(hwnd) != 0 && GetForegroundWindow() == hwnd }
}

#[cfg(not(target_os = "windows"))]
pub fn is_app_foreground(_app: &MainWindow) -> bool {
    true
}

#[cfg(target_os = "windows")]
fn bring_panel_to_front(app: &MainWindow) {
    if let Some(hwnd) = window_hwnd(app) {
        unsafe {
            SetWindowPos(
                hwnd,
                HWND_TOPMOST,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
            );
            SetForegroundWindow(hwnd);
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn bring_panel_to_front(_app: &MainWindow) {}

#[cfg(target_os = "windows")]
pub fn foreground_window() -> Option<HWND> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.is_null() {
            None
        } else {
            Some(hwnd)
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub fn foreground_window() -> Option<()> {
    None
}

#[cfg(target_os = "windows")]
pub fn paste_to_target(target: PasteTarget) {
    let hwnd = match target {
        PasteTarget::Foreground(hwnd) => hwnd,
    };
    let hwnd_usize = hwnd as usize;
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(120));
        let hwnd = hwnd_usize as HWND;
        unsafe {
            SetForegroundWindow(hwnd);
        }
        thread::sleep(Duration::from_millis(80));
        send_ctrl_v();
    });
}

#[cfg(not(target_os = "windows"))]
pub fn paste_to_target(_target: PasteTarget) {}

#[cfg(target_os = "windows")]
fn send_ctrl_v() {
    unsafe {
        let inputs = [
            keyboard_input(VK_CONTROL, 0),
            keyboard_input(VK_V, 0),
            keyboard_input(VK_V, KEYEVENTF_KEYUP),
            keyboard_input(VK_CONTROL, KEYEVENTF_KEYUP),
        ];
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            std::mem::size_of::<INPUT>() as i32,
        );
    }
}

#[cfg(target_os = "windows")]
fn keyboard_input(vk: u16, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}
