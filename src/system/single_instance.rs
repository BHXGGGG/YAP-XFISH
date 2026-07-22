use std::ptr;
use std::sync::Mutex;

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, FALSE, GetLastError, HANDLE};
use windows_sys::Win32::System::Threading::CreateMutexW;

const MUTEX_NAME: &str = "ProxyRs_SingleInstance_Mutex";

/// 首个实例持有的互斥锁句柄。进程退出前独占，防止第二个实例启动。
/// 用 Mutex<Option<HANDLE>> 而非 OnceLock，便于提权重启时临时让出并重新占用。
static INSTANCE: Mutex<Option<HANDLE>> = Mutex::new(None);

fn mutex_name_wide() -> Vec<u16> {
    MUTEX_NAME
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect()
}

/// 确保单实例：若已有实例在运行则返回 false（调用方应退出）；否则返回 true 并持有互斥锁。
pub fn ensure_single_instance() -> bool {
    let wide = mutex_name_wide();
    let handle: HANDLE = unsafe { CreateMutexW(ptr::null(), FALSE, wide.as_ptr()) };
    if handle == 0 {
        // 创建失败也放行（不让一个 API 错误阻断启动），但无法保证单实例。
        return true;
    }
    let err = unsafe { GetLastError() };
    if err == ERROR_ALREADY_EXISTS {
        unsafe { let _ = CloseHandle(handle); }
        false
    } else {
        // 首个实例：持有句柄直至进程退出。
        *INSTANCE.lock().unwrap() = Some(handle);
        true
    }
}

/// 释放单实例互斥锁（用于提权重启前让出所有权，使新实例成为首个实例）。
pub fn release_single_instance() {
    if let Some(h) = INSTANCE.lock().unwrap().take() {
        unsafe { let _ = CloseHandle(h); }
    }
}

/// 重新占用单实例互斥锁（提权失败/被取消时调用，避免重复实例）。
pub fn reacquire_single_instance() {
    let mut g = INSTANCE.lock().unwrap();
    if g.is_some() {
        return;
    }
    let wide = mutex_name_wide();
    let handle: HANDLE = unsafe { CreateMutexW(ptr::null(), FALSE, wide.as_ptr()) };
    if handle == 0 {
        return;
    }
    let err = unsafe { GetLastError() };
    if err == ERROR_ALREADY_EXISTS {
        // 已有其它实例占用，不持有（本进程将作为重复实例处理）。
        unsafe { let _ = CloseHandle(handle); }
    } else {
        *g = Some(handle);
    }
}
