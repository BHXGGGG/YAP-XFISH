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

/// 启动时检测到 TUN 已启用但未提权：弹窗询问用户是否以管理员身份重启。
///
/// - 用户点"是"且提权成功 → 当前进程 `exit(0)`，让新提权实例接管。
/// - 用户点"否"或提权失败/取消 → 把 `enable_tun` 写回 `false` 持久化，下次启动不再触发。
pub fn prompt_tun_elevate_or_disable(config_path: &std::path::Path) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_YESNO, MB_ICONWARNING, IDYES,
    };

    let msg = "TUN 模式需要管理员权限才能创建虚拟网卡。\n\n当前进程未以管理员身份运行，继续启动会导致 sing-box 启动失败（错误 59 / Access is denied）。\n\n是否以管理员身份重启？";
    let title = "YAP-XFISH — 需要管理员权限";
    let msg_wide: Vec<u16> = msg.encode_utf16().chain(std::iter::once(0)).collect();
    let title_wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();

    let ret = unsafe {
        MessageBoxW(
            0,
            msg_wide.as_ptr(),
            title_wide.as_ptr(),
            MB_YESNO | MB_ICONWARNING,
        )
    };

    if ret == IDYES {
        if elevate_and_restart() {
            std::process::exit(0);
        }
        disable_tun_persisted(config_path);
    } else {
        disable_tun_persisted(config_path);
    }
}

fn disable_tun_persisted(config_path: &std::path::Path) {
    let text = match std::fs::read_to_string(config_path) {
        Ok(t) => t,
        Err(_) => return,
    };
    let mut cfg: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return,
    };
    if cfg.get("enable_tun").and_then(|v| v.as_bool()).unwrap_or(false) {
        cfg["enable_tun"] = serde_json::Value::Bool(false);
        if let Ok(s) = serde_json::to_string_pretty(&cfg) {
            let _ = std::fs::write(config_path, s);
        }
    }
}
