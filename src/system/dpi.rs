//! 进程 DPI 感知：避免托盘原生菜单在高分屏上被系统位图拉伸导致字体发糊。
//!
//! 必须在创建任何 HWND / 托盘 / 菜单之前调用。失败时静默降级（旧系统无此 API）。

/// 启用 Per-Monitor DPI Awareness V2。
///
/// - 有清单 + 运行时 API 双重设置，兼容不同启动路径。
/// - 托盘菜单走系统 HMENU，进程 DPI 感知后由系统用 ClearType 原生渲染，字体清晰。
pub fn enable() {
    #[cfg(windows)]
    unsafe {
        // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 = (DPI_AWARENESS_CONTEXT)-4
        // 优先 V2；失败再退到 V1 / System aware。
        type SetProcessDpiAwarenessContextFn =
            unsafe extern "system" fn(value: isize) -> i32;
        type SetProcessDpiAwarenessFn = unsafe extern "system" fn(value: u32) -> i32;

        // user32!SetProcessDpiAwarenessContext (Win10 1703+)
        let user32 = windows_sys::Win32::System::LibraryLoader::LoadLibraryW(
            windows_sys::core::w!("user32.dll"),
        );
        if user32 != 0 {
            let proc = windows_sys::Win32::System::LibraryLoader::GetProcAddress(
                user32,
                windows_sys::s!("SetProcessDpiAwarenessContext"),
            );
            if let Some(f) = proc {
                let f: SetProcessDpiAwarenessContextFn = std::mem::transmute(f);
                // -4 = PER_MONITOR_AWARE_V2
                if f(-4) != 0 {
                    return;
                }
                // -3 = PER_MONITOR_AWARE
                if f(-3) != 0 {
                    return;
                }
            }
        }

        // shcore!SetProcessDpiAwareness (Win8.1+)
        // PROCESS_PER_MONITOR_DPI_AWARE = 2
        let shcore = windows_sys::Win32::System::LibraryLoader::LoadLibraryW(
            windows_sys::core::w!("shcore.dll"),
        );
        if shcore != 0 {
            let proc = windows_sys::Win32::System::LibraryLoader::GetProcAddress(
                shcore,
                windows_sys::s!("SetProcessDpiAwareness"),
            );
            if let Some(f) = proc {
                let f: SetProcessDpiAwarenessFn = std::mem::transmute(f);
                if f(2) == 0 {
                    return;
                }
                // PROCESS_SYSTEM_DPI_AWARE = 1
                let _ = f(1);
            }
        }
    }
}
