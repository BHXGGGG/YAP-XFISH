//! 非阻塞的 Windows 错误弹窗。

/// 在独立线程弹出错误 MessageBox（置顶），不阻塞调用方。
pub fn show_error(title: &str, msg: &str) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        MessageBoxW, MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MB_TOPMOST,
    };
    let title: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
    let msg: Vec<u16> = msg.encode_utf16().chain(std::iter::once(0)).collect();
    std::thread::spawn(move || unsafe {
        MessageBoxW(
            0,
            msg.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR | MB_TOPMOST | MB_SETFOREGROUND,
        );
    });
}
