//! 开机自启：通过注册表 HKCU\Software\Microsoft\Windows\CurrentVersion\Run 实现

#[cfg(target_os = "windows")]
use windows_sys::Win32::System::Registry::{
    HKEY, RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
    HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_SZ,
};

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const APP_NAME: &str = "CCopy";

/// 获取当前可执行文件路径
fn exe_path() -> Option<String> {
    std::env::current_exe()
        .ok()
        .map(|p| p.to_string_lossy().to_string())
}

#[cfg(target_os = "windows")]
fn to_wide(s: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// 打开 Run 注册表键，失败返回 None
#[cfg(target_os = "windows")]
fn open_run_key(access: u32) -> Option<HKEY> {
    let sub_key = to_wide(RUN_KEY);
    let mut handle: HKEY = core::ptr::null_mut();
    let status =
        unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, sub_key.as_ptr(), 0, access, &mut handle) };
    if status != 0 {
        return None;
    }
    Some(handle)
}

/// 是否已设置开机自启
#[cfg(target_os = "windows")]
pub fn is_enabled() -> bool {
    let Some(handle) = open_run_key(KEY_READ) else {
        return false;
    };
    let name = to_wide(APP_NAME);
    let mut len: u32 = 0;
    let status = unsafe {
        RegQueryValueExW(
            handle,
            name.as_ptr(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut len,
        )
    };
    unsafe { RegCloseKey(handle) };
    status == 0
}

#[cfg(not(target_os = "windows"))]
pub fn is_enabled() -> bool {
    false
}

/// 设置开机自启
#[cfg(target_os = "windows")]
pub fn enable() -> bool {
    let Some(path) = exe_path() else {
        return false;
    };
    let Some(handle) = open_run_key(KEY_WRITE) else {
        return false;
    };
    let name = to_wide(APP_NAME);
    let value = to_wide(&format!("\"{path}\""));
    // value 是 UTF-16，字节数 = 字符数 * 2，末尾 null 也计入
    let value_bytes = (value.len() * 2) as u32;
    let status = unsafe {
        RegSetValueExW(
            handle,
            name.as_ptr(),
            0,
            REG_SZ,
            value.as_ptr() as *const u8,
            value_bytes,
        )
    };
    unsafe { RegCloseKey(handle) };
    status == 0
}

#[cfg(not(target_os = "windows"))]
pub fn enable() -> bool {
    false
}

/// 取消开机自启
#[cfg(target_os = "windows")]
pub fn disable() -> bool {
    let Some(handle) = open_run_key(KEY_WRITE) else {
        return false;
    };
    let name = to_wide(APP_NAME);
    let status = unsafe { RegDeleteValueW(handle, name.as_ptr()) };
    unsafe { RegCloseKey(handle) };
    status == 0
}

#[cfg(not(target_os = "windows"))]
pub fn disable() -> bool {
    false
}
