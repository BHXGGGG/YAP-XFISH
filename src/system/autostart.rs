//! 开机启动：写 HKCU\Software\Microsoft\Windows\CurrentVersion\Run。
//! 使用 HKCU 避免触碰 HKLM（无需管理员权限）。
use winreg::enums::HKEY_CURRENT_USER;
use winreg::RegKey;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const VALUE_NAME: &str = "ProxyRs";

/// 设置/取消开机启动。enabled=true 写入当前 exe 路径（含空格自动加引号）；false 删除该值。
pub fn set_autostart(enabled: bool) -> anyhow::Result<()> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu.create_subkey(RUN_KEY)?;
    if enabled {
        let exe = std::env::current_exe()?;
        let path = exe.to_string_lossy().to_string();
        let quoted = if path.contains(' ') {
            format!("\"{}\"", path)
        } else {
            path
        };
        key.set_value(VALUE_NAME, &quoted)?;
    } else {
        // 删除不存在的值会报错，忽略即可。
        let _ = key.delete_value(VALUE_NAME);
    }
    Ok(())
}

/// 当前是否已配置开机启动。
pub fn is_autostart_enabled() -> bool {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    match hkcu.open_subkey(RUN_KEY) {
        Ok(key) => key.get_value::<String, _>(VALUE_NAME).is_ok(),
        Err(_) => false,
    }
}

/// 按当前配置应用开机启动（仅在状态与注册表不一致时写入，避免无谓写盘）。
pub fn apply_autostart(enabled: bool) {
    if is_autostart_enabled() != enabled {
        let _ = set_autostart(enabled);
    }
}
