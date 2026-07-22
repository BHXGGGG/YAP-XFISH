//! Windows 系统代理（Internet Settings / WinINET）。
//!
//! 写入 `HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings`：
//! - ProxyEnable = 1/0
//! - ProxyServer = `127.0.0.1:<port>`
//! - ProxyOverride = 本地绕过列表
//!
//! 然后调用 `InternetSetOptionW` 广播设置变更，让正在运行的浏览器尽快感知。

use anyhow::{Context, Result};
use winreg::enums::*;
use winreg::RegKey;

const KEY_PATH: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";
const DEFAULT_OVERRIDE: &str =
    "localhost;127.*;10.*;172.16.*;172.17.*;172.18.*;172.19.*;172.20.*;172.21.*;172.22.*;172.23.*;172.24.*;172.25.*;172.26.*;172.27.*;172.28.*;172.29.*;172.30.*;172.31.*;192.168.*;<local>";

#[cfg(windows)]
#[link(name = "wininet")]
extern "system" {
    fn InternetSetOptionW(
        h_internet: *mut core::ffi::c_void,
        option: u32,
        buffer: *mut core::ffi::c_void,
        buffer_length: u32,
    ) -> i32;
}

const INTERNET_OPTION_SETTINGS_CHANGED: u32 = 39;
const INTERNET_OPTION_REFRESH: u32 = 37;

/// 启用系统代理，指向本机 mixed 入站端口。
pub fn enable(proxy_port: u16) -> Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu
        .create_subkey(KEY_PATH)
        .context("打开 Internet Settings 注册表键失败")?;
    let server = format!("127.0.0.1:{proxy_port}");
    key.set_value("ProxyEnable", &1u32)
        .context("写入 ProxyEnable 失败")?;
    key.set_value("ProxyServer", &server)
        .context("写入 ProxyServer 失败")?;
    // 若用户已有自定义 Bypass 列表则保留；否则写默认本地绕过
    let existing: Result<String, _> = key.get_value("ProxyOverride");
    if existing.map(|s| s.trim().is_empty()).unwrap_or(true) {
        let _ = key.set_value("ProxyOverride", &DEFAULT_OVERRIDE);
    }
    notify_system();
    Ok(())
}

/// 关闭系统代理（ProxyEnable=0），不删除 ProxyServer 以便下次快速恢复。
pub fn disable() -> Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu
        .create_subkey(KEY_PATH)
        .context("打开 Internet Settings 注册表键失败")?;
    key.set_value("ProxyEnable", &0u32)
        .context("写入 ProxyEnable=0 失败")?;
    notify_system();
    Ok(())
}

/// 读取当前系统是否启用了代理（仅看 ProxyEnable）。
pub fn is_enabled() -> bool {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let Ok(key) = hkcu.open_subkey(KEY_PATH) else {
        return false;
    };
    let v: u32 = key.get_value("ProxyEnable").unwrap_or(0);
    v != 0
}

fn notify_system() {
    #[cfg(windows)]
    unsafe {
        let _ = InternetSetOptionW(
            std::ptr::null_mut(),
            INTERNET_OPTION_SETTINGS_CHANGED,
            std::ptr::null_mut(),
            0,
        );
        let _ = InternetSetOptionW(
            std::ptr::null_mut(),
            INTERNET_OPTION_REFRESH,
            std::ptr::null_mut(),
            0,
        );
    }
}
