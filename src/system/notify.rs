//! Windows 系统通知（Toast，进入通知中心）。
//!
//! 通过隐藏的 PowerShell 调用 WinRT ToastNotificationManager，不引入额外依赖。
//! 标题/正文经环境变量传入并在脚本内做 XML 转义，避免拼接注入。
//! best-effort：失败只记日志，不影响调用方。

#[cfg(windows)]
pub fn toast(title: &str, body: &str) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // PowerShell 自带的 AppUserModelID，无需注册即可显示在通知中心。
    const SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null
[Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] | Out-Null
$t = [System.Security.SecurityElement]::Escape($env:YAP_TOAST_TITLE)
$b = [System.Security.SecurityElement]::Escape($env:YAP_TOAST_BODY)
$xml = New-Object Windows.Data.Xml.Dom.XmlDocument
$xml.LoadXml("<toast><visual><binding template='ToastGeneric'><text>$t</text><text>$b</text></binding></visual></toast>")
$app = '{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe'
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($app).Show([Windows.UI.Notifications.ToastNotification]::new($xml))
"#;
    let res = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", SCRIPT])
        .env("YAP_TOAST_TITLE", title)
        .env("YAP_TOAST_BODY", body)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
    if let Err(e) = res {
        crate::system::log_file::append("notify", "warn", &format!("系统通知发送失败: {e}"));
    }
}

#[cfg(not(windows))]
pub fn toast(_title: &str, _body: &str) {}
