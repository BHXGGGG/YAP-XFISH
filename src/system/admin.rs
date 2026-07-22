//! 管理员权限检测与提权（Windows UAC）。
//!
//! - `is_elevated`：通过进程令牌的 `TokenElevation` 判断当前是否以管理员身份运行。
//! - `elevate_and_restart`：以 `runas` 动词通过 `ShellExecuteExW` 重新启动自身（触发 UAC）。
//!   提权前会让出单实例互斥锁，使新实例成为首个实例；若提权失败/被取消则重新占用。
use std::ptr;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};

/// 当前进程是否以管理员（已提权）身份运行。
pub fn is_elevated() -> bool {
    unsafe {
        let mut token: HANDLE = 0;
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut ret_len: u32 = 0;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            &mut elevation as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret_len,
        ) != 0;
        let _ = CloseHandle(token);
        ok && elevation.TokenIsElevated != 0
    }
}

fn to_wide(s: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// 以管理员身份重新启动自身（触发 UAC）。成功返回 true；失败或被用户取消返回 false。
/// 调用方通常应在返回 true 后退出当前（非提权）实例，由提权后的新实例接管。
pub fn elevate_and_restart() -> bool {
    // 让出单实例互斥锁，使提权后的新实例可成为首个实例。
    crate::system::single_instance::release_single_instance();

    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => {
            crate::system::single_instance::reacquire_single_instance();
            return false;
        }
    };
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg_str = args.join(" ");

    let exe_w = to_wide(exe.as_os_str());
    let arg_w = to_wide(std::ffi::OsStr::new(&arg_str));
    let verb_w = to_wide(std::ffi::OsStr::new("runas"));
    let dir_w = to_wide(
        exe.parent()
            .map(|p| p.as_os_str())
            .unwrap_or_else(|| std::ffi::OsStr::new("")),
    );

    let mut info: SHELLEXECUTEINFOW = unsafe { std::mem::zeroed() };
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = verb_w.as_ptr();
    info.lpFile = exe_w.as_ptr();
    info.lpParameters = if arg_w.len() > 1 {
        arg_w.as_ptr()
    } else {
        ptr::null()
    };
    info.lpDirectory = dir_w.as_ptr();
    info.nShow = 1; // SW_SHOWNORMAL

    let ok = unsafe { ShellExecuteExW(&mut info) != 0 };
    if ok {
        if info.hProcess != 0 {
            unsafe { let _ = CloseHandle(info.hProcess); }
        }
        true
    } else {
        // 提权失败/被取消：重新占用单实例互斥锁，避免重复实例。
        crate::system::single_instance::reacquire_single_instance();
        false
    }
}
